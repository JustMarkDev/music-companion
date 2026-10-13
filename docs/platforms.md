# Platform support

| Area                         | Windows 10/11                           | macOS 11 and later                                                     |
| ---------------------------- | --------------------------------------- | ---------------------------------------------------------------------- |
| Media session                | Windows Media Transport Controls (WMTC) | `MediaRemote`, read through the bundled adapter                        |
| Overlay backdrop             | Mica or Acrylic                         | Liquid Glass on macOS 26, `NSVisualEffectView` earlier                 |
| Stacking                     | Repeated topmost re-assertion           | Raised window level, visible on every Space and beside fullscreen apps |
| Start at login               | `Run` registry key                      | Launch agent                                                           |
| Click-through toggle         | `Ctrl+Shift+L`                          | `⇧⌘L`                                                                  |
| Next / previous / play-pause | `Ctrl+→`, `Ctrl+←`, `Ctrl+Shift+Space`  | `⌃⌘→`, `⌃⌘←`, `⇧⌘Space`                                                |
| Packaging                    | NSIS `.exe` installer                   | Universal `.dmg` (Apple Silicon and Intel)                             |
| Automatic updates            | Yes, signed                             | Not yet; see [macOS updates](../CONTRIBUTING.md#macos-updates)         |

macOS transport shortcuts include Command because macOS reserves `Ctrl+→` and
`Ctrl+←` for Mission Control.

## How macOS reads the now-playing session

macOS has no public equivalent of WMTC, and since macOS 15.4 Apple has refused
`MediaRemote` access to applications without a private entitlement. Music
Companion therefore bundles [`mediaremote-adapter`](https://github.com/ungive/mediaremote-adapter)
(BSD 3-Clause) as a pinned submodule under `src-tauri/vendor/`. The adapter is
loaded by `/usr/bin/perl`, which Apple still entitles, and streams metadata,
position, playback state, and the owning application for every player the system
knows about. Playback commands travel back the same way.

If that adapter cannot reach `MediaRemote`, the overlay falls back to AppleScript
against Music and Spotify. The fallback asks for Automation permission on first
use and cannot see browser sessions, so YouTube and other web players stop
appearing until the adapter works again. The Rust log records which backend is
active at startup.

Because the adapter depends on a private framework, a future macOS release could
break it. The AppleScript fallback exists so the overlay keeps working if that
happens.
