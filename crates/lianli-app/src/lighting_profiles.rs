//! Lighting profiles: a snapshot of everything that decides how every device lights up,
//! saved by name and put back in one go. Devices not connected when a profile is applied
//! keep their part of it in the settings and pick it up when they return.

use crate::effects::{Effect, Span};
use crate::engine::{Config, DeviceConfig, OledConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One device's lighting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedLight {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outer_effect: Option<Effect>,
    #[serde(default)]
    pub delay_ms: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<Span>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reversed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub off: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warmth: Option<i8>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lanes: Vec<Option<Effect>>,
}

impl SavedLight {
    fn of(device: &DeviceConfig) -> Self {
        Self {
            color: device.color,
            sync: device.sync,
            effect: device.effect.clone(),
            outer_effect: device.outer_effect.clone(),
            delay_ms: device.delay_ms,
            span: device.span,
            turn: device.turn,
            reversed: device.reversed,
            off: device.off,
            warmth: device.warmth,
            lanes: device.lanes.clone(),
        }
    }

    fn put(&self, device: &mut DeviceConfig) {
        device.color = self.color;
        device.sync = self.sync;
        device.effect = self.effect.clone();
        device.outer_effect = self.outer_effect.clone();
        device.delay_ms = self.delay_ms;
        device.span = self.span;
        device.turn = self.turn;
        device.reversed = self.reversed;
        device.off = self.off;
        device.warmth = self.warmth;
        device.lanes = self.lanes.clone();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedGroup {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LightingProfile {
    pub name: String,
    #[serde(default)]
    pub devices: BTreeMap<String, SavedLight>,
    /// Group lighting by group id; groups deleted since are skipped.
    #[serde(default)]
    pub groups: BTreeMap<String, SavedGroup>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oled_ring: Option<[u8; 3]>,
    #[serde(default)]
    pub warmth: i8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oled_match: Option<u8>,
    /// What the HydroShift screen plays. Profiles saved before screens were part of them
    /// leave the screen alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oled: Option<OledConfig>,
    /// Every light off; applying a profile without it turns them back on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_off: bool,
}

/// Every device's and group's lighting as the settings have it now.
pub fn capture(config: &Config, name: &str) -> LightingProfile {
    LightingProfile {
        name: name.to_string(),
        devices: config
            .devices
            .iter()
            .map(|(id, device)| (id.clone(), SavedLight::of(device)))
            .filter(|(_, light)| *light != SavedLight::default())
            .collect(),
        groups: config
            .groups
            .iter()
            .map(|g| {
                (
                    g.id.clone(),
                    SavedGroup {
                        color: g.color,
                        effect: g.effect.clone(),
                    },
                )
            })
            .collect(),
        oled_ring: config.oled_ring,
        warmth: config.warmth,
        oled_match: config.oled_match,
        oled: config.oled.clone(),
        all_off: config.all_off,
    }
}

/// The video a profile puts on the screen, when it has one that is still there and isn't
/// already playing.
pub fn screen_change<'a>(config: &Config, profile: &'a LightingProfile) -> Option<&'a OledConfig> {
    profile
        .oled
        .as_ref()
        .filter(|oled| oled.file.is_file() && config.oled.as_ref() != Some(oled))
}

/// Puts a profile's lighting into the settings. Devices it doesn't mention go back to their
/// defaults, so the profile looks the same however the lighting was before; their names,
/// zone sizes and whether they're hidden stay as they are.
pub fn apply(config: &mut Config, profile: &LightingProfile) {
    for (id, device) in config.devices.iter_mut() {
        profile
            .devices
            .get(id)
            .cloned()
            .unwrap_or_default()
            .put(device);
    }
    for (id, light) in &profile.devices {
        if !config.devices.contains_key(id) {
            light.put(config.devices.entry(id.clone()).or_default());
        }
    }
    for group in &mut config.groups {
        if let Some(saved) = profile.groups.get(&group.id) {
            group.color = saved.color;
            group.effect = saved.effect.clone();
        }
    }
    config.oled_ring = profile.oled_ring;
    config.warmth = profile.warmth;
    config.oled_match = profile.oled_match;
    config.all_off = profile.all_off;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::GroupConfig;

    fn config() -> Config {
        let mut config: Config = serde_json::from_str("{}").unwrap();
        config.devices.insert(
            "fan".into(),
            DeviceConfig {
                name: Some("Top fan".into()),
                color: Some([255, 0, 0]),
                delay_ms: 300,
                hidden: true,
                ..DeviceConfig::default()
            },
        );
        config.groups.push(GroupConfig {
            id: "g1".into(),
            name: "Front".into(),
            members: vec!["fan".into()],
            color: Some([0, 255, 0]),
            effect: None,
        });
        config.warmth = 10;
        config
    }

    #[test]
    fn puts_back_what_it_saved_and_keeps_the_rest() {
        let mut config = config();
        let profile = capture(&config, "Red");
        config.devices.get_mut("fan").unwrap().color = Some([0, 0, 255]);
        config.devices.get_mut("fan").unwrap().delay_ms = 0;
        config.devices.insert(
            "strimer".into(),
            DeviceConfig {
                color: Some([9, 9, 9]),
                ..DeviceConfig::default()
            },
        );
        config.groups[0].color = Some([1, 1, 1]);
        config.warmth = -20;

        apply(&mut config, &profile);
        let fan = &config.devices["fan"];
        assert_eq!((fan.color, fan.delay_ms), (Some([255, 0, 0]), 300));
        // not lighting: left alone
        assert_eq!((fan.name.as_deref(), fan.hidden), (Some("Top fan"), true));
        // a device the profile didn't know goes back to its defaults
        assert_eq!(config.devices["strimer"].color, None);
        assert_eq!(config.groups[0].color, Some([0, 255, 0]));
        assert_eq!(config.warmth, 10);
    }

    #[test]
    fn keeps_lighting_for_devices_that_are_away() {
        let mut config = config();
        let mut profile = capture(&config, "Away");
        profile.devices.insert(
            "keyboard".into(),
            SavedLight {
                color: Some([7, 7, 7]),
                ..SavedLight::default()
            },
        );
        config.groups.clear();
        apply(&mut config, &profile);
        assert_eq!(config.devices["keyboard"].color, Some([7, 7, 7]));
        assert!(config.groups.is_empty());
    }
}
