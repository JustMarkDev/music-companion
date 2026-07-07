# Agent Instructions

## Workflow

Use GitHub Issues as the source of truth for future work. Treat each issue as
the task brief, and use Pull Requests as the implementation record: summarize
the change, link the issue, and record verification.

This is a Windows-only Tauri 2 desktop app. It reads Windows Media Transport
Controls, fetches lyrics from LRCLIB, and renders synchronized lyrics with a
TypeScript frontend.

## Source of Truth

- Prefer GitHub Issues over local TODOs, notes, or planning files for active
  work.
- Keep user-facing setup and command documentation in `README.md`.
- Keep contributor workflow details in `CONTRIBUTING.md`.
- Keep versions synchronized in `package.json`, `src-tauri/Cargo.toml`, and
  `src-tauri/tauri.conf.json`.

## Coding Rules

- Keep changes scoped to the issue or explicit request.
- Preserve Windows-only support unless the requested change implements and
  validates another platform.
- Preserve camelCase frontend contracts backed by Rust serde types.
- Treat Tauri commands, command arguments, events, window labels, persisted
  settings, updater config, and release config as compatibility-sensitive.
- Do not add dependencies unless necessary; update manifests and lockfiles
  together when dependencies change.
- Do not commit generated output such as `node_modules/`, `dist/`,
  `src-tauri/target/`, logs, or caches.
- Never expose updater private keys, signing material, tokens, or credentials.

## Verification

Run checks proportionally to the change:

- `bun run check` for TypeScript/frontend changes.
- `cargo test --manifest-path src-tauri/Cargo.toml` for Rust changes.
- `bun run build` for production frontend changes.
- `bun run tauri:build` for packaging, installer, updater, release, native
  dependency, or Tauri build-system changes.

Record any checks not run and any Windows-specific validation limitations.

## Git Rules

- Inspect the working tree before editing.
- Do not overwrite unrelated user work.
- Do not commit or push unless explicitly asked.
- Do not rewrite history, reset, or discard changes unless explicitly asked.
