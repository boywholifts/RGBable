//! Turns L-Connect's HydroShift II screen themes (`.template` files) into RGBable widget
//! layouts: the theme's frames and pictures become one image widget, its live values and
//! labels become widgets, and a still background, when the theme has one, joins the library.
//! The files are read from the user's own L-Connect copy; nothing of them ships with RGBable.

use crate::nrbf::{self, Node};
use crate::oled_modules::{self, Anchor, ModuleKind, OledModule};
use anyhow::{ensure, Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Imported {
    pub name: String,
    pub widgets: Vec<OledModule>,
    pub background: Option<PathBuf>,
    /// The theme's video, when it was found next to the themes.
    pub video: Option<PathBuf>,
    /// L-Connect readings RGBable doesn't have yet.
    pub skipped: Vec<String>,
}

/// The `.template` files under `folder` (a few levels deep).
pub fn find_templates(folder: &Path) -> Vec<PathBuf> {
    let mut found = files_under(folder, |e| e.eq_ignore_ascii_case("template"));
    found.truncate(300);
    found
}

/// Videos under `folder`, by lowercase file stem, to find the ones themes play.
pub fn find_videos(folder: &Path) -> BTreeMap<String, PathBuf> {
    let video = |e: &str| {
        crate::oled_library::MEDIA
            .iter()
            .any(|m| e.eq_ignore_ascii_case(m))
    };
    files_under(folder, video)
        .into_iter()
        .take(2000)
        .filter_map(|path| Some((path.file_stem()?.to_string_lossy().to_lowercase(), path)))
        .collect()
}

fn files_under(folder: &Path, wanted: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u8, wanted: &dyn Fn(&str) -> bool, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth > 0 {
                    walk(&path, depth - 1, wanted, found);
                }
            } else if found.len() < 10_000
                && path
                    .extension()
                    .is_some_and(|e| wanted(&e.to_string_lossy()))
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(folder, 5, &wanted, &mut found);
    found.sort();
    found
}

/// The theme's video among `videos`: by the name L-Connect saved it under, or the theme's.
fn theme_video(root: Node, template: &Path, videos: &BTreeMap<String, PathBuf>) -> Option<PathBuf> {
    let saved = root
        .get("videoPath")
        .str()
        .and_then(|p| p.rsplit(['\\', '/']).next())
        .and_then(|name| Path::new(name).file_stem())
        .map(|stem| stem.to_string_lossy().to_lowercase());
    let own = template
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_lowercase());
    [saved, own]
        .into_iter()
        .flatten()
        .find_map(|stem| videos.get(&stem).cloned())
}

/// A .NET `System.Drawing.Color`: an ARGB value, or one of the named colors.
fn color(node: Node) -> Option<[u8; 3]> {
    if node.is_null() {
        return None;
    }
    let state = node.get("state").int().unwrap_or(0);
    if state & 2 != 0 {
        let argb = node.get("value").int()? as u32;
        return Some([(argb >> 16) as u8, (argb >> 8) as u8, argb as u8]);
    }
    Some(match node.get("knownColor").int()? {
        35 => [0, 0, 0],
        141 => [255, 0, 0],
        164 => [255, 255, 255],
        166 => [255, 255, 0],
        _ => [255, 255, 255],
    })
}

fn reading(name: &str) -> Option<ModuleKind> {
    let name = name.strip_suffix("_F").unwrap_or(name);
    Some(match name {
        "TIME" | "TIME_APM" | "APM" => ModuleKind::Clock,
        "DATE" => ModuleKind::Date,
        "CPUTEMP" => ModuleKind::CpuTemp,
        "GPUTEMP" => ModuleKind::GpuTemp,
        "CPULOAD" => ModuleKind::CpuLoad,
        "GPULOAD" => ModuleKind::GpuLoad,
        "RAMLOAD" => ModuleKind::RamLoad,
        "GPUPWR" => ModuleKind::GpuPower,
        "GPUCLOCK_G" => ModuleKind::GpuClock,
        "WATERTEMPC" | "WATERTEMP" => ModuleKind::Coolant,
        "CPUFAN" | "FANSPEED" => ModuleKind::Fans,
        "PUMPSPEED" | "PUMP" => ModuleKind::Pump,
        _ => return None,
    })
}

/// Font files on this PC by their names, to match the fonts themes ask for.
pub struct FontIndex(BTreeMap<String, PathBuf>);

fn font_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

impl FontIndex {
    pub fn scan() -> Self {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Ok(windows) = std::env::var("WINDIR") {
            dirs.push(Path::new(&windows).join("Fonts"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            dirs.push(Path::new(&local).join(r"Microsoft\Windows\Fonts"));
        }
        let mut files: Vec<PathBuf> = oled_modules::lconnect_fonts();
        for dir in dirs {
            if let Ok(entries) = std::fs::read_dir(dir) {
                files.extend(entries.flatten().map(|e| e.path()).filter(|p| {
                    p.extension().is_some_and(|e| {
                        e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf")
                    })
                }));
            }
        }
        let mut names = BTreeMap::new();
        for file in files.into_iter().take(2000) {
            let Ok(data) = std::fs::read(&file) else {
                continue;
            };
            let Ok(face) = ttf_parser::Face::parse(&data, 0) else {
                continue;
            };
            for record in face.names() {
                // family (1, 16) and full (4) names
                if matches!(record.name_id, 1 | 4 | 16) {
                    if let Some(name) = record.to_string() {
                        names.entry(font_key(&name)).or_insert_with(|| file.clone());
                    }
                }
            }
        }
        Self(names)
    }

    /// The widget font id for a family, preferring its bold face when asked.
    pub fn find(&self, family: &str, bold: bool) -> String {
        let family = font_key(family);
        let bold_name = format!("{family}bold");
        let hit = if bold {
            self.0.get(&bold_name).or_else(|| self.0.get(&family))
        } else {
            self.0.get(&family)
        };
        hit.map(|path| format!("file:{}", path.display()))
            .unwrap_or_default()
    }
}

struct Scale {
    x: f64,
    y: f64,
}

impl Scale {
    fn at(&self, x: f64, y: f64) -> (u16, u16) {
        let per_mille = |v: f64, full: f64| (v / full * 1000.0).round().clamp(0.0, 1000.0) as u16;
        (
            per_mille(x * self.x, f64::from(oled_modules::WIDTH)),
            per_mille(y * self.y, f64::from(oled_modules::HEIGHT)),
        )
    }
}

fn base_widget(kind: ModuleKind) -> OledModule {
    OledModule {
        shadow: false,
        anchor: Anchor::TopLeft,
        unit: false,
        ..OledModule::new(kind)
    }
}

fn text_widget(
    item: Node,
    origin: (f64, f64),
    scale: &Scale,
    fonts: &FontIndex,
    skipped: &mut Vec<String>,
) -> Option<OledModule> {
    let data = item.get("m_data");
    let name = data.get("DataName").str().unwrap_or("");
    let font = item.get("fontConfig");
    let mut widget = match item.get("TypeName").str()? {
        "Text" => {
            let text = data
                .get("_Value")
                .str()
                .or_else(|| data.get("ValueWithUnit").str())?;
            if text.trim().is_empty() {
                return None;
            }
            OledModule {
                text: text.to_string(),
                ..base_widget(ModuleKind::Text)
            }
        }
        "Data" => match reading(name) {
            Some(kind) => {
                let mut widget = base_widget(kind);
                widget.hour12 = name == "TIME_APM";
                widget.format = if name == "APM" {
                    oled_modules::AM_PM_ONLY.to_string()
                } else {
                    data.get("SubName").str().unwrap_or("").to_string()
                };
                widget
            }
            None => {
                skipped.push(name.to_string());
                return None;
            }
        },
        _ => return None,
    };
    let x = origin.0 + item.get("posX").float().unwrap_or(0.0);
    let y = origin.1 + item.get("posY").float().unwrap_or(0.0);
    (widget.x, widget.y) = scale.at(x, y);
    let em = font.get("size").float().unwrap_or(24.0) * scale.y;
    // .NET sizes the em; widget sizes are the whole line, about a fifth taller
    widget.size = (em * 1.2 / f64::from(oled_modules::HEIGHT) * 1000.0)
        .round()
        .clamp(20.0, 600.0) as u16;
    widget.color = color(font.get("color")).unwrap_or([255, 255, 255]);
    widget.anchor = match font.get("alignment").get("index").int() {
        Some(1) => Anchor::TopCenter,
        Some(2) => Anchor::TopRight,
        _ => Anchor::TopLeft,
    };
    if let Some(family) = font.get("name").str() {
        widget.font = fonts.find(family, font.get("isBold").bool().unwrap_or(false));
    }
    Some(widget)
}

fn bar_widget(
    item: Node,
    origin: (f64, f64),
    scale: &Scale,
    skipped: &mut Vec<String>,
) -> Option<OledModule> {
    let name = item.get("m_data").get("DataName").str().unwrap_or("");
    let Some(source) = reading(name) else {
        skipped.push(name.to_string());
        return None;
    };
    let mut widget = base_widget(ModuleKind::Bar);
    widget.source = Some(source);
    let x = origin.0 + item.get("posX").float().unwrap_or(0.0);
    let y = origin.1 + item.get("posY").float().unwrap_or(0.0);
    (widget.x, widget.y) = scale.at(x, y);
    let (w, h) = scale.at(
        item.get("width").float().unwrap_or(100.0),
        item.get("height").float().unwrap_or(20.0),
    );
    (widget.w, widget.h) = (w.max(1), h.max(1));
    widget.color = color(item.get("FrontColor")).unwrap_or([255, 255, 255]);
    let back = color(item.get("BackColor")).unwrap_or([40, 40, 48]);
    // L-Connect draws the empty part faintly
    widget.back = back.map(|c| (u16::from(c) * 30 / 100) as u8);
    if item.get("useSubsection").bool().unwrap_or(false) {
        widget.block =
            (item.get("SplitBlockWidth").float().unwrap_or(0.0) * scale.x).round() as u16;
        widget.gap = (item.get("SplitBlankWidth").float().unwrap_or(0.0) * scale.x).round() as u16;
    }
    Some(widget)
}

fn png(node: Node) -> Option<image::RgbaImage> {
    let bytes = node.get("Data").bytes()?;
    Some(image::load_from_memory(bytes).ok()?.to_rgba8())
}

fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    cleaned.trim().chars().take(60).collect()
}

/// Reads one theme and writes its pictures to `out`.
pub fn import(
    template: &Path,
    out: &Path,
    fonts: &FontIndex,
    videos: &BTreeMap<String, PathBuf>,
) -> Result<Imported> {
    let data =
        std::fs::read(template).with_context(|| format!("reading {}", template.display()))?;
    let graph = nrbf::parse(&data).with_context(|| format!("reading {}", template.display()))?;
    let root = graph.root();
    ensure!(root.class().ends_with("Theme"), "not an L-Connect theme");
    let width = root.get("width").float().unwrap_or(0.0);
    let height = root.get("height").float().unwrap_or(0.0);
    ensure!(
        width > height && width > 0.0,
        "made for another screen ({width} × {height})"
    );
    let scale = Scale {
        x: f64::from(oled_modules::WIDTH) / width,
        y: f64::from(oled_modules::HEIGHT) / height,
    };
    let name = root
        .get("name")
        .str()
        .map(str::to_string)
        .or_else(|| {
            template
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Theme".into());
    let file_name = safe_name(&name);
    std::fs::create_dir_all(out)?;

    let mut skipped = Vec::new();
    let mut widgets = Vec::new();
    let mut frames = image::RgbaImage::new(oled_modules::WIDTH, oled_modules::HEIGHT);
    let mut has_frames = false;
    let mut parts = root.get("ThemeList").items();
    parts.insert(0, root);
    for part in parts {
        let origin = (
            part.get("X").float().unwrap_or(0.0),
            part.get("Y").float().unwrap_or(0.0),
        );
        if part.get("IsHide").bool() == Some(true) {
            continue;
        }
        for item in part.get("GraphList").items() {
            // hidden items are L-Connect's own previews of a module, sample values and all
            if item.get("hide").bool() == Some(true) {
                continue;
            }
            if item.class().ends_with("GraphImage") {
                if let Some(picture) = png(item.get("bitmap")) {
                    let (w, h) = picture.dimensions();
                    let (w, h) = (
                        (f64::from(w) * scale.x).round() as u32,
                        (f64::from(h) * scale.y).round() as u32,
                    );
                    let picture = image::imageops::resize(
                        &picture,
                        w.max(1),
                        h.max(1),
                        image::imageops::FilterType::Triangle,
                    );
                    let (x, y) = (
                        (origin.0 * scale.x).round() as i64,
                        (origin.1 * scale.y).round() as i64,
                    );
                    image::imageops::overlay(&mut frames, &picture, x, y);
                    has_frames = true;
                }
            } else if item.class().ends_with("GraphStatuBar") {
                widgets.extend(bar_widget(item, origin, &scale, &mut skipped));
            } else {
                widgets.extend(text_widget(item, origin, &scale, fonts, &mut skipped));
            }
        }
    }
    if has_frames {
        let path = out.join(format!("{file_name} frames.png"));
        frames
            .save(&path)
            .with_context(|| format!("saving {}", path.display()))?;
        let mut image = base_widget(ModuleKind::Image);
        image.file = path.to_string_lossy().into_owned();
        (image.x, image.y, image.w, image.h) = (0, 0, 1000, 1000);
        widgets.insert(0, image);
    }
    widgets.truncate(oled_modules::MAX_MODULES);

    let video = theme_video(root, template, videos);
    // without its video, a still of it, when the theme has one
    let still = video
        .is_none()
        .then(|| png(root.get("VideoPic")).or_else(|| png(root.get("BgImg"))))
        .flatten();
    let background = match still {
        Some(picture) => {
            let backgrounds = out.join("Backgrounds");
            std::fs::create_dir_all(&backgrounds)?;
            let path = backgrounds.join(format!("{file_name}.png"));
            picture
                .save(&path)
                .with_context(|| format!("saving {}", path.display()))?;
            Some(path)
        }
        None => None,
    };
    skipped.sort();
    skipped.dedup();
    Ok(Imported {
        name,
        widgets,
        background,
        video,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_lconnect_readings() {
        assert_eq!(reading("CPUTEMP_F"), Some(ModuleKind::CpuTemp));
        assert_eq!(reading("TIME_APM"), Some(ModuleKind::Clock));
        assert_eq!(reading("APM"), Some(ModuleKind::Clock));
        assert_eq!(reading("UPSPEED"), None);
    }

    #[test]
    fn matches_font_names_loosely() {
        assert_eq!(font_key("Agency Gothic CT Bold"), "agencygothicctbold");
        let index = FontIndex(BTreeMap::from([(
            "hyperspacerace".to_string(),
            PathBuf::from("h.ttf"),
        )]));
        assert_eq!(index.find("Hyperspace Race", true), "file:h.ttf");
        assert_eq!(index.find("Missing", false), "");
    }
}
