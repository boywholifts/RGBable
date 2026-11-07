//! Lighting effects rendered on the host. Every effect is a periodic function of a phase in
//! 0..1, so a loop of frames repeats seamlessly on devices that play animations themselves.

use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectKind {
    Static,
    Breathing,
    ColorCycle,
    Rainbow,
    RainbowWave,
    Wave,
    Meteor,
    Twinkle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effect {
    pub kind: EffectKind,
    #[serde(default)]
    pub colors: Vec<[u8; 3]>,
    /// 1 (slowest) to 5 (fastest).
    #[serde(default = "default_speed")]
    pub speed: u8,
    /// Percent, 0 to 100.
    #[serde(default = "default_brightness")]
    pub brightness: u8,
    #[serde(default)]
    pub reverse: bool,
}

fn default_speed() -> u8 {
    3
}
fn default_brightness() -> u8 {
    100
}

const FALLBACK_COLOR: [u8; 3] = [128, 0, 255];

impl Effect {
    pub fn is_animated(&self) -> bool {
        self.kind != EffectKind::Static
    }

    pub fn sanitized(mut self) -> Self {
        self.speed = self.speed.clamp(1, 5);
        self.brightness = self.brightness.min(100);
        self.colors.truncate(6);
        self
    }

    fn colors(&self) -> &[[u8; 3]] {
        if self.colors.is_empty() {
            std::slice::from_ref(&FALLBACK_COLOR)
        } else {
            &self.colors
        }
    }

    /// Length of one loop in milliseconds.
    pub fn period_ms(&self) -> u32 {
        let base = match self.kind {
            EffectKind::Static => 1000,
            EffectKind::Breathing => 4000 * self.colors().len() as u32,
            EffectKind::ColorCycle => 3000 * self.colors().len().max(2) as u32,
            EffectKind::Rainbow => 6000,
            EffectKind::RainbowWave | EffectKind::Wave => 4000,
            EffectKind::Meteor => 2400,
            EffectKind::Twinkle => 4000,
        };
        let factor = [2.0, 1.4, 1.0, 0.7, 0.5][usize::from(self.speed.clamp(1, 5) - 1)];
        (base as f32 * factor) as u32
    }

    /// The LEDs at `phase` (0..1 through the loop). `zones` are the LED counts of each
    /// fan or cable section; position-based effects run around each zone separately.
    pub fn frame(&self, zones: &[usize], phase: f32) -> Vec<[u8; 3]> {
        let phase = phase.rem_euclid(1.0);
        let colors = self.colors();
        let mut out = Vec::with_capacity(zones.iter().sum());
        let mut led_index = 0usize;
        for (zone, &count) in zones.iter().enumerate() {
            for i in 0..count {
                let mut p = i as f32 / count.max(1) as f32;
                if self.reverse {
                    p = 1.0 - p;
                }
                let color = match self.kind {
                    EffectKind::Static => colors[0],
                    EffectKind::Breathing => breathing(colors, phase),
                    EffectKind::ColorCycle => gradient(colors, phase),
                    EffectKind::Rainbow => hue(phase),
                    EffectKind::RainbowWave => hue(phase + p),
                    EffectKind::Wave => gradient(colors, phase + p),
                    EffectKind::Meteor => meteor(colors[zone % colors.len()], phase, p),
                    EffectKind::Twinkle => twinkle(colors, phase, led_index),
                };
                out.push(scale(color, self.brightness));
                led_index += 1;
            }
        }
        out
    }

    /// One loop as frames `interval_ms` apart.
    pub fn loop_frames(&self, zones: &[usize], interval_ms: u32) -> Vec<Vec<[u8; 3]>> {
        if !self.is_animated() {
            return vec![self.frame(zones, 0.0)];
        }
        let count = (self.period_ms() / interval_ms.max(1)).max(2);
        (0..count)
            .map(|i| self.frame(zones, i as f32 / count as f32))
            .collect()
    }
}

fn scale(color: [u8; 3], percent: u8) -> [u8; 3] {
    color.map(|c| (u16::from(c) * u16::from(percent) / 100) as u8)
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8)
}

fn dim(color: [u8; 3], level: f32) -> [u8; 3] {
    color.map(|c| (f32::from(c) * level.clamp(0.0, 1.0)).round() as u8)
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Fully saturated hue; `h` wraps at 1.
fn hue(h: f32) -> [u8; 3] {
    let h = h.rem_euclid(1.0) * 6.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    [r, g, b].map(|v: f32| (v * 255.0).round() as u8)
}

/// Smooth loop through the colours; a single colour fades to black and back.
fn gradient(colors: &[[u8; 3]], t: f32) -> [u8; 3] {
    let t = t.rem_euclid(1.0);
    if colors.len() == 1 {
        return dim(colors[0], 0.5 + 0.5 * (2.0 * PI * t).cos());
    }
    let pos = t * colors.len() as f32;
    let index = pos as usize % colors.len();
    mix(
        colors[index],
        colors[(index + 1) % colors.len()],
        smooth(pos.fract()),
    )
}

/// One breath per colour.
fn breathing(colors: &[[u8; 3]], t: f32) -> [u8; 3] {
    let pos = t * colors.len() as f32;
    let index = pos as usize % colors.len();
    let level = (PI * pos.fract()).sin().powi(2);
    dim(colors[index], level)
}

const METEOR_TAIL: f32 = 0.4;

fn meteor(color: [u8; 3], head: f32, p: f32) -> [u8; 3] {
    let behind = (head - p).rem_euclid(1.0);
    if behind >= METEOR_TAIL {
        return [0, 0, 0];
    }
    dim(color, (1.0 - behind / METEOR_TAIL).powi(2))
}

fn hash(n: usize) -> u32 {
    let mut x = (n as u32).wrapping_mul(0x9e37_79b9) ^ 0x85eb_ca6b;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^ (x >> 12)
}

/// Each LED flashes once or twice per loop at its own moment.
fn twinkle(colors: &[[u8; 3]], t: f32, led: usize) -> [u8; 3] {
    let h = hash(led);
    let offset = (h & 0xffff) as f32 / 65536.0;
    let repeats = 1 + (h >> 16) % 2;
    let local = (t * repeats as f32 + offset).fract();
    const FLASH: f32 = 0.18;
    if local >= FLASH {
        return [0, 0, 0];
    }
    let color = colors[(h >> 20) as usize % colors.len()];
    dim(color, (PI * local / FLASH).sin())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect(kind: EffectKind, colors: &[[u8; 3]]) -> Effect {
        Effect {
            kind,
            colors: colors.to_vec(),
            speed: 3,
            brightness: 100,
            reverse: false,
        }
    }

    const ALL: [EffectKind; 8] = [
        EffectKind::Static,
        EffectKind::Breathing,
        EffectKind::ColorCycle,
        EffectKind::Rainbow,
        EffectKind::RainbowWave,
        EffectKind::Wave,
        EffectKind::Meteor,
        EffectKind::Twinkle,
    ];

    #[test]
    fn frames_cover_every_led_and_loop_seamlessly() {
        let zones = [52, 52, 52];
        for kind in ALL {
            let e = effect(kind, &[[255, 0, 0], [0, 0, 255]]);
            assert_eq!(e.frame(&zones, 0.3).len(), 156, "{kind:?}");
            assert_eq!(
                e.frame(&zones, 0.0),
                e.frame(&zones, 1.0),
                "{kind:?} does not loop"
            );
        }
    }

    #[test]
    fn loop_length_follows_speed_and_interval() {
        let slow = Effect {
            speed: 1,
            ..effect(EffectKind::Rainbow, &[])
        };
        let fast = Effect {
            speed: 5,
            ..effect(EffectKind::Rainbow, &[])
        };
        assert_eq!(slow.loop_frames(&[45], 40).len(), 300);
        assert_eq!(fast.loop_frames(&[45], 40).len(), 75);
        assert_eq!(
            effect(EffectKind::Static, &[[1, 2, 3]]).loop_frames(&[4], 40),
            vec![vec![[1, 2, 3]; 4]]
        );
    }

    #[test]
    fn brightness_scales_and_breathing_reaches_black_and_full() {
        let half = Effect {
            brightness: 50,
            ..effect(EffectKind::Static, &[[200, 100, 0]])
        };
        assert_eq!(half.frame(&[1], 0.0), vec![[100, 50, 0]]);
        let breath = effect(EffectKind::Breathing, &[[0, 255, 0]]);
        assert_eq!(breath.frame(&[1], 0.0), vec![[0, 0, 0]]);
        assert_eq!(breath.frame(&[1], 0.5), vec![[0, 255, 0]]);
    }

    #[test]
    fn meteor_head_is_brightest_and_tail_fades() {
        let e = effect(EffectKind::Meteor, &[[255, 255, 255]]);
        let frame = e.frame(&[10], 0.5);
        assert_eq!(frame[5], [255, 255, 255]);
        assert!(frame[4][0] < 255 && frame[4][0] > frame[3][0]);
        assert_eq!(frame[6], [0, 0, 0]);
    }

    #[test]
    fn old_configs_without_optional_fields_load() {
        let e: Effect = serde_json::from_str(r#"{"kind":"rainbow_wave"}"#).unwrap();
        assert_eq!((e.speed, e.brightness, e.reverse), (3, 100, false));
    }
}
