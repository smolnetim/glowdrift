# Contributing to Glowdrift

Thanks for helping. This guide covers the toolchain, the commands to run, and what a pull request needs.

## Toolchain

Windows 11 only.

- Rust stable with the MSVC toolchain (`rust-toolchain.toml` selects stable with `rustfmt` and `clippy`).
- Visual Studio 2022 Build Tools with the "Desktop development with C++" workload and a Windows 10/11 SDK.
- Node.js 24 or newer and npm.
- The WebView2 runtime (included with Windows 11).

## Layout

`crates/glowdrift-core` is platform independent and holds the colour maths, reduction, smoothing and config
schema. `crates/glowdrift-capture` and `crates/glowdrift-outputs` are the Windows-facing crates. `apps/desktop` is
the Tauri v2 app (`src/` is React and TypeScript, `src-tauri/` is the Rust shell). See
[docs/architecture.md](docs/architecture.md).

## Commands

From the repository root:

```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check            # optional locally; CI runs it
```

Frontend, from `apps/desktop`:

```powershell
npm ci
npm run typecheck
npm run build
npm run tauri -- dev        # run the app with hot reload
```

The Tauri crate embeds the built frontend at compile time, so run `npm run build` once before the first
`cargo clippy` or `cargo test` that includes `glowdrift-desktop`.

Keep the config schema, the IPC contract in `apps/desktop/src/lib/ipc.ts` and the Rust types in sync. Add unit
tests for new pure logic, especially in `glowdrift-core`.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/). Release notes are generated from them.

```
feat(capture): select a monitor by id
fix(core): clamp saturation after conversion
docs: describe the update policy
feat(core)!: rename the smoothing field
```

Common types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`. Mark breaking changes
with `!` or a `BREAKING CHANGE:` footer.

## Pull requests

CI must pass before merging. It checks:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- the frontend typecheck and build (and lint, if the project defines a `lint` script)
- a debug build of the desktop app, to validate linking

The Security workflow also runs `cargo deny` (advisories, licences, bans, sources), `npm audit` for production
dependencies, and a secret scan. On a public repository it adds CodeQL and a dependency review of the PR.

Keep pull requests focused. Describe what changed and why, and how you tested it. Do not commit secrets or the
updater private key.

Code comments should describe the code in the file. Do not put project history or who asked for a change in
comments.

## Dependencies

Prefer current stable releases. Dependabot opens weekly grouped updates. New dependencies must pass the licence
policy in `deny.toml` (permissive licences, MPL-2.0 allowed).

## Releases

Maintainers cut releases as described in [docs/releasing.md](docs/releasing.md).

## Licence

By contributing you agree that your contribution is licensed under the [MIT licence](LICENSE).
