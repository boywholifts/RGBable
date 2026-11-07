//! Hardware engine: wireless fans/Strimers, HydroShift II OLED Curve ring,
//! pump telemetry and the OLED video loop. Runs on background threads; the UI
//! talks to it through `Engine`'s methods.

use anyhow::{bail, Context, Result};
use lianli_devices::winusb::hs2_oled_led::Hs2OledLedController;
use lianli_devices::wireless::{DiscoveredDevice, WirelessController, WirelessRgbUpload};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

pub const RING_ID: &str = "oled-ring";
const RING_LEDS: usize = 45;

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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<[u8; 3]>,
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
            error!("{} is not valid, starting with defaults: {e}", path.display());
            Config::default()
        }
    }
}

// ------------------------------------------------------------------ public view types

#[derive(Debug, Clone, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    /// "fans", "strimer" or "ring"
    pub kind: &'static str,
    pub model: String,
    pub fans: u8,
    pub leds: usize,
    pub color: Option<[u8; 3]>,
    pub rpm: Vec<u16>,
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
    target: Target,
    colors: Mutex<Vec<[u8; 3]>>,
    dirty: AtomicBool,
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

    fn build_controllers(&self, devices: &[DiscoveredDevice], ring: Option<Arc<Hs2OledLedController>>) {
        let mut list = Vec::new();
        let mut sorted: Vec<&DiscoveredDevice> = devices.iter().collect();
        sorted.sort_by_key(|d| d.mac);
        for device in sorted {
            let leds: usize = device.fan_type.rgb_zone_led_counts(device.fan_count).iter().sum();
            if leds == 0 {
                continue;
            }
            list.push(Arc::new(Controller {
                id: device.mac_str(),
                kind: if device.fan_count > 0 { "fans" } else { "strimer" },
                model: device.fan_type.display_name().to_string(),
                fans: device.fan_count,
                leds,
                target: Target::Wireless { mac: device.mac },
                colors: Mutex::new(vec![[0, 0, 0]; leds]),
                dirty: AtomicBool::new(false),
            }));
        }
        if let Some(ring) = ring {
            list.push(Arc::new(Controller {
                id: RING_ID.into(),
                kind: "ring",
                model: "HydroShift II OLED Curve ambient ring".into(),
                fans: 0,
                leds: RING_LEDS,
                target: Target::Ring(ring),
                colors: Mutex::new(vec![[0, 0, 0]; RING_LEDS]),
                dirty: AtomicBool::new(false),
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
                    _ => format!("{} fan{} {}", ctl.fans, if ctl.fans == 1 { "" } else { "s" }, &ctl.id[..5]),
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
                    name: saved.and_then(|s| s.name.clone()).unwrap_or(default_name),
                    kind: ctl.kind,
                    model: ctl.model.clone(),
                    fans: ctl.fans,
                    leds: ctl.leds,
                    color,
                    rpm,
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
        ctl.set_all(color);
        let mut config = self.config.lock();
        if id == RING_ID {
            config.oled_ring = Some(color);
        } else {
            config.devices.entry(id.to_string()).or_default().color = Some(color);
        }
        drop(config);
        self.save_config();
        Ok(())
    }

    pub fn set_all_colors(&self, color: [u8; 3]) {
        let ids: Vec<String> = self.controllers.read().iter().map(|c| c.id.clone()).collect();
        for id in ids {
            let _ = self.set_color(&id, color);
        }
    }

    pub fn rename(&self, id: &str, name: &str) {
        let name = name.trim();
        let mut config = self.config.lock();
        let entry = config.devices.entry(id.to_string()).or_default();
        entry.name = (!name.is_empty()).then(|| name.to_string());
        drop(config);
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
        let Some(oled) = self.config().oled else { return };
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

    /// Wireless frames are full RF uploads: at most one per device every 200 ms,
    /// 150 ms between uploads on the shared radio. The ring is USB and immediate.
    fn rgb_writer(self: &Arc<Self>, wireless: &WirelessController) {
        const MIN_INTERVAL: Duration = Duration::from_millis(200);
        const SPACING: Duration = Duration::from_millis(150);
        // A device only counts as having lost its colour after several checks
        // in a row, so stale discovery data cannot trigger a re-send storm.
        const VERIFY_EVERY: Duration = Duration::from_secs(20);
        const STRIKES: u8 = 3;
        let mut sent: BTreeMap<[u8; 6], (Instant, WirelessRgbUpload, u8)> = BTreeMap::new();
        let mut last_verify = Instant::now();

        loop {
            let controllers = self.controllers.read().clone();
            let mut uploaded = false;
            for ctl in &controllers {
                if !ctl.dirty.load(Ordering::Acquire) {
                    continue;
                }
                match &ctl.target {
                    Target::Ring(ring) => {
                        ctl.dirty.store(false, Ordering::Release);
                        let colors = ctl.colors.lock().clone();
                        if let Err(e) = ring.send_rgb_frame(&colors) {
                            warn!("OLED ring: {e:#}");
                        }
                    }
                    Target::Wireless { mac } => {
                        if sent.get(mac).is_some_and(|(at, _, _)| at.elapsed() < MIN_INTERVAL) {
                            continue;
                        }
                        ctl.dirty.store(false, Ordering::Release);
                        let colors = ctl.colors.lock().clone();
                        match wireless
                            .prepare_rgb_upload(mac, &[colors], 5000)
                            .and_then(|u| wireless.send_rgb_upload(mac, &u, 4).map(|()| u))
                        {
                            Ok(upload) => {
                                sent.insert(*mac, (Instant::now(), upload, 0));
                            }
                            Err(e) => {
                                warn!("{}: colour upload failed: {e:#}", ctl.id);
                                ctl.dirty.store(true, Ordering::Release);
                            }
                        }
                        uploaded = true;
                        thread::sleep(SPACING);
                    }
                }
            }

            if last_verify.elapsed() >= VERIFY_EVERY {
                last_verify = Instant::now();
                for ctl in &controllers {
                    let Target::Wireless { mac } = &ctl.target else { continue };
                    let Some((at, upload, strikes)) = sent.get_mut(mac) else { continue };
                    if at.elapsed() < Duration::from_secs(10) {
                        continue;
                    }
                    if wireless.rgb_upload_applied(mac, upload.effect_index()) {
                        *strikes = 0;
                    } else {
                        *strikes += 1;
                        if *strikes >= STRIKES {
                            info!("{} stopped showing its colour, re-sending", ctl.id);
                            *strikes = 0;
                            ctl.dirty.store(true, Ordering::Release);
                        }
                    }
                }
            }

            if !uploaded {
                thread::sleep(Duration::from_millis(30));
            }
        }
    }

    fn telemetry_loop(&self) {
        loop {
            let ring = self.ring.read().clone();
            if let Some(ring) = ring {
                let pump = ring.read_pump_rpm_raw().ok();
                let coolant = ring.read_temps().ok().map(|(c, _)| c);
                *self.telemetry.write() = Telemetry { pump_rpm: pump, coolant_c: coolant };
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

/// Fit to the 2288x1080 landscape screen, rotate 90 into the panel's
/// 1080x2288 frame and encode the way the Linux daemon does. Cached in %TEMP%.
fn encode_for_oled(oled: &OledConfig) -> Result<PathBuf> {
    anyhow::ensure!(oled.file.is_file(), "{} does not exist", oled.file.display());
    let fps = oled.fps.clamp(1, 60);
    let source_modified = std::fs::metadata(&oled.file)?.modified()?;
    let cache_dir = std::env::temp_dir().join("rgbable");
    std::fs::create_dir_all(&cache_dir)?;
    let stem = oled
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "media".into());
    let output = cache_dir.join(format!("{stem}-{}-{fps}.h264", oled.fit));
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
    let mut ffmpeg = std::process::Command::new("ffmpeg");
    ffmpeg.args(["-y", "-loglevel", "error"]);
    if still {
        ffmpeg.args(["-loop", "1", "-t", "1"]);
    }
    ffmpeg
        .arg("-i")
        .arg(&oled.file)
        .args(["-vf", &format!("{fit},transpose=1,fps={fps}")])
        .args(["-r", &fps.to_string(), "-c:v", "libx264", "-preset", "ultrafast"])
        .args(["-x264opts", "bframes=0", "-threads", "4", "-pix_fmt", "yuv420p"])
        .args(["-color_range", "pc", "-an", "-f", "h264"])
        .arg(&output);
    no_window(&mut ffmpeg);
    info!("Encoding {} for the OLED", oled.file.display());
    let result = ffmpeg
        .output()
        .context("running ffmpeg (install it with: winget install Gyan.FFmpeg)")?;
    if !result.status.success() {
        bail!("ffmpeg failed: {}", String::from_utf8_lossy(&result.stderr).trim());
    }
    Ok(output)
}
