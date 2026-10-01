import { describe, test } from "vite-plus/test";
import { LyricsCache } from "./lyrics-cache";
import type { LyricsResult, PlaybackVariant } from "./lyrics";

class MemoryStorage {
  values = new Map<string, string>();
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
  removeItem(key: string) {
    this.values.delete(key);
  }
}

function variant(index: number, durationMs: number): PlaybackVariant {
  return {
    metadataKey: `artist-${index % 200}::song-${index}`,
    durationMs,
  };
}

function lyrics(index: number): LyricsResult {
  return {
    source: "LRCLIB",
    trackName: `Song ${index}`,
    artistName: `Artist ${index % 200}`,
    albumName: "",
    duration: 180,
    instrumental: false,
    syncedLyrics: `[00:00.00]Song ${index}`,
    plainLyrics: `Song ${index}`,
  };
}

function filledCache(size: number) {
  const cache = new LyricsCache(new MemoryStorage());
  const generation = cache.requestGeneration();
  for (let index = 0; index < size; index += 1) {
    cache.putIfCurrent(generation, variant(index, 180_000 + (index % 5) * 1_000), lyrics(index));
  }
  return cache;
}

describe("lyrics cache", () => {
  const hotCache = filledCache(500);
  const probe = variant(250, 181_000);

  test("lyrics cache", async ({ bench }) => {
    await bench.compare(
      bench("lyrics_cache_get_hot", () => {
        hotCache.get(probe);
      }),
      bench("lyrics_cache_put_persist", () => {
        const cache = new LyricsCache(new MemoryStorage());
        const generation = cache.requestGeneration();
        cache.putIfCurrent(generation, variant(1, 180_000), lyrics(1));
      }),
    );
  });
});
