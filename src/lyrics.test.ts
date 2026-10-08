import { describe, expect, it } from "vite-plus/test";
import {
  carryOverTracks,
  getLocalLyricsNotice,
  hasRomanization,
  hasTranslation,
  isSameCachedVariant,
  isSameSong,
  normalizeLyricsMetadata,
  playbackVariant,
  retimeRomanization,
  selectLyricsDisplay,
  startsNewPlaybackVariant,
  type LyricsResult,
  type LyricsResultLine,
  type LyricSegment,
} from "./lyrics";

const segment = (startMs: number, endMs: number, text: string): LyricSegment => ({
  startMs,
  endMs,
  text,
});

/** A line of timed words, each lasting 500 ms from `startMs`. */
const line = (
  startMs: number,
  words: string[],
  extras: Partial<LyricsResultLine> = {},
): LyricsResultLine => ({
  startMs,
  endMs: startMs + words.length * 500,
  voice: 0,
  segments: words.map((word, index) =>
    segment(
      startMs + index * 500,
      startMs + (index + 1) * 500,
      index < words.length - 1 ? `${word} ` : word,
    ),
  ),
  ...extras,
});

const result = (overrides: Partial<LyricsResult> = {}): LyricsResult => ({
  trackName: "Song",
  artistName: "Artist",
  albumName: "Album",
  duration: 180,
  wordTimed: true,
  lines: [
    line(1_000, ["Original", "words"], {
      romanized: { segments: [segment(1_000, 2_000, "Romanized")] },
      translation: "Translated",
    }),
  ],
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

  it("uses duration for lyric variants without breaking song continuity", () => {
    const current = playbackVariant(media(180_000));
    const next = playbackVariant(media(190_000));
    expect(startsNewPlaybackVariant(current, next)).toBe(true);
    expect(isSameSong(current, next)).toBe(true);
  });
});

describe("lyrics display selection", () => {
  it("prefers the romanization of a line when enabled", () => {
    expect(selectLyricsDisplay(result(), "Song", true).lines[0].text).toBe("Romanized");
    expect(selectLyricsDisplay(result(), "Song", false).lines[0].text).toBe("Original words");
  });

  it("keeps the original text of a line the romanization skips", () => {
    const lyrics = result({ lines: [line(1_000, ["Skipped"]), result().lines[0]] });
    const displayed = selectLyricsDisplay(lyrics, "Song", true).lines;
    expect(displayed.map((displayedLine) => displayedLine.text)).toEqual(["Skipped", "Romanized"]);
  });

  it("times the words of a line with the romanized words", () => {
    const lyrics = result({
      lines: [
        line(1_000, ["夢", "なら"], {
          romanized: {
            segments: [segment(1_000, 1_500, "Yume "), segment(1_500, 2_000, "nara")],
          },
        }),
      ],
    });
    expect(selectLyricsDisplay(lyrics, "Song", true).lines[0]).toMatchObject({
      text: "Yume nara",
      words: ["Yume", "nara"],
      segments: [segment(1_000, 1_500, "Yume "), segment(1_500, 2_000, "nara")],
    });
  });

  it("moves a romanization timed as a whole with the words of the original", () => {
    const lyrics = result({
      lines: [
        line(1_000, ["あなた", "わかって"], {
          romanized: { segments: [segment(1_000, 2_000, "Anata wa wakatte")] },
        }),
      ],
    });

    const [displayed] = selectLyricsDisplay(lyrics, "Song", true).lines;

    expect(displayed.text).toBe("Anata wa wakatte");
    expect(displayed.segments?.map((piece) => piece.text)).toEqual(["Anata ", "wa ", "wakatte"]);
    // The words fill the line from its start to its end.
    expect(displayed.segments?.[0].startMs).toBe(1_000);
    expect(displayed.segments?.[2].endMs).toBe(2_000);
  });

  it("leaves a romanization that is already timed word by word alone", () => {
    const own = [segment(1_000, 1_500, "Yume "), segment(1_500, 2_000, "nara")];
    const lyrics = result({
      lines: [line(1_000, ["夢", "なら"], { romanized: { segments: own } })],
    });
    expect(selectLyricsDisplay(lyrics, "Song", true).lines[0].segments).toEqual(own);
  });

  it("keeps background vocals when the romanization has none of its own", () => {
    const background = [segment(1_200, 1_800, "(ooh)")];
    const lyrics = result({
      lines: [
        line(1_000, ["あなた"], {
          background,
          romanized: { segments: [segment(1_000, 1_500, "Anata")] },
        }),
      ],
    });

    expect(selectLyricsDisplay(lyrics, "Song", true).lines[0]).toMatchObject({
      text: "Anata",
      background,
    });
    expect(selectLyricsDisplay(lyrics, "Song", false).lines[0].background).toEqual(background);
  });

  it("shows the background vocals of the romanization when it has them", () => {
    const romanizedBackground = [segment(1_200, 1_800, "(uu)")];
    const lyrics = result({
      lines: [
        line(1_000, ["あなた"], {
          background: [segment(1_200, 1_800, "(ooh)")],
          romanized: {
            segments: [segment(1_000, 1_500, "Anata")],
            background: romanizedBackground,
          },
        }),
      ],
    });

    expect(selectLyricsDisplay(lyrics, "Song", true).lines[0].background).toEqual(
      romanizedBackground,
    );
  });

  it("fills words only for songs whose words are timed", () => {
    const wholeLine = result({
      wordTimed: false,
      lines: [{ ...line(1_000, ["Hello world"]), endMs: 3_000 }],
    });
    expect(selectLyricsDisplay(wholeLine, "Song", false).lines[0].segments).toBeUndefined();
    expect(selectLyricsDisplay(result(), "Song", false).lines[0].segments).toHaveLength(2);
  });

  it("carries the singer, background vocals and translation of each line", () => {
    const background = [segment(1_200, 1_800, "(ooh)")];
    const lyrics = result({
      lines: [line(1_000, ["Lead"], { voice: 1, background, translation: "Guida" })],
    });
    expect(selectLyricsDisplay(lyrics, "Song", false).lines[0]).toMatchObject({
      voice: 1,
      background,
      translation: "Guida",
    });

    const plain = result({ lines: [line(1_000, ["Plain"])] });
    expect(selectLyricsDisplay(plain, "Song", false).lines[0]).not.toHaveProperty("translation");
    expect(selectLyricsDisplay(plain, "Song", false).lines[0]).not.toHaveProperty("background");
  });

  it("inserts an introduction only after the three-second boundary", () => {
    const startingAt = (startMs: number) =>
      selectLyricsDisplay(result({ lines: [line(startMs, ["Hello"])] }), "Song", false).lines;
    expect(startingAt(3_000)[0].text).toBe("Hello");
    expect(startingAt(3_001)[0]).toMatchObject({
      timeMs: 0,
      endTimeMs: 3_001,
      text: "♪",
      words: [],
    });
  });

  it("ends a line where it ends, but not before a short line has been seen", () => {
    const lyrics = result({
      wordTimed: false,
      lines: [
        { ...line(0, ["First"]), endMs: 4_000 },
        { ...line(5_000, ["Blink"]), endMs: 5_050 },
      ],
    });
    const [first, second] = selectLyricsDisplay(lyrics, "Song", false).lines;
    expect(first.endTimeMs).toBe(4_000);
    expect(second.endTimeMs).toBe(5_320);
  });

  it("shows a variant notice for a title with no lyrics", () => {
    expect(selectLyricsDisplay(result({ lines: [] }), "Song (slowed)", true)).toMatchObject({
      mode: "excluded",
      notice: "Slowed - No Lyrics",
      lines: [],
    });
    expect(selectLyricsDisplay(null, "Song", true)).toMatchObject({ mode: "missing", lines: [] });
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
});

describe("romanization and translation", () => {
  it("is found when any line has one", () => {
    expect(hasRomanization(result())).toBe(true);
    expect(hasTranslation(result())).toBe(true);
    const plain = result({ lines: [line(1_000, ["Plain"])] });
    expect(hasRomanization(plain)).toBe(false);
    expect(hasTranslation(plain)).toBe(false);
    expect(hasRomanization(null)).toBe(false);
  });

  describe("after a word sync", () => {
    const synced = (starts: number[]) =>
      result({ lines: starts.map((start) => line(start, ["Synced", "words"])) });
    const previous = () =>
      result({
        wordTimed: false,
        lines: [
          { ...line(1_000, ["A"]), romanized: { segments: [segment(1_000, 1_500, "A")] } },
          { ...line(4_000, ["B"]), translation: "Bee" },
        ],
      });

    it("gives the synced lines the tracks the song had", () => {
      const carried = carryOverTracks(synced([1_200, 4_100]), previous());

      // The synced line lasts twice as long as the line it follows, and the track with it.
      expect(carried.lines[0].romanized).toEqual({ segments: [segment(1_200, 2_200, "A")] });
      expect(carried.lines[0].translation).toBeUndefined();
      expect(carried.lines[1].translation).toBe("Bee");
      expect(carried.lines[1].segments).toEqual(synced([1_200, 4_100]).lines[1].segments);
    });

    it("matches lines by time when the sync has a different number of them", () => {
      const carried = carryOverTracks(synced([1_300, 2_900, 4_100]), previous());

      expect(carried.lines[0].romanized).toEqual({ segments: [segment(1_300, 2_300, "A")] });
      expect(carried.lines[1].romanized).toBeUndefined();
      expect(carried.lines[1].translation).toBeUndefined();
      expect(carried.lines[2].translation).toBe("Bee");
    });

    it("never uses a track on a line that starts far from it", () => {
      const drifted = synced([1_000, 9_000]);
      const carried = carryOverTracks(drifted, previous());

      expect(carried.lines[0].romanized).toBeDefined();
      expect(carried.lines[1].translation).toBeUndefined();

      const nothingNear = synced([20_000]);
      expect(carryOverTracks(nothingNear, previous())).toBe(nothingNear);
    });

    it("carries each track on its own when the synced line already has the other", () => {
      const withTranslation = result({
        lines: [line(1_000, ["Synced", "words"], { translation: "Own translation" })],
      });
      const carried = carryOverTracks(withTranslation, previous());

      expect(carried.lines[0].translation).toBe("Own translation");
      expect(carried.lines[0].romanized).toBeDefined();

      const withRomanization = result({
        lines: [
          line(4_000, ["Synced", "words"], {
            romanized: { segments: [segment(4_000, 5_000, "B")] },
          }),
        ],
      });
      const other = carryOverTracks(withRomanization, previous());

      expect(other.lines[0].romanized).toEqual({ segments: [segment(4_000, 5_000, "B")] });
      expect(other.lines[0].translation).toBe("Bee");
    });

    it("keeps the singer and the background vocals of the song, timed to the synced line", () => {
      const duet = result({
        wordTimed: false,
        lines: [line(1_000, ["A"], { voice: 1, background: [segment(1_100, 1_300, "(oh)")] })],
      });
      const carried = carryOverTracks(synced([1_200]), duet);

      expect(carried.lines[0].voice).toBe(1);
      expect(carried.lines[0].background).toEqual([segment(1_400, 1_800, "(oh)")]);
    });

    it("leaves the sync alone when it has tracks of its own or the song had none", () => {
      const own = result();
      expect(carryOverTracks(own, previous())).toBe(own);

      const plain = synced([1_000, 4_000]);
      const untracked = result({ lines: [line(1_000, ["A"]), line(4_000, ["B"])] });
      expect(carryOverTracks(plain, untracked)).toBe(plain);
      expect(carryOverTracks(plain, null)).toBe(plain);
    });
  });
});

describe("retimeRomanization", () => {
  const original = [segment(0, 1_000, "ab"), segment(2_000, 4_000, "cd")];

  it("lays the romanized words over the original ones by text length", () => {
    const retimed = retimeRomanization(original, [segment(0, 4_000, "aa bb cc dd")]);

    expect(retimed).toEqual([
      segment(0, 500, "aa "),
      segment(500, 1_000, "bb "),
      segment(2_000, 3_000, "cc "),
      segment(3_000, 4_000, "dd"),
    ]);
  });

  it("lets a word that spans two original words run across the gap", () => {
    const retimed = retimeRomanization(original, [segment(0, 4_000, "aaaaaa bb")]);

    expect(retimed.map((word) => [word.startMs, word.endMs])).toEqual([
      [0, 3_000],
      [3_000, 4_000],
    ]);
  });

  it("keeps the romanization as it is when there is nothing to time it by", () => {
    const whole = [segment(0, 1_000, "kyou")];
    expect(retimeRomanization([segment(0, 1_000, " ")], whole)).toBe(whole);
    expect(retimeRomanization(original, [segment(0, 1_000, " ")])).toEqual([
      segment(0, 1_000, " "),
    ]);
  });
});
