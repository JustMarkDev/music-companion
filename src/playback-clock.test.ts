import { describe, expect, it } from "vitest";
import { PlaybackClock } from "./playback-clock";

const sample = (
  overrides: Partial<{
    hasSession: boolean;
    isPlaying: boolean;
    status: string;
    positionMs: number;
    durationMs: number | null;
    playbackRate: number | null;
  }> = {},
) => ({
  hasSession: true,
  isPlaying: true,
  status: "Playing",
  positionMs: 60_000,
  durationMs: 180_000,
  playbackRate: 1,
  ...overrides,
});

describe("PlaybackClock", () => {
  it("advances at the reported rate and falls back to normal speed", () => {
    const clock = new PlaybackClock(1_000, 60_000);
    expect(clock.estimate(sample({ playbackRate: 2 }), 2_000)).toBe(62_000);
    expect(clock.estimate(sample({ playbackRate: Number.NaN }), 2_000)).toBe(61_000);
  });

  it("keeps the live estimate for a pause discrepancy at 750 ms", () => {
    const clock = new PlaybackClock(0, 60_000);
    const previous = sample();
    const update = clock.apply(
      previous,
      sample({ isPlaying: false, status: "Paused", positionMs: 60_250 }),
      true,
      1_000,
    );
    expect(update).toMatchObject({ selectedPositionMs: 61_000, usedLivePosition: true });
    expect(clock.estimate(sample({ isPlaying: false }), 2_000)).toBe(61_000);
  });

  it("accepts a pause discrepancy larger than 750 ms as a seek", () => {
    const clock = new PlaybackClock(0, 60_000);
    const update = clock.apply(
      sample(),
      sample({ isPlaying: false, status: "Paused", positionMs: 60_249 }),
      true,
      1_000,
    );
    expect(update).toMatchObject({ selectedPositionMs: 60_249, usedLivePosition: false });
  });

  it("rejects a stale fallback sample that would rewind playing lyrics", () => {
    const clock = new PlaybackClock(0, 32_476);
    const update = clock.apply(
      sample({ positionMs: 32_476 }),
      sample({ positionMs: 126 }),
      true,
      2_966,
      false,
    );
    expect(update.selectedPositionMs).toBe(35_442);
    expect(clock.syncedPosition(sample(), 2_966)).toBe(35_442);
  });

  it("accepts a five-second seek from a fallback sample", () => {
    const clock = new PlaybackClock(0, 10_000);
    const update = clock.apply(
      sample({ positionMs: 10_000 }),
      sample({ positionMs: 16_000, playbackRate: 0 }),
      true,
      1_000,
      false,
    );
    expect(update.selectedPositionMs).toBe(16_000);
    expect(clock.syncedPosition(sample(), 1_000)).toBe(16_000);
  });

  it("always freezes the live estimate when the paused session is unavailable", () => {
    const clock = new PlaybackClock(0, 60_000);
    const update = clock.apply(
      sample(),
      sample({ isPlaying: false, status: "Paused session unavailable", positionMs: 1 }),
      true,
      1_000,
    );
    expect(update).toMatchObject({ selectedPositionMs: 61_000, usedLivePosition: true });
  });

  it("requires a progressing second sample to confirm resume", () => {
    const clock = new PlaybackClock(0, 60_000);
    const paused = sample({ isPlaying: false, status: "Paused session unavailable" });
    expect(clock.shouldDeferResume(paused, sample({ positionMs: 60_000 }), true)).toBe(true);
    expect(clock.shouldDeferResume(paused, sample({ positionMs: 60_099 }), true)).toBe(true);
    expect(clock.shouldDeferResume(paused, sample({ positionMs: 60_199 }), true)).toBe(false);
  });

  it("does not require confirmation for another playback variant", () => {
    const clock = new PlaybackClock(0, 60_000);
    expect(
      clock.shouldDeferResume(
        sample({ isPlaying: false, status: "Paused session unavailable" }),
        sample(),
        false,
      ),
    ).toBe(false);
  });

  it("does not wrap an advancing clock at a stale YouTube duration", () => {
    const clock = new PlaybackClock(0, 65_946);
    expect(clock.syncedPosition(sample({ durationMs: 64_000 }), 0)).toBe(65_946);
  });

  it("does not wrap when the clock advances beyond the reported duration", () => {
    const clock = new PlaybackClock(0, 190_000);
    expect(clock.syncedPosition(sample(), 0)).toBe(190_000);
  });

  it("does not clamp a paused clock to a stale duration", () => {
    const clock = new PlaybackClock(0, 190_000);
    expect(clock.syncedPosition(sample({ isPlaying: false }), 0)).toBe(190_000);
  });

  it("follows an authoritative position reset when playback loops", () => {
    const clock = new PlaybackClock(0, 179_000);
    clock.apply(sample({ positionMs: 179_000 }), sample({ positionMs: 1_000 }), true, 1_000);
    expect(clock.syncedPosition(sample({ positionMs: 1_000 }), 1_000)).toBe(1_000);
  });
});
