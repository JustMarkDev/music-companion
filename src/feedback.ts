import type { LyricsMode } from "./lyrics";

/** Where feedback goes: the repository's new-issue page, filled in by the app. */
export const FEEDBACK_ISSUES_URL = "https://github.com/JustMarkDev/music-companion/issues/new";

/** GitHub refuses addresses much longer than this, so a report stays well under it. */
const MAX_URL_LENGTH = 6_000;

export type FeedbackReport = {
  appVersion: string;
  platform: string;
  language: string;
  lyricsScript: string;
  wordSync: boolean;
  /** What the player reports; null when nothing is playing. */
  song: {
    title: string;
    artist: string;
    album: string;
    durationMs: number | null;
    player: string;
  } | null;
  lyricsMode: LyricsMode;
  /** What the app found for the song; null when it found nothing. */
  lyrics: {
    trackName: string;
    artistName: string;
    albumName: string;
    wordTimed: boolean;
    /** Which service the lyrics came from. */
    source?: string;
    lines: number;
  } | null;
};

const RESULTS: Record<LyricsMode, string> = {
  synced: "Lyrics shown",
  instrumental: "Marked instrumental",
  excluded: "Skipped (remix, live, cover...)",
  searching: "Still searching",
  missing: "No lyrics found",
  error: "The search failed",
};

function formatLength(durationMs: number | null) {
  if (durationMs === null || !Number.isFinite(durationMs) || durationMs <= 0) return "unknown";
  const seconds = Math.round(durationMs / 1_000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** Text that is safe on one line of a markdown list. */
function inline(value: string) {
  return value.replace(/\s+/g, " ").trim() || "unknown";
}

/**
 * The issue a report becomes: always the same sections, so every report can be read
 * the same way. It is in English whatever the app's language, for the maintainer.
 */
export function buildFeedbackIssue(report: FeedbackReport) {
  const song = report.song;
  const title = song
    ? `Lyrics: ${inline(song.title)} - ${inline(song.artist)}`
    : "Feedback from the app";
  const lines = [
    "<!-- Sent from Music Companion. Check the details, say what is wrong, and press Submit new issue. -->",
    "",
    "### What is wrong?",
    "",
    "<!-- Wrong lyrics / no lyrics / out of sync / missing romanization / something else. A few words are enough. -->",
    "",
    "### Song",
    "",
    ...(song
      ? [
          `- Title: ${inline(song.title)}`,
          `- Artist: ${inline(song.artist)}`,
          `- Album: ${inline(song.album)}`,
          `- Length: ${formatLength(song.durationMs)}`,
          `- Player: ${inline(song.player)}`,
        ]
      : ["- Nothing was playing"]),
    "",
    "### What the app found",
    "",
    `- Result: ${RESULTS[report.lyricsMode]}`,
    ...(report.lyrics
      ? [
          `- Matched: ${inline(report.lyrics.trackName)} - ${inline(report.lyrics.artistName)}`,
          `- Album: ${inline(report.lyrics.albumName)}`,
          `- Source: ${report.lyrics.source ?? "unknown"}`,
          `- Timing: ${report.lyrics.wordTimed ? "word by word" : "line by line"}`,
          `- Lines: ${report.lyrics.lines}`,
        ]
      : []),
    "",
    "### App",
    "",
    `- Version: ${report.appVersion} (${report.platform})`,
    `- Interface language: ${report.language}`,
    `- Lyrics shown as: ${report.lyricsScript}`,
    `- Word sync with AI: ${report.wordSync ? "on" : "off"}`,
  ];
  const body = lines.join("\n");
  const url = new URL(FEEDBACK_ISSUES_URL);
  url.searchParams.set("title", title);
  url.searchParams.set("body", body);
  url.searchParams.set("labels", "lyrics-feedback");
  // A long song name could push the address over the limit; the details come first.
  const address =
    url.toString().length <= MAX_URL_LENGTH
      ? url.toString()
      : `${FEEDBACK_ISSUES_URL}?${new URLSearchParams({ title: title.slice(0, 200), body: body.slice(0, 2_500) })}`;
  return { title, body, url: address };
}
