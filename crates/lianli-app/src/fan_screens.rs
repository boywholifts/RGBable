//! The round 400x400 screens of Lian Li LCD fans, such as the SL-INF V3 Wireless LCD. The
//! fans' lighting and speed go over the wireless receiver, but each screen is its own USB
//! device (the "Flex LCD" endpoint) that plays H.264 like the HydroShift OLED.

use crate::engine::{no_window, oled_cache_dir, ENCODING};
use anyhow::{bail, Context, Result};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{info, warn};

const VID: u16 = 0x1CBE;
/// TL Flex LCD and SL Infinity Flex LCD.
const PIDS: [u16; 2] = [0xA018, 0xA019];
const SIZE: u32 = 400;
const FPS: u32 = 30;
const RESCAN_EVERY: Duration = Duration::from_secs(15);
const RETRY_AFTER: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FanScreenConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// "cover" crops to fill the circle, "contain" fits the whole picture.
    #[serde(default = "default_fit")]
    pub fit: String,
    /// Clockwise turn in degrees (0 to 359), for how the fan is mounted.
    #[serde(default)]
    pub rotation: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

fn default_fit() -> String {
    "cover".into()
}

impl FanScreenConfig {
    pub fn cleaned(mut self) -> Self {
        self.rotation %= 360;
        if self.fit != "contain" {
            self.fit = default_fit();
        }
        self
    }
}

#[derive(Debug, Clone)]
pub struct Screen {
    pub id: String,
    pub pid: u16,
    pub model: &'static str,
    serial: Option<String>,
    bus: u8,
    address: u8,
}

#[derive(Serialize)]
pub struct ScreenView {
    pub id: String,
    pub model: &'static str,
    pub name: String,
    pub file: Option<String>,
    pub fit: String,
    pub rotation: u16,
    pub state: String,
}

struct Worker {
    stop: Arc<AtomicBool>,
    playing: FanScreenConfig,
    thread: Option<thread::JoinHandle<()>>,
}

#[derive(Default)]
pub struct FanScreens {
    found: RwLock<Vec<Screen>>,
    workers: Mutex<HashMap<String, Worker>>,
    states: Arc<RwLock<HashMap<String, String>>>,
}

impl FanScreens {
    pub fn count(&self) -> usize {
        self.found.read().len()
    }

    pub fn views(&self, config: &BTreeMap<String, FanScreenConfig>) -> Vec<ScreenView> {
        let states = self.states.read();
        self.found
            .read()
            .iter()
            .enumerate()
            .map(|(i, screen)| {
                let saved = config.get(&screen.id).cloned().unwrap_or_default();
                ScreenView {
                    id: screen.id.clone(),
                    model: screen.model,
                    name: saved
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Fan screen {}", i + 1)),
                    file: saved
                        .file
                        .as_ref()
                        .map(|f| f.to_string_lossy().into_owned()),
                    fit: saved.fit.clone(),
                    rotation: saved.rotation,
                    state: states
                        .get(&screen.id)
                        .cloned()
                        .unwrap_or_else(|| "Off".into()),
                }
            })
            .collect()
    }

    /// Looks for screens every few seconds and keeps each one playing what it's set to.
    pub fn watch(&self, config: impl Fn() -> BTreeMap<String, FanScreenConfig>) {
        loop {
            match discover() {
                Ok(screens) => {
                    let before: Vec<String> =
                        self.found.read().iter().map(|s| s.id.clone()).collect();
                    let now: Vec<String> = screens.iter().map(|s| s.id.clone()).collect();
                    if before != now {
                        info!("{} fan screen(s) connected", screens.len());
                    }
                    *self.found.write() = screens;
                }
                Err(e) => warn!("looking for fan screens: {e:#}"),
            }
            self.sync(&config());
            thread::sleep(RESCAN_EVERY);
        }
    }

    /// Starts, restarts or stops each screen's stream to match the settings.
    pub fn sync(&self, config: &BTreeMap<String, FanScreenConfig>) {
        let found = self.found.read().clone();
        let mut workers = self.workers.lock();
        let mut stopped = HashMap::new();
        workers.retain(|id, worker| {
            let wanted = found.iter().any(|s| &s.id == id)
                && config
                    .get(id)
                    .is_some_and(|c| *c == worker.playing && c.file.is_some());
            if !wanted {
                worker.stop.store(true, Ordering::Release);
                self.states.write().insert(id.clone(), "Off".into());
                if let Some(thread) = worker.thread.take() {
                    stopped.insert(id.clone(), thread);
                }
            }
            wanted
        });
        for screen in found {
            let Some(saved) = config.get(&screen.id).filter(|c| c.file.is_some()) else {
                continue;
            };
            if workers.contains_key(&screen.id) {
                continue;
            }
            let stop = Arc::new(AtomicBool::new(false));
            let playing = saved.clone();
            let states = self.states.clone();
            // a screen takes one stream at a time: the new one waits for the old to let go
            let before = stopped.remove(&screen.id);
            let id = screen.id.clone();
            let thread = {
                let stop = stop.clone();
                thread::Builder::new()
                    .name("fan-screen".into())
                    .spawn(move || {
                        if let Some(before) = before {
                            let _ = before.join();
                        }
                        if !stop.load(Ordering::Acquire) {
                            play(&screen, &playing, &stop, &states);
                        }
                    })
                    .ok()
            };
            workers.insert(
                id,
                Worker {
                    stop,
                    playing: saved.clone(),
                    thread,
                },
            );
        }
    }
}

fn discover() -> Result<Vec<Screen>> {
    let mut screens: Vec<Screen> = lianli_devices::detect::enumerate_devices()?
        .into_iter()
        .filter(|d| d.vid == VID && PIDS.contains(&d.pid))
        .map(|d| Screen {
            id: format!(
                "fan-screen:{}",
                d.serial
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| format!("{:04x}-{}-{}", d.pid, d.bus, d.address))
            ),
            pid: d.pid,
            model: if d.pid == 0xA018 {
                "TL Wireless LCD"
            } else {
                "SL-INF Wireless LCD"
            },
            serial: d.serial,
            bus: d.bus,
            address: d.address,
        })
        .collect();
    screens.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(screens)
}

fn open(screen: &Screen) -> Result<lianli_devices::winusb::lcd::WinUsbLcdDevice> {
    let device = lianli_devices::detect::enumerate_devices()?
        .into_iter()
        .find(|d| {
            d.vid == VID
                && d.pid == screen.pid
                && match &screen.serial {
                    Some(serial) => d.serial.as_ref() == Some(serial),
                    None => d.bus == screen.bus && d.address == screen.address,
                }
        })
        .context("the fan screen is no longer connected")?;
    lianli_devices::winusb::lcd::WinUsbLcdDevice::open(device.device, screen.pid)
}

fn play(
    screen: &Screen,
    saved: &FanScreenConfig,
    stop: &AtomicBool,
    states: &RwLock<HashMap<String, String>>,
) {
    let set_state = |s: &str| {
        states.write().insert(screen.id.clone(), s.to_string());
    };
    let Some(file) = saved.file.as_deref() else {
        return;
    };
    set_state("Encoding");
    let encoded = match encode(file, &saved.fit, saved.rotation) {
        Ok(path) => path,
        Err(e) => {
            warn!("fan screen media: {e:#}");
            set_state(&format!("Error: {e:#}"));
            return;
        }
    };
    while !stop.load(Ordering::Acquire) {
        set_state("Playing");
        info!("Playing {} on {}", file.display(), screen.id);
        let result =
            open(screen).and_then(|mut lcd| lcd.stream_h264(&encoded, true, stop, FPS as f32));
        if stop.load(Ordering::Acquire) {
            break;
        }
        if let Err(e) = result {
            warn!("{} stream stopped: {e:#}; retrying in 5 s", screen.id);
            set_state("Reconnecting");
        }
        let until = std::time::Instant::now() + RETRY_AFTER;
        while !stop.load(Ordering::Acquire) && std::time::Instant::now() < until {
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Fits the file to the 400x400 screen, turns it for the fan's mounting and encodes it the
/// way the screen takes it (H.264 without B-frames). Kept in the OLED cache folder.
fn encode(file: &Path, fit: &str, rotation: u16) -> Result<PathBuf> {
    let _one_at_a_time = ENCODING.lock();
    anyhow::ensure!(file.is_file(), "{} does not exist", file.display());
    let source = std::fs::metadata(file)?;
    let cache_dir = oled_cache_dir();
    std::fs::create_dir_all(&cache_dir)?;
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "media".into());
    let output = cache_dir.join(format!(
        "{stem}-{:x}-fan-{fit}-{rotation}.h264",
        source.len()
    ));
    if let Ok(meta) = std::fs::metadata(&output) {
        let fresh = source
            .modified()
            .ok()
            .zip(meta.modified().ok())
            .is_none_or(|(src, out)| out >= src);
        if fresh && meta.len() > 0 {
            return Ok(output);
        }
    }
    let partial = output.with_extension("partial");
    let fit = if fit == "contain" {
        format!("scale={SIZE}:{SIZE}:force_original_aspect_ratio=decrease:flags=lanczos,pad={SIZE}:{SIZE}:(ow-iw)/2:(oh-ih)/2:color=black")
    } else {
        format!("scale={SIZE}:{SIZE}:force_original_aspect_ratio=increase:flags=lanczos,crop={SIZE}:{SIZE}")
    };
    // quarter turns are exact; other angles turn about the centre, which the round screen
    // shows whole, with black filling the corners
    let turn = match rotation {
        0 => String::new(),
        90 => ",transpose=1".into(),
        180 => ",hflip,vflip".into(),
        270 => ",transpose=2".into(),
        angle => format!(",rotate={angle}*PI/180:ow={SIZE}:oh={SIZE}:c=black"),
    };
    let mut ffmpeg = std::process::Command::new(crate::oled_library::ffmpeg());
    ffmpeg.args(["-y", "-loglevel", "error"]);
    if crate::engine::is_still_image(file) {
        ffmpeg.args(["-loop", "1", "-t", "1"]);
    }
    ffmpeg
        .arg("-i")
        .arg(file)
        .args(["-vf", &format!("{fit}{turn},fps={FPS}")])
        .args([
            "-r",
            &FPS.to_string(),
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ])
        .args(["-x264opts", "bframes=0", "-pix_fmt", "yuv420p"])
        .args(["-color_range", "pc", "-an", "-f", "h264"])
        .arg(&partial);
    no_window(&mut ffmpeg);
    info!("Encoding {} for the fan screens", file.display());
    let result = ffmpeg
        .output()
        .context("running ffmpeg (install it with: winget install Gyan.FFmpeg)")?;
    if !result.status.success() {
        let _ = std::fs::remove_file(&partial);
        bail!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    std::fs::rename(&partial, &output)?;
    Ok(output)
}

/// What an L-Connect import brought in.
#[derive(Serialize, Default)]
pub struct Imported {
    pub videos: usize,
    pub images: usize,
}

/// Folders of L-Connect's device assets that hold previews and pieces, not things to play.
const SKIP_DIRS: [&str; 7] = [
    "preview",
    "thumbnail",
    "temp",
    "modulars",
    "overlay-snapshots",
    "template",
    "theme",
];
const PLAYABLE: [&str; 5] = ["mp4", "mkv", "webm", "avi", "gif"];
const STILL: [&str; 3] = ["png", "jpg", "jpeg"];

/// Copies the videos and pictures from an L-Connect device asset folder into `out`. Its
/// scrambled .h264 copies and transparent .mov overlays are left out: the .mp4 next to them
/// is the same video.
pub fn import_lconnect(folder: &Path, out: &Path) -> Result<Imported> {
    let mut found = Vec::new();
    collect(folder, 0, &mut found);
    if found.is_empty() {
        bail!("No L-Connect videos or pictures in that folder");
    }
    std::fs::create_dir_all(out)?;
    let mut imported = Imported::default();
    for file in found {
        let Some(name) = file.file_name() else {
            continue;
        };
        let target = out.join(name);
        let same = std::fs::metadata(&target)
            .ok()
            .zip(std::fs::metadata(&file).ok())
            .is_some_and(|(a, b)| a.len() == b.len());
        if !same {
            std::fs::copy(&file, &target).with_context(|| format!("copying {}", file.display()))?;
        }
        if is_ext(&file, &STILL) {
            imported.images += 1;
        } else {
            imported.videos += 1;
        }
    }
    Ok(imported)
}

fn collect(dir: &Path, depth: u8, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if depth < 4 && !SKIP_DIRS.contains(&name.as_str()) {
                collect(&path, depth + 1, found);
            }
        } else if is_ext(&path, &PLAYABLE) || is_ext(&path, &STILL) {
            found.push(path);
        }
    }
}

fn is_ext(path: &Path, list: &[&str]) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| list.contains(&e.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_playable_files_and_skips_previews_and_scrambled_copies() {
        let base = std::env::temp_dir().join(format!("rgbable-fan-import-{}", std::process::id()));
        let assets = base.join("sl-inf-lcd");
        for dir in ["default", "video", "preview"] {
            std::fs::create_dir_all(assets.join(dir)).unwrap();
        }
        std::fs::write(assets.join("default/spin.mp4"), b"v").unwrap();
        std::fs::write(assets.join("default/spin.h264"), b"x").unwrap();
        std::fs::write(assets.join("video/logo.png"), b"p").unwrap();
        std::fs::write(assets.join("video/glow.mov"), b"m").unwrap();
        std::fs::write(assets.join("preview/spin.png"), b"p").unwrap();
        let out = base.join("out");
        let imported = import_lconnect(&base, &out).unwrap();
        assert_eq!((imported.videos, imported.images), (1, 1));
        assert!(out.join("spin.mp4").is_file() && out.join("logo.png").is_file());
        assert!(!out.join("spin.png").exists() && !out.join("glow.mov").exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn cleans_rotation_and_fit() {
        let c = FanScreenConfig {
            rotation: 405,
            fit: "stretch".into(),
            ..Default::default()
        }
        .cleaned();
        assert_eq!((c.rotation, c.fit.as_str()), (45, "cover"));
    }
}
