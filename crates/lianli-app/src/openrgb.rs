//! Minimal OpenRGB SDK client. OpenRGB runs in the background as a server and
//! keeps its own drivers for RAM, GPU, motherboard and other non-Lian Li
//! lighting; RGBable lists those devices and sets their colors over the SDK.
//!
//! Protocol 6 (OpenRGB 1.0) is requested; older servers negotiate down. With
//! protocol 6 every command is acknowledged with a status code, so failures
//! are reported instead of being silently dropped.

use crate::effects::Effect;
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
const PKT_RESIZE_ZONE: u32 = 1000;
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
    pub active_mode: String,
    pub first_color: Option<[u8; 3]>,
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
    animations: Mutex<Animations>,
}

/// Effects OpenRGB devices play, keyed by controller index. `generation` changes whenever
/// the set changes, so the animator puts each device back into direct mode.
/// A run of a device's LEDs that follows one lighting: an effect drawn across `zones`
/// (LED counts), or a plain color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedPart {
    pub zones: Vec<usize>,
    pub effect: Option<Effect>,
    pub color: [u8; 3],
    pub delay_ms: u32,
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
        match part.effect.as_ref().filter(|e| e.is_animated()) {
            Some(effect) => {
                let period = u64::from(effect.period_ms().max(1));
                let behind = elapsed_ms - i64::from(part.delay_ms);
                let phase = behind.rem_euclid(period as i64) as f32 / period as f32;
                frame.extend(effect.frame(&part.zones, phase));
            }
            None => frame.extend(std::iter::repeat_n(part.color, part.leds())),
        }
    }
    crate::render::balance_all(&mut frame);
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
            devices: RwLock::new(Vec::new()),
            listed_ids: RwLock::new(Vec::new()),
            animations: Mutex::new(Animations::default()),
        }
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
    pub fn device_list_changed(&self) -> Result<bool> {
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
            warn!("OpenRGB write failed, disconnecting: {e:#}");
            *guard = None;
            *self.status.write() = "Disconnected from OpenRGB".into();
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
                let mut generation = u64::MAX;
                let mut in_direct_mode = BTreeSet::new();
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
                    if animations.generation != generation {
                        generation = animations.generation;
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
    let mut modes = Vec::new();
    for _ in 0..mode_count {
        modes.push(read_mode(&mut r, version)?);
    }
    let zone_count = r.u16()?;
    let mut zones = Vec::new();
    for _ in 0..zone_count {
        let zone_name = r.string()?;
        r.skip(4)?; // type
        let min = r.u32()? as usize;
        let max = r.u32()? as usize;
        let leds = r.u32()? as usize;
        let matrix = r.u16()? as usize;
        r.skip(matrix)?;
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
    for _ in 0..leds {
        r.string()?; // LED name
        if version < 6 {
            r.u32()?; // LED value
        }
    }
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
        active_mode,
        first_color,
    })
}

/// One mode entry (RGBController::GetModeDescriptionData); returns its name.
fn read_mode(r: &mut Reader<'_>, version: u32) -> Result<String> {
    let name = r.string()?;
    if version < 6 {
        r.skip(4)?; // value
    }
    r.skip(4 * 3)?; // flags, speed min, speed max
    if version >= 3 {
        r.skip(4 * 2)?; // brightness min/max
    }
    r.skip(4 * 3)?; // colors min/max, speed
    if version >= 3 {
        r.skip(4)?; // brightness
    }
    r.skip(4 * 2)?; // direction, color mode
    let colors = r.u16()? as usize;
    r.skip(colors * 4)?;
    Ok(name)
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
