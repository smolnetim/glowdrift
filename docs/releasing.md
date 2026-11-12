# Releasing

How the release pipeline is set up and how to cut a release. For the update model and key handling see
[updates.md](updates.md).

## One-time setup

### 1. Create the repository

Create `smolnetim/glowdrift` on GitHub and push `main`. The default update endpoint
(`https://github.com/smolnetim/glowdrift/releases/latest/download/latest.json`) only works when release assets are
publicly downloadable; see [updates.md](updates.md) for the options if the repository stays private.

### 2. Create the `release` environment and add secrets

The release job runs in a GitHub Environment named `release` (`environment: release` in `release.yml`). It is
what lets you gate the signing secrets behind approvals and tag rules. The workflow also runs when the
environment has no protection rules, and GitHub creates an unprotected `release` environment automatically the
first time the job references it, but create it yourself to configure it:

1. Settings, Environments, New environment, name it `release`.
2. Optional: **Required reviewers**, so a release job waits for approval before it receives any secret.
3. Optional: **Deployment branches and tags**, select "Selected branches and tags" and add a tag rule `v*`, so
   only `v*` tags can use the environment. A manual run (`workflow_dispatch`) uses the ref picked under "Use
   workflow from"; pick the tag itself, or it will be refused by this rule.
4. Put the secrets below on the environment (Environment secrets) to keep them away from other workflows. Secrets
   that already exist as repository secrets, such as `TAURI_SIGNING_PRIVATE_KEY`, keep working with the
   environment and can stay there; an environment secret with the same name takes precedence.

Secrets (repository or environment scope, Settings, Secrets and variables, Actions):

| Secret | Required | Value |
| --- | --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | Yes | Contents of the updater private key file (or its path on the runner; the contents are simplest). |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | If the key has a password | The key's password. May be left unset or empty for a key without one. |
| `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID` | Optional | Service principal for Azure Artifact Signing. |
| `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE` | Optional | Endpoint URL, account name and certificate profile name of the signing account. |

The public key matching the private key is already in `apps/desktop/src-tauri/tauri.conf.json`. If you generate a
new key pair, replace it there too (and read the rotation notes in [updates.md](updates.md) first).

The signing key and password are passed only to the `tauri-action` build step. The Azure service principal values
are passed only to the steps that need them (the endpoint, account and profile when the signing command is
written; client ID, secret and tenant ID to the build step, and only when all six Azure secrets are set). Other
steps (`npm ci`, `cargo install`, release notes) never see them.

`GITHUB_TOKEN` is provided automatically. The release job asks for `contents: write`, `id-token: write` and
`attestations: write` itself; no workflow-level write permission is granted elsewhere.

### 3. Repository settings

- **Actions, General, Workflow permissions**: read repository contents by default. The workflows declare what
  they need.
- **Dependabot**: enable Dependabot alerts and Dependabot security updates (Settings, Advanced Security). The
  version updates are configured in `.github/dependabot.yml`.
- **Secret scanning and push protection**: enable both (available for free on public repositories; private
  repositories need GitHub Secret Protection).
- **Private vulnerability reporting**: Settings, Advanced Security, enable "Private vulnerability reporting". It is
  what [SECURITY.md](../SECURITY.md) points reporters to.
- **Branch protection (or a ruleset) for `main`**, suggested:
  - require a pull request before merging;
  - require the `Build and test (Windows)` check from CI and the `cargo-deny`, `npm audit` and `Secret scan` checks
    from Security;
  - require branches to be up to date and linear history;
  - block force pushes and deletions;
  - restrict who can create `v*` tags with a tag ruleset, so only maintainers can trigger a release.

### 4. Features that depend on repository visibility

| Feature | Private repository | Public repository |
| --- | --- | --- |
| CodeQL code scanning | Needs GitHub Code Security | Free |
| Dependency review action | Needs GitHub Code Security | Free |
| Build provenance attestations | Needs GitHub Enterprise Cloud | Free |
| Secret scanning | Needs GitHub Secret Protection | Free |
| Built-in updater download | Does not work without credentials | Works |

The workflows skip CodeQL, dependency review and the attestation step unless
`github.event.repository.private == false`, so they do not fail on a private repository. The remaining jobs
(cargo-deny, npm audit, gitleaks, CI) run everywhere. gitleaks runs as a checksum-verified, pinned CLI release
over the full git history, so it needs no licence.

## Workflows

| File | Trigger | Does |
| --- | --- | --- |
| `.github/workflows/ci.yml` | Push to `main`, pull requests | Frontend install, typecheck, optional lint and build; `cargo fmt --check`; clippy with `-D warnings`; tests; a debug `tauri build --no-bundle` to validate linking. |
| `.github/workflows/security.yml` | Push to `main`, pull requests, weekly, manual | cargo-deny, npm audit (production dependencies, high and above), dependency review, CodeQL (JavaScript/TypeScript, Rust, Actions), gitleaks (full history). |
| `.github/workflows/release.yml` | Tag `v*.*.*`, manual | Verify, build, sign updater artifacts, publish the release, attest. |

Third-party actions are pinned to full commit SHAs with the version in a trailing comment. Dependabot proposes
updates to the pins.

## What the release workflow does

1. Checks out the tag with full history.
2. Validates the tag format, runs `scripts/check-version.ps1` to confirm the tag equals the version in
   `Cargo.toml`, `apps/desktop/package.json`, `Cargo.lock` and (if present) `tauri.conf.json`, and fails early if
   `TAURI_SIGNING_PRIVATE_KEY` is not set.
3. Generates release notes from the Conventional Commits since the previous `v*.*.*` tag with
   `scripts/release-notes.ps1`.
4. Optionally prepares Authenticode signing (see below).
5. Builds the NSIS installer with `tauri-apps/tauri-action`, which signs the updater artifact, creates the GitHub
   Release, uploads the installer, its `.sig` and `latest.json`.
6. On public repositories, creates a build provenance attestation for the installer and signature.

Tags containing a hyphen (`v0.3.0-rc.1`) are published as pre-releases. The manual trigger takes an existing tag
and rebuilds and republishes it.

### Optional Authenticode signing

Authenticode signing removes the SmartScreen "unknown publisher" experience once the certificate has reputation.
It is optional and skipped when any of the six Azure secrets is missing. The workflow step is untested against a
real account.

How it plugs in: Tauri runs a `signCommand` for the app executable and the installer during the build, so the
updater signature is computed over the already signed installer. The workflow writes a small config override
(`tauri.sign.conf.json`) that sets `bundle.windows.signCommand` to `artifact-signing-cli ... %1` and passes it to
the build.

Options:

- **Azure Artifact Signing** (formerly Azure Trusted Signing): a managed service with short-lived certificates.
  Availability depends on your region and on identity validation. This is what the workflow step supports. Create
  the account and certificate profile in Azure, grant a service principal the signing role, and add the secrets
  above.
- **SignPath** (including its programme for open-source projects): signs through a SignPath project and policy
  and typically needs a different integration, such as signing the built installer as a separate step. Because
  that would invalidate an updater signature computed earlier, sign first and then compute the updater signature
  with `tauri signer sign`, or use a `signCommand` that calls SignPath.
- **A traditional OV or EV certificate**: set `bundle.windows.certificateThumbprint`, `digestAlgorithm` and
  `timestampUrl` in the config, and import the certificate on the runner before the build. Check the current
  rules for private-key storage on hardware tokens or HSMs before buying.

See [Tauri's Windows signing guide](https://v2.tauri.app/distribute/sign/windows/) for the exact configuration.

## Release checklist

1. Make sure `main` is green (CI and Security).
2. Decide the version from the commits since the last release (`feat` means minor, `fix` means patch, breaking
   change means major; below 1.0 breaking changes may go in a minor).
3. Review `CHANGELOG.md`: move or write entries under `Unreleased`.
4. Run the bump script from the repository root:

   ```powershell
   ./scripts/bump-version.ps1 0.2.0
   ```

   It updates every version file, refreshes `Cargo.lock`, moves the `Unreleased` changelog entries under the new
   version, and prints the git commands.
5. Verify locally:

   ```powershell
   ./scripts/check-version.ps1
   cargo test --workspace
   ```

6. Commit, tag and push as printed by the script:

   ```powershell
   git add Cargo.toml Cargo.lock CHANGELOG.md apps/desktop/package.json apps/desktop/package-lock.json
   git commit -m "chore(release): v0.2.0"
   git tag -a v0.2.0 -m "Glowdrift v0.2.0"
   git push origin main v0.2.0
   ```

7. Watch the **Release** workflow. When it finishes, open the release and confirm it contains the `-setup.exe`,
   the `.sig` and `latest.json`.
8. Check that `https://github.com/smolnetim/glowdrift/releases/latest/download/latest.json` serves the new
   version.
9. Install the previous version on a test machine, let it update (or check manually), and confirm the app starts,
   the tray icon appears and the lights respond.
10. If something is wrong, follow the rollback notes in [updates.md](updates.md).
