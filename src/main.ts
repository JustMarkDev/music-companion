import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { PhysicalPosition, PhysicalSize } from "@tauri-apps/api/dpi";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow, type ResizeDirection } from "@tauri-apps/api/window";
import {
  CircleCheck,
  createIcons,
  Maximize2,
  Menu,
  Minus,
  RotateCcw,
  Settings,
  TriangleAlert,
  X,
} from "lucide";
import "./styles.css";
import { formatAccelerator, keyboardEventToAccelerator } from "./hotkeys";
import { LyricsCache } from "./lyrics-cache";
import {
  getLocalLyricsNotice,
  normalizeDisplayMetadata,
  normalizeLyricsMetadata,
  parseLyrics,
  playbackVariant,
  selectLyricsDisplay,
  startsNewPlaybackVariant,
  variantToken,
  type LyricLine,
  type LyricsMode,
  type LyricsResult,
  type PlaybackVariant,
} from "./lyrics";
import { PlaybackClock, PAUSE_POSITION_TOLERANCE_MS } from "./playback-clock";
import {
  decodeSettings,
  DEFAULT_HOTKEYS,
  isHexColor,
  normalizeHexColor,
  type HotkeyAction,
  type SettingsState,
} from "./settings";

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

type HotkeyStatus = {
  action: string;
  accelerator: string;
  registered: boolean;
  error: string | null;
  conflictAction: string | null;
};

let hotkeyStatuses: HotkeyStatus[] = [];

const HOTKEY_ACTION_LABELS: Record<HotkeyAction, string> = {
  pinned: "Pinned mode",
  next: "Next song",
  previous: "Previous song",
  playPause: "Pause song",
};

const SETTINGS_STORAGE_KEY = "music-companion-settings";
const MAIN_WINDOW_GEOMETRY_STORAGE_KEY = "music-companion-main-window-geometry-v2";
const POLLING_INTERVAL_MS = 2_000;
const SYNC_OFFSET_MS = 0;
const RESUME_CONFIRMATION_DELAY_MS = 250;
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
const lyricCache = new LyricsCache(localStorage);
// Rust survives frontend reloads during development, so keep request IDs newer
// than any IDs issued by the previous WebView document.
let lyricsRequestId = Date.now();

let settings = loadSettings();
let currentMedia: MediaState = demoState;
let currentPlaybackVariant: PlaybackVariant | null = playbackVariant(demoState);
let currentTrackKey = currentPlaybackVariant ? variantToken(currentPlaybackVariant) : "";
let lyricsLines: LyricLine[] = tauriAvailable ? [] : parseLyrics(demoLyrics);
let currentLyricsResult: LyricsResult | null = null;
let activeLineIndex = 2;
let lyricsMode: LyricsMode = tauriAvailable ? "searching" : "synced";
let lyricsNotice = "";
let settingsOpen = false;
let pollTimer = 0;
let animationFrame = 0;
const playbackClock = new PlaybackClock(performance.now(), demoState.positionMs);
const demoStartedAtMs = performance.now();
let renderedLyricsKey = "";
let lastScrolledLineIndex = -1;
let pollInFlight = false;
let pollQueued = false;
let pollStartedAtMs = 0;
let mediaEventSequence = 0;
let resumeConfirmationTimer = 0;
let renderedChromeKey = "";
let renderedGradientKey = "";
let mainWindowGeometry: { width: number; height: number; x: number; y: number } | null = null;
let geometrySaveTimer = 0;

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

      <section class="lyrics-viewport" id="lyrics-viewport" aria-live="polite">
        <div class="lyrics-list" id="lyrics-list"></div>
      </section>

      <aside class="settings-panel" id="settings-panel" hidden>
        <div class="settings-header" id="settings-header">
          <div class="settings-heading">
            <h2>Settings</h2>
            <p>Make the overlay feel at home on your desktop.</p>
          </div>
          <button class="icon-button" id="settings-close" title="Close settings" aria-label="Close settings">
            <i data-lucide="x"></i>
          </button>
        </div>

        <section class="settings-section" aria-labelledby="appearance-title">
          <div class="section-heading">
            <h3 id="appearance-title">Appearance</h3>
            <p>Fine-tune the overlay surface and typography.</p>
          </div>
          <div class="settings-card range-group">
            <div class="range-control">
              <label class="range-label" for="opacity">
                <span><strong>Opacity</strong><small>Overlay transparency</small></span>
                <output id="opacity-value">0%</output>
              </label>
              <input id="opacity" type="range" min="0" max="100" step="1" />
            </div>
            <div class="range-control">
              <label class="range-label" for="blur-intensity">
                <span><strong>Blur intensity</strong><small>Background diffusion</small></span>
                <output id="blur-intensity-value">100%</output>
              </label>
              <input id="blur-intensity" type="range" min="1" max="100" step="1" />
            </div>
            <div class="range-control">
              <label class="range-label" for="font-size">
                <span><strong>Lyric size</strong><small>Active and surrounding lines</small></span>
                <output id="font-size-value">1rem</output>
              </label>
              <input id="font-size" type="range" min="0.5" max="3" step="0.05" />
            </div>
            <div class="range-control">
              <label class="range-label" for="line-spacing">
                <span><strong>Line spacing</strong><small>Breathing room between lyrics</small></span>
                <output id="line-spacing-value">0.5em</output>
              </label>
              <input id="line-spacing" type="range" min="0.1" max="1.2" step="0.05" />
            </div>
          </div>
        </section>

        <section class="settings-section" aria-labelledby="material-title">
          <div class="section-heading">
            <h3 id="material-title">Window material</h3>
            <p>Choose the backdrop that best suits your system.</p>
          </div>
          <div class="settings-card material-setting">
            <div class="segmented-control" id="backdrop-material" role="radiogroup" aria-label="Window material">
              <button type="button" data-backdrop-material="acrylic" role="radio">Acrylic</button>
              <button type="button" data-backdrop-material="mica" role="radio">Mica</button>
            </div>
            <p id="material-description">Live frosted-glass blur of windows behind the overlay.</p>
          </div>
        </section>

        <section class="settings-section" aria-labelledby="accent-title">
          <div class="section-heading">
            <h3 id="accent-title">Accent color</h3>
            <p>Follow the current track or choose your own color.</p>
          </div>
          <div class="settings-card accent-color-setting">
            <div class="segmented-control" id="accent-mode" role="radiogroup" aria-label="Accent color mode">
              <button type="button" data-accent-mode="dynamic" role="radio">Dynamic</button>
              <button type="button" data-accent-mode="manual" role="radio">Manual</button>
            </div>
            <div class="manual-accent-controls" id="manual-accent-controls">
              <label class="color-swatch" title="Choose accent color">
                <input id="accent-color" type="color" aria-label="Choose accent color" />
                <span aria-hidden="true"></span>
              </label>
              <input id="accent-color-hex" type="text" inputmode="text" maxlength="7" spellcheck="false" aria-label="Accent color hex value" />
            </div>
          </div>
        </section>

        <section class="settings-section" aria-labelledby="behavior-title">
          <div class="section-heading">
            <h3 id="behavior-title">Behavior</h3>
            <p>Choose how Music Companion starts and displays lyrics.</p>
          </div>
          <div class="settings-card switch-group">
            <label class="switch-row" for="start-login">
              <span><strong>Start at login</strong><small>Launch automatically with Windows</small></span>
              <input id="start-login" type="checkbox" role="switch" />
              <span class="switch-control" aria-hidden="true"></span>
            </label>
            <div class="lyrics-mode-setting">
              <span><strong>Lyrics script</strong><small>Choose the preferred lyric writing system</small></span>
              <div class="segmented-control" id="lyrics-script" role="radiogroup" aria-label="Lyrics script">
                <button type="button" data-lyrics-script="original" role="radio">Original</button>
                <button type="button" data-lyrics-script="romanized" role="radio">Romanized</button>
              </div>
            </div>
          </div>
        </section>

        <section class="settings-section" aria-labelledby="system-title">
          <div class="section-heading">
            <h3 id="system-title">System</h3>
          </div>
          <div class="settings-card system-group">
            <div class="hotkey-setting">
              <span><strong>Pinned mode</strong><small>Toggle click-through mode</small></span>
              <span class="hotkey-value" data-hotkey-action="pinned"><button class="hotkey-reset" type="button" title="Restore default" aria-label="Restore default pinned mode hotkey"><i data-lucide="rotate-ccw"></i></button><button class="hotkey-input" type="button">Ctrl + Shift + L</button></span>
            </div>
            <div class="hotkey-setting">
              <span><strong>Next song</strong><small>Skip to the next track</small></span>
              <span class="hotkey-value" data-hotkey-action="next"><button class="hotkey-reset" type="button" title="Restore default" aria-label="Restore default next song hotkey"><i data-lucide="rotate-ccw"></i></button><button class="hotkey-input" type="button">Ctrl + Right Arrow</button></span>
            </div>
            <div class="hotkey-setting">
              <span><strong>Previous song</strong><small>Return to the previous track</small></span>
              <span class="hotkey-value" data-hotkey-action="previous"><button class="hotkey-reset" type="button" title="Restore default" aria-label="Restore default previous song hotkey"><i data-lucide="rotate-ccw"></i></button><button class="hotkey-input" type="button">Ctrl + Left Arrow</button></span>
            </div>
            <div class="hotkey-setting">
              <span><strong>Pause song</strong><small>Toggle play or pause</small></span>
              <span class="hotkey-value" data-hotkey-action="playPause"><button class="hotkey-reset" type="button" title="Restore default" aria-label="Restore default play/pause hotkey"><i data-lucide="rotate-ccw"></i></button><button class="hotkey-input" type="button">Ctrl + Shift + Space</button></span>
            </div>
            <div class="cache-setting">
              <span><strong>Lyrics cache</strong><small>Remove saved lyrics from this device</small></span>
              <button class="clear-cache-button" id="clear-lyrics-cache">Clear</button>
            </div>
          </div>
        </section>

        <p class="app-version" id="app-version">Music Companion</p>
      </aside>
      <div class="resize-handles" aria-hidden="true">
        <span data-resize-direction="North"></span>
        <span data-resize-direction="East"></span>
        <span data-resize-direction="South"></span>
        <span data-resize-direction="West"></span>
        <span data-resize-direction="NorthEast"></span>
        <span data-resize-direction="SouthEast"></span>
        <span data-resize-direction="SouthWest"></span>
        <span data-resize-direction="NorthWest"></span>
      </div>
    </section>
  </main>
  <div class="toast-region" id="toast-region" aria-live="polite" aria-atomic="false"></div>
`;

createIcons({
  icons: { CircleCheck, Maximize2, Menu, Minus, RotateCcw, Settings, TriangleAlert, X },
});
void renderAppVersion();

if (isSettingsWindow) {
  document.body.classList.add("settings-window");
  settingsOpen = true;
  wireUi();
  wireWindowEvents();
  applySettings();
  renderSettings();
  void syncSettingsAccent();
  void loadHotkeyStatuses();
} else {
  void initializeMainWindowGeometry();
  wireUi();
  wireWindowEvents();
  applySettings();
  renderAll();
  void syncStartAtLogin();
  schedulePolling();
  startSyncLoop();
  void applySavedHotkeys();
}

async function renderAppVersion() {
  if (!tauriAvailable) return;

  try {
    const version = await getVersion();
    document.querySelector<HTMLElement>("#app-version")!.textContent =
      `Music Companion · v${version}`;
  } catch (error) {
    console.warn("Unable to read the application version", error);
  }
}

async function initializeMainWindowGeometry() {
  if (!appWindow || isSettingsWindow) return;

  try {
    const stored = localStorage.getItem(MAIN_WINDOW_GEOMETRY_STORAGE_KEY);
    const geometry = stored ? (JSON.parse(stored) as typeof mainWindowGeometry) : null;
    if (isValidMainWindowGeometry(geometry)) {
      mainWindowGeometry = geometry;
      await appWindow.setSize(new PhysicalSize(geometry.width, geometry.height));
      await appWindow.setPosition(new PhysicalPosition(geometry.x, geometry.y));
    }
  } catch {
    localStorage.removeItem(MAIN_WINDOW_GEOMETRY_STORAGE_KEY);
  }

  if (!mainWindowGeometry) {
    const [size, position] = await Promise.all([appWindow.innerSize(), appWindow.outerPosition()]);
    const geometry = { width: size.width, height: size.height, x: position.x, y: position.y };
    if (isValidMainWindowGeometry(geometry) && (geometry.width > 220 || geometry.height > 110)) {
      mainWindowGeometry = geometry;
      localStorage.setItem(MAIN_WINDOW_GEOMETRY_STORAGE_KEY, JSON.stringify(geometry));
    }
  }

  await appWindow.onResized(({ payload }) => {
    if (payload.width < 220 || payload.height < 110) return;
    if (!mainWindowGeometry && payload.width === 220 && payload.height === 110) return;
    const position = mainWindowGeometry ?? {
      width: payload.width,
      height: payload.height,
      x: 0,
      y: 0,
    };
    queueMainWindowGeometrySave({ ...position, width: payload.width, height: payload.height });
  });
  await appWindow.onMoved(({ payload }) => {
    if (payload.x <= -10_000 || payload.y <= -10_000) return;
    const size = mainWindowGeometry ?? { width: 520, height: 720, x: payload.x, y: payload.y };
    queueMainWindowGeometrySave({ ...size, x: payload.x, y: payload.y });
  });
}

function isValidMainWindowGeometry(
  geometry: typeof mainWindowGeometry,
): geometry is NonNullable<typeof mainWindowGeometry> {
  return Boolean(
    geometry &&
    Number.isFinite(geometry.width) &&
    Number.isFinite(geometry.height) &&
    Number.isFinite(geometry.x) &&
    Number.isFinite(geometry.y) &&
    geometry.width >= 220 &&
    geometry.height >= 110 &&
    geometry.x > -10_000 &&
    geometry.y > -10_000,
  );
}

function queueMainWindowGeometrySave(geometry: NonNullable<typeof mainWindowGeometry>) {
  mainWindowGeometry = geometry;
  window.clearTimeout(geometrySaveTimer);
  geometrySaveTimer = window.setTimeout(() => {
    localStorage.setItem(MAIN_WINDOW_GEOMETRY_STORAGE_KEY, JSON.stringify(geometry));
  }, 180);
}

function wireUi() {
  document.querySelectorAll<HTMLElement>("[data-resize-direction]").forEach((handle) => {
    handle.addEventListener("pointerdown", (event) => {
      if (!appWindow || event.button !== 0) return;
      event.preventDefault();
      event.stopPropagation();
      const direction = handle.dataset.resizeDirection as ResizeDirection;
      void safeWindowAction(() => appWindow.startResizeDragging(direction));
    });
  });

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
      } catch {
        if (compactMenu) compactMenu.hidden = true;
        compactMenuToggle?.setAttribute("aria-expanded", "false");
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
      .catch(() => undefined);
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

  document.querySelector("#accent-mode")?.addEventListener("click", (event) => {
    const mode = (event.target as Element).closest<HTMLButtonElement>("[data-accent-mode]")?.dataset
      .accentMode;
    if (!mode) return;
    settings.accentMode = mode === "manual" ? "manual" : "dynamic";
    saveSettings();
    applySettings();
    renderSettings();
    void syncSettingsAccent();
  });

  document.querySelector("#backdrop-material")?.addEventListener("click", (event) => {
    const material = (event.target as Element).closest<HTMLButtonElement>(
      "[data-backdrop-material]",
    )?.dataset.backdropMaterial;
    if (!material) return;
    settings.backdropMaterial = material === "acrylic" ? "acrylic" : "mica";
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

  document.querySelector("#lyrics-script")?.addEventListener("click", (event) => {
    const script = (event.target as Element).closest<HTMLButtonElement>("[data-lyrics-script]")
      ?.dataset.lyricsScript;
    if (!script) return;
    settings.romanizedLyrics = script === "romanized";
    saveSettings();
    applyLyrics(currentLyricsResult);
    renderLyrics();
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
    showToast("Cache pulita", "I testi salvati sono stati rimossi.");
    if (tauriAvailable) {
      void emit("lyrics-cache-cleared");
    }
  });
  wireHotkeyInputs();
}

function showToast(title: string, description?: string, variant: "success" | "error" = "success") {
  const region = document.querySelector<HTMLDivElement>("#toast-region");
  if (!region) return;

  const toast = document.createElement("div");
  toast.className = `toast toast-${variant} toast-visible`;
  toast.setAttribute("role", "status");
  toast.innerHTML = `
    <i class="toast-icon" data-lucide="${variant === "error" ? "triangle-alert" : "circle-check"}" aria-hidden="true"></i>
    <div class="toast-content">
      <p class="toast-title"></p>
      ${description ? '<p class="toast-description"></p>' : ""}
    </div>`;
  toast.querySelector<HTMLElement>(".toast-title")!.textContent = title;
  const descriptionElement = toast.querySelector<HTMLElement>(".toast-description");
  if (descriptionElement) descriptionElement.textContent = description ?? "";
  region.append(toast);
  updateToastStack(region);
  createIcons({ icons: { CircleCheck, TriangleAlert }, attrs: { "aria-hidden": "true" } });
  window.setTimeout(() => {
    toast.classList.add("toast-exiting");
    updateToastStack(region);
    const removeToast = () => {
      toast.remove();
      updateToastStack(region);
    };
    toast.addEventListener("transitionend", removeToast, { once: true });
    window.setTimeout(removeToast, 250);
  }, 3_000);
}

function updateToastStack(region: HTMLElement) {
  const toasts = [...region.querySelectorAll<HTMLElement>(".toast:not(.toast-exiting)")].reverse();
  toasts.forEach((toast, index) => {
    toast.style.setProperty("--toast-offset", `${index * -7}px`);
    toast.style.setProperty("--toast-scale", String(Math.max(0.9, 1 - index * 0.035)));
    toast.style.setProperty("--toast-opacity", index < 3 ? "1" : "0");
    toast.style.zIndex = String(toasts.length - index);
  });
}

function wireHotkeyInputs() {
  document.querySelectorAll<HTMLElement>("[data-hotkey-action]").forEach((row) => {
    const action = row.dataset.hotkeyAction as HotkeyAction;
    const input = row.querySelector<HTMLButtonElement>(".hotkey-input")!;
    let pending: string | null = null;
    input.addEventListener("focus", () => {
      pending = null;
      input.classList.add("recording");
      input.textContent = "Press shortcut…";
      void safeInvoke("set_hotkey_recording", { recording: true });
    });
    input.addEventListener("keydown", (event) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape") {
        pending = null;
        input.blur();
        return;
      }
      if (["Control", "Shift", "Alt", "Meta"].includes(event.key)) return;
      pending = keyboardEventToAccelerator(event);
      input.textContent = formatAccelerator(pending);
    });
    input.addEventListener("keyup", async (event) => {
      event.preventDefault();
      const accelerator = pending;
      pending = null;
      if (accelerator) await setHotkey(action, accelerator);
      input.blur();
    });
    input.addEventListener("blur", async () => {
      const accelerator = pending;
      pending = null;
      if (accelerator) await setHotkey(action, accelerator);
      input.classList.remove("recording");
      await safeInvoke("set_hotkey_recording", { recording: false });
      renderHotkeyStatuses();
    });
    row.querySelector<HTMLButtonElement>(".hotkey-reset")?.addEventListener("click", (event) => {
      event.stopPropagation();
      void setHotkey(action, DEFAULT_HOTKEYS[action]);
    });
  });
}

async function setHotkey(action: HotkeyAction, accelerator: string, retryFailed = true) {
  if (!tauriAvailable) {
    settings.hotkeys[action] = accelerator;
    saveSettings();
    renderHotkeyStatuses();
    return;
  }

  try {
    const status = await invoke<HotkeyStatus>("register_hotkey", { action, accelerator });
    if (status.registered) {
      settings.hotkeys[action] = accelerator;
      saveSettings();
      hotkeyStatuses = [...hotkeyStatuses.filter((item) => item.action !== action), status];
    } else if (retryFailed) {
      const conflictingAction =
        status.conflictAction ??
        hotkeyStatuses.find(
          (item) =>
            item.action !== action && item.registered && item.accelerator === status.accelerator,
        )?.action;
      if (conflictingAction && conflictingAction in HOTKEY_ACTION_LABELS) {
        showToast(
          `Hotkey già registrata su ${HOTKEY_ACTION_LABELS[conflictingAction as HotkeyAction]}`,
          undefined,
          "error",
        );
      } else {
        showToast(
          "Hotkey non disponibile",
          "La combinazione potrebbe essere utilizzata da un'altra applicazione.",
          "error",
        );
      }
    }
  } catch (error) {
    console.error("Unable to register global hotkey", error);
    if (retryFailed) {
      showToast(
        "Hotkey non disponibile",
        "Non è stato possibile aggiornare la scorciatoia.",
        "error",
      );
    }
  }
  renderHotkeyStatuses();
  if (tauriAvailable && retryFailed) {
    await safeInvoke("retry_failed_hotkeys");
  }
}

async function applySavedHotkeys() {
  if (!tauriAvailable || isSettingsWindow) return;
  for (const action of Object.keys(DEFAULT_HOTKEYS) as HotkeyAction[]) {
    await setHotkey(action, settings.hotkeys[action], false);
  }
  await safeInvoke("retry_failed_hotkeys");
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
      void syncSettingsAccent();
      void loadHotkeyStatuses();
    });
    void listen("media-state-changed", () => void syncSettingsAccent());
    void listen("hotkey-statuses-changed", () => void loadHotkeyStatuses());
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
    applyLyrics(currentLyricsResult);
    renderLyrics();
  });
}

async function loadHotkeyStatuses() {
  if (!tauriAvailable) return;
  try {
    hotkeyStatuses = await invoke<HotkeyStatus[]>("get_hotkey_statuses");
    renderHotkeyStatuses();
  } catch (error) {
    console.error("Unable to load global hotkey statuses", error);
  }
}

function renderHotkeyStatuses() {
  document.querySelectorAll<HTMLElement>("[data-hotkey-action]").forEach((element) => {
    const status = hotkeyStatuses.find((item) => item.action === element.dataset.hotkeyAction);
    const action = element.dataset.hotkeyAction as HotkeyAction;
    const accelerator = settings.hotkeys[action];
    const input = element.querySelector<HTMLButtonElement>(".hotkey-input");
    if (input && !input.classList.contains("recording"))
      input.textContent = formatAccelerator(accelerator);
    element.querySelector<HTMLButtonElement>(".hotkey-reset")!.hidden =
      accelerator === DEFAULT_HOTKEYS[action];
    element.querySelector(".hotkey-warning")?.remove();
    if (!status || status.registered) return;
    const warning = document.createElement("span");
    warning.className = "hotkey-warning";
    warning.title = `This shortcut could not be registered: ${status.error ?? "already in use"}`;
    warning.setAttribute("aria-label", warning.title);
    warning.innerHTML = '<i data-lucide="triangle-alert" aria-hidden="true"></i>';
    element.prepend(warning);
    createIcons({ icons: { TriangleAlert } });
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
  void safeInvoke("set_hotkey_recording", { recording: false });
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
    }
    return;
  }

  pollInFlight = true;
  pollStartedAtMs = performance.now();
  const mediaSequenceAtRequestStart = mediaEventSequence;
  try {
    if (!tauriAvailable) {
      renderChrome();
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
    const nextVariant = playbackVariant(nextMedia);
    const startsNewVariant = startsNewPlaybackVariant(currentPlaybackVariant, nextVariant);
    const sameVariant = Boolean(nextVariant && currentPlaybackVariant && !startsNewVariant);
    if (shouldDeferResume(nextMedia, sameVariant, reason, requestDurationMs)) {
      return;
    }
    syncMediaClock(nextMedia, sameVariant, sampledAtMs, reason, requestDurationMs);
    currentMedia = nextMedia;

    if (nextVariant && startsNewVariant) {
      currentPlaybackVariant = nextVariant;
      currentTrackKey = variantToken(nextVariant);
      lyricsRequestId += 1;
      void safeInvoke("cancel_lyrics_requests", { requestId: lyricsRequestId });
      void loadLyrics(currentMedia, nextVariant);
    }

    if (!nextVariant) {
      currentPlaybackVariant = null;
      currentTrackKey = "";
      lyricsLines = [];
      lyricsMode = "missing";
      invalidateLyricsRender();
    }

    renderChrome();
    renderLyrics();
    applyGradient();
  } catch {
  } finally {
    pollInFlight = false;
    pollStartedAtMs = 0;
    if (pollQueued) {
      pollQueued = false;
      void pollMedia("queued-wmtc-event");
    }
  }
}

async function loadLyrics(media: MediaState, expectedVariant: PlaybackVariant) {
  const expectedTrackKey = variantToken(expectedVariant);
  if (!media.hasSession || !media.title) {
    if (currentTrackKey === expectedTrackKey) {
      lyricsLines = [];
      lyricsMode = "missing";
    }
    return;
  }

  const localNotice = getLocalLyricsNotice(media.title);
  if (localNotice === "Instrumental") {
    if (currentTrackKey === expectedTrackKey) {
      lyricsLines = [];
      lyricsMode = "instrumental";
      lyricsNotice = localNotice;
      invalidateLyricsRender();
      renderLyrics();
    }
    return;
  }

  lyricsNotice = "";
  const lyricsMetadata = normalizeLyricsMetadata(media);
  if (lyricCache.has(expectedVariant)) {
    console.info("[latency] lyrics cache hit", { key: expectedTrackKey });
    if (currentTrackKey === expectedTrackKey) {
      applyLyrics(lyricCache.get(expectedVariant) ?? null, localNotice);
    }
    return;
  }

  if (currentTrackKey === expectedTrackKey) {
    lyricsLines = [];
    lyricsMode = "searching";
    invalidateLyricsRender();
    renderLyrics();
  }

  try {
    const cacheGeneration = lyricCache.requestGeneration();
    const startedAt = performance.now();
    const requestId = lyricsRequestId;
    const result = await invoke<LyricsResult | null>("fetch_lyrics", {
      title: lyricsMetadata.title,
      artist: lyricsMetadata.artist,
      durationMs: media.durationMs,
      requestId,
    });
    const requestIsCurrent = requestId === lyricsRequestId && currentTrackKey === expectedTrackKey;
    if (requestIsCurrent) {
      lyricCache.putIfCurrent(cacheGeneration, expectedVariant, result);
    }
    console.info("[latency] lyrics ready", {
      key: expectedTrackKey,
      durationMs: Math.round((performance.now() - startedAt) * 10) / 10,
      found: Boolean(result),
    });
    if (requestIsCurrent) {
      applyLyrics(result, localNotice);
    }
  } catch {
    if (currentTrackKey === expectedTrackKey) {
      lyricsLines = [];
      lyricsMode = "error";
      invalidateLyricsRender();
      renderLyrics();
    }
  }
}

function applyLyrics(result: LyricsResult | null, fallbackNotice: string | null = null) {
  currentLyricsResult = result;
  const display = selectLyricsDisplay(
    result,
    currentMedia.title,
    settings.romanizedLyrics,
    fallbackNotice,
  );
  lyricsLines = display.lines;
  lyricsMode = display.mode;
  lyricsNotice = display.notice;
  invalidateLyricsRender();
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

  const lastTimedLineMs = lyricsLines.reduce(
    (latest, line) => Math.max(latest, line.timeMs ?? 0),
    0,
  );
  return (
    playbackClock.syncedPosition(currentMedia, performance.now(), lastTimedLineMs) + SYNC_OFFSET_MS
  );
}

function syncMediaClock(
  media: MediaState,
  sameVariant: boolean,
  sampledAtMs: number,
  reason: string,
  requestDurationMs: number,
) {
  if (!media.hasSession && currentMedia.hasSession) {
    logSync("media session cleared", {
      reason,
      requestDurationMs: Math.round(requestDurationMs),
    });
  }

  const wasPlaying = sameVariant && currentMedia.isPlaying;
  const playbackChanged = currentMedia.isPlaying !== media.isPlaying;
  const previousPausedPosition = playbackClock.pausedPosition();
  const update = playbackClock.apply(currentMedia, media, sameVariant, sampledAtMs);

  if (media.isPlaying || !sameVariant) {
    if (!sameVariant || playbackChanged) {
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
    return;
  }

  if (wasPlaying || previousPausedPosition !== update.selectedPositionMs) {
    logSync("pause position reconciled", {
      reason,
      requestDurationMs: Math.round(requestDurationMs),
      track: `${media.artist} — ${media.title}`,
      status: media.status,
      reportedPositionMs: formatSyncTimestamp(media.positionMs),
      livePositionMs: formatSyncTimestamp(
        update.livePositionMs === null ? null : Math.round(update.livePositionMs),
      ),
      differenceMs:
        update.livePositionMs === null
          ? null
          : Math.round(media.positionMs - update.livePositionMs),
      selectedPositionMs: formatSyncTimestamp(Math.round(update.selectedPositionMs)),
      usedLivePosition: update.usedLivePosition,
      toleranceMs: PAUSE_POSITION_TOLERANCE_MS,
    });
  }
}

function shouldDeferResume(
  media: MediaState,
  sameVariant: boolean,
  reason: string,
  requestDurationMs: number,
) {
  const deferred = playbackClock.shouldDeferResume(currentMedia, media, sameVariant);
  window.clearTimeout(resumeConfirmationTimer);
  if (!deferred) return false;

  logSync("deferred unconfirmed resume", {
    reason,
    requestDurationMs: Math.round(requestDurationMs),
    candidatePositionMs: media.positionMs,
    pausedPositionMs: playbackClock.pausedPosition(),
  });
  resumeConfirmationTimer = window.setTimeout(() => {
    void pollMedia("resume-confirmation");
  }, RESUME_CONFIRMATION_DELAY_MS);
  return true;
}

function renderAll() {
  renderChrome();
  renderLyrics();
  renderSettings();
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
  const displayMetadata = currentMedia.hasSession ? normalizeDisplayMetadata(currentMedia) : null;

  const titleText = currentMedia.hasSession
    ? displayMetadata?.title || "Unknown track"
    : "No media session";
  const artistText = currentMedia.hasSession
    ? displayMetadata?.artist || "Unknown artist"
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
      previousTimestampMs: formatSyncTimestamp(previousLine?.timeMs ?? null),
      nextIndex: activeLineIndex,
      nextTimestampMs: formatSyncTimestamp(activeTimestampMs),
      positionMs: formatSyncTimestamp(positionMs),
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

function formatSyncTimestamp(timestampMs: number | null) {
  if (timestampMs === null) {
    return null;
  }

  const normalizedTimestampMs = Math.max(0, Math.floor(timestampMs));
  const totalSeconds = Math.floor(normalizedTimestampMs / 1000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  const milliseconds = normalizedTimestampMs % 1000;
  return `${minutes}:${seconds.toString().padStart(2, "0")}:${milliseconds
    .toString()
    .padStart(3, "0")}`;
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
  const romanizedMode = settings.romanizedLyrics ? "romanized" : "original";

  if (!currentMedia.hasSession) {
    return `no-session:${romanizedMode}`;
  }

  if (
    lyricsMode === "searching" ||
    lyricsMode === "missing" ||
    lyricsMode === "error" ||
    lyricsMode === "unsynced" ||
    lyricsMode === "instrumental" ||
    lyricsMode === "excluded"
  ) {
    return `${lyricsMode}:${currentTrackKey}:${romanizedMode}`;
  }

  if (lyricsLines.length === 0) {
    return `missing:${currentTrackKey}:${romanizedMode}`;
  }

  return `${lyricsMode}:${romanizedMode}:${lyricsLines
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
  document.querySelectorAll<HTMLButtonElement>("[data-accent-mode]").forEach((button) => {
    const selected = button.dataset.accentMode === settings.accentMode;
    button.classList.toggle("active", selected);
    button.setAttribute("aria-checked", String(selected));
  });
  document.querySelectorAll<HTMLButtonElement>("[data-backdrop-material]").forEach((button) => {
    const selected = button.dataset.backdropMaterial === settings.backdropMaterial;
    button.classList.toggle("active", selected);
    button.setAttribute("aria-checked", String(selected));
  });
  document.querySelectorAll<HTMLButtonElement>("[data-lyrics-script]").forEach((button) => {
    const selected = (button.dataset.lyricsScript === "romanized") === settings.romanizedLyrics;
    button.classList.toggle("active", selected);
    button.setAttribute("aria-checked", String(selected));
  });
  document.querySelector<HTMLElement>("#material-description")!.textContent =
    settings.backdropMaterial === "mica"
      ? "Efficient opaque backdrop tinted from your desktop wallpaper."
      : "Live frosted-glass blur of windows behind the overlay.";
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
  renderHotkeyStatuses();
}

function renderSettingValues() {
  document.querySelector<HTMLOutputElement>("#opacity-value")!.value =
    `${Math.round(settings.opacity * 100)}%`;
  document.querySelector<HTMLOutputElement>("#blur-intensity-value")!.value =
    `${settings.blurIntensity}%`;
  document.querySelector<HTMLOutputElement>("#font-size-value")!.value = `${settings.fontSize}rem`;
  document.querySelector<HTMLOutputElement>("#line-spacing-value")!.value =
    `${settings.lineSpacing}em`;
  renderRangeProgress();
}

function renderRangeProgress() {
  document.querySelectorAll<HTMLInputElement>('input[type="range"]').forEach((input) => {
    const minimum = Number(input.min);
    const maximum = Number(input.max);
    const progress = ((Number(input.value) - minimum) / (maximum - minimum)) * 100;
    input.style.setProperty("--range-progress", `${progress}%`);
  });
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

async function syncSettingsAccent() {
  if (!tauriAvailable || !isSettingsWindow || settings.accentMode !== "dynamic") return;
  try {
    currentMedia = await invoke<MediaState>("get_media_state");
    renderedGradientKey = "";
    applyGradient();
  } catch {}
}

async function applyOverlayInteractivity() {
  if (!appWindow) {
    return;
  }
  await safeInvoke("set_window_material", {
    material: settings.backdropMaterial,
    intensity: settings.blurIntensity,
  });
  if (isSettingsWindow) return;
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
  } catch {}
}

async function safeInvoke(command: string, args?: Record<string, unknown>) {
  try {
    await invoke(command, args);
  } catch {}
}

function logSync(event: string, details: Record<string, unknown>) {
  const entry = { timestampMs: formatSyncTimestamp(Math.round(performance.now())), ...details };
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
  return decodeSettings(localStorage.getItem(SETTINGS_STORAGE_KEY));
}

function saveSettings() {
  localStorage.setItem(SETTINGS_STORAGE_KEY, JSON.stringify(settings));
  if (tauriAvailable && isSettingsWindow) {
    void emit("settings-updated", settings);
  }
}

function clearLyricsCache() {
  lyricCache.clear();
  console.info("[latency] lyrics cache cleared");
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
