//! lianli-win: a small Windows background service for Lian Li hardware.
//!
//! It replaces the parts of L-Connect this build needs:
//!   * restores saved colours on every wireless fan group and Strimer at start-up,
//!     and re-sends them if a device drops its effect;
//!   * sets the HydroShift II OLED Curve ambient ring;
//!   * loops a video, GIF or image on the OLED screen (encoded with ffmpeg);
//!   * runs an OpenRGB SDK server so OpenRGB can control the Lian Li devices
//!     next to RAM, GPU and motherboard lighting.
//!
//! Usage: lianli_win [config.json]
//! Fan speed is left alone (the fans follow the motherboard PWM header).
//! L-Connect must not be running: WinUSB devices can only be opened by one program.

// Run without a console window; everything is logged next to the config file.
#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{bail, Context, Result};
use lianli_devices::wireless::{DiscoveredDevice, WirelessController, WirelessRgbUpload};
use lianli_devices::winusb::hs2_oled_led::Hs2OledLedController;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

// ---------------------------------------------------------------- config

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Config {
    /// OpenRGB SDK server port; 0 disables the server.
    #[serde(default = "default_port")]
    openrgb_port: u16,
    /// Per wireless device, keyed by MAC (see `lianli_cli list`).
    #[serde(default)]
    devices: BTreeMap<String, DeviceConfig>,
    /// Colour for the OLED's 45-LED ambient ring.
    #[serde(default)]
    oled_ring: Option<[u8; 3]>,
    /// Media to loop on the OLED screen.
    #[serde(default)]
    oled: Option<OledConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceConfig {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    color: Option<[u8; 3]>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OledConfig {
    file: PathBuf,
    #[serde(default = "default_fps")]
    fps: u32,
    /// "cover" (fill, crop edges) or "contain" (whole picture, black bars).
    #[serde(default = "default_fit")]
    fit: String,
}

fn default_port() -> u16 {
    6743
}
fn default_fps() -> u32 {
    30
}
fn default_fit() -> String {
    "cover".into()
}

fn load_config(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    // Windows PowerShell writes UTF-8 with a byte-order mark; serde_json rejects it.
    let text = text.trim_start_matches('\u{feff}');
    serde_json::from_str(text).with_context(|| format!("parsing {}", path.display()))
}

// ---------------------------------------------------------------- main

fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--selftest-openrgb") {
        return selftest_openrgb();
    }
    let config_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"H:\Claude\lconnect\lianli-win.json"));
    let log_path = config_path.with_extension("log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening log {}", log_path.display()))?;
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(log_file))
        .init();

    info!("lianli-win starting, config {}", config_path.display());
    let config = load_config(&config_path)?;

    wait_for_lconnect_to_exit();

    let stop = Arc::new(AtomicBool::new(false));
    let wireless = Arc::new(connect_wireless());
    let devices = wireless.devices();
    info!("{} wireless device(s) online", devices.len());

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

    let state = Arc::new(State::new(&devices, &config, ring.clone()));

    // Saved colours first, so the case lights up even before OpenRGB connects.
    for ctl in state.controllers.iter() {
        if let Some(color) = ctl.default_color {
            ctl.set_all(color);
        }
    }
    if let (Some(ring), Some(color)) = (&ring, config.oled_ring) {
        if let Err(e) = ring.send_rgb_frame(&[color; 45]) {
            warn!("OLED ring colour: {e:#}");
        }
    }

    {
        let wireless = wireless.clone();
        let state = state.clone();
        let stop = stop.clone();
        thread::Builder::new()
            .name("rgb-writer".into())
            .spawn(move || rgb_writer(&wireless, &state, &stop))?;
    }

    if let Some(oled) = config.oled.clone() {
        let stop = stop.clone();
        thread::Builder::new()
            .name("oled-video".into())
            .spawn(move || oled_loop(&oled, &stop))?;
    }

    if config.openrgb_port != 0 {
        let state = state.clone();
        let stop = stop.clone();
        let port = config.openrgb_port;
        thread::Builder::new()
            .name("openrgb".into())
            .spawn(move || {
                if let Err(e) = openrgb_server(port, &state, &stop) {
                    error!("OpenRGB server stopped: {e:#}");
                }
            })?;
    }

    info!("lianli-win running");
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

fn wait_for_lconnect_to_exit() {
    let mut warned = false;
    loop {
        let mut tasklist = std::process::Command::new("tasklist");
        tasklist.args(["/FI", "IMAGENAME eq L-Connect 3.exe", "/NH"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            tasklist.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let running = tasklist
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("L-Connect"))
            .unwrap_or(false);
        if !running {
            return;
        }
        if !warned {
            warn!("L-Connect is running; waiting for it to exit before taking over the devices");
            warned = true;
        }
        thread::sleep(Duration::from_secs(5));
    }
}

fn connect_wireless() -> WirelessController {
    loop {
        let mut wireless = WirelessController::new();
        let result = wireless
            .connect()
            .and_then(|()| wireless.start_polling())
            .context("starting the wireless dongle");
        match result {
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

// ---------------------------------------------------------------- shared state

enum Target {
    Wireless { mac: [u8; 6] },
    OledRing(Arc<Hs2OledLedController>),
}

struct Controller {
    name: String,
    device_type: u32,
    zones: Vec<(String, usize)>,
    target: Target,
    default_color: Option<[u8; 3]>,
    colors: Mutex<Vec<[u8; 3]>>,
    dirty: AtomicBool,
}

impl Controller {
    fn led_count(&self) -> usize {
        self.zones.iter().map(|(_, n)| n).sum()
    }

    fn set_all(&self, color: [u8; 3]) {
        let count = self.led_count();
        *self.colors.lock() = vec![color; count];
        self.dirty.store(true, Ordering::Release);
    }

    fn set_range(&self, start: usize, colors: &[[u8; 3]]) {
        let mut current = self.colors.lock();
        for (slot, color) in current.iter_mut().skip(start).zip(colors) {
            *slot = *color;
        }
        drop(current);
        self.dirty.store(true, Ordering::Release);
    }

    fn zone_start(&self, zone: usize) -> Option<usize> {
        (zone < self.zones.len()).then(|| self.zones[..zone].iter().map(|(_, n)| n).sum())
    }
}

struct State {
    controllers: Vec<Controller>,
}

// OpenRGB device types
const DEVICE_TYPE_COOLER: u32 = 3;
const DEVICE_TYPE_LEDSTRIP: u32 = 4;

impl State {
    fn new(devices: &[DiscoveredDevice], config: &Config, ring: Option<Arc<Hs2OledLedController>>) -> Self {
        let mut controllers = Vec::new();
        let mut sorted: Vec<&DiscoveredDevice> = devices.iter().collect();
        sorted.sort_by_key(|d| d.mac);
        for device in sorted {
            let saved = config.devices.get(&device.mac_str());
            let counts = device.fan_type.rgb_zone_led_counts(device.fan_count);
            if counts.iter().sum::<usize>() == 0 {
                continue;
            }
            let is_fan = device.fan_count > 0;
            let zones = counts
                .iter()
                .enumerate()
                .map(|(i, &n)| {
                    let label = if is_fan { format!("Fan {}", i + 1) } else { format!("Zone {}", i + 1) };
                    (label, n)
                })
                .collect();
            let fallback = format!("{} {}", device.fan_type.display_name(), &device.mac_str()[..8]);
            controllers.push(Controller {
                name: format!(
                    "Lian Li {}",
                    saved.and_then(|s| s.name.clone()).unwrap_or(fallback)
                ),
                device_type: if is_fan { DEVICE_TYPE_COOLER } else { DEVICE_TYPE_LEDSTRIP },
                zones,
                target: Target::Wireless { mac: device.mac },
                default_color: saved.and_then(|s| s.color),
                colors: Mutex::new(Vec::new()),
                dirty: AtomicBool::new(false),
            });
        }
        if let Some(ring) = ring {
            controllers.push(Controller {
                name: "Lian Li HydroShift II OLED Ring".into(),
                device_type: DEVICE_TYPE_COOLER,
                zones: vec![("Ring".into(), 45)],
                target: Target::OledRing(ring),
                default_color: config.oled_ring,
                colors: Mutex::new(Vec::new()),
                dirty: AtomicBool::new(false),
            });
        }
        for ctl in &controllers {
            let count = ctl.led_count();
            *ctl.colors.lock() = vec![[0, 0, 0]; count];
        }
        Self { controllers }
    }
}

// ---------------------------------------------------------------- colour delivery

/// Each wireless frame is a full RF upload, so live frames are limited to one
/// per device every 200 ms (same rate as the Linux daemon), with 150 ms between
/// uploads on the shared radio. The OLED ring is USB and updates immediately.
fn rgb_writer(wireless: &WirelessController, state: &State, stop: &AtomicBool) {
    const MIN_INTERVAL: Duration = Duration::from_millis(200);
    const SPACING: Duration = Duration::from_millis(150);
    const VERIFY_EVERY: Duration = Duration::from_secs(15);
    let mut last_sent: BTreeMap<[u8; 6], (Instant, WirelessRgbUpload)> = BTreeMap::new();
    let mut last_verify = Instant::now();

    while !stop.load(Ordering::Acquire) {
        let mut sent_any = false;
        for ctl in &state.controllers {
            if !ctl.dirty.load(Ordering::Acquire) {
                continue;
            }
            match &ctl.target {
                Target::OledRing(ring) => {
                    ctl.dirty.store(false, Ordering::Release);
                    let colors = ctl.colors.lock().clone();
                    if let Err(e) = ring.send_rgb_frame(&colors) {
                        warn!("OLED ring: {e:#}");
                    }
                }
                Target::Wireless { mac } => {
                    if last_sent.get(mac).is_some_and(|(at, _)| at.elapsed() < MIN_INTERVAL) {
                        continue;
                    }
                    ctl.dirty.store(false, Ordering::Release);
                    let colors = ctl.colors.lock().clone();
                    match wireless
                        .prepare_rgb_upload(mac, &[colors], 5000)
                        .and_then(|upload| wireless.send_rgb_upload(mac, &upload, 4).map(|()| upload))
                    {
                        Ok(upload) => {
                            last_sent.insert(*mac, (Instant::now(), upload));
                        }
                        Err(e) => {
                            warn!("{}: colour upload failed: {e:#}", ctl.name);
                            ctl.dirty.store(true, Ordering::Release);
                        }
                    }
                    sent_any = true;
                    thread::sleep(SPACING);
                }
            }
        }

        // Devices can drop an effect (missed packet, power blip): re-send what
        // they should be showing if they no longer report it.
        if last_verify.elapsed() >= VERIFY_EVERY {
            last_verify = Instant::now();
            for ctl in &state.controllers {
                if let Target::Wireless { mac } = &ctl.target {
                    if let Some((at, upload)) = last_sent.get(mac) {
                        if at.elapsed() > Duration::from_secs(5)
                            && !wireless.rgb_upload_applied(mac, upload.effect_index())
                        {
                            info!("{} lost its colours, re-sending", ctl.name);
                            ctl.dirty.store(true, Ordering::Release);
                        }
                    }
                }
            }
        }

        if !sent_any {
            thread::sleep(Duration::from_millis(30));
        }
    }
}

// ---------------------------------------------------------------- OLED video

fn oled_loop(oled: &OledConfig, stop: &AtomicBool) {
    use lianli_devices::winusb::lcd::WinUsbLcdDevice;
    let encoded = match encode_for_oled(oled) {
        Ok(path) => path,
        Err(e) => {
            error!("OLED media: {e:#}");
            return;
        }
    };
    while !stop.load(Ordering::Acquire) {
        let result = find_wired(0x1CBE, 0xA068)
            .and_then(|det| WinUsbLcdDevice::open(det.device, 0xA068))
            .and_then(|mut lcd| lcd.stream_h264(&encoded, true, stop, oled.fps as f32));
        match result {
            Ok(()) => {}
            Err(e) => warn!("OLED stream stopped: {e:#}; restarting in 5 s"),
        }
        thread::sleep(Duration::from_secs(5));
    }
}

/// Same pipeline as `lianli_cli oled-play`: fit to the 2288x1080 landscape
/// screen, rotate 90 into the panel's 1080x2288 frame, encode like the daemon.
/// The result is cached next to the config and reused while the source is unchanged.
fn encode_for_oled(oled: &OledConfig) -> Result<PathBuf> {
    anyhow::ensure!(oled.file.is_file(), "{} does not exist", oled.file.display());
    let fps = oled.fps.clamp(1, 60);
    let source_modified = std::fs::metadata(&oled.file)?.modified()?;
    let cache_dir = std::env::temp_dir().join("lianli-win");
    std::fs::create_dir_all(&cache_dir)?;
    let stem = oled
        .file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "media".into());
    let output = cache_dir.join(format!("{stem}-{}-{fps}.h264", oled.fit));
    if let Ok(meta) = std::fs::metadata(&output) {
        if meta.modified().is_ok_and(|m| m >= source_modified) && meta.len() > 0 {
            info!("Using cached OLED encode {}", output.display());
            return Ok(output);
        }
    }

    let fit = match oled.fit.as_str() {
        "contain" => "scale=2288:1080:force_original_aspect_ratio=decrease:flags=lanczos,pad=2288:1080:(ow-iw)/2:(oh-ih)/2:color=black",
        "cover" => "scale=2288:1080:force_original_aspect_ratio=increase:flags=lanczos,crop=2288:1080",
        other => bail!("oled.fit must be cover or contain, not `{other}`"),
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
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        ffmpeg.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    info!("Encoding {} for the OLED...", oled.file.display());
    let status = ffmpeg.status().context("running ffmpeg (winget install Gyan.FFmpeg)")?;
    anyhow::ensure!(status.success(), "ffmpeg failed with {status}");
    Ok(output)
}

// ---------------------------------------------------------------- OpenRGB SDK server

const MAGIC: &[u8; 4] = b"ORGB";
// Protocol 3 = modes with brightness. Newer clients negotiate down to it.
const SERVER_PROTOCOL: u32 = 3;
const PKT_REQUEST_CONTROLLER_COUNT: u32 = 0;
const PKT_REQUEST_CONTROLLER_DATA: u32 = 1;
const PKT_REQUEST_PROTOCOL_VERSION: u32 = 40;
const PKT_SET_CLIENT_NAME: u32 = 50;
const PKT_REQUEST_PROFILE_LIST: u32 = 150;
const PKT_REQUEST_PLUGIN_LIST: u32 = 200;
const PKT_RESIZE_ZONE: u32 = 1000;
const PKT_UPDATE_LEDS: u32 = 1050;
const PKT_UPDATE_ZONE_LEDS: u32 = 1051;
const PKT_UPDATE_SINGLE_LED: u32 = 1052;
const PKT_SET_CUSTOM_MODE: u32 = 1100;
const PKT_UPDATE_MODE: u32 = 1101;
const PKT_SAVE_MODE: u32 = 1102;
const MODE_FLAG_HAS_PER_LED_COLOR: u32 = 1 << 5;
const COLOR_MODE_PER_LED: u32 = 1;
const ZONE_TYPE_LINEAR: u32 = 1;
const MAX_PACKET: usize = 1024 * 1024;

fn openrgb_server(port: u16, state: &Arc<State>, stop: &Arc<AtomicBool>) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("binding OpenRGB SDK port {port}"))?;
    info!("OpenRGB SDK server listening on 127.0.0.1:{port}");
    for stream in listener.incoming() {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let Ok(stream) = stream else { continue };
        let state = state.clone();
        thread::spawn(move || {
            let peer = stream.peer_addr().ok();
            if let Err(e) = serve_client(stream, &state) {
                info!("OpenRGB client {peer:?} disconnected: {e}");
            }
        });
    }
    Ok(())
}

fn serve_client(mut stream: TcpStream, state: &State) -> Result<()> {
    stream.set_nodelay(true)?;
    let mut version = 0u32;
    loop {
        let mut header = [0u8; 16];
        stream.read_exact(&mut header)?;
        anyhow::ensure!(&header[..4] == MAGIC, "bad magic");
        let dev = u32::from_le_bytes(header[4..8].try_into()?) as usize;
        let id = u32::from_le_bytes(header[8..12].try_into()?);
        let size = u32::from_le_bytes(header[12..16].try_into()?) as usize;
        anyhow::ensure!(size <= MAX_PACKET, "packet too large");
        let mut payload = vec![0u8; size];
        stream.read_exact(&mut payload)?;

        match id {
            PKT_REQUEST_PROTOCOL_VERSION => {
                let client = payload.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
                version = client.min(SERVER_PROTOCOL);
                send(&mut stream, 0, id, &SERVER_PROTOCOL.to_le_bytes())?;
            }
            PKT_SET_CLIENT_NAME => {
                info!("OpenRGB client: {}", String::from_utf8_lossy(&payload).trim_end_matches('\0'));
            }
            PKT_REQUEST_CONTROLLER_COUNT => {
                send(&mut stream, 0, id, &(state.controllers.len() as u32).to_le_bytes())?;
            }
            PKT_REQUEST_CONTROLLER_DATA => {
                if let Some(ctl) = state.controllers.get(dev) {
                    let data = controller_data(ctl, version);
                    send(&mut stream, dev as u32, id, &data)?;
                }
            }
            PKT_REQUEST_PROFILE_LIST | PKT_REQUEST_PLUGIN_LIST => {
                send(&mut stream, 0, id, &[6, 0, 0, 0, 0, 0])?;
            }
            PKT_UPDATE_LEDS => {
                if let Some(ctl) = state.controllers.get(dev) {
                    let mut r = Reader::new(&payload);
                    r.skip(4)?;
                    ctl.set_range(0, &r.colors()?);
                }
            }
            PKT_UPDATE_ZONE_LEDS => {
                if let Some(ctl) = state.controllers.get(dev) {
                    let mut r = Reader::new(&payload);
                    r.skip(4)?;
                    let zone = r.u32()? as usize;
                    let colors = r.colors()?;
                    if let Some(start) = ctl.zone_start(zone) {
                        ctl.set_range(start, &colors);
                    }
                }
            }
            PKT_UPDATE_SINGLE_LED => {
                if let Some(ctl) = state.controllers.get(dev) {
                    let mut r = Reader::new(&payload);
                    let led = r.u32()? as usize;
                    let color = r.color()?;
                    ctl.set_range(led, &[color]);
                }
            }
            // Only a Direct (per-LED) mode is exposed, so mode and resize requests are no-ops.
            PKT_SET_CUSTOM_MODE | PKT_UPDATE_MODE | PKT_SAVE_MODE | PKT_RESIZE_ZONE => {}
            _ => {}
        }
    }
}

fn send(stream: &mut TcpStream, dev: u32, id: u32, payload: &[u8]) -> Result<()> {
    let mut packet = Vec::with_capacity(16 + payload.len());
    packet.extend_from_slice(MAGIC);
    packet.extend_from_slice(&dev.to_le_bytes());
    packet.extend_from_slice(&id.to_le_bytes());
    packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    packet.extend_from_slice(payload);
    stream.write_all(&packet)?;
    Ok(())
}

fn write_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&((s.len() + 1) as u16).to_le_bytes());
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
}

/// Controller description in the OpenRGB SDK layout (protocol <= 3).
fn controller_data(ctl: &Controller, version: u32) -> Vec<u8> {
    let mut buf = Vec::with_capacity(1024);
    buf.extend_from_slice(&0u32.to_le_bytes()); // total size, patched below
    buf.extend_from_slice(&ctl.device_type.to_le_bytes());
    write_string(&mut buf, &ctl.name);
    if version >= 1 {
        write_string(&mut buf, "Lian Li");
    }
    write_string(&mut buf, "Lian Li device via lianli-win");
    write_string(&mut buf, "1.0");
    write_string(&mut buf, &ctl.name);
    write_string(&mut buf, "Wireless / USB");

    // One mode: Direct, per-LED colour.
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&0i32.to_le_bytes()); // active mode
    write_string(&mut buf, "Direct");
    buf.extend_from_slice(&0u32.to_le_bytes()); // value
    buf.extend_from_slice(&MODE_FLAG_HAS_PER_LED_COLOR.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes()); // speed min
    buf.extend_from_slice(&0u32.to_le_bytes()); // speed max
    if version >= 3 {
        buf.extend_from_slice(&0u32.to_le_bytes()); // brightness min
        buf.extend_from_slice(&0u32.to_le_bytes()); // brightness max
    }
    buf.extend_from_slice(&0u32.to_le_bytes()); // colors min
    buf.extend_from_slice(&0u32.to_le_bytes()); // colors max
    buf.extend_from_slice(&0u32.to_le_bytes()); // speed
    if version >= 3 {
        buf.extend_from_slice(&0u32.to_le_bytes()); // brightness
    }
    buf.extend_from_slice(&0u32.to_le_bytes()); // direction
    buf.extend_from_slice(&COLOR_MODE_PER_LED.to_le_bytes());
    buf.extend_from_slice(&0u16.to_le_bytes()); // mode colours

    buf.extend_from_slice(&(ctl.zones.len() as u16).to_le_bytes());
    for (name, count) in &ctl.zones {
        write_string(&mut buf, name);
        buf.extend_from_slice(&ZONE_TYPE_LINEAR.to_le_bytes());
        for _ in 0..3 {
            buf.extend_from_slice(&(*count as u32).to_le_bytes()); // min, max, count
        }
        buf.extend_from_slice(&0u16.to_le_bytes()); // no matrix
    }

    let total = ctl.led_count();
    buf.extend_from_slice(&(total as u16).to_le_bytes());
    let mut index = 0u32;
    for (zone, count) in &ctl.zones {
        for i in 0..*count {
            write_string(&mut buf, &format!("{zone} LED {}", i + 1));
            buf.extend_from_slice(&index.to_le_bytes());
            index += 1;
        }
    }

    let colors = ctl.colors.lock().clone();
    buf.extend_from_slice(&(total as u16).to_le_bytes());
    for i in 0..total {
        let c = colors.get(i).copied().unwrap_or([0, 0, 0]);
        buf.extend_from_slice(&c);
        buf.push(0);
    }

    let size = buf.len() as u32;
    buf[..4].copy_from_slice(&size.to_le_bytes());
    buf
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).context("length overflow")?;
        let bytes = self.data.get(self.pos..end).context("truncated packet")?;
        self.pos = end;
        Ok(bytes)
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn color(&mut self) -> Result<[u8; 3]> {
        let c = self.take(4)?;
        Ok([c[0], c[1], c[2]])
    }
    fn colors(&mut self) -> Result<Vec<[u8; 3]>> {
        let n = u16::from_le_bytes(self.take(2)?.try_into()?) as usize;
        (0..n).map(|_| self.color()).collect()
    }
}

/// Hardware-free check of the OpenRGB server: serves fake controllers shaped
/// like this build (3+3+3+1 SL fans, Strimers, OLED ring) on port 6743.
fn selftest_openrgb() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let fan = |name: &str, fans: usize| Controller {
        name: format!("Lian Li {name}"),
        device_type: DEVICE_TYPE_COOLER,
        zones: (0..fans).map(|i| (format!("Fan {}", i + 1), 52)).collect(),
        target: Target::Wireless { mac: [0; 6] },
        default_color: None,
        colors: Mutex::new(vec![[0, 0, 0]; fans * 52]),
        dirty: AtomicBool::new(false),
    };
    let strip = |name: &str, leds: usize| Controller {
        name: format!("Lian Li {name}"),
        device_type: DEVICE_TYPE_LEDSTRIP,
        zones: vec![("Zone 1".into(), leds)],
        target: Target::Wireless { mac: [0; 6] },
        default_color: None,
        colors: Mutex::new(vec![[0, 0, 0]; leds]),
        dirty: AtomicBool::new(false),
    };
    let state = Arc::new(State {
        controllers: vec![fan("Top", 3), fan("Rear", 1), strip("Strimer 24-pin", 116)],
    });
    let stop = Arc::new(AtomicBool::new(false));
    {
        let state = state.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(1));
            for ctl in &state.controllers {
                if ctl.dirty.swap(false, Ordering::AcqRel) {
                    let c = ctl.colors.lock();
                    info!("{} <- first LED {:?}, {} LEDs", ctl.name, c.first(), c.len());
                }
            }
        });
    }
    openrgb_server(6743, &state, &stop)
}
