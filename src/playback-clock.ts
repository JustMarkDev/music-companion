export const PAUSE_POSITION_TOLERANCE_MS = 750;
const PLAYING_FALLBACK_TOLERANCE_MS = 10_000;
export const RESUME_CONFIRMATION_PROGRESS_MS = 100;
// A position that jumps away from the clock is believed once a second sample, at least this
// long after the first, has moved on as time would have since, to within the tolerance. The
// tolerance stays below the gap: a sample that stands still is off by the whole gap, and must
// not be believed.
const DISCONTINUITY_CONFIRMATION_GAP_MS = 1_000;
const DISCONTINUITY_CONFIRMATION_TOLERANCE_MS = 600;

type PlaybackSample = {
  hasSession: boolean;
  isPlaying: boolean;
  status: string;
  positionMs: number;
  durationMs: number | null;
  playbackRate: number | null;
};

export type ClockUpdate = {
  selectedPositionMs: number;
  livePositionMs: number | null;
  usedLivePosition: boolean;
};

export class PlaybackClock {
  private sampledAtMs: number;
  private positionAnchorMs: number;
  private pausedPositionAnchorMs: number | null = null;
  private pendingResumePositionMs: number | null = null;
  private pendingDiscontinuity: { positionMs: number; sampledAtMs: number } | null = null;

  constructor(sampledAtMs: number, positionMs: number) {
    this.sampledAtMs = sampledAtMs;
    this.positionAnchorMs = positionMs;
  }

  estimate(media: PlaybackSample, nowMs: number) {
    if (!media.isPlaying) return this.positionAnchorMs;
    const elapsed = Math.max(0, nowMs - this.sampledAtMs);
    return this.positionAnchorMs + elapsed * playbackRate(media.playbackRate);
  }

  apply(
    previous: PlaybackSample,
    media: PlaybackSample,
    sameSong: boolean,
    sampledAtMs: number,
    allowPlayingDiscontinuity = true,
  ): ClockUpdate {
    if (!media.hasSession) {
      this.sampledAtMs = sampledAtMs;
      this.positionAnchorMs = 0;
      this.pausedPositionAnchorMs = null;
      this.pendingDiscontinuity = null;
      return { selectedPositionMs: 0, livePositionMs: null, usedLivePosition: false };
    }

    const wasPlaying = sameSong && previous.isPlaying;
    const livePositionMs = wasPlaying
      ? this.estimate(previous, sampledAtMs)
      : this.pausedPositionAnchorMs;
    this.sampledAtMs = sampledAtMs;

    if (media.isPlaying || !sameSong) {
      this.pausedPositionAnchorMs = null;
      const discontinuous =
        media.isPlaying &&
        sameSong &&
        !allowPlayingDiscontinuity &&
        livePositionMs !== null &&
        Math.abs(media.positionMs - livePositionMs) > PLAYING_FALLBACK_TOLERANCE_MS;
      // One odd sample is stale data, but a song that restarted or was sought keeps
      // reporting positions that move on from the first, which settles it.
      const useLivePosition = discontinuous && !this.confirmsDiscontinuity(media, sampledAtMs);
      this.pendingDiscontinuity = useLivePosition
        ? { positionMs: media.positionMs, sampledAtMs }
        : null;
      this.positionAnchorMs = useLivePosition ? livePositionMs : media.positionMs;
      return {
        selectedPositionMs: this.positionAnchorMs,
        livePositionMs,
        usedLivePosition: useLivePosition,
      };
    }

    const usedLivePosition =
      livePositionMs !== null &&
      (media.status === "Paused session unavailable" ||
        Math.abs(media.positionMs - livePositionMs) <= PAUSE_POSITION_TOLERANCE_MS);
    const selectedPositionMs =
      usedLivePosition && livePositionMs !== null ? livePositionMs : media.positionMs;
    this.pausedPositionAnchorMs = selectedPositionMs;
    this.positionAnchorMs = selectedPositionMs;
    return { selectedPositionMs, livePositionMs, usedLivePosition };
  }

  private confirmsDiscontinuity(media: PlaybackSample, sampledAtMs: number) {
    const pending = this.pendingDiscontinuity;
    if (!pending || sampledAtMs - pending.sampledAtMs < DISCONTINUITY_CONFIRMATION_GAP_MS) {
      return false;
    }
    const expected =
      pending.positionMs + (sampledAtMs - pending.sampledAtMs) * playbackRate(media.playbackRate);
    return Math.abs(media.positionMs - expected) <= DISCONTINUITY_CONFIRMATION_TOLERANCE_MS;
  }

  shouldDeferResume(previous: PlaybackSample, media: PlaybackSample, sameSong: boolean) {
    const resumingFromUnavailable =
      previous.status === "Paused session unavailable" && media.isPlaying && sameSong;
    if (!resumingFromUnavailable) {
      this.pendingResumePositionMs = null;
      return false;
    }

    if (
      this.pendingResumePositionMs !== null &&
      media.positionMs >= this.pendingResumePositionMs + RESUME_CONFIRMATION_PROGRESS_MS
    ) {
      this.pendingResumePositionMs = null;
      return false;
    }

    this.pendingResumePositionMs = media.positionMs;
    return true;
  }

  syncedPosition(media: PlaybackSample, nowMs: number) {
    return Math.max(0, this.estimate(media, nowMs));
  }

  pausedPosition() {
    return this.pausedPositionAnchorMs;
  }
}

function playbackRate(rate: number | null) {
  return typeof rate === "number" && Number.isFinite(rate) && rate > 0 ? rate : 1;
}
