import { describe, expect, it } from "vitest";
import { LyricsCache, LYRICS_CACHE_STORAGE_KEY } from "./lyrics-cache";
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

const variant = (durationMs: number | null): PlaybackVariant => ({
  metadataKey: "artist::song",
  durationMs,
});
const lyrics = (trackName: string): LyricsResult => ({
  source: "LRCLIB",
  trackName,
  artistName: "Artist",
  albumName: "",
  duration: null,
  instrumental: false,
  syncedLyrics: `[00:00.00]${trackName}`,
  plainLyrics: trackName,
});

describe("LyricsCache", () => {
  it("reuses nearby durations while keeping materially different variants", () => {
    const cache = new LyricsCache(new MemoryStorage());
    cache.putIfCurrent(cache.requestGeneration(), variant(180_000), lyrics("Audio"));
    cache.putIfCurrent(cache.requestGeneration(), variant(210_000), lyrics("Video"));

    expect(cache.get(variant(182_999))?.trackName).toBe("Audio");
    expect(cache.get(variant(207_001))?.trackName).toBe("Video");
  });

  it("keeps unknown duration separate from known variants", () => {
    const cache = new LyricsCache(new MemoryStorage());
    cache.putIfCurrent(cache.requestGeneration(), variant(180_000), lyrics("Known"));
    expect(cache.has(variant(null))).toBe(false);
    cache.putIfCurrent(cache.requestGeneration(), variant(null), lyrics("Unknown"));
    expect(cache.get(variant(null))?.trackName).toBe("Unknown");
  });

  it("replaces matching durations while retaining a different variant", () => {
    const cache = new LyricsCache(new MemoryStorage());
    cache.putIfCurrent(cache.requestGeneration(), variant(180_000), lyrics("Old"));
    cache.putIfCurrent(cache.requestGeneration(), variant(181_000), lyrics("New"));
    cache.putIfCurrent(cache.requestGeneration(), variant(210_000), lyrics("Video"));
    expect(cache.get(variant(180_000))?.trackName).toBe("New");
    expect(cache.get(variant(210_000))?.trackName).toBe("Video");
  });

  it("does not persist negative results across app sessions", () => {
    const storage = new MemoryStorage();
    const cache = new LyricsCache(storage);
    cache.putIfCurrent(cache.requestGeneration(), variant(180_000), null);
    expect(cache.has(variant(180_000))).toBe(true);
    expect(new LyricsCache(storage).has(variant(180_000))).toBe(false);
  });

  it("rejects an in-flight result after the cache is cleared", () => {
    const storage = new MemoryStorage();
    const cache = new LyricsCache(storage);
    const requestGeneration = cache.requestGeneration();
    cache.clear();
    expect(cache.putIfCurrent(requestGeneration, variant(180_000), lyrics("Stale"))).toBe(false);
    expect(cache.has(variant(180_000))).toBe(false);
  });

  it("starts fresh instead of reading the version-three schema", () => {
    const storage = new MemoryStorage();
    storage.setItem("music-companion-lyrics-cache-v3", JSON.stringify([["artist::song", {}]]));
    expect(new LyricsCache(storage).has(variant(180_000))).toBe(false);
    expect(storage.getItem("music-companion-lyrics-cache-v3")).toBeNull();
    expect(storage.getItem(LYRICS_CACHE_STORAGE_KEY)).toBeNull();
  });

  it("discards malformed persisted data", () => {
    const storage = new MemoryStorage();
    storage.setItem(LYRICS_CACHE_STORAGE_KEY, "not-json");
    const cache = new LyricsCache(storage);
    expect(cache.has(variant(180_000))).toBe(false);
    expect(storage.getItem(LYRICS_CACHE_STORAGE_KEY)).toBeNull();
  });

  it("persists at most two hundred results", () => {
    const storage = new MemoryStorage();
    let now = 0;
    const cache = new LyricsCache(storage, () => now++);
    for (let index = 0; index < 205; index += 1) {
      cache.putIfCurrent(
        cache.requestGeneration(),
        { metadataKey: `artist::song-${index}`, durationMs: 180_000 },
        lyrics(String(index)),
      );
    }
    const persisted = JSON.parse(storage.getItem(LYRICS_CACHE_STORAGE_KEY) ?? "[]") as unknown[];
    expect(persisted).toHaveLength(200);
  });
});
