# Troubleshooting

- If no player appears, start playback once so the player publishes media metadata.
- If the wrong player is selected, pause other players and resume or change tracks
  in the intended player.
- If lyrics are incorrect or stale, clear the lyrics cache from settings.
- If the window appears closed, check the system tray or menu bar before starting
  another copy.
- Windows only: if the window does not render, install or repair Microsoft
  WebView2 Runtime.
- macOS only: if only Music and Spotify are detected, the overlay has fallen back
  to AppleScript. Check the Rust log for the active backend.
- macOS only: if the overlay reports no session at all, confirm that something is
  playing in a player that appears in Control Center's Now Playing tile, since
  that is the same source the overlay reads.

For how the macOS backends work, see [Platform support](platforms.md).
