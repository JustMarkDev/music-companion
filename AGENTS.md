# Repository instructions

## Agent skills

### Issue tracker

Issues are tracked as local Markdown files under `.scratch/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Uses the default five canonical triage labels. See `docs/agents/triage-labels.md`.

### Domain docs

Uses a single-context layout. See `docs/agents/domain.md`.

## Repository scope

Music Companion is a Windows 10/11 desktop lyrics overlay built with Tauri 2, TypeScript, Rust, Bun, and Vite+. Preserve Windows-only behavior and the current Tauri/frontend boundary unless a task explicitly changes them.

- Relevant source areas: `src/`, `src-tauri/src/`, `src-tauri/tauri.conf.json`, and `.github/workflows/`.
- Instruction precedence: follow this root guidance, then the closest applicable nested `AGENTS.md` or `AGENTS.override.md` for scoped work.
- Scoped instruction files: none.

## Repository navigation

| Path                            | Purpose                                                                     |
| ------------------------------- | --------------------------------------------------------------------------- |
| `src/main.ts`                   | Overlay UI, settings, lyric parsing, and synchronization                    |
| `src/styles.css`                | Overlay and settings presentation                                           |
| `src-tauri/src/lib.rs`          | WMTC integration, LRCLIB access, tray behavior, updater, and Tauri commands |
| `src-tauri/tauri.conf.json`     | Windows, NSIS packaging, and updater configuration                          |
| `.github/workflows/ci.yml`      | Changed-area pull-request validation and stable required gate               |
| `.github/workflows/release.yml` | Verification and publication for approved `v*` tags                         |

## Verified commands

Use Bun for frontend dependencies and scripts and stable Rust MSVC for native code.

| Task                      | Command                                                                                             |
| ------------------------- | --------------------------------------------------------------------------------------------------- |
| Setup/install             | `bun install --frozen-lockfile`                                                                     |
| Development               | `bun run tauri:dev`                                                                                 |
| Test                      | `cargo test --manifest-path src-tauri/Cargo.toml`                                                   |
| Lint                      | `bun run lint` and `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` |
| Format check              | `bun run format:check` and `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`              |
| Format                    | `bun run format` and `cargo fmt --manifest-path src-tauri/Cargo.toml`                               |
| Type-check                | `bun run check`                                                                                     |
| Build                     | `bun run build`                                                                                     |
| Package                   | `bun run tauri:build`                                                                               |
| Dependency/security audit | `bun audit` and, from `src-tauri`, `cargo audit`                                                    |
| Release                   | Apply the version bump on `main`, tag that commit with `v*`, and push the approved tag              |

## Architecture and dependency constraints

- Keep Windows integration, networking, persistence, tray behavior, and updates in Rust; keep overlay and settings presentation in the frontend.
- Do not introduce another package manager or commit generated `dist/`, `node_modules/`, or `src-tauri/target/` output.
- Ask before adding or replacing a dependency, and explain its maintenance, security, size, licensing, and compatibility tradeoffs.
- Keep pull-request validation split into frontend and Rust/Tauri changed areas, with `Pull request validation` as the stable required gate. Dependabot is not PR CI.
- Releases are approved GitHub releases built from `v*` tags as signed Windows x86-64 NSIS installers with Tauri updater metadata. Release version bumps happen on `main`, not a feature branch.

## Working and autonomy policy

- For requests to answer, explain, review, diagnose, or plan, inspect the relevant materials and report the result. Do not implement changes unless the request also asks for them.
- For requests to change, build, or fix, make the requested in-scope local changes and run relevant non-destructive validation without asking first.
- Require confirmation before destructive operations, external writes, publishing, handling credentials, purchases, irreversible migrations, or a material expansion of scope.
- Preserve user changes and unrelated work. Do not silently overwrite, revert, or reformat outside the requested scope.

## Verification and completion

- Run targeted checks first, then every applicable repository-defined check above when practical.
- Report checks that could not run and why.
- Do not invent commands, claim unverified behavior, or declare completion while required work remains.
- Update tests and documentation when observable behavior changes.
- Leave no placeholder, credential, unrelated generated file, or unexplained behavior change.
