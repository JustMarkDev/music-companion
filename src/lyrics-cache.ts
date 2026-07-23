import { isSameCachedVariant, type LyricsResult, type PlaybackVariant } from "./lyrics";

export const LYRICS_CACHE_STORAGE_KEY = "music-companion-lyrics-cache-v4";
const LEGACY_LYRICS_CACHE_STORAGE_KEY = "music-companion-lyrics-cache-v3";
const MAX_PERSISTED_LYRICS = 200;

type StorageAdapter = Pick<Storage, "getItem" | "setItem" | "removeItem">;

type CacheEntry = {
  variant: PlaybackVariant;
  cachedAt: number;
  result: LyricsResult | null;
};

type PersistedCacheEntry = CacheEntry & { result: LyricsResult };

export class LyricsCache {
  private entries: CacheEntry[] = [];
  private generation = 0;

  constructor(
    private readonly storage: StorageAdapter,
    private readonly now: () => number = Date.now,
  ) {
    this.storage.removeItem(LEGACY_LYRICS_CACHE_STORAGE_KEY);
    this.entries = this.load();
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
    this.storage.removeItem(LYRICS_CACHE_STORAGE_KEY);
  }

  private put(variant: PlaybackVariant, result: LyricsResult | null) {
    this.entries = this.entries.filter((entry) => !isSameCachedVariant(entry.variant, variant));
    this.entries.push({ variant, cachedAt: this.now(), result });
    this.entries = this.entries.slice(-MAX_PERSISTED_LYRICS);
    this.persist();
  }

  private find(variant: PlaybackVariant) {
    return this.entries
      .filter((entry) => isSameCachedVariant(entry.variant, variant))
      .sort((left, right) => {
        const leftDifference = durationDifference(left.variant, variant);
        const rightDifference = durationDifference(right.variant, variant);
        return leftDifference - rightDifference || right.cachedAt - left.cachedAt;
      })[0];
  }

  private load(): CacheEntry[] {
    try {
      const stored = this.storage.getItem(LYRICS_CACHE_STORAGE_KEY);
      const parsed: unknown = stored ? JSON.parse(stored) : [];
      if (!Array.isArray(parsed)) throw new Error("Invalid lyrics cache");
      return parsed.filter(isPersistedCacheEntry).slice(-MAX_PERSISTED_LYRICS);
    } catch {
      this.storage.removeItem(LYRICS_CACHE_STORAGE_KEY);
      return [];
    }
  }

  private persist() {
    const persisted = this.entries.filter(
      (entry): entry is PersistedCacheEntry => entry.result !== null,
    );
    try {
      this.storage.setItem(LYRICS_CACHE_STORAGE_KEY, JSON.stringify(persisted));
    } catch (error) {
      console.warn("Unable to persist the lyrics cache", error);
    }
  }
}

function durationDifference(left: PlaybackVariant, right: PlaybackVariant) {
  if (left.durationMs === null || right.durationMs === null) return 0;
  return Math.abs(left.durationMs - right.durationMs);
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
