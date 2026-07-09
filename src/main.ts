import { invoke } from "@tauri-apps/api/core";
import { PhysicalSize } from "@tauri-apps/api/dpi";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { createIcons, Maximize2, Menu, Minus, Settings, X } from "lucide";
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

type AccentMode = "dynamic" | "manual";

type SettingsState = {
  clickThrough: boolean;
  opacity: number;
  blurIntensity: number;
  fontSize: number;
  lineSpacing: number;
  startAtLogin: boolean;
  accentMode: AccentMode;
  accentColor: string;
};

type CachedLyrics = {
  cachedAt: number;
  result: LyricsResult;
};

type LyricLine = {
  timeMs: number | null;
  endTimeMs: number | null;
  text: string;
  words: string[];
};

const DEFAULT_SETTINGS: SettingsState = {
  clickThrough: false,
  opacity: 0.99,
  blurIntensity: 100,
  fontSize: 1.5,
  lineSpacing: 0.4,
  startAtLogin: false,
  accentMode: "dynamic",
  accentColor: "#22e6c7",
};

const SETTINGS_STORAGE_KEY = "music-companion-settings";
const LYRICS_CACHE_STORAGE_KEY = "music-companion-lyrics-cache-v1";
const MAX_PERSISTED_LYRICS = 200;
const POLLING_INTERVAL_MS = 2_000;
const SYNC_OFFSET_MS = 0;
const LOOP_DETECTION_GRACE_MS = 1_000;
const PAUSE_POSITION_TOLERANCE_MS = 750;
const RESUME_CONFIRMATION_DELAY_MS = 250;
const RESUME_CONFIRMATION_PROGRESS_MS = 100;
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
const lyricCache = loadLyricsCache();
const lyricRequests = new Map<string, Promise<LyricsResult | null>>();
let lyricCacheGeneration = 0;

let settings = loadSettings();
let currentMedia: MediaState = demoState;
let currentTrackKey = "";
let lyricsLines: LyricLine[] = parseLyrics(demoLyrics);
let activeLineIndex = 2;
let lyricsMode:
  | "synced"
  | "unsynced"
  | "instrumental"
  | "excluded"
  | "searching"
  | "missing"
  | "error" = "synced";
let lyricsNotice = "";
let settingsOpen = false;
let pollTimer = 0;
let animationFrame = 0;
let mediaSampledAtMs = performance.now();
let mediaPositionAnchorMs = demoState.positionMs;
const demoStartedAtMs = performance.now();
let renderedLyricsKey = "";
let lastScrolledLineIndex = -1;
let pollInFlight = false;
let pollQueued = false;
let pollStartedAtMs = 0;
let mediaEventSequence = 0;
let pausedPositionAnchorMs: number | null = null;
let resumeConfirmationTimer = 0;
let pendingResumeConfirmation: { positionMs: number } | null = null;
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
            <button data-action="settings">Settings</button>
            <button data-action="minimize">Minimize</button>
            <button data-action="maximize">Maximize</button>
            <button data-action="hide">Hide</button>
            <button class="danger" data-action="close">Close</button>
          </div>
        </div>
      </header>

      <div class="status-strip" id="status-strip" hidden></div>

      <section class="lyrics-viewport" id="lyrics-viewport" aria-live="polite">
        <div class="lyrics-list" id="lyrics-list"></div>
      </section>

      <aside class="settings-panel" id="settings-panel" hidden>
        <div class="settings-header" id="settings-header">
          <h2>Settings</h2>
          <button class="icon-button" id="settings-close" title="Close settings" aria-label="Close settings">
            <i data-lucide="x"></i>
          </button>
        </div>
        <div class="setting-row">
          <label class="range-label" for="opacity">
            <span>Opacity</span>
            <output id="opacity-value">99%</output>
          </label>
          <input id="opacity" type="range" min="0" max="100" step="1" />
        </div>
        <div class="setting-row">
          <label class="range-label" for="blur-intensity">
            <span>Blur intensity</span>
            <output id="blur-intensity-value">100%</output>
          </label>
          <input id="blur-intensity" type="range" min="1" max="100" step="1" />
        </div>
        <div class="setting-row">
          <label class="range-label" for="font-size">
            <span>Lyric size</span>
            <output id="font-size-value">1.5rem</output>
          </label>
          <input id="font-size" type="range" min="0.5" max="3" step="0.05" />
        </div>
        <div class="setting-row">
          <label class="range-label" for="line-spacing">
            <span>Line spacing</span>
            <output id="line-spacing-value">0.4em</output>
          </label>
          <input id="line-spacing" type="range" min="0.1" max="1.2" step="0.05" />
        </div>
        <div class="setting-row accent-color-setting">
          <label for="accent-mode">Accent color</label>
          <select id="accent-mode">
            <option value="dynamic">Dynamic</option>
            <option value="manual">Manual</option>
          </select>
          <div class="manual-accent-controls" id="manual-accent-controls">
            <input id="accent-color" type="color" aria-label="Choose accent color" />
            <input id="accent-color-hex" type="text" inputmode="text" maxlength="7" aria-label="Accent color hex value" />
          </div>
        </div>
        <label class="switch-row" for="start-login">
          <span>Start at login</span>
          <input id="start-login" type="checkbox" />
        </label>
        <div class="setting-row hotkey-setting">
          <label for="click-through-hotkey">Toggle Pinned Mode</label>
          <input id="click-through-hotkey" type="text" value="Ctrl+Shift+L" readonly />
        </div>
        <button class="save-settings" id="settings-save">Salva e chiudi</button>
        <button class="clear-cache-button" id="clear-lyrics-cache">Clear cache</button>
        <p class="app-version">Versione ${packageJson.version}</p>
      </aside>
    </section>
  </main>
`;

createIcons({ icons: { Maximize2, Menu, Minus, Settings, X } });
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

  document.querySelector("#settings-header")?.addEventListener("pointerdown", (event) => {
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
  let compactMenuWindowSize: PhysicalSize | null = null;
  let compactMenuTransition = Promise.resolve();
  let compactMenuRequestedOpen = false;
  const runCompactMenuAction = (action: string) => {
    if (action === "settings") openSettings();
    if (action === "minimize") void safeWindowAction(() => appWindow?.minimize());
    if (action === "maximize") void safeWindowAction(() => appWindow?.toggleMaximize());
    if (action === "hide") void safeWindowAction(() => appWindow?.hide());
    if (action === "close") void safeInvoke("quit_app");
  };
  const expandWindowForCompactMenu = async () => {
    if (!appWindow || !compactMenu || compactMenuWindowSize || (await appWindow.isMaximized())) {
      return;
    }

    const [size, scaleFactor] = await Promise.all([appWindow.innerSize(), appWindow.scaleFactor()]);
    const menuBottom = compactMenu.getBoundingClientRect().bottom;
    const extraHeight = Math.ceil(Math.max(0, menuBottom + 8 - window.innerHeight) * scaleFactor);
    if (extraHeight === 0) return;

    compactMenuWindowSize = size;
    await appWindow.setSize(new PhysicalSize(size.width, size.height + extraHeight));
  };
  const restoreWindowAfterCompactMenu = async () => {
    if (appWindow && compactMenuWindowSize) {
      const size = compactMenuWindowSize;
      await appWindow.setSize(size);
      compactMenuWindowSize = null;
    }
  };
  const applyCompactMenuOpen = async (open: boolean) => {
    if (compactMenu) {
      compactMenu.hidden = !open;
    }
    compactMenuToggle?.setAttribute("aria-expanded", String(open));
    if (open) {
      try {
        await expandWindowForCompactMenu();
      } catch (error) {
        if (compactMenu) compactMenu.hidden = true;
        compactMenuToggle?.setAttribute("aria-expanded", "false");
        showStatus(`Window menu failed: ${String(error)}`);
      }
    } else {
      await restoreWindowAfterCompactMenu();
    }
  };
  const setCompactMenuOpen = (open: boolean) => {
    compactMenuRequestedOpen = open;
    compactMenuTransition = compactMenuTransition
      .catch(() => undefined)
      .then(() => applyCompactMenuOpen(open))
      .catch((error) => {
        showStatus(`Window menu failed: ${String(error)}`);
      });
    return compactMenuTransition;
  };
  compactMenuToggle?.addEventListener("click", (event) => {
    event.stopPropagation();
    void setCompactMenuOpen(!compactMenuRequestedOpen);
  });
  compactMenu?.addEventListener("click", async (event) => {
    const action = (event.target as Element).closest<HTMLButtonElement>("[data-action]")?.dataset
      .action;
    if (!action) return;
    await setCompactMenuOpen(false);
    runCompactMenuAction(action);
  });
  document.addEventListener("pointerdown", (event) => {
    if (!(event.target as Element).closest(".window-actions")) void setCompactMenuOpen(false);
  });

  bindRange("opacity", (value) => {
    settings.opacity = value / 100;
    saveSettings();
    applySettings();
  });

  bindRange("blur-intensity", (value) => {
    settings.blurIntensity = value;
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

  document.querySelector<HTMLSelectElement>("#accent-mode")?.addEventListener("change", (event) => {
    settings.accentMode =
      (event.currentTarget as HTMLSelectElement).value === "manual" ? "manual" : "dynamic";
    saveSettings();
    applySettings();
    renderSettings();
  });

  document.querySelector<HTMLInputElement>("#accent-color")?.addEventListener("input", (event) => {
    const value = (event.currentTarget as HTMLInputElement).value;
    settings.accentColor = normalizeHexColor(value);
    saveSettings();
    applySettings();
    renderSettings();
  });

  document
    .querySelector<HTMLInputElement>("#accent-color-hex")
    ?.addEventListener("change", (event) => {
      const input = event.currentTarget as HTMLInputElement;
      if (isHexColor(input.value)) {
        settings.accentColor = normalizeHexColor(input.value);
        saveSettings();
        applySettings();
      }
      renderSettings();
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

  document.querySelector("#clear-lyrics-cache")?.addEventListener("click", () => {
    clearLyricsCache();
    renderSettings();
    if (tauriAvailable) {
      void emit("lyrics-cache-cleared");
    }
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
  void listen("media-state-changed", () => {
    mediaEventSequence += 1;
    void pollMedia("wmtc-event");
  });
  void listen("lyrics-cache-cleared", () => {
    clearLyricsCache();
  });
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
      void pollMedia("startup");
      pollTimer = window.setInterval(() => void pollMedia("fallback-poll"), POLLING_INTERVAL_MS);
    }, 0);
  });
}

async function pollMedia(reason = "manual") {
  if (pollInFlight) {
    pollQueued = true;
    if (pollStartedAtMs && performance.now() - pollStartedAtMs > 3000) {
      showStatus("Waiting for Windows media session...");
    }
    return;
  }

  pollInFlight = true;
  pollStartedAtMs = performance.now();
  const mediaSequenceAtRequestStart = mediaEventSequence;
  try {
    if (!tauriAvailable) {
      renderChrome();
      renderStatus();
      applyGradient();
      return;
    }

    const startedAt = performance.now();
    const nextMedia = await invoke<MediaState>("get_media_state");
    const sampledAtMs = performance.now();
    const requestDurationMs = sampledAtMs - startedAt;
    // A playback event received while this request was in flight can mean the
    // response describes the instant before a pause. Wait for the queued poll
    // instead of briefly rewinding the lyric clock with that stale sample.
    if (nextMedia.isPlaying && mediaSequenceAtRequestStart !== mediaEventSequence) {
      logSync("ignored stale playing sample", {
        reason,
        requestDurationMs: Math.round(requestDurationMs),
        requestSequence: mediaSequenceAtRequestStart,
        currentSequence: mediaEventSequence,
        positionMs: nextMedia.positionMs,
      });
      pollQueued = true;
      return;
    }
    if (shouldDeferResume(nextMedia, reason, requestDurationMs)) {
      return;
    }
    const nextTrackKey = trackKey(nextMedia);
    syncMediaClock(nextMedia, sampledAtMs, reason, requestDurationMs);
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
    if (pollQueued) {
      pollQueued = false;
      void pollMedia("queued-wmtc-event");
    }
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

  const localNotice = getLocalLyricsNotice(media.title);
  if (localNotice) {
    if (currentTrackKey === expectedTrackKey) {
      lyricsLines = [];
      lyricsMode = localNotice === "Instrumental" ? "instrumental" : "excluded";
      lyricsNotice = localNotice;
      invalidateLyricsRender();
      renderLyrics();
    }
    return;
  }

  lyricsNotice = "";
  const key = trackKey(media);
  if (lyricCache.has(key)) {
    console.info("[latency] lyrics cache hit", { key });
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
    const cacheGeneration = lyricCacheGeneration;
    const startedAt = performance.now();
    let request = lyricRequests.get(key);
    if (!request) {
      request = invoke<LyricsResult | null>("fetch_lyrics", {
        title: media.title,
        artist: media.artist,
        durationMs: media.durationMs,
      });
      lyricRequests.set(key, request);
    } else {
      console.info("[latency] joined in-flight lyrics request", { key });
    }
    const result = await request;
    if (cacheGeneration === lyricCacheGeneration) {
      lyricCache.set(key, result);
      if (result) {
        persistLyricsCache(key, result);
      }
    }
    console.info("[latency] lyrics ready", {
      key,
      durationMs: Math.round((performance.now() - startedAt) * 10) / 10,
      found: Boolean(result),
    });
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
  } finally {
    lyricRequests.delete(key);
  }
}

function applyLyrics(result: LyricsResult | null) {
  lyricsNotice = "";
  if (!result) {
    lyricsLines = [];
    lyricsMode = "missing";
    invalidateLyricsRender();
    return;
  }

  if (result.instrumental) {
    lyricsLines = [createLyricLine(null, "Instrumental")];
    lyricsMode = "instrumental";
    invalidateLyricsRender();
    return;
  }

  if (result.syncedLyrics) {
    lyricsLines = parseLyrics(result.syncedLyrics);
    lyricsMode = "synced";
    invalidateLyricsRender();
    return;
  }

  if (result.plainLyrics) {
    lyricsLines = [];
    lyricsMode = "unsynced";
    lyricsNotice = "No Synced Lyrics";
    invalidateLyricsRender();
    return;
  }

  lyricsLines = [];
  lyricsMode = "missing";
  invalidateLyricsRender();
}

function getLocalLyricsNotice(title: string): string | null {
  if (/\binstrumental\b/i.test(title)) {
    return "Instrumental";
  }

  const slowed = /\bslowed(?:\s+down)?\b/i.test(title);
  const reverb = /\breverb(?:erated)?\b/i.test(title);
  if (slowed && reverb) {
    return "Slowed + Reverb - No Lyrics";
  }

  const variants: Array<[RegExp, string]> = [
    [/\bslowed(?:\s+down)?\b/i, "Slowed"],
    [/\breverb(?:erated)?\b/i, "Reverb"],
    [/\bremix(?:ed)?\b/i, "Remix"],
    [/\bsped[\s-]*up\b|\bspeed[\s-]*up\b/i, "Sped Up"],
    [/\bnightcore\b/i, "Nightcore"],
    [/\bkaraoke\b/i, "Karaoke"],
    [/(?:[([\-–—]\s*live\b|\blive\s+(?:at|from|version|session)\b)/i, "Live"],
    [/\bcover\b/i, "Cover"],
  ];

  const match = variants.find(([pattern]) => pattern.test(title));
  return match ? `${match[1]} - No Lyrics` : null;
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
  };
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

function updateActiveLine(positionMs = getSyncedPositionMs()) {
  if (lyricsLines.length === 0) {
    activeLineIndex = -1;
    return;
  }

  const timed = lyricsLines.some((line) => line.timeMs !== null);
  if (!timed) {
    activeLineIndex = -1;
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
  const durationMs = getReliableLoopDurationMs();
  if (currentMedia.isPlaying && durationMs && position >= durationMs + LOOP_DETECTION_GRACE_MS) {
    return position % durationMs;
  }
  // WMTC duration metadata can be stale while a track is playing. Clamping an
  // advancing clock to it freezes lyric synchronization until the session is
  // refreshed (for example, by pausing and resuming).
  return durationMs && !currentMedia.isPlaying
    ? clamp(position, 0, durationMs)
    : Math.max(0, position);
}

function getReliableLoopDurationMs() {
  const durationMs = currentMedia.durationMs;
  if (!durationMs) {
    return null;
  }

  const lastTimedLineMs = lyricsLines.reduce(
    (latest, line) => Math.max(latest, line.timeMs ?? 0),
    0,
  );
  return durationMs >= lastTimedLineMs ? durationMs : null;
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

function syncMediaClock(
  media: MediaState,
  sampledAtMs: number,
  reason: string,
  requestDurationMs: number,
) {
  if (!media.hasSession) {
    if (currentMedia.hasSession) {
      logSync("media session cleared", {
        reason,
        requestDurationMs: Math.round(requestDurationMs),
      });
    }
    mediaSampledAtMs = sampledAtMs;
    mediaPositionAnchorMs = 0;
    pausedPositionAnchorMs = null;
    return;
  }

  const isSameTrack = trackKey(media) !== "" && trackKey(media) === trackKey(currentMedia);
  const wasPlaying = isSameTrack && currentMedia.isPlaying;
  const playbackChanged = currentMedia.isPlaying !== media.isPlaying;
  const livePositionMs = wasPlaying
    ? getEstimatedMediaPositionMs(sampledAtMs)
    : pausedPositionAnchorMs;

  // The backend converts WMTC Position + LastUpdatedTime + PlaybackRate into
  // a position at sampling time. A playing sample is authoritative, while a
  // paused sample is reconciled with the live frontend estimate below.
  mediaSampledAtMs = sampledAtMs;
  if (media.isPlaying || !isSameTrack) {
    if (!isSameTrack || playbackChanged) {
      logSync("media state applied", {
        reason,
        requestDurationMs: Math.round(requestDurationMs),
        track: `${media.artist} — ${media.title}`,
        previousStatus: currentMedia.status,
        status: media.status,
        positionMs: media.positionMs,
        playbackRate: media.playbackRate,
      });
    }
    pausedPositionAnchorMs = null;
    mediaPositionAnchorMs = media.positionMs;
    return;
  }

  const useLivePosition =
    livePositionMs !== null &&
    (media.status === "Paused session unavailable" ||
      Math.abs(media.positionMs - livePositionMs) <= PAUSE_POSITION_TOLERANCE_MS);

  const previousPausedAnchorMs = pausedPositionAnchorMs;
  const pausedPositionMs =
    useLivePosition && livePositionMs !== null ? livePositionMs : media.positionMs;

  if (wasPlaying || previousPausedAnchorMs !== pausedPositionMs) {
    logSync("pause position reconciled", {
      reason,
      requestDurationMs: Math.round(requestDurationMs),
      track: `${media.artist} — ${media.title}`,
      status: media.status,
      reportedPositionMs: media.positionMs,
      livePositionMs: livePositionMs === null ? null : Math.round(livePositionMs),
      differenceMs: livePositionMs === null ? null : Math.round(media.positionMs - livePositionMs),
      selectedPositionMs: Math.round(pausedPositionMs),
      usedLivePosition: useLivePosition,
      toleranceMs: PAUSE_POSITION_TOLERANCE_MS,
    });
  }

  // Freeze the live frontend estimate as soon as playback pauses. WMTC
  // position samples can arrive late or be quantized; only accept a material
  // correction, which is most likely a seek while paused.
  pausedPositionAnchorMs = pausedPositionMs;
  mediaPositionAnchorMs = pausedPositionAnchorMs;
}

function shouldDeferResume(media: MediaState, reason: string, requestDurationMs: number) {
  const resumingFromUnavailableSession =
    currentMedia.status === "Paused session unavailable" &&
    media.isPlaying &&
    trackKey(media) !== "" &&
    trackKey(media) === trackKey(currentMedia);

  if (!resumingFromUnavailableSession) {
    pendingResumeConfirmation = null;
    window.clearTimeout(resumeConfirmationTimer);
    return false;
  }

  const previousCandidate = pendingResumeConfirmation?.positionMs;
  if (
    previousCandidate !== undefined &&
    media.positionMs >= previousCandidate + RESUME_CONFIRMATION_PROGRESS_MS
  ) {
    logSync("resume confirmed", {
      reason,
      requestDurationMs: Math.round(requestDurationMs),
      firstPositionMs: previousCandidate,
      confirmedPositionMs: media.positionMs,
      progressMs: media.positionMs - previousCandidate,
    });
    pendingResumeConfirmation = null;
    window.clearTimeout(resumeConfirmationTimer);
    return false;
  }

  pendingResumeConfirmation = { positionMs: media.positionMs };
  logSync("deferred unconfirmed resume", {
    reason,
    requestDurationMs: Math.round(requestDurationMs),
    candidatePositionMs: media.positionMs,
    pausedPositionMs: pausedPositionAnchorMs,
    requiredProgressMs: RESUME_CONFIRMATION_PROGRESS_MS,
  });
  window.clearTimeout(resumeConfirmationTimer);
  resumeConfirmationTimer = window.setTimeout(() => {
    void pollMedia("resume-confirmation");
  }, RESUME_CONFIRMATION_DELAY_MS);
  return true;
}

function renderAll() {
  renderChrome();
  renderLyrics();
  renderSettings();
  renderStatus();
  applyGradient();
}

function renderChrome() {
  const nextChromeKey = [currentMedia.hasSession, currentMedia.title, currentMedia.artist].join(
    "|",
  );
  if (nextChromeKey === renderedChromeKey) {
    return;
  }

  renderedChromeKey = nextChromeKey;
  const title = document.querySelector("#title")!;
  const artist = document.querySelector("#artist")!;

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

  if (lyricsMode === "instrumental" || lyricsMode === "excluded" || lyricsMode === "unsynced") {
    list.innerHTML = `<p class="empty-state">${escapeHtml(lyricsNotice || "Instrumental")}</p>`;
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
  if (activeLineIndex !== previousActiveLineIndex) {
    const previousLine = lyricsLines[previousActiveLineIndex];
    const activeLine = lyricsLines[activeLineIndex];
    const activeTimestampMs = activeLine?.timeMs ?? null;
    logSync("lyric line changed", {
      previousIndex: previousActiveLineIndex,
      previousTimestampMs: previousLine?.timeMs ?? null,
      nextIndex: activeLineIndex,
      nextTimestampMs: activeTimestampMs,
      positionMs: Math.round(positionMs),
      timestampDeltaMs:
        activeTimestampMs === null ? null : Math.round(positionMs - activeTimestampMs),
      isPlaying: currentMedia.isPlaying,
      mediaStatus: currentMedia.status,
      playbackRate: currentMedia.playbackRate,
      line: activeLine?.text ?? null,
    });
  }
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
    const distance = Math.abs(lineIndex - activeLineIndex);
    const showNeighborFocus = lyricsMode !== "synced";

    lineElement.classList.toggle("active", lineIndex === activeLineIndex);
    lineElement.classList.toggle("near", showNeighborFocus && distance === 1);
    lineElement.classList.toggle("far", distance > 4);
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

  if (
    lyricsMode === "searching" ||
    lyricsMode === "missing" ||
    lyricsMode === "error" ||
    lyricsMode === "unsynced" ||
    lyricsMode === "instrumental" ||
    lyricsMode === "excluded"
  ) {
    return `${lyricsMode}:${currentTrackKey}`;
  }

  if (lyricsLines.length === 0) {
    return `missing:${currentTrackKey}`;
  }

  return `${lyricsMode}:${lyricsLines
    .map((line) => `${line.timeMs ?? "x"}:${line.text}:${line.words.length}`)
    .join("|")}`;
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
  document.querySelector<HTMLInputElement>("#blur-intensity")!.value = String(
    settings.blurIntensity,
  );
  document.querySelector<HTMLInputElement>("#font-size")!.value = String(settings.fontSize);
  document.querySelector<HTMLInputElement>("#line-spacing")!.value = String(settings.lineSpacing);
  document.querySelector<HTMLInputElement>("#start-login")!.checked = settings.startAtLogin;
  document.querySelector<HTMLSelectElement>("#accent-mode")!.value = settings.accentMode;
  document.querySelector<HTMLInputElement>("#accent-color")!.value = settings.accentColor;
  const accentHex = document.querySelector<HTMLInputElement>("#accent-color-hex")!;
  accentHex.value = settings.accentColor;
  const manualAccent = settings.accentMode === "manual";
  document.querySelector<HTMLInputElement>("#accent-color")!.disabled = !manualAccent;
  accentHex.disabled = !manualAccent;
  document
    .querySelector<HTMLElement>("#manual-accent-controls")
    ?.classList.toggle("disabled", !manualAccent);
  renderSettingValues();
}

function renderSettingValues() {
  document.querySelector<HTMLOutputElement>("#opacity-value")!.value =
    `${Math.round(settings.opacity * 100)}%`;
  document.querySelector<HTMLOutputElement>("#blur-intensity-value")!.value =
    `${settings.blurIntensity}%`;
  document.querySelector<HTMLOutputElement>("#font-size-value")!.value = `${settings.fontSize}rem`;
  document.querySelector<HTMLOutputElement>("#line-spacing-value")!.value =
    `${settings.lineSpacing}em`;
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
  root.style.setProperty("--backdrop-blur", `${settings.blurIntensity * 0.2}px`);
  root.style.setProperty("--lyric-size", `${settings.fontSize}rem`);
  root.style.setProperty("--line-spacing", `${settings.lineSpacing}em`);
  applyGradient();
  renderSettingValues();
  overlay?.classList.toggle("click-through", settings.clickThrough);
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

  await safeInvoke("set_overlay_blur", { intensity: settings.blurIntensity });
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

function logSync(event: string, details: Record<string, unknown>) {
  const entry = { timestampMs: Math.round(performance.now()), ...details };
  if (!tauriAvailable) {
    console.info(`[sync] ${event}`, entry);
    return;
  }

  void invoke("log_sync_diagnostic", { event, details: JSON.stringify(entry) }).catch(() => {});
}

function applyGradient() {
  const nextGradientKey =
    settings.accentMode === "manual"
      ? `manual:${settings.accentColor}`
      : `${currentMedia.artist}:${currentMedia.title}`;
  if (nextGradientKey === renderedGradientKey) {
    return;
  }
  renderedGradientKey = nextGradientKey;

  if (settings.accentMode === "manual") {
    document.documentElement.style.setProperty("--accent", settings.accentColor);
    return;
  }

  const hue = hashHue(nextGradientKey);
  document.documentElement.style.setProperty("--hue", String(hue));
  document.documentElement.style.setProperty("--accent", `hsl(${hue}, var(--bg-saturation), 56%)`);
}

function loadSettings(): SettingsState {
  try {
    const stored = localStorage.getItem(SETTINGS_STORAGE_KEY);
    const loaded = stored ? JSON.parse(stored) : {};
    const fontSize = loadFontSizeSetting(loaded.fontSize);
    return {
      clickThrough: Boolean(loaded.clickThrough),
      opacity: loadNumericSetting(loaded.opacity, DEFAULT_SETTINGS.opacity, 0, 1),
      blurIntensity: loadNumericSetting(
        loaded.blurIntensity,
        DEFAULT_SETTINGS.blurIntensity,
        1,
        100,
      ),
      fontSize,
      lineSpacing: loadLineSpacingSetting(loaded.lineSpacing, fontSize),
      startAtLogin:
        typeof loaded.startAtLogin === "boolean"
          ? loaded.startAtLogin
          : DEFAULT_SETTINGS.startAtLogin,
      accentMode: loaded.accentMode === "manual" ? "manual" : "dynamic",
      accentColor: isHexColor(loaded.accentColor)
        ? normalizeHexColor(loaded.accentColor)
        : DEFAULT_SETTINGS.accentColor,
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

function loadLyricsCache(): Map<string, LyricsResult | null> {
  try {
    const stored = localStorage.getItem(LYRICS_CACHE_STORAGE_KEY);
    const entries = stored ? (JSON.parse(stored) as [string, CachedLyrics][]) : [];
    return new Map(
      entries
        .filter(
          (entry): entry is [string, CachedLyrics] =>
            Array.isArray(entry) &&
            typeof entry[0] === "string" &&
            typeof entry[1]?.cachedAt === "number" &&
            typeof entry[1]?.result === "object" &&
            entry[1].result !== null,
        )
        .slice(-MAX_PERSISTED_LYRICS)
        .map(([key, cached]) => [key, cached.result]),
    );
  } catch {
    localStorage.removeItem(LYRICS_CACHE_STORAGE_KEY);
    return new Map();
  }
}

function persistLyricsCache(key: string, result: LyricsResult) {
  try {
    const stored = localStorage.getItem(LYRICS_CACHE_STORAGE_KEY);
    const entries = stored ? (JSON.parse(stored) as [string, CachedLyrics][]) : [];
    const nextEntries = entries.filter(([storedKey]) => storedKey !== key);
    nextEntries.push([key, { cachedAt: Date.now(), result }]);
    localStorage.setItem(
      LYRICS_CACHE_STORAGE_KEY,
      JSON.stringify(nextEntries.slice(-MAX_PERSISTED_LYRICS)),
    );
  } catch (error) {
    console.warn("Unable to persist the lyrics cache", error);
  }
}

function clearLyricsCache() {
  lyricCacheGeneration += 1;
  lyricCache.clear();
  localStorage.removeItem(LYRICS_CACHE_STORAGE_KEY);
  console.info("[latency] lyrics cache cleared");
}

function loadNumericSetting(value: unknown, fallback: number, minimum: number, maximum: number) {
  return typeof value === "number" && Number.isFinite(value)
    ? clamp(value, minimum, maximum)
    : fallback;
}

function loadFontSizeSetting(value: unknown) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return DEFAULT_SETTINGS.fontSize;
  }

  // Migrate values saved by versions that stored the lyric size in pixels.
  const remValue = value > 3 ? value / 16 : value;
  return clamp(remValue, 0.5, 3);
}

function loadLineSpacingSetting(value: unknown, fontSize: number) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return DEFAULT_SETTINGS.lineSpacing;
  }

  // Migrate values saved by versions that stored line spacing in pixels.
  const emValue = value > 1.2 ? value / (fontSize * 16) : value;
  return clamp(Math.round(emValue * 20) / 20, 0.1, 1.2);
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

function isHexColor(value: unknown): value is string {
  return typeof value === "string" && /^#?[\da-f]{6}$/i.test(value);
}

function normalizeHexColor(value: string) {
  return `#${value.replace(/^#/, "").toUpperCase()}`;
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
