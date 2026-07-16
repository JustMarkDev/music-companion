# Contributing to Music Companion

Focused bug fixes, tests, documentation, accessibility improvements, and features within Music Companion's Windows lyrics-overlay scope are welcome.

## Before you start

- Open an issue or discussion before a large feature, architectural change, compatibility change, or release-workflow change. Small focused fixes do not require one.
- Explain the maintenance, security, size, licensing, and compatibility tradeoffs of any new dependency.
- Work on a focused branch and avoid unrelated formatting or refactoring.

## Local setup

Use Windows 10 or 11 with Bun, Node.js 20 or newer, stable Rust with the MSVC toolchain, and Microsoft WebView2 Runtime.

```powershell
bun install --frozen-lockfile
bun run tauri:dev
```

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
