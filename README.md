# Music Companion

Music Companion is a Windows-only desktop lyrics overlay built with Tauri,
TypeScript, and Rust. It reads the active media session through Windows Media
Transport Controls (WMTC), fetches lyrics from
[LRCLIB](https://lrclib.net/), and displays them in a transparent,
always-on-top window.

## Features

- Works with Spotify, Apple Music, YouTube Music, browsers, VLC, and other
  WMTC-compatible players.
- Displays synchronized lyrics from LRCLIB and clearly reports when only plain
  lyrics are available.
- Caches successful lyric lookups locally for faster repeat playback.
- Supports enhanced LRC word timing and smooth interpolation for line-timed
  lyrics.
- Provides live line and word highlighting with animated scrolling.
- Offers a transparent, resizable, always-on-top overlay.
- Includes configurable opacity, lyric size, line spacing, song-title
  visibility, and start-at-login behavior.
- Hides to the system tray instead of closing.
- Supports click-through mode from the UI or with `Ctrl+Shift+L`.

## Install

Music Companion supports Windows 10 and Windows 11.

1. Open the
   [latest GitHub release](https://github.com/JustMarkDev/music-companion/releases/latest).
2. Download either:
   - The NSIS `.exe` installer for the usual guided installation experience.
   - The `.msi` package for Windows Installer-based deployment.
3. Run the downloaded installer and start Music Companion.

Release builds check for signed updates when they launch. When a newer version
is available, the app downloads and installs it automatically, then restarts.
End users do not need Node.js, Rust, or Python.

## How to use it

1. Start Music Companion.
2. Play a track in a WMTC-compatible media player.
3. Wait for the player to publish track information. The overlay retrieves and
   displays the best available lyrics from LRCLIB.
4. Drag the top area to move the window, or use its edges to resize it.
5. Use the title-bar controls to open settings, minimize, maximize, or hide the
   overlay.

Closing the window hides it instead of quitting the application. Left-click the
Music Companion tray icon to show and unlock the overlay again. The tray menu
also provides **Unlock overlay** and **Quit** actions.

### Click-through mode

Click-through mode allows mouse input to pass through the overlay to windows
underneath it. Toggle it using the lock button, the compact window menu, or the
global `Ctrl+Shift+L` shortcut.

If the overlay is locked or hidden and cannot be selected, left-click its tray
icon to show and unlock it.

### Settings

Open the gear button to configure:

- Overlay opacity.
- Lyric text size.
- Space between lyric lines.
- Whether Music Companion starts when you sign in to Windows.
- Clear the locally saved lyrics cache.

Window position and size are restored between sessions.

## Troubleshooting

### No media session is detected

- Confirm that a track is actively playing or paused in a WMTC-compatible
  player.
- Try changing tracks or restarting the media player so it republishes its
  metadata.
- For browser playback, ensure the browser exposes media controls to Windows.
  Some tabs do not publish metadata until playback has started.

### Multiple players are detected

If more than one media application is playing, Music Companion warns that the
active source is ambiguous. Pause the players you do not want to follow.

### Lyrics are missing or out of sync

Lyrics depend on the artist, title, duration, and data available from LRCLIB.
Some tracks have only plain lyrics, inaccurate timestamps, or no matching entry.
Track variants such as live, instrumental, slowed, or remixed versions may not
match the original recording.

### The app does not open correctly

- Confirm that you are using Windows 10 or 11; other operating systems are not
  supported.
- Install or repair the
  [Microsoft WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)
  if the window cannot render.
- Use the tray icon if the overlay is running but hidden off-screen or locked.

## Development

### Prerequisites

- Windows 10 or 11.
- Bun and Node.js 20 or newer.
- Rust stable with the MSVC Windows toolchain.
- Microsoft WebView2 Runtime, included with most current Windows installations.

The repository includes Vite+ locally; a global Vite+ installation is not
required.

### Setup and run

```powershell
bun install
bun run tauri:dev
```

The Tauri development command starts `bun run dev`, which runs the frontend at
`http://127.0.0.1:1421`.

### Validation and build commands

```powershell
bun run check
cargo test --manifest-path src-tauri/Cargo.toml
bun run build
bun run preview
bun run tauri:build
```

- `bun run check` runs the Vite+ project checks.
- The Cargo command runs the Rust unit tests.
- `bun run build` creates the frontend production bundle in `dist/`.
- `bun run preview` serves the frontend production build locally.
- `bun run tauri:build` creates Windows installers under
  `src-tauri/target/release/bundle/`.

## Architecture

1. The Rust backend reads the current Windows media session through WMTC.
2. When the active track changes, it queries LRCLIB over HTTPS.
3. Successful results are retained in a bounded local cache for later sessions.
4. The TypeScript frontend parses line and enhanced-word timestamps.
5. A local playback clock interpolates between WMTC samples for smooth
   highlighting.
6. For line-timed LRC, the frontend estimates timing across individual words.

### Lyrics latency diagnostics

When running `bun run tauri:dev`, timing entries prefixed with `[latency]`
appear in the Rust terminal and WebView developer console. They report WMTC
refresh triggers, media-state IPC duration, lyric cache hits, in-flight request
reuse, and LRCLIB header/body timing.

The main implementation files are:

- `src/main.ts` — overlay UI, settings, lyric parsing, and synchronization.
- `src/styles.css` — overlay and settings presentation.
- `src-tauri/src/lib.rs` — WMTC integration, LRCLIB requests, tray behavior,
  updater, and Tauri commands.
- `src-tauri/tauri.conf.json` — windows, bundling, and updater configuration.

## Contributing

Bug reports and focused pull requests are welcome. Read
[CONTRIBUTING.md](CONTRIBUTING.md) before starting. Discuss substantial
features, architectural changes, and broad refactors in an issue first.
Use GitHub Issues as the source of truth for planned work, and link each pull
request to the issue it implements.

## Releases

Tags matching `vX.Y.Z` trigger the Windows release workflow, which publishes
both NSIS (`.exe`) and Windows Installer (`.msi`) packages together with signed
updater metadata.

Before a release, keep the version synchronized in:

- `package.json`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`

Updater signing uses the `TAURI_SIGNING_PRIVATE_KEY` GitHub Actions secret. Its
private key must remain confidential and securely backed up. Losing it prevents
future signed updates for existing installations.

## License

This project has not selected a license. No permission to use, copy, modify, or
redistribute the code should be assumed from its public availability.

Music Companion is not affiliated with Spotify, LRCLIB, Apple, YouTube,
Microsoft, VLC, or Lyric Overlay.
