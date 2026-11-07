//! One list of every light RGBable controls, Lian Li devices and OpenRGB devices alike,
//! with the user's names, groups and "Sync everything" choices from the config.

use crate::effects::{Effect, EffectKind};
use crate::engine::{self, Config, Engine, GroupConfig};
use crate::openrgb::{OpenRgb, OrgbDevice};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::warn;

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
    pub sync: bool,
    pub group: Option<String>,
    /// OpenRGB zones whose LED count the user can set (addressable headers).
    pub zones: Vec<ZoneView>,
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
                zones: Vec::new(),
                sync: saved.is_none_or(|s| s.in_sync()),
                group: group_of(&config, &d.id),
                default_name: d.default_name,
                name: d.name,
                id: d.id,
            }
        })
        .collect();
    for device in openrgb.view().devices {
        let id = openrgb_id(&device);
        let saved = config.devices.get(&id);
        list.push(LightView {
            source: "openrgb",
            name: saved
                .and_then(|s| s.name.clone())
                .unwrap_or_else(|| device.name.clone()),
            default_name: device.name.clone(),
            kind: device.kind.to_string(),
            detail: format!("{} LEDs via OpenRGB", device.leds),
            color: saved.and_then(|s| s.color).or(device.first_color),
            effect: saved.and_then(|s| s.effect.clone()),
            zones: device
                .zones
                .iter()
                .enumerate()
                .filter(|(_, z)| z.resizable())
                .map(|(index, z)| ZoneView {
                    index,
                    name: z.name.clone(),
                    leds: z.leds,
                    min: z.min,
                    max: z.max,
                })
                .collect(),
            sync: saved.is_none_or(|s| s.in_sync()),
            group: group_of(&config, &id),
            id,
        });
    }
    list
}

pub fn groups(engine: &Engine) -> Vec<GroupView> {
    engine
        .config()
        .groups
        .into_iter()
        .map(|g| GroupView {
            id: g.id,
            name: g.name,
            color: g.color,
            effect: g.effect,
            members: g.members,
        })
        .collect()
}

fn openrgb_device(openrgb: &OpenRgb, id: &str) -> Option<OrgbDevice> {
    openrgb
        .view()
        .devices
        .into_iter()
        .find(|d| openrgb_id(d) == id)
}

fn openrgb_zones(device: &OrgbDevice) -> Vec<usize> {
    let zones: Vec<usize> = device
        .zones
        .iter()
        .map(|zone| zone.leds)
        .filter(|&n| n > 0)
        .collect();
    if zones.iter().sum::<usize>() == device.leds {
        zones
    } else {
        vec![device.leds]
    }
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
            .entry(id.to_string())
            .or_default()
            .zone_sizes
            .insert(zone, size);
    });
    sync_openrgb_effects(engine, openrgb);
    let saved = engine.config().devices.get(id).cloned().unwrap_or_default();
    if saved.effect.is_none() {
        if let Some(color) = saved.color {
            openrgb.set_color(device.index, color)?;
        }
    }
    Ok(())
}

/// After (re)connecting: restore zone sizes the user set, then the saved effects.
pub fn after_openrgb_connect(engine: &Engine, openrgb: &OpenRgb) {
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

/// Hands OpenRGB the animated effects saved in the config for the devices it has now.
pub fn sync_openrgb_effects(engine: &Engine, openrgb: &OpenRgb) {
    let config = engine.config();
    let effects: BTreeMap<u32, (Effect, Vec<usize>)> = openrgb
        .view()
        .devices
        .iter()
        .filter_map(|device| {
            let effect = config.devices.get(&openrgb_id(device))?.effect.clone()?;
            Some((device.index, (effect, openrgb_zones(device))))
        })
        .collect();
    openrgb.set_animations(effects);
}

pub fn static_effect(color: [u8; 3]) -> Effect {
    Effect {
        kind: EffectKind::Static,
        colors: vec![color],
        speed: 3,
        brightness: 100,
        reverse: false,
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
    sync_openrgb_effects(engine, openrgb);
    if !animated {
        openrgb.set_color(device.index, color)?;
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
    engine.set_all_effect(&effect);
    if !include_openrgb {
        return Ok(());
    }
    let mut failed = Vec::new();
    for light in lights(engine, openrgb)
        .into_iter()
        .filter(|l| l.source == "openrgb" && l.sync)
    {
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
    let saved = engine.config().devices.get(id).cloned().unwrap_or_default();
    let restore = saved.color.or(device.first_color).unwrap_or([0, 0, 0]);
    openrgb.stop_animation(device.index);
    let engine = engine.clone();
    let openrgb = openrgb.clone();
    std::thread::Builder::new()
        .name("identify-openrgb".into())
        .spawn(move || {
            for step in 0..engine::IDENTIFY_STEPS {
                let color = if step % 2 == 0 {
                    [255, 255, 255]
                } else {
                    [0, 0, 0]
                };
                let _ = openrgb.set_color(device.index, color);
                std::thread::sleep(engine::IDENTIFY_STEP);
            }
            if saved.effect.is_some() {
                sync_openrgb_effects(&engine, &openrgb);
            } else if let Err(e) = openrgb.set_color(device.index, restore) {
                tracing::warn!("{}: restoring after identify: {e:#}", device.name);
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
    use super::*;

    fn entry(id: &str, name: &str, group: Option<&str>, sync: bool) -> SetupEntry {
        SetupEntry {
            id: id.into(),
            name: name.into(),
            group: group.map(Into::into),
            sync,
        }
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
