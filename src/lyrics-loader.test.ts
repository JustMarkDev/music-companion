import { describe, expect, it } from "vite-plus/test";
import type { LyricsResult, PlaybackVariant } from "./lyrics";
import { LyricsCache, MemoryLyricsStore } from "./lyrics-cache";
import {
  LyricsRetryBudget,
  loadLyricsResult,
  requestWordTiming,
  type LyricsLoaderDeps,
  type LyricsQuery,
} from "./lyrics-loader";

const variant: PlaybackVariant = { metadataKey: "artist::song", durationMs: 180_000 };
const query: LyricsQuery = { variant, title: "Song", artist: "Artist", durationMs: 180_000 };

const lyrics = (text: string, wordTimed = false): LyricsResult => ({
  trackName: text,
  artistName: "Artist",
  albumName: "",
  duration: null,
  wordTimed,
  lines: [
    {
      startMs: 0,
      endMs: 2_000,
      voice: 0,
      segments: [{ startMs: 0, endMs: 2_000, text }],
    },
  ],
});

function setup(answer: () => Promise<unknown>) {
  const cache = new LyricsCache(new MemoryLyricsStore());
  const calls: { command: string; args: Record<string, unknown> }[] = [];
  const deps: LyricsLoaderDeps = {
    cache,
    invoke: (command, args) => {
      calls.push({ command, args });
      return answer();
    },
  };
  return { cache, calls, deps };
}

const current = (requestId = 7) => ({ track: () => true, request: () => requestId });

describe("loadLyricsResult", () => {
  it("answers from the cache without asking the backend", async () => {
    const { cache, calls, deps } = setup(async () => null);
    await cache.putIfCurrent(cache.requestGeneration(), variant, lyrics("Cached"));

    const loaded = await loadLyricsResult(deps, query, current(), () => {});

    expect(loaded).toMatchObject({ status: "cached", result: { trackName: "Cached" } });
    expect(calls).toEqual([]);
  });

  it("fetches a missing song, shows the searching state first and caches the answer", async () => {
    const { cache, calls, deps } = setup(async () => lyrics("Fetched"));
    let searching = 0;

    const loaded = await loadLyricsResult(deps, query, current(7), () => (searching += 1));

    expect(searching).toBe(1);
    expect(loaded).toMatchObject({ status: "fetched", result: { trackName: "Fetched" } });
    expect(calls[0]).toEqual({
      command: "fetch_lyrics",
      args: { title: "Song", artist: "Artist", durationMs: 180_000, requestId: 7 },
    });
    expect((await cache.get(variant))?.trackName).toBe("Fetched");
  });

  it("remembers a clean miss so the song is not asked for again", async () => {
    const { cache, deps } = setup(async () => null);

    expect(await loadLyricsResult(deps, query, current(), () => {})).toEqual({
      status: "fetched",
      result: null,
    });
    expect(await cache.get(variant)).toBeNull();
  });

  it("drops the answer of a request that a newer one replaced, and does not cache it", async () => {
    let requestId = 7;
    const { cache, deps } = setup(async () => {
      requestId = 8;
      return lyrics("Late");
    });

    const loaded = await loadLyricsResult(
      deps,
      query,
      { track: () => true, request: () => requestId },
      () => {},
    );

    expect(loaded).toEqual({ status: "stale" });
    expect(await cache.get(variant)).toBeUndefined();
  });

  it("drops the answer once the song on screen has changed", async () => {
    let onScreen = true;
    const { cache, deps } = setup(async () => {
      onScreen = false;
      return lyrics("Late");
    });

    const loaded = await loadLyricsResult(
      deps,
      query,
      { track: () => onScreen, request: () => 7 },
      () => {},
    );

    expect(loaded).toEqual({ status: "stale" });
    expect(await cache.get(variant)).toBeUndefined();
  });

  it("reports a failure only for the song still on screen, and caches nothing", async () => {
    let onScreen = true;
    const { cache, deps } = setup(async () => {
      throw new Error("lrc.red: timed out");
    });
    const isCurrent = { track: () => onScreen, request: () => 7 };

    expect(await loadLyricsResult(deps, query, isCurrent, () => {})).toEqual({
      status: "failed",
    });
    onScreen = false;
    expect(await loadLyricsResult(deps, query, isCurrent, () => {})).toEqual({
      status: "stale",
    });
    expect(await cache.get(variant)).toBeUndefined();
  });
});

describe("requestWordTiming", () => {
  it("keeps the romanization the song already had and caches the timed lyrics", async () => {
    const earlier = lyrics("Old");
    earlier.lines[0].romanized = { segments: [{ startMs: 0, endMs: 2_000, text: "Oldo" }] };
    const { cache, deps } = setup(async () => lyrics("New", true));
    await cache.putIfCurrent(cache.requestGeneration(), variant, earlier);

    const synced = await requestWordTiming(deps, query);

    expect(synced?.lines[0].romanized?.segments[0].text).toBe("Oldo");
    expect((await cache.get(variant))?.trackName).toBe("New");
    expect(await cache.wordSyncAttempted(variant)).toBe(true);
  });

  it("remembers a song the backend cannot time", async () => {
    const { cache, deps } = setup(async () => null);
    await cache.putIfCurrent(cache.requestGeneration(), variant, lyrics("Plain"));

    expect(await requestWordTiming(deps, query)).toBeNull();
    expect(await cache.wordSyncAttempted(variant)).toBe(true);
  });

  it("lets a failure through without marking the song, so it can be tried again", async () => {
    const { cache, deps } = setup(async () => {
      throw new Error("lrc.red sync: 429");
    });
    await cache.putIfCurrent(cache.requestGeneration(), variant, lyrics("Plain"));

    await expect(requestWordTiming(deps, query)).rejects.toThrow("429");
    expect(await cache.wordSyncAttempted(variant)).toBe(false);
  });
});

describe("LyricsRetryBudget", () => {
  it("waits longer each time and then stops", () => {
    const budget = new LyricsRetryBudget(15_000, 2);

    expect([budget.next(), budget.next(), budget.next()]).toEqual([15_000, 30_000, null]);
  });

  it("starts over after a reset", () => {
    const budget = new LyricsRetryBudget(15_000, 1);
    budget.next();
    budget.reset();

    expect(budget.next()).toBe(15_000);
  });
});
