# RGBable changelog

Versions follow [Semantic Versioning](https://semver.org). Dev builds (`x.y.z-dev.N`) are untested pre-releases for the Dev update channel.

## [Unreleased]

### Added

- Device groups: create, rename and delete groups, mix Lian Li and OpenRGB devices, colour a whole group, drag devices between groups.
- Identify button that blinks a device white.
- First-run setup wizard that walks through every light (Settings > Run setup again).
- Per-device switch to leave a device out of Sync everything.
- Lighting effects: breathing, colour cycle, rainbow, rainbow wave, wave, meteor and twinkle, with colours, speed, brightness and direction, for everything, a group or one device. Wireless fans and Strimers play effects by themselves; the OLED ring and OpenRGB devices are animated by RGBable.
- RGBable can run OpenRGB for you in the background as administrator (Settings > OpenRGB), install the PawnIO driver RAM lighting needs, and turn off OpenRGB's own start-at-login.
- LED count for addressable motherboard headers (e.g. case ARGB strips), remembered and re-applied.
- A hint on the Lighting page when OpenRGB finds no RAM.
- 10 more effects: strobe, heartbeat, fire, scanner, runway, fill, ripple, starry night, aurora and police.
- SL Flex fans are drawn on their real layout (13 positions on each of the inner and outer rings) and Strimers along their rows; effects can target the inner ring, the outer ring or both.
- L-Connect's own effects for Lian Li fans and Strimers (rainbow morph, staggered, tide, mixing, ping-pong, collide, reflect, hourglass, shuttle run and more), rendered by lian-li-linux's Lian Li renderer.

### Fixed

- SL Flex effects were spread over 52 slots per fan, so patterns ran across the wrong fans and half of each upload went to slots with no LEDs.
- Large effects (fire, twinkle, aurora) never reached wireless fans and Strimers, and switching effects sometimes left a device frozen on the old one: effect loops are now kept to 6 KB, and a device that doesn't confirm an effect is sent a smaller version until it does.
- Device and group colour boxes showed the last plain colour while an effect was running; they now show the effect.
- Update now failed with "Windows cannot find '\\'"; the installer is now started through a batch file and RGBable reopens from its install folder.
- Apply to all sometimes left wireless fans or Strimers unchanged: uploads are now confirmed and resent, and devices are taken out of motherboard-sync mode first.
- One OpenRGB error no longer stops Apply to all for the remaining OpenRGB devices.

## [0.2.0] - 2026-11-07

### Added

- In-app updates from GitHub Releases, with Stable and Dev channels in Settings.
- Windows installer (NSIS) and automatic builds on GitHub.
- The app version is shown in Settings > About.

## [0.1.0] - 2026-11-06

### Added

- First Windows build: Lian Li wireless fans and Strimers, HydroShift OLED ring and screen (videos, GIFs and images), pump, coolant and fan readings.
- RAM, GPU, motherboard and other lighting through OpenRGB running in the background.
- Sync everything colour, device renaming, tray icon and start with Windows.
