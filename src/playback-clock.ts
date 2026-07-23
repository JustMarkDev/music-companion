const LOOP_DETECTION_GRACE_MS = 1_000;
export const PAUSE_POSITION_TOLERANCE_MS = 500;
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
    sameVariant: boolean,
    sampledAtMs: number,
  ): ClockUpdate {
    if (!media.hasSession) {
      this.sampledAtMs = sampledAtMs;
      this.positionAnchorMs = 0;
      this.pausedPositionAnchorMs = null;
      return { selectedPositionMs: 0, livePositionMs: null, usedLivePosition: false };
    }

    const wasPlaying = sameVariant && previous.isPlaying;
    const livePositionMs = wasPlaying
      ? this.estimate(previous, sampledAtMs)
      : this.pausedPositionAnchorMs;
    this.sampledAtMs = sampledAtMs;

    if (media.isPlaying || !sameVariant) {
      this.pausedPositionAnchorMs = null;
      this.positionAnchorMs = media.positionMs;
      return {
        selectedPositionMs: media.positionMs,
        livePositionMs,
        usedLivePosition: false,
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

  shouldDeferResume(previous: PlaybackSample, media: PlaybackSample, sameVariant: boolean) {
    const resumingFromUnavailable =
      previous.status === "Paused session unavailable" && media.isPlaying && sameVariant;
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

  syncedPosition(media: PlaybackSample, nowMs: number, lastTimedLineMs: number) {
    const position = this.estimate(media, nowMs);
    const duration = reliableLoopDuration(media.durationMs, lastTimedLineMs);
    if (media.isPlaying && duration && position >= duration + LOOP_DETECTION_GRACE_MS) {
      return position % duration;
    }
    if (duration && !media.isPlaying) return clamp(position, 0, duration);
    return Math.max(0, position);
  }

  pausedPosition() {
    return this.pausedPositionAnchorMs;
  }
}

function reliableLoopDuration(durationMs: number | null, lastTimedLineMs: number) {
  return durationMs && durationMs >= lastTimedLineMs ? durationMs : null;
}

function playbackRate(rate: number | null) {
  return typeof rate === "number" && Number.isFinite(rate) && rate > 0 ? rate : 1;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}
