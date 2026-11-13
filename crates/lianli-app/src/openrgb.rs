//! Minimal OpenRGB SDK client. OpenRGB runs in the background as a server and
//! keeps its own drivers for RAM, GPU, motherboard and other non-Lian Li
//! lighting; RGBable lists those devices and sets their colors over the SDK.
//!
//! Protocol 6 (OpenRGB 1.0) is requested; older servers negotiate down. With
//! protocol 6 every command is acknowledged with a status code, so failures
//! are reported instead of being silently dropped.

use crate::effects::{Effect, KeyLayout};
use anyhow::{bail, Context, Result};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

const MAGIC: &[u8; 4] = b"ORGB";
const CLIENT_PROTOCOL: u32 = 6;
const PKT_ACK: u32 = 10;
const PKT_REQUEST_CONTROLLER_COUNT: u32 = 0;
const PKT_REQUEST_CONTROLLER_DATA: u32 = 1;
const PKT_REQUEST_PROTOCOL_VERSION: u32 = 40;
const PKT_SET_CLIENT_NAME: u32 = 50;
const PKT_UPDATE_LEDS: u32 = 1050;
const PKT_SET_CUSTOM_MODE: u32 = 1100;
const PKT_UPDATE_MODE: u32 = 1101;
const PKT_SAVE_MODE: u32 = 1102;
const MODE_FLAG_HAS_PER_LED_COLOR: u32 = 1 << 5;
const MODE_FLAG_HAS_MODE_SPECIFIC_COLOR: u32 = 1 << 6;
const MODE_FLAG_MANUAL_SAVE: u32 = 1 << 8;
const MODE_FLAG_AUTOMATIC_SAVE: u32 = 1 << 9;
const MODE_COLORS_PER_LED: u32 = 1;
const MODE_COLORS_MODE_SPECIFIC: u32 = 2;
const PKT_RESIZE_ZONE: u32 = 1000;
const PKT_REQUEST_RESCAN_DEVICES: u32 = 140;
pub const DEFAULT_PORTS: [u16; 2] = [6742, 6743];

#[derive(Debug, Clone, Serialize)]
pub struct OrgbDevice {
    /// Index for protocol < 6, controller ID for protocol 6+ (what packets address).
    pub index: u32,
    pub name: String,
    pub vendor: String,
    pub kind: &'static str,
    pub serial: String,
    pub location: String,
    pub leds: usize,
    pub zones: Vec<Zone>,
    pub modes: Vec<String>,
    /// The modes with their settings, to switch to one and store it on the device.
    #[serde(skip)]
    pub mode_settings: Vec<Mode>,
    pub active_mode: String,
    pub first_color: Option<[u8; 3]>,
    /// Where the keys sit, for devices whose zones come with a key grid (keyboards).
    #[serde(skip)]
    pub keys: Option<KeyLayout>,
}

/// How power-on lighting looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerOnStyle {
    Plain,
    Breathing,
    Off,
}

fn is_off_mode(name: &str) -> bool {
    matches!(name.to_lowercase().as_str(), "off" | "disable" | "disabled")
}

/// A hardware mode as OpenRGB describes it (RGBController::GetModeDescriptionData).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mode {
    pub name: String,
    value: u32,
    flags: u32,
    speed_min: u32,
    speed_max: u32,
    brightness_min: u32,
    brightness_max: u32,
    colors_min: u32,
    colors_max: u32,
    speed: u32,
    brightness: u32,
    direction: u32,
    color_mode: u32,
    colors: Vec<[u8; 3]>,
}

impl Mode {
    fn write(&self, out: &mut Vec<u8>, version: u32) {
        let bytes = self.name.as_bytes();
        out.extend(((bytes.len() + 1) as u16).to_le_bytes());
        out.extend(bytes);
        out.push(0);
        let mut put = |v: u32| out.extend(v.to_le_bytes());
        if version < 6 {
            put(self.value);
        }
        put(self.flags);
        put(self.speed_min);
        put(self.speed_max);
        if version >= 3 {
            put(self.brightness_min);
            put(self.brightness_max);
        }
        put(self.colors_min);
        put(self.colors_max);
        put(self.speed);
        if version >= 3 {
            put(self.brightness);
        }
        put(self.direction);
        put(self.color_mode);
        out.extend((self.colors.len() as u16).to_le_bytes());
        for c in &self.colors {
            out.extend([c[0], c[1], c[2], 0]);
        }
    }

    /// This mode showing `color`, when the device can remember it; per LED tells whether
    /// the color still has to go out to the LEDs.
    fn showing(&self, color: [u8; 3]) -> Option<(Mode, bool)> {
        if self.flags & (MODE_FLAG_MANUAL_SAVE | MODE_FLAG_AUTOMATIC_SAVE) == 0 {
            return None;
        }
        let mut mode = self.clone();
        if is_off_mode(&self.name) {
            return Some((mode, false));
        }
        if self.flags & MODE_FLAG_HAS_MODE_SPECIFIC_COLOR != 0 {
            let count = (self.colors.len() as u32)
                .max(self.colors_min)
                .max(1)
                .min(self.colors_max.max(1));
            mode.color_mode = MODE_COLORS_MODE_SPECIFIC;
            mode.colors = vec![color; count as usize];
            Some((mode, false))
        } else if self.flags & MODE_FLAG_HAS_PER_LED_COLOR != 0 {
            mode.color_mode = MODE_COLORS_PER_LED;
            Some((mode, true))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Zone {
    pub name: String,
    pub leds: usize,
    pub min: usize,
    pub max: usize,
}

impl Zone {
    pub fn resizable(&self) -> bool {
        self.min != self.max
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct OrgbView {
    pub connected: bool,
    pub port: Option<u16>,
    pub status: String,
    pub devices: Vec<OrgbDevice>,
}

pub struct OpenRgb {
    stream: Mutex<Option<(TcpStream, u32)>>,
    port: RwLock<Option<u16>>,
    status: RwLock<String>,
    devices: RwLock<Vec<OrgbDevice>>,
    listed_ids: RwLock<Vec<u32>>,
    connected_at: RwLock<Option<Instant>>,
    /// A write named a controller OpenRGB no longer has: its list needs reading again.
    stale: std::sync::atomic::AtomicBool,
    animations: Mutex<Animations>,
    /// Keyboards left alone while Vial has them: nothing is written to them.
    keyboards_held: std::sync::atomic::AtomicBool,
}

/// Effects OpenRGB devices play, keyed by controller index. `generation` changes whenever
/// the set changes, so the animator puts each device back into direct mode.
/// A run of a device's LEDs that follows one lighting: an effect drawn across `zones`
/// (LED counts), or a plain color.
#[derive(Debug, Clone, PartialEq)]
pub struct LedPart {
    pub zones: Vec<usize>,
    /// This part's keys, when it is (part of) a keyboard.
    pub keys: Option<KeyLayout>,
    pub effect: Option<Effect>,
    pub color: [u8; 3],
    pub delay_ms: u32,
    /// Its own white balance, or render::GLOBAL_WARMTH.
    pub warmth: i8,
    /// An ASUS Polymo panel on this header: which of its pictures shows the lighting.
    pub polymo: Option<Polymo>,
}

/// The pictures of an ASUS ROG Polymo panel. It sits on the board's first addressable
/// header: LEDs 1-11 light the ROG logo in five sections, LEDs 12-19 the HERO logo in four
/// (`Diamonds`, kept as the saved name); a picture whose LEDs are black is off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Polymo {
    Logo,
    Diamonds,
    Both,
}

impl Polymo {
    const LOGO: [std::ops::Range<usize>; 5] = [0..3, 3..5, 5..7, 7..9, 9..11];
    const DIAMONDS: [std::ops::Range<usize>; 4] = [11..13, 13..15, 15..17, 17..19];

    fn sections(self) -> Vec<std::ops::Range<usize>> {
        match self {
            Self::Logo => Self::LOGO.to_vec(),
            Self::Diamonds => Self::DIAMONDS.to_vec(),
            Self::Both => Self::LOGO.iter().chain(&Self::DIAMONDS).cloned().collect(),
        }
    }

    /// The header's LEDs with `colors` (one per section) on the chosen picture.
    fn paint(self, colors: &[[u8; 3]], leds: usize) -> Vec<[u8; 3]> {
        let mut out = vec![[0; 3]; leds];
        for (range, color) in self.sections().into_iter().zip(colors) {
            for led in range.filter(|&l| l < leds) {
                out[led] = *color;
            }
        }
        out
    }
}

impl LedPart {
    pub fn leds(&self) -> usize {
        self.zones.iter().sum()
    }
}

/// The device frame `elapsed_ms` into its effects.
pub fn compose(parts: &[LedPart], elapsed_ms: i64) -> Vec<[u8; 3]> {
    let mut frame = Vec::new();
    for part in parts {
        let start = frame.len();
        if let Some(polymo) = part.polymo {
            let sections = polymo.sections().len();
            let colors = match part.effect.as_ref().filter(|e| e.is_animated()) {
                Some(effect) => {
                    let period = u64::from(effect.period_ms().max(1));
                    let behind = elapsed_ms - i64::from(part.delay_ms);
                    let phase = behind.rem_euclid(period as i64) as f32 / period as f32;
                    effect.frame(&[sections], phase)
                }
                None => vec![part.color; sections],
            };
            frame.extend(polymo.paint(&colors, part.leds()));
            crate::render::balance_with(&mut frame[start..], part.warmth);
            continue;
        }
        // Lian Li's own effects play L-Connect's frames for a strip of this length, so they
        // look the same as on the Lian Li devices
        let strip = part
            .effect
            .as_ref()
            .filter(|_| part.keys.is_none())
            .and_then(|e| crate::render::strip_frames(e, part.leds()));
        if let Some(strip) = strip {
            let behind = (elapsed_ms - i64::from(part.delay_ms)).rem_euclid(strip.loop_ms());
            let at = (behind / i64::from(strip.interval_ms)) as usize % strip.frames.len();
            frame.extend(strip.frames[at].iter().copied());
            crate::render::balance_with(&mut frame[start..], part.warmth);
            continue;
        }
        match part.effect.as_ref().filter(|e| e.is_animated()) {
            Some(effect) => {
                let period = u64::from(effect.period_ms().max(1));
                let behind = elapsed_ms - i64::from(part.delay_ms);
                let phase = behind.rem_euclid(period as i64) as f32 / period as f32;
                match part.keys.as_ref().filter(|_| effect.kind.is_keyboard()) {
                    Some(keys) => frame.extend(effect.frame_keys(keys, phase)),
                    None => frame.extend(effect.frame(&part.zones, phase)),
                }
            }
            None => frame.extend(std::iter::repeat_n(part.color, part.leds())),
        }
        crate::render::balance_with(&mut frame[start..], part.warmth);
    }
    frame
}

#[derive(Default)]
struct Animations {
    effects: BTreeMap<u32, Vec<LedPart>>,
    generation: u64,
}

const ANIMATION_FRAME: Duration = Duration::from_millis(50);

impl OpenRgb {
    pub fn new() -> Self {
        Self {
            stream: Mutex::new(None),
            port: RwLock::new(None),
            status: RwLock::new("Not connected".into()),
            connected_at: RwLock::new(None),
            stale: std::sync::atomic::AtomicBool::new(false),
            devices: RwLock::new(Vec::new()),
            listed_ids: RwLock::new(Vec::new()),
            animations: Mutex::new(Animations::default()),
            keyboards_held: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Stops or resumes writing to keyboards (while Vial edits one, RGBable's frames would
    /// fight its changes). Returns whether that changed anything.
    pub fn hold_keyboards(&self, held: bool) -> bool {
        use std::sync::atomic::Ordering;
        if self.keyboards_held.swap(held, Ordering::AcqRel) == held {
            return false;
        }
        if !held {
            // back into direct mode on the next frame, in case Vial changed the mode
            self.animations.lock().generation += 1;
        }
        true
    }

    fn is_held(&self, index: u32) -> bool {
        self.keyboards_held
            .load(std::sync::atomic::Ordering::Acquire)
            && self
                .devices
                .read()
                .iter()
                .any(|d| d.index == index && d.kind == "keyboard")
    }

    pub fn view(&self) -> OrgbView {
        OrgbView {
            connected: self.stream.lock().is_some(),
            port: *self.port.read(),
            status: self.status.read().clone(),
            devices: self.devices.read().clone(),
        }
    }

    /// Connect to the first OpenRGB server that answers, preferring `port`.
    /// A server with no devices is skipped, since that is usually not the
    /// OpenRGB instance that owns the hardware.
    pub fn connect(&self, preferred: Option<u16>) -> Result<()> {
        let mut ports: Vec<u16> = preferred.into_iter().collect();
        ports.extend(DEFAULT_PORTS.iter().filter(|p| Some(**p) != preferred));
        let mut last_error = None;
        for port in ports {
            match Self::open(port) {
                Ok((mut stream, version)) => match list_devices(&mut stream, version) {
                    Ok((ids, devices)) if !devices.is_empty() => {
                        info!(
                            "OpenRGB on port {port}: {} device(s), protocol {version}",
                            devices.len()
                        );
                        *self.status.write() = format!("Connected to OpenRGB on port {port}");
                        *self.port.write() = Some(port);
                        *self.devices.write() = devices;
                        *self.listed_ids.write() = ids;
                        *self.connected_at.write() = Some(Instant::now());
                        self.stale
                            .store(false, std::sync::atomic::Ordering::Release);
                        // when OpenRGB's list changes, its controllers can get new IDs:
                        // effects for IDs it no longer has would only fail, every frame
                        {
                            let current: BTreeSet<u32> =
                                self.devices.read().iter().map(|d| d.index).collect();
                            let mut animations = self.animations.lock();
                            let before = animations.effects.len();
                            animations
                                .effects
                                .retain(|index, _| current.contains(index));
                            if animations.effects.len() != before {
                                animations.generation += 1;
                            }
                        }
                        *self.stream.lock() = Some((stream, version));
                        return Ok(());
                    }
                    Ok(_) => {
                        last_error = Some(anyhow::anyhow!("OpenRGB on port {port} has no devices"))
                    }
                    Err(e) => last_error = Some(e),
                },
                Err(e) => last_error = Some(e),
            }
        }
        *self.stream.lock() = None;
        *self.port.write() = None;
        self.devices.write().clear();
        let error = last_error.unwrap_or_else(|| anyhow::anyhow!("no OpenRGB server found"));
        *self.status.write() = format!(
            "OpenRGB not found. In OpenRGB: Settings > start the SDK server (port 6742). ({error:#})"
        );
        Err(error)
    }

    fn open(port: u16) -> Result<(TcpStream, u32)> {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = TcpStream::connect_timeout(&address, Duration::from_millis(800))
            .with_context(|| format!("connecting to 127.0.0.1:{port}"))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        send(&mut stream, 0, PKT_SET_CLIENT_NAME, b"RGBable\0")?;
        send(
            &mut stream,
            0,
            PKT_REQUEST_PROTOCOL_VERSION,
            &CLIENT_PROTOCOL.to_le_bytes(),
        )?;
        let reply = expect(&mut stream, PKT_REQUEST_PROTOCOL_VERSION)?;
        let server = reply
            .get(..4)
            .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
        Ok((stream, server.min(CLIENT_PROTOCOL)))
    }

    /// True when OpenRGB now lists different controllers than were read at connect. OpenRGB
    /// starts its server before detection finishes, and devices can be plugged in later.
    pub fn is_connected(&self) -> bool {
        self.stream.lock().is_some()
    }

    pub fn is_stale(&self) -> bool {
        self.stale.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn device_names(&self) -> Vec<String> {
        self.devices.read().iter().map(|d| d.name.clone()).collect()
    }

    /// How long ago RGBable last (re)read OpenRGB's devices.
    pub fn connected_for(&self) -> Option<Duration> {
        self.connected_at.read().map(|at| at.elapsed())
    }

    /// Asks OpenRGB to look for devices again; it only looks once when it starts, so a device
    /// that wasn't ready that early in boot stays missing until asked. The device list check
    /// notices what it finds.
    pub fn rescan(&self) -> Result<()> {
        let mut guard = self.stream.lock();
        let (stream, _) = guard.as_mut().context("not connected to OpenRGB")?;
        send(stream, 0, PKT_REQUEST_RESCAN_DEVICES, &[])
    }

    pub fn device_list_changed(&self) -> Result<bool> {
        if self.stale.load(std::sync::atomic::Ordering::Acquire) {
            anyhow::ensure!(self.is_connected(), "not connected to OpenRGB");
            return Ok(true);
        }
        let mut guard = self.stream.lock();
        let (stream, version) = guard.as_mut().context("not connected to OpenRGB")?;
        let version = *version;
        send(stream, 0, PKT_REQUEST_CONTROLLER_COUNT, &[])?;
        let reply = expect(stream, PKT_REQUEST_CONTROLLER_COUNT)?;
        let ids = controller_ids(&reply, version);
        drop(guard);
        let known = self.listed_ids.read().clone();
        Ok(ids != known)
    }

    /// Re-read one controller from the server (to verify what it applied).
    pub fn refresh(&self, index: u32) -> Result<OrgbDevice> {
        let mut guard = self.stream.lock();
        let Some((stream, version)) = guard.as_mut() else {
            bail!("OpenRGB is not connected")
        };
        let version = *version;
        let request: Vec<u8> = if version >= 6 {
            Vec::new()
        } else {
            version.to_le_bytes().to_vec()
        };
        send(stream, index, PKT_REQUEST_CONTROLLER_DATA, &request)?;
        let data = expect(stream, PKT_REQUEST_CONTROLLER_DATA)?;
        parse_controller(index, &data, version)
    }

    /// Put the device in its custom/direct mode and paint every LED one color.
    pub fn set_color(&self, index: u32, color: [u8; 3]) -> Result<()> {
        let leds = self.led_count(index)?;
        self.set_leds(index, &vec![color; leds], true)
    }

    pub fn led_count(&self, index: u32) -> Result<usize> {
        self.devices
            .read()
            .iter()
            .find(|d| d.index == index)
            .map(|d| d.leds)
            .context("unknown OpenRGB device")
    }

    /// Writes one color per LED; `custom_mode` first switches the device to direct control.
    pub fn set_leds(&self, index: u32, colors: &[[u8; 3]], custom_mode: bool) -> Result<()> {
        if self.is_held(index) {
            return Ok(());
        }
        let leds = colors.len();
        let mut guard = self.stream.lock();
        let Some((stream, version)) = guard.as_mut() else {
            bail!("OpenRGB is not connected")
        };
        let version = *version;
        let mut payload = Vec::with_capacity(6 + leds * 4);
        payload.extend_from_slice(&((6 + leds * 4) as u32).to_le_bytes());
        payload.extend_from_slice(&(leds as u16).to_le_bytes());
        for color in colors {
            payload.extend_from_slice(color);
            payload.push(0);
        }
        let mode = if custom_mode {
            send(stream, index, PKT_SET_CUSTOM_MODE, &[])
                .and_then(|()| expect_ack(stream, version, PKT_SET_CUSTOM_MODE))
        } else {
            Ok(())
        };
        let result = mode
            .and_then(|()| send(stream, index, PKT_UPDATE_LEDS, &payload))
            .and_then(|()| expect_ack(stream, version, PKT_UPDATE_LEDS));
        if let Err(e) = &result {
            let text = e.to_string();
            if text.contains("invalid device ID") || text.contains("invalid data") {
                // OpenRGB changed its device list or a zone's size (a rescan, a late device,
                // its saved sizes loading): the connection is fine, the list needs reading again
                if !self.stale.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    info!(
                        "OpenRGB's controller {index} changed ({text}); reading its devices again"
                    );
                }
            } else {
                warn!("OpenRGB write failed, disconnecting: {e:#}");
                *guard = None;
                *self.status.write() = "Disconnected from OpenRGB".into();
            }
        }
        result
    }

    /// Sets how many LEDs an addressable header zone drives; OpenRGB saves it in its own settings.
    pub fn resize_zone(&self, index: u32, zone: usize, size: usize) -> Result<()> {
        {
            let mut guard = self.stream.lock();
            let Some((stream, version)) = guard.as_mut() else {
                bail!("OpenRGB is not connected")
            };
            let version = *version;
            let mut payload = Vec::with_capacity(8);
            payload.extend_from_slice(&(zone as i32).to_le_bytes());
            payload.extend_from_slice(&(size as i32).to_le_bytes());
            send(stream, index, PKT_RESIZE_ZONE, &payload)?;
            expect_ack(stream, version, PKT_RESIZE_ZONE)?;
        }
        let device = self.refresh(index)?;
        if let Some(slot) = self.devices.write().iter_mut().find(|d| d.index == index) {
            *slot = device;
        }
        Ok(())
    }

    /// Replaces the animated effects; returns once no frame of a removed effect can still be sent.
    pub fn set_animation(&self, index: u32, parts: Vec<LedPart>) {
        let mut animations = self.animations.lock();
        if animations.effects.get(&index) != Some(&parts) {
            animations.effects.insert(index, parts);
            animations.generation += 1;
        }
    }

    /// Makes the animator switch every device back to direct mode on its next frame.
    pub fn reassert_direct(&self) {
        self.animations.lock().generation += 1;
    }

    /// Stores `color` in the device's own memory as a plain or breathing hardware mode, so
    /// it shows from power-on until RGBable takes over. False when the device can't
    /// remember a mode like that. Leaves the device in that mode: put it back in direct
    /// mode afterwards.
    pub fn save_power_on(&self, index: u32, color: [u8; 3], style: PowerOnStyle) -> Result<bool> {
        const PLAIN: &[&str] = &["static", "fixed", "solid"];
        let tries: Vec<(&[&str], [u8; 3])> = match style {
            PowerOnStyle::Plain => vec![(PLAIN, color)],
            PowerOnStyle::Breathing => vec![(&["breathing", "breathe", "pulse"], color)],
            // the device's own off mode when it has one, else plain black
            PowerOnStyle::Off => vec![
                (&["off", "disable", "disabled"], [0, 0, 0]),
                (PLAIN, [0, 0, 0]),
            ],
        };
        for (names, color) in tries {
            if self.save_power_on_as(index, color, names)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Stores the first of the modes named `names` the device can remember; false when it
    /// has none of them.
    fn save_power_on_as(&self, index: u32, color: [u8; 3], names: &[&str]) -> Result<bool> {
        let Some(device) = self
            .devices
            .read()
            .iter()
            .find(|d| d.index == index)
            .cloned()
        else {
            bail!("OpenRGB has no device {index}");
        };
        let Some((mode_index, (mode, per_led))) = device
            .mode_settings
            .iter()
            .enumerate()
            .filter(|(_, m)| names.contains(&m.name.to_lowercase().as_str()))
            .find_map(|(i, m)| m.showing(color).map(|shown| (i, shown)))
        else {
            return Ok(false);
        };
        let mut guard = self.stream.lock();
        let (stream, version) = guard.as_mut().context("not connected to OpenRGB")?;
        let version = *version;
        anyhow::ensure!(version >= 3, "OpenRGB is too old to store modes on devices");
        let mut payload = Vec::new();
        payload.extend((mode_index as i32).to_le_bytes());
        mode.write(&mut payload, version);
        let mut packet = Vec::with_capacity(payload.len() + 4);
        packet.extend(((payload.len() + 4) as u32).to_le_bytes());
        packet.extend(payload);
        send(stream, index, PKT_UPDATE_MODE, &packet)?;
        expect_ack(stream, version, PKT_UPDATE_MODE)?;
        if per_led {
            let mut leds = Vec::with_capacity(6 + device.leds * 4);
            leds.extend(((6 + device.leds * 4) as u32).to_le_bytes());
            leds.extend((device.leds as u16).to_le_bytes());
            for _ in 0..device.leds {
                leds.extend([color[0], color[1], color[2], 0]);
            }
            send(stream, index, PKT_UPDATE_LEDS, &leds)?;
            expect_ack(stream, version, PKT_UPDATE_LEDS)?;
        }
        if mode.flags & MODE_FLAG_MANUAL_SAVE != 0 {
            send(stream, index, PKT_SAVE_MODE, &packet)?;
            expect_ack(stream, version, PKT_SAVE_MODE)?;
        }
        Ok(true)
    }

    pub fn stop_animation(&self, index: u32) {
        let mut animations = self.animations.lock();
        if animations.effects.remove(&index).is_some() {
            animations.generation += 1;
        }
    }

    /// Plays animated effects at about 20 frames a second; idles while there are none.
    pub fn start_animator(self: &Arc<Self>) {
        let openrgb = self.clone();
        std::thread::Builder::new()
            .name("openrgb-effects".into())
            .spawn(move || {
                // OpenRGB finishes finding slow devices (GPUs especially) after it starts its
                // server and may then load its own profile onto them, taking them out of
                // direct mode; for a while after connecting, direct mode is asked for again.
                const SETTLING: Duration = Duration::from_secs(180);
                const REASSERT_EVERY: Duration = Duration::from_secs(10);
                let mut generation = u64::MAX;
                let mut in_direct_mode = BTreeSet::new();
                let mut last_reassert = Instant::now();
                loop {
                    let tick = Instant::now();
                    let connected = openrgb.stream.lock().is_some();
                    let animations = openrgb.animations.lock();
                    if !connected || animations.effects.is_empty() {
                        drop(animations);
                        generation = u64::MAX;
                        std::thread::sleep(Duration::from_millis(500));
                        continue;
                    }
                    let settling = openrgb
                        .connected_at
                        .read()
                        .is_some_and(|at| at.elapsed() < SETTLING);
                    if animations.generation != generation
                        || (settling && last_reassert.elapsed() >= REASSERT_EVERY)
                    {
                        generation = animations.generation;
                        last_reassert = Instant::now();
                        in_direct_mode.clear();
                    }
                    let elapsed = crate::render::clock_ms();
                    for (&index, parts) in &animations.effects {
                        let first = in_direct_mode.insert(index);
                        if openrgb
                            .set_leds(index, &compose(parts, elapsed), first)
                            .is_err()
                        {
                            in_direct_mode.remove(&index);
                        }
                    }
                    drop(animations);
                    std::thread::sleep(ANIMATION_FRAME.saturating_sub(tick.elapsed()));
                }
            })
            .expect("spawn OpenRGB effects thread");
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

/// Protocol 6 acknowledges every packet; check the status for `id`.
fn expect_ack(stream: &mut TcpStream, version: u32, id: u32) -> Result<()> {
    if version < 6 {
        return Ok(());
    }
    loop {
        let ack = expect(stream, PKT_ACK)?;
        anyhow::ensure!(ack.len() >= 8, "short ACK");
        let acked = u32::from_le_bytes(ack[..4].try_into()?);
        let status = u32::from_le_bytes(ack[4..8].try_into()?);
        if acked != id {
            continue; // ACK for an earlier packet (client name, flags, ...)
        }
        return match status {
            0 => Ok(()),
            2 => bail!("OpenRGB: command not supported"),
            3 => bail!("OpenRGB: command not allowed"),
            4 => bail!("OpenRGB: invalid device ID"),
            5 => bail!("OpenRGB: invalid data"),
            other => bail!("OpenRGB: error status {other}"),
        };
    }
}

/// Read packets until one with `id` arrives (servers may interleave notifications).
fn expect(stream: &mut TcpStream, id: u32) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        anyhow::ensure!(Instant::now() < deadline, "OpenRGB did not answer");
        let mut header = [0u8; 16];
        stream.read_exact(&mut header)?;
        anyhow::ensure!(&header[..4] == MAGIC, "not an OpenRGB server");
        let kind = u32::from_le_bytes(header[8..12].try_into()?);
        let size = u32::from_le_bytes(header[12..16].try_into()?) as usize;
        anyhow::ensure!(size <= 8 * 1024 * 1024, "OpenRGB packet too large");
        let mut payload = vec![0u8; size];
        stream.read_exact(&mut payload)?;
        if kind == id {
            return Ok(payload);
        }
    }
}

/// Protocol 6 addresses controllers by ID (listed after the count), older ones by index.
fn controller_ids(reply: &[u8], version: u32) -> Vec<u32> {
    let count = reply
        .get(..4)
        .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
        .min(256);
    if version >= 6 {
        (0..count as usize)
            .filter_map(|i| reply.get(4 + i * 4..8 + i * 4))
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .collect()
    } else {
        (0..count).collect()
    }
}

fn list_devices(stream: &mut TcpStream, version: u32) -> Result<(Vec<u32>, Vec<OrgbDevice>)> {
    send(stream, 0, PKT_REQUEST_CONTROLLER_COUNT, &[])?;
    let reply = expect(stream, PKT_REQUEST_CONTROLLER_COUNT)?;
    let ids = controller_ids(&reply, version);
    let mut devices = Vec::new();
    for &index in &ids {
        let request: Vec<u8> = if version >= 6 {
            Vec::new()
        } else {
            version.to_le_bytes().to_vec()
        };
        send(stream, index, PKT_REQUEST_CONTROLLER_DATA, &request)?;
        let data = expect(stream, PKT_REQUEST_CONTROLLER_DATA)?;
        match parse_controller(index, &data, version) {
            Ok(device) => devices.push(device),
            Err(e) => warn!("OpenRGB device {index}: {e:#}"),
        }
    }
    Ok((ids, devices))
}

/// Where each LED sits on the zones' key grids, scaled to 0..1 across and down, and which are
/// the gaming keys. LEDs off every grid go along the bottom.
fn key_layout(
    zones: &[Zone],
    grids: &[Option<(usize, usize, Vec<u32>)>],
    names: &[String],
) -> Option<KeyLayout> {
    if grids.iter().all(Option::is_none) {
        return None;
    }
    let total = names.len().max(zones.iter().map(|z| z.leds).sum());
    let mut points: Vec<Option<[f32; 2]>> = vec![None; total];
    let mut offset = 0;
    for (zone, grid) in zones.iter().zip(grids) {
        if let Some((height, width, map)) = grid {
            for (cell, &led) in map.iter().enumerate() {
                let index = offset + led as usize;
                if led == u32::MAX || led as usize >= zone.leds || index >= total {
                    continue;
                }
                let (row, column) = (cell / width, cell % width);
                let across = column as f32 / (*width - 1).max(1) as f32;
                let down = row as f32 / (*height - 1).max(1) as f32;
                points[index].get_or_insert([across, down]);
            }
        }
        offset += zone.leds;
    }
    const GAMING: [&str; 8] = [
        "W",
        "A",
        "S",
        "D",
        "Up Arrow",
        "Down Arrow",
        "Left Arrow",
        "Right Arrow",
    ];
    let highlighted = (0..total)
        .map(|i| {
            names.get(i).is_some_and(|name| {
                let key = name.strip_prefix("Key: ").unwrap_or(name);
                GAMING.iter().any(|g| key.eq_ignore_ascii_case(g))
            })
        })
        .collect();
    let points = points
        .into_iter()
        .enumerate()
        .map(|(i, p)| p.unwrap_or([i as f32 / total.max(1) as f32, 1.0]))
        .collect();
    Some(KeyLayout {
        points,
        highlighted,
    })
}

fn kind_name(kind: u32) -> &'static str {
    match kind {
        0 => "motherboard",
        1 => "ram",
        2 => "gpu",
        3 => "cooler",
        4 => "ledstrip",
        5 => "keyboard",
        6 => "mouse",
        7 => "mousemat",
        8 => "headset",
        9 => "headset-stand",
        10 => "gamepad",
        11 => "light",
        12 => "speaker",
        13 => "virtual",
        14 => "storage",
        15 => "case",
        16 => "microphone",
        17 => "accessory",
        18 => "keypad",
        _ => "other",
    }
}

/// Parse a controller description; layout follows OpenRGB's
/// RGBController::GetDeviceDescriptionData for protocol versions 1 to 6.
fn parse_controller(index: u32, data: &[u8], version: u32) -> Result<OrgbDevice> {
    let mut r = Reader { data, pos: 0 };
    r.u32()?; // data size
    let kind = r.u32()?;
    let name = r.string()?;
    let vendor = if version >= 1 {
        r.string()?
    } else {
        String::new()
    };
    r.string()?; // description
    r.string()?; // version
    let serial = r.string()?;
    let location = r.string()?;
    let mode_count = r.u16()?;
    let active = r.u32()? as usize;
    let mut mode_settings = Vec::new();
    for _ in 0..mode_count {
        mode_settings.push(read_mode(&mut r, version)?);
    }
    let modes: Vec<String> = mode_settings.iter().map(|m| m.name.clone()).collect();
    let zone_count = r.u16()?;
    let mut zones = Vec::new();
    let mut grids: Vec<Option<(usize, usize, Vec<u32>)>> = Vec::new();
    for _ in 0..zone_count {
        let zone_name = r.string()?;
        r.skip(4)?; // type
        let min = r.u32()? as usize;
        let max = r.u32()? as usize;
        let leds = r.u32()? as usize;
        let matrix = r.u16()? as usize;
        grids.push(if matrix >= 8 {
            let height = r.u32()? as usize;
            let width = r.u32()? as usize;
            let cells = (matrix - 8) / 4;
            let mut map = Vec::with_capacity(cells);
            for _ in 0..cells {
                map.push(r.u32()?);
            }
            (height * width == cells && height > 0 && width > 0).then_some((height, width, map))
        } else {
            r.skip(matrix)?;
            None
        });
        if version >= 4 {
            let segments = r.u16()?;
            for _ in 0..segments {
                r.string()?; // name
                r.skip(4 * 3)?; // type, start, count
                if version >= 6 {
                    let matrix = r.u16()? as usize;
                    r.skip(matrix)?;
                    r.skip(4)?; // flags
                }
            }
        }
        if version >= 5 {
            r.skip(4)?; // zone flags
        }
        if version >= 6 {
            r.skip(4)?; // zone active mode
            let zone_modes = r.u16()?;
            for _ in 0..zone_modes {
                read_mode(&mut r, version)?;
            }
            r.string()?; // zone display name
        }
        zones.push(Zone {
            name: zone_name,
            leds,
            min,
            max,
        });
    }
    let leds = r.u16()? as usize;
    let mut led_names = Vec::with_capacity(leds);
    for _ in 0..leds {
        led_names.push(r.string()?);
        if version < 6 {
            r.u32()?; // LED value
        }
    }
    let keys = key_layout(&zones, &grids, &led_names);
    let colors = r.u16()? as usize;
    let first_color = if colors > 0 {
        let c = r.take(4)?;
        Some([c[0], c[1], c[2]])
    } else {
        None
    };
    let active_mode = modes
        .get(active)
        .cloned()
        .unwrap_or_else(|| format!("#{active}"));
    Ok(OrgbDevice {
        index,
        name,
        vendor,
        kind: kind_name(kind),
        serial,
        location,
        leds,
        zones,
        modes,
        mode_settings,
        active_mode,
        first_color,
        keys,
    })
}

/// One mode entry (RGBController::GetModeDescriptionData).
fn read_mode(r: &mut Reader<'_>, version: u32) -> Result<Mode> {
    let mut mode = Mode {
        name: r.string()?,
        ..Default::default()
    };
    if version < 6 {
        mode.value = r.u32()?;
    }
    mode.flags = r.u32()?;
    mode.speed_min = r.u32()?;
    mode.speed_max = r.u32()?;
    if version >= 3 {
        mode.brightness_min = r.u32()?;
        mode.brightness_max = r.u32()?;
    }
    mode.colors_min = r.u32()?;
    mode.colors_max = r.u32()?;
    mode.speed = r.u32()?;
    if version >= 3 {
        mode.brightness = r.u32()?;
    }
    mode.direction = r.u32()?;
    mode.color_mode = r.u32()?;
    let colors = r.u16()? as usize;
    for _ in 0..colors {
        let c = r.take(4)?;
        mode.colors.push([c[0], c[1], c[2]]);
    }
    Ok(mode)
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self.pos.checked_add(n).context("length overflow")?;
        let bytes = self
            .data
            .get(self.pos..end)
            .context("truncated controller data")?;
        self.pos = end;
        Ok(bytes)
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }
    fn string(&mut self) -> Result<String> {
        let n = self.u16()? as usize;
        let bytes = self.take(n)?;
        Ok(String::from_utf8_lossy(bytes)
            .trim_end_matches('\0')
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn polymo_lights_only_the_chosen_picture() {
        let part = |polymo| LedPart {
            zones: vec![28],
            keys: None,
            effect: None,
            color: [255, 0, 0],
            delay_ms: 0,
            warmth: 0,
            polymo,
        };
        let logo = compose(&[part(Some(Polymo::Logo))], 0);
        assert_eq!(logo.len(), 28);
        assert!(logo[..11].iter().all(|c| c[0] > 0));
        assert!(logo[11..].iter().all(|c| *c == [0, 0, 0]));
        let diamonds = compose(&[part(Some(Polymo::Diamonds))], 0);
        assert!(diamonds[..11].iter().all(|c| *c == [0, 0, 0]));
        assert!(diamonds[11..19].iter().all(|c| c[0] > 0));
        assert!(diamonds[19..].iter().all(|c| *c == [0, 0, 0]));
        let both = compose(&[part(Some(Polymo::Both))], 0);
        assert!(both[..19].iter().all(|c| c[0] > 0));
    }

    use super::*;

    #[test]
    fn modes_are_written_back_the_way_they_were_read() {
        let mode = Mode {
            name: "Breathing".into(),
            value: 3,
            flags: MODE_FLAG_HAS_MODE_SPECIFIC_COLOR | MODE_FLAG_MANUAL_SAVE,
            speed_min: 1,
            speed_max: 5,
            brightness_min: 0,
            brightness_max: 100,
            colors_min: 1,
            colors_max: 2,
            speed: 3,
            brightness: 100,
            direction: 0,
            color_mode: MODE_COLORS_MODE_SPECIFIC,
            colors: vec![[255, 0, 128]],
        };
        for version in [1, 3, 5, 6] {
            let mut data = Vec::new();
            mode.write(&mut data, version);
            let mut r = Reader {
                data: &data,
                pos: 0,
            };
            let back = read_mode(&mut r, version).unwrap();
            assert_eq!(r.pos, data.len(), "version {version}");
            let mut expected = mode.clone();
            if version < 3 {
                (
                    expected.brightness_min,
                    expected.brightness_max,
                    expected.brightness,
                ) = (0, 0, 0);
            }
            if version >= 6 {
                expected.value = 0;
            }
            assert_eq!(back, expected, "version {version}");
        }
    }

    #[test]
    fn only_modes_a_device_remembers_can_show_a_power_on_color() {
        let mut mode = Mode {
            name: "Static".into(),
            flags: MODE_FLAG_HAS_MODE_SPECIFIC_COLOR,
            colors_min: 1,
            colors_max: 1,
            ..Default::default()
        };
        assert!(mode.showing([1, 2, 3]).is_none());
        mode.flags |= MODE_FLAG_AUTOMATIC_SAVE;
        let (shown, per_led) = mode.showing([1, 2, 3]).unwrap();
        assert_eq!(
            (shown.colors, shown.color_mode, per_led),
            (vec![[1, 2, 3]], MODE_COLORS_MODE_SPECIFIC, false)
        );
        mode.flags = MODE_FLAG_HAS_PER_LED_COLOR | MODE_FLAG_MANUAL_SAVE;
        assert!(mode.showing([1, 2, 3]).unwrap().1);
        // a device's own off mode needs no color, only to be remembered
        let off = Mode {
            name: "Off".into(),
            flags: MODE_FLAG_MANUAL_SAVE,
            ..Default::default()
        };
        assert_eq!(off.showing([0, 0, 0]), Some((off.clone(), false)));
        assert!(Mode { flags: 0, ..off }.showing([0, 0, 0]).is_none());
    }

    #[test]
    fn keys_are_placed_on_their_grid_and_gaming_keys_found() {
        let zones = [Zone {
            name: "Keyboard".into(),
            leds: 4,
            min: 4,
            max: 4,
        }];
        // a 2 x 3 grid with an empty cell; LED 3 is on no cell
        let grids = [Some((2, 3, vec![0, u32::MAX, 1, 2, 3, u32::MAX]))];
        let names: Vec<String> = ["Key: Q", "Key: W", "Key: Escape", "Key: Up Arrow"]
            .map(String::from)
            .to_vec();
        let keys = key_layout(&zones, &grids, &names).unwrap();
        assert_eq!(keys.points[0], [0.0, 0.0]);
        assert_eq!(keys.points[1], [1.0, 0.0]);
        assert_eq!(keys.points[2], [0.0, 1.0]);
        assert_eq!(keys.points[3], [0.5, 1.0]);
        assert_eq!(keys.highlighted, vec![false, true, false, true]);
        assert!(key_layout(&zones, &[None], &names).is_none());
    }
}
