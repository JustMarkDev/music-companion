# Agent Instructions

## Project Context

Music Companion is a Windows-only desktop lyrics overlay built with Tauri,
TypeScript, Rust, Bun, and Vite+.

## Development Workflow

- Work on feature branches and keep changes scoped.
- Prefer small, reviewable diffs over broad refactors.
- Use pull requests as the main review and integration point.
- State assumptions when a task depends on unclear product or technical
  decisions.
- Preserve existing behavior unless the requested change explicitly says
  otherwise.

## CI Policy

- CI should run on pull requests only.
- Do not add push, schedule, issue, discussion, or deployment triggers to CI.
- Draft pull requests are not expected to run CI.
- Release workflows are limited to approved tagged releases.

## Validation Commands

Use the relevant commands before opening or updating a pull request:

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

For local formatting, run:

```powershell
bun run format
cargo fmt --manifest-path src-tauri/Cargo.toml
```

## Release Policy

- Releases are based on version tags matching `v*`.
- When explicitly asked to prepare or perform a release, do not create a
  feature branch. Apply the release version bump directly on `main`, then tag
  that `main` commit.
