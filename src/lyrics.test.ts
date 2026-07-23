import { describe, expect, it } from "vitest";
import {
  getLocalLyricsNotice,
  isSameCachedVariant,
  normalizeLyricsMetadata,
  parseLyrics,
  playbackVariant,
  selectLyricsDisplay,
  startsNewPlaybackVariant,
  type LyricsResult,
} from "./lyrics";

const result = (overrides: Partial<LyricsResult> = {}): LyricsResult => ({
  source: "LRCLIB",
  trackName: "Song",
  artistName: "Artist",
  albumName: "Album",
  duration: 180,
  instrumental: false,
  syncedLyrics: "[00:01.00]Original",
  romanizedSyncedLyrics: "[00:01.00]Romanized",
  plainLyrics: "Original",
  ...overrides,
});

describe("playback variants", () => {
  const media = (durationMs: number | null) => ({
    hasSession: true,
    artist: "Artist",
    title: "Song",
    durationMs,
  });

  it("normalizes official video metadata and synthetic channel names", () => {
    expect(
      normalizeLyricsMetadata({
        artist: "ExampleArtistVEVO",
        title: "Example Artist - The Song (Official Music Video)",
      }),
    ).toEqual({ artist: "Example Artist", title: "The Song" });
  });

  it("keeps unrelated artist-title text that has no video descriptor", () => {
    expect(normalizeLyricsMetadata({ artist: "Uploader", title: "Artist - Song" })).toEqual({
      artist: "Uploader",
      title: "Artist - Song",
    });
  });

  it("matches cached durations at the inclusive three-second boundary", () => {
    expect(
      isSameCachedVariant(playbackVariant(media(180_000))!, playbackVariant(media(183_000))!),
    ).toBe(true);
    expect(
      isSameCachedVariant(playbackVariant(media(180_000))!, playbackVariant(media(183_001))!),
    ).toBe(false);
  });

  it("keeps a known active variant when duration disappears, then refreshes when it becomes known", () => {
    const known = playbackVariant(media(180_000));
    const unknown = playbackVariant(media(null));
    expect(startsNewPlaybackVariant(known, unknown)).toBe(false);
    expect(startsNewPlaybackVariant(unknown, known)).toBe(true);
  });

  it("treats materially different durations as new active variants", () => {
    expect(
      startsNewPlaybackVariant(playbackVariant(media(180_000)), playbackVariant(media(190_000))),
    ).toBe(true);
  });
});

describe("LRC parsing", () => {
  it("parses fractions, strips word tags, and ignores metadata", () => {
    const lines = parseLyrics("[ar:Artist]\n[00:01.2]<00:01.20>Hello   world");
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatchObject({
      timeMs: 1_200,
      text: "Hello world",
      words: ["Hello", "world"],
    });
  });

  it("inserts an introduction only after the three-second boundary", () => {
    expect(parseLyrics("[00:03.00]Hello")[0].text).toBe("Hello");
    expect(parseLyrics("[00:03.01]Hello")[0]).toMatchObject({ timeMs: 0, text: "♪" });
  });

  it("keeps empty timed lines as instrumental breaks", () => {
    expect(parseLyrics("[00:00.00]")[0]).toMatchObject({ timeMs: 0, text: "♪", words: [] });
  });

  it("ends a line at the next timestamp and estimates the final line", () => {
    const lines = parseLyrics("[00:00.00]First line\n[00:05.00]Last");
    expect(lines[0].endTimeMs).toBe(5_000);
    expect(lines[1].endTimeMs).toBe(6_400);
  });
});

describe("lyrics display selection", () => {
  it("prefers romanized synchronized lyrics when enabled", () => {
    expect(selectLyricsDisplay(result(), "Song", true).lines[0].text).toBe("Romanized");
    expect(selectLyricsDisplay(result(), "Song", false).lines[0].text).toBe("Original");
  });

  it("shows a variant notice instead of unsynchronized fallback lyrics", () => {
    expect(
      selectLyricsDisplay(
        result({ syncedLyrics: null, romanizedSyncedLyrics: null }),
        "Song (slowed)",
        true,
      ),
    ).toMatchObject({ mode: "excluded", notice: "Slowed - No Lyrics", lines: [] });
  });

  it("recognizes instrumental and combined variant titles", () => {
    expect(getLocalLyricsNotice("Song instrumental")).toBe("Instrumental");
    expect(selectLyricsDisplay(null, "Song instrumental", true)).toMatchObject({
      mode: "instrumental",
      notice: "Instrumental",
    });
    expect(getLocalLyricsNotice("Song slowed down + reverberated")).toBe(
      "Slowed + Reverb - No Lyrics",
    );
  });

  it("represents LRCLIB instrumental results explicitly", () => {
    expect(selectLyricsDisplay(result({ instrumental: true }), "Song", true)).toMatchObject({
      mode: "instrumental",
      lines: [{ text: "Instrumental" }],
    });
  });
});
