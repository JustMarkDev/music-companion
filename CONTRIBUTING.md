# Contributing

Issues and pull requests are welcome. Discuss substantial features, architectural changes, and
broad refactors before implementation, and run the [quality checks](#quality-checks) before
opening a pull request. Pull request titles follow Conventional Commits, because releases are
generated from them.

## Technology stack

- Tauri 2 and Rust provide platform integration, media-session monitoring, the
  tray and menu bar, networking, persistence, and updates.
- TypeScript, HTML, and CSS implement the overlay and settings interface.
- Bun manages frontend dependencies and scripts.
- Vite+ provides development, checking, linting, formatting, and builds.

## Development

Prerequisites on both platforms are Bun, Node.js 20 or newer, and stable Rust.
Vite+ is installed locally.

- Windows: Windows 10 or 11, the Rust MSVC toolchain, and Microsoft WebView2
  Runtime.
- macOS: macOS 11 or later, Xcode Command Line Tools, and CMake
  (`brew install cmake`) to compile the bundled MediaRemote adapter.

Clone with submodules so the macOS adapter source is present:

```bash
git clone --recurse-submodules https://github.com/JustMarkDev/music-companion.git
```

In an existing clone, run `git submodule update --init --recursive`.

```bash
bun install --frozen-lockfile
bun run tauri:dev
```

The development server listens on `http://127.0.0.1:1421`. Development builds
emit `[latency]` diagnostics for media refreshes, IPC, cache activity, and lyrics
provider requests in the Rust terminal and WebView console.

On macOS, `build.rs` compiles the adapter framework into `src-tauri/resources/macos/`,
which is generated output and is not committed.

To see the interface with a sample song and no Tauri or media session, run `bun run dev` and open
`http://127.0.0.1:1421/` in a browser, or add `?view=settings` for the settings window.

## Quality checks

```bash
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

Run `bun run format` and `cargo fmt --manifest-path src-tauri/Cargo.toml` to
apply formatting locally.

## Build and release

```bash
bun run tauri:build           # host platform
bun run tauri:build:windows   # Windows x86-64 NSIS installer
bun run tauri:build:macos     # universal macOS .app and .dmg
```

Artifacts are written below:

- `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`
- `src-tauri/target/universal-apple-darwin/release/bundle/dmg/`

A universal macOS build needs both Rust targets:

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
```

Approved releases are published from version tags matching `v*`. Both jobs create
signed Tauri updater artifacts.

Every action in `.github/workflows` is pinned to a commit SHA, and the release job holds the
updater signing key, so keep it that way: Dependabot bumps the SHAs, and a moving ref such as
`@main` would let upstream code read the key. If a build job fails after the tag exists, use
**Re-run failed jobs** on the same run: the `release` job is not repeated, the builds upload to
the draft release of that tag, and **Publish release** runs once every build has passed.

### macOS updates

macOS auto-update works without an Apple Developer account: both jobs sign updater
artifacts with `TAURI_SIGNING_PRIVATE_KEY`, and the macOS job adds the universal
`.app.tar.gz` to `latest.json` next to the Windows entry. Unsigned builds still need
the Control-click **Open** on first launch. To also sign and notarize, add
`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`, `APPLE_PASSWORD`, and `APPLE_TEAM_ID` as repository secrets; no other
change is needed.

## Project structure

```text
src/main.ts                     Overlay UI, settings, and lyric synchronization
src/settings.ts                 Settings decoding and per-platform defaults
src/lyrics-cache.ts             Lyrics cache in IndexedDB, with word-sync bookkeeping
src/lyrics-loader.ts            Lyrics lookup, word-timing request, and retry budget (no DOM)
src/styles.css                  Overlay and settings styles
src-tauri/src/lib.rs            Shared commands, tray, and updater
src-tauri/src/lyrics.rs         lrc.red lookup, ranking of its matches, and word sync
src-tauri/src/ttml.rs           Lyrics model and the parser for lrc.red's TTML files
src-tauri/src/media_macos.rs    macOS now-playing reader and transport control
src-tauri/src/backdrop_macos.rs macOS overlay backdrop
src-tauri/src/z_order_macos.rs  macOS overlay stacking
src-tauri/src/media_windows.rs  Windows now-playing events and transport control
src-tauri/src/backdrop_windows.rs  Windows Acrylic backdrop
src-tauri/src/z_order_windows.rs   Windows topmost monitor
src-tauri/build.rs              Builds and stages the macOS MediaRemote adapter
src-tauri/tauri.conf.json       Shared window and updater configuration
src-tauri/tauri.windows.conf.json  NSIS packaging and Windows updater install mode
src-tauri/tauri.macos.conf.json    App and DMG packaging, bundled adapter
src-tauri/vendor/               Pinned MediaRemote adapter submodule
docs/                           User guides and README images
.github/                        Dependabot, pull-request validation, and releases
```
