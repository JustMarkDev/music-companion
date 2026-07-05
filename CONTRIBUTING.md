# Contributing to Music Companion

Thank you for helping improve Music Companion. The project accepts focused bug
fixes and enhancements through GitHub pull requests.

## Before you start

Search the existing
[issues](https://github.com/JustMarkDev/music-companion/issues) before opening
a new report or starting work.

Open an issue before implementing a substantial feature, architectural change,
new dependency, or broad refactor. Small, focused fixes do not require prior
discussion. Early agreement helps avoid work that does not fit the project's
scope or Windows-only design.

## Development workflow

1. Fork the repository.
2. Create a focused branch from the current default branch.
3. Install dependencies:

   ```powershell
   npm ci
   ```

4. Make a scoped change. Avoid unrelated formatting or refactoring.
5. Add or update tests where practical.
6. Update documentation when behavior, setup, commands, or configuration
   changes.
7. Run the relevant validation commands.
8. Push the branch to your fork and open a pull request.

Do not commit generated or local output such as `node_modules/`, `dist/`,
`src-tauri/target/`, logs, or editor-specific files.

## Project structure

- `src/main.ts` contains the TypeScript frontend behavior, settings, lyric
  parsing, and synchronization.
- `src/styles.css` contains the frontend presentation.
- `src-tauri/src/lib.rs` contains the Rust backend, including WMTC integration,
  LRCLIB requests, tray behavior, updater, and Tauri commands.
- `src-tauri/tauri.conf.json` contains application, window, bundle, and updater
  configuration.
- `.github/workflows/release.yml` builds and publishes Windows releases.

Follow the style and structure of the surrounding TypeScript, CSS, Rust, and
Tauri code. Keep frontend camelCase data contracts aligned with Rust
`#[serde(rename_all = "camelCase")]` types, and avoid breaking existing Tauri
commands or events.

## Validate your change

Run these checks before every pull request:

```powershell
npm run check
cargo test --manifest-path src-tauri/Cargo.toml
```

Also run:

```powershell
npm run build
```

when frontend production behavior changes.

Run:

```powershell
npm run tauri:build
```

when changing Tauri configuration, Rust dependencies, installers, updater
behavior, release automation, or the build system.

If you cannot run a check, state which check was omitted and why in the pull
request. Music Companion is Windows-only, so contributors working elsewhere
must explicitly identify any Windows-specific validation they could not
perform.

## Pull requests

Keep each pull request focused on one concern. Its description should include:

- What changed and why.
- A linked issue when one exists.
- Manual test steps and results.
- The automated checks that were run.
- Any checks that were not run and the reason.
- Screenshots or a short recording for visible UI changes.
- Known limitations or follow-up work.

Do not include signing keys, secrets, tokens, personal data, or unrelated
generated files. Maintainers may request revisions or close changes that are
out of scope.

## Reporting bugs

A useful bug report includes:

- Windows version.
- Music Companion version.
- Media player or browser and playback source.
- Clear reproduction steps.
- Expected behavior.
- Actual behavior.
- Whether the problem is consistent or intermittent.
- Relevant logs or error messages with sensitive data removed.
- Screenshots or recordings when they help explain the problem.

## Version and release changes

Version changes must remain synchronized across:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

Do not modify updater public keys, signing configuration, or release secrets
without explicit maintainer coordination.

## License status

The project has not selected a license. Public availability does not by itself
grant permission to use, copy, modify, or redistribute the code. This guide
does not add licensing terms or make representations about how a submitted
contribution may be used; contact the maintainer before contributing if this
affects your decision.
