/** Shared lyrics and metadata fixtures for Vitest microbenchmarks. */
import type { LyricsResult } from "./lyrics";

export function buildLyricsResult(lineCount = 80): LyricsResult {
  const lines = Array.from({ length: lineCount }, (_, index) => {
    const startMs = Math.floor(index * 2_400);
    const words = ["Line", String(index), "with", "a", "few", "more", "words"];
    const segments = words.map((word, wordIndex) => ({
      startMs: startMs + wordIndex * 300,
      endMs: startMs + (wordIndex + 1) * 300,
      text: wordIndex < words.length - 1 ? `${word} ` : word,
    }));
    return {
      startMs,
      endMs: startMs + words.length * 300,
      voice: index % 7 === 0 ? 1 : 0,
      segments,
      romanized: { segments },
      translation: `Translation of line ${index}`,
    };
  });
  return {
    trackName: "Bench Track",
    artistName: "Bench Artist",
    albumName: "Bench Album",
    duration: 192,
    wordTimed: true,
    lines,
  };
}

export const NOISY_METADATA = {
  artist: "ExampleArtistVEVO",
  title: "Example Artist - The Song (Official Music Video)",
} as const;

export const SYNCED_RESULT = buildLyricsResult(40);
