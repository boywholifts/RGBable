//! Picks the main colors of a video or image, for lighting that matches the OLED.

use anyhow::{bail, Context, Result};
use std::path::Path;

const WIDTH: usize = 48;
const HEIGHT: usize = 27;
const FRAMES: usize = 16;

/// Up to `count` vivid colors from `file`, the most present first.
pub fn from_media(file: &Path, count: usize) -> Result<Vec<[u8; 3]>> {
    let pixels = sample(file)?;
    Ok(pick(&pixels, count.clamp(1, 8)))
}

/// Small frames from the video's keyframes (only those are decoded, so even long 4K
/// files are quick), or the image itself.
fn sample(file: &Path) -> Result<Vec<[u8; 3]>> {
    let mut ffmpeg = std::process::Command::new(crate::oled_library::ffmpeg());
    ffmpeg
        .args(["-v", "error", "-skip_frame", "nokey", "-i"])
        .arg(file)
        .args(["-vf", &format!("scale={WIDTH}:{HEIGHT}")])
        .args(["-fps_mode", "passthrough", "-frames:v", &FRAMES.to_string()])
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        ffmpeg.creation_flags(0x0800_0000);
    }
    let output = ffmpeg.output().context("running ffmpeg")?;
    if !output.status.success() || output.stdout.len() < 3 {
        bail!(
            "ffmpeg couldn't read {}: {}",
            file.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output
        .stdout
        .chunks_exact(3)
        .map(|c| [c[0], c[1], c[2]])
        .collect())
}

const HUE_BINS: usize = 36;
/// Pixels this saturated count as a color; below `GREY_SATURATION` as grey or white.
const COLOR_SATURATION: f32 = 0.35;
const GREY_SATURATION: f32 = 0.2;
/// What a grey or white area of the video becomes on LEDs.
const LED_GREY: [u8; 3] = [170, 170, 180];

fn hsv(c: [u8; 3]) -> (f32, f32, f32) {
    let [r, g, b] = c.map(|v| f32::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    let saturation = if max > 0.0 { delta / max } else { 0.0 };
    (hue, saturation, max)
}

fn from_hsv(hue: f32, saturation: f32) -> [u8; 3] {
    let h = hue.rem_euclid(1.0) * 6.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    [r, g, b].map(|v: f32| ((1.0 - saturation + saturation * v) * 255.0).round() as u8)
}

#[derive(Default, Clone, Copy)]
struct Bin {
    weight: f32,
    sin: f32,
    cos: f32,
    saturation: f32,
}

/// The main hues of the bright, colorful pixels, each at full brightness and saturation as
/// strong as in the video, plus grey when the video has large light grey areas.
/// Averaging pixels (k-means) mixed glows with their white highlights and gave pale,
/// washed-out colors, so this picks peaks of a hue histogram instead.
pub fn pick(pixels: &[[u8; 3]], count: usize) -> Vec<[u8; 3]> {
    let mut bins = [Bin::default(); HUE_BINS];
    let mut grey = 0.0f32;
    for &pixel in pixels {
        let (hue, saturation, value) = hsv(pixel);
        if value < 0.15 {
            continue;
        }
        if saturation >= COLOR_SATURATION {
            let weight = saturation * value;
            let bin = &mut bins[(hue * HUE_BINS as f32) as usize % HUE_BINS];
            let angle = hue * std::f32::consts::TAU;
            bin.weight += weight;
            bin.sin += angle.sin() * weight;
            bin.cos += angle.cos() * weight;
            bin.saturation += saturation * weight;
        } else if saturation < GREY_SATURATION && value > 0.35 {
            grey += value * 0.5;
        }
    }

    let mut picked: Vec<([u8; 3], f32)> = Vec::new();
    let mut taken = [false; HUE_BINS];
    while picked.len() < count {
        let best = (0..HUE_BINS)
            .filter(|&i| !taken[i])
            .max_by(|&a, &b| bins[a].weight.total_cmp(&bins[b].weight));
        let Some(best) = best.filter(|&i| bins[i].weight > 0.0) else {
            break;
        };
        // a peak and its neighbors are one color
        let mut merged = Bin::default();
        for offset in [HUE_BINS - 2, HUE_BINS - 1, 0, 1, 2] {
            let i = (best + offset) % HUE_BINS;
            if !taken[i] {
                taken[i] = true;
                merged.weight += bins[i].weight;
                merged.sin += bins[i].sin;
                merged.cos += bins[i].cos;
                merged.saturation += bins[i].saturation;
            }
        }
        let hue = merged.sin.atan2(merged.cos) / std::f32::consts::TAU;
        let saturation = (merged.saturation / merged.weight).max(0.75);
        picked.push((from_hsv(hue, saturation), merged.weight));
    }

    let strongest = picked.first().map_or(0.0, |p| p.1);
    if grey > strongest * 0.4 || picked.is_empty() {
        let at = picked
            .iter()
            .position(|p| p.1 < grey)
            .unwrap_or(picked.len());
        picked.insert(at, (LED_GREY, grey));
        picked.truncate(count.max(1));
    }
    picked.into_iter().map(|(c, _)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_red_glow_stays_red_however_many_highlights_and_dark_pixels() {
        let mut pixels = vec![[4, 4, 6]; 4000];
        pixels.extend(vec![[255, 40, 10]; 120]);
        pixels.extend(vec![[255, 120, 60]; 40]);
        pixels.extend(vec![[255, 235, 225]; 60]);
        let colors = pick(&pixels, 1);
        let (hue, saturation, value) = hsv(colors[0]);
        assert!(!(0.04..0.96).contains(&hue), "not red: {colors:?}");
        assert!(saturation > 0.8 && value > 0.99, "washed out: {colors:?}");
    }

    #[test]
    fn grey_areas_come_through_as_grey_next_to_the_main_color() {
        let mut pixels = vec![[0, 0, 0]; 2000];
        pixels.extend(vec![[230, 30, 10]; 200]);
        pixels.extend(vec![[150, 155, 165]; 900]);
        let colors = pick(&pixels, 3);
        assert!(colors.contains(&LED_GREY), "{colors:?}");
        assert_eq!(
            colors.len(),
            2,
            "only red and grey are in the video: {colors:?}"
        );
    }

    #[test]
    fn separate_hues_become_separate_colors() {
        let mut pixels = vec![[5, 5, 8]; 1000];
        pixels.extend(vec![[200, 0, 120]; 80]);
        pixels.extend(vec![[0, 150, 160]; 40]);
        let colors = pick(&pixels, 2);
        assert_eq!(colors.len(), 2);
        assert!(colors[0][0] == 255 && colors[0][1] == 0, "{colors:?}");
        assert!(colors[1][2] == 255 && colors[1][0] == 0, "{colors:?}");
    }
}
