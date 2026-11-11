//! Startup lighting: what the PC shows while it's off (and, if chosen, locked), either a
//! lighting profile or everything dark. When Windows asks to shut down or restart, it goes
//! onto the devices: the Lian Li wireless devices keep their last effect through a power
//! cycle, and on a restart every other device keeps its last frame, so it's what the build
//! boots with. When RGBable starts after sign-in, or the PC is unlocked, the lighting from
//! before comes back.

use crate::engine::Engine;
use crate::lighting_profiles;
use crate::openrgb::OpenRgb;
use crate::{lights, render};
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

#[cfg_attr(not(windows), allow(dead_code))]
/// How long shutdown waits for the wireless devices to confirm the profile.
const CONFIRM_WAIT: Duration = Duration::from_secs(8);

/// Whether there's startup lighting to put on.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn wanted(config: &crate::engine::Config) -> bool {
    config.startup_dark || config.startup_profile.is_some()
}

/// Puts the startup lighting on, keeping the lighting from before. Returns once the
/// wireless devices have it, so the PC can switch off.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn enter(engine: &Engine, openrgb: &OpenRgb, why: &str) {
    let applied = engine.update_config(|c| {
        if c.before_standby.is_some() {
            return None;
        }
        let profile = if c.startup_dark {
            None
        } else {
            let name = c.startup_profile.clone()?;
            Some(
                c.lighting_profiles
                    .iter()
                    .find(|p| p.name == name)
                    .cloned()?,
            )
        };
        c.before_standby = Some(Box::new(lighting_profiles::capture(c, "before standby")));
        match profile {
            Some(profile) => {
                lighting_profiles::apply(c, &profile);
                Some(format!("the startup profile {}", profile.name))
            }
            None => {
                c.all_off = true;
                Some("everything dark".to_string())
            }
        }
    });
    let Some(what) = applied else {
        return;
    };
    info!("{why}: showing {what}");
    relight(engine, openrgb);
    engine.wait_for_lighting(CONFIRM_WAIT);
    engine.store_wireless_lighting();
}

/// At shutdown: the startup profile goes on just long enough for the wireless devices to
/// store it for power-on, then everything goes dark until the PC is off.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn shut_down(engine: &Engine, openrgb: &OpenRgb) {
    enter(engine, openrgb, "Windows is shutting down");
    if engine.config().startup_dark {
        return;
    }
    engine.hold_stored_lighting(true);
    engine.update_config(|c| c.all_off = true);
    relight(engine, openrgb);
    engine.wait_for_lighting(CONFIRM_WAIT);
}

/// Puts back the lighting from before the startup profile went on.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn leave(engine: &Engine, openrgb: &OpenRgb) {
    engine.hold_stored_lighting(false);
    if restore(engine) {
        info!("back from the startup profile");
        relight(engine, openrgb);
    }
}

/// At start, before anything is shown: the lighting from before the PC went off.
pub fn restore(engine: &Engine) -> bool {
    engine.update_config(|c| {
        let Some(before) = c.before_standby.take() else {
            return false;
        };
        lighting_profiles::apply(c, &before);
        true
    })
}

#[cfg_attr(not(windows), allow(dead_code))]
fn relight(engine: &Engine, openrgb: &OpenRgb) {
    render::WARMTH.store(engine.config().warmth, std::sync::atomic::Ordering::Relaxed);
    render::restart_clock(500);
    engine.reload_lighting();
    lights::sync_openrgb_effects(engine, openrgb);
}

/// Watches Windows for shutdown, restart, sign-out and the lock screen.
#[cfg(windows)]
pub fn watch(engine: Arc<Engine>, openrgb: Arc<OpenRgb>) {
    let _ = WATCHED.set((engine, openrgb));
    let _ = std::thread::Builder::new()
        .name("session-watch".into())
        .spawn(windows_watch::run);
}

#[cfg(not(windows))]
pub fn watch(_engine: Arc<Engine>, _openrgb: Arc<OpenRgb>) {}

#[cfg(windows)]
static WATCHED: std::sync::OnceLock<(Arc<Engine>, Arc<OpenRgb>)> = std::sync::OnceLock::new();

#[cfg(windows)]
mod windows_watch {
    use super::WATCHED;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::RemoteDesktop::{
        WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION,
    };
    use windows_sys::Win32::System::Shutdown::{
        ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
        TranslateMessage, MSG, WM_ENDSESSION, WM_QUERYENDSESSION, WM_WTSSESSION_CHANGE, WNDCLASSW,
        WTS_SESSION_LOCK, WTS_SESSION_UNLOCK,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A hidden top-level window: Windows only tells those about the session ending.
    pub fn run() {
        let class = wide("RGBableSessionWatch");
        unsafe {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            if hwnd.is_null() {
                tracing::warn!("couldn't watch for shutdown and the lock screen");
                return;
            }
            WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let Some((engine, openrgb)) = WATCHED.get() else {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        };
        match msg {
            // done when Windows asks rather than when it says the session is over: by then
            // the app's own window has already shut RGBable down
            WM_QUERYENDSESSION => {
                if super::wanted(&engine.config()) {
                    let reason = wide("Setting up your startup lighting");
                    ShutdownBlockReasonCreate(hwnd, reason.as_ptr());
                    super::shut_down(engine, openrgb);
                    ShutdownBlockReasonDestroy(hwnd);
                }
                1
            }
            WM_ENDSESSION => {
                // the shutdown was called off (another app refused): back to the lighting
                if wparam == 0 {
                    super::leave(engine, openrgb);
                }
                0
            }
            WM_WTSSESSION_CHANGE => {
                match wparam as u32 {
                    WTS_SESSION_LOCK if engine.config().startup_profile_when_locked => {
                        super::enter(engine, openrgb, "the PC is locked");
                    }
                    WTS_SESSION_UNLOCK => super::leave(engine, openrgb),
                    _ => {}
                }
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::DeviceConfig;

    #[test]
    fn startup_profile_goes_on_and_the_lighting_from_before_comes_back() {
        let dir = std::env::temp_dir().join(format!("rgbable-standby-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let engine = Engine::new(dir.join("config.json"));
        let openrgb = OpenRgb::new();
        engine.update_config(|c| {
            c.devices.insert(
                "fan".into(),
                DeviceConfig {
                    name: Some("Top".into()),
                    color: Some([0, 0, 255]),
                    ..DeviceConfig::default()
                },
            );
            c.devices.get_mut("fan").unwrap().color = Some([255, 0, 0]);
            let profile = lighting_profiles::capture(c, "Boot");
            c.lighting_profiles.push(profile);
            c.devices.get_mut("fan").unwrap().color = Some([0, 0, 255]);
            c.startup_profile = Some("Boot".into());
        });

        enter(&engine, &openrgb, "test");
        let during = engine.config();
        assert_eq!(during.devices["fan"].color, Some([255, 0, 0]));
        assert!(during.before_standby.is_some());
        // a second shutdown message doesn't overwrite what to come back to
        enter(&engine, &openrgb, "test");

        // the next start puts it back, as if the PC had been off
        let restarted = Engine::new(dir.join("config.json"));
        assert!(restore(&restarted));
        let after = restarted.config();
        assert_eq!(after.devices["fan"].color, Some([0, 0, 255]));
        assert_eq!(after.devices["fan"].name.as_deref(), Some("Top"));
        assert!(after.before_standby.is_none());
        assert!(!restore(&restarted));

        // dark instead of a profile: everything off, then back as it was
        restarted.update_config(|c| c.startup_dark = true);
        enter(&restarted, &openrgb, "test");
        assert!(restarted.config().all_off);
        leave(&restarted, &openrgb);
        let back = restarted.config();
        assert!(!back.all_off);
        assert_eq!(back.devices["fan"].color, Some([0, 0, 255]));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
