export const PAUSE_POSITION_TOLERANCE_MS = 750;
const PLAYING_FALLBACK_TOLERANCE_MS = 10_000;
export const RESUME_CONFIRMATION_PROGRESS_MS = 100;

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
      return { selectedPositionMs: 0, livePositionMs: null, usedLivePosition: false };
    }

    const wasPlaying = sameSong && previous.isPlaying;
    const livePositionMs = wasPlaying
      ? this.estimate(previous, sampledAtMs)
      : this.pausedPositionAnchorMs;
    this.sampledAtMs = sampledAtMs;

    if (media.isPlaying || !sameSong) {
      this.pausedPositionAnchorMs = null;
      const useLivePosition =
        media.isPlaying &&
        sameSong &&
        !allowPlayingDiscontinuity &&
        livePositionMs !== null &&
        Math.abs(media.positionMs - livePositionMs) > PLAYING_FALLBACK_TOLERANCE_MS;
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
