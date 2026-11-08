//! Videos and images for the OLED screen: OLEDable's library folders (found through its
//! settings file) plus folders the user adds in RGBable. Thumbnails are made with ffmpeg and
//! cached in the temp folder.

use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const MEDIA: &[&str] = &[
    "mp4", "mkv", "mov", "webm", "avi", "gif", "png", "jpg", "jpeg", "bmp", "webp",
];
const MAX_ITEMS: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub struct LibraryItem {
    pub path: String,
    pub name: String,
    pub folder: String,
    pub size: u64,
    pub modified: u64,
    pub image: bool,
    pub thumb: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryFolder {
    pub path: String,
    pub label: String,
    pub from_oledable: bool,
    pub exists: bool,
}

pub fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MEDIA.contains(&e.to_ascii_lowercase().as_str()))
}

fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["png", "jpg", "jpeg", "bmp", "webp"].contains(&e.to_ascii_lowercase().as_str())
    })
}

/// OLEDable's settings: next to its exe when that folder is writable (the default install),
/// otherwise in Local AppData.
fn oledable_settings() -> Option<serde_json::Value> {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?;
    let candidates = [
        local
            .join("Programs")
            .join("OLEDable")
            .join("OLEDable.json"),
        local.join("OLEDable").join("OLEDable.json"),
        local.join("HydroShift Recorder").join("OLEDable.json"),
    ];
    candidates.iter().find_map(|path| {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
    })
}

fn videos_folder() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("Videos"))
}

/// OLEDable's converted-videos folder and its library folder, as OLEDable itself defines them.
pub fn oledable_folders() -> Vec<PathBuf> {
    let settings = oledable_settings();
    let text = |key: &str| {
        settings
            .as_ref()
            .and_then(|s| s.get(key))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
    };
    let Some(out) = text("OutDir").or_else(|| videos_folder().map(|v| v.join("OLEDable"))) else {
        return Vec::new();
    };
    let converted = text("ConvertedDir").unwrap_or_else(|| out.join("Converted"));
    vec![converted, out]
}

pub fn folders(link_oledable: bool, extra: &[PathBuf]) -> Vec<LibraryFolder> {
    let mut list = Vec::new();
    if link_oledable {
        for (path, label) in oledable_folders()
            .into_iter()
            .zip(["OLEDable · converted", "OLEDable"])
        {
            list.push(LibraryFolder {
                exists: path.is_dir(),
                path: path.to_string_lossy().into_owned(),
                label: label.to_string(),
                from_oledable: true,
            });
        }
    }
    for path in extra {
        list.push(LibraryFolder {
            exists: path.is_dir(),
            label: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            path: path.to_string_lossy().into_owned(),
            from_oledable: false,
        });
    }
    list
}

fn thumb_dir() -> PathBuf {
    std::env::temp_dir().join("rgbable").join("thumbs")
}

fn thumb_path(file: &Path, modified: u64) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    file.hash(&mut hasher);
    modified.hash(&mut hasher);
    thumb_dir().join(format!("{:016x}.jpg", hasher.finish()))
}

/// Lists the media in the library folders, newest first; a file in two folders is listed once.
pub fn list(folders: &[LibraryFolder]) -> Vec<LibraryItem> {
    let mut seen = HashSet::new();
    let mut items = Vec::new();
    for folder in folders.iter().filter(|f| f.exists) {
        let Ok(entries) = std::fs::read_dir(&folder.path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file()
                || !is_media(&path)
                || !seen.insert(path.to_string_lossy().to_lowercase())
            {
                continue;
            }
            let meta = entry.metadata().ok();
            let modified = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            let image = is_image(&path);
            let thumb = if image {
                Some(path.to_string_lossy().into_owned())
            } else {
                let t = thumb_path(&path, modified);
                t.is_file().then(|| t.to_string_lossy().into_owned())
            };
            items.push(LibraryItem {
                name: path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: path.to_string_lossy().into_owned(),
                folder: folder.label.clone(),
                size: meta.map_or(0, |m| m.len()),
                modified,
                image,
                thumb,
            });
        }
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.modified));
    items.truncate(MAX_ITEMS);
    items
}

/// Makes the missing video thumbnails, one ffmpeg run at a time; returns how many it made.
pub fn make_thumbnails(items: &[LibraryItem], ffmpeg: &Path) -> usize {
    let _ = std::fs::create_dir_all(thumb_dir());
    let mut made = 0;
    for item in items.iter().filter(|i| !i.image && i.thumb.is_none()) {
        let target = thumb_path(Path::new(&item.path), item.modified);
        let mut command = std::process::Command::new(ffmpeg);
        command
            .args(["-v", "error", "-y", "-ss", "1", "-i"])
            .arg(&item.path)
            .args(["-frames:v", "1", "-vf", "scale=360:-2", "-q:v", "5"]);
        command.arg(&target);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let ok = command.output().is_ok_and(|o| o.status.success()) && target.is_file();
        if !ok {
            // Very short clips have no frame at one second; take the first one instead.
            let mut retry = std::process::Command::new(ffmpeg);
            retry
                .args(["-v", "error", "-y", "-i"])
                .arg(&item.path)
                .args(["-frames:v", "1", "-vf", "scale=360:-2", "-q:v", "5"])
                .arg(&target);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                retry.creation_flags(0x0800_0000);
            }
            let _ = retry.output();
        }
        if target.is_file() {
            made += 1;
        }
    }
    made
}

/// ffmpeg from PATH, or the one OLEDable's installer set up.
pub fn ffmpeg() -> PathBuf {
    let from_oledable = oledable_settings()
        .and_then(|s| {
            s.get("FfmpegPath")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
        })
        .filter(|p| p.is_file());
    from_oledable.unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

pub fn thumbs_folder() -> PathBuf {
    thumb_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_media_newest_first_and_skips_other_files() {
        let dir = std::env::temp_dir().join(format!("rgbable-lib-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.mp4"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("b.PNG"), b"x").unwrap();
        let folders = vec![LibraryFolder {
            path: dir.to_string_lossy().into_owned(),
            label: "test".into(),
            from_oledable: false,
            exists: true,
        }];
        let items = list(&folders);
        let names: Vec<_> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(items.len(), 2, "{names:?}");
        assert!(items
            .iter()
            .any(|i| i.name == "b" && i.image && i.thumb.is_some()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
