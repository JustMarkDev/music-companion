import { describe, test } from "vite-plus/test";
import { LyricsCache, MemoryLyricsStore } from "./lyrics-cache";
import type { LyricsResult, PlaybackVariant } from "./lyrics";

function variant(index: number, durationMs: number): PlaybackVariant {
  return {
    metadataKey: `artist-${index % 200}::song-${index}`,
    durationMs,
  };
}

function lyrics(index: number): LyricsResult {
  return {
    trackName: `Song ${index}`,
    artistName: `Artist ${index % 200}`,
    albumName: "",
    duration: 180,
    wordTimed: false,
    lines: [
      {
        startMs: 0,
        endMs: 2_000,
        voice: 0,
        segments: [{ startMs: 0, endMs: 2_000, text: `Song ${index}` }],
      },
    ],
  };
}

async function filledCache(size: number) {
  const cache = new LyricsCache(new MemoryLyricsStore());
  const generation = cache.requestGeneration();
  for (let index = 0; index < size; index += 1) {
    await cache.putIfCurrent(
      generation,
      variant(index, 180_000 + (index % 5) * 1_000),
      lyrics(index),
    );
  }
  return cache;
}

describe("lyrics cache", async () => {
  const hotCache = await filledCache(500);
  const probe = variant(250, 181_000);
  await hotCache.get(probe);

  test("lyrics cache", async ({ bench }) => {
    await bench.compare(
      bench("lyrics_cache_get_hot", async () => {
        await hotCache.get(probe);
      }),
      bench("lyrics_cache_put_persist", async () => {
        const cache = new LyricsCache(new MemoryLyricsStore());
        const generation = cache.requestGeneration();
        await cache.putIfCurrent(generation, variant(1, 180_000), lyrics(1));
      }),
    );
  });
});
