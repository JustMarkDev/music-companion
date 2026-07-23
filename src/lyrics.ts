export const PLAYBACK_VARIANT_TOLERANCE_MS = 3_000;
const INTRODUCTION_THRESHOLD_MS = 3_000;
const INSTRUMENTAL_BREAK_ICON = "♪";

export type LyricsResult = {
  source: string;
  trackName: string;
  artistName: string;
  albumName: string;
  duration: number | null;
  instrumental: boolean;
  syncedLyrics: string | null;
  romanizedSyncedLyrics?: string | null;
  plainLyrics: string | null;
};

export type LyricLine = {
  timeMs: number | null;
  endTimeMs: number | null;
  text: string;
  words: string[];
};

export type LyricsMode =
  | "synced"
  | "unsynced"
  | "instrumental"
  | "excluded"
  | "searching"
  | "missing"
  | "error";

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

  if (!result) {
    if (currentNotice === "Instrumental") {
      return { lines: [], mode: "instrumental", notice: "Instrumental" };
    }
    return {
      lines: [],
      mode: variantFallback ? "excluded" : "missing",
      notice: variantFallback ?? "",
    };
  }
  if (result.instrumental) {
    return { lines: [createLyricLine(null, "Instrumental")], mode: "instrumental", notice: "" };
  }

  const displayed =
    romanizedLyrics && result.romanizedSyncedLyrics
      ? result.romanizedSyncedLyrics
      : result.syncedLyrics;
  if (displayed) return { lines: parseLyrics(displayed), mode: "synced", notice: "" };
  if (result.plainLyrics) {
    return {
      lines: [],
      mode: variantFallback ? "excluded" : "unsynced",
      notice: variantFallback ?? "No Synced Lyrics",
    };
  }
  return {
    lines: [],
    mode: variantFallback ? "excluded" : "missing",
    notice: variantFallback ?? "",
  };
}

export function parseLyrics(raw: string): LyricLine[] {
  const lines: LyricLine[] = [];
  const pattern = /\[(\d{1,2}):(\d{2})(?:[.:](\d{1,3}))?\]/g;
  const metadataPattern = /^\[[a-z]+:/i;

  for (const rawLine of raw.split(/\r?\n/)) {
    if (metadataPattern.test(rawLine.trim())) continue;
    const matches = [...rawLine.matchAll(pattern)];
    const textWithWordTags = rawLine.replace(pattern, "").trim();
    if (matches.length === 0 && textWithWordTags) {
      lines.push(createLyricLine(null, textWithWordTags));
      continue;
    }
    for (const match of matches) {
      lines.push(createLyricLine(parseTimeParts(match[1], match[2], match[3]), textWithWordTags));
    }
  }

  const sorted = lines.sort((a, b) => (a.timeMs ?? 0) - (b.timeMs ?? 0));
  const firstTimed = sorted.find((line) => line.timeMs !== null);
  if (firstTimed && firstTimed.timeMs! > INTRODUCTION_THRESHOLD_MS) {
    sorted.unshift(createLyricLine(0, ""));
  }
  return finalizeLyricTimings(sorted);
}

function createLyricLine(timeMs: number | null, textWithWordTags: string): LyricLine {
  const text = textWithWordTags
    .replace(/<\d{1,2}:\d{2}(?:[.:]\d{1,3})?>/g, "")
    .replace(/\s+/g, " ")
    .trim();
  return {
    timeMs,
    endTimeMs: null,
    text: text || INSTRUMENTAL_BREAK_ICON,
    words: text.match(/\S+/g) ?? [],
  };
}

function finalizeLyricTimings(lines: LyricLine[]) {
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.timeMs === null) continue;
    const next = lines.slice(index + 1).find((candidate) => candidate.timeMs !== null);
    const estimated = clamp(line.words.length * 460, 1400, 7200);
    line.endTimeMs = Math.max(line.timeMs + 320, next?.timeMs ?? line.timeMs + estimated);
  }
  return lines;
}

function parseTimeParts(minutes: string, seconds: string, fraction = "0") {
  return (
    Number(minutes) * 60_000 + Number(seconds) * 1_000 + Number(fraction.padEnd(3, "0").slice(0, 3))
  );
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

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}
