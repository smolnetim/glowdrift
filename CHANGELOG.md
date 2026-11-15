# Changelog

All notable changes to Glowdrift are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

GitHub Releases additionally carry notes generated from the Conventional Commit history of each release.

## [Unreleased]

## [0.2.0] - 2026-11-15

### Added

- Philips Hue light through the bridge's local API: pair once with the bridge's link button, then drive each Hue light from its own screen zone. Zones are chosen on a monitor thumbnail and shown as live light tiles.
- Region editor improvements: a display picker, region presets and numeric region fields, also for the iCUE light.
- Installer folder page on first interactive install, with an editable path, the Windows folder picker and inline validation.
- Install mode choice: all users (`Program Files`, asks for administrator permission) or only you (`%LOCALAPPDATA%\Programs`). Updates keep the existing mode.
- Updating an all-users install asks for Windows permission before the engine stops, so a declined prompt leaves the app running and the update staged.
- Pull request builds publish an unsigned setup file and a sticky comment with the download link.

### Changed

- The smoothing rate follows the fastest enabled light (up to 60 Hz), or 20 Hz while the preview is on, and the smoother idles when no light is active.
- The engine and iCUE light use much less CPU: fewer per-frame allocations and copies in capture, reduction, colour publishing and status reporting, and one batched request per update for all iCUE devices.
- **Breaking:** Output is now Light throughout the app, and "raw" colour is now "target" colour. The saved config key `outputs` is now `lights`, with no migration, so any light settings saved under the old key are ignored. The IPC command, event and payload names changed accordingly.

## [0.1.0] - 2026-11-12

### Added

- Project scaffold: Rust workspace (`glowdrift-core`, `glowdrift-capture`, `glowdrift-lights`) and a Tauri v2 desktop app with a React and TypeScript frontend.
- Continuous integration, security scanning, and a tag-driven release pipeline with signed updater artifacts.
