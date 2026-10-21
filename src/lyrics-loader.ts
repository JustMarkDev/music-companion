import { carryOverTracks, type LyricsResult, type PlaybackVariant } from "./lyrics";
import type { LyricsCache } from "./lyrics-cache";

/** The lyrics lookups, apart from the DOM, so the stale-answer rules can be tested. */
export type LyricsLoaderDeps = {
  cache: Pick<LyricsCache, "get" | "requestGeneration" | "putIfCurrent" | "markWordSyncAttempted">;
  invoke: (command: string, args: Record<string, unknown>) => Promise<unknown>;
};

export type LyricsQuery = {
  variant: PlaybackVariant;
  title: string;
  artist: string;
  durationMs: number | null;
};

export type LyricsLoad =
  /** The song or request was replaced while this one ran; nothing may be shown. */
  | { status: "stale" }
  | { status: "cached"; result: LyricsResult | null }
  | { status: "fetched"; result: LyricsResult | null }
  | { status: "failed" };

/**
 * Looks a song up in the cache, then through the backend. A miss is cached only
 * while the request is still the newest one for the song on screen, so a slow
 * answer for a song that was skipped never replaces a newer one.
 */
export async function loadLyricsResult(
  deps: LyricsLoaderDeps,
  query: LyricsQuery,
  isCurrent: { track: () => boolean; request: () => number },
  onSearching: () => void,
): Promise<LyricsLoad> {
  const cached = await deps.cache.get(query.variant);
  if (!isCurrent.track()) return { status: "stale" };
  if (cached !== undefined) return { status: "cached", result: cached };

  onSearching();
  try {
    const cacheGeneration = deps.cache.requestGeneration();
    const requestId = isCurrent.request();
    const result = (await deps.invoke("fetch_lyrics", {
      title: query.title,
      artist: query.artist,
      durationMs: query.durationMs,
      requestId,
    })) as LyricsResult | null;
    if (requestId !== isCurrent.request() || !isCurrent.track()) return { status: "stale" };
    void deps.cache.putIfCurrent(cacheGeneration, query.variant, result);
    return { status: "fetched", result };
  } catch {
    return isCurrent.track() ? { status: "failed" } : { status: "stale" };
  }
}

/**
 * Asks the backend to time every word of a song. `null` means it cannot, which is
 * remembered so it is not asked again; a failure throws and is not remembered. The
 * romanization and translation the song already has are kept.
 */
export async function requestWordTiming(
  deps: LyricsLoaderDeps,
  query: LyricsQuery,
): Promise<LyricsResult | null> {
  const cacheGeneration = deps.cache.requestGeneration();
  const timed = (await deps.invoke("sync_lyrics_words", {
    title: query.title,
    artist: query.artist,
    durationMs: query.durationMs,
  })) as LyricsResult | null;
  const synced = timed && carryOverTracks(timed, await deps.cache.get(query.variant));
  if (!synced) {
    await deps.cache.markWordSyncAttempted(query.variant);
    return null;
  }
  // Kept even if the song has changed since, so playing it again needs no wait.
  await deps.cache.putIfCurrent(cacheGeneration, query.variant, synced, {
    wordSyncAttempted: true,
  });
  return synced;
}

/**
 * Retries after a transient failure, with a growing delay and then not at all, so
 * a stalled lookup never sticks until the next track but the provider is never
 * polled forever.
 */
export class LyricsRetryBudget {
  private used = 0;

  constructor(
    private readonly baseDelayMs: number,
    private readonly maxRetries: number,
  ) {}

  /** The wait before the next retry, or `null` once the budget is spent. */
  next() {
    if (this.used >= this.maxRetries) return null;
    this.used += 1;
    return this.baseDelayMs * this.used;
  }

  reset() {
    this.used = 0;
  }
}
