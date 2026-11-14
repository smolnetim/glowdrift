# Let the user choose between all users and only me

Status: accepted. Amends [0001](0001-nsis-with-first-install-folder-choice.md), which fixed the installer to per-user.

## Context

A per-user install under `%LOCALAPPDATA%\Programs` needs no administrator rights, but people expect the usual
`C:\Program Files` location, and some want one installation for every account. Program Files needs elevation to
write, so updates of such an installation need elevation too. Accepting one permission prompt per update is fine,
and a click-to-install update notice makes the prompt predictable.

## Decision

Stay on NSIS with our template and install mode `both`:

- The first interactive install asks: **all users** (preselected, `C:\Program Files\T1M\Glowdrift`, `HKLM`) or
  **only me** (`%LOCALAPPDATA%\Programs\T1M\Glowdrift`, `HKCU`).
- The setup file starts unelevated and requests elevation only for all users, by starting itself again with the
  `runas` verb. Per-user installs and updates never prompt.
- An existing installation decides the mode, so updates and reinstalls never change it. Switching modes means
  uninstalling first; a conflicting `/allusers` or `/currentuser` is refused.
- The app installs updates only after a click. For an installation under Program Files it starts the verified setup
  file elevated before stopping anything, so a declined prompt leaves the app running and the update staged.

## Considered options

- **Elevate at launch (`MULTIUSER_EXECUTIONLEVEL Highest`, as upstream).** Shows a prompt to administrators for
  per-user installs as well. Rejected.
- **Per-machine only.** Every install and update prompts, with no way to avoid it. Rejected in favour of a choice.
- **Update service running as SYSTEM.** Would allow prompt-free updates of Program Files, but adds a permanent
  privileged component that must verify payloads itself. Not worth it while one prompt per update is acceptable.

## Consequences

- The installer carries its own mode detection and elevation logic (`install-mode.nsh`), because the bundled
  `MultiUser.nsh` can only elevate at launch.
- The autostart entry and the opt-in app data deletion act on the account the setup or uninstaller runs as.
- The all-users code paths (real elevation, the common Start menu, uninstall from the temp copy) are covered by the
  all-users pass of the installer smoke test in CI and by manual checks, not by local runs without elevation.
