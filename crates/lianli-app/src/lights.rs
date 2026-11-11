//! One list of every light RGBable controls, Lian Li devices and OpenRGB devices alike,
//! with the user's names, groups and "Sync everything" choices from the config.

use crate::effects::{Effect, EffectKind};
use crate::engine::{self, Config, Engine, GroupConfig};
use crate::openrgb::{compose, LedPart, OpenRgb, OrgbDevice, Zone};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{info, warn};

pub const OPENRGB_PREFIX: &str = "orgb:";

#[derive(Debug, Clone, Serialize)]
pub struct LightView {
    pub id: String,
    pub source: &'static str,
    pub name: String,
    pub default_name: String,
    pub kind: String,
    pub detail: String,
    pub color: Option<[u8; 3]>,
    pub effect: Option<Effect>,
    pub outer_effect: Option<Effect>,
    /// SL Flex fans: effects can target the inner or outer ring.
    pub sides: bool,
    /// Strimers: how many lanes can run their own effects, and those effects.
    pub lanes: usize,
    pub lane_effects: Vec<Option<Effect>>,
    pub sync: bool,
    pub group: Option<String>,
    /// Kept off the device lists until shown; also left out of Sync everything.
    pub hidden: bool,
    /// Its own off switch (all lighting can also be off at once).
    pub off: bool,
    /// Its own white balance, when it doesn't follow the global one.
    pub warmth: Option<i8>,
    /// OpenRGB zones whose LED count the user can set (addressable headers).
    pub zones: Vec<ZoneView>,
    pub delay_ms: u32,
    pub span: Option<crate::effects::Span>,
    pub turn: Option<crate::effects::Span>,
    /// The group (or All Devices) whose lighting it can go back to, when it differs from it.
    pub resync: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ZoneView {
    pub index: usize,
    pub name: String,
    pub leds: usize,
    pub min: usize,
    pub max: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupView {
    pub id: String,
    pub name: String,
    pub color: Option<[u8; 3]>,
    pub effect: Option<Effect>,
    pub members: Vec<String>,
    /// Its lighting differs from All Devices', so it can go back to that.
    pub resync: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetupEntry {
    pub id: String,
    pub name: String,
    pub group: Option<String>,
    pub sync: bool,
}

/// OpenRGB renumbers controllers between runs, so devices are keyed by name and bus location.
pub fn openrgb_id(device: &OrgbDevice) -> String {
    let place = if device.location.is_empty() {
        &device.serial
    } else {
        &device.location
    };
    format!("{OPENRGB_PREFIX}{}@{}", device.name, place)
}

fn group_of(config: &Config, id: &str) -> Option<String> {
    config
        .groups
        .iter()
        .find(|g| g.members.iter().any(|m| m == id))
        .map(|g| g.id.clone())
}

pub fn lights(engine: &Engine, openrgb: &OpenRgb) -> Vec<LightView> {
    let config = engine.config();
    let mut list: Vec<LightView> = engine
        .devices()
        .into_iter()
        .map(|d| {
            let saved = config.devices.get(&d.id);
            LightView {
                source: "lianli",
                kind: match d.kind {
                    "ring" => "OLED ring".into(),
                    "strimer" => "Strimer".into(),
                    _ => format!("{} fan{}", d.fans, if d.fans == 1 { "" } else { "s" }),
                },
                detail: d.model,
                color: d.color,
                effect: d.effect,
                outer_effect: d.outer_effect,
                sides: d.sides,
                lanes: d.lanes,
                lane_effects: d.lane_effects,
                delay_ms: 0,
                span: None,
                turn: None,
                resync: None,
                zones: Vec::new(),
                sync: saved.is_none_or(|s| s.in_sync()),
                group: group_of(&config, &d.id),
                hidden: false,
                off: false,
                warmth: None,
                default_name: d.default_name,
                name: d.name,
                id: d.id,
            }
        })
        .collect();
    for device in openrgb.view().devices {
        let id = openrgb_id(&device);
        let saved = config.devices.get(&id);
        let device_name = saved
            .and_then(|s| s.name.clone())
            .unwrap_or_else(|| device.name.clone());
        let device_color = saved.and_then(|s| s.color).or(device.first_color);
        let device_effect = saved.and_then(|s| s.effect.clone());
        let zoned = device.zones.iter().map(|z| z.leds).sum::<usize>() == device.leds;
        let headers: Vec<(usize, &Zone)> = if zoned {
            device
                .zones
                .iter()
                .enumerate()
                // a header set to 0 LEDs stays listed once it has a name or lighting of its
                // own, so it can be given LEDs again where it was turned off
                .filter(|(index, z)| {
                    is_header(z)
                        || z.resizable() && config.devices.contains_key(&header_id(&id, *index))
                })
                .collect()
        } else {
            Vec::new()
        };
        let own_leds = device.leds - headers.iter().map(|(_, z)| z.leds).sum::<usize>();
        let zone_view = |index: usize, z: &Zone| ZoneView {
            index,
            name: z.name.clone(),
            leds: z.leds,
            min: z.min,
            max: z.max,
        };
        if own_leds > 0 || headers.is_empty() {
            list.push(LightView {
                source: "openrgb",
                name: device_name.clone(),
                default_name: device.name.clone(),
                kind: device.kind.to_string(),
                detail: format!("{own_leds} LEDs via OpenRGB"),
                color: device_color,
                effect: device_effect.clone(),
                outer_effect: None,
                sides: false,
                lanes: 0,
                lane_effects: Vec::new(),
                delay_ms: 0,
                span: None,
                turn: None,
                resync: None,
                zones: device
                    .zones
                    .iter()
                    .enumerate()
                    .filter(|(_, z)| z.resizable())
                    .map(|(index, z)| zone_view(index, z))
                    .collect(),
                sync: saved.is_none_or(|s| s.in_sync()),
                group: group_of(&config, &id),
                hidden: false,
                off: false,
                warmth: None,
                id: id.clone(),
            });
        }
        for (index, zone) in headers {
            let header = header_id(&id, index);
            let own = config.devices.get(&header);
            let lit = has_lighting(&config, &header);
            let default_name = format!("{} · {}", device.name, zone.name);
            list.push(LightView {
                source: "openrgb",
                name: own
                    .and_then(|s| s.name.clone())
                    .unwrap_or_else(|| default_name.clone()),
                default_name,
                kind: "header".to_string(),
                detail: if zone.leds == 0 {
                    format!(
                        "No LEDs set on {}, {}: set its LED count",
                        device_name, zone.name
                    )
                } else {
                    format!("{} LEDs on {}, {}", zone.leds, device_name, zone.name)
                },
                color: if lit {
                    own.and_then(|s| s.color)
                } else {
                    device_color
                },
                effect: if lit {
                    own.and_then(|s| s.effect.clone())
                } else {
                    device_effect.clone()
                },
                outer_effect: None,
                sides: false,
                lanes: 0,
                lane_effects: Vec::new(),
                delay_ms: 0,
                span: None,
                turn: None,
                resync: None,
                zones: vec![zone_view(index, zone)],
                sync: own.is_none_or(|s| s.in_sync()),
                group: group_of(&config, &header),
                hidden: false,
                off: false,
                warmth: None,
                id: header,
            });
        }
    }
    for light in &mut list {
        let saved = config.devices.get(&light.id);
        light.delay_ms = saved.map_or(0, |d| d.delay_ms);
        light.span = saved.and_then(|d| d.span);
        light.turn = saved.and_then(|d| d.turn);
        light.hidden = saved.is_some_and(|d| d.hidden);
        light.off = saved.is_some_and(|d| d.off);
        light.warmth = saved.and_then(|d| d.warmth);
        light.resync = resync_target(&config, &light.id, light.sync)
            .filter(|(_, effect)| !shows(&config, &light.id, effect))
            .map(|(name, _)| name);
    }
    list
}

/// The lighting a device goes back to: its group's, or All Devices' when it isn't in a
/// group with lighting of its own and is part of All Devices.
fn resync_target(config: &Config, id: &str, in_sync: bool) -> Option<(String, Effect)> {
    let group = config
        .groups
        .iter()
        .find(|g| g.members.iter().any(|m| m == id));
    if let Some(g) = group {
        if let Some(effect) = g.effect.clone().or_else(|| g.color.map(static_effect)) {
            return Some((g.name.clone(), effect));
        }
    }
    let reversed = config.devices.get(id).is_some_and(|d| d.reversed);
    in_sync
        .then(|| config.all_effect.clone())
        .flatten()
        .map(|e| ("All Devices".to_string(), e.reversed_if(reversed)))
}

/// Whether a device's saved lighting is this effect throughout.
fn shows(config: &Config, id: &str, effect: &Effect) -> bool {
    let Some(d) = config.devices.get(id) else {
        return false;
    };
    if !d.lanes.is_empty() || d.outer_effect.is_some() {
        return false;
    }
    if effect.is_animated() {
        let flipped = effect.clone().reversed_if(d.reversed);
        d.effect.as_ref() == Some(effect) || d.effect.as_ref() == Some(&flipped)
    } else {
        d.effect.is_none() && d.color == Some(effect.frame(&[1], 0.0)[0])
    }
}

/// Puts a device back on its group's lighting, or All Devices'.
pub fn resync_device(engine: &Engine, openrgb: &OpenRgb, id: &str) -> Result<String> {
    let config = engine.config();
    let in_sync = config.devices.get(id).is_none_or(|d| d.in_sync());
    let (name, effect) =
        resync_target(&config, id, in_sync).context("there is no group or All Devices lighting to go back to")?;
    go_dark(engine, openrgb, &[id.to_string()]);
    set_effect(engine, openrgb, id, effect)?;
    Ok(name)
}

/// Puts a group back on All Devices' lighting.
pub fn resync_group(engine: &Engine, openrgb: &OpenRgb, group: &str) -> Result<()> {
    let effect = engine
        .config()
        .all_effect
        .context("All Devices has no lighting to go back to yet")?;
    set_group_effect(engine, openrgb, group, effect)
}

/// Whether a light (an OpenRGB device or one of its headers) shows black.
fn is_off(config: &Config, id: &str) -> bool {
    config.all_off || config.devices.get(id).is_some_and(|d| d.off)
}

/// Gives a device its own white balance, or (None) puts it back on the global one.
pub fn set_device_warmth(
    engine: &Engine,
    openrgb: &OpenRgb,
    id: &str,
    warmth: Option<i8>,
) -> Result<()> {
    let warmth = warmth.map(|w| w.clamp(-50, 50));
    engine.update_config(|c| c.devices.entry(id.to_string()).or_default().warmth = warmth);
    engine.apply_darkness();
    if id.starts_with(OPENRGB_PREFIX) {
        let device = openrgb_device(openrgb, id).context("that OpenRGB device is not connected")?;
        push_device(engine, openrgb, &device)?;
    }
    Ok(())
}

/// Turns one device's lights off or back on; its lighting is kept.
pub fn set_off(engine: &Engine, openrgb: &OpenRgb, id: &str, off: bool) -> Result<()> {
    engine.update_config(|c| c.devices.entry(id.to_string()).or_default().off = off);
    engine.apply_darkness();
    if id.starts_with(OPENRGB_PREFIX) {
        let device = openrgb_device(openrgb, id).context("that OpenRGB device is not connected")?;
        push_device(engine, openrgb, &device)?;
    }
    Ok(())
}

/// Turns every light off, or back to what each device is set to.
pub fn set_all_off(engine: &Engine, openrgb: &OpenRgb, off: bool) {
    engine.update_config(|c| c.all_off = off);
    engine.apply_darkness();
    sync_openrgb_effects(engine, openrgb);
}

pub fn groups(engine: &Engine) -> Vec<GroupView> {
    let config = engine.config();
    config
        .groups
        .clone()
        .into_iter()
        .map(|g| GroupView {
            resync: config.all_effect.as_ref().is_some_and(|all| {
                let own = g.effect.clone().or_else(|| g.color.map(static_effect));
                own.as_ref().is_none_or(|own| {
                    if all.is_animated() {
                        own != all
                    } else {
                        own.is_animated() || own.frame(&[1], 0.0) != all.frame(&[1], 0.0)
                    }
                })
            }),
            id: g.id,
            name: g.name,
            color: g.color,
            effect: g.effect,
            members: g.members,
        })
        .collect()
}

/// Motherboard addressable headers get their own id: the device id plus `#z<zone index>`.
pub const ZONE_MARK: &str = "#z";

pub fn header_id(device_id: &str, zone: usize) -> String {
    format!("{device_id}{ZONE_MARK}{zone}")
}

/// Splits a light id into its OpenRGB device id and, for a header, the zone index.
fn split_id(id: &str) -> (&str, Option<usize>) {
    match id.rsplit_once(ZONE_MARK) {
        Some((device, zone)) => match zone.parse() {
            Ok(zone) => (device, Some(zone)),
            Err(_) => (id, None),
        },
        None => (id, None),
    }
}

/// An addressable header with LEDs on it is shown and lit as its own device.
fn is_header(zone: &Zone) -> bool {
    zone.resizable() && zone.leds > 0
}

fn openrgb_device(openrgb: &OpenRgb, id: &str) -> Option<OrgbDevice> {
    let (device_id, _) = split_id(id);
    openrgb
        .view()
        .devices
        .into_iter()
        .find(|d| openrgb_id(d) == device_id)
}

fn has_lighting(config: &Config, id: &str) -> bool {
    config
        .devices
        .get(id)
        .is_some_and(|d| d.effect.is_some() || d.color.is_some())
}

/// The device's LEDs as runs that each follow one saved lighting. A header follows its own
/// lighting once it has one, and the motherboard's until then.
fn device_parts(config: &Config, device: &OrgbDevice) -> Vec<LedPart> {
    let device_id = openrgb_id(device);
    let board_delay = config.devices.get(&device_id).map_or(0, |d| d.delay_ms);
    let board_span = config.devices.get(&device_id).and_then(|d| d.span);
    let board_turn = config.devices.get(&device_id).and_then(|d| d.turn);
    let mut runs: Vec<(String, Vec<usize>)> = Vec::new();
    let zoned: usize = device.zones.iter().map(|z| z.leds).sum();
    if zoned == device.leds {
        for (index, zone) in device.zones.iter().enumerate().filter(|(_, z)| z.leds > 0) {
            let own = header_id(&device_id, index);
            let source = if is_header(zone) && (has_lighting(config, &own) || is_off(config, &own))
            {
                own
            } else {
                device_id.clone()
            };
            match runs.last_mut() {
                Some((last, zones)) if *last == source => zones.push(zone.leds),
                _ => runs.push((source, vec![zone.leds])),
            }
        }
    } else {
        runs.push((device_id, vec![device.leds]));
    }
    let mut start = 0;
    runs.into_iter()
        .map(|(source, zones)| {
            let dark = is_off(config, &source);
            let saved = config.devices.get(&source).filter(|_| !dark);
            let leds: usize = zones.iter().sum();
            let keys = device.keys.as_ref().map(|k| k.slice(start, leds));
            start += leds;
            LedPart {
                warmth: config
                    .devices
                    .get(&source)
                    .and_then(|d| d.warmth)
                    .unwrap_or(crate::render::GLOBAL_WARMTH),
                zones,
                keys,
                effect: saved.and_then(|s| s.effect.clone()).map(|e| Effect {
                    span: saved.and_then(|s| s.span).or(board_span),
                    turn: saved.and_then(|s| s.turn).or(board_turn),
                    ..e
                }),
                color: if dark {
                    [0, 0, 0]
                } else {
                    saved
                        .and_then(|s| s.color)
                        .or(device.first_color)
                        .unwrap_or([0, 0, 0])
                },
                delay_ms: saved
                    .map(|s| s.delay_ms)
                    .filter(|&d| d > 0)
                    .unwrap_or(board_delay),
            }
        })
        .collect()
}

/// Sends a device its saved lighting: animated parts go to the animator, otherwise one frame.
/// Devices the user never set are left as they are.
fn push_device(engine: &Engine, openrgb: &OpenRgb, device: &OrgbDevice) -> Result<()> {
    let config = engine.config();
    let parts = device_parts(&config, device);
    if parts
        .iter()
        .any(|p| p.effect.as_ref().is_some_and(Effect::is_animated))
    {
        openrgb.set_animation(device.index, parts);
        return Ok(());
    }
    openrgb.stop_animation(device.index);
    let device_id = openrgb_id(device);
    let touched = has_lighting(&config, &device_id)
        || is_off(&config, &device_id)
        || (0..device.zones.len()).any(|z| {
            let header = header_id(&device_id, z);
            has_lighting(&config, &header) || is_off(&config, &header)
        });
    if touched {
        openrgb.set_leds(device.index, &compose(&parts, 0), true)?;
    }
    Ok(())
}

/// Sets an addressable zone's LED count and remembers it, so it is re-applied if OpenRGB
/// ever loses the size.
pub fn resize_zone(
    engine: &Engine,
    openrgb: &OpenRgb,
    id: &str,
    zone: usize,
    size: usize,
) -> Result<()> {
    let (device_id, header) = split_id(id);
    let zone = header.unwrap_or(zone);
    let device = openrgb_device(openrgb, id).context("that OpenRGB device is not connected")?;
    let info = device.zones.get(zone).context("no such zone")?;
    anyhow::ensure!(
        info.resizable() && (info.min..=info.max).contains(&size),
        "{} takes {} to {} LEDs",
        info.name,
        info.min,
        info.max
    );
    openrgb.resize_zone(device.index, zone, size)?;
    engine.update_config(|c| {
        c.devices
            .entry(device_id.to_string())
            .or_default()
            .zone_sizes
            .insert(zone, size);
    });
    let device = openrgb_device(openrgb, id).context("that OpenRGB device is not connected")?;
    push_device(engine, openrgb, &device)
}

/// After (re)connecting: restore zone sizes the user set, then everyone's saved lighting.
pub fn after_openrgb_connect(engine: &Engine, openrgb: &OpenRgb) {
    remember_zone_sizes(engine, openrgb);
    let config = engine.config();
    for device in openrgb.view().devices {
        let Some(saved) = config.devices.get(&openrgb_id(&device)) else {
            continue;
        };
        for (&zone, &size) in &saved.zone_sizes {
            let current = device.zones.get(zone);
            if current
                .is_some_and(|z| z.resizable() && z.leds != size && (z.min..=z.max).contains(&size))
            {
                if let Err(e) = openrgb.resize_zone(device.index, zone, size) {
                    warn!("{}: restoring zone size: {e:#}", device.name);
                }
            }
        }
    }
    sync_openrgb_effects(engine, openrgb);
}

/// Keeps the LED count of every addressable header OpenRGB knows the size of (set in
/// OpenRGB itself, say), so it can be put back if OpenRGB later lists that header empty.
fn remember_zone_sizes(engine: &Engine, openrgb: &OpenRgb) {
    let devices = openrgb.view().devices;
    let config = engine.config();
    let learned: Vec<(String, usize, usize)> = devices
        .iter()
        .flat_map(|device| {
            let id = openrgb_id(device);
            let saved = config.devices.get(&id);
            device
                .zones
                .iter()
                .enumerate()
                .filter(|(_, z)| z.resizable() && z.leds > 0)
                .filter(move |(zone, _)| saved.is_none_or(|s| !s.zone_sizes.contains_key(zone)))
                .map(move |(zone, z)| (id.clone(), zone, z.leds))
                .collect::<Vec<_>>()
        })
        .collect();
    if learned.is_empty() {
        return;
    }
    engine.update_config(|c| {
        for (id, zone, size) in learned {
            c.devices
                .entry(id)
                .or_default()
                .zone_sizes
                .insert(zone, size);
        }
    });
}

/// Sends every OpenRGB device its saved lighting.
pub fn sync_openrgb_effects(engine: &Engine, openrgb: &OpenRgb) {
    for device in openrgb.view().devices {
        if let Err(e) = push_device(engine, openrgb, &device) {
            warn!("{}: {e:#}", device.name);
        }
    }
}

/// What saving power-on lighting did, by device name.
#[derive(serde::Serialize, Default)]
pub struct PowerOnSaved {
    pub saved: Vec<String>,
    /// Devices that can't keep a mode like that in their own memory.
    pub unsupported: Vec<String>,
    pub failed: Vec<String>,
}

/// Stores a plain or breathing version of each OpenRGB device's lighting in the device
/// itself, then puts every device back under RGBable's control.
pub fn save_power_on_lighting(
    engine: &Engine,
    openrgb: &OpenRgb,
    style: crate::openrgb::PowerOnStyle,
) -> PowerOnSaved {
    // with a startup profile, the PC boots showing it, so that's what these should match
    let mut config = engine.config();
    if let Some(profile) = config
        .startup_profile
        .as_ref()
        .and_then(|name| config.lighting_profiles.iter().find(|p| &p.name == name))
        .cloned()
    {
        crate::lighting_profiles::apply(&mut config, &profile);
    }
    if config.startup_dark {
        config.all_off = true;
    }
    let mut result = PowerOnSaved::default();
    for device in openrgb.view().devices {
        let id = openrgb_id(&device);
        if config.devices.get(&id).is_some_and(|d| d.hidden) {
            continue;
        }
        let name = config
            .devices
            .get(&id)
            .and_then(|d| d.name.clone())
            .unwrap_or_else(|| device.name.clone());
        let color = power_on_color(&config, &device);
        match openrgb.save_power_on(device.index, color, style) {
            Ok(true) => result.saved.push(name),
            Ok(false) => result.unsupported.push(name),
            Err(e) => {
                warn!("{name}: storing power-on lighting: {e:#}");
                result.failed.push(name);
            }
        }
    }
    info!(
        "power-on lighting stored on {} OpenRGB device(s); {} can't keep it",
        result.saved.len(),
        result.unsupported.len()
    );
    openrgb.reassert_direct();
    sync_openrgb_effects(engine, openrgb);
    result
}

/// One color that stands for a device's lighting: its effect's first color, its plain
/// color, or the first LED of what it shows.
fn power_on_color(config: &Config, device: &OrgbDevice) -> [u8; 3] {
    let id = openrgb_id(device);
    if is_off(config, &id) {
        return [0, 0, 0];
    }
    let saved = config.devices.get(&id);
    saved
        .and_then(|d| d.effect.as_ref())
        .and_then(|e| e.colors.first().copied())
        .or_else(|| saved.and_then(|d| d.color))
        .or_else(|| compose(&device_parts(config, device), 0).first().copied())
        .filter(|c| *c != [0, 0, 0])
        .unwrap_or([255, 255, 255])
}

pub fn static_effect(color: [u8; 3]) -> Effect {
    Effect {
        kind: EffectKind::Static,
        colors: vec![color],
        speed: 3,
        brightness: 100,
        reverse: false,
        ..Effect::default()
    }
}

pub fn set_effect(engine: &Engine, openrgb: &OpenRgb, id: &str, effect: Effect) -> Result<()> {
    let effect = effect.sanitized();
    if !id.starts_with(OPENRGB_PREFIX) {
        return engine.set_effect(id, effect);
    }
    let device = openrgb_device(openrgb, id).context("that OpenRGB device is not connected")?;
    let animated = effect.is_animated();
    let color = effect.frame(&[1], 0.0)[0];
    engine.update_config(|c| {
        let entry = c.devices.entry(id.to_string()).or_default();
        if animated {
            entry.effect = Some(effect);
        } else {
            entry.effect = None;
            entry.color = Some(color);
        }
    });
    push_device(engine, openrgb, &device)
}

pub fn set_lane_effects(engine: &Engine, id: &str, lanes: &[u8], effect: Effect) -> Result<()> {
    engine.set_lane_effects(id, lanes, effect.sanitized())
}

pub fn set_delay(engine: &Engine, openrgb: &OpenRgb, id: &str, delay_ms: u32) -> Result<()> {
    let delay_ms = delay_ms.min(MAX_DELAY_MS);
    engine.update_config(|c| c.devices.entry(id.to_string()).or_default().delay_ms = delay_ms);
    if !id.starts_with(OPENRGB_PREFIX) {
        engine.relight(id);
        return Ok(());
    }
    let device = openrgb_device(openrgb, id).context("that OpenRGB device is not connected")?;
    push_device(engine, openrgb, &device)
}

pub const MAX_DELAY_MS: u32 = 10_000;

/// Flips the direction of a device's current effect (both rings of an SL Flex fan). A header
/// that follows its board gets its own copy, so the board itself is left alone.
pub fn set_reverse(engine: &Engine, openrgb: &OpenRgb, id: &str, reverse: bool) -> Result<()> {
    let light = lights(engine, openrgb)
        .into_iter()
        .find(|l| l.id == id)
        .with_context(|| format!("no device {id}"))?;
    let effects = [light.effect, light.outer_effect];
    anyhow::ensure!(
        effects.iter().flatten().any(Effect::is_animated),
        "{} has a plain color, so there is no direction to reverse",
        light.name
    );
    for effect in effects.into_iter().flatten() {
        set_effect(engine, openrgb, id, Effect { reverse, ..effect })?;
    }
    engine.update_config(|c| c.devices.entry(id.to_string()).or_default().reversed = reverse);
    Ok(())
}

pub fn set_speed(engine: &Engine, openrgb: &OpenRgb, id: &str, speed: u8) -> Result<()> {
    let light = lights(engine, openrgb)
        .into_iter()
        .find(|l| l.id == id)
        .with_context(|| format!("no device {id}"))?;
    let effects = [light.effect, light.outer_effect];
    anyhow::ensure!(
        effects.iter().flatten().any(Effect::is_animated),
        "{} has a plain color, so there is no speed to change",
        light.name
    );
    let speed = speed.clamp(1, 5);
    for effect in effects.into_iter().flatten() {
        set_effect(engine, openrgb, id, Effect { speed, ..effect })?;
    }
    Ok(())
}

/// One device's place in a sequence: its start delay, and its span when effects run
/// through the devices as one strip.
#[derive(Debug, Clone, Deserialize)]
pub struct SequenceEntry {
    pub id: String,
    #[serde(default)]
    pub delay_ms: u32,
    #[serde(default)]
    pub span: Option<crate::effects::Span>,
    #[serde(default)]
    pub turn: Option<crate::effects::Span>,
}

pub fn set_hidden(engine: &Engine, id: &str, hidden: bool) {
    engine.update_config(|c| c.devices.entry(id.to_string()).or_default().hidden = hidden);
}

/// Saves a whole sequence at once and resends only the devices that changed.
pub fn set_sequence(engine: &Engine, openrgb: &OpenRgb, entries: &[SequenceEntry]) -> Result<()> {
    let changed: Vec<String> = engine.update_config(|c| {
        entries
            .iter()
            .filter_map(|e| {
                let delay = e.delay_ms.min(MAX_DELAY_MS);
                let entry = c.devices.entry(e.id.clone()).or_default();
                let different =
                    entry.delay_ms != delay || entry.span != e.span || entry.turn != e.turn;
                entry.delay_ms = delay;
                entry.span = e.span;
                entry.turn = e.turn;
                different.then(|| e.id.clone())
            })
            .collect()
    });
    // Saving starts every effect over, so a flow begins at the first device; the start waits
    // about as long as sending each wireless device its loop takes.
    let wireless = entries
        .iter()
        .filter(|e| !e.id.starts_with(OPENRGB_PREFIX))
        .count() as i64;
    crate::render::restart_clock(500 + 500 * wireless);
    engine.relight_all();
    relight_all(engine, openrgb, &changed)
}

/// Runs moving effects across all of a device's fans (or LEDs) instead of on each fan.
pub fn set_across(engine: &Engine, openrgb: &OpenRgb, id: &str, across: bool) -> Result<()> {
    engine.update_config(|c| {
        c.devices.entry(id.to_string()).or_default().span =
            across.then_some(crate::effects::Span::WHOLE);
    });
    relight_all(engine, openrgb, &[id.to_string()])
}

fn relight_all(engine: &Engine, openrgb: &OpenRgb, ids: &[String]) -> Result<()> {
    let mut boards = std::collections::BTreeSet::new();
    for id in ids {
        if id.starts_with(OPENRGB_PREFIX) {
            boards.insert(split_id(id).0.to_string());
        } else {
            engine.relight(id);
        }
    }
    for board in boards {
        if let Some(device) = openrgb_device(openrgb, &board) {
            push_device(engine, openrgb, &device)?;
        }
    }
    Ok(())
}

pub fn set_color(engine: &Engine, openrgb: &OpenRgb, id: &str, color: [u8; 3]) -> Result<()> {
    set_effect(engine, openrgb, id, static_effect(color))
}

/// Applies an effect to every light that is in "Sync everything"; stops at the first OpenRGB error.
pub fn set_all(
    engine: &Engine,
    openrgb: &OpenRgb,
    effect: Effect,
    include_openrgb: bool,
) -> Result<()> {
    let effect = effect.sanitized();
    let targets: Vec<String> = lights(engine, openrgb)
        .into_iter()
        .filter(|l| l.sync && (include_openrgb || l.source != "openrgb"))
        .map(|l| l.id)
        .collect();
    go_dark(engine, openrgb, &targets);
    engine.set_all_effect(&effect);
    engine.update_config(|c| {
        c.all_effect = Some(effect.clone());
        for g in &mut c.groups {
            if effect.is_animated() {
                g.effect = Some(effect.clone());
            } else {
                g.effect = None;
                g.color = Some(effect.frame(&[1], 0.0)[0]);
            }
        }
    });
    if !include_openrgb {
        return Ok(());
    }
    let mut failed = Vec::new();
    for light in lights(engine, openrgb)
        .into_iter()
        .filter(|l| l.source == "openrgb" && l.sync)
    {
        let reversed = engine
            .config()
            .devices
            .get(&light.id)
            .is_some_and(|d| d.reversed);
        let effect = effect.clone().reversed_if(reversed);
        let first = set_effect(engine, openrgb, &light.id, effect.clone());
        let result = first.or_else(|e| {
            tracing::warn!(
                "{}: {e:#}; reconnecting to OpenRGB and retrying",
                light.name
            );
            openrgb.connect(engine.config().openrgb_port)?;
            after_openrgb_connect(engine, openrgb);
            set_effect(engine, openrgb, &light.id, effect.clone())
        });
        if let Err(e) = result {
            tracing::warn!("{}: {e:#}", light.name);
            failed.push(light.name);
        }
    }
    if !failed.is_empty() {
        bail!("couldn't set {} through OpenRGB", failed.join(", "));
    }
    Ok(())
}

/// What the last OLED color match did, shown under the option on the OLED page.
pub static OLED_MATCH_STATUS: parking_lot::Mutex<String> = parking_lot::const_mutex(String::new());

/// Gives every synced device `count` colors from what's on the OLED. Each keeps its own
/// effect (and an SL Flex fan both rings'); a plain color takes the main color, and groups
/// keep their effect with the new colors.
pub fn match_oled(engine: &Engine, openrgb: &OpenRgb, count: u8) -> Result<Vec<[u8; 3]>> {
    let result = recolor_from_oled(engine, openrgb, count);
    *OLED_MATCH_STATUS.lock() = match &result {
        Ok((colors, devices)) => format!(
            "Matched {devices} device{} to {}",
            if *devices == 1 { "" } else { "s" },
            colors
                .iter()
                .map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Err(e) => format!("Couldn't match the lighting: {e:#}"),
    };
    result.map(|(colors, _)| colors)
}

fn recolor_from_oled(
    engine: &Engine,
    openrgb: &OpenRgb,
    count: u8,
) -> Result<(Vec<[u8; 3]>, usize)> {
    let oled = engine
        .config()
        .oled
        .context("nothing is playing on the OLED")?;
    let colors = crate::palette::from_media(&oled.file, usize::from(count))?;
    let recolor = |effect: Effect| -> Effect {
        match effect.kind {
            EffectKind::Custom | EffectKind::Script if colors.len() > 1 => Effect {
                kind: EffectKind::Wave,
                colors: colors.clone(),
                ..Effect::default()
            },
            EffectKind::Static | EffectKind::Custom | EffectKind::Script => {
                static_effect(colors[0])
            }
            _ => Effect {
                colors: colors.clone(),
                ..effect
            },
        }
    };
    engine.update_config(|c| {
        for g in &mut c.groups {
            if let Some(effect) = g.effect.take() {
                g.effect = Some(recolor(effect));
            } else {
                g.color = Some(colors[0]);
            }
        }
    });
    let mut matched = 0;
    let mut first_error = None;
    for light in lights(engine, openrgb).into_iter().filter(|l| l.sync) {
        let effects: Vec<Effect> = [light.effect, light.outer_effect]
            .into_iter()
            .flatten()
            .collect();
        let effects = if effects.is_empty() {
            vec![static_effect(colors[0])]
        } else {
            effects.into_iter().map(recolor).collect()
        };
        let result = effects
            .into_iter()
            .try_for_each(|e| set_effect(engine, openrgb, &light.id, e));
        match result {
            Ok(()) => matched += 1,
            Err(e) => {
                warn!("matching {} to the OLED: {e:#}", light.name);
                first_error.get_or_insert(e);
            }
        }
    }
    if matched == 0 {
        if let Some(e) = first_error {
            return Err(e);
        }
    }
    Ok((colors, matched))
}

/// A device that joins a group takes on what the group's other devices are running (or the
/// group's own lighting when it was empty).
pub fn adopt_group_lighting(
    engine: &Engine,
    openrgb: &OpenRgb,
    id: &str,
    group: &str,
) -> Result<()> {
    let config = engine.config();
    let Some(g) = config.groups.iter().find(|g| g.id == group) else {
        bail!("no such group")
    };
    let all = lights(engine, openrgb);
    let neighbour = all.iter().find(|l| l.id != id && g.members.contains(&l.id));
    let effects: Vec<Effect> = match neighbour {
        Some(l) => match [l.effect.clone(), l.outer_effect.clone()] {
            [None, None] => l.color.map(static_effect).into_iter().collect(),
            effects => effects.into_iter().flatten().collect(),
        },
        None => match (&g.effect, g.color) {
            (Some(effect), _) => vec![effect.clone()],
            (None, Some(color)) => vec![static_effect(color)],
            (None, None) => Vec::new(),
        },
    };
    for effect in effects {
        set_effect(engine, openrgb, id, effect)?;
    }
    Ok(())
}

/// With clean transitions on, the devices about to change go dark first and the new
/// lighting starts on all of them together, instead of each switching as it is reached.
pub fn go_dark(engine: &Engine, openrgb: &OpenRgb, ids: &[String]) {
    if !engine.config().instant_changes {
        darken(engine, openrgb, ids);
    }
}

/// Turns every light off and waits until they are, so a new lighting profile starts on
/// all of them together.
pub fn blackout(engine: &Engine, openrgb: &OpenRgb) {
    let ids: Vec<String> = lights(engine, openrgb).into_iter().map(|l| l.id).collect();
    darken(engine, openrgb, &ids);
}

fn darken(engine: &Engine, openrgb: &OpenRgb, ids: &[String]) {
    let mut boards = std::collections::BTreeSet::new();
    for id in ids.iter().filter(|id| id.starts_with(OPENRGB_PREFIX)) {
        boards.insert(split_id(id).0.to_string());
    }
    for board in boards {
        if let Some(device) = openrgb_device(openrgb, &board) {
            openrgb.stop_animation(device.index);
            if let Err(e) = openrgb.set_color(device.index, [0, 0, 0]) {
                warn!("{}: {e:#}", device.name);
            }
        }
    }
    engine.blank_and_wait(ids, std::time::Duration::from_secs(5));
}

pub fn set_group_effect(
    engine: &Engine,
    openrgb: &OpenRgb,
    group: &str,
    effect: Effect,
) -> Result<()> {
    let effect = effect.sanitized();
    let members = engine.update_config(|c| {
        let g = c.groups.iter_mut().find(|g| g.id == group)?;
        if effect.is_animated() {
            g.effect = Some(effect.clone());
        } else {
            g.effect = None;
            g.color = Some(effect.frame(&[1], 0.0)[0]);
        }
        Some(g.members.clone())
    });
    let Some(members) = members else {
        bail!("no such group")
    };
    go_dark(engine, openrgb, &members);
    let mut first_error = None;
    for id in members {
        if let Err(e) = set_effect(engine, openrgb, &id, effect.clone()) {
            tracing::warn!("{id}: {e:#}");
            first_error.get_or_insert(e);
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub fn identify(engine: &Arc<Engine>, openrgb: &Arc<OpenRgb>, id: &str) -> Result<()> {
    if !id.starts_with(OPENRGB_PREFIX) {
        return engine.identify(id);
    }
    let device = openrgb_device(openrgb, id).context("that OpenRGB device is not connected")?;
    let (_, header) = split_id(id);
    let start: usize = header.map_or(0, |zone| {
        device.zones.iter().take(zone).map(|z| z.leds).sum()
    });
    let len = header.map_or(device.leds, |zone| {
        device.zones.get(zone).map_or(0, |z| z.leds)
    });
    let parts = device_parts(&engine.config(), &device);
    openrgb.stop_animation(device.index);
    let engine = engine.clone();
    let openrgb = openrgb.clone();
    std::thread::Builder::new()
        .name("identify-openrgb".into())
        .spawn(move || {
            let mut frame = compose(&parts, 0);
            frame.resize(device.leds, [0, 0, 0]);
            for step in 0..engine::IDENTIFY_STEPS {
                let color = if step % 2 == 0 {
                    [255, 255, 255]
                } else {
                    [0, 0, 0]
                };
                for led in frame.iter_mut().skip(start).take(len) {
                    *led = color;
                }
                let _ = openrgb.set_leds(device.index, &frame, step == 0);
                std::thread::sleep(engine::IDENTIFY_STEP);
            }
            let base = compose(&parts, 0);
            if let Err(e) = push_device(&engine, &openrgb, &device) {
                warn!("{}: restoring after identify: {e:#}", device.name);
            }
            if !parts.iter().any(|p| p.effect.is_some()) && base.len() == device.leds {
                let _ = openrgb.set_leds(device.index, &base, false);
            }
        })?;
    Ok(())
}

// ------------------------------------------------------------------ config edits

fn new_group_id(config: &Config) -> String {
    (1..)
        .map(|n| format!("g{n}"))
        .find(|id| config.groups.iter().all(|g| &g.id != id))
        .expect("unbounded")
}

pub fn create_group(config: &mut Config, name: &str) -> String {
    let name = name.trim();
    let name = if name.is_empty() { "New group" } else { name };
    let id = new_group_id(config);
    config.groups.push(GroupConfig {
        id: id.clone(),
        name: name.to_string(),
        ..Default::default()
    });
    id
}

pub fn rename_group(config: &mut Config, group: &str, name: &str) {
    let name = name.trim();
    if let Some(g) = config.groups.iter_mut().find(|g| g.id == group) {
        if !name.is_empty() {
            g.name = name.to_string();
        }
    }
}

pub fn delete_group(config: &mut Config, group: &str) {
    config.groups.retain(|g| g.id != group);
}

/// Moves a device into `group`, or out of every group when `group` is `None`.
pub fn move_to_group(config: &mut Config, device: &str, group: Option<&str>) {
    for g in &mut config.groups {
        g.members.retain(|m| m != device);
    }
    if let Some(g) = group.and_then(|id| config.groups.iter_mut().find(|g| g.id == id)) {
        g.members.push(device.to_string());
    }
}

pub fn set_sync(config: &mut Config, device: &str, sync: bool) {
    config.devices.entry(device.to_string()).or_default().sync = (!sync).then_some(false);
}

pub fn set_name(config: &mut Config, device: &str, name: &str, default_name: Option<&str>) {
    let name = name.trim();
    let custom = !name.is_empty() && Some(name) != default_name;
    config.devices.entry(device.to_string()).or_default().name = custom.then(|| name.to_string());
}

/// Applies the setup wizard in one go; group names that don't exist yet become new groups.
pub fn apply_setup(config: &mut Config, entries: &[SetupEntry], defaults: &[(String, String)]) {
    for entry in entries {
        let default_name = defaults
            .iter()
            .find(|(id, _)| id == &entry.id)
            .map(|(_, n)| n.as_str());
        set_name(config, &entry.id, &entry.name, default_name);
        set_sync(config, &entry.id, entry.sync);
        let group = entry
            .group
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|name| {
                match config
                    .groups
                    .iter()
                    .find(|g| g.name.eq_ignore_ascii_case(name))
                {
                    Some(g) => g.id.clone(),
                    None => create_group(config, name),
                }
            });
        move_to_group(config, &entry.id, group.as_deref());
    }
    config.setup_done = true;
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_device_goes_back_to_its_group_then_to_all_devices() {
        let wave = Effect {
            kind: EffectKind::Wave,
            colors: vec![[255, 0, 0]],
            ..Effect::default()
        };
        let mut config = Config::default();
        config.all_effect = Some(wave.clone());
        config.devices.insert(
            "a".into(),
            crate::engine::DeviceConfig {
                color: Some([0, 0, 255]),
                ..Default::default()
            },
        );
        let (name, effect) = resync_target(&config, "a", true).unwrap();
        assert_eq!(name, "All Devices");
        assert!(!shows(&config, "a", &effect));
        config.devices.get_mut("a").unwrap().effect = Some(wave.clone());
        assert!(shows(&config, "a", &effect));

        config.groups.push(GroupConfig {
            id: "g".into(),
            name: "Front fans".into(),
            members: vec!["a".into()],
            color: Some([0, 255, 0]),
            effect: None,
        });
        let (name, effect) = resync_target(&config, "a", true).unwrap();
        assert_eq!(name, "Front fans");
        assert!(!shows(&config, "a", &effect));
        assert!(resync_target(&config, "b", false).is_none());
    }
    use super::*;

    fn entry(id: &str, name: &str, group: Option<&str>, sync: bool) -> SetupEntry {
        SetupEntry {
            id: id.into(),
            name: name.into(),
            group: group.map(Into::into),
            sync,
        }
    }

    fn board() -> OrgbDevice {
        let zone = |name: &str, leds, min, max| Zone {
            name: name.into(),
            leds,
            min,
            max,
        };
        OrgbDevice {
            index: 3,
            name: "Z690 Hero".into(),
            vendor: "ASUS".into(),
            kind: "motherboard",
            serial: String::new(),
            location: "HID".into(),
            leds: 12,
            zones: vec![
                zone("Onboard", 4, 4, 4),
                zone("Addressable 1", 0, 0, 120),
                zone("Addressable 4", 8, 0, 120),
            ],
            modes: Vec::new(),
            mode_settings: Vec::new(),
            active_mode: String::new(),
            first_color: Some([1, 2, 3]),
            keys: None,
        }
    }

    #[test]
    fn headers_follow_the_board_until_they_get_their_own_lighting() {
        let device = board();
        let mut config = Config::default();
        let board_id = openrgb_id(&device);
        config.devices.entry(board_id.clone()).or_default().color = Some([255, 0, 0]);
        let parts = device_parts(&config, &device);
        assert_eq!(parts.len(), 1);
        assert_eq!(compose(&parts, 0), vec![[255, 0, 0]; 12]);

        let header = header_id(&board_id, 2);
        assert_eq!(split_id(&header), (board_id.as_str(), Some(2)));
        config.devices.entry(header).or_default().color = Some([0, 0, 255]);
        let frame = compose(&device_parts(&config, &device), 0);
        assert_eq!(frame[..4], [[255, 0, 0]; 4]);
        assert_eq!(frame[4..], [[0, 0, 255]; 8]);
    }

    #[test]
    fn a_delayed_device_shows_what_the_others_showed_earlier() {
        let device = board();
        let board_id = openrgb_id(&device);
        let effect = Effect {
            kind: EffectKind::Meteor,
            colors: vec![[255, 0, 0]],
            ..Effect::default()
        };
        let mut config = Config::default();
        config.devices.entry(board_id.clone()).or_default().effect = Some(effect);
        let in_step = device_parts(&config, &device);
        config.devices.get_mut(&board_id).unwrap().delay_ms = 700;
        let delayed = device_parts(&config, &device);
        assert_eq!(compose(&delayed, 1000), compose(&in_step, 300));
        assert_ne!(compose(&delayed, 1000), compose(&in_step, 1000));
    }

    #[test]
    fn moving_a_device_leaves_its_old_group() {
        let mut config = Config::default();
        let front = create_group(&mut config, "Front");
        let cables = create_group(&mut config, "Cables");
        move_to_group(&mut config, "fan", Some(&front));
        move_to_group(&mut config, "fan", Some(&cables));
        assert!(config.groups[0].members.is_empty());
        assert_eq!(config.groups[1].members, ["fan"]);
        move_to_group(&mut config, "fan", None);
        assert!(config.groups.iter().all(|g| g.members.is_empty()));
    }

    #[test]
    fn group_ids_stay_unique_after_deleting() {
        let mut config = Config::default();
        let a = create_group(&mut config, "A");
        let b = create_group(&mut config, "B");
        delete_group(&mut config, &a);
        let c = create_group(&mut config, "C");
        assert_ne!(b, c);
    }

    #[test]
    fn setup_reuses_groups_by_name_and_records_choices() {
        let mut config = Config::default();
        let existing = create_group(&mut config, "Front fans");
        let defaults = vec![("m1".to_string(), "Monitor".to_string())];
        apply_setup(
            &mut config,
            &[
                entry("f1", "Top", Some("front FANS"), true),
                entry("m1", "Monitor", Some("Desk"), false),
                entry("s1", "  ", None, true),
            ],
            &defaults,
        );
        assert!(config.setup_done);
        assert_eq!(config.groups.len(), 2);
        assert_eq!(config.groups[0].id, existing);
        assert_eq!(config.groups[0].members, ["f1"]);
        assert_eq!(config.groups[1].name, "Desk");
        assert_eq!(config.devices["f1"].name.as_deref(), Some("Top"));
        assert_eq!(config.devices["m1"].name, None);
        assert!(!config.devices["m1"].in_sync());
        assert!(config.devices["s1"].in_sync());
    }
}
