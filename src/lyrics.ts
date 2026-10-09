export const PLAYBACK_VARIANT_TOLERANCE_MS = 3_000;
const INTRODUCTION_THRESHOLD_MS = 3_000;
const MIN_LINE_DURATION_MS = 320;
const INSTRUMENTAL_BREAK_ICON = "♪";
/** How far a carried-over track may start from the line it is put on. */
const CARRY_OVER_TOLERANCE_MS = 3_000;

/** One timed piece of a line: a word, or a syllable when the source splits words. */
export type LyricSegment = {
  startMs: number;
  endMs: number;
  /** Includes the spacing that follows it, so segments concatenate to the line. */
  text: string;
};

/** A line written in one script: its lead vocal and any background vocals. */
export type LyricsLineText = {
  segments: LyricSegment[];
  background?: LyricSegment[];
};

/** A line as the backend sends it. */
export type LyricsResultLine = LyricsLineText & {
  startMs: number;
  endMs: number;
  /** 0 for the lead singer, 1 for the other singer of a duet. */
  voice: number;
  romanized?: LyricsLineText;
  translation?: string;
};

export type LyricsResult = {
  trackName: string;
  artistName: string;
  albumName: string;
  duration: number | null;
  /** True when some line times its individual words. */
  wordTimed: boolean;
  lines: LyricsResultLine[];
};

export type LyricLine = {
  timeMs: number;
  endTimeMs: number;
  text: string;
  words: string[];
  voice: number;
  /** Present only when the source timed individual words. */
  segments?: LyricSegment[];
  background?: LyricSegment[];
  translation?: string;
};

export type LyricsMode = "synced" | "instrumental" | "excluded" | "searching" | "missing" | "error";

export type PlaybackVariant = {
  metadataKey: string;
  durationMs: number | null;
};

type MediaMetadata = {
  hasSession: boolean;
  artist: string;
  title: string;
  durationMs: number | null;
};

export type LyricsDisplay = {
  lines: LyricLine[];
  mode: LyricsMode;
  notice: string;
};

export function playbackVariant(media: MediaMetadata): PlaybackVariant | null {
  if (!media.hasSession || !media.title) return null;
  const metadata = normalizeLyricsMetadata(media);
  return {
    metadataKey: `${normalizeTrackField(metadata.artist)}::${normalizeTrackField(metadata.title)}`,
    durationMs: validDuration(media.durationMs),
  };
}

export function isSameCachedVariant(left: PlaybackVariant, right: PlaybackVariant) {
  if (left.metadataKey !== right.metadataKey) return false;
  if (left.durationMs === null || right.durationMs === null) {
    return left.durationMs === right.durationMs;
  }
  return Math.abs(left.durationMs - right.durationMs) <= PLAYBACK_VARIANT_TOLERANCE_MS;
}

export function isSameSong(current: PlaybackVariant | null, next: PlaybackVariant | null) {
  return Boolean(current && next && current.metadataKey === next.metadataKey);
}

export function startsNewPlaybackVariant(
  current: PlaybackVariant | null,
  next: PlaybackVariant | null,
) {
  if (!current || !next) return current !== next;
  if (current.metadataKey !== next.metadataKey) return true;
  if (next.durationMs === null) return false;
  if (current.durationMs === null) return true;
  return Math.abs(current.durationMs - next.durationMs) > PLAYBACK_VARIANT_TOLERANCE_MS;
}

export function variantToken(variant: PlaybackVariant) {
  return `${variant.metadataKey}::${variant.durationMs ?? "unknown"}`;
}

export function normalizeLyricsMetadata(media: Pick<MediaMetadata, "artist" | "title">) {
  const artist = normalizeLyricsArtist(media.artist);
  const title = media.title.trim();
  const normalizedTitle = normalizeLyricsTitle(title);
  const combinedTitle = normalizedTitle.match(/^(.+?)\s+[-\u2013\u2014]\s+(.+)$/);

  if (!combinedTitle) return { artist, title: normalizedTitle };

  const titleArtist = combinedTitle[1].trim();
  const songTitle = combinedTitle[2].trim();
  const hasVideoDescriptor = normalizedTitle !== title;
  if (
    !hasVideoDescriptor &&
    normalizeArtistComparison(titleArtist) !== normalizeArtistComparison(artist)
  ) {
    return { artist, title: normalizedTitle };
  }

  return { artist: titleArtist, title: songTitle };
}

export function normalizeDisplayMetadata(media: Pick<MediaMetadata, "artist" | "title">) {
  const metadata = normalizeLyricsMetadata(media);
  return { ...metadata, title: normalizeLyricsTitle(metadata.title) };
}

export function getLocalLyricsNotice(title: string): string | null {
  if (/\binstrumental\b/i.test(title)) return "Instrumental";

  const slowed = /\bslowed(?:\s+down)?\b/i.test(title);
  const reverb = /\breverb(?:erated)?\b/i.test(title);
  if (slowed && reverb) return "Slowed + Reverb - No Lyrics";

  const variants: Array<[RegExp, string]> = [
    [/\bslowed(?:\s+down)?\b/i, "Slowed"],
    [/\breverb(?:erated)?\b/i, "Reverb"],
    [/\bremix(?:ed)?\b/i, "Remix"],
    [/\bsped[\s-]*up\b|\bspeed[\s-]*up\b/i, "Sped Up"],
    [/\bnightcore\b/i, "Nightcore"],
    [/\bkaraoke\b/i, "Karaoke"],
    [/(?:[([\-–—]\s*live\b|\blive\s+(?:at|from|version|session)\b)/i, "Live"],
    [/\bcover\b/i, "Cover"],
  ];
  const match = variants.find(([pattern]) => pattern.test(title));
  return match ? `${match[1]} - No Lyrics` : null;
}

/**
 * Display policy for scripts and translations.
 *
 * A song has a single main line: the romanization when Romanized mode is on and
 * lrc.red provides one for the line, otherwise the original. The kanji and its
 * romanization are never shown as two main lines. lrc.red's translation, when
 * the user enables it, is an optional secondary line under the main one — also
 * in Romanized mode — so romanized and translated can co-occur, but only as
 * main line plus sub-line. A translation that only echoes the line is dropped
 * (see `isRedundantTranslation`).
 */
export function selectLyricsDisplay(
  result: LyricsResult | null,
  title: string,
  romanizedLyrics: boolean,
  fallbackNotice: string | null = null,
): LyricsDisplay {
  const currentNotice = fallbackNotice ?? getLocalLyricsNotice(title);
  const variantFallback = currentNotice === "Instrumental" ? null : currentNotice;

  if (!result || result.lines.length === 0) {
    if (currentNotice === "Instrumental") {
      return { lines: [], mode: "instrumental", notice: "Instrumental" };
    }
    return {
      lines: [],
      mode: variantFallback ? "excluded" : "missing",
      notice: variantFallback ?? "",
    };
  }

  return { lines: buildLines(result, romanizedLyrics), mode: "synced", notice: "" };
}

/** True when at least one line has a romanization. */
export function hasRomanization(result: LyricsResult | null | undefined) {
  return Boolean(result?.lines.some((line) => line.romanized));
}

/** True when at least one line has a translation. */
export function hasTranslation(result: LyricsResult | null | undefined) {
  return Boolean(result?.lines.some((line) => line.translation));
}

/**
 * Gives freshly word-synced lyrics what the song had before and the sync does not
 * carry: its romanization, translation, background vocals and the singer of each
 * line. Each is carried on its own, so one the synced line already has does not
 * stop the others. Two lines are paired when each is the other's nearest in time,
 * within a few seconds, so nothing is put on a line it does not belong to; what is
 * carried is re-timed to the synced line.
 */
export function carryOverTracks(synced: LyricsResult, previous: LyricsResult | null | undefined) {
  if (!previous) return synced;

  const forward = synced.lines.map((line) => nearestLine(line.startMs, previous.lines));
  const backward = previous.lines.map((line) => nearestLine(line.startMs, synced.lines));
  let carried = false;
  const lines = synced.lines.map((line, index) => {
    const match = forward[index];
    if (match < 0 || backward[match] !== index) return line;
    const earlier = previous.lines[match];
    const carriedLine = carryOverLine(line, earlier);
    if (carriedLine !== line) carried = true;
    return carriedLine;
  });
  return carried ? { ...synced, lines } : synced;
}

/** `line` with what `earlier`, the same line before a sync, had and it lacks. */
function carryOverLine(line: LyricsResultLine, earlier: LyricsResultLine): LyricsResultLine {
  const extras: Partial<LyricsResultLine> = {};
  if (earlier.romanized && !line.romanized) {
    extras.romanized = {
      segments: retimeSegments(earlier.romanized.segments, earlier, line),
      ...(earlier.romanized.background?.length
        ? { background: retimeSegments(earlier.romanized.background, earlier, line) }
        : {}),
    };
  }
  if (earlier.translation && !line.translation) extras.translation = earlier.translation;
  if (earlier.background?.length && !line.background?.length) {
    extras.background = retimeSegments(earlier.background, earlier, line);
  }
  if (earlier.voice > 0 && line.voice === 0) extras.voice = earlier.voice;
  return Object.keys(extras).length > 0 ? { ...line, ...extras } : line;
}

/** Maps times within the span of the line `from` onto the span of the line `to`. */
function retimeSegments(
  segments: LyricSegment[],
  from: Pick<LyricsResultLine, "startMs" | "endMs">,
  to: Pick<LyricsResultLine, "startMs" | "endMs">,
): LyricSegment[] {
  const length = from.endMs - from.startMs;
  const scale = length > 0 ? (to.endMs - to.startMs) / length : 1;
  const at = (ms: number) => Math.max(0, Math.round(to.startMs + (ms - from.startMs) * scale));
  return segments.map((segment) => ({
    ...segment,
    startMs: at(segment.startMs),
    endMs: at(segment.endMs),
  }));
}

/** Index of the line that starts closest to `startMs`, or -1 when none is within tolerance. */
function nearestLine(startMs: number, lines: LyricsResultLine[]) {
  let nearest = -1;
  let nearestDistance = CARRY_OVER_TOLERANCE_MS + 1;
  lines.forEach((line, index) => {
    const distance = Math.abs(line.startMs - startMs);
    if (distance < nearestDistance) {
      nearest = index;
      nearestDistance = distance;
    }
  });
  return nearest;
}

/**
 * The line as lrc.red breaks it: each segment carries its trailing space, so
 * adjacent syllable spans concatenate ("kowa" + "re" is "koware") while spaced
 * spans stay apart. Runs of whitespace collapse to one; the line is trimmed.
 */
function joinSegments(segments: LyricSegment[]) {
  return segments
    .map((segment) => segment.text)
    .join("")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * Folds a line for echo comparison: single-spaced, trimmed, lowercased.
 */
function foldLineText(value: string) {
  return value.replace(/\s+/g, " ").trim().toLowerCase();
}

/**
 * True when every letter of the line is Latin, accents included (mirroring the
 * provider match in `src-tauri/src/lyrics.rs`); digits, punctuation and spacing
 * are ignored. Needs at least one letter, so "♪" or "…" are never Latin-only.
 */
export function isLatinOnlyLine(value: string) {
  const letters = Array.from(value).filter((character) => /\p{L}/u.test(character));
  return (
    letters.length > 0 &&
    letters.every((character) => /[A-Za-z\u00C0-\u024F\u1E00-\u1EFF]/u.test(character))
  );
}

/** Below this edit similarity a Latin translation is a real translation, not an echo. */
const REDUNDANT_TRANSLATION_SIMILARITY = 0.6;

/** Edit similarity of two folded lines: 0 for nothing alike, 1 for identical. */
function lineSimilarity(left: string, right: string) {
  const a = Array.from(foldLineText(left));
  const b = Array.from(foldLineText(right));
  if (a.length === 0 || b.length === 0) return a.length === b.length ? 1 : 0;
  let previous = Array.from({ length: b.length + 1 }, (_, index) => index);
  for (let i = 1; i <= a.length; i += 1) {
    const current = [i];
    for (let j = 1; j <= b.length; j += 1) {
      current[j] = Math.min(
        previous[j] + 1,
        current[j - 1] + 1,
        previous[j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1),
      );
    }
    previous = current;
  }
  return 1 - previous[b.length] / Math.max(a.length, b.length);
}

/**
 * True when showing `translation` under the line would only repeat it, so the
 * sub-line is dropped. A translation matching the displayed (possibly
 * romanized) line is always redundant. Otherwise the original decides: lrc.red
 * echoes Latin-script lines into its English translation track ("just u…" as
 * "Just you…") instead of translating them, so a Latin-only original with a
 * near-identical Latin translation is redundant too. Genuine translations into
 * another language ("Des yeux…" into "Eyes that…") share little surface text
 * and are kept.
 */
export function isRedundantTranslation(
  displayedText: string,
  translation: string,
  originalText: string = displayedText,
) {
  if (foldLineText(displayedText) === foldLineText(translation)) return true;
  return (
    isLatinOnlyLine(originalText) &&
    isLatinOnlyLine(translation) &&
    lineSimilarity(originalText, translation) >= REDUNDANT_TRANSLATION_SIMILARITY
  );
}

/**
 * Gives each word of a romanization the time of the original words it sits
 * under. Romanized words do not pair with the original ones, so the line is
 * laid out by text length: a word starts where the share of the original text
 * that precedes it is played. Words are re-split on lrc.red's own breaks —
 * adjacent syllable spans concatenate while spaced spans stay apart — and
 * rejoined with single spaces, so the rebuilt line keeps the provider's
 * spacing between syllables and words.
 */
export function retimeRomanization(
  original: LyricSegment[],
  romanized: LyricSegment[],
): LyricSegment[] {
  const words = joinSegments(romanized).match(/\S+/g) ?? [];
  const weights = original.map((segment) => Array.from(segment.text.replace(/\s+/g, "")).length);
  const total = weights.reduce((sum, weight) => sum + weight, 0);
  if (words.length === 0 || total === 0) return romanized;

  // The time at which `share` (0-1) of the original text has been played. Where one
  // original word ends and the next begins, a word's start takes the next one's time
  // and its end the previous one's.
  const timeAt = (share: number, isEnd: boolean) => {
    let passed = 0;
    const target = share * total;
    for (let index = 0; index < original.length; index += 1) {
      if (weights[index] === 0) continue;
      const reached = isEnd ? target <= passed + weights[index] : target < passed + weights[index];
      if (reached || index === original.length - 1) {
        const { startMs, endMs } = original[index];
        const within = Math.min(1, Math.max(0, (target - passed) / weights[index]));
        return Math.round(startMs + within * (endMs - startMs));
      }
      passed += weights[index];
    }
    return original[original.length - 1].endMs;
  };

  const letters = words.reduce((sum, word) => sum + Array.from(word).length, 0);
  let before = 0;
  return words.map((word, index) => {
    const startMs = timeAt(before / letters, false);
    before += Array.from(word).length;
    return {
      startMs,
      endMs: timeAt(before / letters, true),
      text: index < words.length - 1 ? `${word} ` : word,
    };
  });
}

function buildLines(result: LyricsResult, romanizedLyrics: boolean): LyricLine[] {
  const lines = result.lines.map((line) =>
    createLyricLine(line, result.wordTimed, romanizedLyrics),
  );
  const first = lines[0];
  if (first.timeMs > INTRODUCTION_THRESHOLD_MS) {
    lines.unshift({
      timeMs: 0,
      endTimeMs: first.timeMs,
      text: INSTRUMENTAL_BREAK_ICON,
      words: [],
      voice: 0,
    });
  }
  return lines;
}

function createLyricLine(
  line: LyricsResultLine,
  wordTimed: boolean,
  romanizedLyrics: boolean,
): LyricLine {
  // A line the romanization skips keeps its original text. Either way there is
  // one main line, never the original beside its romanization.
  const shown = romanizedLyrics && line.romanized ? line.romanized : line;
  // A romanization timed as a whole, under words that are timed one by one, moves with them.
  const segments =
    shown !== line && wordTimed && shown.segments.length < line.segments.length
      ? retimeRomanization(line.segments, shown.segments)
      : shown.segments;
  const text = joinSegments(segments);
  const lyricLine: LyricLine = {
    timeMs: line.startMs,
    endTimeMs: Math.max(line.endMs, line.startMs + MIN_LINE_DURATION_MS),
    text: text || INSTRUMENTAL_BREAK_ICON,
    words: text.match(/\S+/g) ?? [],
    voice: line.voice,
  };
  if (wordTimed && segments.length > 0) lyricLine.segments = segments;
  // A transliteration of the lead vocal has no background vocals of its own.
  const background = shown.background?.length ? shown.background : line.background;
  if (background?.length) lyricLine.background = background;
  // The translation stays an optional secondary line (rendered by the overlay
  // only when the setting is on); one that only echoes the line is dropped.
  if (line.translation) {
    const originalText = joinSegments(line.segments);
    if (!isRedundantTranslation(text, line.translation, originalText)) {
      lyricLine.translation = line.translation;
    }
  }
  return lyricLine;
}

function normalizeLyricsTitle(title: string) {
  let normalized = title.trim();
  while (true) {
    const start = Math.max(normalized.lastIndexOf("("), normalized.lastIndexOf("["));
    if (start < 0) return normalized;
    const closing = normalized[start] === "(" ? ")" : "]";
    if (!normalized.endsWith(closing)) return normalized;
    if (!isVideoDescriptor(normalized.slice(start + 1, -1))) return normalized;
    normalized = normalized.slice(0, start).trimEnd();
  }
}

function isVideoDescriptor(label: string) {
  const normalized = label.trim();
  return (
    /^(?:official\s+)?(?:(?:music|lyric(?:s)?|hd|4k)\s+)*video(?:\s+(?:hd|4k))?$/i.test(
      normalized,
    ) || /^(?:official\s+)?(?:audio|visuali[sz]er)$/i.test(normalized)
  );
}

function normalizeLyricsArtist(artist: string) {
  const synthetic = /(?:[-\u2013\u2014]\s*topic|vevo)\s*$/i.test(artist);
  const normalized = artist.replace(/\s*(?:[-\u2013\u2014]\s*topic|vevo)\s*$/i, "").trim();
  if (!synthetic) return normalized;
  return normalized
    .replace(/([\p{Ll}\p{N}])(\p{Lu})/gu, "$1 $2")
    .replace(/(\p{Lu})(\p{Lu}\p{Ll})/gu, "$1 $2");
}

function normalizeArtistComparison(artist: string) {
  return normalizeTrackField(artist).replace(/[^\p{L}\p{N}]/gu, "");
}

function normalizeTrackField(value: string) {
  return value.trim().replace(/\s+/g, " ").toLowerCase();
}

function validDuration(value: number | null) {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : null;
}
