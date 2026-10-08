import { describe, expect, it } from "vite-plus/test";
import { LyricsCache, MAX_PERSISTED_LYRICS, MemoryLyricsStore } from "./lyrics-cache";
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

const text = (result: LyricsResult | null | undefined) =>
  result?.lines[0].segments.map((segment) => segment.text).join("");

const variant = (durationMs: number | null): PlaybackVariant => ({
  metadataKey: "artist::song",
  durationMs,
});
const song = (index: number): PlaybackVariant => ({
  metadataKey: `artist::song-${index}`,
  durationMs: 180_000,
});
const lyrics = (trackName: string): LyricsResult => ({
  trackName,
  artistName: "Artist",
  albumName: "",
  duration: null,
  wordTimed: false,
  lines: [
    {
      startMs: 0,
      endMs: 2_000,
      voice: 0,
      segments: [{ startMs: 0, endMs: 2_000, text: trackName }],
    },
  ],
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
    expect(text(await reopened.get(variant(180_000)))).toBe("Saved");
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

  it("answers with the newer result when a song is replaced while its lyrics are being read", async () => {
    const store = new MemoryLyricsStore();
    const saved = new LyricsCache(store);
    await put(saved, variant(180_000), lyrics("Old"));
    await saved.flush();

    let reachedRead!: () => void;
    const reading = new Promise<void>((resolve) => (reachedRead = resolve));
    let finishRead!: () => void;
    const readMayFinish = new Promise<void>((resolve) => (finishRead = resolve));
    const readResult = store.readResult.bind(store);
    store.readResult = async (id) => {
      reachedRead();
      await readMayFinish;
      return readResult(id);
    };

    const cache = new LyricsCache(store);
    const pending = cache.get(variant(180_000));
    await reading;
    await put(cache, variant(180_000), lyrics("New"));
    finishRead();

    expect((await pending)?.trackName).toBe("New");
    expect((await cache.get(variant(180_000)))?.trackName).toBe("New");
  });

  it("stores the result as it is, with its romanization and translation", async () => {
    const store = new MemoryLyricsStore();
    const cache = new LyricsCache(store);
    const base = lyrics("Synced");
    const result: LyricsResult = {
      ...base,
      lines: [
        {
          ...base.lines[0],
          romanized: { segments: [{ startMs: 0, endMs: 2_000, text: "Shinku" }] },
          translation: "Synced",
        },
      ],
    };
    await put(cache, variant(180_000), result);
    await cache.flush();

    const [record] = await store.readIndex();
    expect(await store.readResult(record.id)).toEqual(result);
    expect(await new LyricsCache(store).get(variant(180_000))).toEqual(result);
  });

  it("forgets a stored result that is not in the current format", async () => {
    const store = new MemoryLyricsStore();
    const cache = new LyricsCache(store);
    await put(cache, variant(180_000), lyrics("Old"));
    await cache.flush();
    const [record] = await store.readIndex();
    await store.write(record, { source: "LRCLIB", syncedLyrics: "[00:00.00]Old" } as never);

    expect(await new LyricsCache(store).get(variant(180_000))).toBeUndefined();
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
    it("removes the localStorage caches of the schemas that were dropped", async () => {
      for (const obsoleteKey of [
        "music-companion-lyrics-cache-v3",
        "music-companion-lyrics-cache-v4",
        "music-companion-lyrics-cache-v5",
      ]) {
        const storage = new MemoryStorage();
        storage.setItem(obsoleteKey, JSON.stringify([["artist::song", {}]]));
        const cache = new LyricsCache(new MemoryLyricsStore(), storage);

        expect(await cache.has(variant(180_000))).toBe(false);
        expect(storage.getItem(obsoleteKey)).toBeNull();
      }
    });
  });
});
