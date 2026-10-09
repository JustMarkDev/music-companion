import { isSameCachedVariant, type LyricsResult, type PlaybackVariant } from "./lyrics";

// Where the cache lived before it moved to IndexedDB. Those copies hold LRC text
// from providers that are gone, so they are removed rather than carried over.
const OBSOLETE_LYRICS_CACHE_STORAGE_KEYS = [
  "music-companion-lyrics-cache-v3",
  "music-companion-lyrics-cache-v4",
  "music-companion-lyrics-cache-v5",
];
export const MAX_PERSISTED_LYRICS = 10_000;
/** Lyrics kept in memory besides the index; the rest are read from the store on demand. */
const MAX_LOADED_RESULTS = 64;

type StorageAdapter = Pick<Storage, "removeItem">;

/** What is known about a cached song without its lyrics, which are the bulk of the data. */
export type IndexRecord = {
  id: number;
  variant: PlaybackVariant;
  cachedAt: number;
  /** True once word timing was requested for the song and nothing more can be had. */
  wordSyncAttempted?: boolean;
};

export type LyricsStore = {
  readIndex(): Promise<IndexRecord[]>;
  readResult(id: number): Promise<LyricsResult | undefined>;
  write(record: IndexRecord, result: LyricsResult): Promise<void>;
  writeIndex(record: IndexRecord): Promise<void>;
  remove(ids: number[]): Promise<void>;
  clear(): Promise<void>;
};

export type PutOptions = { wordSyncAttempted?: boolean };

type CacheEntry = IndexRecord & {
  /** `null` is a cached miss, `undefined` is a result that is in the store but not loaded. */
  result: LyricsResult | null | undefined;
};

export class LyricsCache {
  private entries: CacheEntry[] = [];
  private entriesByMetadataKey = new Map<string, CacheEntry[]>();
  private loadedResults = new Set<CacheEntry>();
  private generation = 0;
  private nextId = 1;
  private loading: Promise<void> | null = null;
  private writes: Promise<void> = Promise.resolve();
  private failedWrites = 0;

  constructor(
    private readonly store: LyricsStore,
    obsoleteStorage: StorageAdapter | null = null,
    private readonly now: () => number = Date.now,
  ) {
    for (const key of OBSOLETE_LYRICS_CACHE_STORAGE_KEYS) obsoleteStorage?.removeItem(key);
  }

  async get(variant: PlaybackVariant): Promise<LyricsResult | null | undefined> {
    await this.ensureLoaded();
    const entry = this.find(variant);
    if (!entry) return undefined;
    if (entry.result !== undefined) {
      this.touch(entry);
      return entry.result;
    }

    const generation = this.generation;
    const result = await this.readFromStore(entry);
    if (generation !== this.generation) return undefined;
    // A put or an eviction during the read may have replaced the entry, which
    // then must not come back; what is indexed now is the answer.
    if (!this.isIndexed(entry)) return this.get(variant);
    if (!result) {
      this.drop(entry);
      return undefined;
    }
    entry.result = result;
    this.touch(entry);
    return result;
  }

  async has(variant: PlaybackVariant) {
    await this.ensureLoaded();
    return this.find(variant) !== undefined;
  }

  async wordSyncAttempted(variant: PlaybackVariant) {
    await this.ensureLoaded();
    return this.find(variant)?.wordSyncAttempted === true;
  }

  requestGeneration() {
    return this.generation;
  }

  async putIfCurrent(
    generation: number,
    variant: PlaybackVariant,
    result: LyricsResult | null,
    options: PutOptions = {},
  ) {
    await this.ensureLoaded();
    if (generation !== this.generation) return false;
    this.put(variant, result, options);
    return true;
  }

  /** Records that word timing cannot be had for this song, so it is not asked for again. */
  async markWordSyncAttempted(variant: PlaybackVariant) {
    await this.ensureLoaded();
    const entry = this.find(variant);
    if (!entry || entry.wordSyncAttempted) return;
    entry.wordSyncAttempted = true;
    if (entry.result !== null) void this.enqueue(() => this.store.writeIndex(toIndexRecord(entry)));
  }

  /**
   * Forgets one song, so a retry after a transient failure looks it up again
   * instead of reusing the cached miss. Persisted hits are removed from the
   * store as well; anything else is memory-only.
   */
  async forget(variant: PlaybackVariant) {
    await this.ensureLoaded();
    const matching = (this.entriesByMetadataKey.get(variant.metadataKey) ?? []).filter((entry) =>
      isSameCachedVariant(entry.variant, variant),
    );
    if (matching.length === 0) return;
    this.generation += 1;
    for (const entry of matching) this.drop(entry);
  }

  async clear() {
    this.generation += 1;
    this.entries = [];
    this.entriesByMetadataKey.clear();
    this.loadedResults.clear();
    this.loading = Promise.resolve();
    await this.enqueue(() => this.store.clear());
  }

  /** Resolves once every write queued so far has reached the store. */
  flush() {
    return this.writes;
  }

  private ensureLoaded() {
    this.loading ??= this.load();
    return this.loading;
  }

  private async load() {
    const generation = this.generation;
    try {
      const records = await this.store.readIndex();
      // Clearing while the index is still being read must not bring it back.
      if (generation !== this.generation) return;
      for (const record of records.filter(isIndexRecord).sort((a, b) => a.id - b.id)) {
        this.entries.push({ ...record, result: undefined });
        this.nextId = Math.max(this.nextId, record.id + 1);
      }
      this.rebuildIndex();
    } catch (error) {
      console.warn("Unable to read the lyrics cache", error);
    }
  }

  private put(
    variant: PlaybackVariant,
    result: LyricsResult | null,
    options: PutOptions,
    cachedAt = this.now(),
  ) {
    this.removeMatching(variant);
    const entry: CacheEntry = {
      id: this.nextId++,
      variant,
      cachedAt,
      result,
      ...(options.wordSyncAttempted ? { wordSyncAttempted: true } : {}),
    };
    this.entries.push(entry);
    this.addToIndex(entry);

    if (result !== null) {
      this.touch(entry);
      void this.enqueue(() => this.store.write(toIndexRecord(entry), result));
    }
    this.trimToLimit();
  }

  private removeMatching(variant: PlaybackVariant) {
    const removed = (this.entriesByMetadataKey.get(variant.metadataKey) ?? []).filter((entry) =>
      isSameCachedVariant(entry.variant, variant),
    );
    for (const entry of removed) this.drop(entry);
  }

  private trimToLimit() {
    const overflow = this.entries.length - MAX_PERSISTED_LYRICS;
    if (overflow <= 0) return;
    for (const entry of this.entries.slice(0, overflow)) this.drop(entry);
  }

  /** Forgets an entry in memory and in the store. */
  private drop(entry: CacheEntry) {
    const index = this.entries.indexOf(entry);
    if (index >= 0) this.entries.splice(index, 1);
    this.removeFromIndex(entry);
    this.loadedResults.delete(entry);
    if (entry.result !== null) void this.enqueue(() => this.store.remove([entry.id]));
  }

  /** Keeps the most recently used lyrics in memory and lets go of the rest. */
  private touch(entry: CacheEntry) {
    this.loadedResults.delete(entry);
    this.loadedResults.add(entry);
    for (const loaded of this.loadedResults) {
      if (this.loadedResults.size <= MAX_LOADED_RESULTS) break;
      this.loadedResults.delete(loaded);
      if (loaded.result !== null) loaded.result = undefined;
    }
  }

  private async readFromStore(entry: CacheEntry) {
    try {
      // Reads wait for pending writes, so a result just put is never missed.
      await this.writes;
      const result = await this.store.readResult(entry.id);
      return isLyricsResult(result) ? result : undefined;
    } catch (error) {
      console.warn("Unable to read a cached lyrics result", error);
      return undefined;
    }
  }

  private isIndexed(entry: CacheEntry) {
    return this.entriesByMetadataKey.get(entry.variant.metadataKey)?.includes(entry) === true;
  }

  private enqueue(operation: () => Promise<void>) {
    this.writes = this.writes.then(operation).catch((error) => {
      this.failedWrites += 1;
      console.warn("Unable to update the lyrics cache", error);
    });
    return this.writes;
  }

  private find(variant: PlaybackVariant) {
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

/** A store that keeps everything in memory: the fallback without IndexedDB, and the test double. */
export class MemoryLyricsStore implements LyricsStore {
  private records = new Map<number, IndexRecord>();
  private results = new Map<number, LyricsResult>();

  async readIndex() {
    return [...this.records.values()];
  }
  async readResult(id: number) {
    return this.results.get(id);
  }
  async write(record: IndexRecord, result: LyricsResult) {
    this.records.set(record.id, record);
    this.results.set(record.id, result);
  }
  async writeIndex(record: IndexRecord) {
    this.records.set(record.id, record);
  }
  async remove(ids: number[]) {
    for (const id of ids) {
      this.records.delete(id);
      this.results.delete(id);
    }
  }
  async clear() {
    this.records.clear();
    this.results.clear();
  }
}

const DATABASE_NAME = "music-companion";
const DATABASE_VERSION = 2;
const INDEX_STORE = "lyrics-index";
const RESULT_STORE = "lyrics-results";

/**
 * One record per song, with the index apart from the lyrics so that opening the
 * app reads a few hundred kilobytes instead of every cached lyric.
 */
export class IndexedDbLyricsStore implements LyricsStore {
  private database: Promise<IDBDatabase> | null = null;

  readIndex() {
    return this.run(
      [INDEX_STORE],
      "readonly",
      ([index]) => index.getAll() as IDBRequest<IndexRecord[]>,
    );
  }

  readResult(id: number) {
    return this.run(
      [RESULT_STORE],
      "readonly",
      ([results]) => results.get(id) as IDBRequest<LyricsResult | undefined>,
    );
  }

  async write(record: IndexRecord, result: LyricsResult) {
    await this.run([INDEX_STORE, RESULT_STORE], "readwrite", ([index, results]) => {
      index.put(record);
      return results.put(result, record.id);
    });
  }

  async writeIndex(record: IndexRecord) {
    await this.run([INDEX_STORE], "readwrite", ([index]) => index.put(record));
  }

  async remove(ids: number[]) {
    if (ids.length === 0) return;
    await this.run([INDEX_STORE, RESULT_STORE], "readwrite", ([index, results]) => {
      let last!: IDBRequest;
      for (const id of ids) {
        index.delete(id);
        last = results.delete(id);
      }
      return last;
    });
  }

  async clear() {
    await this.run([INDEX_STORE, RESULT_STORE], "readwrite", ([index, results]) => {
      index.clear();
      return results.clear();
    });
  }

  private open() {
    this.database ??= new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open(DATABASE_NAME, DATABASE_VERSION);
      request.onupgradeneeded = (event) => {
        if (event.oldVersion < 1) {
          request.result.createObjectStore(INDEX_STORE, { keyPath: "id" });
          request.result.createObjectStore(RESULT_STORE);
        } else if (event.oldVersion < 2) {
          // Version 1 held LRC text from providers that are gone.
          request.transaction!.objectStore(INDEX_STORE).clear();
          request.transaction!.objectStore(RESULT_STORE).clear();
        }
      };
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
    });
    // A failed open is retried by the next call instead of failing forever.
    this.database.catch(() => {
      this.database = null;
    });
    return this.database;
  }

  /** Runs one transaction and resolves with the last request's value once it has committed. */
  private async run<T>(
    names: string[],
    mode: IDBTransactionMode,
    work: (stores: IDBObjectStore[]) => IDBRequest<T>,
  ) {
    const database = await this.open();
    return new Promise<T>((resolve, reject) => {
      const transaction = database.transaction(names, mode);
      const request = work(names.map((name) => transaction.objectStore(name)));
      transaction.oncomplete = () => resolve(request.result);
      transaction.onerror = () => reject(transaction.error);
      transaction.onabort = () => reject(transaction.error);
    });
  }
}

/** The cache the app uses: IndexedDB where it exists, memory otherwise. */
export function createLyricsCache(obsoleteStorage: StorageAdapter | null) {
  const store =
    typeof indexedDB === "undefined" ? new MemoryLyricsStore() : new IndexedDbLyricsStore();
  return new LyricsCache(store, obsoleteStorage);
}

function toIndexRecord(entry: CacheEntry): IndexRecord {
  return {
    id: entry.id,
    variant: entry.variant,
    cachedAt: entry.cachedAt,
    ...(entry.wordSyncAttempted ? { wordSyncAttempted: true } : {}),
  };
}

function durationDifference(left: PlaybackVariant, right: PlaybackVariant) {
  if (left.durationMs === null || right.durationMs === null) return 0;
  return Math.abs(left.durationMs - right.durationMs);
}

function isLyricsResult(value: unknown): value is LyricsResult {
  if (!value || typeof value !== "object") return false;
  const result = value as Partial<LyricsResult>;
  return typeof result.wordTimed === "boolean" && Array.isArray(result.lines);
}

function isValidVariant(value: unknown): value is PlaybackVariant {
  const variant = value as Partial<PlaybackVariant> | undefined;
  return (
    typeof variant?.metadataKey === "string" &&
    (variant.durationMs === null ||
      (typeof variant.durationMs === "number" &&
        Number.isFinite(variant.durationMs) &&
        variant.durationMs > 0))
  );
}

function isIndexRecord(value: unknown): value is IndexRecord {
  if (!value || typeof value !== "object") return false;
  const record = value as Partial<IndexRecord>;
  return (
    typeof record.id === "number" &&
    Number.isInteger(record.id) &&
    typeof record.cachedAt === "number" &&
    Number.isFinite(record.cachedAt) &&
    isValidVariant(record.variant)
  );
}
