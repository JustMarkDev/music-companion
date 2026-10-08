import { describe, expect, it } from "vite-plus/test";
import {
  LEGACY_LYRICS_CACHE_STORAGE_KEY,
  LyricsCache,
  MAX_PERSISTED_LYRICS,
  MemoryLyricsStore,
} from "./lyrics-cache";
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
const song = (index: number): PlaybackVariant => ({
  metadataKey: `artist::song-${index}`,
  durationMs: 180_000,
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

async function put(cache: LyricsCache, key: PlaybackVariant, result: LyricsResult | null) {
  return cache.putIfCurrent(cache.requestGeneration(), key, result);
}

describe("LyricsCache", () => {
  it("reuses nearby durations while keeping materially different variants", async () => {
    const cache = new LyricsCache(new MemoryLyricsStore());
    await put(cache, variant(180_000), lyrics("Audio"));
    await put(cache, variant(210_000), lyrics("Video"));

    expect((await cache.get(variant(182_999)))?.trackName).toBe("Audio");
    expect((await cache.get(variant(207_001)))?.trackName).toBe("Video");
  });

  it("keeps unknown duration separate from known variants", async () => {
    const cache = new LyricsCache(new MemoryLyricsStore());
    await put(cache, variant(180_000), lyrics("Known"));
    expect(await cache.has(variant(null))).toBe(false);
    await put(cache, variant(null), lyrics("Unknown"));
    expect((await cache.get(variant(null)))?.trackName).toBe("Unknown");
  });

  it("replaces matching durations while retaining a different variant", async () => {
    const cache = new LyricsCache(new MemoryLyricsStore());
    await put(cache, variant(180_000), lyrics("Old"));
    await put(cache, variant(181_000), lyrics("New"));
    await put(cache, variant(210_000), lyrics("Video"));
    expect((await cache.get(variant(180_000)))?.trackName).toBe("New");
    expect((await cache.get(variant(210_000)))?.trackName).toBe("Video");
  });

  it("does not persist negative results across app sessions", async () => {
    const store = new MemoryLyricsStore();
    const cache = new LyricsCache(store);
    await put(cache, variant(180_000), null);
    expect(await cache.has(variant(180_000))).toBe(true);
    await cache.flush();
    expect(await new LyricsCache(store).has(variant(180_000))).toBe(false);
  });

  it("keeps results across app sessions", async () => {
    const store = new MemoryLyricsStore();
    const cache = new LyricsCache(store);
    await put(cache, variant(180_000), lyrics("Saved"));
    await cache.flush();

    const reopened = new LyricsCache(store);
    expect((await reopened.get(variant(180_000)))?.syncedLyrics).toContain("Saved");
  });

  it("rejects an in-flight result after the cache is cleared", async () => {
    const store = new MemoryLyricsStore();
    const cache = new LyricsCache(store);
    const requestGeneration = cache.requestGeneration();
    await cache.clear();
    expect(await cache.putIfCurrent(requestGeneration, variant(180_000), lyrics("Stale"))).toBe(
      false,
    );
    expect(await cache.has(variant(180_000))).toBe(false);
  });

  it("empties the store when cleared", async () => {
    const store = new MemoryLyricsStore();
    const cache = new LyricsCache(store);
    await put(cache, variant(180_000), lyrics("Gone"));
    await cache.clear();

    expect(await store.readIndex()).toEqual([]);
    expect(await new LyricsCache(store).has(variant(180_000))).toBe(false);
  });

  it("does not bring the cache back when it is cleared while still loading", async () => {
    const store = new MemoryLyricsStore();
    const saved = new LyricsCache(store);
    await put(saved, variant(180_000), lyrics("Saved"));
    await saved.flush();

    const cache = new LyricsCache(store);
    const loading = cache.has(variant(180_000));
    await cache.clear();
    expect(await loading).toBe(false);
    expect(await cache.has(variant(180_000))).toBe(false);
  });

  it("holds ten thousand songs and drops the oldest beyond that", async () => {
    expect(MAX_PERSISTED_LYRICS).toBe(10_000);
    const store = new MemoryLyricsStore();
    let now = 0;
    const cache = new LyricsCache(store, null, () => now++);
    for (let index = 0; index < MAX_PERSISTED_LYRICS + 5; index += 1) {
      await put(cache, song(index), lyrics(String(index)));
    }
    await cache.flush();

    expect(await store.readIndex()).toHaveLength(MAX_PERSISTED_LYRICS);
    expect(await cache.has(song(4))).toBe(false);
    expect((await cache.get(song(5)))?.trackName).toBe("5");
    expect((await cache.get(song(MAX_PERSISTED_LYRICS + 4)))?.trackName).toBe(
      String(MAX_PERSISTED_LYRICS + 4),
    );
  });

  it("serves songs from the store once they are no longer held in memory", async () => {
    const store = new MemoryLyricsStore();
    const cache = new LyricsCache(store);
    for (let index = 0; index < 200; index += 1) {
      await put(cache, song(index), lyrics(`Song ${index}`));
    }

    expect((await cache.get(song(0)))?.trackName).toBe("Song 0");
    expect((await cache.get(song(199)))?.trackName).toBe("Song 199");
  });

  it("forgets a song whose lyrics have gone missing from the store", async () => {
    const store = new MemoryLyricsStore();
    const saved = new LyricsCache(store);
    await put(saved, variant(180_000), lyrics("Lost"));
    await saved.flush();
    const [record] = await store.readIndex();

    const cache = new LyricsCache(store);
    await store.remove([record.id]);
    await store.writeIndex(record);
    expect(await cache.get(variant(180_000))).toBeUndefined();
    expect(await cache.has(variant(180_000))).toBe(false);
  });

  it("omits plain lyrics from storage when synced lyrics are present", async () => {
    const store = new MemoryLyricsStore();
    const cache = new LyricsCache(store);
    await put(cache, variant(180_000), {
      ...lyrics("Synced"),
      plainLyrics: "Plain fallback that should not be stored",
      romanizedSyncedLyrics: null,
    });
    await cache.flush();

    const [record] = await store.readIndex();
    const stored = await store.readResult(record.id);
    expect(stored?.plainLyrics).toBeNull();
    expect(stored).not.toHaveProperty("romanizedSyncedLyrics");
    expect((await cache.get(variant(180_000)))?.syncedLyrics).toContain("Synced");
  });

  it("keeps working in memory when the store cannot be read or written", async () => {
    const broken = new MemoryLyricsStore();
    broken.readIndex = () => Promise.reject(new Error("unavailable"));
    broken.write = () => Promise.reject(new Error("unavailable"));

    const cache = new LyricsCache(broken);
    expect(await cache.has(variant(180_000))).toBe(false);
    await put(cache, variant(180_000), lyrics("Memory"));
    expect((await cache.get(variant(180_000)))?.trackName).toBe("Memory");
  });

  describe("word sync", () => {
    it("remembers that word timing was asked for, across sessions", async () => {
      const store = new MemoryLyricsStore();
      const cache = new LyricsCache(store);
      await put(cache, variant(180_000), lyrics("Line synced"));
      expect(await cache.wordSyncAttempted(variant(180_000))).toBe(false);

      await cache.markWordSyncAttempted(variant(180_000));
      await cache.flush();
      expect(await cache.wordSyncAttempted(variant(180_000))).toBe(true);
      expect(await new LyricsCache(store).wordSyncAttempted(variant(180_000))).toBe(true);
    });

    it("stores an upgraded result already marked as attempted", async () => {
      const store = new MemoryLyricsStore();
      const cache = new LyricsCache(store);
      await put(cache, variant(180_000), lyrics("Line synced"));
      await cache.putIfCurrent(cache.requestGeneration(), variant(180_000), lyrics("Word timed"), {
        wordSyncAttempted: true,
      });
      await cache.flush();

      const reopened = new LyricsCache(store);
      expect((await reopened.get(variant(180_000)))?.trackName).toBe("Word timed");
      expect(await reopened.wordSyncAttempted(variant(180_000))).toBe(true);
    });

    it("starts a replaced result unmarked, so a fresh lookup may be synced again", async () => {
      const cache = new LyricsCache(new MemoryLyricsStore());
      await put(cache, variant(180_000), lyrics("First"));
      await cache.markWordSyncAttempted(variant(180_000));
      await put(cache, variant(180_000), lyrics("Second"));

      expect(await cache.wordSyncAttempted(variant(180_000))).toBe(false);
    });

    it("tracks misses for the session only", async () => {
      const store = new MemoryLyricsStore();
      const cache = new LyricsCache(store);
      await put(cache, variant(180_000), null);
      await cache.markWordSyncAttempted(variant(180_000));
      await cache.flush();

      expect(await cache.wordSyncAttempted(variant(180_000))).toBe(true);
      expect(await store.readIndex()).toEqual([]);
    });

    it("knows nothing about songs that are not cached", async () => {
      const cache = new LyricsCache(new MemoryLyricsStore());
      await cache.markWordSyncAttempted(variant(180_000));
      expect(await cache.wordSyncAttempted(variant(180_000))).toBe(false);
    });
  });

  describe("earlier versions", () => {
    const legacyEntry = (name: string, cachedAt: number) => ({
      variant: { metadataKey: `artist::${name}`, durationMs: 180_000 },
      cachedAt,
      result: lyrics(name),
    });

    it("moves the localStorage cache into the store and removes it", async () => {
      const storage = new MemoryStorage();
      storage.setItem(
        LEGACY_LYRICS_CACHE_STORAGE_KEY,
        JSON.stringify([legacyEntry("old", 1), legacyEntry("new", 2)]),
      );
      const store = new MemoryLyricsStore();
      const cache = new LyricsCache(store, storage);

      expect(
        (await cache.get({ metadataKey: "artist::old", durationMs: 180_000 }))?.trackName,
      ).toBe("old");
      expect(storage.getItem(LEGACY_LYRICS_CACHE_STORAGE_KEY)).toBeNull();
      expect(await store.readIndex()).toHaveLength(2);
      expect(
        (await new LyricsCache(store).get({ metadataKey: "artist::new", durationMs: 180_000 }))
          ?.trackName,
      ).toBe("new");
    });

    it("discards a malformed localStorage cache", async () => {
      const storage = new MemoryStorage();
      storage.setItem(LEGACY_LYRICS_CACHE_STORAGE_KEY, "not-json");
      const cache = new LyricsCache(new MemoryLyricsStore(), storage);

      expect(await cache.has(variant(180_000))).toBe(false);
      expect(storage.getItem(LEGACY_LYRICS_CACHE_STORAGE_KEY)).toBeNull();
    });

    it("skips invalid legacy entries and keeps the valid ones", async () => {
      const storage = new MemoryStorage();
      storage.setItem(
        LEGACY_LYRICS_CACHE_STORAGE_KEY,
        JSON.stringify([{ variant: { metadataKey: 1 } }, legacyEntry("kept", 1), null]),
      );
      const cache = new LyricsCache(new MemoryLyricsStore(), storage);

      expect(
        (await cache.get({ metadataKey: "artist::kept", durationMs: 180_000 }))?.trackName,
      ).toBe("kept");
    });

    it("removes the schemas that were dropped earlier", async () => {
      for (const legacyKey of [
        "music-companion-lyrics-cache-v3",
        "music-companion-lyrics-cache-v4",
      ]) {
        const storage = new MemoryStorage();
        storage.setItem(legacyKey, JSON.stringify([["artist::song", {}]]));
        const cache = new LyricsCache(new MemoryLyricsStore(), storage);

        expect(await cache.has(variant(180_000))).toBe(false);
        expect(storage.getItem(legacyKey)).toBeNull();
      }
    });
  });
});
