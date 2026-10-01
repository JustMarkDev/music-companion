import { describe, test } from "vite-plus/test";
import { PlaybackClock } from "./playback-clock";

const playing = {
  hasSession: true,
  isPlaying: true,
  status: "Playing",
  positionMs: 60_000,
  durationMs: 180_000,
  playbackRate: 1,
};

const paused = {
  ...playing,
  isPlaying: false,
  status: "Paused",
  positionMs: 60_250,
};

describe("playback clock", () => {
  test("playback clock", async ({ bench }) => {
    await bench.compare(
      bench("playback_clock_estimate", () => {
        const clock = new PlaybackClock(1_000, 60_000);
        clock.estimate(playing, 1_016);
      }),
      bench("playback_clock_apply", () => {
        const clock = new PlaybackClock(0, 60_000);
        clock.apply(playing, paused, true, 1_000);
        clock.apply(paused, { ...playing, positionMs: 61_000 }, true, 2_000);
      }),
    );
  });
});
