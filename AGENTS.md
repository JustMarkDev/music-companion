# Repository instructions

## Agent skills

### Issue tracker

Issues are tracked as local Markdown files under `.scratch/`. See `docs/agents/issue-tracker.md`.

### Triage labels

Uses the default five canonical triage labels. See `docs/agents/triage-labels.md`.

### Domain docs

Uses a single-context layout. See `docs/agents/domain.md`.

## Repository scope

Music Companion is a Windows 10/11 and macOS 11+ desktop lyrics overlay built with Tauri 2, TypeScript, Rust, Bun, and Vite+. Preserve behavior on both platforms and the current Tauri/frontend boundary unless a task explicitly changes them.

- Relevant source areas: `src/`, `src-tauri/src/`, `src-tauri/build.rs`, `src-tauri/tauri*.conf.json`, and `.github/workflows/`.
- Platform backends sit behind one shared interface. `media`, `persistent_backdrop`, and `overlay_z_order` each resolve to a Windows or macOS implementation, so shared orchestration must not branch on the platform.
- `src-tauri/vendor/` is a pinned upstream submodule. Do not edit or reformat it; change the pin instead.
- Instruction precedence: follow this root guidance, then the closest applicable nested `AGENTS.md` or `AGENTS.override.md` for scoped work.
- Scoped instruction files: none.

## Repository navigation

| Path                                | Purpose                                                             |
| ----------------------------------- | ------------------------------------------------------------------- |
| `src/main.ts`                       | Overlay UI, settings, lyric parsing, and synchronization            |
| `src/settings.ts`                   | Settings decoding and per-platform hotkey defaults                  |
| `src/styles.css`                    | Overlay and settings presentation                                   |
| `src-tauri/src/lib.rs`              | Shared commands, WMTC integration, LRCLIB access, tray, and updater |
| `src-tauri/src/media_macos.rs`      | macOS now-playing reader and transport control                      |
| `src-tauri/src/backdrop_macos.rs`   | macOS overlay backdrop                                              |
| `src-tauri/src/z_order_macos.rs`    | macOS overlay stacking                                              |
| `src-tauri/build.rs`                | Builds and stages the macOS MediaRemote adapter                     |
| `src-tauri/tauri.conf.json`         | Shared window and updater configuration                             |
| `src-tauri/tauri.windows.conf.json` | NSIS packaging and Windows updater install mode                     |
| `src-tauri/tauri.macos.conf.json`   | App and DMG packaging plus the bundled adapter                      |
| `.github/workflows/ci.yml`          | Changed-area pull-request validation and stable required gate       |
| `.github/workflows/release.yml`     | Verification and publication for approved `v*` tags                 |

## Verified commands

Use Bun for frontend dependencies and scripts, stable Rust MSVC for native code on Windows, and stable Rust with Xcode Command Line Tools and CMake on macOS. Clone submodules before building for macOS.

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
| Package                   | `bun run tauri:build`, or `bun run tauri:build:windows` / `bun run tauri:build:macos`               |
| Dependency/security audit | `bun audit` and, from `src-tauri`, `cargo audit`                                                    |
| Release                   | Apply the version bump on `main`, tag that commit with `v*`, and push the approved tag              |

## Architecture and dependency constraints

- Keep platform integration, networking, persistence, tray and menu bar behavior, and updates in Rust; keep overlay and settings presentation in the frontend.
- Do not introduce another package manager or commit generated `dist/`, `node_modules/`, `src-tauri/target/`, or `src-tauri/resources/macos/` output.
- Ask before adding or replacing a dependency, and explain its maintenance, security, size, licensing, and compatibility tradeoffs.
- Keep pull-request validation split into frontend and Rust/Tauri changed areas, with `Pull request validation` as the stable required gate. Rust/Tauri checks run on Windows and macOS. Dependabot is not PR CI.
- Releases are approved GitHub releases built from `v*` tags: signed Windows x86-64 NSIS installers with Tauri updater metadata, plus a universal macOS `.dmg`. Release version bumps happen on `main`, not a feature branch.
- macOS release signing, notarization, and automatic updates are wired but inactive until Apple Developer ID secrets exist. Keep them degrading to an unsigned build rather than failing.

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
