# RGBable troubleshooting

Start with the log: `%APPDATA%\RGBable\rgbable.log` (press Win + R and paste `%APPDATA%\RGBable`). Attach it when you report a problem. A crash that happens before the log starts is written to `rgbable-crash.log` in the same folder.

## Only one program can control each light

RGB apps don't share. Two programs driving the same device take turns overwriting each other, so lighting flickers, reverts to the previous effect or a rainbow, or a device disappears altogether.

Programs that commonly fight RGBable:

| Program | What it holds | What to do |
| --- | --- | --- |
| L-Connect | Lian Li fans, Strimers, the HydroShift screen | Quit it and turn off its "start with Windows". RGBable waits until it has closed. |
| SignalRGB | GPU, motherboard, ARGB headers, keyboards, RAM | Turn off its start with Windows in Task Manager → Startup apps. |
| Glorious Core (shows as "Electron") | Glorious keyboards such as the GMMK Pro | Turn off its start with Windows. While it runs, OpenRGB can't see the keyboard. |
| Armoury Crate, MSI Center, iCUE, NZXT CAM | Their own brand's boards, GPUs and peripherals | Turn off their lighting services or start with Windows. |

RGBable notes in its log when SignalRGB, Glorious Core, Armoury Crate, MSI Center or iCUE is running.

## Only one copy of OpenRGB

RGBable needs exactly one OpenRGB running. A second copy opens the same devices: RGBable talks to one copy while the other keeps the hardware, so lighting is accepted without errors but never shows, or devices like keyboards go missing.

Check with Task Manager → Details: there should be a single `OpenRGB.exe`. To see what started each copy, run in PowerShell:

```powershell
Get-CimInstance Win32_Process -Filter "name='OpenRGB.exe'" | Select ProcessId,ParentProcessId,CreationDate
```

A parent of `services.exe` means OpenRGB is installed as a Windows service. A parent of `svchost.exe` is a scheduled task, usually RGBable's own.

Ways an extra copy gets started:

- **L-Connect with OpenRGB integration.** L-Connect can start its own OpenRGB. If you're coming from L-Connect, quit it and turn off its start with Windows.
- **OpenRGB as a Windows service.** OpenRGB's installer can set it up as a service that runs from boot. The service runs as the SYSTEM account, so it doesn't use your OpenRGB settings: zone sizes for ARGB headers and some detection settings are missing, which leaves headers on the board's rainbow and some devices undetected. RGBable uses a running service rather than starting a second copy, but we recommend turning the service off and letting RGBable run OpenRGB instead. In PowerShell as administrator:

  ```powershell
  Get-CimInstance Win32_Service | Where-Object PathName -match 'OpenRGB' | Select Name,PathName
  Stop-Service -Name '<Name>' -Force
  Set-Service -Name '<Name>' -StartupType Disabled
  ```

- **OpenRGB's own "Start at login".** RGBable turns this off when you enable **Run OpenRGB for me**. If you set it again in OpenRGB, check Task Manager → Startup apps and the Startup folder (`shell:startup`).

## A motherboard ARGB header stays on rainbow

OpenRGB doesn't know how many LEDs are on an addressable header until it's told. With the size at 0, RGBable has no LEDs to light there, so the header keeps the board's built-in effect.

Set the LED count once: in RGBable, open the header's **⋯ → LED count**, or in OpenRGB select the motherboard and **Resize** the zone. RGBable remembers each header's size and puts it back if OpenRGB loses it.

On some boards the header numbers OpenRGB uses don't match the labels printed on the board, and a header only works when the ones before it have a size too. If a strip stays dark, give the earlier headers a size as well, then give the headers different colors to find which one drives the strip.

## Some motherboard lighting ignores RGBable

A few built-in motherboard light features aren't controllable through OpenRGB, so RGBable can't change them or turn them off either. One example is the Polymo panel on ASUS ROG boards such as the Maximus Z690 Hero. To check, change the board's lighting in OpenRGB itself: if OpenRGB has no effect on a light, use the board maker's app (Armoury Crate for ASUS) or the BIOS LED lighting settings for it. A BIOS setting may turn off other onboard lighting too.

## A keyboard or other device isn't found

- Make sure its maker's app isn't running (see the table above) and only one OpenRGB runs.
- RGBable asks OpenRGB to look again about 25 and 60 seconds after starting for devices you've given lighting that it hasn't found. A device that's simply off, like a controller, isn't rescanned for, because a rescan briefly resets every device.
- While the Vial app is open, RGBable leaves keyboards alone so Vial can show its own lighting.

## RGBable doesn't start with Windows

RGBable starts at logon through a scheduled task named **RGBable at logon**, which survives updates. Check it with:

```bat
schtasks /Query /TN "RGBable at logon" /V /FO LIST
```

If it's missing, turn **Settings → Start with Windows** off and on; RGBable shows an error if Windows refuses. If it's listed but RGBable didn't start, send the **Last Run Time** and **Last Result** lines with your log. A note on the Settings page shows when the task is disabled in Task Scheduler.

## Fans run at full speed briefly after boot

Wireless fans on a coolant curve wait up to 15 seconds for the first coolant reading before falling back to full speed, so a slow start no longer spins them up. If readings never arrive, full speed is the safe fallback.

## The HydroShift screen shows nothing or "Can't preview"

- FFmpeg must be installed: `winget install Gyan.FFmpeg`, then restart RGBable.
- Quit L-Connect; it holds the screen.
- The preview says why a file can't be shown. L-Connect's `.h264` video files are scrambled and can't be played; bring its themes over with **Import L-Connect themes…** under Widgets on the HydroShift screen page instead.

## ASUS ROG Polymo panel

The Polymo panel isn't a separate device: it is the first 19 LEDs of the board's first addressable header (OpenRGB's "Addressable 1" zone). LEDs 1-11 light the ROG logo and HERO lettering, LEDs 12-19 the diamond pattern. It only follows live (Direct) colors, so OpenRGB's built-in modes such as Static don't change it. In RGBable, pick a Polymo picture for that header on the Devices page; RGBable gives headers that have no LEDs set 20 by default, which covers it; if you set the header yourself, keep it at 19 or more.
