# RGBable changelog

Versions follow [Semantic Versioning](https://semver.org). Dev builds (`x.y.z-dev.N`) are untested pre-releases for the Dev update channel.

## [Unreleased]

### Added

- Fan screens: the round LCDs of SL-INF and TL Wireless LCD fans play a video, GIF or picture each, turned to any angle. The tab shows only when they're connected. L-Connect's fan screen videos and pictures can be imported into the library (new, still being tested).
- Videos sent from OLEDable join the library.
- Startup lighting: everything dark, or a profile, from shutdown until you sign in (and, if you like, while the PC is locked). RGBable puts it on while Windows shuts down or restarts, and your own lighting comes back once you've signed in.
- Per-device color temperature, for a device that looks warmer or cooler than the rest.
- Changing a single device's lighting goes dark first, like All Devices and groups, when "Go dark before changing lighting" is on.
- Power-on lighting: store a plain or breathing version of the current colors, or Off, in OpenRGB devices that can keep a mode in their own memory.
- Fan screens can store a power-on picture.
- Strimer lanes: each lane of a Strimer can run its own effect, like L-Connect's Separate mode. The effect panel shows the Strimer's lanes: click one or several, then apply once to all of them. The panel stays open for the next lanes; Whole puts one effect back on the entire Strimer.
- Addressable headers that OpenRGB lists with no LEDs, and that you never gave a size, now get 20 LEDs, so whatever is on them (such as an ASUS Polymo panel) lights up without setting it by hand.
- ASUS Polymo panel: on an ASUS board's first addressable header (which drives the panel), the Devices page can pick the ROG logo, the diamonds, or both; effects then run across the picture's sections and the other picture stays dark.
- Resync: a device that was given its own lighting gets a resync button (and a menu item) that puts it back on its group's lighting, or All Devices' when it isn't in a group. A group can go back to All Devices' lighting the same way.
- At shutdown the startup profile only flashes on long enough to be stored, then the lights go dark until the PC is off.
- Wireless fans and Strimers now store their lighting, as L-Connect does, so after a full shutdown they come back with RGBable's lighting (or the startup lighting) instead of an old L-Connect effect.

### Changed

- One effect list, grouped by look: Solid, Rainbow, Moving, Ambient, Keyboard and Mine. Lian Li effects that looked the same as RGBable's (Static, Rainbow, Breathing, Color cycle, Meteor, Runway, Ripple, Twinkle, Drizzle) are listed once, and devices with the Lian Li version play it automatically (except in Flow through or Take turns); Lian Li-only effects sit in the same groups with a Lian Li label. The Keyboard tab comes first when a keyboard is chosen.
- Reorganized so each setting has one home. Lighting is for what's showing (with a Timing button on the All Devices card for Together, Delays, Flow through and Take turns); the new Devices page holds each device's own settings (name, LED count, color temperature, In All Devices, Polymo picture, shown on Lighting); devices move between groups there by dragging, each row shows only the settings that differ from the default, a click opens the device's settings in a side panel, and Ctrl/Shift-click or a group's ⋯ menu changes several at once; Settings groups startup and power-on lighting under When the PC starts. The device ⋯ menu is shorter and links to both.
- Drizzle no longer runs through a flow: in Flow through, a Strimer with Drizzle keeps its own (Lian Li) drizzle while effects like Meteor flow across the other devices.
- Wireless devices whose effect had to be shrunk on a crowded radio (when every device changes at once, as when the lighting follows a new HydroShift video) get the full effect back after about 30 seconds instead of 5 minutes.
- The Lighting profile menu no longer has a Lights off item; the button next to it does that.
- The Everything card is now called All Devices.

## [1.0.0] - 2026-11-09

The first stable release. RGBable now covers what most builds used L-Connect for, plus the rest of the PC's RGB through OpenRGB.

### Lighting

- Lian Li wireless SL fans (including SL Flex inner and outer rings) and wireless Strimers, the HydroShift II OLED Curve ring, and wired Galahad II LCD and UNI HUB controllers (new, still being tested).
- RAM, GPU, motherboard, ARGB headers, keyboards and monitors through OpenRGB, which RGBable can run for you as administrator, with the PawnIO driver for RAM.
- 20+ effects, L-Connect's own Lian Li effects, keyboard effects on the key grid, and custom effects from layers, a recipe builder or a script, with a live preview.
- Sync everything, groups, and a Devices page that times devices together, staggered, flowing through the build as one strip, or taking turns. Each device keeps its own direction and speed, also through Sync everything.
- Lighting profiles that save every device's lighting and, if you choose, the screen video. Picking one asks before applying.
- Turn a single device off, or all lighting at once from the Lighting and Devices pages, the profile menu or the tray. Devices keep their lighting for when they're back on.
- Match the lighting to the colors of the video on the OLED screen; white balance for LEDs.
- Motherboard ARGB headers are devices of their own; their LED counts are remembered and put back if OpenRGB loses them.

### HydroShift OLED screen

- Videos, GIFs and images with a live preview and a library shared with OLEDable.
- Widgets over the video: clock, CPU and GPU temperature and load, coolant, pump and fan speed, bars, text and images, with fonts and saved layouts.
- Import L-Connect screen themes, including their animated overlays.

### Cooling

- Fan profiles with fixed speeds or coolant curves for wireless fans, and pump speed for the HydroShift.
- Fans wait for the first coolant reading at startup instead of jumping to full speed.

### App

- Starts with Windows through a scheduled task that survives updates, and says when Windows has it disabled.
- Updates itself from GitHub Releases on the Stable or Dev channel, retrying when GitHub times out.
- First-run setup wizard, tray menu (open, all lighting off or on, restart, quit), and a crash log for errors before the main log starts.

### Fixed

- Lighting that applied briefly at startup and then reverted: RGBable resends it while OpenRGB settles, and no longer starts a second OpenRGB when one (such as a Windows service) is already running.
- Devices OpenRGB finds late, or after a rescan, are picked up without dropping the connection.
- Wireless effects that never arrived or froze: uploads are confirmed, resent and shrunk until they fit, and loops stay in step on the receiver's clock.
- White flashes in the window, the update loop when a helper kept RGBable open, and the screen preview after an update.

See [Troubleshooting](https://github.com/boywholifts/RGBable/blob/windows-port/docs/rgbable/troubleshooting.md) if you're coming from L-Connect, SignalRGB or another RGB app: only one program, and one copy of OpenRGB, should control each light.

## [0.2.0] - 2026-11-07

### Added

- In-app updates from GitHub Releases, with Stable and Dev channels in Settings.
- Windows installer (NSIS) and automatic builds on GitHub.
- The app version is shown in Settings > About.

## [0.1.0] - 2026-11-06

### Added

- First Windows build: Lian Li wireless fans and Strimers, HydroShift OLED ring and screen (videos, GIFs and images), pump, coolant and fan readings.
- RAM, GPU, motherboard and other lighting through OpenRGB running in the background.
- Sync everything color, device renaming, tray icon and start with Windows.
