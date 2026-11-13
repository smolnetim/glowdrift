# Update policy

How Glowdrift ships updates, how they are verified, and what that means for where releases are hosted.

## Summary

- One channel: **stable**, published as GitHub Releases. Pre-release tags (for example `v0.3.0-rc.1`) are
  published as GitHub pre-releases and are never offered to installed copies.
- The installer and the update payload are the same NSIS setup file. Every update payload carries a **minisign
  signature** that the app verifies against a public key compiled into it.
- Updates download silently in the background and are applied when the app restarts.
- The app installs **per user**, by default in `%LOCALAPPDATA%\Programs\T1M\Glowdrift` (a first interactive install
  offers a folder choice), so neither installing nor updating needs administrator rights or shows a prompt. See
  [Installation and updates](#installation-and-updates).
- To roll back, install an older installer over the current one.

## How the built-in updater works

The app uses the official Tauri updater plugin (`tauri-plugin-updater`).

1. When it checks for updates, the app requests the static manifest
   `https://github.com/smolnetim/glowdrift/releases/latest/download/latest.json`. GitHub redirects
   `releases/latest` to the newest non-pre-release, non-draft release.
2. `latest.json` lists, per platform, the version, a download URL and the contents of the matching `.sig` file.
3. If the version is newer than the running one, the app downloads the installer and verifies its minisign
   signature against the embedded public key. A file that fails verification is discarded.
4. The new version is applied on the next restart. The Windows install mode is `passive`: the installer shows a
   small progress window and asks no questions.
5. `autoUpdate` in the config turns the background check off.

The updater and installer settings live in `apps/desktop/src-tauri/tauri.conf.json` (`plugins.updater` and
`bundle`). The frontend drives the check through `@tauri-apps/plugin-updater` and restarts through
`@tauri-apps/plugin-process`. Nothing about the user or the PC is sent beyond what an ordinary HTTPS request to
GitHub contains, because the endpoint is a plain static file.

## Installation and updates

The app installs **per user** and never asks for administrator rights, for the first install or for updates.
The NSIS installer is configured with install mode `currentUser` (`bundle.windows.nsis.installMode`), which makes
its manifest request the invoking user's own rights (`RequestExecutionLevel user`), so Windows shows no UAC
prompt on administrator and standard accounts alike. It uses a copy of the bundler's installer template
(`apps/desktop/src-tauri/installer/installer.nsi`, with the changes listed at the top of that file) and hooks
(`installer/hooks.nsh`).

- **Default folder.** `%LOCALAPPDATA%\Programs\T1M\Glowdrift`, with registry data under `HKCU`. The path is built
  from the user's "Programs" known folder, falling back to `%LOCALAPPDATA%\Programs` if that lookup fails.
- **Folder page (first install only).** An interactive first install shows a page for the install folder, with an
  editable path field (typing and pasting work), a **Browse...** button that opens the standard Windows folder
  picker, and the required and free space of the target drive. The page is skipped for silent (`/S`) and passive
  (`/P`) installs, for `/UPDATE`, when `/D=` is given, and whenever an installation already exists. An existing
  installation is detected once at startup by its Uninstall entry in `HKCU`, so the reinstall page cannot cause the
  folder page to appear. If a remembered folder exists, the page starts with it; otherwise with the default.
  - Browse treats the picked folder as a parent and shows `<picked>\Glowdrift`, unless the picked folder is an
    existing empty folder or already ends in `Glowdrift`, in which case it is used as is. A drive root such as
    `D:\` becomes `D:\Glowdrift`. Typed text is used exactly as written.
  - The path is validated when the field loses focus, after Browse and when **Next** is pressed, with an inline
    message under the field (never a dialog). **Back** and **Cancel** are never blocked. **Next** stays enabled and
    stays on the page while the path is invalid.
  - Rejected: relative paths, UNC and network paths, paths with environment variables, characters Windows does not
    allow in paths, a drive root alone, anything under Program Files or the Windows folder (they need
    administrator rights), network drives, folders that cannot be written without elevation, and drives with too
    little free space. A folder that does not exist yet is accepted and created during the install.
  - The messages are `LangString`s, so they can be translated.
- **Location is reused.** The installer stores the install folder in the default value of `Software\T1M\Glowdrift`
  in `HKCU`. An update or a reinstall reads it and installs there without showing the folder page, so an
  installation is updated where it is and is never moved.
- **Moving an install** means uninstalling and installing again; there is no "change folder" action for an existing
  installation. Uninstall first, then run the setup file again and pick the new folder on the folder page.
- **Start menu.** One `Glowdrift` shortcut directly in the user's Start menu Programs folder. `/NS` skips it.
- **What the updater runs.** It starts the downloaded setup file with `ShellExecuteW("open", ...)` and the
  arguments `/P /UPDATE` (passive mode: a small progress window, no questions), then exits the app. Because the
  installer needs no elevation, the update runs without a prompt.
- **A running instance is asked to quit first.** Before replacing or removing files, the installer starts
  `glowdrift-desktop.exe --quit`, which makes the running instance stop the engine, release the lights and exit,
  then waits at most 10 seconds for the executable to be released. If the app is not running, nothing is started.
  If it is still running after the wait, the bundler's standard running-app check takes over (a prompt, or in
  silent and passive modes a forced close through the Windows Restart Manager).
- **Autostart entry.** The app writes its autostart entry itself, as the value `Glowdrift` under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (path to the executable plus `--minimized`). On every
  install the installer rewrites that value, if it exists, to the current install folder's executable and keeps the
  original arguments, so the entry keeps working after a reinstall into a different folder. It adds nothing if the
  value is absent. The entry is removed on uninstall, not during an update.
- **Uninstall.** Removes the shortcut, the files and the empty `T1M` folder above the install folder. Every real
  uninstall also removes the remembered install folder (`Software\T1M\Glowdrift`, and `Software\T1M` if it is then
  empty), so the next install starts from the default again. During an update the remembered folder and the
  autostart entry are kept.
- **Delete app data.** The uninstaller offers an opt-in checkbox, unchecked by default. Only when it is checked are
  the app data folders `%APPDATA%\gg.t1m.glowdrift` and `%LOCALAPPDATA%\gg.t1m.glowdrift` deleted. Otherwise app data
  survives an uninstall and a later install picks it up.
- **Repair.** Windows Settings offers no Modify or Repair (`NoModify` and `NoRepair` are set). To repair, run the
  setup file again over the installation; it reinstalls into the same folder.

### Setup file switches

| Switch | Effect |
| --- | --- |
| `/S` | Silent: no UI, no questions. |
| `/P` | Passive: progress window only, no questions. The updater uses `/P /UPDATE`. |
| `/UPDATE` | Marks the run as an update: keeps the remembered folder and the autostart entry. |
| `/NS` | Do not create the Start menu shortcut. |
| `/R` | Restart the app after a silent or passive install. |
| `/ARGS <arguments>` | Command-line arguments for the app started by `/R`. |
| `/D=<folder>` | Install folder. Must be the last argument and must not be quoted, even if the path contains spaces. Skips the folder page. |

The installer's behaviour is verified by the installer smoke test in CI (silent install into a temporary folder,
update over it, uninstall; files and registry keys are asserted after each step) plus manual checks of the
interactive pages, which the smoke test cannot drive.

## Signing keys

Updater signatures are independent from Windows code signing.

| | Updater signature (minisign) | Authenticode signature |
| --- | --- | --- |
| Purpose | The app trusts update payloads | Windows SmartScreen and Smart App Control reputation |
| Required | Yes, the updater refuses unsigned payloads | No, optional, off until a certificate or signing service is configured |
| Verified by | Glowdrift itself | Windows |

**Private key.** Generated with `npm run tauri -- signer generate -w <path>`. It is stored as the GitHub Actions
secret `TAURI_SIGNING_PRIVATE_KEY` (and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if the key has a password). It must
never be committed; `*.key` is in `.gitignore`. Keep an offline backup: if the private key is lost, installed
copies can no longer be updated by the built-in updater, and users would have to install a new version manually.

**Public key.** Stored in `apps/desktop/src-tauri/tauri.conf.json` under `plugins.updater.pubkey`. It is not a
secret.

### Rotating the key

A rotation has to be bridged, because installed copies only trust the key they were built with. A sound approach
(not yet exercised):

1. Generate a new key pair. Keep the old private key available, and keep the old Actions secret in place for now.
2. Publish a **bridge release** whose app contains the *new* public key but whose update payload is signed with the
   *old* private key. Installed copies accept it, because they verify against the old key.
3. Replace the Actions secret with the new private key. Every release from now on is signed with the new key and
   accepted by copies that have installed the bridge release.
4. Copies that skipped the bridge release can no longer auto-update and need a manual install. Keep the bridge
   release available long enough for most users to pass through it.

The Tauri updater also allows setting the public key at run time through the plugin builder, which overrides the
configuration file; that could be used to carry more than one key, but it is not used today.

If the private key is ever exposed, rotate immediately, publish a bridge release signed with the old key if you
still control it, and tell users to reinstall manually if you do not.

## Rolling back

There is no in-app downgrade, because the updater only moves forward. To go back, download the older installer
from the Releases page and run it over the current install. Per-user data (the config file) is kept, and older
versions ignore fields they do not know.

If a release is bad, the fastest mitigation for everyone is to publish a fixed higher version, because
`releases/latest` always serves the newest release. Marking a bad release as a pre-release or deleting it makes
`latest.json` point at the previous release again. Installed copies already on the bad version will not downgrade
to it.

## Why the releases must be publicly reachable

The updater requests `latest.json` and the installer without credentials. GitHub serves release assets of a
**private** repository only to authenticated users, so with a private repository the request returns 404 and
every update check fails. The built-in updater therefore needs the release assets to be publicly downloadable.

Options if the source repository stays private:

1. **Releases-only public repository.** Create a small public repository (for example `glowdrift-releases`) that
   holds only releases. The release workflow publishes the assets there (for example by creating the release in the
   public repository with a token that has access to it), and `plugins.updater.endpoints` points at that
   repository's `releases/latest/download/latest.json`. Source stays private; binaries are public.
2. **Static endpoint.** Publish `latest.json` and the installers to a token-less static host such as GitHub Pages
   (which serves public content even from a private repository on plans that allow it), object storage, or any
   HTTPS host, and point `endpoints` at it. The `url` fields inside `latest.json` must point to publicly reachable
   files.
3. **Make the repository public.** The simplest option, and the one the default configuration assumes.

Both alternatives need a change to `plugins.updater.endpoints` and to the release workflow. Embedding a token in
the app to reach private releases is not an option: anything inside the app can be extracted.

Note that other security features are also tied to repository visibility. See [releasing.md](releasing.md).

## Versioning

- **Semantic Versioning** (`MAJOR.MINOR.PATCH`). While the version is below 1.0, minor versions may contain
  breaking changes.
- **Conventional Commits** drive the release notes (`feat` for features, `fix` for fixes, `!` or a
  `BREAKING CHANGE:` footer for breaking changes).
- The version lives in one place per tool, kept in sync by `scripts/bump-version.ps1` and checked by
  `scripts/check-version.ps1`, which the release workflow runs before building. The tag must be `v` plus that
  version.

## Configuration compatibility promise

- A newer version always loads configuration files written by older versions: missing fields take defaults.
- Older versions ignore fields they do not know, so a rollback does not break on a newer file.
- A change that renames, removes or reinterprets a setting increments `schemaVersion` and ships with a migration.
  Migrations only run forward; the previous file is not rewritten until the app saves a new one.
- A configuration file that cannot be read is copied to `config.json.bak` and replaced by defaults, so an
  update never leaves the app unable to start.
