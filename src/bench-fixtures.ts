/** Shared LRC and metadata fixtures for Vitest microbenchmarks. */

export function buildSyncedLyrics(lineCount = 80): string {
  const lines: string[] = ["[ar:Bench Artist]", "[ti:Bench Track]"];
  for (let index = 0; index < lineCount; index += 1) {
    const totalSeconds = Math.floor(index * 2.4);
    const minutes = String(Math.floor(totalSeconds / 60)).padStart(2, "0");
    const seconds = String(totalSeconds % 60).padStart(2, "0");
    const fraction = String((index * 37) % 100).padStart(2, "0");
    lines.push(
      `[${minutes}:${seconds}.${fraction}]<${minutes}:${seconds}.${fraction}>Line ${index} with a few more words`,
    );
  }
  return lines.join("\n");
}

export const NOISY_METADATA = {
  artist: "ExampleArtistVEVO",
  title: "Example Artist - The Song (Official Music Video)",
} as const;

export const SYNCED_RESULT = {
  source: "LRCLIB",
  trackName: "Bench Track",
  artistName: "Bench Artist",
  albumName: "Bench Album",
  duration: 192,
  instrumental: false,
  syncedLyrics: buildSyncedLyrics(40),
  romanizedSyncedLyrics: buildSyncedLyrics(40),
  plainLyrics: "Bench Track",
};
