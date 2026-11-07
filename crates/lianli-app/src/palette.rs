//! Picks the main colours of a video or image, for lighting that matches the OLED.

use anyhow::{bail, Context, Result};
use std::path::Path;

const WIDTH: usize = 48;
const HEIGHT: usize = 27;
const FRAMES: usize = 16;

/// Up to `count` vivid colours from `file`, the most present first.
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

fn value_and_saturation(c: [u8; 3]) -> (f32, f32) {
    let max = f32::from(*c.iter().max().unwrap_or(&0)) / 255.0;
    let min = f32::from(*c.iter().min().unwrap_or(&0)) / 255.0;
    (max, if max > 0.0 { (max - min) / max } else { 0.0 })
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
}

/// K-means over the pixels that would show on LEDs (near-black and grey ones are
/// dropped, since OLED videos are mostly dark). Clusters are ranked by size and
/// colourfulness, then brightened to full, because dim colours look muddy on LEDs.
pub fn pick(pixels: &[[u8; 3]], count: usize) -> Vec<[u8; 3]> {
    let mut points: Vec<[f32; 3]> = pixels
        .iter()
        .filter(|&&p| {
            let (v, s) = value_and_saturation(p);
            v > 0.18 && s > 0.25
        })
        .map(|p| p.map(f32::from))
        .collect();
    if points.len() < count {
        points = pixels.iter().map(|p| p.map(f32::from)).collect();
    }
    if points.is_empty() {
        return vec![[255, 255, 255]];
    }

    // farthest-point start, so small but distinct colours get a cluster
    let mut centres = vec![points[points.len() / 2]];
    while centres.len() < count.min(points.len()) {
        let far = points
            .iter()
            .copied()
            .max_by(|a, b| {
                let da = centres
                    .iter()
                    .map(|c| distance(*a, *c))
                    .fold(f32::MAX, f32::min);
                let db = centres
                    .iter()
                    .map(|c| distance(*b, *c))
                    .fold(f32::MAX, f32::min);
                da.total_cmp(&db)
            })
            .unwrap_or(points[0]);
        centres.push(far);
    }

    let mut sizes = vec![0usize; centres.len()];
    for _ in 0..12 {
        let mut sums = vec![[0f32; 3]; centres.len()];
        sizes = vec![0; centres.len()];
        for p in &points {
            let nearest = (0..centres.len())
                .min_by(|&a, &b| distance(*p, centres[a]).total_cmp(&distance(*p, centres[b])))
                .unwrap_or(0);
            sizes[nearest] += 1;
            for i in 0..3 {
                sums[nearest][i] += p[i];
            }
        }
        for (k, centre) in centres.iter_mut().enumerate() {
            if sizes[k] > 0 {
                *centre = sums[k].map(|v| v / sizes[k] as f32);
            }
        }
    }

    let mut ranked: Vec<([u8; 3], f32)> = centres
        .iter()
        .zip(&sizes)
        .filter(|(_, &n)| n > 0)
        .map(|(c, &n)| {
            let rgb = c.map(|v| v.round().clamp(0.0, 255.0) as u8);
            let (_, s) = value_and_saturation(rgb);
            (brighten(rgb), n as f32 * (0.3 + s))
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    ranked.into_iter().map(|(c, _)| c).collect()
}

fn brighten(c: [u8; 3]) -> [u8; 3] {
    let max = *c.iter().max().unwrap_or(&0);
    if max == 0 {
        return [255, 255, 255];
    }
    c.map(|v| (u16::from(v) * 255 / u16::from(max)) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_bright_colours_in_a_dark_frame() {
        let mut pixels = vec![[5, 5, 8]; 1000];
        pixels.extend(vec![[200, 0, 120]; 80]);
        pixels.extend(vec![[0, 150, 160]; 40]);
        let colors = pick(&pixels, 2);
        assert_eq!(colors.len(), 2);
        assert_eq!(colors[0], [255, 0, 153]);
        assert_eq!(colors[1], [0, 239, 255]);
    }
}
