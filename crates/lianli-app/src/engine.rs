//! Hardware engine: wireless fans/Strimers, HydroShift II OLED Curve ring,
//! pump telemetry and the OLED video loop. Runs on background threads; the UI
//! talks to it through `Engine`'s methods.

use crate::effects::{Effect, Side};
use crate::render::{self, Lighting};
use anyhow::{bail, Context, Result};
use lianli_devices::winusb::hs2_oled_led::Hs2OledLedController;
use lianli_devices::wireless::{DiscoveredDevice, WirelessController, WirelessRgbUpload};
use lianli_shared::rgb::RgbRenderProfile;
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

pub const RING_ID: &str = "oled-ring";
const RING_LEDS: usize = 45;
pub const IDENTIFY_STEPS: u32 = 6;
pub const IDENTIFY_STEP: Duration = Duration::from_millis(700);
const RING_FRAME: Duration = Duration::from_millis(33);
const WIRELESS_FRAME_MS: u32 = 40;
const WIRELESS_SLOWEST_FRAME_MS: u32 = 640;

// ------------------------------------------------------------------ config

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceConfig>,
    #[serde(default)]
    pub oled_ring: Option<[u8; 3]>,
    #[serde(default)]
    pub oled: Option<OledConfig>,
    /// OpenRGB SDK server to read RAM / GPU / motherboard lighting from.
    #[serde(default)]
    pub openrgb_port: Option<u16>,
    #[serde(default)]
    pub update_channel: crate::updater::Channel,
    #[serde(default)]
    pub groups: Vec<GroupConfig>,
    #[serde(default)]
    pub setup_done: bool,
    #[serde(default)]
    pub manage_openrgb: bool,
    /// Leave OLEDable's video folders out of the OLED library.
    #[serde(default)]
    pub oled_hide_oledable: bool,
    /// Extra folders the user added to the OLED library.
    #[serde(default)]
    pub oled_folders: Vec<PathBuf>,
    /// Effects the user built or wrote and saved by name.
    #[serde(default)]
    pub custom_effects: Vec<Effect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openrgb_exe: Option<PathBuf>,
    /// The quick colours in the effect panel, as #rrggbb. Empty means the built-in set.
    #[serde(default)]
    pub color_presets: Vec<String>,
    /// When set, synced lighting takes this many colours from each new OLED video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oled_match: Option<u8>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
    /// SL Flex outer-ring effect when it differs from `effect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outer_effect: Option<Effect>,
    /// OpenRGB addressable zone sizes the user set, by zone index.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub zone_sizes: BTreeMap<usize, usize>,
}

impl DeviceConfig {
    pub fn in_sync(&self) -> bool {
        self.sync.unwrap_or(true)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OledConfig {
    pub file: PathBuf,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default = "default_fit")]
    pub fit: String,
}

fn default_fps() -> u32 {
    30
}
fn default_fit() -> String {
    "cover".into()
}

pub fn load_config(path: &Path) -> Config {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Config::default();
    };
    // Windows PowerShell writes UTF-8 with a byte-order mark; serde_json rejects it.
    match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
        Ok(config) => config,
        Err(e) => {
            error!(
                "{} is not valid, starting with defaults: {e}",
                path.display()
            );
            Config::default()
        }
    }
}

// ------------------------------------------------------------------ public view types

#[derive(Debug, Clone, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub default_name: String,
    /// "fans", "strimer" or "ring"
    pub kind: &'static str,
    pub model: String,
    pub fans: u8,
    pub leds: usize,
    pub color: Option<[u8; 3]>,
    pub rpm: Vec<u16>,
    pub effect: Option<Effect>,
    pub outer_effect: Option<Effect>,
    pub sides: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Telemetry {
    pub pump_rpm: Option<u16>,
    pub coolant_c: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OledView {
    pub file: Option<String>,
    pub fit: String,
    pub fps: u32,
    pub state: String,
}

// ------------------------------------------------------------------ controllers

enum Target {
    Wireless { mac: [u8; 6] },
    Ring(Arc<Hs2OledLedController>),
}

struct Controller {
    id: String,
    kind: &'static str,
    model: String,
    fans: u8,
    leds: usize,
    zones: Vec<usize>,
    target: Target,
    profile: Option<RgbRenderProfile>,
    lighting: Mutex<Lighting>,
    colors: Mutex<Vec<[u8; 3]>>,
    dirty: AtomicBool,
    identifying: AtomicBool,
}

impl Controller {
    fn set_all(&self, color: [u8; 3]) {
        *self.colors.lock() = vec![color; self.leds];
        self.dirty.store(true, Ordering::Release);
    }
}

// ------------------------------------------------------------------ engine

pub struct Engine {
    config_path: PathBuf,
    config: Mutex<Config>,
    status: RwLock<String>,
    wireless: RwLock<Option<Arc<WirelessController>>>,
    ring: RwLock<Option<Arc<Hs2OledLedController>>>,
    controllers: RwLock<Vec<Arc<Controller>>>,
    telemetry: RwLock<Telemetry>,
    oled_state: RwLock<String>,
    oled_stop: Mutex<Arc<AtomicBool>>,
}

impl Engine {
    pub fn new(config_path: PathBuf) -> Arc<Self> {
        let config = load_config(&config_path);
        Arc::new(Self {
            config_path,
            config: Mutex::new(config),
            status: RwLock::new("Starting".into()),
            wireless: RwLock::new(None),
            ring: RwLock::new(None),
            controllers: RwLock::new(Vec::new()),
            telemetry: RwLock::new(Telemetry::default()),
            oled_state: RwLock::new("Off".into()),
            oled_stop: Mutex::new(Arc::new(AtomicBool::new(false))),
        })
    }

    pub fn config(&self) -> Config {
        self.config.lock().clone()
    }

    pub fn status(&self) -> String {
        self.status.read().clone()
    }

    fn set_status(&self, status: impl Into<String>) {
        let status = status.into();
        info!("status: {status}");
        *self.status.write() = status;
    }

    fn save_config(&self) {
        let config = self.config.lock().clone();
        match serde_json::to_string_pretty(&config) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&self.config_path, text) {
                    warn!("saving {}: {e}", self.config_path.display());
                }
            }
            Err(e) => warn!("serialising config: {e}"),
        }
    }

    /// Bring hardware up on a background thread. The UI is usable immediately
    /// and shows `status()` while this runs.
    pub fn start(self: &Arc<Self>) {
        let engine = self.clone();
        thread::Builder::new()
            .name("engine-start".into())
            .spawn(move || {
                if let Err(e) = engine.bring_up() {
                    error!("start-up failed: {e:#}");
                    engine.set_status(format!("Error: {e:#}"));
                }
            })
            .expect("spawn engine thread");
    }

    fn bring_up(self: &Arc<Self>) -> Result<()> {
        self.wait_for_lconnect();

        self.set_status("Connecting to the wireless dongle");
        let wireless = Arc::new(connect_wireless());
        let devices = wireless.devices();
        info!("{} wireless device(s) online", devices.len());
        *self.wireless.write() = Some(wireless.clone());

        let ring = match find_wired(0x0416, 0x8051) {
            Ok(det) => match Hs2OledLedController::new(det.device) {
                Ok(led) => Some(Arc::new(led)),
                Err(e) => {
                    warn!("OLED LED MCU unavailable: {e:#}");
                    None
                }
            },
            Err(_) => None,
        };
        *self.ring.write() = ring.clone();

        self.build_controllers(&devices, ring);

        // Restore saved colours.
        let config = self.config();
        for ctl in self.controllers.read().iter() {
            let saved = if ctl.id == RING_ID {
                config.oled_ring
            } else {
                config.devices.get(&ctl.id).and_then(|d| d.color)
            };
            if let Some(color) = saved {
                ctl.set_all(color);
            }
            let lighting = config
                .devices
                .get(&ctl.id)
                .map(|d| Lighting::from_saved(d.effect.clone(), d.outer_effect.clone()))
                .unwrap_or_default();
            if !lighting.is_static() {
                *ctl.lighting.lock() = lighting;
                ctl.dirty.store(true, Ordering::Release);
            }
        }

        {
            let engine = self.clone();
            thread::Builder::new()
                .name("rgb-writer".into())
                .spawn(move || engine.rgb_writer(&wireless))?;
        }
        {
            let engine = self.clone();
            thread::Builder::new()
                .name("telemetry".into())
                .spawn(move || engine.telemetry_loop())?;
        }
        if config.oled.is_some() {
            self.restart_oled();
        }
        self.set_status("Ready");
        Ok(())
    }

    fn wait_for_lconnect(&self) {
        let mut announced = false;
        while lconnect_running() {
            if !announced {
                self.set_status("Waiting for L-Connect to close");
                announced = true;
            }
            thread::sleep(Duration::from_secs(3));
        }
    }

    fn build_controllers(
        &self,
        devices: &[DiscoveredDevice],
        ring: Option<Arc<Hs2OledLedController>>,
    ) {
        let mut list = Vec::new();
        let mut sorted: Vec<&DiscoveredDevice> = devices.iter().collect();
        sorted.sort_by_key(|d| d.mac);
        for device in sorted {
            let zones = device.fan_type.rgb_zone_led_counts(device.fan_count);
            let leds: usize = zones.iter().sum();
            if leds == 0 {
                continue;
            }
            list.push(Arc::new(Controller {
                id: device.mac_str(),
                kind: if device.fan_count > 0 {
                    "fans"
                } else {
                    "strimer"
                },
                model: device.fan_type.display_name().to_string(),
                fans: device.fan_count,
                leds,
                profile: device.fan_type.rgb_render_profile(device.fan_count),
                zones,
                target: Target::Wireless { mac: device.mac },
                lighting: Mutex::new(Lighting::default()),
                colors: Mutex::new(vec![[0, 0, 0]; leds]),
                dirty: AtomicBool::new(false),
                identifying: AtomicBool::new(false),
            }));
        }
        if let Some(ring) = ring {
            list.push(Arc::new(Controller {
                id: RING_ID.into(),
                kind: "ring",
                model: "HydroShift II OLED Curve ambient ring".into(),
                fans: 0,
                leds: RING_LEDS,
                profile: None,
                zones: vec![RING_LEDS],
                target: Target::Ring(ring),
                lighting: Mutex::new(Lighting::default()),
                colors: Mutex::new(vec![[0, 0, 0]; RING_LEDS]),
                dirty: AtomicBool::new(false),
                identifying: AtomicBool::new(false),
            }));
        }
        *self.controllers.write() = list;
    }

    // -------------------------------------------------------------- UI API

    pub fn devices(&self) -> Vec<DeviceView> {
        let config = self.config();
        let live: BTreeMap<String, DiscoveredDevice> = self
            .wireless
            .read()
            .as_ref()
            .map(|w| w.devices().into_iter().map(|d| (d.mac_str(), d)).collect())
            .unwrap_or_default();
        self.controllers
            .read()
            .iter()
            .map(|ctl| {
                let saved = config.devices.get(&ctl.id);
                let default_name = match ctl.kind {
                    "ring" => "OLED ring".to_string(),
                    "strimer" => format!("Strimer {}", &ctl.id[..5]),
                    _ => format!(
                        "{} fan{} {}",
                        ctl.fans,
                        if ctl.fans == 1 { "" } else { "s" },
                        &ctl.id[..5]
                    ),
                };
                let color = if ctl.id == RING_ID {
                    config.oled_ring
                } else {
                    saved.and_then(|s| s.color)
                };
                let rpm = live
                    .get(&ctl.id)
                    .map(|d| d.fan_rpms[..usize::from(d.fan_count.min(4))].to_vec())
                    .unwrap_or_default();
                DeviceView {
                    id: ctl.id.clone(),
                    name: saved
                        .and_then(|s| s.name.clone())
                        .unwrap_or_else(|| default_name.clone()),
                    default_name,
                    kind: ctl.kind,
                    model: ctl.model.clone(),
                    fans: ctl.fans,
                    leds: ctl.leds,
                    color,
                    rpm,
                    effect: saved.and_then(|s| s.effect.clone()),
                    outer_effect: saved.and_then(|s| s.outer_effect.clone()),
                    sides: render::has_sides(ctl.profile),
                }
            })
            .collect()
    }

    pub fn set_color(&self, id: &str, color: [u8; 3]) -> Result<()> {
        let ctl = self
            .controllers
            .read()
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .with_context(|| format!("no device {id}"))?;
        *ctl.lighting.lock() = Lighting::default();
        ctl.set_all(color);
        let mut config = self.config.lock();
        let entry = config.devices.entry(id.to_string()).or_default();
        entry.effect = None;
        entry.outer_effect = None;
        if id == RING_ID {
            config.oled_ring = Some(color);
        } else {
            entry.color = Some(color);
        }
        drop(config);
        self.save_config();
        Ok(())
    }

    /// A whole-device static effect becomes a plain colour, so the device keeps a single saved colour.
    pub fn set_effect(&self, id: &str, effect: Effect) -> Result<()> {
        if !effect.is_animated() && effect.side == Side::Both {
            return self.set_color(id, effect.frame(&[1], 0.0)[0]);
        }
        let ctl = self
            .controllers
            .read()
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .with_context(|| format!("no device {id}"))?;
        let (saved, outer) = {
            let mut lighting = ctl.lighting.lock();
            lighting.apply(effect, render::has_sides(ctl.profile));
            lighting.to_saved()
        };
        ctl.dirty.store(true, Ordering::Release);
        self.update_config(|c| {
            let entry = c.devices.entry(id.to_string()).or_default();
            entry.effect = saved;
            entry.outer_effect = outer;
        });
        Ok(())
    }

    pub fn set_all_effect(&self, effect: &Effect) {
        let config = self.config();
        let ids: Vec<String> = self
            .controllers
            .read()
            .iter()
            .map(|c| c.id.clone())
            .filter(|id| config.devices.get(id).is_none_or(DeviceConfig::in_sync))
            .collect();
        for id in ids {
            let _ = self.set_effect(&id, effect.clone());
        }
    }

    pub fn update_config<R>(&self, change: impl FnOnce(&mut Config) -> R) -> R {
        let result = change(&mut self.config.lock());
        self.save_config();
        result
    }

    fn saved_color(&self, id: &str) -> Option<[u8; 3]> {
        let config = self.config.lock();
        if id == RING_ID {
            config.oled_ring
        } else {
            config.devices.get(id).and_then(|d| d.color)
        }
    }

    /// Blinks a device white so the user can tell which one it is, then restores its colour.
    pub fn identify(self: &Arc<Self>, id: &str) -> Result<()> {
        let ctl = self
            .controllers
            .read()
            .iter()
            .find(|c| c.id == id)
            .cloned()
            .with_context(|| format!("no device {id}"))?;
        if ctl.identifying.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let engine = self.clone();
        thread::Builder::new()
            .name("identify".into())
            .spawn(move || {
                for step in 0..IDENTIFY_STEPS {
                    ctl.set_all(if step % 2 == 0 {
                        [255, 255, 255]
                    } else {
                        [0, 0, 0]
                    });
                    thread::sleep(IDENTIFY_STEP);
                }
                ctl.identifying.store(false, Ordering::Release);
                ctl.set_all(engine.saved_color(&ctl.id).unwrap_or([0, 0, 0]));
            })?;
        Ok(())
    }

    pub fn set_update_channel(&self, channel: crate::updater::Channel) {
        self.config.lock().update_channel = channel;
        self.save_config();
    }

    pub fn telemetry(&self) -> Telemetry {
        self.telemetry.read().clone()
    }

    pub fn oled(&self) -> OledView {
        let config = self.config();
        let oled = config.oled.as_ref();
        OledView {
            file: oled.map(|o| o.file.to_string_lossy().into_owned()),
            fit: oled.map_or_else(default_fit, |o| o.fit.clone()),
            fps: oled.map_or(30, |o| o.fps),
            state: self.oled_state.read().clone(),
        }
    }

    pub fn set_oled(self: &Arc<Self>, file: Option<PathBuf>, fit: String, fps: u32) -> Result<()> {
        if !matches!(fit.as_str(), "cover" | "contain") {
            bail!("fit must be cover or contain");
        }
        let fps = fps.clamp(1, 60);
        {
            let mut config = self.config.lock();
            config.oled = match file {
                Some(file) => Some(OledConfig { file, fps, fit }),
                None => config.oled.take().map(|mut o| {
                    o.fit = fit;
                    o.fps = fps;
                    o
                }),
            };
        }
        self.save_config();
        self.restart_oled();
        Ok(())
    }

    pub fn stop_oled(&self) {
        self.oled_stop.lock().store(true, Ordering::Release);
        self.config.lock().oled = None;
        self.save_config();
        *self.oled_state.write() = "Off".into();
    }

    fn restart_oled(self: &Arc<Self>) {
        let Some(oled) = self.config().oled else {
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        let previous = std::mem::replace(&mut *self.oled_stop.lock(), stop.clone());
        previous.store(true, Ordering::Release);
        let engine = self.clone();
        thread::Builder::new()
            .name("oled-video".into())
            .spawn(move || engine.oled_loop(&oled, &stop))
            .expect("spawn oled thread");
    }

    // -------------------------------------------------------------- workers

    /// Wireless lighting goes out one device at a time and is confirmed through the
    /// receiver's status polls (about every 500 ms). An upload the device didn't take is
    /// resent, and a device in motherboard-sync mode is switched out of it first.
    /// The ring is USB: static colours are sent once, effects are streamed.
    fn rgb_writer(self: &Arc<Self>, wireless: &WirelessController) {
        const SPACING: Duration = Duration::from_millis(150);
        const CONFIRM_WAIT: Duration = Duration::from_millis(1500);
        const RESENDS: u8 = 4;
        const MB_SYNC_WAIT: Duration = Duration::from_millis(250);
        const ERROR_BACKOFF: Duration = Duration::from_secs(1);
        const VERIFY_EVERY: Duration = Duration::from_secs(20);
        // A confirmed device only counts as having lost its lighting after several
        // checks in a row, so stale discovery data cannot trigger a re-send storm.
        const STRIKES: u8 = 2;
        const SHRINK_AFTER_SENDS: u8 = 2;

        enum State {
            Pending {
                upload: WirelessRgbUpload,
                sent: Instant,
                sends: u8,
            },
            Settled {
                upload: WirelessRgbUpload,
                strikes: u8,
            },
        }
        let mut states: BTreeMap<[u8; 6], State> = BTreeMap::new();
        let mut not_before: BTreeMap<[u8; 6], Instant> = BTreeMap::new();
        // How far each device's effects had to be shrunk before it accepted them.
        let mut shrink: BTreeMap<[u8; 6], u8> = BTreeMap::new();
        let mut last_verify = Instant::now();
        let started = Instant::now();
        let mut last_ring_frame = Instant::now();

        loop {
            let controllers = self.controllers.read().clone();
            let mut sent_any = false;
            for ctl in &controllers {
                let mac = match &ctl.target {
                    Target::Ring(ring) => {
                        self.drive_ring(ctl, ring, started, &mut last_ring_frame);
                        continue;
                    }
                    Target::Wireless { mac } => *mac,
                };
                if not_before.get(&mac).is_some_and(|t| Instant::now() < *t) {
                    continue;
                }

                let resend = match states.get(&mac) {
                    Some(State::Pending {
                        upload,
                        sent,
                        sends,
                    }) if !ctl.dirty.load(Ordering::Acquire) => {
                        if wireless.rgb_upload_applied(&mac, upload.effect_index()) {
                            let upload = upload.clone();
                            states.insert(mac, State::Settled { upload, strikes: 0 });
                            None
                        } else if sent.elapsed() < CONFIRM_WAIT {
                            None
                        } else if *sends >= SHRINK_AFTER_SENDS
                            && !ctl.lighting.lock().is_static()
                            && shrink.get(&mac).copied().unwrap_or(0) < MAX_SHRINK
                        {
                            let level = shrink.get(&mac).copied().unwrap_or(0) + 1;
                            shrink.insert(mac, level);
                            warn!(
                                "{}: effect not confirmed, retrying it smaller (level {level})",
                                ctl.id
                            );
                            Some((None, 0))
                        } else if *sends > RESENDS {
                            warn!("{}: lighting not confirmed after {sends} sends, checking again later", ctl.id);
                            let upload = upload.clone();
                            states.insert(mac, State::Settled { upload, strikes: 0 });
                            None
                        } else {
                            Some((Some(upload.clone()), *sends))
                        }
                    }
                    _ => None,
                };
                let dirty = ctl.dirty.load(Ordering::Acquire);
                if resend.is_none() && !dirty {
                    continue;
                }

                match wireless.mb_rgb_ready_for_upload(&mac) {
                    Ok(true) => {}
                    Ok(false) => {
                        not_before.insert(mac, Instant::now() + MB_SYNC_WAIT);
                        continue;
                    }
                    Err(e) => {
                        warn!("{}: preparing lighting upload: {e:#}", ctl.id);
                        not_before.insert(mac, Instant::now() + ERROR_BACKOFF);
                        continue;
                    }
                }

                let (upload, sends) = match resend {
                    Some((Some(upload), sends)) if !dirty => (Ok(upload), sends),
                    _ => {
                        ctl.dirty.store(false, Ordering::Release);
                        // A missed confirmation can be radio trouble rather than size, so a
                        // new effect starts at full size instead of inheriting a still frame.
                        if dirty {
                            shrink.remove(&mac);
                        }
                        let level = shrink.get(&mac).copied().unwrap_or(0);
                        (prepare_lighting(wireless, ctl, &mac, level), 0)
                    }
                };
                let result = upload.and_then(|u| wireless.send_rgb_upload(&mac, &u, 4).map(|()| u));
                sent_any = true;
                match result {
                    Ok(upload) => {
                        states.insert(
                            mac,
                            State::Pending {
                                upload,
                                sent: Instant::now(),
                                sends: sends + 1,
                            },
                        );
                    }
                    Err(e) => {
                        warn!("{}: lighting upload failed: {e:#}", ctl.id);
                        ctl.dirty.store(true, Ordering::Release);
                        not_before.insert(mac, Instant::now() + ERROR_BACKOFF);
                    }
                }
                thread::sleep(SPACING);
            }

            if last_verify.elapsed() >= VERIFY_EVERY {
                last_verify = Instant::now();
                for ctl in &controllers {
                    let Target::Wireless { mac } = &ctl.target else {
                        continue;
                    };
                    let Some(State::Settled { upload, strikes }) = states.get_mut(mac) else {
                        continue;
                    };
                    if wireless.rgb_upload_applied(mac, upload.effect_index()) {
                        *strikes = 0;
                        continue;
                    }
                    *strikes += 1;
                    if *strikes >= STRIKES {
                        info!("{} stopped showing its lighting, re-sending", ctl.id);
                        let upload = upload.clone();
                        states.insert(
                            *mac,
                            State::Pending {
                                upload,
                                sent: Instant::now()
                                    .checked_sub(CONFIRM_WAIT)
                                    .unwrap_or_else(Instant::now),
                                sends: 1,
                            },
                        );
                    }
                }
            }

            if !sent_any {
                thread::sleep(Duration::from_millis(30));
            }
        }
    }

    fn drive_ring(
        &self,
        ctl: &Controller,
        ring: &Hs2OledLedController,
        started: Instant,
        last_frame: &mut Instant,
    ) {
        let effect = ctl.lighting.lock().primary().map(Effect::generic);
        if let Some(effect) = effect.filter(|_| !ctl.identifying.load(Ordering::Acquire)) {
            ctl.dirty.store(false, Ordering::Release);
            if last_frame.elapsed() < RING_FRAME {
                return;
            }
            *last_frame = Instant::now();
            let elapsed = started.elapsed().as_millis() as u64;
            let period = u64::from(effect.period_ms().max(1));
            let phase = (elapsed % period) as f32 / period as f32;
            if let Err(e) = ring.send_rgb_frame(&effect.frame(&ctl.zones, phase)) {
                warn!("OLED ring: {e:#}");
            }
        } else if ctl.dirty.swap(false, Ordering::AcqRel) {
            let colors = ctl.colors.lock().clone();
            if let Err(e) = ring.send_rgb_frame(&colors) {
                warn!("OLED ring: {e:#}");
            }
        }
    }

    fn telemetry_loop(&self) {
        loop {
            let ring = self.ring.read().clone();
            if let Some(ring) = ring {
                let pump = ring.read_pump_rpm_raw().ok();
                let coolant = ring.read_temps().ok().map(|(c, _)| c);
                *self.telemetry.write() = Telemetry {
                    pump_rpm: pump,
                    coolant_c: coolant,
                };
            }
            thread::sleep(Duration::from_secs(2));
        }
    }

    fn oled_loop(&self, oled: &OledConfig, stop: &AtomicBool) {
        use lianli_devices::winusb::lcd::WinUsbLcdDevice;
        *self.oled_state.write() = "Encoding".into();
        let encoded = match encode_for_oled(oled) {
            Ok(path) => path,
            Err(e) => {
                error!("OLED media: {e:#}");
                *self.oled_state.write() = format!("Error: {e:#}");
                return;
            }
        };
        while !stop.load(Ordering::Acquire) {
            *self.oled_state.write() = "Playing".into();
            let result = find_wired(0x1CBE, 0xA068)
                .and_then(|det| WinUsbLcdDevice::open(det.device, 0xA068))
                .and_then(|mut lcd| lcd.stream_h264(&encoded, true, stop, oled.fps as f32));
            if stop.load(Ordering::Acquire) {
                break;
            }
            if let Err(e) = result {
                warn!("OLED stream stopped: {e:#}; retrying in 5 s");
                *self.oled_state.write() = "Reconnecting".into();
            }
            thread::sleep(Duration::from_secs(5));
        }
    }
}

// ------------------------------------------------------------------ helpers

fn no_window(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = command;
}

pub fn lconnect_running() -> bool {
    if !cfg!(windows) {
        return false;
    }
    let mut tasklist = std::process::Command::new("tasklist");
    tasklist.args(["/FI", "IMAGENAME eq L-Connect 3.exe", "/NH"]);
    no_window(&mut tasklist);
    tasklist
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("L-Connect"))
        .unwrap_or(false)
}

fn connect_wireless() -> WirelessController {
    loop {
        let mut wireless = WirelessController::new();
        match wireless
            .connect()
            .and_then(|()| wireless.start_polling())
            .context("starting the wireless dongle")
        {
            Ok(()) => {
                let deadline = Instant::now() + Duration::from_secs(10);
                while wireless.discovered_device_count() == 0 && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(200));
                }
                return wireless;
            }
            Err(e) => {
                warn!("{e:#}; retrying in 10 s");
                wireless.stop();
                thread::sleep(Duration::from_secs(10));
            }
        }
    }
}

fn find_wired(vid: u16, pid: u16) -> Result<lianli_devices::detect::DetectedDevice> {
    lianli_devices::detect::enumerate_devices()?
        .into_iter()
        .find(|d| d.vid == vid && d.pid == pid)
        .with_context(|| format!("{vid:04x}:{pid:04x} not found"))
}

static ENCODING: Mutex<()> = parking_lot::const_mutex(());
static PREPARE_WANTED: AtomicU64 = AtomicU64::new(0);

fn oled_cache_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(|d| PathBuf::from(d).join("RGBable").join("oled-cache"))
        .unwrap_or_else(|| std::env::temp_dir().join("rgbable"))
}

/// Encodes in the background so a later Play starts at once. Only the newest request
/// is kept: older ones that haven't started yet are skipped.
pub fn prepare_oled(oled: OledConfig) {
    let ticket = PREPARE_WANTED.fetch_add(1, Ordering::AcqRel) + 1;
    thread::spawn(move || {
        if PREPARE_WANTED.load(Ordering::Acquire) != ticket {
            return;
        }
        if let Err(e) = encode_for_oled(&oled) {
            warn!("Preparing {} for the OLED: {e:#}", oled.file.display());
        }
    });
}

/// Fit to the 2288x1080 landscape screen, rotate 90 into the panel's
/// 1080x2288 frame and encode the way the Linux daemon does. The panel takes raw
/// H.264 without B-frames, so files from OLEDable are encoded too, but only once:
/// the result is kept in %LOCALAPPDATA%\RGBable\oled-cache.
fn encode_for_oled(oled: &OledConfig) -> Result<PathBuf> {
    let _one_at_a_time = ENCODING.lock();
    anyhow::ensure!(
        oled.file.is_file(),
        "{} does not exist",
        oled.file.display()
    );
    let fps = oled.fps.clamp(1, 60);
    let source = std::fs::metadata(&oled.file)?;
    let source_modified = source.modified()?;
    let cache_dir = oled_cache_dir();
    std::fs::create_dir_all(&cache_dir)?;
    let stem = oled
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "media".into());
    let output = cache_dir.join(format!("{stem}-{:x}-{}-{fps}.h264", source.len(), oled.fit));
    let partial = output.with_extension("partial");
    if let Ok(meta) = std::fs::metadata(&output) {
        if meta.modified().is_ok_and(|m| m >= source_modified) && meta.len() > 0 {
            return Ok(output);
        }
    }
    let fit = match oled.fit.as_str() {
        "contain" => "scale=2288:1080:force_original_aspect_ratio=decrease:flags=lanczos,pad=2288:1080:(ow-iw)/2:(oh-ih)/2:color=black",
        _ => "scale=2288:1080:force_original_aspect_ratio=increase:flags=lanczos,crop=2288:1080",
    };
    let lower = oled.file.to_string_lossy().to_lowercase();
    let still = [".png", ".jpg", ".jpeg", ".bmp", ".webp", ".tif", ".tiff"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    let mut ffmpeg = std::process::Command::new(crate::oled_library::ffmpeg());
    ffmpeg.args(["-y", "-loglevel", "error"]);
    if still {
        ffmpeg.args(["-loop", "1", "-t", "1"]);
    }
    ffmpeg
        .arg("-i")
        .arg(&oled.file)
        .args(["-vf", &format!("{fit},transpose=1,fps={fps}")])
        .args([
            "-r",
            &fps.to_string(),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ])
        .args([
            "-x264opts",
            "bframes=0",
            "-threads",
            "4",
            "-pix_fmt",
            "yuv420p",
        ])
        .args(["-color_range", "pc", "-an", "-f", "h264"])
        .arg(&partial);
    no_window(&mut ffmpeg);
    info!("Encoding {} for the OLED", oled.file.display());
    let result = ffmpeg
        .output()
        .context("running ffmpeg (install it with: winget install Gyan.FFmpeg)")?;
    if !result.status.success() {
        let _ = std::fs::remove_file(&partial);
        bail!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    std::fs::rename(&partial, &output)?;
    Ok(output)
}

/// Compressed bytes an effect loop may use at shrink level 0. The protocol allows 12 KB,
/// but devices failed to take loops near that size, so loops start at half of it and are
/// halved again for each level a device needed.
const UPLOAD_BUDGET: usize = 6 * 1024;
/// At this level an effect is shown as a still frame.
const MAX_SHRINK: u8 = 3;

/// Static colours go up as one frame; effects as a loop the device plays by itself, drawn
/// on the device's real layout and made smaller until it fits the size budget.
fn prepare_lighting(
    wireless: &WirelessController,
    ctl: &Controller,
    mac: &[u8; 6],
    shrink: u8,
) -> Result<WirelessRgbUpload> {
    let lighting = ctl.lighting.lock().clone();
    let base = ctl.colors.lock().clone();
    if lighting.is_static() || ctl.identifying.load(Ordering::Acquire) {
        return wireless.prepare_rgb_upload(mac, &[base], 5000);
    }
    let frames = if shrink < MAX_SHRINK {
        render::wireless_frames(
            &lighting,
            &base,
            &ctl.zones,
            ctl.profile,
            UPLOAD_BUDGET >> shrink,
            WIRELESS_SLOWEST_FRAME_MS,
            WIRELESS_FRAME_MS,
        )?
    } else {
        warn!("{}: showing its effect as a still frame", ctl.id);
        render::still(&lighting, &base, &ctl.zones, ctl.profile)?
    };
    info!(
        "{}: {} frames, {} bytes",
        ctl.id,
        frames.frames.len(),
        frames.compressed_len().unwrap_or(0)
    );
    wireless.prepare_rgb_animation(mac, &frames.frames, frames.timing)
}
