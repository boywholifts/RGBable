//! Hardware engine: wireless fans/Strimers, HydroShift II OLED Curve ring,
//! pump telemetry and the OLED video loop. Runs on background threads; the UI
//! talks to it through `Engine`'s methods.

use crate::effects::{Effect, Side};
use crate::fans::{self, Decision, FanMode, FanProfile, PumpMode};
use crate::oled_modules::{self, ModuleKind, OledModule, Readings};
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
use tracing::{debug, error, info, warn};

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
    /// Widgets drawn over the screen videos, by video path; `SHARED_WIDGETS` for the rest.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub oled_modules: BTreeMap<String, Vec<OledModule>>,
    /// Widget layouts saved or imported by name, to load into the editor.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub widget_layouts: BTreeMap<String, Vec<OledModule>>,
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
    /// The quick colors in the effect panel, as #rrggbb. Empty means the built-in set.
    #[serde(default)]
    pub color_presets: Vec<String>,
    /// When set, synced lighting takes this many colors from each new OLED video.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oled_match: Option<u8>,
    /// Skip going dark before Everything or a group changes lighting (devices then switch
    /// one by one as they are reached).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub instant_changes: bool,
    /// White balance, -50 (cooler) to 50 (warmer).
    #[serde(default, skip_serializing_if = "is_zero_i8")]
    pub warmth: i8,
    /// Per-device fan modes from before profiles; moved into a profile on load.
    #[serde(default, skip_serializing)]
    fans: BTreeMap<String, FanMode>,
    #[serde(default)]
    pub fan_profiles: Vec<FanProfile>,
    #[serde(default)]
    pub fan_profile: usize,
}

/// The `oled_modules` key of the layout every video without its own uses.
pub const SHARED_WIDGETS: &str = "*";

impl Config {
    /// A video's own widgets, or the shared layout.
    pub fn widgets_for(&self, file: &str) -> Vec<OledModule> {
        self.oled_modules
            .get(file)
            .or_else(|| self.oled_modules.get(SHARED_WIDGETS))
            .cloned()
            .unwrap_or_default()
    }

    pub fn active_fan_profile(&self) -> Option<&FanProfile> {
        self.fan_profiles.get(self.fan_profile)
    }

    fn add_fan_profiles(&mut self) {
        if self.fan_profiles.is_empty() {
            self.fan_profiles = fans::starter_profiles();
            self.fan_profile = fans::STARTER_ACTIVE;
        }
        if !self.fans.is_empty() {
            self.fan_profiles.push(FanProfile {
                name: "Custom".into(),
                all_fans: FanMode::Motherboard,
                fans: std::mem::take(&mut self.fans),
                pump: PumpMode::Unchanged,
            });
            self.fan_profile = self.fan_profiles.len() - 1;
        }
        self.fan_profile = self.fan_profile.min(self.fan_profiles.len() - 1);
    }
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
    /// How far behind the others this device runs its effect, so identical devices don't move in step.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub delay_ms: u32,
    /// Runs moving effects across the device's fans, or through a chain of devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<crate::effects::Span>,
    /// This device's time slot when devices take turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<crate::effects::Span>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

fn is_zero_i8(value: &i8) -> bool {
    *value == 0
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl DeviceConfig {
    pub fn in_sync(&self) -> bool {
        !self.hidden && self.sync.unwrap_or(true)
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
    let mut config = read_config(path);
    config.add_fan_profiles();
    config
}

fn read_config(path: &Path) -> Config {
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
    /// Current speed of each fan, in percent.
    pub speed: Vec<u8>,
    pub fan_mode: FanMode,
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
    pub modules: BTreeMap<String, Vec<OledModule>>,
    pub layouts: BTreeMap<String, Vec<OledModule>>,
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
    /// The device has confirmed (or was given up on for) its latest lighting.
    settled: AtomicBool,
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
    last_coolant: RwLock<Option<(u8, Instant)>>,
    oled_state: RwLock<String>,
    oled_stop: Mutex<Arc<AtomicBool>>,
    oled_thread: Mutex<Option<thread::JoinHandle<()>>>,
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
            last_coolant: RwLock::new(None),
            oled_state: RwLock::new("Off".into()),
            oled_stop: Mutex::new(Arc::new(AtomicBool::new(false))),
            oled_thread: Mutex::new(None),
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

    const FAN_KEEPER_TICK: Duration = Duration::from_secs(1);
    const PUMP_REFRESH: Duration = Duration::from_secs(30);
    const FAN_REFRESH: Duration = Duration::from_secs(10);

    /// Wireless fans keep whatever speed they were last given, and one that resets comes back
    /// stopped, so each second every fan device not at its speed is given it. Motherboard mode follows
    /// the board's fan signal, which the dongle measures on its fan cable; SL V3 fans do that
    /// themselves once told to.
    fn keep_fans_spinning(&self, wireless: &WirelessController) {
        wireless.set_fg_sync(true);
        let mut warned_no_signal = false;
        let mut pump_sent: Option<(PumpMode, Instant)> = None;
        // Speeds go out when a fan isn't at its speed, and now and then regardless: sending
        // every fan its speed every second crowded out lighting uploads on the radio.
        let mut last_sent: BTreeMap<[u8; 6], Instant> = BTreeMap::new();
        loop {
            master_clock(wireless);
            let board = wireless.motherboard_pwm();
            let coolant = self.last_coolant.read().map(|(c, at)| (c, at.elapsed()));
            let profile = self.config.lock().active_fan_profile().cloned();
            let mut needs_board = false;
            for device in wireless.devices().into_iter().filter(|d| d.fan_count > 0) {
                let mode = profile
                    .as_ref()
                    .map(|p| p.fan_mode(&device.mac_str()))
                    .unwrap_or_default();
                let board_driven =
                    mode == FanMode::Motherboard && device.fan_type.supports_hw_mobo_sync();
                let result = match fans::decide(&mode, board, coolant) {
                    _ if board_driven => wireless.set_hardware_pwm_sync(&device.mac),
                    Decision::Duty(duty) => {
                        let running = &device.current_pwm[..usize::from(device.fan_count.min(4))];
                        let due = last_sent
                            .get(&device.mac)
                            .is_none_or(|at| at.elapsed() >= Self::FAN_REFRESH);
                        if due || running.iter().any(|&pwm| pwm.abs_diff(duty) > 2) {
                            last_sent.insert(device.mac, Instant::now());
                            wireless.set_fan_speeds_by_mac(&device.mac, &[duty; 4])
                        } else {
                            Ok(())
                        }
                    }
                    Decision::KeepUnlessStopped => {
                        needs_board = true;
                        let running = &device.current_pwm[..usize::from(device.fan_count.min(4))];
                        if running.contains(&0) {
                            wireless.set_fan_speeds_by_mac(&device.mac, &[255; 4])
                        } else {
                            Ok(())
                        }
                    }
                };
                if let Err(e) = result {
                    debug!("{}: fan speed: {e:#}", device.mac_str());
                }
            }
            if needs_board && !warned_no_signal {
                warn!("the wireless dongle reports no motherboard fan signal; stopped fans run at full speed");
            }
            warned_no_signal = needs_board;

            let pump = profile.map(|p| p.pump).unwrap_or_default();
            let due = pump_sent
                .as_ref()
                .is_none_or(|(sent, at)| *sent != pump || at.elapsed() >= Self::PUMP_REFRESH);
            let ring = self.ring.read().clone();
            if let (true, Some(ring)) = (due, ring) {
                let result = match pump {
                    PumpMode::Unchanged => Ok(()),
                    PumpMode::Motherboard => ring.set_mb_sync(true),
                    PumpMode::Fixed { rpm } => ring.set_pump_rpm(rpm),
                };
                match result {
                    Ok(()) => pump_sent = Some((pump, Instant::now())),
                    Err(e) => {
                        debug!("pump speed: {e:#}");
                        pump_sent = None;
                    }
                }
            }
            thread::sleep(Self::FAN_KEEPER_TICK);
        }
    }

    fn edit_fan_profile(&self, change: impl FnOnce(&mut FanProfile)) {
        self.update_config(|c| {
            let index = c.fan_profile;
            if let Some(profile) = c.fan_profiles.get_mut(index) {
                change(profile);
            }
        });
    }

    pub fn set_fan_mode(&self, id: &str, mode: FanMode) {
        let mode = fans::clean_fan_mode(mode);
        self.edit_fan_profile(|p| {
            if mode == p.all_fans {
                p.fans.remove(id);
            } else {
                p.fans.insert(id.to_string(), mode);
            }
        });
    }

    pub fn set_all_fan_modes(&self, mode: FanMode) {
        let mode = fans::clean_fan_mode(mode);
        self.edit_fan_profile(|p| {
            p.fans.clear();
            p.all_fans = mode;
        });
    }

    pub fn set_pump_mode(&self, mode: PumpMode) {
        let (min, max) = Hs2OledLedController::PUMP_RPM_RANGE;
        let mode = match mode {
            PumpMode::Fixed { rpm } => PumpMode::Fixed {
                rpm: rpm.clamp(min, max),
            },
            other => other,
        };
        self.edit_fan_profile(|p| p.pump = mode);
    }

    pub fn select_fan_profile(&self, index: usize) {
        self.update_config(|c| {
            if index < c.fan_profiles.len() {
                c.fan_profile = index;
            }
        });
    }

    /// Adds a copy of the active profile under `name` and makes it active.
    pub fn add_fan_profile(&self, name: &str) {
        self.update_config(|c| {
            let mut profile = c
                .active_fan_profile()
                .cloned()
                .unwrap_or_else(|| FanProfile {
                    name: String::new(),
                    all_fans: FanMode::Motherboard,
                    fans: BTreeMap::new(),
                    pump: PumpMode::Unchanged,
                });
            profile.name = name.trim().chars().take(40).collect();
            c.fan_profiles.push(profile);
            c.fan_profile = c.fan_profiles.len() - 1;
        });
    }

    pub fn rename_fan_profile(&self, index: usize, name: &str) {
        let name: String = name.trim().chars().take(40).collect();
        if name.is_empty() {
            return;
        }
        self.update_config(|c| {
            if let Some(profile) = c.fan_profiles.get_mut(index) {
                profile.name = name;
            }
        });
    }

    /// The last profile can't be deleted.
    pub fn delete_fan_profile(&self, index: usize) {
        self.update_config(|c| {
            if c.fan_profiles.len() > 1 && index < c.fan_profiles.len() {
                c.fan_profiles.remove(index);
                if c.fan_profile > index || c.fan_profile == c.fan_profiles.len() {
                    c.fan_profile = c.fan_profile.saturating_sub(1);
                }
            }
        });
    }

    /// Pairs devices RGBable already knows (they are in the config) that have reset and
    /// paired with no dongle at all, as fans do after a crash. Devices paired with another
    /// dongle, or unpaired by the user, are left alone.
    fn repair_pairing(&self, wireless: &WirelessController) {
        let known: Vec<String> = self.config().devices.keys().cloned().collect();
        let mut repaired = 0;
        for device in wireless.unbound_devices() {
            let id = device.mac_str();
            if device.master_mac != [0; 6]
                || !known.contains(&id)
                || wireless.is_manually_unbound(&device.mac)
            {
                continue;
            }
            match wireless.bind_device(&device.mac) {
                Ok(()) => {
                    info!("{id}: lost its pairing, paired it with this dongle again");
                    repaired += 1;
                }
                Err(e) => warn!("{id}: pairing again failed: {e:#}"),
            }
        }
        if repaired > 0 {
            thread::sleep(Duration::from_secs(2));
        }
    }

    fn bring_up(self: &Arc<Self>) -> Result<()> {
        self.wait_for_lconnect();

        self.set_status("Connecting to the wireless dongle");
        let wireless = Arc::new(connect_wireless());
        self.repair_pairing(&wireless);
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

        // Restore saved colors.
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
            let (engine, wireless) = (self.clone(), wireless.clone());
            thread::Builder::new()
                .name("fan-keeper".into())
                .spawn(move || engine.keep_fans_spinning(&wireless))?;
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
                settled: AtomicBool::new(true),
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
                settled: AtomicBool::new(true),
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
                let fan_count = |d: &DiscoveredDevice| usize::from(d.fan_count.min(4));
                let rpm = live
                    .get(&ctl.id)
                    .map(|d| d.fan_rpms[..fan_count(d)].to_vec())
                    .unwrap_or_default();
                let speed = live
                    .get(&ctl.id)
                    .map(|d| {
                        d.current_pwm[..fan_count(d)]
                            .iter()
                            .map(|&duty| fans::duty_percent(duty))
                            .collect()
                    })
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
                    speed,
                    fan_mode: config
                        .active_fan_profile()
                        .map(|p| p.fan_mode(&ctl.id))
                        .unwrap_or_default(),
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

    /// A whole-device static effect becomes a plain color, so the device keeps a single saved color.
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

    /// Turns devices off without touching their saved lighting, then waits (at most
    /// `timeout`) until the wireless ones have confirmed it, so a new effect can start on
    /// all of them from dark instead of replacing the old one device by device.
    pub fn blank_and_wait(&self, ids: &[String], timeout: Duration) {
        let targets: Vec<Arc<Controller>> = self
            .controllers
            .read()
            .iter()
            .filter(|c| ids.contains(&c.id))
            .cloned()
            .collect();
        for ctl in &targets {
            *ctl.lighting.lock() = Lighting::default();
            ctl.settled.store(false, Ordering::Release);
            ctl.set_all([0, 0, 0]);
        }
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let done = targets.iter().all(|c| {
                matches!(c.target, Target::Ring(_))
                    || (c.settled.load(Ordering::Acquire) && !c.dirty.load(Ordering::Acquire))
            });
            if done {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Sends every Lian Li device its lighting again (after the white balance changed).
    pub fn relight_all(&self) {
        for ctl in self.controllers.read().iter() {
            ctl.dirty.store(true, Ordering::Release);
        }
    }

    /// Sends a wireless device its lighting again (after a setting such as its delay changed).
    pub fn relight(&self, id: &str) {
        if let Some(ctl) = self.controllers.read().iter().find(|c| c.id == id) {
            ctl.dirty.store(true, Ordering::Release);
        }
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

    /// Blinks a device white so the user can tell which one it is, then restores its color.
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
            modules: config.oled_modules.clone(),
            layouts: config.widget_layouts.clone(),
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
        let mut thread = self.oled_thread.lock();
        let before = thread.take();
        // the screen takes one stream at a time: the new one waits for the old one to let go
        *thread = Some(
            thread::Builder::new()
                .name("oled-video".into())
                .spawn(move || {
                    if let Some(before) = before {
                        let _ = before.join();
                    }
                    if !stop.load(Ordering::Acquire) {
                        engine.oled_loop(&oled, &stop);
                    }
                })
                .expect("spawn oled thread"),
        );
    }

    // -------------------------------------------------------------- workers

    /// Wireless lighting goes out one device at a time and is confirmed through the
    /// receiver's status polls (about every 500 ms). An upload the device didn't take is
    /// resent, and a device in motherboard-sync mode is switched out of it first.
    /// The ring is USB: static colors are sent once, effects are streamed.
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
        const RETRY_FULL_AFTER: Duration = Duration::from_secs(300);

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
        let mut shrunk_at: BTreeMap<[u8; 6], Instant> = BTreeMap::new();
        let mut last_verify = Instant::now();
        let mut last_ring_frame = Instant::now();

        loop {
            let controllers = self.controllers.read().clone();
            let mut sent_any = false;
            for ctl in &controllers {
                let mac = match &ctl.target {
                    Target::Ring(ring) => {
                        self.drive_ring(ctl, ring, &mut last_ring_frame);
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
                            ctl.settled.store(true, Ordering::Release);
                            None
                        } else if sent.elapsed() < CONFIRM_WAIT {
                            None
                        } else if *sends >= SHRINK_AFTER_SENDS
                            && !ctl.lighting.lock().is_static()
                            && shrink.get(&mac).copied().unwrap_or(0) < MAX_SHRINK
                        {
                            let level = shrink.get(&mac).copied().unwrap_or(0) + 1;
                            shrink.insert(mac, level);
                            shrunk_at.insert(mac, Instant::now());
                            warn!(
                                "{}: effect not confirmed, retrying it smaller (level {level})",
                                ctl.id
                            );
                            Some((None, 0))
                        } else if *sends > RESENDS {
                            warn!("{}: lighting not confirmed after {sends} sends, checking again later", ctl.id);
                            let upload = upload.clone();
                            states.insert(mac, State::Settled { upload, strikes: 0 });
                            ctl.settled.store(true, Ordering::Release);
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
                // A device still taking in one upload can freeze on a frame if the next one
                // starts over it, as when an inner and an outer effect change back to back.
                // The newer lighting waits for the first to land (or time out).
                if let Some(State::Pending { upload, sent, .. }) = states.get(&mac) {
                    if dirty
                        && sent.elapsed() < CONFIRM_WAIT
                        && !wireless.rgb_upload_applied(&mac, upload.effect_index())
                    {
                        continue;
                    }
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

                let prepare = |level: u8| {
                    let (delay, span, turn) = self
                        .config()
                        .devices
                        .get(&ctl.id)
                        .map_or((0, None, None), |d| (d.delay_ms, d.span, d.turn));
                    prepare_lighting(wireless, ctl, &mac, level, delay, span, turn)
                };
                let (upload, sends) = match resend {
                    // prepared again rather than re-sent as is: the loop's starting point
                    // depends on when it lands, and a stale one puts the device out of step
                    Some((Some(_), sends)) if !dirty => {
                        (prepare(shrink.get(&mac).copied().unwrap_or(0)), sends)
                    }
                    _ => {
                        ctl.dirty.store(false, Ordering::Release);
                        ctl.settled.store(false, Ordering::Release);
                        // A missed confirmation can be radio trouble rather than size, so a
                        // new effect starts at full size instead of inheriting a still frame.
                        if dirty {
                            shrink.remove(&mac);
                            shrunk_at.remove(&mac);
                        }
                        (prepare(shrink.get(&mac).copied().unwrap_or(0)), 0)
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
                    // A shrunk effect (or a still frame) came from a bad spell on the radio,
                    // so the full effect is tried again after a while rather than kept.
                    if shrunk_at
                        .get(mac)
                        .is_some_and(|at| at.elapsed() >= RETRY_FULL_AFTER)
                    {
                        shrunk_at.remove(mac);
                        info!("{}: trying its full effect again", ctl.id);
                        ctl.dirty.store(true, Ordering::Release);
                        continue;
                    }
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

    fn drive_ring(&self, ctl: &Controller, ring: &Hs2OledLedController, last_frame: &mut Instant) {
        let effect = ctl.lighting.lock().primary().map(Effect::generic);
        if let Some(effect) = effect.filter(|_| !ctl.identifying.load(Ordering::Acquire)) {
            ctl.dirty.store(false, Ordering::Release);
            if last_frame.elapsed() < RING_FRAME {
                return;
            }
            *last_frame = Instant::now();
            let (delay, span, turn) = self
                .config
                .lock()
                .devices
                .get(&ctl.id)
                .map_or((0, None, None), |d| (i64::from(d.delay_ms), d.span, d.turn));
            let effect = Effect {
                span,
                turn,
                ..effect
            };
            let behind = render::clock_ms() - delay;
            let period = u64::from(effect.period_ms().max(1));
            let phase = behind.rem_euclid(period as i64) as f32 / period as f32;
            let mut frame = effect.frame(&ctl.zones, phase);
            render::balance_all(&mut frame);
            if let Err(e) = ring.send_rgb_frame(&frame) {
                warn!("OLED ring: {e:#}");
            }
        } else if ctl.dirty.swap(false, Ordering::AcqRel) {
            let mut colors = ctl.colors.lock().clone();
            render::balance_all(&mut colors);
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
                if let Some(celsius) = coolant.filter(|c| (1..=100).contains(c)) {
                    *self.last_coolant.write() = Some((celsius, Instant::now()));
                }
                *self.telemetry.write() = Telemetry {
                    pump_rpm: pump,
                    coolant_c: coolant,
                };
            }
            thread::sleep(Duration::from_secs(2));
        }
    }

    /// What the screen's widgets show right now.
    /// `cpu` asks for the CPU temperature too, which starts its administrator helper.
    pub fn readings(&self, gpu: Option<&mut oled_modules::Gpu>, cpu: bool) -> Readings {
        let telemetry = self.telemetry.read().clone();
        let rpms: Vec<u32> = self
            .wireless
            .read()
            .as_ref()
            .map(|w| {
                w.devices()
                    .iter()
                    .flat_map(|d| d.fan_rpms[..usize::from(d.fan_count.min(4))].to_vec())
                    .filter(|&rpm| rpm > 0)
                    .map(u32::from)
                    .collect()
            })
            .unwrap_or_default();
        let gpu = gpu.map(|g| g.read()).unwrap_or_default();
        Readings {
            coolant_c: telemetry.coolant_c,
            cpu_temp_c: cpu
                .then(|| crate::cpu_temp::read(self.config_path.parent()?))
                .flatten(),
            pump_rpm: telemetry.pump_rpm,
            gpu_temp_c: gpu.temp_c,
            gpu_load: gpu.load,
            gpu_power_w: gpu.power_w,
            gpu_clock_mhz: gpu.clock_mhz,
            cpu_load: crate::system_load::cpu_load(),
            ram_load: crate::system_load::ram_load(),
            fans_rpm: (!rpms.is_empty()).then(|| rpms.iter().sum::<u32>() / rpms.len() as u32),
        }
    }

    pub fn set_oled_modules(self: &Arc<Self>, file: String, modules: Vec<OledModule>) {
        let modules: Vec<OledModule> = modules
            .into_iter()
            .take(oled_modules::MAX_MODULES)
            .map(OledModule::cleaned)
            .collect();
        let playing = self.update_config(|c| {
            if modules.is_empty() {
                c.oled_modules.remove(&file);
            } else {
                c.oled_modules.insert(file.clone(), modules);
            }
            c.oled.as_ref().is_some_and(|o| {
                let playing = o.file.to_string_lossy();
                playing == file.as_str()
                    || file == SHARED_WIDGETS && !c.oled_modules.contains_key(playing.as_ref())
            })
        });
        if playing {
            self.restart_oled();
        }
    }

    fn oled_loop(&self, oled: &OledConfig, stop: &AtomicBool) {
        let modules = self.config.lock().widgets_for(&oled.file.to_string_lossy());
        if !modules.is_empty() {
            return self.oled_live_loop(oled, &modules, stop);
        }
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
            sleep_unless_stopped(Duration::from_secs(5), stop);
        }
    }
}

impl Engine {
    /// Plays the video with its widgets drawn on, encoding it as it plays: one ffmpeg reads
    /// the video and an overlay image this sends once a second, and its output goes straight
    /// to the screen.
    fn oled_live_loop(&self, oled: &OledConfig, modules: &[OledModule], stop: &AtomicBool) {
        use lianli_devices::winusb::lcd::WinUsbLcdDevice;
        while !stop.load(Ordering::Acquire) {
            *self.oled_state.write() = "Playing".into();
            let result = live_ffmpeg(oled, modules).and_then(|mut ffmpeg| {
                let overlay_in = ffmpeg.stdin.take().context("ffmpeg's input")?;
                let mut video_out = ffmpeg.stdout.take().context("ffmpeg's output")?;
                let streamed = thread::scope(|scope| {
                    // closing ffmpeg's overlay input when stopped is what lets it run on
                    // and unblock the reader below
                    scope.spawn(move || self.feed_overlay(modules, overlay_in, stop));
                    let streamed = find_wired(0x1CBE, 0xA068)
                        .and_then(|det| WinUsbLcdDevice::open(det.device, 0xA068))
                        .and_then(|mut lcd| {
                            lcd.stream_h264_reader(&mut video_out, stop, oled.fps as f32)
                        });
                    let _ = ffmpeg.kill();
                    streamed
                });
                let finished = ffmpeg.wait_with_output();
                if let Ok(output) = finished.as_ref() {
                    let errors = String::from_utf8_lossy(&output.stderr);
                    if !errors.trim().is_empty() && !stop.load(Ordering::Acquire) {
                        warn!("ffmpeg (screen widgets): {}", errors.trim());
                    }
                }
                streamed
            });
            if stop.load(Ordering::Acquire) {
                break;
            }
            if let Err(e) = result {
                warn!("OLED stream with widgets stopped: {e:#}; retrying in 5 s");
                *self.oled_state.write() = "Reconnecting".into();
            }
            sleep_unless_stopped(Duration::from_secs(5), stop);
        }
    }

    /// Sends ffmpeg the widgets as an upright overlay frame once a second until stopped or
    /// ffmpeg goes away. The frames' timing is what paces the video to real time.
    fn feed_overlay(
        &self,
        modules: &[OledModule],
        mut into: impl std::io::Write,
        stop: &AtomicBool,
    ) {
        let mut gpu = modules
            .iter()
            .any(|m| m.reading().uses_gpu())
            .then(oled_modules::Gpu::new);
        let cpu = modules.iter().any(|m| m.reading().uses_cpu());
        let mut next = Instant::now();
        while !stop.load(Ordering::Acquire) {
            let readings = self.readings(gpu.as_mut(), cpu);
            let frame = oled_modules::render(modules, &readings, chrono::Local::now());
            if into
                .write_all(&oled_modules::rotate_clockwise(&frame))
                .is_err()
            {
                return;
            }
            next += Duration::from_secs(1);
            thread::sleep(next.saturating_duration_since(Instant::now()));
        }
    }
}

fn sleep_unless_stopped(duration: Duration, stop: &AtomicBool) {
    let until = Instant::now() + duration;
    while !stop.load(Ordering::Acquire) && Instant::now() < until {
        thread::sleep(Duration::from_millis(100));
    }
}

fn live_ffmpeg(oled: &OledConfig, modules: &[OledModule]) -> Result<std::process::Child> {
    anyhow::ensure!(
        oled.file.is_file(),
        "{} does not exist",
        oled.file.display()
    );
    let fps = oled.fps.clamp(1, 60);
    let fit = match oled.fit.as_str() {
        "contain" => "scale=2288:1080:force_original_aspect_ratio=decrease,pad=2288:1080:(ow-iw)/2:(oh-ih)/2:color=black",
        _ => "scale=2288:1080:force_original_aspect_ratio=increase,crop=2288:1080",
    };
    let still = is_still_image(&oled.file);
    let mut ffmpeg = std::process::Command::new(crate::oled_library::ffmpeg());
    ffmpeg.args(["-loglevel", "error"]);
    if still {
        ffmpeg.args(["-loop", "1"]);
    } else {
        ffmpeg.args(["-stream_loop", "-1"]);
    }
    ffmpeg
        .arg("-i")
        .arg(&oled.file)
        .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s"])
        .arg(format!("{}x{}", oled_modules::HEIGHT, oled_modules::WIDTH))
        .args(["-framerate", "1", "-i", "-"]);
    let upright = crate::oled_library::upright(&oled.file);
    // one animated overlay at most: each is another video to decode alongside
    let animation = modules
        .iter()
        .find(|m| m.kind == ModuleKind::Video && Path::new(&m.file).is_file());
    let base = match animation {
        Some(module) => {
            ffmpeg.args(["-stream_loop", "-1", "-i"]).arg(&module.file);
            let (left, top, w, h) = oled_modules::pixel_box(module);
            let turn = crate::oled_library::upright(Path::new(&module.file));
            let alpha = f32::from(module.opacity.clamp(10, 100)) / 100.0;
            format!(
                "[0:v]{upright}{fit}[back];\
                 [2:v]{turn}scale={w}:{h},format=rgba,colorchannelmixer=aa={alpha:.2}[animation];\
                 [back][animation]overlay={left}:{top}:format=auto,"
            )
        }
        None => format!("[0:v]{upright}{fit},"),
    };
    ffmpeg
        .args([
            "-filter_complex",
            &format!(
                "{base}transpose=1,fps={fps}[video];[1:v]fps={fps}[readouts];\
                 [video][readouts]overlay=format=auto:repeatlast=1"
            ),
        ])
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
        .args(["-color_range", "pc", "-an", "-f", "h264", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    no_window(&mut ffmpeg);
    info!("Playing {} on the OLED with widgets", oled.file.display());
    ffmpeg
        .spawn()
        .context("running ffmpeg (install it with: winget install Gyan.FFmpeg)")
}

fn is_still_image(file: &Path) -> bool {
    let lower = file.to_string_lossy().to_lowercase();
    [".png", ".jpg", ".jpeg", ".bmp", ".webp", ".tif", ".tiff"]
        .iter()
        .any(|ext| lower.ends_with(ext))
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
    let upright = crate::oled_library::upright(&oled.file);
    let turned = if upright.is_empty() { "" } else { "-u" };
    let output = cache_dir.join(format!(
        "{stem}-{:x}-{}-{fps}{turned}.h264",
        source.len(),
        oled.fit
    ));
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
        .args(["-vf", &format!("{upright}{fit},transpose=1,fps={fps}")])
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

/// Compressed bytes an effect loop may use at shrink level 0. The protocol allows 12 KB, but
/// with seven devices on the radio, loops of 5 KB went unconfirmed on every one of them, so
/// loops start at a quarter of it and are halved again for each level a device needed.
const UPLOAD_BUDGET: usize = 3 * 1024;
/// At this level an effect is shown as a still frame.
const MAX_SHRINK: u8 = 3;

/// Static colors go up as one frame; effects as a loop the device plays by itself, drawn
/// on the device's real layout and made smaller until it fits the size budget.
/// A recent reading of the dongle's RF master clock that wireless loops play on. Read again
/// every so often, so a difference in speed between it and the PC's clock can't build up.
fn master_clock(wireless: &WirelessController) -> Option<(u32, Instant)> {
    const REREAD: Duration = Duration::from_secs(30);
    static READING: Mutex<Option<(u32, Instant)>> = Mutex::new(None);
    let mut reading = READING.lock();
    if reading.is_none_or(|(_, at)| at.elapsed() >= REREAD) {
        match wireless.read_rgb_clock() {
            Ok(clock) if clock.0 != 0 => {
                render::note_master_clock(clock.0, clock.1);
                *reading = Some(clock);
            }
            Ok(_) => {}
            Err(e) => debug!("reading the RF master clock: {e:#}"),
        }
    }
    *reading
}

fn prepare_lighting(
    wireless: &WirelessController,
    ctl: &Controller,
    mac: &[u8; 6],
    shrink: u8,
    delay_ms: u32,
    span: Option<crate::effects::Span>,
    turn: Option<crate::effects::Span>,
) -> Result<WirelessRgbUpload> {
    let mut lighting = ctl.lighting.lock().clone();
    for effect in [&mut lighting.inner, &mut lighting.outer]
        .into_iter()
        .flatten()
    {
        effect.span = span;
        effect.turn = turn;
    }
    let base = ctl.colors.lock().clone();
    if lighting.is_static() || ctl.identifying.load(Ordering::Acquire) {
        let mut base = base;
        render::balance_all(&mut base);
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
    let frames = frames.on_clock(delay_ms, master_clock(wireless));
    let mut frames = frames;
    for frame in &mut frames.frames {
        render::balance_all(frame);
    }
    wireless.prepare_rgb_animation(mac, &frames.frames, frames.timing)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loaded(json: &str) -> Config {
        let mut config: Config = serde_json::from_str(json).unwrap();
        config.add_fan_profiles();
        config
    }

    #[test]
    fn a_new_config_starts_on_the_standard_profile() {
        let config = loaded("{}");
        let active = config.active_fan_profile().unwrap();
        assert_eq!(active.name, "Standard");
        assert_eq!(active.fan_mode("aa"), FanMode::Motherboard);
        assert_eq!(active.pump, PumpMode::Unchanged);
    }

    #[test]
    fn fan_settings_from_before_profiles_become_the_active_profile() {
        let config = loaded(r#"{"fans":{"aa":{"mode":"fixed","percent":40}}}"#);
        let active = config.active_fan_profile().unwrap();
        assert_eq!(active.name, "Custom");
        assert_eq!(active.fan_mode("aa"), FanMode::Fixed { percent: 40 });
        assert_eq!(active.fan_mode("bb"), FanMode::Motherboard);
        let saved = serde_json::to_string(&config).unwrap();
        assert_eq!(loaded(&saved).fan_profiles.len(), config.fan_profiles.len());
    }
}
