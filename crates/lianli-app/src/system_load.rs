//! How busy the CPU and memory are, for the screen's widgets.

/// Percent of CPU time spent working since the previous call.
#[cfg(windows)]
pub fn cpu_load() -> Option<u32> {
    use parking_lot::Mutex;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;
    static LAST: Mutex<Option<(u64, u64)>> = Mutex::new(None);
    let ticks = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
    let (mut idle, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the three out-pointers are valid FILETIMEs.
    if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
        return None;
    }
    // kernel time includes idle time
    let (idle, total) = (ticks(idle), ticks(kernel) + ticks(user));
    let previous = LAST.lock().replace((idle, total));
    let (last_idle, last_total) = previous?;
    let total = total.checked_sub(last_total).filter(|&t| t > 0)?;
    let idle = idle.saturating_sub(last_idle).min(total);
    Some(((total - idle) * 100 / total) as u32)
}

/// Percent of physical memory in use.
#[cfg(windows)]
pub fn ram_load() -> Option<u32> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct's length field is set as the API requires.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.dwMemoryLoad)
}

#[cfg(not(windows))]
pub fn cpu_load() -> Option<u32> {
    None
}

#[cfg(not(windows))]
pub fn ram_load() -> Option<u32> {
    None
}
