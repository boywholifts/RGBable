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
pub const MAX_MODULES: usize = 40;

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
    CpuLoad,
    RamLoad,
    GpuPower,
    GpuClock,
    /// A picture (`file`), stretched over `w` × `h`.
    Image,
    /// A bar filled by another reading (`source`).
    Bar,
}

impl ModuleKind {
    pub fn uses_gpu(self) -> bool {
        matches!(
            self,
            Self::GpuTemp | Self::GpuLoad | Self::GpuPower | Self::GpuClock
        )
    }

    pub fn uses_cpu(self) -> bool {
        self == Self::CpuTemp
    }
}

/// Which point of a widget `x` and `y` place: its middle, or the top of its text at its left
/// edge, middle or right edge (how L-Connect lays text out).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    #[default]
    Center,
    TopLeft,
    TopCenter,
    TopRight,
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
    #[serde(default)]
    pub anchor: Anchor,
    /// Readouts show their unit (°C, RPM, %); off where the theme prints it separately.
    #[serde(default = "yes")]
    pub unit: bool,
    /// The clock's or date's layout, as L-Connect names them: "h:m:s", "Y-M-D".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
    /// An image widget's picture file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    /// An image's or bar's size, in thousandths of the frame's width and height.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub w: u16,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub h: u16,
    /// What fills a bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ModuleKind>,
    /// A bar's empty part.
    #[serde(default = "bar_back")]
    pub back: [u8; 3],
    /// A segmented bar's block and gap widths, in pixels of the frame.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub block: u16,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gap: u16,
}

fn is_zero(v: &u16) -> bool {
    *v == 0
}
fn bar_back() -> [u8; 3] {
    [40, 40, 48]
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
    pub fn new(kind: ModuleKind) -> Self {
        Self {
            kind,
            x: 500,
            y: 500,
            size: 100,
            color: [255, 255, 255],
            label: String::new(),
            text: String::new(),
            hour12: false,
            font: String::new(),
            opacity: full(),
            shadow: yes(),
            caption_scale: caption_scale(),
            anchor: Anchor::default(),
            unit: yes(),
            format: String::new(),
            file: String::new(),
            w: 0,
            h: 0,
            source: None,
            back: bar_back(),
            block: 0,
            gap: 0,
        }
    }

    pub fn cleaned(mut self) -> Self {
        self.x = self.x.min(1000);
        self.y = self.y.min(1000);
        self.size = self.size.clamp(20, 600);
        self.label = self.label.chars().take(40).collect();
        self.text = self.text.chars().take(60).collect();
        self.opacity = self.opacity.clamp(10, 100);
        self.caption_scale = self.caption_scale.clamp(15, 100);
        self.w = self.w.min(1000);
        self.h = self.h.min(1000);
        self.format = self.format.chars().take(20).collect();
        self.block = self.block.min(500);
        self.gap = self.gap.min(500);
        if self
            .source
            .is_some_and(|s| matches!(s, ModuleKind::Bar | ModuleKind::Image | ModuleKind::Text))
        {
            self.source = None;
        }
        self
    }

    /// The reading this widget shows.
    pub fn reading(&self) -> ModuleKind {
        match self.kind {
            ModuleKind::Bar => self.source.unwrap_or(ModuleKind::CpuLoad),
            kind => kind,
        }
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
    pub cpu_load: Option<u32>,
    pub ram_load: Option<u32>,
    pub gpu_power_w: Option<u32>,
    pub gpu_clock_mhz: Option<u32>,
}

impl Readings {
    /// A reading as a number and its unit.
    fn number(&self, kind: ModuleKind) -> Option<(String, &'static str)> {
        let n = |v: Option<u32>, unit| v.map(|v| (v.to_string(), unit));
        match kind {
            ModuleKind::Coolant => n(self.coolant_c.map(u32::from), "°C"),
            ModuleKind::CpuTemp => n(self.cpu_temp_c, "°C"),
            ModuleKind::Pump => n(self.pump_rpm.map(u32::from), " RPM"),
            ModuleKind::GpuTemp => n(self.gpu_temp_c, "°C"),
            ModuleKind::GpuLoad => n(self.gpu_load, "%"),
            ModuleKind::Fans => n(self.fans_rpm, " RPM"),
            ModuleKind::CpuLoad => n(self.cpu_load, "%"),
            ModuleKind::RamLoad => n(self.ram_load, "%"),
            ModuleKind::GpuPower => n(self.gpu_power_w, " W"),
            ModuleKind::GpuClock => self
                .gpu_clock_mhz
                .map(|mhz| (format!("{:.2}", f64::from(mhz) / 1000.0), " GHz")),
            _ => None,
        }
    }

    /// How full a bar showing `kind` is, 0 to 1.
    pub fn fraction(&self, kind: ModuleKind) -> Option<f32> {
        let of = |v: Option<u32>, full: f32| v.map(|v| (v as f32 / full).clamp(0.0, 1.0));
        match kind {
            ModuleKind::Coolant => of(self.coolant_c.map(u32::from), 60.0),
            ModuleKind::CpuTemp | ModuleKind::GpuTemp => of(
                if kind == ModuleKind::CpuTemp {
                    self.cpu_temp_c
                } else {
                    self.gpu_temp_c
                },
                100.0,
            ),
            ModuleKind::Pump => of(self.pump_rpm.map(u32::from), 3000.0),
            ModuleKind::Fans => of(self.fans_rpm, 2500.0),
            ModuleKind::GpuLoad => of(self.gpu_load, 100.0),
            ModuleKind::CpuLoad => of(self.cpu_load, 100.0),
            ModuleKind::RamLoad => of(self.ram_load, 100.0),
            ModuleKind::GpuPower => of(self.gpu_power_w, 600.0),
            ModuleKind::GpuClock => of(self.gpu_clock_mhz, 3000.0),
            _ => None,
        }
    }
}

pub fn value(
    module: &OledModule,
    readings: &Readings,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    match module.kind {
        ModuleKind::Clock => {
            let seconds = module.format.contains('s');
            let layout = match (module.hour12, seconds) {
                (true, true) => "%-I:%M:%S %p",
                (true, false) => "%-I:%M %p",
                (false, true) => "%H:%M:%S",
                (false, false) => "%H:%M",
            };
            now.format(layout).to_string()
        }
        ModuleKind::Date => {
            let layout = match module.format.as_str() {
                "Y-M-D" => "%Y-%m-%d",
                "M-D" => "%m-%d",
                "D/M/Y" => "%d/%m/%Y",
                "M/D/Y" => "%m/%d/%Y",
                _ => "%a %b %-d",
            };
            now.format(layout).to_string()
        }
        ModuleKind::Text => module.text.clone(),
        ModuleKind::Image | ModuleKind::Bar => String::new(),
        kind => match readings.number(kind) {
            Some((number, unit)) if module.unit => format!("{number}{unit}"),
            Some((number, _)) => number,
            None => "–".into(),
        },
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
        let opacity = f32::from(module.opacity.clamp(10, 100)) / 100.0;
        let x = f32::from(module.x) / 1000.0 * WIDTH as f32;
        let y = f32::from(module.y) / 1000.0 * HEIGHT as f32;
        let w = f32::from(module.w) / 1000.0 * WIDTH as f32;
        let h = f32::from(module.h) / 1000.0 * HEIGHT as f32;
        match module.kind {
            ModuleKind::Image => draw_image(&mut frame, module, x, y, w, h, opacity),
            ModuleKind::Bar => {
                let filled = readings.fraction(module.reading()).unwrap_or(0.0);
                draw_bar(&mut frame, module, x, y, w, h, filled, opacity);
            }
            _ => draw_readout(&mut frame, module, readings, now, x, y, opacity),
        }
    }
    frame
}

/// Where a widget's box starts, given its anchor point and size.
fn top_left(anchor: Anchor, x: f32, y: f32, w: f32, h: f32) -> (f32, f32) {
    match anchor {
        Anchor::Center => (x - w / 2.0, y - h / 2.0),
        Anchor::TopLeft => (x, y),
        Anchor::TopCenter => (x - w / 2.0, y),
        Anchor::TopRight => (x - w, y),
    }
}

fn draw_readout(
    frame: &mut [u8],
    module: &OledModule,
    readings: &Readings,
    now: chrono::DateTime<chrono::Local>,
    x: f32,
    y: f32,
    opacity: f32,
) {
    let Some(font) = font(&module.font) else {
        return;
    };
    let size = f32::from(module.size) / 1000.0 * HEIGHT as f32;
    let text = value(module, readings, now);
    let caption = module.label.trim();
    let caption_size = size * f32::from(module.caption_scale) / 100.0;
    let gap = size * 0.08;
    let total = if caption.is_empty() {
        size
    } else {
        size + gap + caption_size
    };
    let top = match module.anchor {
        Anchor::Center => y - total / 2.0,
        _ => y,
    };
    let shadow = (size / 22.0).max(2.0);
    let layers = |alpha: f32| {
        let mut layers = Vec::new();
        if module.shadow {
            layers.push((shadow, [0, 0, 0], 0.6 * alpha));
        }
        layers.push((0.0, module.color, alpha));
        layers
    };
    let left_of = |line: &str, size: f32| {
        let width = text_width(&font, line, size);
        top_left(module.anchor, x, 0.0, width, 0.0).0
    };
    if !caption.is_empty() {
        let left = left_of(caption, caption_size);
        for (offset, color, alpha) in layers(0.75 * opacity) {
            draw_text(
                frame,
                &font,
                caption,
                caption_size,
                left + offset,
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
    let left = left_of(&text, size);
    for (offset, color, alpha) in layers(opacity) {
        draw_text(
            frame,
            &font,
            &text,
            size,
            left + offset,
            value_top + offset,
            color,
            alpha,
        );
    }
}

fn text_width(font: &FontVec, text: &str, size: f32) -> f32 {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut width = 0.0;
    let mut previous = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(previous) = previous {
            width += scaled.kern(previous, id);
        }
        width += scaled.h_advance(id);
        previous = Some(id);
    }
    width
}

fn blend(frame: &mut [u8], px: i64, py: i64, color: [u8; 3], alpha: f32) {
    if px < 0 || py < 0 || px >= i64::from(WIDTH) || py >= i64::from(HEIGHT) {
        return;
    }
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let i = ((py as u32 * WIDTH + px as u32) * 4) as usize;
    let under = f32::from(frame[i + 3]) / 255.0;
    let out = a + under * (1.0 - a);
    for (channel, &c) in color.iter().enumerate() {
        let mixed = (f32::from(c) * a + f32::from(frame[i + channel]) * under * (1.0 - a)) / out;
        frame[i + channel] = mixed.round() as u8;
    }
    frame[i + 3] = (out * 255.0).round() as u8;
}

#[allow(clippy::too_many_arguments)]
fn draw_bar(
    frame: &mut [u8],
    module: &OledModule,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    filled: f32,
    opacity: f32,
) {
    let (left, top) = top_left(module.anchor, x, y, w, h);
    let (left, top, w, h) = (
        left.round() as i64,
        top.round() as i64,
        w.round() as i64,
        h.round() as i64,
    );
    let fill_to = (w as f32 * filled).round() as i64;
    let block = i64::from(module.block);
    let gap = i64::from(module.gap);
    for dx in 0..w.max(0) {
        let in_gap = block > 0 && dx % (block + gap) >= block;
        if in_gap {
            continue;
        }
        let lit = if block > 0 {
            // a segment lights once the fill reaches its middle
            let start = dx - dx % (block + gap);
            start + block / 2 < fill_to
        } else {
            dx < fill_to
        };
        let color = if lit { module.color } else { module.back };
        for dy in 0..h.max(0) {
            blend(frame, left + dx, top + dy, color, opacity);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_image(frame: &mut [u8], module: &OledModule, x: f32, y: f32, w: f32, h: f32, opacity: f32) {
    let (w, h) = (w.round().max(1.0) as u32, h.round().max(1.0) as u32);
    let Some(image) = image(&module.file, w, h) else {
        return;
    };
    let (left, top) = top_left(module.anchor, x, y, w as f32, h as f32);
    let (left, top) = (left.round() as i64, top.round() as i64);
    for (ix, iy, pixel) in image.enumerate_pixels() {
        let [r, g, b, a] = pixel.0;
        if a > 0 {
            blend(
                frame,
                left + i64::from(ix),
                top + i64::from(iy),
                [r, g, b],
                f32::from(a) / 255.0 * opacity,
            );
        }
    }
}

type Pictures = BTreeMap<(String, u32, u32), Option<Arc<image::RgbaImage>>>;

/// A picture scaled to `w` × `h`, loaded once per size.
fn image(file: &str, w: u32, h: u32) -> Option<Arc<image::RgbaImage>> {
    static LOADED: parking_lot::Mutex<Pictures> = parking_lot::Mutex::new(BTreeMap::new());
    let mut loaded = LOADED.lock();
    if loaded.len() > 64 {
        loaded.clear();
    }
    loaded
        .entry((file.to_string(), w, h))
        .or_insert_with(|| {
            let picture = image::open(file).ok()?.to_rgba8();
            let picture = if picture.dimensions() == (w, h) {
                picture
            } else {
                image::imageops::resize(&picture, w, h, image::imageops::FilterType::Triangle)
            };
            Some(Arc::new(picture))
        })
        .clone()
}

/// Draws `text` with its line box starting at `left`, `top`.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    frame: &mut [u8],
    font: &FontVec,
    text: &str,
    size: f32,
    left: f32,
    top: f32,
    color: [u8; 3],
    alpha: f32,
) {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut caret = 0.0f32;
    let mut previous = None;
    let baseline = top + scaled.ascent();
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(previous) = previous {
            caret += scaled.kern(previous, id);
        }
        let glyph = id
            .with_scale_and_position(PxScale::from(size), ab_glyph::point(left + caret, baseline));
        caret += scaled.h_advance(id);
        previous = Some(id);
        let Some(outlined) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outlined.px_bounds();
        outlined.draw(|gx, gy, coverage| {
            let px = bounds.min.x as i64 + i64::from(gx);
            let py = bounds.min.y as i64 + i64::from(gy);
            blend(frame, px, py, color, coverage * alpha);
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

#[derive(Debug, Clone, Copy, Default)]
pub struct GpuReading {
    pub temp_c: Option<u32>,
    pub load: Option<u32>,
    pub power_w: Option<u32>,
    pub clock_mhz: Option<u32>,
}

/// NVIDIA GPU readings, through the driver's NVML library.
pub struct Gpu {
    nvml: Option<nvml_wrapper::Nvml>,
    last: Option<(Instant, GpuReading)>,
}

impl Gpu {
    const EVERY: Duration = Duration::from_secs(2);

    pub fn new() -> Self {
        let nvml = nvml_wrapper::Nvml::init()
            .map_err(|e| tracing::info!("no NVIDIA GPU readings: {e}"))
            .ok();
        Self { nvml, last: None }
    }

    /// The first GPU's readings, at most every couple of seconds.
    pub fn read(&mut self) -> GpuReading {
        if let Some((at, reading)) = self.last {
            if at.elapsed() < Self::EVERY {
                return reading;
            }
        }
        use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
        let reading = self
            .nvml
            .as_ref()
            .and_then(|nvml| nvml.device_by_index(0).ok())
            .map(|gpu| GpuReading {
                temp_c: gpu.temperature(TemperatureSensor::Gpu).ok(),
                load: gpu.utilization_rates().ok().map(|u| u.gpu),
                power_w: gpu.power_usage().ok().map(|mw| mw / 1000),
                clock_mhz: gpu.clock_info(Clock::Graphics).ok(),
            })
            .unwrap_or_default();
        self.last = Some((Instant::now(), reading));
        reading
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(kind: ModuleKind) -> OledModule {
        OledModule {
            size: 200,
            text: "Hi".into(),
            ..OledModule::new(kind)
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
