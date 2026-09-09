# Music Companion agent guide

## Scope

Music Companion is a lyrics overlay for Windows 10/11 and macOS 11 or later. It
uses Tauri 2, TypeScript, Rust, Bun, and Vite+. Preserve observable behavior on
both platforms unless the task explicitly narrows or changes support.

## Architecture invariants

- Keep platform integration, networking, persistence, tray and menu bar behavior,
  and updates in Rust. Keep overlay and settings presentation in the frontend.
- Keep shared Rust orchestration platform-neutral. The `media`,
  `persistent_backdrop`, and `overlay_z_order` modules must expose the same
  interface through their Windows and macOS implementations; contain platform
  branching inside those backends.
- Treat `src-tauri/vendor/` as a pinned upstream submodule. Change its pin to
  adopt upstream work; leave vendored contents untouched.
- Use Bun exclusively for frontend dependencies and scripts. Keep `dist/`,
  `node_modules/`, `src-tauri/target/`, and `src-tauri/resources/macos/` as
  uncommitted generated output.

Start frontend behavior work in `src/main.ts` and the focused modules under
`src/`. Start shared Tauri work in `src-tauri/src/lib.rs`; the macOS media,
backdrop, and stacking backends live in their adjacent `*_macos.rs` files.

## Sources of truth

- Inspect `package.json` and `bun.lock` for frontend commands or dependencies.
- Inspect `src-tauri/Cargo.toml` for Rust dependencies and targets.
- For window, packaging, or updater work, inspect the shared and
  platform-specific `src-tauri/tauri*.conf.json` files together.
- Before changing CI or releases, read the matching file in `.github/workflows/`.
- Check user-facing behavior and platform prerequisites against `README.md`.
  Update any statement that the change makes inaccurate.

For issue or spec work, use local Markdown under `.scratch/<feature-slug>/`.
Place the spec at `spec.md` and numbered implementation tickets at
`issues/<NN>-<slug>.md`. Record triage in a `Status:` line using one of
`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, or `wontfix`;
append discussion under `## Comments`.

## Change boundaries

- Preserve the current Tauri/frontend boundary and both-platform behavior while
  making the smallest coherent change that satisfies the request.
- Obtain approval before adding or replacing a dependency. Present its
  maintenance, security, size, licensing, and platform-compatibility tradeoffs.
- Obtain approval before destructive operations, publishing, handling
  credentials, irreversible migrations, or materially expanding the requested
  scope.
- Keep user work and unrelated changes intact. Keep the final diff focused on
  the request.
- Preserve the CI contract: changed-area frontend and Rust/Tauri jobs feed the
  stable `Pull request validation` gate, and native validation runs on Windows
  and macOS. Dependabot remains separate from pull-request CI.
- Preserve the release contract: approved `v*` tags publish a signed Windows
  x86-64 NSIS installer with updater metadata and a universal macOS DMG. Apply
  release version bumps on `main`. Without Apple Developer ID secrets, publish
  the macOS DMG unsigned and leave its updater disabled.

## Verification

Run focused checks first, then every applicable check below. A change is
verified only when each applicable check passes, or the final report names the
check that could not run and the concrete reason.

- After frontend or shared-configuration changes, run `bun run test`,
  `bun run check`, `bun run lint`, `bun run format:check`, and `bun run build`.
- After Rust or Tauri changes, run
  `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`,
  `cargo test --manifest-path src-tauri/Cargo.toml`, and
  `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`.
- After dependency changes, run `bun audit`. Run `cargo audit` from `src-tauri`
  when Rust dependencies change.
- Use stable Rust with MSVC on Windows and Xcode Command Line
  Tools plus CMake on macOS. Initialize submodules before macOS builds. Verify
  platform-backend changes on both operating systems when practical, and state
  which platforms were exercised.

Update tests for changed behavior and documentation for user-visible behavior.
Before completion, inspect the diff and confirm that it contains no placeholder,
credential, unrelated generated file, accidental reformatting, or unexplained
behavior change.
