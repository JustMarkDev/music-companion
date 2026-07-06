# Repository Instructions for Coding Agents

## Project scope

Music Companion is a Windows-only Tauri 2 desktop lyrics overlay. It reads
Windows Media Transport Controls (WMTC), retrieves lyrics from LRCLIB, and
renders synchronized lyrics in a TypeScript frontend.

Do not introduce support claims for other operating systems unless the
underlying implementation and validation are part of the requested change.

## Repository map

- `src/main.ts` — frontend behavior, settings, lyric parsing, synchronization,
  and Tauri IPC usage.
- `src/styles.css` — overlay and settings presentation.
- `src-tauri/src/lib.rs` — media integration, lyrics lookup, tray behavior,
  updater, commands, and Rust unit tests.
- `src-tauri/src/main.rs` — native application entry point.
- `src-tauri/tauri.conf.json` — application windows, bundling, and updater
  configuration.
- `.github/workflows/release.yml` — tagged Windows release publishing.
- `README.md` — user and developer documentation.
- `CONTRIBUTING.md` — human contribution workflow.

## Setup and commands

Use the repository's npm and Cargo manifests:

```powershell
npm ci
npm run tauri:dev
npm run check
cargo test --manifest-path src-tauri/Cargo.toml
npm run build
npm run preview
npm run tauri:build
```

Apply validation proportionally:

- Run `npm run check` for code changes.
- Run `cargo test --manifest-path src-tauri/Cargo.toml` for Rust changes and
  before handoff when the environment supports it.
- Run `npm run build` for frontend production changes.
- Run `npm run tauri:build` for packaging, installer, updater, release, native
  dependency, or Tauri build-system changes.
- Record checks not run and any Windows-specific validation limitations.

## Change discipline

- Keep changes scoped to the request and preserve unrelated user work.
- Inspect the working tree before editing; do not overwrite existing changes.
- Avoid unrelated formatting, refactoring, or dependency updates.
- Do not commit `node_modules/`, `dist/`, `src-tauri/target/`, logs, caches, or
  other generated output.
- Avoid new dependencies unless they are necessary for the requested outcome.
  When dependencies change, update the appropriate manifest and lockfile
  together.
- Update documentation when user-visible behavior, setup, commands,
  configuration, or contributor workflow changes.

## Compatibility constraints

- Keep versions synchronized across `package.json`, `src-tauri/Cargo.toml`, and
  `src-tauri/tauri.conf.json`.
- Preserve camelCase frontend contracts backed by Rust
  `#[serde(rename_all = "camelCase")]` types.
- Treat Tauri commands, command arguments, events, window labels, and persisted
  settings as compatibility-sensitive interfaces.
- Preserve the Windows-only compile constraint and WMTC behavior unless the
  task explicitly changes platform support.
- Do not weaken Tauri capabilities or security configuration without explaining
  and validating the impact.

## Security and releases

- When triggering a GitHub Actions release workflow, verify only that the
  workflow has started successfully. Do not wait for it to finish; end the task
  once the run is in progress.
- Never expose or commit updater private keys, signing material, GitHub secrets,
  tokens, or credentials.
- Treat updater public-key and endpoint changes as release-critical.
- Do not change release tags, installer targets, update behavior, or signing
  configuration incidentally.
- A private signing key must never be derived from, replaced by, or stored
  alongside the public updater key in `src-tauri/tauri.conf.json`.
