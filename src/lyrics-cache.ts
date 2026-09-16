import { isSameCachedVariant, type LyricsResult, type PlaybackVariant } from "./lyrics";

export const LYRICS_CACHE_STORAGE_KEY = "music-companion-lyrics-cache-v4";
const LEGACY_LYRICS_CACHE_STORAGE_KEY = "music-companion-lyrics-cache-v3";
export const MAX_PERSISTED_LYRICS = 1_000;

type StorageAdapter = Pick<Storage, "getItem" | "setItem" | "removeItem">;

type CacheEntry = {
  variant: PlaybackVariant;
  cachedAt: number;
  result: LyricsResult | null;
};

type PersistedCacheEntry = CacheEntry & { result: LyricsResult };

export class LyricsCache {
  private entries: CacheEntry[] = [];
  private entriesByMetadataKey = new Map<string, CacheEntry[]>();
  private generation = 0;
  private loaded = false;

  constructor(
    private readonly storage: StorageAdapter,
    private readonly now: () => number = Date.now,
  ) {
    this.storage.removeItem(LEGACY_LYRICS_CACHE_STORAGE_KEY);
  }

  get(variant: PlaybackVariant): LyricsResult | null | undefined {
    return this.find(variant)?.result;
  }

  has(variant: PlaybackVariant) {
    return this.find(variant) !== undefined;
  }

  requestGeneration() {
    return this.generation;
  }

  putIfCurrent(generation: number, variant: PlaybackVariant, result: LyricsResult | null) {
    if (generation !== this.generation) return false;
    this.put(variant, result);
    return true;
  }

  clear() {
    this.generation += 1;
    this.entries = [];
    this.entriesByMetadataKey.clear();
    this.loaded = true;
    this.storage.removeItem(LYRICS_CACHE_STORAGE_KEY);
  }

  private ensureLoaded() {
    if (this.loaded) return;
    this.loaded = true;
    this.entries = this.load();
    this.rebuildIndex();
  }

  private put(variant: PlaybackVariant, result: LyricsResult | null) {
    this.ensureLoaded();
    this.removeMatching(variant);
    const entry: CacheEntry = { variant, cachedAt: this.now(), result };
    this.entries.push(entry);
    this.addToIndex(entry);
    this.trimToLimit();
    this.persist();
  }

  private removeMatching(variant: PlaybackVariant) {
    const candidates = this.entriesByMetadataKey.get(variant.metadataKey);
    if (!candidates?.length) return;

    const removed = candidates.filter((entry) => isSameCachedVariant(entry.variant, variant));
    if (removed.length === 0) return;

    for (const entry of removed) {
      this.removeFromIndex(entry);
    }
    this.entries = this.entries.filter((entry) => !isSameCachedVariant(entry.variant, variant));
  }

  private trimToLimit() {
    if (this.entries.length <= MAX_PERSISTED_LYRICS) return;
    const overflow = this.entries.length - MAX_PERSISTED_LYRICS;
    const removed = this.entries.splice(0, overflow);
    for (const entry of removed) {
      this.removeFromIndex(entry);
    }
  }

  private find(variant: PlaybackVariant) {
    this.ensureLoaded();
    const candidates = this.entriesByMetadataKey.get(variant.metadataKey);
    if (!candidates?.length) return undefined;

    let best: CacheEntry | undefined;
    let bestDifference = Number.POSITIVE_INFINITY;
    let bestCachedAt = Number.NEGATIVE_INFINITY;

    for (const entry of candidates) {
      if (!isSameCachedVariant(entry.variant, variant)) continue;
      const difference = durationDifference(entry.variant, variant);
      if (
        difference < bestDifference ||
        (difference === bestDifference && entry.cachedAt > bestCachedAt)
      ) {
        best = entry;
        bestDifference = difference;
        bestCachedAt = entry.cachedAt;
      }
    }

    return best;
  }

  private load(): CacheEntry[] {
    try {
      const stored = this.storage.getItem(LYRICS_CACHE_STORAGE_KEY);
      const parsed: unknown = stored ? JSON.parse(stored) : [];
      if (!Array.isArray(parsed)) throw new Error("Invalid lyrics cache");
      return parsed
        .filter(isPersistedCacheEntry)
        .slice(-MAX_PERSISTED_LYRICS)
        .map(normalizePersistedEntry);
    } catch {
      this.storage.removeItem(LYRICS_CACHE_STORAGE_KEY);
      return [];
    }
  }

  private persist() {
    while (true) {
      const positiveEntries = this.entries.filter(
        (entry): entry is PersistedCacheEntry => entry.result !== null,
      );
      const persisted = positiveEntries.map(toPersistedEntry);

      try {
        this.storage.setItem(LYRICS_CACHE_STORAGE_KEY, JSON.stringify(persisted));
        return;
      } catch (error) {
        if (positiveEntries.length === 0) {
          console.warn("Unable to persist the lyrics cache", error);
          return;
        }

        // localStorage quotas are typically ~5MB. Drop the oldest chunk and retry
        // so a larger cache still survives instead of failing open.
        const dropCount = Math.max(1, Math.ceil(positiveEntries.length * 0.1));
        const dropped = new Set(positiveEntries.slice(0, dropCount));
        this.entries = this.entries.filter(
          (entry) => entry.result === null || !dropped.has(entry as PersistedCacheEntry),
        );
        this.rebuildIndex();
      }
    }
  }

  private rebuildIndex() {
    this.entriesByMetadataKey.clear();
    for (const entry of this.entries) {
      this.addToIndex(entry);
    }
  }

  private addToIndex(entry: CacheEntry) {
    const existing = this.entriesByMetadataKey.get(entry.variant.metadataKey);
    if (existing) {
      existing.push(entry);
      return;
    }
    this.entriesByMetadataKey.set(entry.variant.metadataKey, [entry]);
  }

  private removeFromIndex(entry: CacheEntry) {
    const existing = this.entriesByMetadataKey.get(entry.variant.metadataKey);
    if (!existing) return;
    const index = existing.indexOf(entry);
    if (index >= 0) existing.splice(index, 1);
    if (existing.length === 0) {
      this.entriesByMetadataKey.delete(entry.variant.metadataKey);
    }
  }
}

function durationDifference(left: PlaybackVariant, right: PlaybackVariant) {
  if (left.durationMs === null || right.durationMs === null) return 0;
  return Math.abs(left.durationMs - right.durationMs);
}

function toPersistedEntry(entry: PersistedCacheEntry): PersistedCacheEntry {
  return {
    variant: entry.variant,
    cachedAt: entry.cachedAt,
    result: compactLyricsResult(entry.result),
  };
}

function compactLyricsResult(result: LyricsResult): LyricsResult {
  return {
    source: result.source,
    trackName: result.trackName,
    artistName: result.artistName,
    albumName: result.albumName,
    duration: result.duration,
    instrumental: result.instrumental,
    syncedLyrics: result.syncedLyrics,
    // Synced lyrics already cover display; plain text only matters as a fallback.
    plainLyrics: result.syncedLyrics ? null : result.plainLyrics,
    ...(result.romanizedSyncedLyrics
      ? { romanizedSyncedLyrics: result.romanizedSyncedLyrics }
      : {}),
  };
}

function normalizePersistedEntry(entry: PersistedCacheEntry): PersistedCacheEntry {
  return {
    variant: entry.variant,
    cachedAt: entry.cachedAt,
    result: {
      source: entry.result.source,
      trackName: entry.result.trackName,
      artistName: entry.result.artistName,
      albumName: entry.result.albumName,
      duration: entry.result.duration,
      instrumental: entry.result.instrumental,
      syncedLyrics: entry.result.syncedLyrics ?? null,
      plainLyrics: entry.result.plainLyrics ?? null,
      romanizedSyncedLyrics: entry.result.romanizedSyncedLyrics ?? null,
    },
  };
}

function isPersistedCacheEntry(value: unknown): value is PersistedCacheEntry {
  if (!value || typeof value !== "object") return false;
  const entry = value as Partial<PersistedCacheEntry>;
  const variant = entry.variant as Partial<PlaybackVariant> | undefined;
  return (
    typeof entry.cachedAt === "number" &&
    Number.isFinite(entry.cachedAt) &&
    typeof variant?.metadataKey === "string" &&
    (variant.durationMs === null ||
      (typeof variant.durationMs === "number" &&
        Number.isFinite(variant.durationMs) &&
        variant.durationMs > 0)) &&
    typeof entry.result === "object" &&
    entry.result !== null
  );
}
