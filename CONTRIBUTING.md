# Contributing

Thanks for taking the time to improve Music Companion. Keep changes focused and
reviewable, and preserve existing behavior unless the issue or pull request
explicitly calls for a behavior change.

## Before You Start

- Use a feature branch.
- Open or link an issue for substantial features, architecture changes, or broad
  refactors.
- State assumptions when a product or technical decision is unclear.
- Keep pull requests scoped to one problem.

## Development Setup

Music Companion is Windows-only.

Prerequisites:

- Windows 10 or Windows 11.
- Bun and Node.js 20 or newer.
- Rust stable with the MSVC Windows toolchain.
- Microsoft WebView2 Runtime.

Install dependencies and run the app:

```powershell
bun install
bun run tauri:dev
```

## Validation

Run the relevant checks before opening or updating a pull request:

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

## Pull Requests

- CI runs on pull requests only.
- Draft pull requests are not expected to run CI.
- Keep diffs small enough to review comfortably.
- Include testing notes in the pull request description.
- Link the issue or discussion the pull request implements when applicable.

## Release Notes

Releases are based on approved version tags matching `v*`. Release workflow
changes should stay limited to tagged release builds.
