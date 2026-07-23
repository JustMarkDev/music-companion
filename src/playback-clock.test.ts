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

  it("keeps the live estimate for a pause discrepancy at 500 ms", () => {
    const clock = new PlaybackClock(0, 60_000);
    const previous = sample();
    const update = clock.apply(
      previous,
      sample({ isPlaying: false, status: "Paused", positionMs: 60_500 }),
      true,
      1_000,
    );
    expect(update).toMatchObject({ selectedPositionMs: 61_000, usedLivePosition: true });
    expect(clock.estimate(sample({ isPlaying: false }), 2_000)).toBe(61_000);
  });

  it("accepts a pause discrepancy larger than 500 ms as a seek", () => {
    const clock = new PlaybackClock(0, 60_000);
    const update = clock.apply(
      sample(),
      sample({ isPlaying: false, status: "Paused", positionMs: 60_499 }),
      true,
      1_000,
    );
    expect(update).toMatchObject({ selectedPositionMs: 60_499, usedLivePosition: false });
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

  it("wraps reliable loops only after the one-second grace period", () => {
    const clock = new PlaybackClock(0, 180_500);
    expect(clock.syncedPosition(sample(), 0, 170_000)).toBe(180_500);
    const wrappedClock = new PlaybackClock(0, 181_001);
    expect(wrappedClock.syncedPosition(sample(), 0, 170_000)).toBe(1_001);
  });

  it("does not wrap against a duration shorter than the lyric timeline", () => {
    const clock = new PlaybackClock(0, 190_000);
    expect(clock.syncedPosition(sample(), 0, 185_000)).toBe(190_000);
  });

  it("clamps a paused clock to a reliable duration", () => {
    const clock = new PlaybackClock(0, 190_000);
    expect(clock.syncedPosition(sample({ isPlaying: false }), 0, 170_000)).toBe(180_000);
  });
});
