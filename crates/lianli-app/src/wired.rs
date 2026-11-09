//! Lian Li devices on USB cables rather than the wireless dongle: Galahad II LCD (and
//! HydroShift LCD) AIOs and UNI HUB (ENE 6K77) fan controllers. Windows reaches them
//! through its own HID driver (hidapi), since libusb can't open devices that driver owns.

use crate::effects::{Effect, EffectKind};
use lianli_devices::ene6k77::{Ene6k77Controller, Ene6k77GroupDevice};
use lianli_devices::hydroshift_lcd::{self, HydroShiftLcdController};
use lianli_devices::registry::SharedHid;
use lianli_devices::traits::{FanDevice, RgbDevice};
use lianli_shared::device_id::{lookup_device, DeviceFamily};
use lianli_shared::rgb::{RgbDirection, RgbEffect, RgbMode};
use lianli_transport::{HidApiTransport, HidTarget};
use parking_lot::Mutex;
use std::collections::BTreeSet;
use std::sync::Arc;
use tracing::{info, warn};

/// Vendor IDs Lian Li's wired devices use, logged on discovery so a user's log shows what
/// is plugged in even when RGBable doesn't know it yet.
const LIAN_LI_VIDS: &[u16] = &[0x0416, 0x0CF2, 0x04FC, 0x1CBE, 0x39B0];

pub enum FanHandle {
    /// One port (group) of a UNI HUB.
    Hub(Arc<Ene6k77Controller>, u8),
    /// The radiator fans of an AIO, all on one speed.
    Aio(Arc<HydroShiftLcdController>),
}

impl FanHandle {
    pub fn set_duty(&self, duty: u8) -> anyhow::Result<()> {
        match self {
            Self::Hub(hub, group) => {
                hub.set_mb_rpm_sync(*group, false)?;
                hub.set_fan_speed(*group, duty)
            }
            Self::Aio(aio) => aio.set_fan_speed(0, duty),
        }
    }

    /// Hands the speed to the motherboard's fan header, where the device can follow it.
    pub fn follow_board(&self) -> anyhow::Result<bool> {
        match self {
            Self::Hub(hub, group) => hub.set_mb_rpm_sync(*group, true).map(|()| true),
            Self::Aio(_) => Ok(false),
        }
    }

    pub fn ready(&self) -> bool {
        match self {
            Self::Hub(..) => true,
            Self::Aio(aio) => aio.is_ready_for_control(),
        }
    }

    pub fn rpm(&self) -> Vec<u16> {
        match self {
            Self::Hub(hub, group) => hub
                .read_rpms()
                .map(|r| vec![r[usize::from(*group)]])
                .unwrap_or_default(),
            Self::Aio(aio) => aio.read_fan_rpm().unwrap_or_default(),
        }
    }
}

/// One thing the UI lists: a hub port's fans, or an AIO's pump head or radiator fans.
pub struct WiredPart {
    pub id: String,
    pub name: String,
    /// "fans" or "ring"
    pub kind: &'static str,
    pub model: String,
    pub fans: u8,
    pub leds: usize,
    pub rgb: Arc<dyn RgbDevice>,
    /// The device's lighting zone, or every zone it has.
    pub zone: Option<u8>,
    pub fan: Option<FanHandle>,
    /// The latest fan speeds read back, for the UI.
    pub rpm: Mutex<Vec<u16>>,
}

pub struct Wired {
    pub parts: Vec<Arc<WiredPart>>,
    pub aios: Vec<Arc<HydroShiftLcdController>>,
}

fn open(
    vid: u16,
    pid: u16,
    usage_page: Option<u16>,
    serial: Option<String>,
) -> anyhow::Result<SharedHid> {
    let transport = HidApiTransport::open(HidTarget {
        vid,
        pid,
        usage_page,
        serial,
    })?;
    Ok(Arc::new(parking_lot::Mutex::new(Box::new(transport))))
}

/// Finds and opens the wired devices RGBable can drive.
pub fn discover() -> Wired {
    let mut wired = Wired {
        parts: Vec::new(),
        aios: Vec::new(),
    };
    let api = match hidapi::HidApi::new() {
        Ok(api) => api,
        Err(e) => {
            warn!("listing HID devices: {e}");
            return wired;
        }
    };
    let mut seen = BTreeSet::new();
    let mut opened = BTreeSet::new();
    for info in api.device_list() {
        let (vid, pid) = (info.vendor_id(), info.product_id());
        if !LIAN_LI_VIDS.contains(&vid) {
            continue;
        }
        let serial = info.serial_number().map(str::to_string);
        let known = lookup_device(vid, pid);
        if seen.insert((vid, pid, serial.clone(), info.usage_page())) {
            info!(
                "HID device {vid:04x}:{pid:04x} usage page {:04x} interface {} ({}){}",
                info.usage_page(),
                info.interface_number(),
                info.product_string().unwrap_or("unnamed"),
                known.map_or(String::new(), |k| format!(": {}", k.name))
            );
        }
        let Some(entry) = known else { continue };
        if !matches!(
            entry.family,
            DeviceFamily::Galahad2Lcd | DeviceFamily::HydroShiftLcd | DeviceFamily::Ene6k77
        ) || entry
            .hid_usage_page
            .is_some_and(|page| page != info.usage_page())
            || !opened.insert((vid, pid, serial.clone()))
        {
            continue;
        }
        let key = format!("{pid:04x}-{}", serial.as_deref().unwrap_or("0"));
        let result = match entry.family {
            DeviceFamily::Ene6k77 => {
                open_hub(&mut wired, vid, pid, entry.hid_usage_page, serial, &key)
            }
            _ => open_aio(&mut wired, vid, pid, entry.hid_usage_page, serial, &key),
        };
        match result {
            Ok(()) => info!("{} ready", entry.name),
            Err(e) => warn!("{} unavailable: {e:#}", entry.name),
        }
    }
    wired
}

fn open_hub(
    wired: &mut Wired,
    vid: u16,
    pid: u16,
    usage_page: Option<u16>,
    serial: Option<String>,
    key: &str,
) -> anyhow::Result<()> {
    let hub = Arc::new(Ene6k77Controller::new(
        open(vid, pid, usage_page, serial)?,
        pid,
    )?);
    let model = hub.model().name().to_string();
    for (group, device) in hub.group_devices() {
        let fans = hub.fan_quantity(group);
        wired.parts.push(Arc::new(WiredPart {
            id: format!("hub-{key}-{group}"),
            name: format!("{model} port {}", group + 1),
            kind: "fans",
            model: model.clone(),
            fans,
            leds: usize::from(fans) * usize::from(hub.leds_per_fan()),
            rgb: Arc::new(device) as Arc<Ene6k77GroupDevice> as Arc<dyn RgbDevice>,
            zone: None,
            fan: Some(FanHandle::Hub(Arc::clone(&hub), group)),
            rpm: Mutex::new(Vec::new()),
        }));
    }
    Ok(())
}

fn open_aio(
    wired: &mut Wired,
    vid: u16,
    pid: u16,
    usage_page: Option<u16>,
    serial: Option<String>,
    key: &str,
) -> anyhow::Result<()> {
    let (aio, rgb) = hydroshift_lcd::open_with_hid(open(vid, pid, usage_page, serial)?, pid)?;
    let model = aio.variant().name().to_string();
    let rgb: Arc<dyn RgbDevice> = rgb;
    let zones = rgb.zone_info();
    for (zone, info) in zones.iter().enumerate() {
        let is_fans = info.name.to_lowercase().contains("fan");
        wired.parts.push(Arc::new(WiredPart {
            id: format!("aio-{key}-{zone}"),
            name: format!("{model} {}", info.name.to_lowercase()),
            kind: if is_fans { "fans" } else { "ring" },
            model: model.clone(),
            fans: if is_fans { 3 } else { 0 },
            leds: usize::from(info.led_count),
            rgb: Arc::clone(&rgb),
            zone: Some(zone as u8),
            fan: is_fans.then(|| FanHandle::Aio(Arc::clone(&aio))),
            rpm: Mutex::new(Vec::new()),
        }));
    }
    if !zones.iter().any(|z| z.name.to_lowercase().contains("fan")) && aio.fan_slot_count() > 0 {
        // lighting without a fan zone: the fans still need their speed
        wired.parts.push(Arc::new(WiredPart {
            id: format!("aio-{key}-fans"),
            name: format!("{model} fans"),
            kind: "fans",
            model: model.clone(),
            fans: 3,
            leds: 0,
            rgb: Arc::clone(&rgb),
            zone: None,
            fan: Some(FanHandle::Aio(Arc::clone(&aio))),
            rpm: Mutex::new(Vec::new()),
        }));
    }
    wired.aios.push(aio);
    Ok(())
}

/// The device's own effect closest to an RGBable effect: wired devices animate their
/// lighting themselves, from a mode and up to four colors.
pub fn hardware_effect(effect: Option<&Effect>, color: [u8; 3], modes: &[RgbMode]) -> RgbEffect {
    let Some(effect) = effect else {
        return RgbEffect {
            mode: RgbMode::Static,
            colors: vec![color],
            ..RgbEffect::default()
        };
    };
    let wanted = match effect.kind {
        EffectKind::Lianli => effect.mode.unwrap_or(RgbMode::Rainbow),
        EffectKind::Static => RgbMode::Static,
        EffectKind::Breathing | EffectKind::Heartbeat => RgbMode::Breathing,
        EffectKind::ColorCycle => RgbMode::ColorCycle,
        EffectKind::Rainbow | EffectKind::RainbowWave | EffectKind::Aurora => RgbMode::Rainbow,
        EffectKind::Wave => RgbMode::Wave,
        EffectKind::Meteor | EffectKind::Scanner => RgbMode::Meteor,
        EffectKind::Runway | EffectKind::Fill => RgbMode::Runway,
        EffectKind::Ripple => RgbMode::Ripple,
        EffectKind::StarryNight | EffectKind::Twinkle => RgbMode::StaticStarryNight,
        _ => RgbMode::Static,
    };
    let fallbacks = [wanted, RgbMode::Rainbow, RgbMode::Static];
    let mode = fallbacks
        .into_iter()
        .find(|m| modes.contains(m))
        .unwrap_or(RgbMode::Static);
    let mut colors: Vec<[u8; 3]> = effect.colors.iter().take(4).copied().collect();
    if colors.is_empty() {
        colors.push(color);
    }
    RgbEffect {
        mode,
        colors,
        speed: effect.speed.clamp(1, 5) - 1,
        brightness: (u16::from(effect.brightness.min(100)) * 4 / 100) as u8,
        direction: if effect.reverse {
            RgbDirection::CounterClockwise
        } else {
            RgbDirection::Clockwise
        },
        ..RgbEffect::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(kind: EffectKind) -> Effect {
        serde_json::from_value(serde_json::json!({ "kind": kind, "colors": [[255, 0, 0], [0, 0, 255]], "speed": 5, "brightness": 100 }))
            .unwrap()
    }

    #[test]
    fn maps_effects_to_what_the_device_has() {
        let modes = [RgbMode::Static, RgbMode::Breathing, RgbMode::Rainbow];
        let breathing = hardware_effect(Some(&effect(EffectKind::Breathing)), [0, 0, 0], &modes);
        assert_eq!(breathing.mode, RgbMode::Breathing);
        assert_eq!((breathing.speed, breathing.brightness), (4, 4));
        assert_eq!(breathing.colors, vec![[255, 0, 0], [0, 0, 255]]);
        // the device has no meteor: the closest it has
        let meteor = hardware_effect(Some(&effect(EffectKind::Meteor)), [0, 0, 0], &modes);
        assert_eq!(meteor.mode, RgbMode::Rainbow);
        let plain = hardware_effect(None, [1, 2, 3], &modes);
        assert_eq!(
            (plain.mode, plain.colors),
            (RgbMode::Static, vec![[1, 2, 3]])
        );
    }
}
