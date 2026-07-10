# Agent Instructions

## Project Scope

Music Companion is a Windows 10/11 desktop lyrics overlay built with Tauri 2,
TypeScript, Rust, Bun, and Vite+. Preserve Windows-only behavior and the current
Tauri/frontend boundary unless a task explicitly changes them.

## Repository Navigation

- `src/main.ts`: overlay UI, settings, lyric parsing, and synchronization.
- `src/styles.css`: overlay and settings presentation.
- `src-tauri/src/lib.rs`: WMTC integration, LRCLIB access, tray behavior, updater,
  and Tauri commands.
- `src-tauri/tauri.conf.json`: windows, NSIS packaging, and updater configuration.
- `.github/workflows/`: pull-request CI and approved tagged releases.

## Setup and Development

```powershell
bun install --frozen-lockfile
bun run tauri:dev
```

Use Bun for frontend dependencies and scripts. Use the stable Rust MSVC toolchain
for native code. Do not introduce another package manager or commit generated
`dist/`, `node_modules/`, or `src-tauri/target/` output.

## Validation

Run every check relevant to the changed area. Before opening or updating a pull
request, run the complete suite when practical:

```powershell
bun install --frozen-lockfile
bun run check
bun run lint
bun run format:check
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
bun run build
bun audit
cd src-tauri
cargo audit
```

For local formatting:

```powershell
bun run format
cargo fmt --manifest-path src-tauri/Cargo.toml
```

A change is complete when applicable checks pass, user-visible behavior is
documented, and no placeholder, credential, unrelated generated file, or
unexplained behavior change remains.

## Change and Approval Boundaries

- Work on focused feature branches and keep diffs reviewable.
- Preserve existing behavior unless the request explicitly changes it.
- Discuss broad features, architecture changes, and migrations before implementation.
- Obtain approval before adding or replacing dependencies; explain maintenance,
  security, size, and compatibility tradeoffs.
- Obtain approval before destructive data or Git operations, credential changes,
  publishing, releases, or other external side effects.
- Never commit signing keys, tokens, passwords, or other secret values.
- CI runs only for non-draft pull requests and exposes one stable required gate.
  Do not add push, schedule, issue, discussion, or deployment triggers to CI.

## Release Policy

Releases are approved GitHub releases built from tags matching `v*`. Release
artifacts are signed Windows x86-64 NSIS installers and Tauri updater metadata.
When explicitly asked to prepare or perform a release, apply the version bump on
`main` and tag that commit; do not create a feature branch for release work.
