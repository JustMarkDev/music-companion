import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { createIcons, Eye, EyeOff, Grip, Minus, Pin, PinOff, Power, Settings, X } from "lucide";
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
  alwaysOnTop: boolean;
  opacity: number;
  fontSize: number;
  lineSpacing: number;
  bgSaturation: number;
  pollingIntervalMs: number;
  startAtLogin: boolean;
  autoShowOnSpotify: boolean;
  startHidden: boolean;
};

type LyricLine = {
  timeMs: number | null;
  text: string;
};

const DEFAULT_SETTINGS: SettingsState = {
  alwaysOnTop: true,
  opacity: 0.96,
  fontSize: 28,
  lineSpacing: 10,
  bgSaturation: 74,
  pollingIntervalMs: 500,
  startAtLogin: false,
  autoShowOnSpotify: true,
  startHidden: false,
};

const demoState: MediaState = {
  hasSession: true,
  isPlaying: true,
  status: "Playing",
  title: "Midnight Driver",
  artist: "Music Companion",
  album: "Local Preview",
  sourceApp: "Preview",
  positionMs: 42300,
  durationMs: 184000,
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
const lyricCache = new Map<string, LyricsResult | null>();

let settings = loadSettings();
let currentMedia: MediaState = demoState;
let currentTrackKey = "";
let lyricsLines: LyricLine[] = parseLyrics(demoLyrics);
let activeLineIndex = 2;
let lyricsMode: "synced" | "plain" | "instrumental" | "missing" = "synced";
let settingsOpen = false;
let pollTimer = 0;

document.querySelector<HTMLDivElement>("#app")!.innerHTML = `
  <main class="shell">
    <section class="overlay" id="overlay">
      <header class="chrome" data-drag-region>
        <button class="icon-button handle" id="drag-handle" title="Drag window" aria-label="Drag window">
          <i data-lucide="grip"></i>
        </button>
        <div class="track-meta">
          <p class="eyebrow" id="source">Windows media session</p>
          <h1 id="title">Music Companion</h1>
          <p id="artist">Waiting for media</p>
        </div>
        <div class="window-actions">
          <button class="icon-button" id="pin-toggle" title="Always on top" aria-label="Always on top">
            <i data-lucide="pin"></i>
          </button>
          <button class="icon-button" id="settings-toggle" title="Settings" aria-label="Settings">
            <i data-lucide="settings"></i>
          </button>
          <button class="icon-button" id="minimize" title="Minimize" aria-label="Minimize">
            <i data-lucide="minus"></i>
          </button>
          <button class="icon-button danger" id="close" title="Hide" aria-label="Hide">
            <i data-lucide="x"></i>
          </button>
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
          <input id="opacity" type="range" min="30" max="100" step="1" />
        </div>
        <div class="setting-row">
          <label for="font-size">Lyric size</label>
          <input id="font-size" type="range" min="18" max="48" step="1" />
        </div>
        <div class="setting-row">
          <label for="line-spacing">Line spacing</label>
          <input id="line-spacing" type="range" min="2" max="26" step="1" />
        </div>
        <div class="setting-row">
          <label for="bg-saturation">Color</label>
          <input id="bg-saturation" type="range" min="20" max="100" step="1" />
        </div>
        <div class="setting-row">
          <label for="polling">Polling</label>
          <select id="polling">
            <option value="250">250 ms</option>
            <option value="500">500 ms</option>
            <option value="750">750 ms</option>
            <option value="1000">1000 ms</option>
          </select>
        </div>
        <label class="switch-row" for="start-login">
          <span>Start at login</span>
          <input id="start-login" type="checkbox" />
        </label>
        <label class="switch-row" for="auto-spotify">
          <span>Show on Spotify</span>
          <input id="auto-spotify" type="checkbox" />
        </label>
      </aside>
    </section>
  </main>
`;

createIcons({ icons: { Eye, EyeOff, Grip, Minus, Pin, PinOff, Power, Settings, X } });
wireUi();
applySettings();
renderAll();
void syncStartAtLogin();
schedulePolling();

function wireUi() {
  document.querySelector("#drag-handle")?.addEventListener("pointerdown", () => {
    if (appWindow) {
      void appWindow.startDragging();
    }
  });

  document.querySelector("#settings-toggle")?.addEventListener("click", () => {
    settingsOpen = !settingsOpen;
    renderSettings();
  });

  document.querySelector("#settings-close")?.addEventListener("click", () => {
    settingsOpen = false;
    renderSettings();
  });

  document.querySelector("#pin-toggle")?.addEventListener("click", () => {
    settings.alwaysOnTop = !settings.alwaysOnTop;
    saveSettings();
    applyAlwaysOnTop();
    renderChrome();
  });

  document.querySelector("#minimize")?.addEventListener("click", () => {
    if (appWindow) {
      void appWindow.minimize();
    }
  });

  document.querySelector("#close")?.addEventListener("click", () => {
    if (appWindow) {
      void appWindow.hide();
    }
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

  bindRange("bg-saturation", (value) => {
    settings.bgSaturation = value;
    saveSettings();
    applySettings();
  });

  document.querySelector<HTMLSelectElement>("#polling")?.addEventListener("change", (event) => {
    settings.pollingIntervalMs = Number((event.currentTarget as HTMLSelectElement).value);
    saveSettings();
    schedulePolling();
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

  document.querySelector<HTMLInputElement>("#auto-spotify")?.addEventListener("change", (event) => {
    settings.autoShowOnSpotify = (event.currentTarget as HTMLInputElement).checked;
    saveSettings();
  });
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
  pollTimer = window.setInterval(() => void pollMedia(), settings.pollingIntervalMs);
  void pollMedia();
}

async function pollMedia() {
  if (!tauriAvailable) {
    updateActiveLine();
    renderAll();
    return;
  }

  try {
    currentMedia = await invoke<MediaState>("get_media_state");
    const nextTrackKey = trackKey(currentMedia);

    if (nextTrackKey && nextTrackKey !== currentTrackKey) {
      currentTrackKey = nextTrackKey;
      await loadLyrics(currentMedia);
    }

    if (!nextTrackKey) {
      currentTrackKey = "";
      lyricsLines = [];
      lyricsMode = "missing";
    }

    if (
      appWindow &&
      settings.autoShowOnSpotify &&
      isSpotifySession(currentMedia) &&
      currentMedia.isPlaying
    ) {
      await appWindow.show();
      await appWindow.setFocus();
    }

    updateActiveLine();
    renderAll();
  } catch (error) {
    showStatus(`Media bridge error: ${String(error)}`);
  }
}

async function loadLyrics(media: MediaState) {
  if (!media.hasSession || !media.title) {
    lyricsLines = [];
    lyricsMode = "missing";
    return;
  }

  const key = trackKey(media);
  if (lyricCache.has(key)) {
    applyLyrics(lyricCache.get(key) ?? null);
    return;
  }

  showStatus("Fetching lyrics...");
  try {
    const result = await invoke<LyricsResult | null>("fetch_lyrics", {
      title: media.title,
      artist: media.artist,
      album: media.album,
      durationMs: media.durationMs,
    });
    lyricCache.set(key, result);
    applyLyrics(result);
  } catch {
    lyricCache.set(key, null);
    applyLyrics(null);
  }
}

function applyLyrics(result: LyricsResult | null) {
  if (!result) {
    lyricsLines = [];
    lyricsMode = "missing";
    return;
  }

  if (result.instrumental) {
    lyricsLines = [{ timeMs: null, text: "Instrumental" }];
    lyricsMode = "instrumental";
    return;
  }

  if (result.syncedLyrics) {
    lyricsLines = parseLyrics(result.syncedLyrics);
    lyricsMode = "synced";
    return;
  }

  if (result.plainLyrics) {
    lyricsLines = result.plainLyrics
      .split(/\r?\n/)
      .map((text) => ({ timeMs: null, text: text.trim() }))
      .filter((line) => line.text.length > 0);
    lyricsMode = "plain";
    return;
  }

  lyricsLines = [];
  lyricsMode = "missing";
}

function parseLyrics(raw: string): LyricLine[] {
  const lines: LyricLine[] = [];
  const pattern = /\[(\d{1,2}):(\d{2})(?:[.:](\d{1,3}))?\]/g;

  for (const rawLine of raw.split(/\r?\n/)) {
    const matches = [...rawLine.matchAll(pattern)];
    const text = rawLine.replace(pattern, "").trim();

    if (matches.length === 0 && text) {
      lines.push({ timeMs: null, text });
      continue;
    }

    for (const match of matches) {
      const minutes = Number(match[1]);
      const seconds = Number(match[2]);
      const fraction = match[3] ?? "0";
      const millis = Number(fraction.padEnd(3, "0").slice(0, 3));
      lines.push({
        timeMs: minutes * 60_000 + seconds * 1000 + millis,
        text: text || " ",
      });
    }
  }

  return lines.sort((a, b) => (a.timeMs ?? 0) - (b.timeMs ?? 0));
}

function updateActiveLine() {
  if (lyricsLines.length === 0) {
    activeLineIndex = -1;
    return;
  }

  const timed = lyricsLines.some((line) => line.timeMs !== null);
  if (!timed) {
    const ratio = currentMedia.durationMs
      ? clamp(currentMedia.positionMs / currentMedia.durationMs, 0, 0.98)
      : 0;
    activeLineIndex = Math.floor(ratio * lyricsLines.length);
    return;
  }

  let nextIndex = 0;
  for (let index = 0; index < lyricsLines.length; index += 1) {
    const time = lyricsLines[index].timeMs;
    if (time !== null && time <= currentMedia.positionMs + 160) {
      nextIndex = index;
    }
  }
  activeLineIndex = nextIndex;
}

function renderAll() {
  renderChrome();
  renderLyrics();
  renderSettings();
  renderStatus();
  applyGradient();
}

function renderChrome() {
  const source = document.querySelector("#source")!;
  const title = document.querySelector("#title")!;
  const artist = document.querySelector("#artist")!;
  const pinToggle = document.querySelector("#pin-toggle")!;

  title.textContent = currentMedia.hasSession
    ? currentMedia.title || "Unknown track"
    : "No media session";
  artist.textContent = currentMedia.hasSession
    ? currentMedia.artist || "Unknown artist"
    : "Play something";
  source.textContent = currentMedia.sourceApp || currentMedia.status || "Windows media session";
  pinToggle.innerHTML = settings.alwaysOnTop
    ? `<i data-lucide="pin"></i>`
    : `<i data-lucide="pin-off"></i>`;
  createIcons({ icons: { Pin, PinOff } });
}

function renderLyrics() {
  const list = document.querySelector<HTMLDivElement>("#lyrics-list")!;

  if (!currentMedia.hasSession) {
    list.innerHTML = `<p class="empty-state">Play something in a Windows media app.</p>`;
    return;
  }

  if (lyricsMode === "missing" || lyricsLines.length === 0) {
    list.innerHTML = `<p class="empty-state">No lyrics found for this track.</p>`;
    return;
  }

  list.innerHTML = lyricsLines
    .map((line, index) => {
      const distance = Math.abs(index - activeLineIndex);
      const className = [
        "lyric-line",
        index === activeLineIndex ? "active" : "",
        distance === 1 ? "near" : "",
        distance > 4 ? "far" : "",
      ]
        .filter(Boolean)
        .join(" ");
      return `<p class="${className}" data-index="${index}">${escapeHtml(line.text)}</p>`;
    })
    .join("");

  requestAnimationFrame(() => {
    const active = list.querySelector<HTMLElement>(".lyric-line.active");
    active?.scrollIntoView({ behavior: "smooth", block: "center" });
  });
}

function renderSettings() {
  const panel = document.querySelector<HTMLElement>("#settings-panel")!;
  panel.hidden = !settingsOpen;

  document.querySelector<HTMLInputElement>("#opacity")!.value = String(
    Math.round(settings.opacity * 100),
  );
  document.querySelector<HTMLInputElement>("#font-size")!.value = String(settings.fontSize);
  document.querySelector<HTMLInputElement>("#line-spacing")!.value = String(settings.lineSpacing);
  document.querySelector<HTMLInputElement>("#bg-saturation")!.value = String(settings.bgSaturation);
  document.querySelector<HTMLSelectElement>("#polling")!.value = String(settings.pollingIntervalMs);
  document.querySelector<HTMLInputElement>("#start-login")!.checked = settings.startAtLogin;
  document.querySelector<HTMLInputElement>("#auto-spotify")!.checked = settings.autoShowOnSpotify;
}

function renderStatus() {
  const status = document.querySelector<HTMLElement>("#status-strip")!;
  const messages = [];

  if (currentMedia.playingSessionCount > 1) {
    messages.push("Multiple players are active");
  }

  if (currentMedia.hasSession && !currentMedia.isPlaying) {
    messages.push(currentMedia.status);
  }

  if (lyricsMode === "plain") {
    messages.push("Plain lyrics");
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
  root.style.setProperty("--overlay-opacity", String(settings.opacity));
  root.style.setProperty("--lyric-size", `${settings.fontSize}px`);
  root.style.setProperty("--line-spacing", `${settings.lineSpacing}px`);
  root.style.setProperty("--bg-saturation", `${settings.bgSaturation}%`);
  void applyAlwaysOnTop();
}

async function applyAlwaysOnTop() {
  if (!appWindow) {
    return;
  }
  try {
    await appWindow.setAlwaysOnTop(settings.alwaysOnTop);
  } catch {
    await invoke("set_always_on_top", { enabled: settings.alwaysOnTop });
  }
}

function applyGradient() {
  const hue = hashHue(`${currentMedia.artist}:${currentMedia.title}`);
  document.documentElement.style.setProperty("--hue", String(hue));
  document.documentElement.style.setProperty("--hue-2", String((hue + 128) % 360));
}

function loadSettings(): SettingsState {
  try {
    const stored = localStorage.getItem("music-companion-settings");
    return stored ? { ...DEFAULT_SETTINGS, ...JSON.parse(stored) } : DEFAULT_SETTINGS;
  } catch {
    return DEFAULT_SETTINGS;
  }
}

function saveSettings() {
  localStorage.setItem("music-companion-settings", JSON.stringify(settings));
}

function trackKey(media: MediaState) {
  if (!media.hasSession || !media.title) {
    return "";
  }
  return `${media.artist.toLowerCase()}::${media.title.toLowerCase()}::${media.album.toLowerCase()}`;
}

function isSpotifySession(media: MediaState) {
  return media.sourceApp.toLowerCase().includes("spotify");
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
