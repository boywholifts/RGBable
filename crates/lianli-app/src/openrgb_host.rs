//! Runs OpenRGB in the background so the user never has to open it.
//!
//! RAM and some motherboard lighting sit on SMBus, which OpenRGB can only reach as
//! administrator with the PawnIO driver installed. A one-time, elevated setup registers a
//! scheduled task that runs OpenRGB with highest privileges; RGBable then starts that task
//! without a UAC prompt whenever it starts.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TASK: &str = r"\RGBable\OpenRGB";
const PAWNIO_WINGET_ID: &str = "namazso.PawnIO";

#[derive(Debug, Clone, Default, Serialize)]
pub struct HostStatus {
    pub exe: Option<String>,
    pub pawnio: bool,
    pub task: bool,
    pub task_running: bool,
}

fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

fn run(program: &str, args: &[&str]) -> Result<Output> {
    hidden(Command::new(program).args(args))
        .output()
        .with_context(|| format!("running {program}"))
}

fn ps_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Writes a PowerShell script to a temp file that records any error in a side file.
fn script_file(name: &str, script: &str) -> Result<(PathBuf, PathBuf)> {
    let path = std::env::temp_dir().join("rgbable").join(name);
    std::fs::create_dir_all(path.parent().expect("temp dir has a parent"))?;
    let error_file = path.with_extension("err.txt");
    let _ = std::fs::remove_file(&error_file);
    std::fs::write(
        &path,
        format!(
            "$ErrorActionPreference = 'Stop'\r\ntry {{\r\n{script}}} catch {{\r\n  $_ | Out-String | Set-Content -Path {err}\r\n  exit 1\r\n}}\r\n",
            err = ps_quote(&error_file.to_string_lossy()),
        ),
    )?;
    Ok((path, error_file))
}

/// Runs a PowerShell script as the current user and waits for it.
fn run_script(name: &str, script: &str) -> Result<()> {
    let (path, error_file) = script_file(name, script)?;
    let output = run(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &path.to_string_lossy(),
        ],
    )?;
    if !output.status.success() {
        let reason = std::fs::read_to_string(&error_file)
            .unwrap_or_else(|_| String::from_utf8_lossy(&output.stderr).into_owned());
        bail!("{}", reason.trim());
    }
    Ok(())
}

/// Runs a PowerShell script elevated (one UAC prompt) and waits for it.
fn run_elevated_script(name: &str, script: &str) -> Result<()> {
    let (path, error_file) = script_file(name, script)?;
    let launcher = format!(
        "$p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File',{}; exit $p.ExitCode",
        ps_quote(&format!("\"{}\"", path.display()))
    );
    let output = run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &launcher],
    )?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        if detail.contains("canceled by the user") || detail.contains("cancelled by the user") {
            bail!("the administrator prompt was declined");
        }
        let reason = std::fs::read_to_string(&error_file).unwrap_or_else(|_| detail.to_string());
        bail!("setup failed: {}", reason.trim());
    }
    Ok(())
}

pub fn find_exe(configured: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = configured.filter(|p| p.is_file()) {
        return Some(path.to_path_buf());
    }
    let mut candidates = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(var) {
            candidates.push(PathBuf::from(dir).join("OpenRGB").join("OpenRGB.exe"));
        }
    }
    if let Some(dir) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(&dir)
                .join("Programs")
                .join("OpenRGB")
                .join("OpenRGB.exe"),
        );
        candidates.push(PathBuf::from(dir).join("OpenRGB").join("OpenRGB.exe"));
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .or_else(running_exe)
}

fn running_exe() -> Option<PathBuf> {
    let output = run(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "(Get-Process OpenRGB -ErrorAction SilentlyContinue | Select-Object -First 1).Path",
        ],
    )
    .ok()?;
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!path.is_empty())
        .then(|| PathBuf::from(path))
        .filter(|p| p.is_file())
}

pub fn pawnio_installed() -> bool {
    cfg!(windows) && run("sc", &["query", "PawnIO"]).is_ok_and(|o| o.status.success())
}

fn task_state() -> Option<String> {
    let output = run("schtasks", &["/Query", "/TN", TASK, "/FO", "CSV", "/NH"]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn status(configured: Option<&Path>) -> HostStatus {
    if !cfg!(windows) {
        return HostStatus::default();
    }
    let task = task_state();
    HostStatus {
        exe: find_exe(configured).map(|p| p.to_string_lossy().into_owned()),
        pawnio: pawnio_installed(),
        task: task.is_some(),
        task_running: task.is_some_and(|t| t.contains("Running")),
    }
}

/// Installs PawnIO with winget; its installer asks for administrator rights itself.
pub fn install_pawnio() -> Result<()> {
    let output = run(
        "winget",
        &[
            "install",
            "--id",
            PAWNIO_WINGET_ID,
            "--exact",
            "--silent",
            "--accept-package-agreements",
            "--accept-source-agreements",
            "--disable-interactivity",
        ],
    )
    .context("winget isn't available; install PawnIO from https://pawnio.eu")?;
    if !pawnio_installed() {
        let text = String::from_utf8_lossy(&output.stdout);
        let last = text
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        bail!("PawnIO didn't install ({last}); you can install it from https://pawnio.eu");
    }
    Ok(())
}

/// One elevated step: a scheduled task that runs OpenRGB as administrator with its SDK server.
pub fn create_task(exe: &Path) -> Result<()> {
    let script = format!(
        "$action = New-ScheduledTaskAction -Execute {exe} -Argument '--server --startminimized' -WorkingDirectory {dir}\r\n\
         $principal = New-ScheduledTaskPrincipal -UserId {user} -LogonType Interactive -RunLevel Highest\r\n\
         $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\r\n\
         Register-ScheduledTask -TaskName 'OpenRGB' -TaskPath '\\RGBable\\' -Action $action -Principal $principal -Settings $settings -Force | Out-Null\r\n",
        exe = ps_quote(&exe.to_string_lossy()),
        dir = ps_quote(&exe.parent().unwrap_or(exe).to_string_lossy()),
        user = ps_quote(&current_user()),
    );
    run_elevated_script("openrgb-task.ps1", &script)?;
    anyhow::ensure!(task_state().is_some(), "the OpenRGB task wasn't created");
    Ok(())
}

/// A scheduled task in RGBable's task folder that runs `exe` as administrator, started on demand.
pub fn create_helper_task(name: &str, exe: &Path, argument: &str) -> Result<()> {
    let script = format!(
        "$action = New-ScheduledTaskAction -Execute {exe} -Argument {argument}\r\n\
         $principal = New-ScheduledTaskPrincipal -UserId {user} -LogonType Interactive -RunLevel Highest\r\n\
         $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\r\n\
         Register-ScheduledTask -TaskName {name} -TaskPath '\\RGBable\\' -Action $action -Principal $principal -Settings $settings -Force | Out-Null\r\n",
        exe = ps_quote(&exe.to_string_lossy()),
        argument = ps_quote(argument),
        user = ps_quote(&current_user()),
        name = ps_quote(name),
    );
    run_elevated_script("helper-task.ps1", &script)
}

const LOGON_TASK: &str = "RGBable at logon";

/// Starts RGBable at logon through Task Scheduler rather than the registry's Run list: it
/// survives updates (whose uninstall step clears Run entries), and Task Scheduler records
/// how each start went. Runs as the user, without administrator rights, shortly after logon.
pub fn create_logon_task(exe: &Path, may_prompt: bool) -> Result<()> {
    let script = format!(
        "$action = New-ScheduledTaskAction -Execute {exe} -Argument '--minimized' -WorkingDirectory {dir}\r\n\
         $trigger = New-ScheduledTaskTrigger -AtLogOn -User {user}\r\n\
         $trigger.Delay = 'PT10S'\r\n\
         $principal = New-ScheduledTaskPrincipal -UserId {user} -LogonType Interactive -RunLevel Limited\r\n\
         $settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew\r\n\
         Register-ScheduledTask -TaskName {name} -TaskPath '\\' -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null\r\n",
        exe = ps_quote(&exe.to_string_lossy()),
        dir = ps_quote(&exe.parent().unwrap_or(exe).to_string_lossy()),
        user = ps_quote(&current_user()),
        name = ps_quote(LOGON_TASK),
    );
    let first = run_script("logon-task.ps1", &script);
    if first.is_ok() && logon_task().is_some() {
        return Ok(());
    }
    let detail = match first {
        Err(e) => format!("{e:#}"),
        Ok(()) => "Task Scheduler didn't keep it".to_string(),
    };
    if !may_prompt {
        bail!("adding the logon task: {detail}");
    }
    // some setups only let administrators add logon tasks
    tracing::info!("adding the logon task needs administrator rights here: {detail}");
    run_elevated_script("logon-task.ps1", &script)?;
    anyhow::ensure!(
        logon_task().is_some(),
        "Task Scheduler didn't keep the logon task"
    );
    Ok(())
}

pub fn remove_logon_task() -> Result<()> {
    if logon_task().is_none() {
        return Ok(());
    }
    let output = run("schtasks", &["/Delete", "/TN", LOGON_TASK, "/F"])?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// The logon task's details (`schtasks /Query /V` as a list), when it exists: what it runs
/// and how its last start went.
pub fn logon_task() -> Option<String> {
    let output = run(
        "schtasks",
        &["/Query", "/TN", LOGON_TASK, "/V", "/FO", "LIST"],
    )
    .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn remove_task() -> Result<()> {
    if task_state().is_none() {
        return Ok(());
    }
    run_elevated_script(
        "openrgb-task-remove.ps1",
        "Unregister-ScheduledTask -TaskName 'OpenRGB' -TaskPath '\\RGBable\\' -Confirm:$false\r\n",
    )
}

fn current_user() -> String {
    match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(user)) => format!("{domain}\\{user}"),
        (_, Ok(user)) => user,
        _ => String::new(),
    }
}

/// Starts the administrator OpenRGB. A copy the user started normally can't reach RAM and
/// would hold the SDK port, so it is closed first; the elevated copy can't be closed from
/// here and is left alone.
pub fn start() -> Result<()> {
    if task_state().is_some_and(|t| t.contains("Running")) {
        return Ok(());
    }
    // OpenRGB's installer can also set it up as a Windows service, running from boot with
    // full access: a second copy from the task would fight it for the devices
    if let Some(service) = running_service() {
        tracing::info!("OpenRGB already runs as the Windows service {service}; using that");
        return Ok(());
    }
    let _ = run("taskkill", &["/IM", "OpenRGB.exe", "/F"]);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let output = run("schtasks", &["/Run", "/TN", TASK])?;
    if !output.status.success() {
        bail!(
            "couldn't start OpenRGB: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// The name of a running Windows service that runs OpenRGB, if there is one.
fn running_service() -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let output = run(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Service | Where-Object { $_.State -eq 'Running' -and $_.PathName -match 'OpenRGB' } | Select-Object -First 1 -ExpandProperty Name",
        ],
    )
    .ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Other RGB programs that drive the same devices and overwrite RGBable's lighting.
pub fn competing_rgb_apps() -> Vec<&'static str> {
    [
        ("SignalRgb.exe", "SignalRGB"),
        ("Glorious Core.exe", "Glorious Core"),
        ("ArmouryCrate.exe", "Armoury Crate"),
        ("MSI Center.exe", "MSI Center"),
        ("iCUE.exe", "iCUE"),
    ]
    .into_iter()
    .filter(|(exe, _)| process_running(exe))
    .map(|(_, name)| name)
    .collect()
}

/// OpenRGB re-creates its Startup-folder shortcut whenever it starts with "Start at login"
/// on, which would launch a second copy without administrator rights; turn that off.
pub fn disable_openrgb_autostart() {
    let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) else {
        return;
    };
    let config = appdata.join("OpenRGB").join("OpenRGB.json");
    if let Ok(text) = std::fs::read_to_string(&config) {
        if let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(enabled) = json.pointer_mut("/AutoStart/enabled") {
                if enabled.as_bool() == Some(true) {
                    *enabled = serde_json::Value::Bool(false);
                    if let Ok(text) = serde_json::to_string_pretty(&json) {
                        let _ = std::fs::write(&config, text);
                    }
                }
            }
        }
    }
    let startup = appdata.join(r"Microsoft\Windows\Start Menu\Programs\Startup");
    let shortcut = startup.join("OpenRGB.lnk");
    if shortcut.is_file() {
        let _ = std::fs::rename(&shortcut, startup.join("OpenRGB.lnk.disabled-by-RGBable"));
    }
}

/// Restarts the administrator OpenRGB, e.g. after PawnIO was installed.
pub fn restart() -> Result<()> {
    let _ = run("schtasks", &["/End", "/TN", TASK]);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    start()
}

/// Whether a program with this file name (like "Vial.exe") is running.
#[cfg(windows)]
pub fn process_running(exe: &str) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: the snapshot handle is checked before use and closed once; the entry's size
    // field is set as the API requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut found = false;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            let name = &entry.szExeFile;
            let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
            if String::from_utf16_lossy(&name[..len]).eq_ignore_ascii_case(exe) {
                found = true;
                break;
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        found
    }
}

#[cfg(not(windows))]
pub fn process_running(_exe: &str) -> bool {
    false
}
