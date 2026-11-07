// RGBable: Windows desktop app for Lian Li hardware (unofficial).
// No console window in release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod effects;
mod engine;
mod lights;
mod openrgb;
mod updater;

use engine::{DeviceView, Engine, OledView, Telemetry};
use effects::Effect;
use lights::{GroupView, LightView, SetupEntry};
use openrgb::{OpenRgb, OrgbView};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use updater::{Channel, UpdateView, Updater};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, State};
use tracing::{error, info};

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
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
fn overview(state: State<'_, AppState>) -> Overview {
    Overview {
        status: state.engine.status(),
        lconnect_running: engine::lconnect_running(),
        devices: state.engine.devices(),
        telemetry: state.engine.telemetry(),
        oled: state.engine.oled(),
        openrgb: state.openrgb.view(),
        autostart: autostart_enabled(),
        data_dir: data_dir().to_string_lossy().into_owned(),
        lights: lights::lights(&state.engine, &state.openrgb),
        groups: lights::groups(&state.engine),
        setup_done: state.engine.config().setup_done,
        update: state.updater.view(),
        update_channel: state.engine.config().update_channel,
    }
}

fn chain(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
fn set_color(state: State<'_, AppState>, id: String, color: [u8; 3]) -> Result<(), String> {
    lights::set_color(&state.engine, &state.openrgb, &id, color).map_err(chain)
}

#[tauri::command]
fn set_all(state: State<'_, AppState>, effect: Effect, include_openrgb: bool) -> Result<(), String> {
    lights::set_all(&state.engine, &state.openrgb, effect, include_openrgb).map_err(chain)
}

#[tauri::command]
fn set_effect(state: State<'_, AppState>, id: String, effect: Effect) -> Result<(), String> {
    lights::set_effect(&state.engine, &state.openrgb, &id, effect).map_err(chain)
}

#[tauri::command]
fn set_group_effect(state: State<'_, AppState>, group: String, effect: Effect) -> Result<(), String> {
    lights::set_group_effect(&state.engine, &state.openrgb, &group, effect).map_err(chain)
}

fn default_name(state: &AppState, id: &str) -> Option<String> {
    lights::lights(&state.engine, &state.openrgb)
        .into_iter()
        .find(|l| l.id == id)
        .map(|l| l.default_name)
}

#[tauri::command]
fn rename(state: State<'_, AppState>, id: String, name: String) {
    let default = default_name(&state, &id);
    state.engine.update_config(|c| lights::set_name(c, &id, &name, default.as_deref()));
}

#[tauri::command]
fn identify(state: State<'_, AppState>, id: String) -> Result<(), String> {
    lights::identify(&state.engine, &state.openrgb, &id).map_err(chain)
}

#[tauri::command]
fn set_sync(state: State<'_, AppState>, id: String, sync: bool) {
    state.engine.update_config(|c| lights::set_sync(c, &id, sync));
}

#[tauri::command]
fn create_group(state: State<'_, AppState>, name: String) -> String {
    state.engine.update_config(|c| lights::create_group(c, &name))
}

#[tauri::command]
fn rename_group(state: State<'_, AppState>, group: String, name: String) {
    state.engine.update_config(|c| lights::rename_group(c, &group, &name));
}

#[tauri::command]
fn delete_group(state: State<'_, AppState>, group: String) {
    state.engine.update_config(|c| lights::delete_group(c, &group));
}

#[tauri::command]
fn move_to_group(state: State<'_, AppState>, id: String, group: Option<String>) {
    state.engine.update_config(|c| lights::move_to_group(c, &id, group.as_deref()));
}

#[tauri::command]
fn set_group_color(state: State<'_, AppState>, group: String, color: [u8; 3]) -> Result<(), String> {
    lights::set_group_effect(&state.engine, &state.openrgb, &group, lights::static_effect(color)).map_err(chain)
}

#[tauri::command]
fn finish_setup(state: State<'_, AppState>, entries: Vec<SetupEntry>) {
    let defaults: Vec<(String, String)> = lights::lights(&state.engine, &state.openrgb)
        .into_iter()
        .map(|l| (l.id, l.default_name))
        .collect();
    state.engine.update_config(|c| lights::apply_setup(c, &entries, &defaults));
}

#[tauri::command]
async fn openrgb_connect(state: State<'_, AppState>) -> Result<(), String> {
    let openrgb = state.openrgb.clone();
    let engine = state.engine.clone();
    let preferred = engine.config().openrgb_port;
    tauri::async_runtime::spawn_blocking(move || {
        openrgb.connect(preferred)?;
        lights::sync_openrgb_effects(&engine, &openrgb);
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
            &["mp4", "mkv", "mov", "webm", "avi", "gif", "png", "jpg", "jpeg", "bmp", "webp"],
        )
        .blocking_pick_file()
        .and_then(|path| path.into_path().ok())
        .map(|path| path.to_string_lossy().into_owned())
}

#[tauri::command]
fn set_oled(state: State<'_, AppState>, file: Option<String>, fit: String, fps: u32) -> Result<(), String> {
    state.engine.set_oled(file.map(PathBuf::from), fit, fps).map_err(err)
}

#[tauri::command]
fn stop_oled(state: State<'_, AppState>) {
    state.engine.stop_oled();
}

#[tauri::command]
fn set_autostart(enabled: bool) -> Result<(), String> {
    set_autostart_impl(enabled).map_err(|e| format!("{e:#}"))
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
async fn set_update_channel(state: State<'_, AppState>, channel: Channel) -> Result<UpdateView, String> {
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
    updater::launch_installer(&setup).map_err(|e| format!("{e:#}"))?;
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
    cfg!(windows)
        && reg(&["query", RUN_KEY, "/v", "RGBable"]).is_ok_and(|o| o.status.success())
}

fn set_autostart_impl(enabled: bool) -> anyhow::Result<()> {
    let output = if enabled {
        let exe = std::env::current_exe()?;
        let value = format!("\"{}\" --minimized", exe.display());
        reg(&["add", RUN_KEY, "/v", "RGBable", "/t", "REG_SZ", "/d", &value, "/f"])?
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
        let text = format!("{result:?}\n{}", serde_json::to_string_pretty(&view).unwrap_or_default());
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
    engine.start();

    let openrgb = Arc::new(OpenRgb::new());
    openrgb.start_animator();
    {
        let openrgb = openrgb.clone();
        let engine = engine.clone();
        let preferred = engine.config().openrgb_port;
        std::thread::spawn(move || {
            // OpenRGB may still be starting at logon; try for about a minute.
            for _ in 0..12 {
                if openrgb.connect(preferred).is_ok() {
                    lights::sync_openrgb_effects(&engine, &openrgb);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });
    }

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
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { engine, openrgb, updater })
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
            stop_oled,
            set_autostart,
            check_update,
            set_update_channel,
            install_update
        ])
        .setup(move |app| {
            let show = MenuItem::with_id(app, "show", "Open RGBable", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit (stops lighting control)", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let mut tray = TrayIconBuilder::with_id("rgbable")
                .tooltip("RGBable")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main(app),
                    "quit" => app.exit(0),
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
