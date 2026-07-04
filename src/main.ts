import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { createIcons, Lock, Maximize2, Menu, Minus, Settings, Unlock, X } from "lucide";
import packageJson from "../package.json";
import "./styles.css";

type MediaState = {
  hasSession: boolean;
  isPlaying: boolean;
  status: string;
  title: string;
  artist: string;
  album: string;
  sourceApp: string;
  positionMs: number;
  durationMs: number | null;
  playbackRate: number | null;
  playingSessionCount: number;
};

type LyricsResult = {
  source: string;
  trackName: string;
  artistName: string;
  albumName: string;
  duration: number | null;
  instrumental: boolean;
  syncedLyrics: string | null;
  plainLyrics: string | null;
};

type SettingsState = {
  clickThrough: boolean;
  showSongTitle: boolean;
  opacity: number;
  fontSize: number;
  lineSpacing: number;
  startAtLogin: boolean;
};

type LyricLine = {
  timeMs: number | null;
  endTimeMs: number | null;
  text: string;
  words: string[];
  emulated: boolean;
};

const DEFAULT_SETTINGS: SettingsState = {
  clickThrough: false,
  showSongTitle: false,
  opacity: 0.99,
  fontSize: 28,
  lineSpacing: 10,
  startAtLogin: false,
};

const SETTINGS_STORAGE_KEY = "music-companion-settings";
const POLLING_INTERVAL_MS = 100;
const SYNC_OFFSET_MS = 0;
const LOOP_DETECTION_GRACE_MS = 1_000;
const demoState: MediaState = {
  hasSession: true,
  isPlaying: true,
  status: "Playing",
  title: "Midnight Driver",
  artist: "Music Companion",
  album: "Local Preview",
  sourceApp: "Preview",
  positionMs: 36750,
  durationMs: 184000,
  playbackRate: 1,
  playingSessionCount: 1,
};

const demoLyrics = `[00:00.00] Waiting for a song
[00:12.20] The window catches the rhythm
[00:23.40] Every line finds its light
[00:36.90] Floating over work and play
[00:49.10] Music Companion keeps time
[01:03.00] The chorus arrives in color
[01:18.40] Then slips back into the night`;

const tauriAvailable = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const appWindow = tauriAvailable ? getCurrentWindow() : null;
const isSettingsWindow =
  appWindow?.label === "settings" ||
  new URLSearchParams(window.location.search).get("view") === "settings";
const lyricCache = new Map<string, LyricsResult | null>();

let settings = loadSettings();
let currentMedia: MediaState = demoState;
let currentTrackKey = "";
let lyricsLines: LyricLine[] = parseLyrics(demoLyrics);
let activeLineIndex = 2;
let lyricsMode: "synced" | "plain" | "instrumental" | "searching" | "missing" | "error" = "synced";
let settingsOpen = false;
let pollTimer = 0;
let animationFrame = 0;
let mediaSampledAtMs = performance.now();
let mediaPositionAnchorMs = demoState.positionMs;
let demoStartedAtMs = performance.now();
let renderedLyricsKey = "";
let lastScrolledLineIndex = -1;
let lyricDurationMs: number | null = demoState.durationMs;
let pollInFlight = false;
let pollStartedAtMs = 0;
let renderedChromeKey = "";
let renderedGradientKey = "";

document.querySelector<HTMLDivElement>("#app")!.innerHTML = `
  <main class="shell">
    <section class="overlay" id="overlay">
      <header class="chrome" id="chrome" data-drag-region>
        <div class="track-meta" data-drag-region>
          <h1 id="title">Music Companion</h1>
          <p id="artist">Waiting for media</p>
        </div>
        <div class="window-actions">
          <div class="secondary-actions">
            <button class="icon-button" id="lock-toggle" title="Enable click-through lock" aria-label="Enable click-through lock">
              <i data-lucide="unlock"></i>
            </button>
            <button class="icon-button" id="settings-toggle" title="Settings" aria-label="Settings">
              <i data-lucide="settings"></i>
            </button>
            <button class="icon-button" id="minimize" title="Minimize" aria-label="Minimize">
              <i data-lucide="minus"></i>
            </button>
            <button class="icon-button" id="maximize" title="Maximize" aria-label="Maximize">
              <i data-lucide="maximize-2"></i>
            </button>
          </div>
          <button class="icon-button danger" id="close" title="Hide" aria-label="Hide">
            <i data-lucide="x"></i>
          </button>
          <button class="icon-button compact-menu-toggle" id="compact-menu-toggle" title="Window menu" aria-label="Window menu" aria-expanded="false">
            <i data-lucide="menu"></i>
          </button>
          <div class="compact-menu" id="compact-menu" hidden>
            <button data-action="lock">Click through <span>Ctrl+Shift+L</span></button>
            <button data-action="settings">Settings</button>
            <button data-action="minimize">Minimize</button>
            <button data-action="maximize">Maximize</button>
            <button class="danger" data-action="close">Hide</button>
          </div>
        </div>
      </header>

      <div class="status-strip" id="status-strip" hidden></div>

      <section class="lyrics-viewport" id="lyrics-viewport" aria-live="polite">
        <div class="lyrics-list" id="lyrics-list"></div>
      </section>

      <aside class="settings-panel" id="settings-panel" hidden>
        <div class="settings-header">
          <h2>Settings</h2>
          <button class="icon-button" id="settings-close" title="Close settings" aria-label="Close settings">
            <i data-lucide="x"></i>
          </button>
        </div>
        <div class="setting-row">
          <label for="opacity">Opacity</label>
          <input id="opacity" type="range" min="80" max="100" step="1" />
        </div>
        <div class="setting-row">
          <label for="font-size">Lyric size</label>
          <input id="font-size" type="range" min="18" max="48" step="1" />
        </div>
        <div class="setting-row">
          <label for="line-spacing">Line spacing</label>
          <input id="line-spacing" type="range" min="2" max="26" step="1" />
        </div>
        <label class="switch-row" for="start-login">
          <span>Start at login</span>
          <input id="start-login" type="checkbox" />
        </label>
        <label class="switch-row" for="show-song-title">
          <span>Titolo della canzone</span>
          <input id="show-song-title" type="checkbox" />
        </label>
        <button class="save-settings" id="settings-save">Salva e chiudi</button>
        <p class="app-version">Versione ${packageJson.version}</p>
      </aside>
    </section>
  </main>
`;

createIcons({ icons: { Lock, Maximize2, Menu, Minus, Settings, Unlock, X } });
if (isSettingsWindow) {
  document.body.classList.add("settings-window");
  settingsOpen = true;
  wireUi();
  wireWindowEvents();
  applySettings();
  renderSettings();
} else {
  wireUi();
  wireWindowEvents();
  applySettings();
  renderAll();
  void syncStartAtLogin();
  schedulePolling();
  startSyncLoop();
}

function wireUi() {
  const overlay = document.querySelector<HTMLElement>("#overlay");
  overlay?.addEventListener("pointermove", (event) => {
    const rect = overlay.getBoundingClientRect();
    overlay.classList.toggle("controls-visible", event.clientY - rect.top <= 72);
  });
  overlay?.addEventListener("pointerleave", () => {
    if (!settingsOpen) {
      overlay.classList.remove("controls-visible");
    }
  });

  document.querySelector("#chrome")?.addEventListener("pointerdown", (event) => {
    if (
      appWindow &&
      event instanceof PointerEvent &&
      event.button === 0 &&
      event.target instanceof Element &&
      !event.target.closest("button")
    ) {
      void safeWindowAction(() => appWindow.startDragging());
    }
  });

  document.querySelector("#lyrics-viewport")?.addEventListener("dblclick", () => {
    openSettings();
  });

  document.querySelector("#lyrics-viewport")?.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    openSettings();
  });

  document.querySelector("#settings-toggle")?.addEventListener("click", () => {
    openSettings();
  });

  document.querySelector("#settings-close")?.addEventListener("click", () => {
    closeSettings();
  });

  document.querySelector("#settings-save")?.addEventListener("click", closeSettings);

  document.querySelector("#lock-toggle")?.addEventListener("click", () => {
    toggleOverlayLock();
  });

  document.querySelector("#minimize")?.addEventListener("click", () => {
    void safeWindowAction(() => appWindow?.minimize());
  });

  document.querySelector("#maximize")?.addEventListener("click", () => {
    void safeWindowAction(() => appWindow?.toggleMaximize());
  });

  document.querySelector("#close")?.addEventListener("click", () => {
    void safeWindowAction(() => appWindow?.hide());
  });

  const compactMenu = document.querySelector<HTMLElement>("#compact-menu");
  const compactMenuToggle = document.querySelector<HTMLButtonElement>("#compact-menu-toggle");
  const setCompactMenuOpen = (open: boolean) => {
    if (compactMenu) compactMenu.hidden = !open;
    compactMenuToggle?.setAttribute("aria-expanded", String(open));
  };
  compactMenuToggle?.addEventListener("click", (event) => {
    event.stopPropagation();
    setCompactMenuOpen(compactMenu?.hidden ?? true);
  });
  compactMenu?.addEventListener("click", (event) => {
    const action = (event.target as Element).closest<HTMLButtonElement>("[data-action]")?.dataset
      .action;
    if (!action) return;
    setCompactMenuOpen(false);
    if (action === "lock") toggleOverlayLock();
    if (action === "settings") {
      openSettings();
    }
    if (action === "minimize") void safeWindowAction(() => appWindow?.minimize());
    if (action === "maximize") void safeWindowAction(() => appWindow?.toggleMaximize());
    if (action === "close") void safeWindowAction(() => appWindow?.hide());
  });
  document.addEventListener("pointerdown", (event) => {
    if (!(event.target as Element).closest(".window-actions")) setCompactMenuOpen(false);
  });

  bindRange("opacity", (value) => {
    settings.opacity = value / 100;
    saveSettings();
    applySettings();
  });

  bindRange("font-size", (value) => {
    settings.fontSize = value;
    saveSettings();
    applySettings();
    renderLyrics();
  });

  bindRange("line-spacing", (value) => {
    settings.lineSpacing = value;
    saveSettings();
    applySettings();
  });

  document
    .querySelector<HTMLInputElement>("#start-login")
    ?.addEventListener("change", async (event) => {
      settings.startAtLogin = (event.currentTarget as HTMLInputElement).checked;
      saveSettings();
      if (tauriAvailable) {
        await invoke("set_start_at_login", { enabled: settings.startAtLogin });
      }
    });

  document
    .querySelector<HTMLInputElement>("#show-song-title")
    ?.addEventListener("change", (event) => {
      settings.showSongTitle = (event.currentTarget as HTMLInputElement).checked;
      saveSettings();
      applySettings();
    });
}

function wireWindowEvents() {
  if (!tauriAvailable) {
    return;
  }
  if (isSettingsWindow) {
    void listen("settings-window-opened", () => {
      settings = loadSettings();
      applySettings();
      renderSettings();
    });
    return;
  }

  void listen("overlay-unlocked", () => {
    settings.clickThrough = false;
    saveSettings();
    applySettings();
    renderChrome();
  });
  void listen("toggle-overlay-lock", toggleOverlayLock);
  void listen<SettingsState>("settings-updated", () => {
    settings = loadSettings();
    applySettings();
    renderSettings();
  });
}

function openSettings() {
  if (tauriAvailable && !isSettingsWindow) {
    void safeInvoke("show_settings_window");
    return;
  }
  settingsOpen = true;
  renderSettings();
}

function closeSettings() {
  if (isSettingsWindow && appWindow) {
    void safeWindowAction(() => appWindow.hide());
    return;
  }
  settingsOpen = false;
  renderSettings();
}

function toggleOverlayLock() {
  settings.clickThrough = !settings.clickThrough;
  saveSettings();
  applySettings();
  renderChrome();
}

function bindRange(id: string, onChange: (value: number) => void) {
  document.querySelector<HTMLInputElement>(`#${id}`)?.addEventListener("input", (event) => {
    onChange(Number((event.currentTarget as HTMLInputElement).value));
  });
}

async function syncStartAtLogin() {
  if (!tauriAvailable) {
    return;
  }

  try {
    settings.startAtLogin = await invoke<boolean>("get_start_at_login");
    saveSettings();
    renderSettings();
  } catch {
    renderSettings();
  }
}

function schedulePolling() {
  window.clearInterval(pollTimer);
  window.requestAnimationFrame(() => {
    window.setTimeout(() => {
      void pollMedia();
      pollTimer = window.setInterval(() => void pollMedia(), POLLING_INTERVAL_MS);
    }, 0);
  });
}

async function pollMedia() {
  if (pollInFlight) {
    if (pollStartedAtMs && performance.now() - pollStartedAtMs > 3000) {
      showStatus("Waiting for Windows media session...");
    }
    return;
  }

  pollInFlight = true;
  pollStartedAtMs = performance.now();
  try {
    if (!tauriAvailable) {
      renderChrome();
      renderStatus();
      applyGradient();
      return;
    }

    const nextMedia = await invoke<MediaState>("get_media_state");
    const sampledAtMs = performance.now();
    const nextTrackKey = trackKey(nextMedia);
    syncMediaClock(nextMedia, sampledAtMs);
    currentMedia = nextMedia;

    if (nextTrackKey && nextTrackKey !== currentTrackKey) {
      currentTrackKey = nextTrackKey;
      void loadLyrics(currentMedia, nextTrackKey);
    }

    if (!nextTrackKey) {
      currentTrackKey = "";
      lyricsLines = [];
      lyricsMode = "missing";
      invalidateLyricsRender();
    }

    renderChrome();
    renderLyrics();
    renderStatus();
    applyGradient();
  } catch (error) {
    showStatus(`Media bridge error: ${String(error)}`);
  } finally {
    pollInFlight = false;
    pollStartedAtMs = 0;
  }
}

async function loadLyrics(media: MediaState, expectedTrackKey = trackKey(media)) {
  if (!media.hasSession || !media.title) {
    if (currentTrackKey === expectedTrackKey) {
      lyricsLines = [];
      lyricsMode = "missing";
    }
    return;
  }

  const key = trackKey(media);
  if (lyricCache.has(key)) {
    if (currentTrackKey === key) {
      applyLyrics(lyricCache.get(key) ?? null);
    }
    return;
  }

  if (currentTrackKey === key) {
    lyricsLines = [];
    lyricsMode = "searching";
    invalidateLyricsRender();
    renderLyrics();
  }

  try {
    const result = await invoke<LyricsResult | null>("fetch_lyrics", {
      title: media.title,
      artist: media.artist,
      album: media.album,
      durationMs: media.durationMs,
    });
    lyricCache.set(key, result);
    if (currentTrackKey === key) {
      applyLyrics(result);
    }
  } catch (error) {
    if (currentTrackKey === key) {
      lyricsLines = [];
      lyricsMode = "error";
      invalidateLyricsRender();
      renderLyrics();
      showStatus(`Lyrics search failed: ${String(error)}`);
    }
  }
}

function applyLyrics(result: LyricsResult | null) {
  if (!result) {
    lyricsLines = [];
    lyricsMode = "missing";
    invalidateLyricsRender();
    return;
  }

  if (result.instrumental) {
    lyricDurationMs = getResultDurationMs(result);
    lyricsLines = [createLyricLine(null, "Instrumental")];
    lyricsMode = "instrumental";
    invalidateLyricsRender();
    return;
  }

  if (result.syncedLyrics) {
    lyricDurationMs = getResultDurationMs(result);
    lyricsLines = ensureAnimatedTimings(parseLyrics(result.syncedLyrics));
    lyricsMode = "synced";
    invalidateLyricsRender();
    return;
  }

  if (result.plainLyrics) {
    lyricDurationMs = getResultDurationMs(result);
    lyricsLines = emulateLineTimings(
      result.plainLyrics
        .split(/\r?\n/)
        .map((text) => createLyricLine(null, text.trim()))
        .filter((line) => line.text.length > 0),
      getEffectiveDurationMs(),
    );
    lyricsMode = "plain";
    invalidateLyricsRender();
    return;
  }

  lyricsLines = [];
  lyricsMode = "missing";
  invalidateLyricsRender();
}

function parseTimeParts(minutes: string, seconds: string, fraction = "0") {
  return (
    Number(minutes) * 60_000 + Number(seconds) * 1000 + Number(fraction.padEnd(3, "0").slice(0, 3))
  );
}

function splitWords(value: string) {
  return value.trim().match(/\S+/g) ?? [];
}

function estimateLineDuration(line: LyricLine) {
  return clamp(line.words.length * 460, 1400, 7200);
}

function getResultDurationMs(result: LyricsResult) {
  return result.duration ? result.duration * 1000 : currentMedia.durationMs;
}

function parseLyrics(raw: string): LyricLine[] {
  const lines: LyricLine[] = [];
  const pattern = /\[(\d{1,2}):(\d{2})(?:[.:](\d{1,3}))?\]/g;
  const metadataPattern = /^\[[a-z]+:/i;

  for (const rawLine of raw.split(/\r?\n/)) {
    if (metadataPattern.test(rawLine.trim())) {
      continue;
    }

    const matches = [...rawLine.matchAll(pattern)];
    const textWithWordTags = rawLine.replace(pattern, "").trim();

    if (matches.length === 0 && textWithWordTags) {
      lines.push(createLyricLine(null, textWithWordTags));
      continue;
    }

    for (const match of matches) {
      lines.push(createLyricLine(parseTimeParts(match[1], match[2], match[3]), textWithWordTags));
    }
  }

  return finalizeLyricTimings(lines.sort((a, b) => (a.timeMs ?? 0) - (b.timeMs ?? 0)));
}

function createLyricLine(timeMs: number | null, textWithWordTags: string): LyricLine {
  const text = stripWordTags(textWithWordTags);
  return {
    timeMs,
    endTimeMs: null,
    text: text || " ",
    words: splitWords(text),
    emulated: false,
  };
}

function ensureAnimatedTimings(lines: LyricLine[]) {
  if (lines.some((line) => line.timeMs !== null)) {
    return lines;
  }

  return emulateLineTimings(lines, getEffectiveDurationMs(lines));
}

function stripWordTags(textWithWordTags: string) {
  return textWithWordTags
    .replace(/<\d{1,2}:\d{2}(?:[.:]\d{1,3})?>/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

function finalizeLyricTimings(lines: LyricLine[]) {
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.timeMs === null) {
      continue;
    }

    const nextTimedLine = lines.slice(index + 1).find((candidate) => candidate.timeMs !== null);
    const fallbackEnd = line.timeMs + estimateLineDuration(line);
    line.endTimeMs = Math.max(line.timeMs + 320, nextTimedLine?.timeMs ?? fallbackEnd);
  }

  return lines;
}

function emulateLineTimings(lines: LyricLine[], durationMs: number) {
  if (lines.length === 0) {
    return lines;
  }

  const weights = lines.map((line) =>
    Math.max(2, line.words.length || splitWords(line.text).length),
  );
  const totalWeight = weights.reduce((total, weight) => total + weight, 0);
  let cursor = 0;

  for (let index = 0; index < lines.length; index += 1) {
    const lineDuration =
      index === lines.length - 1
        ? durationMs - cursor
        : (durationMs * weights[index]) / totalWeight;
    const endTimeMs = index === lines.length - 1 ? durationMs : cursor + lineDuration;

    lines[index].timeMs = cursor;
    lines[index].endTimeMs = Math.max(cursor + 900, Math.min(durationMs, endTimeMs));
    lines[index].emulated = true;
    cursor = lines[index].endTimeMs ?? cursor;
  }

  return lines;
}

function updateActiveLine(positionMs = getSyncedPositionMs()) {
  if (lyricsLines.length === 0) {
    activeLineIndex = -1;
    return;
  }

  const timed = lyricsLines.some((line) => line.timeMs !== null);
  if (!timed) {
    const ratio = clamp(positionMs / getEffectiveDurationMs(), 0, 0.98);
    activeLineIndex = Math.floor(ratio * lyricsLines.length);
    return;
  }

  let nextIndex = -1;
  for (let index = 0; index < lyricsLines.length; index += 1) {
    const time = lyricsLines[index].timeMs;
    if (time !== null && time <= positionMs) {
      nextIndex = index;
    }
  }
  activeLineIndex = nextIndex;
}

function getSyncedPositionMs() {
  if (!tauriAvailable) {
    const duration = currentMedia.durationMs ?? 180_000;
    const elapsed = performance.now() - demoStartedAtMs;
    return (demoState.positionMs + elapsed + SYNC_OFFSET_MS + duration) % duration;
  }

  const position = getEstimatedMediaPositionMs(performance.now()) + SYNC_OFFSET_MS;
  const durationMs = lyricDurationMs ?? currentMedia.durationMs;
  if (
    currentMedia.isPlaying &&
    durationMs &&
    position >= durationMs + LOOP_DETECTION_GRACE_MS
  ) {
    return position % durationMs;
  }
  // WMTC duration metadata can be stale while a track is playing. Clamping an
  // advancing clock to it freezes lyric synchronization until the session is
  // refreshed (for example, by pausing and resuming).
  return durationMs && !currentMedia.isPlaying
    ? clamp(position, 0, durationMs)
    : Math.max(0, position);
}

function getEstimatedMediaPositionMs(nowMs: number) {
  if (!currentMedia.isPlaying) {
    return mediaPositionAnchorMs;
  }

  const elapsed = Math.max(0, nowMs - mediaSampledAtMs);
  return mediaPositionAnchorMs + elapsed * getPlaybackRate();
}

function getPlaybackRate() {
  const rate = currentMedia.playbackRate;
  return typeof rate === "number" && Number.isFinite(rate) && rate > 0 ? rate : 1;
}

function syncMediaClock(media: MediaState, sampledAtMs: number) {
  if (!media.hasSession) {
    mediaSampledAtMs = sampledAtMs;
    mediaPositionAnchorMs = 0;
    return;
  }

  // The backend has already converted WMTC Position + LastUpdatedTime +
  // PlaybackRate into the position at sampling time. Treat every sample as
  // authoritative. This naturally handles forward seeks, backward seeks and
  // repeat-one without trying to infer which kind of discontinuity occurred.
  mediaSampledAtMs = sampledAtMs;
  mediaPositionAnchorMs = media.positionMs;
}

function renderAll() {
  renderChrome();
  renderLyrics();
  renderSettings();
  renderStatus();
  applyGradient();
}

function renderChrome() {
  const nextChromeKey = [
    currentMedia.hasSession,
    currentMedia.title,
    currentMedia.artist,
    settings.clickThrough,
  ].join("|");
  if (nextChromeKey === renderedChromeKey) {
    return;
  }

  renderedChromeKey = nextChromeKey;
  const title = document.querySelector("#title")!;
  const artist = document.querySelector("#artist")!;
  const lockToggle = document.querySelector("#lock-toggle")!;

  const titleText = currentMedia.hasSession
    ? currentMedia.title || "Unknown track"
    : "No media session";
  const artistText = currentMedia.hasSession
    ? currentMedia.artist || "Unknown artist"
    : "Play something";
  title.textContent = titleText;
  title.setAttribute("title", titleText);
  artist.textContent = artistText;
  artist.setAttribute("title", artistText);
  lockToggle.innerHTML = settings.clickThrough
    ? `<i data-lucide="lock"></i>`
    : `<i data-lucide="unlock"></i>`;
  lockToggle.setAttribute(
    "title",
    settings.clickThrough ? "Disable click-through lock" : "Enable click-through lock",
  );
  lockToggle.setAttribute(
    "aria-label",
    settings.clickThrough ? "Disable click-through lock" : "Enable click-through lock",
  );
  createIcons({ icons: { Lock, Unlock } });
}

function renderLyrics() {
  const list = document.querySelector<HTMLDivElement>("#lyrics-list")!;
  const nextRenderKey = getLyricsRenderKey();

  if (nextRenderKey === renderedLyricsKey) {
    return;
  }

  renderedLyricsKey = nextRenderKey;
  lastScrolledLineIndex = -1;

  if (!currentMedia.hasSession) {
    list.innerHTML = `<p class="empty-state">Play something in a Windows media app.</p>`;
    return;
  }

  if (lyricsMode === "searching") {
    list.innerHTML = `
      <p class="empty-state lyrics-searching" aria-label="Searching for lyrics">
        <span>Searching for lyrics</span><span class="searching-dots" aria-hidden="true"><i></i><i></i><i></i></span>
      </p>`;
    return;
  }

  if (lyricsMode === "error") {
    list.innerHTML = `<p class="empty-state">Unable to search for lyrics.</p>`;
    return;
  }

  if (lyricsMode === "missing" || lyricsLines.length === 0) {
    list.innerHTML = `<p class="empty-state">No lyrics found.</p>`;
    return;
  }

  list.innerHTML = lyricsLines
    .map((line, index) => {
      const distance = Math.abs(index - activeLineIndex);
      const showNeighborFocus = lyricsMode !== "synced";
      const className = [
        "lyric-line",
        index === activeLineIndex ? "active" : "",
        showNeighborFocus && distance === 1 ? "near" : "",
        distance > 4 ? "far" : "",
      ]
        .filter(Boolean)
        .join(" ");
      return `<p class="${className}" data-line-index="${index}">${escapeHtml(line.text)}</p>`;
    })
    .join("");

  updateSyncFrame();
}

function startSyncLoop() {
  window.cancelAnimationFrame(animationFrame);
  const tick = () => {
    updateSyncFrame();
    animationFrame = window.requestAnimationFrame(tick);
  };
  animationFrame = window.requestAnimationFrame(tick);
}

function updateSyncFrame() {
  const previousActiveLineIndex = activeLineIndex;
  const positionMs = getSyncedPositionMs();
  updateActiveLine(positionMs);
  if (activeLineIndex !== previousActiveLineIndex || lastScrolledLineIndex === -1) {
    updateLyricDom();
  }
}

function updateLyricDom() {
  const list = document.querySelector<HTMLDivElement>("#lyrics-list");
  if (!list || lyricsLines.length === 0) {
    return;
  }

  const lineElements = list.querySelectorAll<HTMLElement>(".lyric-line");
  lineElements.forEach((lineElement) => {
    const lineIndex = Number(lineElement.dataset.lineIndex);
    const line = lyricsLines[lineIndex];
    const distance = Math.abs(lineIndex - activeLineIndex);
    const showNeighborFocus = lyricsMode !== "synced";

    lineElement.classList.toggle("active", lineIndex === activeLineIndex);
    lineElement.classList.toggle("near", showNeighborFocus && distance === 1);
    lineElement.classList.toggle("far", distance > 4);
    lineElement.classList.toggle("emulated", Boolean(line?.emulated));
  });

  if (activeLineIndex !== lastScrolledLineIndex) {
    lastScrolledLineIndex = activeLineIndex;
    if (activeLineIndex < 0) {
      list.scrollTo({ top: 0, behavior: "auto" });
      return;
    }
    list
      .querySelector<HTMLElement>(`.lyric-line[data-line-index="${activeLineIndex}"]`)
      ?.scrollIntoView({ behavior: "smooth", block: "center" });
  }
}

function getLyricsRenderKey() {
  if (!currentMedia.hasSession) {
    return "no-session";
  }

  if (lyricsMode === "searching" || lyricsMode === "missing" || lyricsMode === "error") {
    return `${lyricsMode}:${currentTrackKey}`;
  }

  if (lyricsLines.length === 0) {
    return `missing:${currentTrackKey}`;
  }

  return `${lyricsMode}:${lyricsLines
    .map((line) => `${line.timeMs ?? "x"}:${line.text}:${line.words.length}`)
    .join("|")}`;
}

function getEffectiveDurationMs(lines = lyricsLines) {
  return currentMedia.durationMs ?? lyricDurationMs ?? estimateLyricsDuration(lines);
}

function estimateLyricsDuration(lines: LyricLine[]) {
  const wordCount = lines.reduce((total, line) => total + Math.max(1, line.words.length), 0);
  return clamp(wordCount * 430, 30_000, 8 * 60_000);
}

function invalidateLyricsRender() {
  renderedLyricsKey = "";
}

function renderSettings() {
  const panel = document.querySelector<HTMLElement>("#settings-panel")!;
  const overlay = document.querySelector<HTMLElement>("#overlay");
  overlay?.classList.toggle("settings-open", settingsOpen);
  overlay?.classList.toggle("controls-visible", settingsOpen);
  panel.hidden = !settingsOpen;

  document.querySelector<HTMLInputElement>("#opacity")!.value = String(
    Math.round(settings.opacity * 100),
  );
  document.querySelector<HTMLInputElement>("#font-size")!.value = String(settings.fontSize);
  document.querySelector<HTMLInputElement>("#line-spacing")!.value = String(settings.lineSpacing);
  document.querySelector<HTMLInputElement>("#start-login")!.checked = settings.startAtLogin;
  document.querySelector<HTMLInputElement>("#show-song-title")!.checked = settings.showSongTitle;
}

function renderStatus() {
  const status = document.querySelector<HTMLElement>("#status-strip")!;
  const messages = [];

  if (currentMedia.playingSessionCount > 1) {
    messages.push("Multiple players are active");
  }

  status.textContent = messages.join("  |  ");
  status.hidden = messages.length === 0;
}

function showStatus(message: string) {
  const status = document.querySelector<HTMLElement>("#status-strip")!;
  status.textContent = message;
  status.hidden = false;
}

function applySettings() {
  const root = document.documentElement;
  const overlay = document.querySelector<HTMLElement>("#overlay");
  root.style.setProperty("--overlay-opacity", String(settings.opacity));
  root.style.setProperty("--lyric-size", `${settings.fontSize}px`);
  root.style.setProperty("--line-spacing", `${settings.lineSpacing}px`);
  overlay?.classList.toggle("click-through", settings.clickThrough);
  overlay?.classList.toggle("hide-locked-title", !settings.showSongTitle);
  if (settings.clickThrough) {
    overlay?.classList.remove("controls-visible");
  }
  void applyOverlayInteractivity();
}

async function applyOverlayInteractivity() {
  if (!appWindow || isSettingsWindow) {
    return;
  }
  try {
    await appWindow.setAlwaysOnTop(true);
  } catch {
    await safeInvoke("set_always_on_top", { enabled: true });
  }

  await safeWindowAction(() => appWindow.setIgnoreCursorEvents(settings.clickThrough));
}

async function safeWindowAction(action: () => Promise<void> | undefined) {
  try {
    await action();
  } catch (error) {
    showStatus(`Window action failed: ${String(error)}`);
  }
}

async function safeInvoke(command: string, args?: Record<string, unknown>) {
  try {
    await invoke(command, args);
  } catch (error) {
    showStatus(`Window command failed: ${String(error)}`);
  }
}

function applyGradient() {
  const nextGradientKey = `${currentMedia.artist}:${currentMedia.title}`;
  if (nextGradientKey === renderedGradientKey) {
    return;
  }
  renderedGradientKey = nextGradientKey;
  const hue = hashHue(nextGradientKey);
  document.documentElement.style.setProperty("--hue", String(hue));
  document.documentElement.style.setProperty("--hue-2", String((hue + 128) % 360));
}

function loadSettings(): SettingsState {
  try {
    const stored = localStorage.getItem(SETTINGS_STORAGE_KEY);
    const loaded = stored ? JSON.parse(stored) : {};
    return {
      clickThrough: Boolean(loaded.clickThrough),
      showSongTitle:
        typeof loaded.showSongTitle === "boolean"
          ? loaded.showSongTitle
          : DEFAULT_SETTINGS.showSongTitle,
      opacity: loadNumericSetting(loaded.opacity, DEFAULT_SETTINGS.opacity, 0.8, 1),
      fontSize: loadNumericSetting(loaded.fontSize, DEFAULT_SETTINGS.fontSize, 18, 48),
      lineSpacing: loadNumericSetting(loaded.lineSpacing, DEFAULT_SETTINGS.lineSpacing, 2, 26),
      startAtLogin:
        typeof loaded.startAtLogin === "boolean"
          ? loaded.startAtLogin
          : DEFAULT_SETTINGS.startAtLogin,
    };
  } catch {
    return { ...DEFAULT_SETTINGS };
  }
}

function saveSettings() {
  localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify(settings));
  if (tauriAvailable && isSettingsWindow) {
    void emit("settings-updated", settings);
  }
}

function loadNumericSetting(value: unknown, fallback: number, minimum: number, maximum: number) {
  return typeof value === "number" && Number.isFinite(value)
    ? clamp(value, minimum, maximum)
    : fallback;
}

function trackKey(media: MediaState) {
  if (!media.hasSession || !media.title) {
    return "";
  }
  // Album metadata is not stable across all WMTC providers and may briefly
  // disappear during a seek. It must not make the same track look new.
  return `${normalizeTrackField(media.artist)}::${normalizeTrackField(media.title)}`;
}

function normalizeTrackField(value: string) {
  return value.trim().replace(/\s+/g, " ").toLowerCase();
}

function hashHue(input: string) {
  let hash = 0;
  for (let index = 0; index < input.length; index += 1) {
    hash = (hash * 31 + input.charCodeAt(index)) | 0;
  }
  return Math.abs(hash) % 360;
}

function escapeHtml(value: string) {
  return value.replace(/[&<>"']/g, (char) => {
    const map: Record<string, string> = {
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#039;",
    };
    return map[char];
  });
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}
