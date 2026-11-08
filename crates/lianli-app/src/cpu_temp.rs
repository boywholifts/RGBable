//! The Intel CPU's package temperature, for the screen's widgets.
//!
//! Windows only lets administrators read it, through the PawnIO driver (which OpenRGB's RAM
//! support already installs). RGBable runs as the user, so a one-time elevated setup registers
//! a scheduled task that starts RGBable itself as a small administrator helper; the helper
//! writes the temperature to a file each second, and stops once RGBable hasn't asked for it
//! for a while.

use anyhow::{Context, Result};
use parking_lot::Mutex;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

pub const HELPER_ARG: &str = "--cpu-temp-helper";
const TASK: &str = r"\RGBable\CPU temperature";
/// A reading older than this is treated as none.
const FRESH: Duration = Duration::from_secs(5);
/// The helper stops when RGBable hasn't asked for a reading for this long.
const UNWANTED_AFTER: Duration = Duration::from_secs(30);
const START_RETRY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub supported: bool,
    pub pawnio: bool,
    pub task: bool,
}

fn reading_file(dir: &Path) -> PathBuf {
    dir.join("cpu-temp.txt")
}

fn wanted_file(dir: &Path) -> PathBuf {
    dir.join("cpu-temp.wanted")
}

fn age(path: &Path) -> Option<Duration> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}

/// The latest temperature in °C, asking the helper to run (and starting it) as needed.
pub fn read(dir: &Path) -> Option<u32> {
    static LAST_WANT: Mutex<Option<Instant>> = Mutex::new(None);
    static LAST_START: Mutex<Option<Instant>> = Mutex::new(None);
    {
        let mut last = LAST_WANT.lock();
        if last.is_none_or(|at| at.elapsed() >= Duration::from_secs(5)) {
            let _ = std::fs::write(wanted_file(dir), b"1");
            *last = Some(Instant::now());
        }
    }
    let file = reading_file(dir);
    let fresh = age(&file).is_some_and(|a| a < FRESH);
    if !fresh {
        let mut last = LAST_START.lock();
        if last.is_none_or(|at| at.elapsed() >= START_RETRY) && task_exists() {
            *last = Some(Instant::now());
            start_task();
        }
        return None;
    }
    std::fs::read_to_string(file).ok()?.trim().parse().ok()
}

pub fn status() -> Status {
    Status {
        supported: cfg!(windows) && is_intel(),
        pawnio: crate::openrgb_host::pawnio_installed(),
        task: task_exists(),
    }
}

fn is_intel() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        // SAFETY: CPUID leaf 0 is available on every x86-64 CPU. (Older compilers still
        // mark the intrinsic unsafe.)
        #[allow(unused_unsafe)]
        let id = unsafe { std::arch::x86_64::__cpuid(0) };
        let vendor = [id.ebx, id.edx, id.ecx]
            .iter()
            .flat_map(|r| r.to_le_bytes())
            .collect::<Vec<u8>>();
        vendor == b"GenuineIntel"
    }
    #[cfg(not(target_arch = "x86_64"))]
    false
}

fn hidden(command: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

fn task_exists() -> bool {
    cfg!(windows)
        && hidden(std::process::Command::new("schtasks").args(["/Query", "/TN", TASK]))
            .output()
            .is_ok_and(|o| o.status.success())
}

fn start_task() {
    let _ = hidden(std::process::Command::new("schtasks").args(["/Run", "/TN", TASK])).output();
}

/// One administrator prompt: installs PawnIO if needed and registers the helper's task.
pub fn setup() -> Result<()> {
    anyhow::ensure!(
        is_intel(),
        "CPU temperature is only supported on Intel CPUs for now"
    );
    if !crate::openrgb_host::pawnio_installed() {
        crate::openrgb_host::install_pawnio()?;
    }
    let exe = std::env::current_exe().context("finding RGBable's own program file")?;
    crate::openrgb_host::create_helper_task("CPU temperature", &exe, HELPER_ARG)?;
    anyhow::ensure!(task_exists(), "the CPU temperature task wasn't created");
    start_task();
    Ok(())
}

/// The administrator helper: reads the temperature each second until RGBable stops asking.
pub fn run_helper(dir: &Path) -> Result<()> {
    let msr = msr::IntelMsr::open()?;
    let target = msr.temperature_target()?;
    let file = reading_file(dir);
    let partial = file.with_extension("partial");
    loop {
        if age(&wanted_file(dir)).is_none_or(|a| a > UNWANTED_AFTER) {
            return Ok(());
        }
        if let Ok(readout) = msr.package_readout() {
            let celsius = target.saturating_sub(readout);
            if std::fs::write(&partial, celsius.to_string()).is_ok() {
                let _ = std::fs::rename(&partial, &file);
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(windows)]
mod msr {
    use anyhow::{bail, Result};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const MODULE: &[u8] = include_bytes!("../res/pawnio/IntelMSR.bin");
    const DEVICE_TYPE: u32 = 41_394 << 16;
    const LOAD_BINARY: u32 = DEVICE_TYPE | (0x821 << 2);
    const EXECUTE: u32 = DEVICE_TYPE | (0x841 << 2);
    const NAME_LEN: usize = 32;
    const IA32_TEMPERATURE_TARGET: u64 = 0x1A2;
    const IA32_PACKAGE_THERM_STATUS: u64 = 0x1B1;

    pub struct IntelMsr(HANDLE);

    // SAFETY: the handle is only used through DeviceIoControl, which is thread-safe.
    unsafe impl Send for IntelMsr {}

    impl IntelMsr {
        pub fn open() -> Result<Self> {
            let path: Vec<u16> = r"\\?\GLOBALROOT\Device\PawnIO"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            // SAFETY: the path is NUL-terminated and outlives the call.
            let handle = unsafe {
                CreateFileW(
                    path.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    std::ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE || handle.is_null() {
                bail!("can't open the PawnIO driver (is it installed, and is this running as administrator?)");
            }
            let msr = Self(handle);
            msr.control(LOAD_BINARY, MODULE, &mut [])?;
            Ok(msr)
        }

        fn control(&self, code: u32, input: &[u8], output: &mut [u8]) -> Result<u32> {
            let mut returned = 0u32;
            // SAFETY: both buffers are valid for the lengths passed.
            let ok = unsafe {
                DeviceIoControl(
                    self.0,
                    code,
                    input.as_ptr().cast(),
                    input.len() as u32,
                    if output.is_empty() {
                        std::ptr::null_mut()
                    } else {
                        output.as_mut_ptr().cast()
                    },
                    output.len() as u32,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                bail!("PawnIO request failed: {}", std::io::Error::last_os_error());
            }
            Ok(returned)
        }

        fn read(&self, msr: u64) -> Result<u64> {
            let mut input = vec![0u8; NAME_LEN + 8];
            let name = b"ioctl_read_msr";
            input[..name.len()].copy_from_slice(name);
            input[NAME_LEN..].copy_from_slice(&msr.to_le_bytes());
            let mut output = [0u8; 8];
            let returned = self.control(EXECUTE, &input, &mut output)?;
            anyhow::ensure!(returned >= 8, "PawnIO returned no value for MSR {msr:#x}");
            Ok(u64::from_le_bytes(output))
        }

        /// TjMax, the temperature the readouts count down from.
        pub fn temperature_target(&self) -> Result<u32> {
            let target = ((self.read(IA32_TEMPERATURE_TARGET)? >> 16) & 0xFF) as u32;
            Ok(if target == 0 { 100 } else { target })
        }

        /// How many degrees below TjMax the package is.
        pub fn package_readout(&self) -> Result<u32> {
            Ok(((self.read(IA32_PACKAGE_THERM_STATUS)? >> 16) & 0x7F) as u32)
        }
    }

    impl Drop for IntelMsr {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by CreateFileW and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(not(windows))]
mod msr {
    use anyhow::{bail, Result};

    pub struct IntelMsr;

    impl IntelMsr {
        pub fn open() -> Result<Self> {
            bail!("CPU temperature is only read on Windows")
        }
        pub fn temperature_target(&self) -> Result<u32> {
            bail!("unsupported")
        }
        pub fn package_readout(&self) -> Result<u32> {
            bail!("unsupported")
        }
    }
}
