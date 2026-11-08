//! Widgets (clock, temperatures, speeds, text) drawn over the HydroShift screen's video.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, VariableFont};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The video as RGBable lays it out, before it is turned upright for the panel.
pub const WIDTH: u32 = 2288;
pub const HEIGHT: u32 = 1080;
pub const MAX_MODULES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleKind {
    Clock,
    Date,
    Coolant,
    CpuTemp,
    Pump,
    GpuTemp,
    GpuLoad,
    Fans,
    Text,
}

impl ModuleKind {
    pub fn uses_gpu(self) -> bool {
        matches!(self, Self::GpuTemp | Self::GpuLoad)
    }

    pub fn uses_cpu(self) -> bool {
        self == Self::CpuTemp
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OledModule {
    pub kind: ModuleKind,
    /// Center of the readout, in thousandths of the frame's width and height.
    pub x: u16,
    pub y: u16,
    /// Height of the value's text, in thousandths of the frame height.
    pub size: u16,
    pub color: [u8; 3],
    /// A small caption above the value; empty for none.
    #[serde(default)]
    pub label: String,
    /// What a text module shows.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub hour12: bool,
    /// One of `FONTS`' ids, `file:` and a font file's path, or empty for the default.
    #[serde(default)]
    pub font: String,
    /// Percent.
    #[serde(default = "full")]
    pub opacity: u8,
    #[serde(default = "yes")]
    pub shadow: bool,
    /// The caption's height as a percentage of the value's.
    #[serde(default = "caption_scale")]
    pub caption_scale: u8,
}

fn full() -> u8 {
    100
}
fn yes() -> bool {
    true
}
fn caption_scale() -> u8 {
    38
}

impl OledModule {
    pub fn cleaned(mut self) -> Self {
        self.x = self.x.min(1000);
        self.y = self.y.min(1000);
        self.size = self.size.clamp(20, 600);
        self.label = self.label.chars().take(40).collect();
        self.text = self.text.chars().take(60).collect();
        self.opacity = self.opacity.clamp(10, 100);
        self.caption_scale = self.caption_scale.clamp(15, 100);
        self
    }
}

/// Fonts that come with RGBable, all under the SIL Open Font License (see ui/fonts), with the
/// weight variable ones are drawn at.
pub const FONTS: &[(&str, &str, &[u8], Option<f32>)] = &[
    (
        "orbitron",
        "Orbitron",
        include_bytes!("../ui/fonts/Orbitron.ttf"),
        Some(700.0),
    ),
    (
        "oxanium",
        "Oxanium",
        include_bytes!("../ui/fonts/Oxanium.ttf"),
        Some(700.0),
    ),
    (
        "rajdhani",
        "Rajdhani",
        include_bytes!("../ui/fonts/Rajdhani-Bold.ttf"),
        None,
    ),
    (
        "chakra",
        "Chakra Petch",
        include_bytes!("../ui/fonts/ChakraPetch-Bold.ttf"),
        None,
    ),
    (
        "russo",
        "Russo One",
        include_bytes!("../ui/fonts/RussoOne-Regular.ttf"),
        None,
    ),
    (
        "audiowide",
        "Audiowide",
        include_bytes!("../ui/fonts/Audiowide-Regular.ttf"),
        None,
    ),
    (
        "bebas",
        "Bebas Neue",
        include_bytes!("../ui/fonts/BebasNeue-Regular.ttf"),
        None,
    ),
    (
        "techmono",
        "Share Tech Mono",
        include_bytes!("../ui/fonts/ShareTechMono-Regular.ttf"),
        None,
    ),
    (
        "monoton",
        "Monoton",
        include_bytes!("../ui/fonts/Monoton-Regular.ttf"),
        None,
    ),
    (
        "pixel",
        "Press Start 2P",
        include_bytes!("../ui/fonts/PressStart2P-Regular.ttf"),
        None,
    ),
];

/// Font files that came with L-Connect, so its widgets' looks can be reused. They stay where
/// L-Connect put them and are only read.
pub fn lconnect_fonts() -> Vec<PathBuf> {
    let program_files = [
        std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into()),
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into()),
    ];
    let mut found = Vec::new();
    for base in program_files {
        for dir in [r"LIAN LI\L-Connect 3", "L-Connect 3", r"LIAN LI\L-Connect"] {
            collect_fonts(&Path::new(&base).join(dir), 6, &mut found);
        }
    }
    found.sort();
    found.dedup();
    found.truncate(200);
    found
}

fn collect_fonts(dir: &Path, depth: u8, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_fonts(&path, depth - 1, found);
            }
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
        {
            found.push(path);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Readings {
    pub coolant_c: Option<u8>,
    pub cpu_temp_c: Option<u32>,
    pub pump_rpm: Option<u16>,
    pub gpu_temp_c: Option<u32>,
    pub gpu_load: Option<u32>,
    pub fans_rpm: Option<u32>,
}

pub fn value(
    module: &OledModule,
    readings: &Readings,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let or_dash = |v: Option<String>| v.unwrap_or_else(|| "–".into());
    match module.kind {
        ModuleKind::Clock if module.hour12 => now.format("%-I:%M %p").to_string(),
        ModuleKind::Clock => now.format("%H:%M").to_string(),
        ModuleKind::Date => now.format("%a %b %-d").to_string(),
        ModuleKind::Coolant => or_dash(readings.coolant_c.map(|c| format!("{c}°C"))),
        ModuleKind::CpuTemp => or_dash(readings.cpu_temp_c.map(|c| format!("{c}°C"))),
        ModuleKind::Pump => or_dash(readings.pump_rpm.map(|r| format!("{r} RPM"))),
        ModuleKind::GpuTemp => or_dash(readings.gpu_temp_c.map(|c| format!("{c}°C"))),
        ModuleKind::GpuLoad => or_dash(readings.gpu_load.map(|p| format!("{p}%"))),
        ModuleKind::Fans => or_dash(readings.fans_rpm.map(|r| format!("{r} RPM"))),
        ModuleKind::Text => module.text.clone(),
    }
}

fn default_font() -> Option<Arc<FontVec>> {
    static FONT: OnceLock<Option<Arc<FontVec>>> = OnceLock::new();
    FONT.get_or_init(|| {
        let windows = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let candidates = [
            format!(r"{windows}\Fonts\bahnschrift.ttf"),
            format!(r"{windows}\Fonts\segoeuib.ttf"),
            format!(r"{windows}\Fonts\arialbd.ttf"),
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf".into(),
        ];
        candidates
            .iter()
            .find_map(|path| FontVec::try_from_vec(std::fs::read(path).ok()?).ok())
            .or_else(|| bundled("rajdhani"))
            .map(Arc::new)
    })
    .clone()
}

fn bundled(id: &str) -> Option<FontVec> {
    let (_, _, bytes, weight) = FONTS.iter().find(|f| f.0 == id)?;
    let mut font = FontVec::try_from_vec(bytes.to_vec()).ok()?;
    if let Some(weight) = weight {
        font.set_variation(b"wght", *weight);
    }
    Some(font)
}

/// The font a widget asked for, loaded once; the default if it can't be had.
fn font(id: &str) -> Option<Arc<FontVec>> {
    static LOADED: parking_lot::Mutex<BTreeMap<String, Option<Arc<FontVec>>>> =
        parking_lot::Mutex::new(BTreeMap::new());
    if id.is_empty() {
        return default_font();
    }
    let loaded = LOADED
        .lock()
        .entry(id.to_string())
        .or_insert_with(|| {
            let font = match id.strip_prefix("file:") {
                Some(path) => std::fs::read(path)
                    .ok()
                    .and_then(|b| FontVec::try_from_vec(b).ok()),
                None => bundled(id),
            };
            font.map(Arc::new)
        })
        .clone();
    loaded.or_else(default_font)
}

/// Every module drawn onto a transparent landscape frame, as straight RGBA.
pub fn render(
    modules: &[OledModule],
    readings: &Readings,
    now: chrono::DateTime<chrono::Local>,
) -> Vec<u8> {
    let mut frame = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    for module in modules {
        let Some(font) = font(&module.font) else {
            continue;
        };
        let size = f32::from(module.size) / 1000.0 * HEIGHT as f32;
        let x = f32::from(module.x) / 1000.0 * WIDTH as f32;
        let y = f32::from(module.y) / 1000.0 * HEIGHT as f32;
        let opacity = f32::from(module.opacity.clamp(10, 100)) / 100.0;
        let text = value(module, readings, now);
        let caption = module.label.trim();
        let caption_size = size * f32::from(module.caption_scale) / 100.0;
        let gap = size * 0.08;
        let total = if caption.is_empty() {
            size
        } else {
            size + gap + caption_size
        };
        let top = y - total / 2.0;
        let shadow = (size / 22.0).max(2.0);
        let layers = |alpha: f32| {
            let mut layers = Vec::new();
            if module.shadow {
                layers.push((shadow, [0, 0, 0], 0.6 * alpha));
            }
            layers.push((0.0, module.color, alpha));
            layers
        };
        if !caption.is_empty() {
            for (offset, color, alpha) in layers(0.75 * opacity) {
                draw_text(
                    &mut frame,
                    &font,
                    caption,
                    caption_size,
                    x + offset,
                    top + offset,
                    color,
                    alpha,
                );
            }
        }
        let value_top = if caption.is_empty() {
            top
        } else {
            top + caption_size + gap
        };
        for (offset, color, alpha) in layers(opacity) {
            draw_text(
                &mut frame,
                &font,
                &text,
                size,
                x + offset,
                value_top + offset,
                color,
                alpha,
            );
        }
    }
    frame
}

/// Draws `text` centered on `center_x`, its line box starting at `top`.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    frame: &mut [u8],
    font: &FontVec,
    text: &str,
    size: f32,
    center_x: f32,
    top: f32,
    color: [u8; 3],
    alpha: f32,
) {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut glyphs = Vec::new();
    let mut caret = 0.0f32;
    let mut previous = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(previous) = previous {
            caret += scaled.kern(previous, id);
        }
        glyphs.push(id.with_scale_and_position(PxScale::from(size), ab_glyph::point(caret, 0.0)));
        caret += scaled.h_advance(id);
        previous = Some(id);
    }
    let left = center_x - caret / 2.0;
    let baseline = top + scaled.ascent();
    for glyph in glyphs {
        let glyph = ab_glyph::Glyph {
            position: ab_glyph::point(glyph.position.x + left, baseline),
            ..glyph
        };
        let Some(outlined) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outlined.px_bounds();
        outlined.draw(|gx, gy, coverage| {
            let px = bounds.min.x as i64 + i64::from(gx);
            let py = bounds.min.y as i64 + i64::from(gy);
            if px < 0 || py < 0 || px >= i64::from(WIDTH) || py >= i64::from(HEIGHT) {
                return;
            }
            let a = (coverage * alpha).clamp(0.0, 1.0);
            if a <= 0.0 {
                return;
            }
            let i = ((py as u32 * WIDTH + px as u32) * 4) as usize;
            let under = f32::from(frame[i + 3]) / 255.0;
            let out = a + under * (1.0 - a);
            for (channel, &c) in color.iter().enumerate() {
                let mixed =
                    (f32::from(c) * a + f32::from(frame[i + channel]) * under * (1.0 - a)) / out;
                frame[i + channel] = mixed.round() as u8;
            }
            frame[i + 3] = (out * 255.0).round() as u8;
        });
    }
}

/// Turns a landscape frame upright for the panel, as ffmpeg's `transpose=1` turns the video.
pub fn rotate_clockwise(frame: &[u8]) -> Vec<u8> {
    let (w, h) = (WIDTH as usize, HEIGHT as usize);
    let mut out = vec![0u8; frame.len()];
    for oy in 0..w {
        for ox in 0..h {
            let (ix, iy) = (oy, h - 1 - ox);
            let from = (iy * w + ix) * 4;
            let to = (oy * h + ox) * 4;
            out[to..to + 4].copy_from_slice(&frame[from..from + 4]);
        }
    }
    out
}

/// NVIDIA GPU temperature and load, read through the driver's NVML library.
pub struct Gpu {
    nvml: Option<nvml_wrapper::Nvml>,
    last: Option<(Instant, Option<u32>, Option<u32>)>,
}

impl Gpu {
    const EVERY: Duration = Duration::from_secs(2);

    pub fn new() -> Self {
        let nvml = nvml_wrapper::Nvml::init()
            .map_err(|e| tracing::info!("no NVIDIA GPU readings: {e}"))
            .ok();
        Self { nvml, last: None }
    }

    /// Temperature in °C and load in percent of the first GPU.
    pub fn read(&mut self) -> (Option<u32>, Option<u32>) {
        if let Some((at, temp, load)) = self.last {
            if at.elapsed() < Self::EVERY {
                return (temp, load);
            }
        }
        let reading = self
            .nvml
            .as_ref()
            .and_then(|nvml| nvml.device_by_index(0).ok())
            .map(|gpu| {
                let temp = gpu
                    .temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu)
                    .ok();
                let load = gpu.utilization_rates().ok().map(|u| u.gpu);
                (temp, load)
            })
            .unwrap_or((None, None));
        self.last = Some((Instant::now(), reading.0, reading.1));
        reading
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(kind: ModuleKind) -> OledModule {
        OledModule {
            kind,
            x: 500,
            y: 500,
            size: 200,
            color: [255, 255, 255],
            label: String::new(),
            text: "Hi".into(),
            hour12: false,
            font: String::new(),
            opacity: 100,
            shadow: true,
            caption_scale: 38,
        }
    }

    #[test]
    fn readouts_show_their_values_or_a_dash() {
        let now = chrono::Local::now();
        let readings = Readings {
            coolant_c: Some(31),
            gpu_load: Some(7),
            ..Readings::default()
        };
        assert_eq!(value(&module(ModuleKind::Coolant), &readings, now), "31°C");
        assert_eq!(value(&module(ModuleKind::GpuLoad), &readings, now), "7%");
        assert_eq!(value(&module(ModuleKind::Pump), &readings, now), "–");
        assert_eq!(value(&module(ModuleKind::Text), &readings, now), "Hi");
        assert_eq!(value(&module(ModuleKind::Clock), &readings, now).len(), 5);
    }

    #[test]
    fn every_bundled_font_loads() {
        for (id, ..) in FONTS {
            assert!(bundled(id).is_some(), "{id}");
        }
    }

    #[test]
    fn rotating_matches_ffmpegs_clockwise_transpose() {
        let mut frame = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
        frame[..4].copy_from_slice(&[1, 2, 3, 4]);
        let upright = rotate_clockwise(&frame);
        // the top-left corner of the landscape frame ends up top-right
        let top_right = ((HEIGHT - 1) * 4) as usize;
        assert_eq!(&upright[top_right..top_right + 4], &[1, 2, 3, 4]);
    }
}
