# Music Companion

Music Companion is a Windows 10/11 desktop lyrics overlay. It follows the active
Windows Media Transport Controls (WMTC) session, retrieves lyrics from
[LRCLIB](https://lrclib.net/), and presents them in a transparent, always-on-top
Tauri window.

## Key Capabilities

- Follows WMTC-compatible players, including desktop players and browsers.
- Displays synchronized lyrics with smooth highlighting and scrolling.
- Handles plain lyrics, instrumental tracks, missing lyrics, and common track
  variants without silently presenting a poor match.
- Normalizes browser metadata such as `- Topic`, `VEVO`, and matching
  `Artist - Song` video titles before searching for lyrics.
- Caches successful lookups and restores overlay position, size, and settings.
- Provides opacity, blur, typography, accent-color, start-at-login, and cache
  controls in a separate settings window.
- Supports a global `Ctrl+Shift+L` click-through toggle and tray recovery.
- Checks for signed updates in release builds.

## Technology Stack

- Tauri 2 and Rust provide Windows integration, WMTC monitoring, the tray,
  networking, persistence, and updates.
- TypeScript, HTML, and CSS implement the overlay and settings interface.
- Bun manages frontend dependencies and scripts.
- Vite+ provides development, checking, linting, formatting, and builds.

## Installation and Use

Download the NSIS `.exe` from the
[latest GitHub release](https://github.com/JustMarkDev/music-companion/releases/latest).
End users do not need Bun, Node.js, or Rust.

Start Music Companion and play a track in a WMTC-compatible player. Move the
pointer near the top of the overlay to reveal its controls. Drag the top area to
move it, resize it from the window edges, and open settings with the gear button,
a double-click, or a right-click in the lyric area.

Closing the overlay hides it in the system tray. If click-through mode makes the
window unselectable, left-click the tray icon to show and unlock it.

## Troubleshooting

- If no player appears, start playback once so the player publishes WMTC metadata.
- If the wrong player is selected, pause other players and resume or change tracks
  in the intended player.
- If lyrics are incorrect or stale, clear the lyrics cache from settings.
- If the window does not render, install or repair Microsoft WebView2 Runtime.
- If the window appears closed, check the system tray before starting another copy.

## Development

Prerequisites are Windows 10 or 11, Bun, Node.js 20 or newer, stable Rust with the
MSVC toolchain, and Microsoft WebView2 Runtime. Vite+ is installed locally.

```powershell
bun install --frozen-lockfile
bun run tauri:dev
```

The development server listens on `http://127.0.0.1:1421`. Development builds
emit `[latency]` diagnostics for WMTC refreshes, IPC, cache activity, and LRCLIB
requests in the Rust terminal and WebView console.

## Quality Checks

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

Run `bun run format` and `cargo fmt --manifest-path src-tauri/Cargo.toml` to
apply formatting locally.

## Build and Release

```powershell
bun run tauri:build
```

The Windows x86-64 NSIS installer is written below
`src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`. Approved releases
are published from version tags matching `v*`; release builds also create signed
Tauri updater artifacts.

## Project Structure

```text
src/main.ts                 Overlay UI, settings, and lyric synchronization
src/styles.css              Overlay and settings styles
src-tauri/src/lib.rs        WMTC, LRCLIB, tray, updater, and Tauri commands
src-tauri/tauri.conf.json   Window, NSIS bundle, and updater configuration
.github/workflows/          Pull-request CI and tagged release automation
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Discuss substantial features,
architectural changes, and broad refactors before implementation.

## License

Music Companion is available under the [MIT License](LICENSE). It is not
affiliated with Spotify, LRCLIB, Apple, YouTube, Microsoft, VLC, or Lyric Overlay.
