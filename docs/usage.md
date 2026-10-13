# Using Music Companion

## Install

Download the installer for your platform from the
[latest GitHub release](https://github.com/JustMarkDev/music-companion/releases/latest):

- Windows: the NSIS `.exe`.
- macOS: the universal `.dmg`, then drag Music Companion to Applications.

End users do not need Bun, Node.js, Rust, or CMake.

macOS builds are not yet notarized, so Gatekeeper blocks the first launch. Open
Music Companion once with Control-click then **Open**, or allow it under **System
Settings → Privacy & Security**. Signing and notarization activate as soon as
Developer ID credentials are added to the release workflow, with no code change.

## Use

Start Music Companion and play a track in any player the system reports. Move the
pointer near the top of the overlay to reveal its controls. Drag the top area to
move it, resize it from the window edges, and open settings with the gear button,
a double-click, or a right-click in the lyric area. Switching to Music Companion with
Alt+Tab or Cmd+Tab moves the pointer onto the top area, ready to drag.

Closing the overlay hides it. On Windows it hides to the system tray, and on macOS
it hides to the menu bar while staying in the Dock. If click-through mode makes the
window unselectable, left-click the tray or menu bar icon to show and unlock it.

Default hotkeys are listed under [Platform support](platforms.md) and can be changed in
Settings → Shortcuts.

Having trouble? See [Troubleshooting](troubleshooting.md).
