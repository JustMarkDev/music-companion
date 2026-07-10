# Music Companion

Music Companion is a Windows-only desktop lyrics overlay built with Tauri,
TypeScript, Rust, Bun, and Vite+. It follows the active Windows Media Transport
Controls (WMTC) session, fetches lyrics from [LRCLIB](https://lrclib.net/), and
displays them in a transparent, always-on-top overlay.

## Features

- Works with Spotify, Apple Music, YouTube Music, browsers, VLC, and other
  WMTC-compatible players.
- Selects the current or actively playing media session and keeps following the
  selected player when more than one player is open.
- Fetches synchronized lyrics from LRCLIB, with a broad fallback search when the
  structured lookup only returns unsynced lyrics.
- Caches successful lyric lookups locally for faster repeat playback.
- Displays synced lyrics with smooth line highlighting and animated scrolling.
- Reports plain lyrics, instrumental tracks, unsupported variants, missing
  lyrics, and lyric lookup errors clearly in the overlay.
- Detects common track variants such as slowed, reverb, remix, sped up,
  nightcore, karaoke, live, and cover versions to avoid mismatched lyrics.
- Uses a transparent, resizable, always-on-top overlay with persistent acrylic
  blur.
- Restores the overlay position and size between sessions.
- Reasserts the overlay above other topmost windows while preserving foreground
  app input.
- Provides a separate settings window for opacity, blur intensity, lyric size,
  line spacing, start-at-login, and cache clearing.
- Hides to the system tray instead of closing.
- Supports pinned click-through mode with `Ctrl+Shift+L`; the tray icon can
  show and unlock the overlay again.
- Checks for signed updates automatically in release builds.

## Install

Music Companion supports Windows 10 and Windows 11.

1. Open the
   [latest GitHub release](https://github.com/JustMarkDev/music-companion/releases/latest).
2. Download the NSIS `.exe` installer.
3. Run the installer and start Music Companion.

Release builds check for signed updates when they launch. When a newer version
is available, the app downloads and installs it automatically, then restarts.
End users do not need Bun, Node.js, Rust, or Python.

## How to Use It

1. Start Music Companion.
2. Play or pause a track in a WMTC-compatible media player.
3. Wait for the player to publish track metadata. Music Companion follows the
   selected media session and searches LRCLIB for the best matching lyrics.
4. Move the pointer near the top of the overlay to show the window controls.
5. Drag the top area to move the overlay, or drag the window edges to resize it.
6. Use the gear button, double-click the lyric area, or right-click the lyric
   area to open settings.
7. Use the close button to hide the overlay to the tray, or use the tray menu to
   quit the app.

When the overlay is hidden or pinned in click-through mode, left-click the Music
Companion tray icon to show and unlock it. The tray menu also includes
**Unlock overlay** and **Quit**.

### Click-Through Mode

Click-through mode lets mouse input pass through the overlay to the app
underneath it. Toggle it with `Ctrl+Shift+L`.

If the overlay cannot be selected, left-click the tray icon. This shows the
overlay and disables click-through mode.

### Settings

Open settings to configure:

- Overlay opacity.
- Acrylic blur intensity.
- Lyric text size.
- Space between lyric lines.
- Accent color mode and color.
- Whether Music Companion starts when you sign in to Windows.
- Local lyrics cache clearing.

Accent color can be Dynamic, which derives colors from the current track, or
Manual, which applies one selected color to the overlay glow, top accent,
gradient, and controls. Changes apply immediately and are saved locally.

Settings are saved locally. Window position and size are restored between
sessions.

## Troubleshooting

### No Media Session Is Detected

- Confirm that a track is playing or paused in a WMTC-compatible player.
- Try changing tracks or restarting the media player so it republishes metadata.
- For browser playback, start playback in the tab first and confirm the browser
  exposes media controls to Windows.
- Some players only publish WMTC metadata after playback has started once.

### The Wrong Player Is Followed

Music Companion prefers the active Windows media session, keeps the last track
visible at its paused position when no session is available, and falls back to
another playing session when needed.

Pause the players you do not want to follow, then change tracks or resume the
target player so Windows publishes a fresh media event.

### Lyrics Are Missing, Plain, or Out of Sync

- Lyrics depend on LRCLIB data and the metadata published by the player.
- Some tracks only have plain lyrics, inaccurate timestamps, or no matching
  entry.
- Live, instrumental, slowed, reverb, remixed, sped up, nightcore, karaoke, and
  cover versions may not match the original recording.
- If the wrong lyrics were cached, open settings and clear the lyrics cache.

### The Overlay Is Hidden, Locked, or Off-Screen

- Left-click the tray icon to show and unlock the overlay.
- Use the tray menu's **Unlock overlay** action if the overlay is in
  click-through mode.
- Resize or move the overlay after it becomes selectable.

### The App Does Not Open Correctly

- Confirm that you are using Windows 10 or Windows 11.
- Install or repair the
  [Microsoft WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)
  if the window cannot render.
- Check whether the app is already running in the system tray.

## Development

### Prerequisites

- Windows 10 or 11.
- Bun and Node.js 20 or newer.
- Rust stable with the MSVC Windows toolchain.
- Microsoft WebView2 Runtime, included with most current Windows installations.

The repository includes Vite+ locally; a global Vite+ installation is not
required.

### Setup and Run

```powershell
bun install
bun run tauri:dev
```

The Tauri development command starts `bun run dev`, which runs the frontend at
`http://127.0.0.1:1421`.

### Validation and Build Commands

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

For local automatic formatting, run:

```powershell
bun run format
cargo fmt --manifest-path src-tauri/Cargo.toml
```

To build the Windows installer locally, run:

```powershell
bun run tauri:build
```

The installer is written under `src-tauri/target/release/bundle/nsis/`.

## Architecture

1. The Rust backend reads Windows media sessions through WMTC.
2. The backend subscribes to media session, playback, and property changes and
   emits frontend refresh events.
3. When the active track changes, the backend queries LRCLIB over HTTPS.
4. Successful results are retained in a bounded local cache for later sessions.
5. The TypeScript frontend parses line timestamps and renders the overlay.
6. A local playback clock interpolates between WMTC samples for smooth
   highlighting.

### Lyrics Latency Diagnostics

When running `bun run tauri:dev`, timing entries prefixed with `[latency]`
appear in the Rust terminal and WebView developer console. They report WMTC
refresh triggers, media-state IPC duration, lyric cache hits, in-flight request
reuse, and LRCLIB header/body timing.

The main implementation files are:

- `src/main.ts` - overlay UI, settings, lyric parsing, and synchronization.
- `src/styles.css` - overlay and settings presentation.
- `src-tauri/src/lib.rs` - WMTC integration, LRCLIB requests, tray behavior,
  updater, and Tauri commands.
- `src-tauri/tauri.conf.json` - windows, bundling, and updater configuration.

## Contributing

Bug reports and focused pull requests are welcome. Read
[CONTRIBUTING.md](CONTRIBUTING.md) before starting. Discuss substantial
features, architectural changes, and broad refactors in an issue first.

## License

Music Companion is licensed under the MIT License.

Music Companion is not affiliated with Spotify, LRCLIB, Apple, YouTube,
Microsoft, VLC, or Lyric Overlay.
