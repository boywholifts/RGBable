//! Turns a device's lighting into the frames a wireless device stores and loops.
//!
//! Devices are drawn on their real layout: an SL V4 (SL Flex) fan has 13 positions on its
//! inner ring and 13 on its outer ring, packed into the first 26 of its 52 slots; a Strimer
//! repeats one 29- or 22-LED line along its rows. L-Connect effects go through the Lian Li
//! renderer from lianli-media, which knows each family's own patterns.

use crate::effects::{Effect, EffectKind, Side};
use anyhow::{Context, Result};
use lianli_media::rgb::{family, sync_layout::Layout};
use lianli_shared::rgb::{
    RgbEffect, RgbPlaybackTiming, RgbRegionConfig, RgbRenderFamily, RgbRenderProfile, RgbScope,
};

const SL_V4_POSITIONS: usize = 13;
const SL_V4_SLOTS_PER_FAN: usize = 26;

/// What a device shows on each of its sides; `None` means its plain color.
/// Devices without sides only use `inner`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lighting {
    pub inner: Option<Effect>,
    pub outer: Option<Effect>,
}

impl Lighting {
    pub fn whole(effect: Effect) -> Self {
        Self {
            inner: Some(effect.clone()),
            outer: Some(effect),
        }
    }

    pub fn is_static(&self) -> bool {
        self.inner.is_none() && self.outer.is_none()
    }

    /// Puts an effect on the side it names; a whole-device effect replaces both sides.
    pub fn apply(&mut self, effect: Effect, has_sides: bool) {
        match (has_sides, effect.side) {
            (true, Side::Inner) => self.inner = Some(effect),
            (true, Side::Outer) => self.outer = Some(effect),
            _ => *self = Self::whole(effect),
        }
    }

    pub fn from_saved(effect: Option<Effect>, outer: Option<Effect>) -> Self {
        Self {
            outer: outer.or_else(|| effect.clone().filter(|e| e.side == Side::Both)),
            inner: effect,
        }
    }

    /// The config form: `effect` is the whole-device or inner effect, `outer` only when it differs.
    pub fn to_saved(&self) -> (Option<Effect>, Option<Effect>) {
        let outer = (self.outer != self.inner
            || self.inner.as_ref().is_some_and(|e| e.side != Side::Both))
        .then(|| self.outer.clone())
        .flatten();
        (self.inner.clone(), outer)
    }

    /// The effect to show for the device as a whole (UI, devices without sides).
    pub fn primary(&self) -> Option<&Effect> {
        self.inner.as_ref().or(self.outer.as_ref())
    }
}

pub fn has_sides(profile: Option<RgbRenderProfile>) -> bool {
    profile.is_some_and(|p| p.family == RgbRenderFamily::SlV4)
}

pub struct Frames {
    pub frames: Vec<Vec<[u8; 3]>>,
    pub timing: RgbPlaybackTiming,
}

impl Frames {
    pub fn compressed_len(&self) -> Result<usize> {
        let raw: Vec<u8> = self.frames.iter().flatten().flatten().copied().collect();
        Ok(lianli_devices::tinyuz::compress(&raw)?.len())
    }

    /// Keeps every `step`-th frame and stretches the timing to match.
    fn decimated(self, step: usize) -> Self {
        if step <= 1 || self.frames.len() < 2 * step {
            return self;
        }
        let frames: Vec<_> = self.frames.into_iter().step_by(step).collect();
        let step = step as u32;
        let mut timing = self.timing;
        timing.interval_hundredths = timing
            .interval_hundredths
            .saturating_mul(step)
            .min(6_553_599);
        if timing.secondary_frame_count > 0 {
            timing.secondary_interval_ticks =
                timing.secondary_interval_ticks.saturating_mul(step as u16);
            timing.secondary_frame_count =
                timing.secondary_frame_count.div_ceil(step as u16).max(1);
            timing.secondary_frame_count = timing.secondary_frame_count.min(frames.len() as u16);
        }
        Self { frames, timing }
    }
}

/// A Lian Li effect rendered the way L-Connect draws it, or `None` when this device's
/// family doesn't have that effect (the caller then falls back to RGBable's version).
fn lianli_frames(
    lighting: &Lighting,
    base: &[[u8; 3]],
    profile: RgbRenderProfile,
) -> Option<Frames> {
    let native = |e: &Option<Effect>| e.as_ref().is_some_and(|e| e.kind == EffectKind::Lianli);
    if !native(&lighting.inner) && !native(&lighting.outer) {
        return None;
    }
    let to_region = |effect: Option<&Effect>, scope: RgbScope| -> Option<RgbRegionConfig> {
        let effect = match effect {
            Some(e) if e.kind == EffectKind::Lianli => e.clone(),
            Some(_) => return None,
            None => Effect {
                kind: EffectKind::Lianli,
                mode: Some(lianli_shared::rgb::RgbMode::Static),
                colors: vec![base.first().copied().unwrap_or([0, 0, 0])],
                ..Effect::default()
            },
        };
        let mode = effect.mode?;
        let parameters = lianli_media::rgb::parameters::for_scope(profile, scope)
            .into_iter()
            .find(|p| p.mode == mode)?;
        let wanted = if parameters.per_fan_colors {
            usize::from(profile.fan_count.max(1))
        } else {
            effect.colors.len().clamp(
                usize::from(parameters.min_colors),
                usize::from(parameters.max_colors),
            )
        };
        let source = if effect.colors.is_empty() {
            vec![[255, 255, 255]]
        } else {
            effect.colors.clone()
        };
        let colors: Vec<[u8; 3]> = (0..wanted)
            .map(|i| source[i.min(source.len() - 1)])
            .collect();
        let direction = effect
            .direction
            .filter(|d| parameters.directions.contains(d))
            .or_else(|| parameters.directions.first().copied())
            .unwrap_or_default();
        Some(RgbRegionConfig {
            effect: RgbEffect {
                mode,
                colors,
                speed: effect.speed.clamp(1, 5) - 1,
                brightness: ((u16::from(effect.brightness.min(100)) + 12) / 25) as u8,
                direction,
                scope,
                disabled: false,
            },
            flip: false,
        })
    };
    let whole = (!has_sides(Some(profile)) || lighting.inner == lighting.outer)
        .then(|| to_region(lighting.primary(), RgbScope::All))
        .flatten();
    let regions = match whole {
        Some(region) => vec![region],
        None if has_sides(Some(profile)) => vec![
            to_region(lighting.inner.as_ref(), RgbScope::Inner)?,
            to_region(lighting.outer.as_ref(), RgbScope::Outer)?,
        ],
        None => return None,
    };
    match family::render(profile, &regions) {
        Ok(animation) => Some(Frames {
            timing: animation.timing(),
            frames: animation.frames,
        }),
        Err(e) => {
            tracing::debug!("Lian Li renderer: {e:#}");
            None
        }
    }
}

/// How many times an effect repeats in a loop of `period_ms`, so both sides loop together.
fn repeats(effect: &Effect, period_ms: u32) -> f32 {
    (period_ms as f32 / effect.period_ms().max(1) as f32)
        .round()
        .max(1.0)
}

/// RGBable's own effects, drawn on the device's real layout.
fn generic_frames(
    lighting: &Lighting,
    base: &[[u8; 3]],
    zones: &[usize],
    profile: Option<RgbRenderProfile>,
    interval_ms: u32,
) -> Frames {
    let inner = lighting.inner.as_ref().map(Effect::generic);
    let outer = lighting.outer.as_ref().map(Effect::generic);
    let period = inner
        .iter()
        .chain(outer.iter())
        .filter(|e| e.is_animated())
        .map(Effect::period_ms)
        .max();
    let Some(period) = period else {
        let still = |e: &Option<Effect>| e.as_ref().map(|e| e.frame(&[1], 0.0)[0]);
        let frame = paint(base, zones, profile, |side, _, _| match side {
            Side::Outer => still(&outer),
            _ => still(&inner),
        });
        return Frames {
            frames: vec![frame],
            timing: RgbPlaybackTiming::from_millis(5000),
        };
    };
    let layout = logical_layout(zones, profile);
    let animated: Vec<&Effect> = inner
        .iter()
        .chain(outer.iter())
        .filter(|e| e.is_animated())
        .collect();
    let steps = animated[0].steps(&layout.zones);
    let stepped = steps.filter(|_| {
        animated
            .iter()
            .all(|e| e.steps(&layout.zones) == steps && e.period_ms() == period)
    });
    let (count, interval_ms) = match stepped {
        Some(steps) => {
            let step_ms = (period as f32 / steps as f32).max(1.0);
            let skip = (interval_ms as f32 / step_ms).round().max(1.0) as usize;
            let count = (steps / skip).max(2);
            (count, (period as f32 / count as f32).round() as u32)
        }
        None => ((period / interval_ms.max(1)).max(2) as usize, interval_ms),
    };
    let frames = (0..count)
        .map(|n| {
            let t = n as f32 / count as f32;
            let render = |e: &Option<Effect>| {
                e.as_ref()
                    .map(|e| e.frame(&layout.zones, (t * repeats(e, period)).fract()))
            };
            let (inner_frame, outer_frame) = (render(&inner), render(&outer));
            paint(base, zones, profile, |side, logical, _| match side {
                Side::Outer => outer_frame.as_ref().map(|f| f[logical]),
                _ => inner_frame.as_ref().map(|f| f[logical]),
            })
        })
        .collect();
    Frames {
        frames,
        timing: RgbPlaybackTiming::from_millis(interval_ms as u16),
    }
}

struct LogicalLayout {
    zones: Vec<usize>,
}

fn logical_layout(zones: &[usize], profile: Option<RgbRenderProfile>) -> LogicalLayout {
    match profile {
        Some(p) if p.family == RgbRenderFamily::SlV4 => LogicalLayout {
            zones: vec![SL_V4_POSITIONS; usize::from(p.fan_count)],
        },
        Some(p) => match Layout::for_profile(p) {
            Some(layout) => LogicalLayout {
                zones: vec![layout.logical_led_count()],
            },
            None => LogicalLayout {
                zones: zones.to_vec(),
            },
        },
        None => LogicalLayout {
            zones: zones.to_vec(),
        },
    }
}

/// Builds a physical frame: `color(side, logical_index, physical_index)` gives each slot's
/// color, or `None` to keep the device's plain color there.
fn paint(
    base: &[[u8; 3]],
    zones: &[usize],
    profile: Option<RgbRenderProfile>,
    color: impl Fn(Side, usize, usize) -> Option<[u8; 3]>,
) -> Vec<[u8; 3]> {
    let physical: usize = zones.iter().sum();
    let mut frame: Vec<[u8; 3]> = (0..physical)
        .map(|i| base.get(i).copied().unwrap_or([0, 0, 0]))
        .collect();
    match profile {
        Some(p) if p.family == RgbRenderFamily::SlV4 => {
            frame.iter_mut().for_each(|c| *c = [0, 0, 0]);
            for fan in 0..usize::from(p.fan_count) {
                for i in 0..SL_V4_POSITIONS {
                    let logical = fan * SL_V4_POSITIONS + i;
                    for (side, offset) in [(Side::Inner, 0), (Side::Outer, SL_V4_POSITIONS)] {
                        let slot = fan * SL_V4_SLOTS_PER_FAN + offset + i;
                        if let Some(target) = frame.get_mut(slot) {
                            *target = color(side, logical, slot)
                                .unwrap_or_else(|| base.get(slot).copied().unwrap_or([0, 0, 0]));
                        }
                    }
                }
            }
        }
        Some(p) => {
            if let Some(layout) = Layout::for_profile(p) {
                let logical_count = layout.logical_led_count();
                let index: Vec<usize> = (0..logical_count).collect();
                let mapped: Vec<[u8; 3]> = index
                    .iter()
                    .map(|&i| [i as u8, (i >> 8) as u8, 1])
                    .collect();
                if let Ok(projected) = layout.project_frame(&mapped, 0..logical_count, false) {
                    for (slot, marker) in projected.iter().enumerate() {
                        if marker[2] == 1 && slot < frame.len() {
                            let logical = usize::from(marker[0]) | (usize::from(marker[1]) << 8);
                            if let Some(c) = color(Side::Both, logical, slot) {
                                frame[slot] = c;
                            }
                        }
                    }
                    return frame;
                }
            }
            for (slot, target) in frame.iter_mut().enumerate() {
                if let Some(c) = color(Side::Both, slot, slot) {
                    *target = c;
                }
            }
        }
        None => {
            for (slot, target) in frame.iter_mut().enumerate() {
                if let Some(c) = color(Side::Both, slot, slot) {
                    *target = c;
                }
            }
        }
    }
    frame
}

/// The loop to upload at `shrink` level: L-Connect effects come from the Lian Li renderer and
/// are thinned out to fit; RGBable effects use longer frames until they fit `budget`.
/// Devices with L-Connect's own Drizzling (the Strimers) play it instead of the RGBable copy.
fn native_drizzle(lighting: &Lighting, profile: RgbRenderProfile) -> Lighting {
    let has_it = lianli_media::rgb::parameters::for_scope(profile, RgbScope::All)
        .iter()
        .any(|p| p.mode == lianli_shared::rgb::RgbMode::Drizzling);
    let swap = |e: &Option<Effect>| {
        e.clone().map(|e| match e.kind {
            EffectKind::Drizzle if has_it => Effect {
                kind: EffectKind::Lianli,
                mode: Some(lianli_shared::rgb::RgbMode::Drizzling),
                ..e
            },
            _ => e,
        })
    };
    Lighting {
        inner: swap(&lighting.inner),
        outer: swap(&lighting.outer),
    }
}

pub fn wireless_frames(
    lighting: &Lighting,
    base: &[[u8; 3]],
    zones: &[usize],
    profile: Option<RgbRenderProfile>,
    budget: usize,
    slowest_ms: u32,
    first_ms: u32,
) -> Result<Frames> {
    let drizzle = profile.map(|p| native_drizzle(lighting, p));
    if let Some(native) =
        profile.and_then(|p| lianli_frames(drizzle.as_ref().unwrap_or(lighting), base, p))
    {
        let mut step = 1;
        loop {
            let candidate = Frames {
                frames: native.frames.clone(),
                timing: native.timing,
            }
            .decimated(step);
            if candidate.compressed_len()? <= budget || candidate.frames.len() <= 2 {
                return Ok(candidate);
            }
            step *= 2;
        }
    }
    let mut interval = first_ms;
    loop {
        let frames = generic_frames(lighting, base, zones, profile, interval);
        if frames.frames.len() == 1 || frames.compressed_len()? <= budget || interval >= slowest_ms
        {
            if frames.frames.len() > 1 && frames.compressed_len()? > budget {
                return still(lighting, base, zones, profile).context("effect too large");
            }
            return Ok(frames);
        }
        interval *= 2;
    }
}

/// The first frame of the lighting, held still.
pub fn still(
    lighting: &Lighting,
    base: &[[u8; 3]],
    zones: &[usize],
    profile: Option<RgbRenderProfile>,
) -> Result<Frames> {
    let mut frames = generic_frames(lighting, base, zones, profile, 1000);
    frames.frames.truncate(1);
    frames.timing = RgbPlaybackTiming::from_millis(5000);
    Ok(frames)
}

/// Frames for the editor's preview: one SL Flex fan, inner ring then outer ring (26 LEDs).
pub type PreviewFrames = (Vec<Vec<[u8; 3]>>, u32);

pub fn preview(effect: Effect) -> Result<PreviewFrames, String> {
    if effect.kind == EffectKind::Script {
        crate::script::check(effect.script.as_deref().unwrap_or(""), &effect.colors)?;
    }
    let profile = RgbRenderProfile {
        family: RgbRenderFamily::SlV4,
        fan_count: 1,
        led_count: 52,
        right_attach: false,
    };
    let mut lighting = Lighting::default();
    lighting.apply(effect, true);
    let base = vec![[0, 0, 0]; 52];
    let frames = lianli_frames(&lighting, &base, profile)
        .unwrap_or_else(|| generic_frames(&lighting, &base, &[52], Some(profile), 40));
    let step = frames.frames.len().div_ceil(150).max(1);
    let frames = frames.decimated(step);
    let interval_ms = (frames.timing.interval_hundredths / 160).max(10);
    Ok((
        frames
            .frames
            .into_iter()
            .map(|f| f[..26].to_vec())
            .collect(),
        interval_ms,
    ))
}

/// An L-Connect effect the editor can offer, with what it accepts on SL Flex fans.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LianliEffect {
    pub mode: lianli_shared::rgb::RgbMode,
    pub whole: bool,
    pub side: bool,
    pub min_colors: u8,
    pub max_colors: u8,
    pub per_fan_colors: bool,
    pub directions: Vec<lianli_shared::rgb::RgbDirection>,
    pub speed: bool,
}

pub fn lianli_effects() -> Vec<LianliEffect> {
    use lianli_media::rgb::parameters::for_scope;
    let profile = RgbRenderProfile {
        family: RgbRenderFamily::SlV4,
        fan_count: 3,
        led_count: 156,
        right_attach: false,
    };
    let whole = for_scope(profile, RgbScope::All);
    let side = for_scope(profile, RgbScope::Inner);
    let mut list: Vec<LianliEffect> = Vec::new();
    for (parameters, is_whole) in whole
        .iter()
        .map(|p| (p, true))
        .chain(side.iter().map(|p| (p, false)))
    {
        if parameters.mode == lianli_shared::rgb::RgbMode::Off {
            continue;
        }
        if let Some(existing) = list.iter_mut().find(|e| e.mode == parameters.mode) {
            if is_whole {
                existing.whole = true;
            } else {
                existing.side = true;
            }
            continue;
        }
        list.push(LianliEffect {
            mode: parameters.mode,
            whole: is_whole,
            side: !is_whole,
            min_colors: parameters.min_colors,
            max_colors: parameters.max_colors,
            per_fan_colors: parameters.per_fan_colors,
            directions: parameters.directions.clone(),
            speed: parameters.supports_speed,
        });
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sl_v4(fans: u8) -> RgbRenderProfile {
        RgbRenderProfile {
            family: RgbRenderFamily::SlV4,
            fan_count: fans,
            led_count: u16::from(fans) * 52,
            right_attach: false,
        }
    }

    fn solid(color: [u8; 3]) -> Effect {
        Effect {
            colors: vec![color],
            ..Effect::default()
        }
    }

    #[test]
    fn sl_v4_sides_land_in_each_fans_first_26_slots() {
        let mut lighting = Lighting::default();
        lighting.apply(
            Effect {
                side: Side::Inner,
                ..solid([255, 0, 0])
            },
            true,
        );
        lighting.apply(
            Effect {
                side: Side::Outer,
                ..solid([0, 0, 255])
            },
            true,
        );
        let base = vec![[9, 9, 9]; 156];
        let frames = generic_frames(&lighting, &base, &[52, 52, 52], Some(sl_v4(3)), 40);
        let frame = &frames.frames[0];
        for fan in 0..3 {
            assert_eq!(frame[fan * 26], [255, 0, 0]);
            assert_eq!(frame[fan * 26 + 12], [255, 0, 0]);
            assert_eq!(frame[fan * 26 + 13], [0, 0, 255]);
            assert_eq!(frame[fan * 26 + 25], [0, 0, 255]);
        }
        assert!(frame[78..].iter().all(|c| *c == [0, 0, 0]));
    }

    #[test]
    fn one_side_effect_keeps_the_other_side_plain() {
        let mut lighting = Lighting::default();
        lighting.apply(
            Effect {
                side: Side::Outer,
                ..solid([0, 255, 0])
            },
            true,
        );
        let base = vec![[7, 7, 7]; 52];
        let frame = &generic_frames(&lighting, &base, &[52], Some(sl_v4(1)), 40).frames[0];
        assert_eq!(frame[0], [7, 7, 7]);
        assert_eq!(frame[13], [0, 255, 0]);
    }

    #[test]
    fn saved_sides_round_trip() {
        let mut lighting = Lighting::default();
        lighting.apply(solid([1, 1, 1]), true);
        let (effect, outer) = lighting.to_saved();
        assert!(outer.is_none());
        assert_eq!(Lighting::from_saved(effect, outer), lighting);
        lighting.apply(
            Effect {
                side: Side::Outer,
                ..solid([2, 2, 2])
            },
            true,
        );
        let (effect, outer) = lighting.to_saved();
        assert_eq!(Lighting::from_saved(effect, outer), lighting);
    }

    #[test]
    fn strimer_rows_repeat_the_same_line() {
        let profile = RgbRenderProfile {
            family: RgbRenderFamily::Strimer,
            fan_count: 0,
            led_count: 116,
            right_attach: false,
        };
        let lighting = Lighting::whole(Effect {
            kind: EffectKind::Wave,
            colors: vec![[255, 0, 0], [0, 0, 255]],
            ..Effect::default()
        });
        let frame =
            &generic_frames(&lighting, &vec![[0; 3]; 116], &[116], Some(profile), 40).frames[3];
        assert_eq!(frame[5], frame[29 + 5]);
        assert_eq!(frame[5], frame[87 + 5]);
    }

    #[test]
    fn drizzle_uses_the_strimers_own_drizzling_and_a_copy_on_fans() {
        let strimer = RgbRenderProfile {
            family: RgbRenderFamily::Strimer,
            fan_count: 0,
            led_count: 116,
            right_attach: false,
        };
        let drizzle = Lighting::whole(Effect {
            kind: EffectKind::Drizzle,
            colors: vec![[255, 0, 0], [0, 0, 255]],
            ..Effect::default()
        });
        let native = native_drizzle(&drizzle, strimer);
        assert_eq!(native.inner.as_ref().unwrap().kind, EffectKind::Lianli);
        assert_eq!(
            native.inner.unwrap().mode,
            Some(lianli_shared::rgb::RgbMode::Drizzling)
        );
        assert_eq!(native_drizzle(&drizzle, sl_v4(3)), drizzle);
        let frames = wireless_frames(
            &drizzle,
            &[[0; 3]; 116],
            &[116],
            Some(strimer),
            6 * 1024,
            640,
            40,
        );
        assert!(frames.unwrap().frames.len() > 1);
    }

    #[test]
    fn drizzle_on_sl_flex_fits_with_one_frame_per_led_step() {
        for colors in [1, 3, 6] {
            let drizzle = Lighting::whole(Effect {
                kind: EffectKind::Drizzle,
                colors: vec![
                    [255, 0, 0],
                    [0, 255, 0],
                    [0, 0, 255],
                    [255, 255, 0],
                    [0, 255, 255],
                    [255, 0, 255],
                ][..colors]
                    .to_vec(),
                ..Effect::default()
            });
            let frames = wireless_frames(
                &drizzle,
                &[[0; 3]; 156],
                &[52, 52, 52],
                Some(sl_v4(3)),
                6 * 1024,
                640,
                40,
            )
            .unwrap();
            println!(
                "{colors}: {} frames, {:?}, {} bytes",
                frames.frames.len(),
                frames.timing,
                frames.compressed_len().unwrap()
            );
            assert_eq!(
                frames.frames.len(),
                colors * 2 * SL_V4_POSITIONS,
                "{colors} colors"
            );
        }
    }

    #[test]
    fn every_lianli_effect_renders_and_fits_on_sl_flex() {
        for info in lianli_effects() {
            let effect = Effect {
                kind: EffectKind::Lianli,
                mode: Some(info.mode),
                colors: vec![[255, 0, 0], [0, 255, 0], [0, 0, 255], [255, 255, 0]],
                side: if info.whole { Side::Both } else { Side::Inner },
                ..Effect::default()
            };
            let mut lighting = Lighting::default();
            lighting.apply(effect.clone(), true);
            if !info.whole {
                lighting.apply(
                    Effect {
                        side: Side::Outer,
                        ..effect
                    },
                    true,
                );
            }
            let frames = lianli_frames(&lighting, &vec![[0; 3]; 156], sl_v4(3))
                .unwrap_or_else(|| panic!("{:?} did not render", info.mode));
            assert_eq!(frames.frames[0].len(), 156, "{:?}", info.mode);
            let fitted = wireless_frames(
                &lighting,
                &vec![[0; 3]; 156],
                &[52, 52, 52],
                Some(sl_v4(3)),
                6 * 1024,
                640,
                40,
            )
            .unwrap();
            assert!(
                fitted.compressed_len().unwrap() <= 6 * 1024,
                "{:?}",
                info.mode
            );
        }
    }

    #[test]
    fn custom_and_script_effects_preview_and_fit() {
        let layered = Effect {
            kind: EffectKind::Custom,
            name: Some("Neon".into()),
            layers: vec![
                crate::effects::Layer {
                    effect: Effect {
                        kind: EffectKind::RainbowWave,
                        ..Effect::default()
                    },
                    blend: crate::effects::Blend::Over,
                    opacity: 100,
                },
                crate::effects::Layer {
                    effect: Effect {
                        kind: EffectKind::Twinkle,
                        colors: vec![[255, 255, 255]],
                        ..Effect::default()
                    },
                    blend: crate::effects::Blend::Brightest,
                    opacity: 80,
                },
            ],
            ..Effect::default()
        };
        let script = Effect {
            kind: EffectKind::Script,
            script: Some(crate::script::TEMPLATE.into()),
            colors: vec![[255, 0, 128], [0, 200, 255]],
            ..Effect::default()
        };
        for effect in [layered, script] {
            let (frames, interval) = preview(effect.clone()).unwrap();
            assert!(frames.len() > 10 && frames[0].len() == 26 && interval >= 10);
            assert!(frames.iter().any(|f| f.iter().any(|c| *c != [0, 0, 0])));
            let fitted = wireless_frames(
                &Lighting::whole(effect),
                &vec![[0; 3]; 156],
                &[52, 52, 52],
                Some(sl_v4(3)),
                6 * 1024,
                640,
                40,
            )
            .unwrap();
            assert!(fitted.compressed_len().unwrap() <= 6 * 1024);
        }
        let broken = Effect {
            kind: EffectKind::Script,
            script: Some("rgb(1,".into()),
            ..Effect::default()
        };
        assert!(preview(broken).is_err());
    }

    #[test]
    fn lianli_effects_render_through_the_lian_li_renderer() {
        let lighting = Lighting::whole(Effect {
            kind: EffectKind::Lianli,
            mode: Some(lianli_shared::rgb::RgbMode::Meteor),
            colors: vec![[255, 0, 0]],
            ..Effect::default()
        });
        let frames = wireless_frames(
            &lighting,
            &vec![[0; 3]; 156],
            &[52, 52, 52],
            Some(sl_v4(3)),
            6 * 1024,
            640,
            40,
        )
        .unwrap();
        assert!(frames.frames.len() > 1);
        assert_eq!(frames.frames[0].len(), 156);
        assert!(frames.compressed_len().unwrap() <= 6 * 1024);
    }
}
