// RGBable: Windows desktop app for Lian Li hardware (unofficial).
// No console window in release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod effects;
mod engine;
mod fans;
mod lights;
mod oled_library;
mod openrgb;
mod openrgb_host;
mod palette;
mod render;
mod script;
mod updater;

use anyhow::Context;
use effects::Effect;
use engine::{DeviceView, Engine, OledView, Telemetry};
use lights::{GroupView, LightView, SetupEntry};
use openrgb::{OpenRgb, OrgbView};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State};
use tracing::{error, info};
use updater::{Channel, UpdateView, Updater};

struct AppState {
    engine: Arc<Engine>,
    openrgb: Arc<OpenRgb>,
    updater: Arc<Updater>,
}

#[derive(Serialize)]
struct Overview {
    status: String,
    lconnect_running: bool,
    devices: Vec<DeviceView>,
    telemetry: Telemetry,
    oled: OledView,
    openrgb: OrgbView,
    autostart: bool,
    data_dir: String,
    lights: Vec<LightView>,
    groups: Vec<GroupView>,
    setup_done: bool,
    update: UpdateView,
    update_channel: Channel,
    manage_openrgb: bool,
    color_presets: Vec<String>,
    oled_match: Option<u8>,
    oled_match_status: String,
    warmth: i8,
    clean_transitions: bool,
    fan_profiles: Vec<String>,
    fan_profile: usize,
    all_fans: fans::FanMode,
    pump: fans::PumpMode,
    pump_range: (u16, u16),
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Runs off the UI thread: it takes locks the lighting threads also hold.
#[tauri::command(async)]
fn overview(state: State<'_, AppState>) -> Result<Overview, String> {
    let config = state.engine.config();
    let status = state.engine.status();
    Ok(Overview {
        lconnect_running: status != "Ready" && engine::lconnect_running(),
        status,
        devices: state.engine.devices(),
        telemetry: state.engine.telemetry(),
        oled: state.engine.oled(),
        openrgb: state.openrgb.view(),
        autostart: autostart_cached(),
        data_dir: data_dir().to_string_lossy().into_owned(),
        lights: lights::lights(&state.engine, &state.openrgb),
        groups: lights::groups(&state.engine),
        setup_done: config.setup_done,
        update: state.updater.view(),
        update_channel: config.update_channel,
        manage_openrgb: config.manage_openrgb,
        color_presets: config.color_presets.clone(),
        oled_match: config.oled_match,
        oled_match_status: lights::OLED_MATCH_STATUS.lock().clone(),
        warmth: config.warmth,
        clean_transitions: !config.instant_changes,
        fan_profiles: config.fan_profiles.iter().map(|p| p.name.clone()).collect(),
        fan_profile: config.fan_profile,
        all_fans: config
            .active_fan_profile()
            .map(|p| p.all_fans.clone())
            .unwrap_or_default(),
        pump: config
            .active_fan_profile()
            .map(|p| p.pump)
            .unwrap_or_default(),
        pump_range: lianli_devices::winusb::hs2_oled_led::Hs2OledLedController::PUMP_RPM_RANGE,
    })
}

static AUTOSTART: std::sync::OnceLock<std::sync::atomic::AtomicBool> = std::sync::OnceLock::new();

/// The registry is read once (it spawns `reg`), then tracked as the user changes it.
fn autostart_cached() -> bool {
    AUTOSTART
        .get_or_init(|| std::sync::atomic::AtomicBool::new(autostart_enabled()))
        .load(std::sync::atomic::Ordering::Acquire)
}

fn chain(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command(async)]
fn set_sequence(
    state: State<'_, AppState>,
    entries: Vec<lights::SequenceEntry>,
) -> Result<(), String> {
    lights::set_sequence(&state.engine, &state.openrgb, &entries).map_err(chain)
}

#[tauri::command(async)]
fn set_across(state: State<'_, AppState>, id: String, across: bool) -> Result<(), String> {
    lights::set_across(&state.engine, &state.openrgb, &id, across).map_err(chain)
}

#[tauri::command(async)]
fn set_reverse(state: State<'_, AppState>, id: String, reverse: bool) -> Result<(), String> {
    lights::set_reverse(&state.engine, &state.openrgb, &id, reverse).map_err(chain)
}

#[tauri::command(async)]
fn set_speed(state: State<'_, AppState>, id: String, speed: u8) -> Result<(), String> {
    lights::set_speed(&state.engine, &state.openrgb, &id, speed).map_err(chain)
}

#[tauri::command(async)]
fn set_delay(state: State<'_, AppState>, id: String, delay_ms: u32) -> Result<(), String> {
    lights::set_delay(&state.engine, &state.openrgb, &id, delay_ms).map_err(chain)
}

#[tauri::command(async)]
fn set_color(state: State<'_, AppState>, id: String, color: [u8; 3]) -> Result<(), String> {
    lights::set_color(&state.engine, &state.openrgb, &id, color).map_err(chain)
}

#[tauri::command]
async fn set_all(
    state: State<'_, AppState>,
    effect: Effect,
    include_openrgb: bool,
) -> Result<(), String> {
    let engine = state.engine.clone();
    let openrgb = state.openrgb.clone();
    tauri::async_runtime::spawn_blocking(move || {
        lights::set_all(&engine, &openrgb, effect, include_openrgb)
    })
    .await
    .map_err(err)?
    .map_err(chain)
}

#[tauri::command(async)]
fn set_effect(state: State<'_, AppState>, id: String, effect: Effect) -> Result<(), String> {
    lights::set_effect(&state.engine, &state.openrgb, &id, effect).map_err(chain)
}

#[tauri::command(async)]
fn set_group_effect(
    state: State<'_, AppState>,
    group: String,
    effect: Effect,
) -> Result<(), String> {
    lights::set_group_effect(&state.engine, &state.openrgb, &group, effect).map_err(chain)
}

fn default_name(state: &AppState, id: &str) -> Option<String> {
    lights::lights(&state.engine, &state.openrgb)
        .into_iter()
        .find(|l| l.id == id)
        .map(|l| l.default_name)
}

#[tauri::command(async)]
fn rename(state: State<'_, AppState>, id: String, name: String) {
    let default = default_name(&state, &id);
    state
        .engine
        .update_config(|c| lights::set_name(c, &id, &name, default.as_deref()));
}

#[tauri::command(async)]
fn identify(state: State<'_, AppState>, id: String) -> Result<(), String> {
    lights::identify(&state.engine, &state.openrgb, &id).map_err(chain)
}

#[tauri::command(async)]
fn set_sync(state: State<'_, AppState>, id: String, sync: bool) {
    state
        .engine
        .update_config(|c| lights::set_sync(c, &id, sync));
}

#[tauri::command(async)]
fn create_group(state: State<'_, AppState>, name: String) -> String {
    state
        .engine
        .update_config(|c| lights::create_group(c, &name))
}

#[tauri::command(async)]
fn rename_group(state: State<'_, AppState>, group: String, name: String) {
    state
        .engine
        .update_config(|c| lights::rename_group(c, &group, &name));
}

#[tauri::command(async)]
fn delete_group(state: State<'_, AppState>, group: String) {
    state
        .engine
        .update_config(|c| lights::delete_group(c, &group));
}

#[tauri::command(async)]
fn move_to_group(
    state: State<'_, AppState>,
    id: String,
    group: Option<String>,
) -> Result<(), String> {
    state
        .engine
        .update_config(|c| lights::move_to_group(c, &id, group.as_deref()));
    match group {
        Some(group) => {
            lights::adopt_group_lighting(&state.engine, &state.openrgb, &id, &group).map_err(chain)
        }
        None => Ok(()),
    }
}

#[tauri::command(async)]
fn set_group_color(
    state: State<'_, AppState>,
    group: String,
    color: [u8; 3],
) -> Result<(), String> {
    lights::set_group_effect(
        &state.engine,
        &state.openrgb,
        &group,
        lights::static_effect(color),
    )
    .map_err(chain)
}

#[tauri::command(async)]
fn finish_setup(state: State<'_, AppState>, entries: Vec<SetupEntry>) {
    let defaults: Vec<(String, String)> = lights::lights(&state.engine, &state.openrgb)
        .into_iter()
        .map(|l| (l.id, l.default_name))
        .collect();
    state
        .engine
        .update_config(|c| lights::apply_setup(c, &entries, &defaults));
}

#[tauri::command]
async fn openrgb_connect(state: State<'_, AppState>) -> Result<(), String> {
    let openrgb = state.openrgb.clone();
    let engine = state.engine.clone();
    let preferred = engine.config().openrgb_port;
    tauri::async_runtime::spawn_blocking(move || {
        openrgb.connect(preferred)?;
        lights::after_openrgb_connect(&engine, &openrgb);
        anyhow::Ok(())
    })
    .await
    .map_err(err)?
    .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
async fn pick_media(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .file()
        .set_title("Choose what the OLED should loop")
        .add_filter(
            "Video, GIF or image",
            &[
                "mp4", "mkv", "mov", "webm", "avi", "gif", "png", "jpg", "jpeg", "bmp", "webp",
            ],
        )
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command(async)]
fn set_oled(
    app: AppHandle,
    state: State<'_, AppState>,
    file: Option<String>,
    fit: String,
    fps: u32,
) -> Result<(), String> {
    let new_file = file.is_some();
    if let Some(file) = &file {
        allow_preview(&app, std::path::Path::new(file));
    }
    state
        .engine
        .set_oled(file.map(PathBuf::from), fit, fps)
        .map_err(err)?;
    if new_file {
        match_oled_in_background(&state.engine, &state.openrgb);
    }
    Ok(())
}

fn match_oled_in_background(engine: &Arc<Engine>, openrgb: &Arc<OpenRgb>) {
    let Some(count) = engine.config().oled_match else {
        return;
    };
    let (engine, openrgb) = (engine.clone(), openrgb.clone());
    std::thread::spawn(move || {
        if let Err(e) = lights::match_oled(&engine, &openrgb, count) {
            tracing::warn!("matching the lighting to the OLED: {e:#}");
        }
    });
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Colors from `file`, or from what's on the OLED, without applying them.
#[tauri::command]
async fn oled_palette(
    state: State<'_, AppState>,
    file: Option<String>,
    count: u8,
) -> Result<Vec<String>, String> {
    let file = match file {
        Some(file) => PathBuf::from(file),
        None => state
            .engine
            .config()
            .oled
            .map(|o| o.file)
            .ok_or("Nothing is playing on the OLED")?,
    };
    tauri::async_runtime::spawn_blocking(move || palette::from_media(&file, usize::from(count)))
        .await
        .map_err(err)?
        .map(|colors| colors.into_iter().map(hex).collect())
        .map_err(chain)
}

#[tauri::command]
async fn match_oled_now(state: State<'_, AppState>, count: u8) -> Result<(), String> {
    let (engine, openrgb) = (state.engine.clone(), state.openrgb.clone());
    tauri::async_runtime::spawn_blocking(move || {
        lights::match_oled(&engine, &openrgb, count.clamp(1, 6))
    })
    .await
    .map_err(err)?
    .map(|_| ())
    .map_err(chain)
}

#[tauri::command]
async fn set_oled_match(state: State<'_, AppState>, count: Option<u8>) -> Result<(), String> {
    let count = count.map(|c| c.clamp(1, 6));
    state.engine.update_config(|c| c.oled_match = count);
    let Some(count) = count else {
        return Ok(());
    };
    if state.engine.config().oled.is_none() {
        return Ok(());
    }
    let engine = state.engine.clone();
    let openrgb = state.openrgb.clone();
    tauri::async_runtime::spawn_blocking(move || lights::match_oled(&engine, &openrgb, count))
        .await
        .map_err(err)?
        .map(|_| ())
        .map_err(chain)
}

#[tauri::command(async)]
fn set_color_presets(state: State<'_, AppState>, presets: Vec<String>) {
    let presets: Vec<String> = presets
        .into_iter()
        .filter(|p| {
            p.len() == 7 && p.starts_with('#') && p[1..].chars().all(|c| c.is_ascii_hexdigit())
        })
        .take(12)
        .collect();
    state.engine.update_config(|c| c.color_presets = presets);
}

/// Lets the OLED page show the chosen file through the asset protocol; nothing else is readable.
fn allow_preview(app: &AppHandle, file: &std::path::Path) {
    use tauri::Manager;
    if let Err(e) = app.asset_protocol_scope().allow_file(file) {
        tracing::warn!("OLED preview for {}: {e}", file.display());
    }
}

#[derive(Serialize)]
struct OledLibrary {
    folders: Vec<oled_library::LibraryFolder>,
    items: Vec<oled_library::LibraryItem>,
    linked: bool,
    making_thumbs: bool,
}

static MAKING_THUMBS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
async fn oled_library(app: AppHandle, state: State<'_, AppState>) -> Result<OledLibrary, String> {
    use std::sync::atomic::Ordering;
    use tauri::Manager;
    let config = state.engine.config();
    let linked = !config.oled_hide_oledable;
    let (folders, items) = tauri::async_runtime::spawn_blocking(move || {
        let folders = oled_library::folders(linked, &config.oled_folders);
        let items = oled_library::list(&folders);
        (folders, items)
    })
    .await
    .map_err(err)?;
    let scope = app.asset_protocol_scope();
    for folder in folders.iter().filter(|f| f.exists) {
        let _ = scope.allow_directory(&folder.path, false);
    }
    let _ = scope.allow_directory(oled_library::thumbs_folder(), false);
    let missing = items.iter().any(|i| i.thumb.is_none());
    if missing && !MAKING_THUMBS.swap(true, Ordering::AcqRel) {
        let pending = items.clone();
        std::thread::spawn(move || {
            oled_library::make_thumbnails(&pending, &oled_library::ffmpeg());
            MAKING_THUMBS.store(false, Ordering::Release);
        });
    }
    Ok(OledLibrary {
        folders,
        items,
        linked,
        making_thumbs: missing,
    })
}

#[tauri::command]
async fn add_oled_folder(app: AppHandle, state: State<'_, AppState>) -> Result<bool, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(path) = app
        .dialog()
        .file()
        .set_title("Add a folder of videos to the OLED library")
        .blocking_pick_folder()
        .and_then(|p| p.into_path().ok())
    else {
        return Ok(false);
    };
    state.engine.update_config(|c| {
        if !c.oled_folders.contains(&path) {
            c.oled_folders.push(path);
        }
    });
    Ok(true)
}

#[tauri::command(async)]
fn remove_oled_folder(state: State<'_, AppState>, path: String) {
    let path = PathBuf::from(path);
    state
        .engine
        .update_config(|c| c.oled_folders.retain(|p| p != &path));
}

#[tauri::command(async)]
fn set_oled_link(state: State<'_, AppState>, linked: bool) {
    state
        .engine
        .update_config(|c| c.oled_hide_oledable = !linked);
}

#[tauri::command(async)]
fn prepare_oled(app: AppHandle, file: String, fit: String, fps: u32) {
    let file = PathBuf::from(file);
    allow_preview(&app, &file);
    engine::prepare_oled(engine::OledConfig { file, fit, fps });
}

/// Restarts RGBable, and the OpenRGB service when RGBable runs it, so newly plugged-in
/// devices are found.
#[tauri::command]
async fn restart_app(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || restart(&app))
        .await
        .map_err(err)?
        .map_err(chain)
}

fn restart(app: &AppHandle) -> anyhow::Result<()> {
    let managed = tauri::Manager::try_state::<AppState>(app)
        .is_some_and(|s| s.engine.config().manage_openrgb);
    if managed {
        if let Err(e) = openrgb_host::restart() {
            tracing::warn!("restarting OpenRGB: {e:#}");
        }
    }
    updater::relaunch_after_exit()?;
    tracing::info!("restarting RGBable");
    app.exit(0);
    Ok(())
}

#[tauri::command(async)]
fn stop_oled(state: State<'_, AppState>) {
    state.engine.stop_oled();
}

#[tauri::command(async)]
fn set_clean_transitions(state: State<'_, AppState>, enabled: bool) {
    state.engine.update_config(|c| c.instant_changes = !enabled);
}

#[tauri::command(async)]
fn set_warmth(state: State<'_, AppState>, warmth: i8) {
    let warmth = warmth.clamp(-50, 50);
    state.engine.update_config(|c| c.warmth = warmth);
    render::WARMTH.store(warmth, std::sync::atomic::Ordering::Relaxed);
    state.engine.relight_all();
    lights::sync_openrgb_effects(&state.engine, &state.openrgb);
}

#[tauri::command(async)]
fn set_fan_mode(state: State<'_, AppState>, id: String, mode: fans::FanMode) {
    state.engine.set_fan_mode(&id, mode);
}

#[tauri::command(async)]
fn set_all_fan_modes(state: State<'_, AppState>, mode: fans::FanMode) {
    state.engine.set_all_fan_modes(mode);
}

#[tauri::command(async)]
fn set_pump_mode(state: State<'_, AppState>, mode: fans::PumpMode) {
    state.engine.set_pump_mode(mode);
}

#[tauri::command(async)]
fn select_fan_profile(state: State<'_, AppState>, index: usize) {
    state.engine.select_fan_profile(index);
}

#[tauri::command(async)]
fn add_fan_profile(state: State<'_, AppState>, name: String) {
    state.engine.add_fan_profile(&name);
}

#[tauri::command(async)]
fn rename_fan_profile(state: State<'_, AppState>, index: usize, name: String) {
    state.engine.rename_fan_profile(index, &name);
}

#[tauri::command(async)]
fn delete_fan_profile(state: State<'_, AppState>, index: usize) {
    state.engine.delete_fan_profile(index);
}

#[tauri::command(async)]
fn set_autostart(enabled: bool) -> Result<(), String> {
    set_autostart_impl(enabled).map_err(|e| format!("{e:#}"))?;
    AUTOSTART
        .get_or_init(|| std::sync::atomic::AtomicBool::new(enabled))
        .store(enabled, std::sync::atomic::Ordering::Release);
    Ok(())
}

#[derive(Serialize)]
struct OpenRgbSetup {
    managed: bool,
    #[serde(flatten)]
    host: openrgb_host::HostStatus,
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(err)?
        .map_err(chain)
}

#[tauri::command]
async fn openrgb_setup_status(state: State<'_, AppState>) -> Result<OpenRgbSetup, String> {
    let config = state.engine.config();
    let host = blocking(move || Ok(openrgb_host::status(config.openrgb_exe.as_deref()))).await?;
    Ok(OpenRgbSetup {
        managed: state.engine.config().manage_openrgb,
        host,
    })
}

#[tauri::command]
async fn install_pawnio(state: State<'_, AppState>) -> Result<(), String> {
    let engine = state.engine.clone();
    let openrgb = state.openrgb.clone();
    blocking(move || {
        openrgb_host::install_pawnio()?;
        if engine.config().manage_openrgb {
            openrgb_host::restart()?;
            connect_openrgb_in_background(engine, openrgb, false);
        }
        Ok(())
    })
    .await
}

#[tauri::command]
async fn set_openrgb_managed(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    let engine = state.engine.clone();
    let openrgb = state.openrgb.clone();
    blocking(move || {
        if enabled {
            let exe = openrgb_host::find_exe(engine.config().openrgb_exe.as_deref())
                .context("OpenRGB isn't installed; install it from openrgb.org, or use Locate if it's somewhere else")?;
            openrgb_host::create_task(&exe)?;
            engine.update_config(|c| c.manage_openrgb = true);
            openrgb_host::disable_openrgb_autostart();
            openrgb_host::start()?;
            connect_openrgb_in_background(engine, openrgb, false);
        } else {
            openrgb_host::remove_task()?;
            engine.update_config(|c| c.manage_openrgb = false);
        }
        Ok(())
    })
    .await
}

#[tauri::command]
async fn locate_openrgb(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .set_title("Find OpenRGB.exe")
        .add_filter("OpenRGB", &["exe"])
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok());
    if let Some(path) = &picked {
        let path = path.clone();
        state.engine.update_config(|c| c.openrgb_exe = Some(path));
    }
    Ok(picked.map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn resize_zone(
    state: State<'_, AppState>,
    id: String,
    zone: usize,
    size: usize,
) -> Result<(), String> {
    let engine = state.engine.clone();
    let openrgb = state.openrgb.clone();
    blocking(move || lights::resize_zone(&engine, &openrgb, &id, zone, size)).await
}

#[derive(Serialize)]
struct Preview {
    frames: Vec<Vec<[u8; 3]>>,
    interval_ms: u32,
}

#[tauri::command]
async fn preview_effect(effect: Effect) -> Result<Preview, String> {
    tauri::async_runtime::spawn_blocking(move || render::preview(effect.sanitized()))
        .await
        .map_err(err)?
        .map(|(frames, interval_ms)| Preview {
            frames,
            interval_ms,
        })
}

#[tauri::command(async)]
fn script_template() -> &'static str {
    script::TEMPLATE
}

#[tauri::command(async)]
fn custom_effects(state: State<'_, AppState>) -> Vec<Effect> {
    state.engine.config().custom_effects
}

#[tauri::command(async)]
fn save_custom_effect(state: State<'_, AppState>, effect: Effect) -> Result<(), String> {
    let effect = effect.sanitized();
    let name = effect
        .name
        .clone()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .ok_or("give the effect a name")?;
    if effect.kind == effects::EffectKind::Script {
        script::check(effect.script.as_deref().unwrap_or(""), &effect.colors)?;
    }
    state.engine.update_config(|c| {
        let effect = Effect {
            name: Some(name.clone()),
            ..effect
        };
        match c
            .custom_effects
            .iter_mut()
            .find(|e| e.name.as_deref() == Some(&name))
        {
            Some(existing) => *existing = effect,
            None => c.custom_effects.push(effect),
        }
    });
    Ok(())
}

#[tauri::command(async)]
fn delete_custom_effect(state: State<'_, AppState>, name: String) {
    state.engine.update_config(|c| {
        c.custom_effects
            .retain(|e| e.name.as_deref() != Some(&name))
    });
}

#[tauri::command]
async fn export_custom_effects(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let effects = state.engine.config().custom_effects;
    if effects.is_empty() {
        return Err("there are no saved effects to export yet".into());
    }
    let Some(path) = app
        .dialog()
        .file()
        .set_title("Export your effects")
        .set_file_name("my-effects.rgbable.json")
        .add_filter("RGBable effects", &["json"])
        .blocking_save_file()
        .and_then(|p| p.into_path().ok())
    else {
        return Ok(None);
    };
    let text = serde_json::to_string_pretty(&effects).map_err(err)?;
    std::fs::write(&path, text).map_err(err)?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn import_custom_effects(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    use tauri_plugin_dialog::DialogExt;
    let Some(path) = app
        .dialog()
        .file()
        .set_title("Import effects")
        .add_filter("RGBable effects", &["json"])
        .blocking_pick_file()
        .and_then(|p| p.into_path().ok())
    else {
        return Ok(0);
    };
    let text = std::fs::read_to_string(&path).map_err(err)?;
    let text = text.trim_start_matches('\u{feff}');
    let effects: Vec<Effect> = serde_json::from_str::<Vec<Effect>>(text)
        .or_else(|_| serde_json::from_str::<Effect>(text).map(|e| vec![e]))
        .map_err(|e| format!("that isn't an RGBable effects file: {e}"))?;
    let mut imported = 0;
    for effect in effects.into_iter().map(Effect::sanitized) {
        let Some(name) = effect.name.clone().filter(|n| !n.trim().is_empty()) else {
            continue;
        };
        if effect.kind == effects::EffectKind::Script
            && script::check(effect.script.as_deref().unwrap_or(""), &effect.colors).is_err()
        {
            continue;
        }
        state.engine.update_config(|c| {
            match c
                .custom_effects
                .iter_mut()
                .find(|e| e.name.as_deref() == Some(&name))
            {
                Some(existing) => *existing = effect,
                None => c.custom_effects.push(effect),
            }
        });
        imported += 1;
    }
    Ok(imported)
}

#[tauri::command(async)]
fn lianli_effects() -> Vec<render::LianliEffect> {
    render::lianli_effects()
}

#[tauri::command]
async fn check_update(state: State<'_, AppState>) -> Result<UpdateView, String> {
    let updater = state.updater.clone();
    let channel = state.engine.config().update_channel;
    tauri::async_runtime::spawn_blocking(move || {
        updater.check(channel);
        updater.view()
    })
    .await
    .map_err(err)
}

#[tauri::command]
async fn set_update_channel(
    state: State<'_, AppState>,
    channel: Channel,
) -> Result<UpdateView, String> {
    state.engine.set_update_channel(channel);
    check_update(state).await
}

#[tauri::command]
async fn install_update(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let updater = state.updater.clone();
    let setup = tauri::async_runtime::spawn_blocking(move || updater.download())
        .await
        .map_err(err)?
        .map_err(|e| format!("{e:#}"))?;
    if let Err(e) = updater::launch_installer(&setup) {
        let message = format!("{e:#}");
        state.updater.failed(&message);
        return Err(message);
    }
    app.exit(0);
    Ok(())
}

// ------------------------------------------------------------------ paths, autostart

/// Portable mode: a `rgbable-data` folder next to the exe holds config and log.
/// Otherwise %APPDATA%\RGBable.
fn data_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let portable = dir.join("rgbable-data");
            if portable.is_dir() {
                return portable;
            }
        }
    }
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RGBable")
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut command = std::process::Command::new("reg");
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.output()
}

fn autostart_enabled() -> bool {
    cfg!(windows) && reg(&["query", RUN_KEY, "/v", "RGBable"]).is_ok_and(|o| o.status.success())
}

fn set_autostart_impl(enabled: bool) -> anyhow::Result<()> {
    let output = if enabled {
        let exe = std::env::current_exe()?;
        let value = format!("\"{}\" --minimized", exe.display());
        reg(&[
            "add", RUN_KEY, "/v", "RGBable", "/t", "REG_SZ", "/d", &value, "/f",
        ])?
    } else {
        reg(&["delete", RUN_KEY, "/v", "RGBable", "/f"])?
    };
    anyhow::ensure!(
        output.status.success() || !enabled,
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// `--oled <file>` (from OLEDable, or a second launch) plays that file on the OLED screen.
fn handle_args(app: &AppHandle, engine: &Arc<Engine>, args: &[String]) -> bool {
    let Some(file) = args
        .iter()
        .position(|a| a == "--oled")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
    else {
        return false;
    };
    if !file.is_file() {
        tracing::warn!("--oled: {} not found", file.display());
        return false;
    }
    allow_preview(app, &file);
    let current = engine.config().oled;
    let fit = current
        .as_ref()
        .map_or_else(|| "cover".to_string(), |o| o.fit.clone());
    let fps = current.as_ref().map_or(30, |o| o.fps);
    match engine.set_oled(Some(file.clone()), fit, fps) {
        Ok(()) => {
            tracing::info!("playing {} on the OLED (from another app)", file.display());
            if let Some(state) = tauri::Manager::try_state::<AppState>(app) {
                match_oled_in_background(&state.engine, &state.openrgb);
            }
        }
        Err(e) => tracing::warn!("--oled {}: {e:#}", file.display()),
    }
    true
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

// ------------------------------------------------------------------ main

fn main() {
    // Diagnostics: `rgbable --probe-openrgb [port]` lists OpenRGB devices and exits.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--probe-openrgb") {
        let port = args.get(2).and_then(|p| p.parse().ok());
        let client = OpenRgb::new();
        let result = client.connect(port);
        let view = client.view();
        let text = format!(
            "{result:?}\n{}",
            serde_json::to_string_pretty(&view).unwrap_or_default()
        );
        println!("{text}");
        let _ = std::fs::write(data_dir().join("openrgb-probe.txt"), text);
        return;
    }
    // Diagnostics: `rgbable --openrgb-test r,g,b` sets every OpenRGB device and reads it back.
    if args.get(1).map(String::as_str) == Some("--openrgb-test") {
        let color: Vec<u8> = args
            .get(2)
            .map(|s| s.split(',').filter_map(|p| p.trim().parse().ok()).collect())
            .unwrap_or_else(|| vec![255, 255, 255]);
        let color = [color[0], color[1], color[2]];
        let client = OpenRgb::new();
        let mut out = String::new();
        match client.connect(None) {
            Err(e) => out.push_str(&format!("connect failed: {e:#}\n")),
            Ok(()) => {
                let view = client.view();
                out.push_str(&format!("{}\n", view.status));
                for d in view.devices {
                    out.push_str(&format!(
                        "\n[{}] {} ({}, {} LEDs)\n  modes: {:?}\n  before: mode={} first={:?}\n",
                        d.index, d.name, d.kind, d.leds, d.modes, d.active_mode, d.first_color
                    ));
                    let set = client.set_color(d.index, color);
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    match (set, client.refresh(d.index)) {
                        (Err(e), _) => out.push_str(&format!("  set FAILED: {e:#}\n")),
                        (Ok(()), Ok(after)) => out.push_str(&format!(
                            "  after:  mode={} first={:?}\n",
                            after.active_mode, after.first_color
                        )),
                        (Ok(()), Err(e)) => out.push_str(&format!("  read-back FAILED: {e:#}\n")),
                    }
                }
            }
        }
        println!("{out}");
        let _ = std::fs::write(data_dir().join("openrgb-test.txt"), out);
        return;
    }
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("rgbable.log"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .with_ansi(false);
    match log {
        Ok(file) => builder.with_writer(std::sync::Mutex::new(file)).init(),
        Err(_) => builder.init(),
    }
    std::panic::set_hook(Box::new(|panic| error!("panic: {panic}")));

    let config_path = dir.join("config.json");
    info!("RGBable starting, data in {}", dir.display());
    let engine = Engine::new(config_path);
    render::WARMTH.store(engine.config().warmth, std::sync::atomic::Ordering::Relaxed);
    engine.start();

    let openrgb = Arc::new(OpenRgb::new());
    openrgb.start_animator();
    connect_openrgb_in_background(engine.clone(), openrgb.clone(), true);

    let updater = Arc::new(Updater::default());
    if cfg!(not(debug_assertions)) {
        let updater = updater.clone();
        let engine = engine.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(15));
            loop {
                updater.check(engine.config().update_channel);
                std::thread::sleep(updater::CHECK_INTERVAL);
            }
        });
    }

    let minimized = std::env::args().any(|a| a == "--minimized");
    let preview_file = engine.config().oled.map(|o| o.file);
    let startup_args: Vec<String> = std::env::args().collect();
    let handoff_engine = engine.clone();
    let startup_engine = engine.clone();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(
            move |app, args, _cwd| {
                let played = handle_args(app, &handoff_engine, &args);
                if !played || !args.iter().any(|a| a == "--background") {
                    show_main(app);
                }
            },
        ))
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            engine,
            openrgb,
            updater,
        })
        .invoke_handler(tauri::generate_handler![
            overview,
            set_color,
            set_all,
            rename,
            openrgb_connect,
            identify,
            set_sync,
            create_group,
            rename_group,
            delete_group,
            move_to_group,
            set_group_color,
            set_effect,
            set_group_effect,
            finish_setup,
            pick_media,
            set_oled,
            prepare_oled,
            restart_app,
            set_delay,
            set_sequence,
            set_across,
            set_warmth,
            set_fan_mode,
            set_all_fan_modes,
            set_pump_mode,
            select_fan_profile,
            add_fan_profile,
            rename_fan_profile,
            delete_fan_profile,
            set_clean_transitions,
            set_reverse,
            set_speed,
            oled_palette,
            set_oled_match,
            match_oled_now,
            set_color_presets,
            stop_oled,
            set_autostart,
            check_update,
            set_update_channel,
            install_update,
            openrgb_setup_status,
            install_pawnio,
            set_openrgb_managed,
            locate_openrgb,
            resize_zone,
            lianli_effects,
            preview_effect,
            script_template,
            custom_effects,
            save_custom_effect,
            delete_custom_effect,
            export_custom_effects,
            import_custom_effects,
            oled_library,
            add_oled_folder,
            remove_oled_folder,
            set_oled_link
        ])
        .setup(move |app| {
            if let Some(file) = &preview_file {
                allow_preview(app.handle(), file);
            }
            handle_args(app.handle(), &startup_engine, &startup_args);
            let show = MenuItem::with_id(app, "show", "Open RGBable", true, None::<&str>)?;
            let quit = MenuItem::with_id(
                app,
                "quit",
                "Quit (stops lighting control)",
                true,
                None::<&str>,
            )?;
            let restart_item =
                MenuItem::with_id(app, "restart", "Restart RGBable", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &restart_item, &quit])?;
            let mut tray = TrayIconBuilder::with_id("rgbable")
                .tooltip("RGBable")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
                    "restart" => {
                        let app = app.clone();
                        std::thread::spawn(move || {
                            if let Err(e) = restart(&app) {
                                tracing::warn!("restart: {e:#}");
                            }
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            if !minimized {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window keeps RGBable running in the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!());
    if let Err(e) = result {
        error!("RGBable stopped: {e}");
    }
}

/// Starts the managed OpenRGB if the user set that up, then connects to OpenRGB, which may
/// still be starting (at logon, or just launched), so this keeps trying for about a minute.
fn connect_openrgb_in_background(engine: Arc<Engine>, openrgb: Arc<OpenRgb>, start_host: bool) {
    std::thread::spawn(move || {
        let config = engine.config();
        if start_host && config.manage_openrgb {
            openrgb_host::disable_openrgb_autostart();
            if let Err(e) = openrgb_host::start() {
                error!("starting OpenRGB: {e:#}");
            }
        }
        for _ in 0..12 {
            if openrgb.connect(config.openrgb_port).is_ok() {
                lights::after_openrgb_connect(&engine, &openrgb);
                watch_openrgb_devices(&engine, &openrgb);
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    });
}

/// Picks up devices OpenRGB finds after RGBable connected: quickly while OpenRGB is still
/// detecting at startup, then twice a minute for devices plugged in later. Runs for the
/// life of the app.
fn watch_openrgb_devices(engine: &Arc<Engine>, openrgb: &Arc<OpenRgb>) {
    static WATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WATCHING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let started = std::time::Instant::now();
    loop {
        let wait = if started.elapsed() < std::time::Duration::from_secs(90) {
            3
        } else {
            30
        };
        std::thread::sleep(std::time::Duration::from_secs(wait));
        let changed = match openrgb.device_list_changed() {
            Ok(changed) => changed,
            Err(_) => continue,
        };
        if !changed {
            continue;
        }
        let port = engine.config().openrgb_port;
        match openrgb.connect(port) {
            Ok(()) => {
                tracing::info!("OpenRGB's device list changed; reloaded it");
                lights::after_openrgb_connect(engine, openrgb);
            }
            Err(e) => tracing::warn!("reloading OpenRGB devices: {e:#}"),
        }
    }
}
