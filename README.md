# Music Companion

A Windows-only Tauri + Vite+ lyrics overlay inspired by Lyric Overlay and LyPy. It reads the active media session from Windows Media Transport Controls, fetches synced lyrics from LRCLIB, and displays them in a transparent always-on-top desktop overlay.

## Features

- Windows Media Transport Controls support for Spotify, Apple Music, YouTube Music, browsers, VLC, and other compatible players.
- LRCLIB synced lyrics, with plain-lyrics fallback.
- Enhanced LRC word timing when available, plus smooth per-word interpolation for line-timed lyrics.
- Transparent, resizable, always-on-top overlay.
- Live line and word highlighting with smooth scrolling and animated word transitions.
- Settings for opacity, lyric size, spacing, saturation, polling interval, start at login, and Spotify-focused auto-show behavior.
- Tray support: close hides the overlay, tray menu can show or quit the app.
- Global `Ctrl+Shift+L` hotkey to lock or unlock click-through mode.

## Prerequisites

- Windows 10 or 11.
- For development from source:
  - Node.js 20+.
  - Vite+ CLI is optional globally; the repo includes a local `vite-plus` dependency and npm scripts call `vp`.
  - Rust stable with the MSVC Windows toolchain.
  - Microsoft WebView2 Runtime, which is included on most Windows 10/11 installs.

End-user release builds do not require Node.js, Rust, or Python.

## Setup

```powershell
npm install
npm run tauri:dev
```

The Tauri dev command starts `npm run dev`, which runs `vp dev` on the Vite+ toolchain.

## Build

```powershell
npm run tauri:build
```

The Windows installer output is created under `src-tauri/target/release/bundle/`.

## Vite+ Commands

```powershell
npm run dev      # vp dev
npm run build    # vp build
npm run check    # vp check
npm run preview  # vp preview
```

## How It Works

1. The Rust backend asks Windows for the current media session through WMTC.
2. When the active track changes, the app queries LRCLIB over HTTPS.
3. The frontend parses line timestamps and enhanced word timestamps when present.
4. A local playback clock extrapolates between WMTC samples so highlighting stays smooth between polls.
5. When a track only has line-level LRC, the app estimates per-word timing across each line.

## Notes

- If multiple media apps are playing, the overlay warns so you can pause extras.
- Some browser tabs do not publish WMTC metadata until media controls are enabled by the browser.
- The app is not affiliated with Spotify, LRCLIB, Apple, YouTube, Microsoft, or Lyric Overlay.
