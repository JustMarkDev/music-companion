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
 * Gives freshly word-synced lyrics the romanization and translation the song
 * had before, which the sync does not return. Only done when both versions have
 * the same lines, so a track is never put on the wrong ones.
 */
export function carryOverTracks(synced: LyricsResult, previous: LyricsResult | null | undefined) {
  if (!previous || hasRomanization(synced) || hasTranslation(synced)) return synced;
  if (!hasRomanization(previous) && !hasTranslation(previous)) return synced;
  if (previous.lines.length !== synced.lines.length) return synced;
  const aligned = synced.lines.every(
    (line, index) =>
      Math.abs(line.startMs - previous.lines[index].startMs) <= CARRY_OVER_TOLERANCE_MS,
  );
  if (!aligned) return synced;

  return {
    ...synced,
    lines: synced.lines.map((line, index) => {
      const { romanized, translation } = previous.lines[index];
      return {
        ...line,
        ...(romanized ? { romanized } : {}),
        ...(translation ? { translation } : {}),
      };
    }),
  };
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
  // A line the romanization skips keeps its original text.
  const shown = romanizedLyrics && line.romanized ? line.romanized : line;
  const text = shown.segments
    .map((segment) => segment.text)
    .join("")
    .replace(/\s+/g, " ")
    .trim();
  const lyricLine: LyricLine = {
    timeMs: line.startMs,
    endTimeMs: Math.max(line.endMs, line.startMs + MIN_LINE_DURATION_MS),
    text: text || INSTRUMENTAL_BREAK_ICON,
    words: text.match(/\S+/g) ?? [],
    voice: line.voice,
  };
  if (wordTimed && shown.segments.length > 0) lyricLine.segments = shown.segments;
  if (shown.background?.length) lyricLine.background = shown.background;
  if (line.translation) lyricLine.translation = line.translation;
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
