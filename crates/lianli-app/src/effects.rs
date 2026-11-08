//! Lighting effects rendered on the host. Every effect is a periodic function of a phase in
//! 0..1, so a loop of frames repeats seamlessly on devices that play animations themselves.

use lianli_shared::rgb::{RgbDirection, RgbMode};
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
    Strobe,
    Heartbeat,
    Fire,
    Scanner,
    Runway,
    Fill,
    Ripple,
    StarryNight,
    Aurora,
    Police,
    Drizzle,
    /// Layers of other effects blended together (the effect builder).
    Custom,
    /// A Rhai script that colors each LED.
    Script,
    /// One of L-Connect's own effects (`mode`), rendered by the Lian Li device renderer.
    /// Devices that don't have it show the closest RGBable effect.
    Lianli,
}

/// Which ring of an SL Flex fan an effect is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    #[default]
    Both,
    Inner,
    Outer,
}

/// A device's place on one long strip, in thousandths: a whole device that runs an effect
/// across all its fans is 0..1000, the second of four chained devices 250..500.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u16,
    pub len: u16,
}

impl Span {
    pub const WHOLE: Self = Self {
        start: 0,
        len: 1000,
    };

    fn start(self) -> f32 {
        f32::from(self.start.min(999)) / 1000.0
    }

    fn len(self) -> f32 {
        f32::from(self.len.clamp(1, 1000)) / 1000.0
    }
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
    #[serde(default)]
    pub side: Side,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<RgbMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<RgbDirection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<Layer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// The recipe-builder settings a script was generated from, as JSON, so it can be reopened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<String>,
    /// Where this device sits on a strip that runs across its fans or through several devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    /// The share of a longer loop this device gets when devices take turns; it is dark the
    /// rest of the time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<Span>,
    /// Name of a saved custom effect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Blend {
    #[default]
    Over,
    Add,
    Multiply,
    Brightest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    pub effect: Effect,
    #[serde(default)]
    pub blend: Blend,
    /// Percent, 0 to 100.
    #[serde(default = "default_brightness")]
    pub opacity: u8,
}

pub const MAX_LAYERS: usize = 6;

fn default_speed() -> u8 {
    3
}
fn default_brightness() -> u8 {
    100
}

const FALLBACK_COLOR: [u8; 3] = [128, 0, 255];
/// Fire and aurora change every LED every frame; rendering them in small blocks keeps their
/// loops small enough for the wireless devices' animation memory.
const COARSE_BLOCK: usize = 4;

impl Default for Effect {
    fn default() -> Self {
        Self {
            kind: EffectKind::Static,
            colors: Vec::new(),
            speed: default_speed(),
            brightness: default_brightness(),
            reverse: false,
            side: Side::Both,
            mode: None,
            direction: None,
            layers: Vec::new(),
            script: None,
            recipe: None,
            name: None,
            span: None,
            turn: None,
        }
    }
}

impl Effect {
    /// Each layer loops a whole number of times within the longest layer's loop, so the
    /// combination still repeats seamlessly.
    fn layered(&self, zones: &[usize], phase: f32) -> Vec<[u8; 3]> {
        let leds: usize = zones.iter().sum();
        let period = self.period_ms() as f32;
        let mut out = vec![[0f32; 3]; leds];
        for layer in &self.layers {
            let effect = layer.effect.generic();
            let repeats = (period / effect.period_ms().max(1) as f32).round().max(1.0);
            let frame = effect.frame(zones, (phase * repeats).fract());
            let opacity = f32::from(layer.opacity.min(100)) / 100.0;
            for (target, color) in out.iter_mut().zip(frame) {
                for i in 0..3 {
                    let c = f32::from(color[i]);
                    let blended = match layer.blend {
                        Blend::Over => c,
                        Blend::Add => target[i] + c,
                        Blend::Multiply => target[i] * c / 255.0,
                        Blend::Brightest => target[i].max(c),
                    };
                    target[i] += (blended.min(255.0) - target[i]) * opacity;
                }
            }
        }
        out.into_iter()
            .map(|c| {
                scale(
                    c.map(|v| v.round().clamp(0.0, 255.0) as u8),
                    self.brightness,
                )
            })
            .collect()
    }

    /// The RGBable effect closest to an L-Connect one, for devices without a Lian Li renderer.
    pub fn generic(&self) -> Effect {
        if self.kind != EffectKind::Lianli {
            return self.clone();
        }
        let kind = match self.mode {
            Some(RgbMode::Static) => EffectKind::Static,
            Some(RgbMode::Rainbow) => EffectKind::Rainbow,
            Some(RgbMode::RainbowMorph | RgbMode::Endless) => EffectKind::RainbowWave,
            Some(RgbMode::Breathing) => EffectKind::Breathing,
            Some(RgbMode::Runway | RgbMode::Staggered) => EffectKind::Runway,
            Some(RgbMode::Meteor | RgbMode::Pioneer) => EffectKind::Meteor,
            Some(RgbMode::Tide | RgbMode::Mixing | RgbMode::River | RgbMode::GradientRibbon) => {
                EffectKind::Wave
            }
            Some(RgbMode::Render | RgbMode::Stack | RgbMode::Hourglass) => EffectKind::Fill,
            Some(
                RgbMode::PingPong
                | RgbMode::Collide
                | RgbMode::Reflect
                | RgbMode::Duel
                | RgbMode::ShuttleRun,
            ) => EffectKind::Scanner,
            Some(RgbMode::Ripple) => EffectKind::Ripple,
            Some(RgbMode::ElectricCurrent) => EffectKind::Strobe,
            Some(RgbMode::Twinkle) => EffectKind::Twinkle,
            Some(RgbMode::Drizzling) => EffectKind::Drizzle,
            _ => EffectKind::ColorCycle,
        };
        Effect {
            kind,
            mode: None,
            ..self.clone()
        }
    }

    /// The span, for effects that move along the LEDs; effects that change every LED at once
    /// (breathing, strobe) look the same on any span, so they ignore it.
    pub fn travel_span(&self) -> Option<Span> {
        let moves = matches!(
            self.generic().kind,
            EffectKind::RainbowWave
                | EffectKind::Wave
                | EffectKind::Meteor
                | EffectKind::Scanner
                | EffectKind::Runway
                | EffectKind::Fill
                | EffectKind::Ripple
                | EffectKind::Aurora
                | EffectKind::Police
                | EffectKind::Drizzle
        );
        self.span.filter(|_| moves)
    }

    pub fn is_animated(&self) -> bool {
        self.kind != EffectKind::Static
    }

    pub fn sanitized(mut self) -> Self {
        self.speed = self.speed.clamp(1, 5);
        self.brightness = self.brightness.min(100);
        self.colors.truncate(6);
        self.layers.truncate(MAX_LAYERS);
        for layer in &mut self.layers {
            layer.opacity = layer.opacity.min(100);
            layer.effect.layers.clear();
            layer.effect.script = None;
            layer.effect.recipe = None;
            if matches!(layer.effect.kind, EffectKind::Custom | EffectKind::Script) {
                layer.effect.kind = EffectKind::Static;
            }
            layer.effect = std::mem::take(&mut layer.effect).sanitized();
        }
        if let Some(script) = &mut self.script {
            script.truncate(crate::script::MAX_SCRIPT_LEN);
        }
        if let Some(recipe) = &mut self.recipe {
            recipe.truncate(4096);
        }
        self
    }

    fn colors(&self) -> &[[u8; 3]] {
        if self.colors.is_empty() {
            std::slice::from_ref(&FALLBACK_COLOR)
        } else {
            &self.colors
        }
    }

    /// Effects that move one LED at a time look smooth only when each frame is one step;
    /// in-between frames blur the drops and make the loop too big for wireless devices.
    pub fn steps(&self, zones: &[usize]) -> Option<usize> {
        if self.turn.is_some() {
            return None;
        }
        match self.generic().kind {
            EffectKind::Drizzle => {
                let longest = match self.travel_span() {
                    Some(span) => {
                        (zones.iter().sum::<usize>() as f32 / span.len()).round() as usize
                    }
                    None => zones.iter().copied().max().unwrap_or(1),
                }
                .max(1);
                Some(self.colors().len() * 2 * longest)
            }
            _ => None,
        }
    }

    /// Length of one loop in milliseconds.
    pub fn period_ms(&self) -> u32 {
        if let Some(turn) = self.turn {
            let own = Effect {
                turn: None,
                ..self.clone()
            };
            return (own.period_ms() as f32 / stretch(turn.len())) as u32;
        }
        if self.kind == EffectKind::Lianli {
            return self.generic().period_ms();
        }
        if self.kind == EffectKind::Custom {
            return self
                .layers
                .iter()
                .map(|l| l.effect.period_ms())
                .max()
                .unwrap_or(1000);
        }
        let base = match self.kind {
            EffectKind::Static => 1000,
            EffectKind::Breathing => 4000 * self.colors().len() as u32,
            EffectKind::ColorCycle => 3000 * self.colors().len().max(2) as u32,
            EffectKind::Rainbow => 6000,
            EffectKind::RainbowWave | EffectKind::Wave => 4000,
            EffectKind::Meteor => 2400,
            EffectKind::Twinkle => 4000,
            EffectKind::Strobe => 1200 * self.colors().len() as u32,
            EffectKind::Heartbeat => 1600 * self.colors().len() as u32,
            EffectKind::Fire => 2400,
            EffectKind::Scanner => 2400,
            EffectKind::Runway => 3000,
            EffectKind::Fill => 3000 * self.colors().len() as u32,
            EffectKind::Ripple => 3000,
            EffectKind::StarryNight => 4000,
            EffectKind::Aurora => 8000,
            EffectKind::Police => 1600,
            EffectKind::Drizzle => 2400 * self.colors().len() as u32,
            EffectKind::Lianli => 3000,
            EffectKind::Custom => 1000,
            EffectKind::Script => 4000,
        };
        let factor = [2.0, 1.4, 1.0, 0.7, 0.5][usize::from(self.speed.clamp(1, 5) - 1)];
        (base as f32 * factor / self.travel_span().map_or(1.0, |s| stretch(s.len()))) as u32
    }

    /// The LEDs at `phase` (0..1 through the loop). `zones` are the LED counts of each
    /// fan or cable section; position-based effects run around each zone separately.
    pub fn frame(&self, zones: &[usize], phase: f32) -> Vec<[u8; 3]> {
        if let Some(turn) = self.turn {
            let phase = phase.rem_euclid(1.0);
            let offset = phase - turn.start();
            if !(0.0..turn.len()).contains(&offset) {
                return vec![[0, 0, 0]; zones.iter().sum()];
            }
            let own = Effect {
                turn: None,
                ..self.clone()
            };
            return own.frame(zones, offset / turn.len());
        }
        if self.kind == EffectKind::Lianli {
            return self.generic().frame(zones, phase);
        }
        if self.kind == EffectKind::Custom {
            return self.layered(zones, phase);
        }
        if self.kind == EffectKind::Script {
            let source = self.script.as_deref().unwrap_or("");
            let frame =
                crate::script::render_frame(source, self.colors(), zones, phase.rem_euclid(1.0))
                    .unwrap_or_else(|_| vec![[0, 0, 0]; zones.iter().sum()]);
            return frame
                .into_iter()
                .map(|c| scale(c, self.brightness))
                .collect();
        }
        let phase = phase.rem_euclid(1.0);
        let colors = self.colors();
        let span = self.travel_span();
        let whole_device = [zones.iter().sum::<usize>()];
        let zones = if span.is_some() {
            &whole_device[..]
        } else {
            zones
        };
        let (start, len) = span.map_or((0.0, 1.0), |s| (s.start(), s.len()));
        let mut out = Vec::with_capacity(zones.iter().sum());
        let mut led_index = 0usize;
        for (zone, &count) in zones.iter().enumerate() {
            let coarse = matches!(self.kind, EffectKind::Fire | EffectKind::Aurora);
            let strip_leds = (count as f32 / len).round().max(1.0) as usize;
            for i in 0..count {
                let i = if coarse { i - i % COARSE_BLOCK } else { i };
                let mut local_end = if count > 1 {
                    i as f32 / (count - 1) as f32
                } else {
                    0.5
                };
                let mut local = i as f32 / count.max(1) as f32;
                // reversed runs the other way within its own part of the strip
                if self.reverse {
                    local = 1.0 - local;
                    local_end = 1.0 - local_end;
                }
                let p = start + len * local;
                let end_to_end = start + len * local_end;
                let color = match self.kind {
                    EffectKind::Static => colors[0],
                    EffectKind::Breathing => breathing(colors, phase),
                    EffectKind::ColorCycle => gradient(colors, phase),
                    EffectKind::Rainbow => hue(phase),
                    EffectKind::RainbowWave => hue(phase + p),
                    EffectKind::Wave => gradient(colors, phase + p),
                    EffectKind::Meteor => meteor(colors[zone % colors.len()], phase, p, len),
                    EffectKind::Twinkle => twinkle(colors, phase, led_index),
                    EffectKind::Strobe => strobe(colors, phase),
                    EffectKind::Heartbeat => heartbeat(colors, phase),
                    EffectKind::Fire => fire(phase, p, led_index - led_index % COARSE_BLOCK),
                    EffectKind::Scanner => scanner(colors[zone % colors.len()], phase, end_to_end),
                    EffectKind::Runway => runway(colors, phase, p),
                    EffectKind::Fill => fill(colors, phase, p),
                    EffectKind::Ripple => ripple(colors, phase, end_to_end),
                    EffectKind::StarryNight => starry_night(colors, phase, led_index),
                    EffectKind::Aurora => aurora(colors, phase, p),
                    EffectKind::Police => police(colors, phase, p),
                    EffectKind::Drizzle => drizzle(colors, phase, p, strip_leds, zone),
                    EffectKind::Lianli | EffectKind::Custom | EffectKind::Script => colors[0],
                };
                out.push(scale(color, self.brightness));
                led_index += 1;
            }
        }
        out
    }

    #[cfg(test)]
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

/// How much longer a loop gets when a device is one part of a flow or one of several turns.
/// In proportion (a whole strip of 15 devices taking 15 loops) it crawled, so it grows with
/// the square root instead and the effect moves faster through each device.
fn stretch(share: f32) -> f32 {
    share.sqrt()
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

/// Smooth loop through the colors; a single color fades to black and back.
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

/// One breath per color.
fn breathing(colors: &[[u8; 3]], t: f32) -> [u8; 3] {
    let pos = t * colors.len() as f32;
    let index = pos as usize % colors.len();
    let level = (PI * pos.fract()).sin().powi(2);
    dim(colors[index], level)
}

const METEOR_TAIL: f32 = 0.4;

/// `share` is the part of the strip this device holds: in a flow the tail stays about
/// as long as on one device instead of lighting several devices at once.
fn meteor(color: [u8; 3], head: f32, p: f32, share: f32) -> [u8; 3] {
    let tail = METEOR_TAIL * share;
    let behind = (head - p).rem_euclid(1.0);
    if behind >= tail {
        return [0, 0, 0];
    }
    dim(color, (1.0 - behind / tail).powi(2))
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

fn bump(x: f32, width: f32) -> f32 {
    if x.abs() >= width {
        0.0
    } else {
        (PI * 0.5 * x / width).cos().powi(2)
    }
}

/// Four quick flashes per color.
fn strobe(colors: &[[u8; 3]], t: f32) -> [u8; 3] {
    let flashes = t * colors.len() as f32 * 4.0;
    if flashes.fract() < 0.3 {
        colors[(flashes as usize / 4) % colors.len()]
    } else {
        [0, 0, 0]
    }
}

/// A strong beat, a softer second beat, then rest; one heartbeat per color.
fn heartbeat(colors: &[[u8; 3]], t: f32) -> [u8; 3] {
    let pos = t * colors.len() as f32;
    let local = pos.fract();
    let level = bump(local - 0.08, 0.08).max(0.6 * bump(local - 0.3, 0.08));
    dim(colors[pos as usize % colors.len()], level)
}

const FIRE: [[u8; 3]; 4] = [[40, 0, 0], [200, 20, 0], [255, 110, 0], [255, 200, 40]];

/// Flicker built from whole-number frequencies, so it loops without a seam.
fn fire(t: f32, p: f32, led: usize) -> [u8; 3] {
    let h = hash(led);
    let o1 = (h & 0xff) as f32 / 256.0;
    let o2 = ((h >> 8) & 0xff) as f32 / 256.0;
    let o3 = ((h >> 16) & 0xff) as f32 / 256.0;
    let heat = 0.55
        + 0.22 * (2.0 * PI * (2.0 * t + o1 + p)).sin()
        + 0.15 * (2.0 * PI * (5.0 * t + o2)).sin()
        + 0.1 * (2.0 * PI * (11.0 * t + o3)).sin();
    let pos = heat.clamp(0.0, 0.999) * (FIRE.len() - 1) as f32;
    let index = pos as usize;
    mix(FIRE[index], FIRE[index + 1], pos.fract())
}

/// A light sweeping from one end to the other and back.
fn scanner(color: [u8; 3], t: f32, x: f32) -> [u8; 3] {
    let head = 1.0 - (2.0 * t - 1.0).abs();
    dim(color, bump(x - head, 0.18))
}

const RUNWAY_BLOCKS: f32 = 4.0;

/// Blocks of color marching along; a single color alternates with dark gaps.
fn runway(colors: &[[u8; 3]], t: f32, p: f32) -> [u8; 3] {
    let u = (p * RUNWAY_BLOCKS - t).rem_euclid(1.0);
    if colors.len() == 1 {
        return if u < 0.5 { colors[0] } else { [0, 0, 0] };
    }
    colors[(u * colors.len() as f32) as usize % colors.len()]
}

/// Fills with a color, drains, then the next color.
fn fill(colors: &[[u8; 3]], t: f32, p: f32) -> [u8; 3] {
    let pos = t * colors.len() as f32;
    let local = pos.fract();
    let color = colors[pos as usize % colors.len()];
    let lit = if local < 0.5 {
        p < local * 2.0
    } else {
        p >= (local - 0.5) * 2.0
    };
    if lit {
        color
    } else {
        [0, 0, 0]
    }
}

/// Rings spreading out from the middle; each ring takes the next color.
fn ripple(colors: &[[u8; 3]], t: f32, x: f32) -> [u8; 3] {
    let distance = (x - 0.5).abs() * 2.0;
    let rings = t * 2.0;
    let radius = rings.fract() * 1.3;
    dim(
        colors[rings as usize % colors.len()],
        bump(distance - radius, 0.25),
    )
}

/// A dim base color with stars twinkling over it.
fn starry_night(colors: &[[u8; 3]], t: f32, led: usize) -> [u8; 3] {
    let base = dim(colors[0], 0.25);
    let star = if colors.len() > 1 {
        &colors[1..]
    } else {
        &[[255, 255, 255]]
    };
    let sparkle = twinkle(star, t, led);
    [0, 1, 2].map(|i| base[i].max(sparkle[i]))
}

/// Slowly drifting bands of color.
fn aurora(colors: &[[u8; 3]], t: f32, p: f32) -> [u8; 3] {
    let drift = 0.18 * (2.0 * PI * (t + p)).sin() + 0.08 * (2.0 * PI * (2.0 * t - p)).sin();
    let level = 0.55 + 0.45 * (2.0 * PI * (t + 2.0 * p)).sin().abs();
    dim(gradient(colors, p * 0.5 + drift + t), level)
}

const DRIZZLE_TAIL: [f32; 5] = [1.0, 0.5, 0.125, 0.03, 0.0];

/// L-Connect's Drizzling: pairs of short drops run along each zone with a lane-length gap
/// after them. Each pair takes the next color, and zones start at different points.
fn drizzle(colors: &[[u8; 3]], t: f32, p: f32, count: usize, zone: usize) -> [u8; 3] {
    let length = count.max(1) as f32;
    let cycle = length * 2.0;
    let offset = (hash(zone) as usize % count.max(1)) as f32;
    let travelled = t * colors.len() as f32 * cycle;
    let along = p * length + offset + travelled;
    let pattern = along.rem_euclid(cycle);
    let second = (length / 2.0).ceil();
    let tail = |d: f32| {
        if !(0.0..4.0).contains(&d) {
            return 0.0;
        }
        let i = d as usize;
        DRIZZLE_TAIL[i] + (DRIZZLE_TAIL[i + 1] - DRIZZLE_TAIL[i]) * d.fract()
    };
    let level = tail(pattern).max(tail(pattern - second));
    let drop = (along / cycle).floor() as i64 + zone as i64;
    dim(colors[drop.rem_euclid(colors.len() as i64) as usize], level)
}

/// Two halves flashing twice each, in turn.
fn police(colors: &[[u8; 3]], t: f32, p: f32) -> [u8; 3] {
    let side = (t * 2.0) as usize % 2;
    let on = (t * 8.0).fract() < 0.4;
    let in_half = if side == 0 { p < 0.5 } else { p >= 0.5 };
    if on && in_half {
        colors[side % colors.len()]
    } else {
        [0, 0, 0]
    }
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
            ..Effect::default()
        }
    }

    const ALL: [EffectKind; 19] = [
        EffectKind::Static,
        EffectKind::Breathing,
        EffectKind::ColorCycle,
        EffectKind::Rainbow,
        EffectKind::RainbowWave,
        EffectKind::Wave,
        EffectKind::Meteor,
        EffectKind::Twinkle,
        EffectKind::Strobe,
        EffectKind::Heartbeat,
        EffectKind::Fire,
        EffectKind::Scanner,
        EffectKind::Runway,
        EffectKind::Fill,
        EffectKind::Ripple,
        EffectKind::StarryNight,
        EffectKind::Aurora,
        EffectKind::Police,
        EffectKind::Drizzle,
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
    fn a_spanned_meteor_travels_from_one_fan_and_device_into_the_next() {
        let lit = |frame: &[[u8; 3]]| frame.iter().any(|c| c[0] > 100);
        let across = Effect {
            span: Some(Span::WHOLE),
            ..effect(EffectKind::Meteor, &[[255, 0, 0]])
        };
        let frame = across.frame(&[13, 13, 13], 0.2);
        assert!(
            lit(&frame[..13]) && !lit(&frame[26..]),
            "only the first fan near the start"
        );
        let frame = across.frame(&[13, 13, 13], 0.9);
        assert!(
            lit(&frame[26..]) && !lit(&frame[..10]),
            "the last fan near the end"
        );

        let first = Effect {
            span: Some(Span { start: 0, len: 500 }),
            ..across.clone()
        };
        let second = Effect {
            span: Some(Span {
                start: 500,
                len: 500,
            }),
            ..across.clone()
        };
        assert_eq!(first.period_ms(), second.period_ms());
        assert_eq!(
            first.period_ms(),
            (across.period_ms() as f32 / stretch(0.5)) as u32
        );
        assert!(lit(&first.frame(&[20], 0.3)) && !lit(&second.frame(&[20], 0.3)));
        assert!(!lit(&first.frame(&[20], 0.8)) && lit(&second.frame(&[20], 0.8)));
        let first_reversed = Effect {
            reverse: true,
            ..first.clone()
        };
        assert!(
            lit(&first_reversed.frame(&[20], 0.3)) && !lit(&first_reversed.frame(&[20], 0.8)),
            "a reversed device keeps its place in the chain"
        );
    }

    #[test]
    fn devices_taking_turns_are_dark_outside_their_slot() {
        let breathing = effect(EffectKind::Breathing, &[[0, 255, 0]]);
        let first = Effect {
            turn: Some(Span { start: 0, len: 500 }),
            ..breathing.clone()
        };
        let second = Effect {
            turn: Some(Span {
                start: 500,
                len: 500,
            }),
            ..breathing.clone()
        };
        assert_eq!(
            first.period_ms(),
            (breathing.period_ms() as f32 / stretch(0.5)) as u32
        );
        assert_eq!(first.frame(&[1], 0.25), breathing.frame(&[1], 0.5));
        assert_eq!(second.frame(&[1], 0.25), vec![[0, 0, 0]]);
        assert_eq!(second.frame(&[1], 0.75), breathing.frame(&[1], 0.5));
        assert_eq!(first.frame(&[1], 0.75), vec![[0, 0, 0]]);
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
