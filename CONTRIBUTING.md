# Contributing to Music Companion

Focused bug fixes, tests, documentation, accessibility improvements, and features within Music Companion's Windows and macOS lyrics-overlay scope are welcome.

## Before you start

- Open an issue or discussion before a large feature, architectural change, compatibility change, or release-workflow change. Small focused fixes do not require one.
- Explain the maintenance, security, size, licensing, and compatibility tradeoffs of any new dependency.
- Work on a focused branch and avoid unrelated formatting or refactoring.

## Local setup

Use Bun, Node.js 20 or newer, and stable Rust on either supported platform.

- Windows 10 or 11 with the Rust MSVC toolchain and Microsoft WebView2 Runtime.
- macOS 11 or later with Xcode Command Line Tools and CMake (`brew install cmake`),
  which `build.rs` needs to compile the bundled MediaRemote adapter.

Clone with `--recurse-submodules`, or run `git submodule update --init --recursive`
in an existing clone, so the macOS adapter source is present.

```bash
bun install --frozen-lockfile
bun run tauri:dev
```

Changes to platform backends should be checked on both platforms when possible.
State in the pull request which platforms you verified.

## Making changes

Follow the existing TypeScript/Tauri architecture and preserve behavior unless the change intentionally alters it. Update tests and documentation when behavior changes.

Before submitting a change, run every applicable check:

```powershell
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

## Pull requests

Pull requests run changed-area validation for the frontend and Rust/Tauri code. The stable `Pull request validation` check succeeds only when every affected area passes; unaffected checks are skipped.

## Change checklist

- [ ] The change has a clear purpose and focused scope.
- [ ] Applicable checks pass locally.
- [ ] Tests cover new or changed behavior where practical.
- [ ] Documentation reflects user-visible changes.
- [ ] New dependencies are justified.
- [ ] No credentials, secrets, unrelated generated files, or placeholders are included.

Use short, imperative commit subjects. Conventional Commits, signed commits, a DCO, and a CLA are not required.
