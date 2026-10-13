<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="Music Companion app icon" width="96" height="96" />
</p>

<h1 align="center">Music Companion</h1>

<p align="center">
  <b>Synced lyrics, floating over everything.</b><br />
  A Windows 10/11 and macOS lyrics overlay that follows whatever you're playing.
</p>

<p align="center">
  <a href="https://music-companion-orcin.vercel.app/"><img src="docs/assets/demo.webp" alt="Lyrics fill in word by word over a city skyline while the settings window cycles through its tabs" width="100%" /></a>
</p>

<p align="center">
  <a href="https://github.com/JustMarkDev/music-companion/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/JustMarkDev/music-companion?style=for-the-badge&labelColor=1a1033&color=ff5a8a" /></a>
  <a href="https://github.com/JustMarkDev/music-companion/releases"><img alt="Total downloads" src="https://img.shields.io/github/downloads/JustMarkDev/music-companion/total?style=for-the-badge&labelColor=1a1033&color=a770ff" /></a>
  <a href="https://github.com/JustMarkDev/music-companion/actions/workflows/ci.yml"><img alt="CI status" src="https://img.shields.io/github/actions/workflow/status/JustMarkDev/music-companion/ci.yml?branch=main&style=for-the-badge&labelColor=1a1033&label=CI" /></a>
  <a href="LICENSE"><img alt="License: GPL-3.0" src="https://img.shields.io/github/license/JustMarkDev/music-companion?style=for-the-badge&labelColor=1a1033&color=7c9cff" /></a>
</p>

<p align="center">
  <a href="https://music-companion-orcin.vercel.app/"><img alt="Visit the website" src="https://img.shields.io/badge/Visit%20the-website-ff5a8a?style=for-the-badge&logo=vercel&logoColor=white" /></a>
  &nbsp;
  <a href="https://github.com/JustMarkDev/music-companion/releases/latest"><img alt="Download for Windows" src="https://img.shields.io/badge/Download%20for-Windows-0078D4?style=for-the-badge&logo=windows11&logoColor=white" /></a>
  &nbsp;
  <a href="https://github.com/JustMarkDev/music-companion/releases/latest"><img alt="Download for macOS" src="https://img.shields.io/badge/Download%20for-macOS-57606a?style=for-the-badge&logo=apple&logoColor=white" /></a>
</p>

<p align="center">
  <sub>The <a href="https://music-companion-orcin.vercel.app/">website</a> runs the real settings window next to a live lyrics preview. The animation above is captured from it.</sub>
</p>

## Highlights

- 🎤 **Word-by-word sync**, with line-by-line as the fallback, from [lrc.red](https://lrc.red/).
- 🌏 **Romanization and translation** for Japanese, Korean, Chinese, Arabic, and Devanagari lyrics.
- 👥 **Duets and background vocals**, set apart on the right and under the line.
- 🪟 **Glass overlay**: transparent and always on top, with click-through when you need it.
- 🎹 **Global hotkeys** and a tray or menu bar icon, so it never gets in the way.
- 🎨 **Yours to tune**: opacity, blur, typography, and a fresh accent colour for every song.

See the [full feature list](docs/features.md).

## Install

Download the Windows `.exe` or the universal macOS `.dmg` from the
[latest release](https://github.com/JustMarkDev/music-companion/releases/latest), start Music Companion, and play
something. That's it: no Bun, Node.js, Rust, or CMake needed.

macOS builds are not yet notarized, so the first launch needs a Control-click then **Open**.
Details are in [Using Music Companion](docs/usage.md).

## Documentation

| Guide                                      | What's in it                                                 |
| ------------------------------------------ | ------------------------------------------------------------ |
| [Using Music Companion](docs/usage.md)     | First launch, controls, and the tray or menu bar             |
| [Features](docs/features.md)               | Everything the overlay does                                  |
| [Platform support](docs/platforms.md)      | Windows and macOS differences, and how macOS reads playback  |
| [Troubleshooting](docs/troubleshooting.md) | When no player appears or lyrics look wrong                  |
| [Contributing](CONTRIBUTING.md)            | Building from source, quality checks, releases, and the code |

## License

Music Companion is available under the [GNU General Public License v3.0 only](LICENSE). It is not
affiliated with Spotify, lrc.red, Apple, YouTube, Microsoft, VLC, or Lyric Overlay.
The bundled MediaRemote adapter is licensed separately under the BSD 3-Clause
License; see `src-tauri/vendor/mediaremote-adapter/LICENSE`.
