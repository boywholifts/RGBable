//! The HydroShift II OLED as a Windows monitor, like L-Connect's secondary display mode.
//!
//! Windows only extends the desktop onto a display it has a driver for, so this installs the
//! open-source Virtual Display Driver (MIT, signed: github.com/VirtualDrivers) once, with one
//! UAC prompt, along with two scheduled tasks that switch its monitor on and off without
//! further prompts. While the mode is on, RGBable copies that monitor to the OLED
//! (see `Engine::oled_monitor_loop`). The monitor is switched off whenever the mode isn't
//! on, so it doesn't sit in Windows' display layout unseen.

use crate::openrgb_host::{current_user, hidden, ps_quote, run, run_elevated_script};
use anyhow::{bail, Context, Result};
use std::time::{Duration, Instant};

const DRIVER_URL: &str = "https://github.com/VirtualDrivers/Virtual-Display-Driver/releases/download/25.7.23/VirtualDisplayDriver-x86.Driver.Only.zip";
const DRIVER_SHA256: &str = "e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a";
const NEFCON_URL: &str =
    "https://github.com/nefarius/nefcon/releases/download/v1.14.0/nefcon_v1.14.0.zip";
const NEFCON_SHA256: &str = "a15557da24a9efca203158de3b43b0eaf982db231f0194031f1ed428bc13e669";
/// Where the driver reads its settings (its default, also set in its registry key).
const SETTINGS_DIR: &str = r"C:\VirtualDisplayDriver";
const TASK_ON: &str = "AIO monitor on";
const TASK_OFF: &str = "AIO monitor off";
const PIPE: &str = r"\\.\pipe\MTTVirtualDisplayPipe";
/// The OLED's panel, landscape.
pub const WIDE: (u32, u32) = (2288, 1080);

fn device_command(verb: &str) -> String {
    format!(
        "Get-PnpDevice -Class Display -ErrorAction SilentlyContinue | Where-Object {{ $_.HardwareID -contains 'Root\\MttVDD' }} | {verb}-PnpDevice -Confirm:$false"
    )
}

/// The driver's settings: one monitor at the OLED's size, landscape or portrait.
fn settings_xml(portrait: bool) -> String {
    let (w, h) = if portrait { (WIDE.1, WIDE.0) } else { WIDE };
    format!(
        "<?xml version='1.0' encoding='utf-8'?>\r\n<vdd_settings>\r\n  <monitors><count>1</count></monitors>\r\n  <gpu><friendlyname>default</friendlyname></gpu>\r\n  <global><g_refresh_rate>60</g_refresh_rate></global>\r\n  <resolutions>\r\n    <resolution><width>{w}</width><height>{h}</height><refresh_rate>60</refresh_rate></resolution>\r\n  </resolutions>\r\n  <options>\r\n    <CustomEdid>false</CustomEdid>\r\n    <PreventSpoof>false</PreventSpoof>\r\n    <EdidCeaOverride>false</EdidCeaOverride>\r\n    <HardwareCursor>false</HardwareCursor>\r\n    <SDR10bit>false</SDR10bit>\r\n    <HDRPlus>false</HDRPlus>\r\n    <logging>false</logging>\r\n    <debuglogging>false</debuglogging>\r\n  </options>\r\n</vdd_settings>\r\n"
    )
}

/// Whether the one-time setup has been done.
pub fn installed() -> bool {
    run(
        "schtasks",
        &["/Query", "/TN", &format!(r"\RGBable\{TASK_ON}")],
    )
    .is_ok_and(|o| o.status.success())
}

/// The one-time, elevated setup: downloads and checks the driver and the tool that
/// installs it, installs the driver with its monitor switched off, and adds the on/off
/// tasks. Settings go where the user can change them later without a prompt.
pub fn install(portrait: bool) -> Result<()> {
    let user = ps_quote(&current_user());
    let task = |name: &str, verb: &str| {
        format!(
            "$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument {args}\r\n\
             $principal = New-ScheduledTaskPrincipal -UserId {user} -LogonType Interactive -RunLevel Highest\r\n\
             $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 2) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\r\n\
             Register-ScheduledTask -TaskName {name} -TaskPath '\\RGBable\\' -Action $action -Principal $principal -Settings $settings -Force | Out-Null\r\n",
            args = ps_quote(&format!(
                "-NoProfile -NonInteractive -WindowStyle Hidden -Command \"{}\"",
                device_command(verb)
            )),
            name = ps_quote(name),
        )
    };
    let script = format!(
        "$work = Join-Path $env:ProgramData 'RGBable\\VirtualDisplay'\r\n\
         New-Item -ItemType Directory -Path $work -Force | Out-Null\r\n\
         [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12\r\n\
         foreach ($get in @(@{{u={driver_url}; f='driver.zip'; h={driver_sha}}}, @{{u={nefcon_url}; f='nefcon.zip'; h={nefcon_sha}}})) {{\r\n\
           $file = Join-Path $work $get.f\r\n\
           Invoke-WebRequest -Uri $get.u -OutFile $file -UseBasicParsing\r\n\
           if ((Get-FileHash $file -Algorithm SHA256).Hash -ne $get.h) {{ throw \"$($get.f) didn't download intact\" }}\r\n\
           Expand-Archive -Path $file -DestinationPath $work -Force\r\n\
         }}\r\n\
         $settingsDir = {settings_dir}\r\n\
         New-Item -ItemType Directory -Path $settingsDir -Force | Out-Null\r\n\
         Set-Content -Path (Join-Path $settingsDir 'vdd_settings.xml') -Value {xml} -Encoding UTF8\r\n\
         & icacls $settingsDir /grant ('{{0}}:(OI)(CI)M' -f {user}) | Out-Null\r\n\
         New-Item -Path 'HKLM:\\SOFTWARE\\MikeTheTech\\VirtualDisplayDriver' -Force | Out-Null\r\n\
         Set-ItemProperty -Path 'HKLM:\\SOFTWARE\\MikeTheTech\\VirtualDisplayDriver' -Name 'VDDPATH' -Value $settingsDir\r\n\
         $catalog = Join-Path $work 'VirtualDisplayDriver\\mttvdd.cat'\r\n\
         $certificates = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2Collection\r\n\
         $certificates.Import([IO.File]::ReadAllBytes($catalog))\r\n\
         $store = New-Object System.Security.Cryptography.X509Certificates.X509Store('TrustedPublisher', 'LocalMachine')\r\n\
         $store.Open('ReadWrite'); foreach ($c in $certificates) {{ $store.Add($c) }}; $store.Close()\r\n\
         $present = Get-PnpDevice -Class Display -ErrorAction SilentlyContinue | Where-Object {{ $_.HardwareID -contains 'Root\\MttVDD' }}\r\n\
         if (-not $present) {{\r\n\
           $p = Start-Process -FilePath (Join-Path $work 'x64\\nefconw.exe') -ArgumentList 'install', ('\"' + (Join-Path $work 'VirtualDisplayDriver\\MttVDD.inf') + '\"'), 'Root\\MttVDD' -Wait -PassThru\r\n\
           if ($p.ExitCode -ne 0) {{ throw \"the driver didn't install (nefcon exit $($p.ExitCode))\" }}\r\n\
           Start-Sleep -Seconds 5\r\n\
         }}\r\n\
         {off}\r\n\
         {on_task}{off_task}",
        driver_url = ps_quote(DRIVER_URL),
        driver_sha = ps_quote(&DRIVER_SHA256.to_uppercase()),
        nefcon_url = ps_quote(NEFCON_URL),
        nefcon_sha = ps_quote(&NEFCON_SHA256.to_uppercase()),
        settings_dir = ps_quote(SETTINGS_DIR),
        xml = ps_quote(&settings_xml(portrait)),
        off = device_command("Disable"),
        on_task = task(TASK_ON, "Enable"),
        off_task = task(TASK_OFF, "Disable"),
    );
    run_elevated_script("aio-monitor-setup.ps1", &script)?;
    anyhow::ensure!(installed(), "the on/off tasks weren't created");
    Ok(())
}

/// Removes the driver, its settings and the tasks (one UAC prompt).
pub fn uninstall() -> Result<()> {
    let script = format!(
        "Get-PnpDevice -Class Display -ErrorAction SilentlyContinue | Where-Object {{ $_.HardwareID -contains 'Root\\MttVDD' }} | ForEach-Object {{ & pnputil /remove-device $_.InstanceId | Out-Null }}\r\n\
         Get-WindowsDriver -Online -ErrorAction SilentlyContinue | Where-Object {{ $_.OriginalFileName -like '*mttvdd.inf' }} | ForEach-Object {{ & pnputil /delete-driver $_.Driver /uninstall /force | Out-Null }}\r\n\
         foreach ($name in @({on}, {off})) {{ Unregister-ScheduledTask -TaskName $name -TaskPath '\\RGBable\\' -Confirm:$false -ErrorAction SilentlyContinue }}\r\n\
         Remove-Item -Path {settings_dir} -Recurse -Force -ErrorAction SilentlyContinue\r\n\
         Remove-Item -Path (Join-Path $env:ProgramData 'RGBable\\VirtualDisplay') -Recurse -Force -ErrorAction SilentlyContinue\r\n",
        on = ps_quote(TASK_ON),
        off = ps_quote(TASK_OFF),
        settings_dir = ps_quote(SETTINGS_DIR),
    );
    run_elevated_script("aio-monitor-remove.ps1", &script)
}

/// Sets landscape or portrait. Takes effect the next time the monitor comes on, or now
/// (the driver reloads) if it's on.
pub fn set_orientation(portrait: bool) -> Result<()> {
    let path = std::path::Path::new(SETTINGS_DIR).join("vdd_settings.xml");
    let wanted = settings_xml(portrait);
    if std::fs::read_to_string(&path).is_ok_and(|now| now.trim_start_matches('\u{feff}') == wanted)
    {
        return Ok(());
    }
    std::fs::write(&path, wanted).with_context(|| format!("writing {}", path.display()))?;
    if display().is_some() {
        reload_driver()?;
    }
    Ok(())
}

/// Asks the driver to reread its settings, through its pipe (open to every user).
fn reload_driver() -> Result<()> {
    use std::io::Write;
    let mut pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(PIPE)
        .context("the virtual display driver isn't running")?;
    let message: Vec<u8> = "RELOAD_DRIVER"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    pipe.write_all(&message)?;
    Ok(())
}

/// Switches the virtual monitor on or off, and waits (on: up to 15 s) for Windows to show it.
pub fn switch(on: bool) -> Result<()> {
    if !installed() {
        bail!("set up the AIO as a monitor first");
    }
    if (display().is_some()) == on {
        return Ok(());
    }
    let task = format!(r"\RGBable\{}", if on { TASK_ON } else { TASK_OFF });
    let output = run("schtasks", &["/Run", "/TN", &task])?;
    if !output.status.success() {
        bail!(
            "starting {task}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if (display().is_some()) == on {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    if on {
        bail!("Windows didn't add the virtual monitor");
    }
    Ok(())
}

/// The virtual monitor while Windows shows it: its name (\\.\DISPLAYn) and size.
#[cfg(windows)]
pub fn display() -> Option<(String, u32, u32)> {
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayDevicesW, EnumDisplaySettingsExW, DEVMODEW, DISPLAY_DEVICEW,
        DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, ENUM_CURRENT_SETTINGS,
    };
    let text = |w: &[u16]| {
        let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
        String::from_utf16_lossy(&w[..end])
    };
    for index in 0..32 {
        let mut device = DISPLAY_DEVICEW {
            cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        // SAFETY: device is a sized DISPLAY_DEVICEW with cb set, as the call requires
        if unsafe { EnumDisplayDevicesW(std::ptr::null(), index, &mut device, 0) } == 0 {
            break;
        }
        if device.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0
            || !text(&device.DeviceString).contains("Virtual Display Driver")
        {
            continue;
        }
        let mut mode = DEVMODEW {
            dmSize: std::mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        // SAFETY: DeviceName is the null-terminated name the previous call filled in
        if unsafe {
            EnumDisplaySettingsExW(
                device.DeviceName.as_ptr(),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
                0,
            )
        } == 0
        {
            continue;
        }
        return Some((
            text(&device.DeviceName),
            mode.dmPelsWidth,
            mode.dmPelsHeight,
        ));
    }
    None
}

#[cfg(not(windows))]
pub fn display() -> Option<(String, u32, u32)> {
    None
}

/// The Desktop Duplication output index of the virtual monitor, found by trying each output
/// of the main graphics card for one of its size (no other screen here is that size).
pub fn capture_output(size: (u32, u32)) -> Option<u32> {
    (0..8).find(|&index| {
        let mut probe = std::process::Command::new(crate::oled_library::ffmpeg());
        probe.args([
            "-hide_banner",
            "-f",
            "lavfi",
            "-i",
            &format!("ddagrab=output_idx={index}:framerate=5"),
            "-frames:v",
            "1",
            "-f",
            "null",
            "-",
        ]);
        hidden(&mut probe);
        probe.output().ok().is_some_and(|o| {
            crate::oled_library::parse_video_size(&String::from_utf8_lossy(&o.stderr)) == Some(size)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_hold_the_oleds_size_either_way_round() {
        assert!(settings_xml(false).contains("<width>2288</width><height>1080</height>"));
        assert!(settings_xml(true).contains("<width>1080</width><height>2288</height>"));
    }
}
