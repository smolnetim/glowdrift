# Stay on NSIS and offer a folder choice on first install

Status: accepted. Supersedes the earlier decision to remove the install folder page from the NSIS template.

## Context

The Tauri updater plugin accepts NSIS and MSI payloads but not MSIX. Glowdrift installs per user (`currentUser`),
so neither installing nor updating shows a UAC prompt. We have invested in a copy of the bundler's NSIS template,
installer hooks and a quit-first step that asks a running instance to exit before files are replaced.

The folder page was removed earlier because the stock page opens the legacy tree-style folder dialog, which is
awkward and does not match Windows 11. Without a page, people had no way to choose where the app goes.

## Decision

Stay on NSIS with our template, per-user only, and add a custom folder page:

- It appears only on a first interactive install: not with `/S`, `/P`, `/UPDATE` or `/D=`, and not when an
  installation already exists. Updates and reinstalls always reuse the remembered folder.
- **Browse...** opens the modern `IFileDialog` folder picker, implemented in the installer script with the NSIS
  `System` plugin and `Win\COM.nsh`. No third-party plugin DLL is added.
- The typed or picked path is validated (absolute local path, no network drive or UNC, no Program Files or Windows
  folder, writable without elevation, enough space) with an inline message.

## Considered options

- **WiX / MSI.** Supported by the updater, but it needs a second toolchain, a rewrite of the hooks and the
  quit-first behaviour, and per-user MSI packaging with a custom folder UI is no simpler.
- **MSIX.** Not supported by the updater plugin, so in-app updates would be lost.
- **Third-party folder picker plugin DLL.** Would give the same dialog, but adds an unsigned binary to the
  build that has to be vetted and kept compatible with the NSIS toolchain Tauri downloads.
- **Helper executable for the picker.** Another artifact to build, ship and sign, and a process boundary to
  handle failures across, for one dialog.

## Consequences

- The template diff to maintain on Tauri upgrades stays small: the page lives in a separate include and in the
  hooks, and the template only gains the lines that wire them in.
- Moving an existing install means uninstalling and installing again; there is no "change folder" action.
- The folder page logic (picker, validation) is our own code and is covered by manual checks, while the install,
  update and uninstall flow is covered by the installer smoke test in CI.
