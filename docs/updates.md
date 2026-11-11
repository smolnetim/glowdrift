# Update policy

How Glowdrift ships updates, how they are verified, and what that means for where releases are hosted.

## Summary

- One channel: **stable**, published as GitHub Releases. Pre-release tags (for example `v0.3.0-rc.1`) are
  published as GitHub pre-releases and are never offered to installed copies.
- The installer and the update payload are the same NSIS setup file. Every update payload carries a **minisign
  signature** that the app verifies against a public key compiled into it.
- Updates download silently in the background and are applied when the app restarts.
- The app installs **per user**, so neither installing nor updating needs administrator rights.
- To roll back, install an older installer over the current one.

## How the built-in updater works

The app uses the official Tauri updater plugin (`tauri-plugin-updater`).

1. When it checks for updates, the app requests the static manifest
   `https://github.com/smolnetim/glowdrift/releases/latest/download/latest.json`. GitHub redirects
   `releases/latest` to the newest non-pre-release, non-draft release.
2. `latest.json` lists, per platform, the version, a download URL and the contents of the matching `.sig` file.
3. If the version is newer than the running one, the app downloads the installer and verifies its minisign
   signature against the embedded public key. A file that fails verification is discarded.
4. The new version is applied on the next restart. The Windows install mode is `passive`: a small progress window,
   no questions asked.
5. `autoUpdate` in the config turns the background check off.

The updater and installer settings live in `apps/desktop/src-tauri/tauri.conf.json` (`plugins.updater` and
`bundle`). The frontend drives the check through `@tauri-apps/plugin-updater` and restarts through
`@tauri-apps/plugin-process`. Nothing about the user or the PC is sent beyond what an ordinary HTTPS request to
GitHub contains, because the endpoint is a plain static file.

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
