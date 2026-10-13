# Features

Music Companion is a Windows 10/11 and macOS desktop lyrics overlay. It follows the active
system media session, retrieves lyrics from [lrc.red](https://lrc.red/), and presents them in a
transparent, always-on-top Tauri window.

- Follows the system media session on both platforms, including desktop players
  and browsers.
- Displays synchronized lyrics with smooth highlighting and scrolling.
- Highlights word by word when lrc.red times individual words (many tracks), and line by
  line otherwise. Romanized lyrics keep their own word timing.
- Reads each song as a TTML file from lrc.red, the only lyrics provider. Lyrics, romanization,
  translation, singers, and background vocals all come from that one file.
- Romanizes Japanese, Korean, Chinese, Arabic, and Devanagari lyrics with the transliteration
  lrc.red provides. Songs without one fall back to the original; nothing is romanized on the
  device. Romanized mode shows the romanization as the lyric line. Original + romanized keeps
  the original and puts each word's romanization above it (the whole romanized line when words
  can't be paired), and hides the translation, which would need a third line.
- Optionally shows lrc.red's translation under each line (Settings → Show translation, off by
  default) when the song has one. Translations that only echo the line, such as
  English translations of English lines, are hidden.
- Sets the second singer of a duet on the right and shows background vocals smaller under the
  line, filled word by word like the lead.
- Optionally asks lrc.red's alignment model to time every word of songs whose lyrics are not
  word-timed (Settings → Sync words with AI, off by default). The first sync of a song can take
  10 seconds or more; the lyrics on screen are upgraded in place and the result is saved.
- Recognizes instrumental tracks and common track variants (remixes, live cuts, covers) from the
  title, and says so instead of silently presenting a poor match.
- Normalizes browser metadata such as `- Topic`, `VEVO`, and `Artist - Song`
  titles for display and lyrics searches. Official-video labels are removed,
  and their title artist takes precedence over mismatched YouTube channel metadata.
- Caches up to 10,000 successful lookups on the device and restores overlay position, size,
  and settings.
- Provides opacity, blur, typography, accent-color, start-at-login, and cache
  controls in a separate settings window.
- Supports configurable global playback hotkeys, with a Windows media-key
  fallback for browser sessions that decline next or previous commands.
- Supports a global click-through toggle and tray or menu bar recovery.
- Checks for signed updates in release builds on Windows.
