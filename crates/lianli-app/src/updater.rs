use anyhow::{bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

pub const REPO: &str = "boywholifts/rgbable";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_SETUP_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Stable,
    Dev,
}

#[derive(Debug, Clone, Serialize)]
pub struct Update {
    pub version: String,
    pub notes: String,
    pub page: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateView {
    pub current: String,
    pub checking: bool,
    pub installing: bool,
    pub available: Option<Update>,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Updater {
    state: Mutex<UpdateView>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    html_url: String,
    prerelease: bool,
    draft: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
}

impl Release {
    fn setup(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| {
            let name = a.name.to_ascii_lowercase();
            name.contains("setup") && name.ends_with(".exe")
        })
    }
}

/// Orders `1.2.3`, `v1.2.3` and `1.2.3-dev.4`; a release sorts after every dev build of the same number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    dev: Option<u64>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let core = core.split('+').next()?;
        let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
        let major = parts.next()??;
        let minor = parts.next()??;
        let patch = parts.next().unwrap_or(Some(0))?;
        let dev = pre.map(|pre| {
            pre.rsplit('.')
                .next()
                .and_then(|n| n.split('+').next()?.parse().ok())
                .unwrap_or(0)
        });
        Some(Self {
            major,
            minor,
            patch,
            dev,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.dev, other.dev) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(&b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .user_agent(concat!("RGBable/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn newest(releases: Vec<Release>, channel: Channel, current: Version) -> Option<Update> {
    releases
        .into_iter()
        .filter(|r| !r.draft && (channel == Channel::Dev || !r.prerelease))
        .filter_map(|r| Some((Version::parse(&r.tag_name)?, r)))
        .filter(|(v, r)| *v > current && r.setup().is_some())
        .max_by_key(|(v, _)| *v)
        .map(|(_, r)| {
            let setup = r.setup().expect("filtered on setup");
            Update {
                version: r.tag_name.trim_start_matches('v').to_string(),
                notes: r.body.clone().unwrap_or_default(),
                page: r.html_url.clone(),
                url: setup.browser_download_url.clone(),
                size: setup.size,
            }
        })
}

fn fetch(channel: Channel) -> Result<Option<Update>> {
    let current = Version::parse(CURRENT).context("this build has no valid version")?;
    let releases: Vec<Release> = agent()
        .get(&format!(
            "https://api.github.com/repos/{REPO}/releases?per_page=30"
        ))
        .set("Accept", "application/vnd.github+json")
        .call()
        .context("contacting GitHub")?
        .into_json()
        .context("reading the release list")?;
    Ok(newest(releases, channel, current))
}

impl Updater {
    pub fn view(&self) -> UpdateView {
        let mut view = self.state.lock().clone();
        view.current = CURRENT.to_string();
        view
    }

    pub fn check(&self, channel: Channel) {
        {
            let mut state = self.state.lock();
            if state.checking || state.installing {
                return;
            }
            state.checking = true;
        }
        let result = fetch(channel);
        let mut state = self.state.lock();
        state.checking = false;
        match result {
            Ok(available) => {
                if let Some(update) = &available {
                    tracing::info!("update {} available", update.version);
                }
                state.available = available;
                state.error = None;
            }
            Err(e) => {
                tracing::warn!("update check failed: {e:#}");
                state.error = Some(format!("{e:#}"));
            }
        }
    }

    /// Downloads the installer; the caller runs it with [`launch_installer`] and then exits.
    pub fn download(&self) -> Result<PathBuf> {
        let update = {
            let mut state = self.state.lock();
            let Some(update) = state.available.clone() else {
                bail!("no update available")
            };
            if state.installing {
                bail!("an update is already downloading");
            }
            state.installing = true;
            update
        };
        let result = download_setup(&update);
        let mut state = self.state.lock();
        if let Err(e) = &result {
            state.installing = false;
            state.error = Some(format!("{e:#}"));
        }
        result
    }
}

fn download_setup(update: &Update) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("rgbable").join("update");
    std::fs::create_dir_all(&dir)?;
    let name = update
        .url
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or("RGBable-setup.exe");
    let path = dir.join(name);
    let response = agent()
        .get(&update.url)
        .call()
        .context("downloading the update")?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_SETUP_BYTES)
        .read_to_end(&mut bytes)
        .context("downloading the update")?;
    if bytes.len() < 100 * 1024 || (update.size > 0 && bytes.len() as u64 != update.size) {
        bail!("the update didn't download completely, try again later");
    }
    std::fs::write(&path, bytes).with_context(|| format!("saving {}", path.display()))?;
    Ok(path)
}

/// Runs the NSIS setup silently once this process has exited, then starts the updated app.
pub fn launch_installer(setup: &std::path::Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let script = format!(
        "timeout /t 2 /nobreak >nul & start \"\" /wait \"{}\" /S & start \"\" \"{}\"",
        setup.display(),
        exe.display()
    );
    let mut command = std::process::Command::new("cmd");
    command.args(["/C", &script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command.spawn().context("starting the installer")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn release_is_newer_than_its_dev_builds() {
        assert!(v("v0.2.0") > v("0.2.0-dev.14"));
        assert!(v("0.2.0-dev.14") > v("0.2.0-dev.9"));
        assert!(v("0.2.1-dev.1") > v("0.2.0"));
        assert_eq!(v("v1.0"), v("1.0.0"));
        assert!(Version::parse("nightly").is_none());
    }

    fn release(tag: &str, prerelease: bool) -> Release {
        Release {
            tag_name: tag.into(),
            body: None,
            html_url: String::new(),
            prerelease,
            draft: false,
            assets: vec![Asset {
                name: format!("RGBable_{tag}_x64-setup.exe"),
                browser_download_url: format!("https://example/{tag}"),
                size: 1,
            }],
        }
    }

    #[test]
    fn channel_selects_newest_eligible_release() {
        let list = || {
            vec![
                release("v0.2.0", false),
                release("v0.3.0-dev.2", true),
                release("v0.1.0", false),
            ]
        };
        let current = v("0.1.0");
        assert_eq!(
            newest(list(), Channel::Stable, current).unwrap().version,
            "0.2.0"
        );
        assert_eq!(
            newest(list(), Channel::Dev, current).unwrap().version,
            "0.3.0-dev.2"
        );
        assert!(newest(list(), Channel::Stable, v("0.2.0")).is_none());
    }
}
