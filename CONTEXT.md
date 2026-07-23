# Music Companion

Music Companion follows the media playing on Windows and presents lyrics synchronized to that playback.

## Language

**Song**:
A musical work identified by its artist and title, independent of the application or provider playing it.

**Playback variant**:
A particular rendition of a song whose duration can require different synchronized lyrics. Variants with durations within three seconds of each other are treated as the same for lyric lookup and reuse.
_Avoid_: Track when discussing duration-sensitive lyric identity

**Media session**:
Windows' current report of a player's media and playback state. A media session may temporarily disappear while its playback variant remains current.

**Playback clock**:
Music Companion's estimate of the current position within a playback variant, used to synchronize lyric lines.
