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
  alwaysOnTop: boolean;
  opacity: number;
  fontSize: number;
  lineSpacing: number;
  bgSaturation: number;
  syncOffsetMs: number;
  pollingIntervalMs: number;
  startAtLogin: boolean;
  autoShowOnSpotify: boolean;
  startHidden: boolean;
};

type LyricLine = {
  timeMs: number | null;
  endTimeMs: number | null;
  text: string;
  words: LyricWord[];
  hasExactWordTiming: boolean;
};

type LyricWord = {
  text: string;
  startMs: number | null;
  endMs: number | null;
  exact: boolean;
};

const DEFAULT_SETTINGS: SettingsState = {
  alwaysOnTop: true,
  opacity: 0.96,
  fontSize: 28,
  lineSpacing: 10,
  bgSaturation: 74,
  syncOffsetMs: 0,
  pollingIntervalMs: 100,
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
  positionMs: 36750,
  durationMs: 184000,
  playbackRate: 1,
  playingSessionCount: 1,
};

const demoLyrics = `[00:00.00]<00:00.00>Waiting <00:00.48>for <00:00.82>a <00:01.06>song
[00:12.20]<00:12.20>The <00:12.44>window <00:12.98>catches <00:13.54>the <00:13.80>rhythm
[00:23.40]<00:23.40>Every <00:23.88>line <00:24.34>finds <00:24.82>its <00:25.12>light
[00:36.90]<00:36.90>Floating <00:37.48>over <00:37.92>work <00:38.34>and <00:38.62>play
[00:49.10]<00:49.10>Music <00:49.56>Companion <00:50.20>keeps <00:50.68>time
[01:03.00]<01:03.00>The <01:03.22>chorus <01:03.82>arrives <01:04.32>in <01:04.58>color
[01:18.40]<01:18.40>Then <01:18.78>slips <01:19.18>back <01:19.58>into <01:20.00>the <01:20.22>night`;

const tauriAvailable = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const appWindow = tauriAvailable ? getCurrentWindow() : null;
const lyricCache = new Map<string, LyricsResult | null>();

let settings = loadSettings();
let currentMedia: MediaState = demoState;
let currentTrackKey = "";
let lyricsLines: LyricLine[] = parseLyrics(demoLyrics);
let activeLineIndex = 2;
let activeWordIndex = -1;
let lyricsMode: "synced" | "plain" | "instrumental" | "missing" = "synced";
let settingsOpen = false;
let pollTimer = 0;
let animationFrame = 0;
let mediaSampledAtMs = performance.now();
let demoStartedAtMs = performance.now();
let renderedLyricsKey = "";
let lastScrolledLineIndex = -1;

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
          <label for="sync-offset">Sync offset</label>
          <input id="sync-offset" type="range" min="-400" max="400" step="10" />
        </div>
        <div class="setting-row">
          <label for="polling">Polling</label>
          <select id="polling">
            <option value="50">50 ms</option>
            <option value="100">100 ms</option>
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
startSyncLoop();

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

  bindRange("sync-offset", (value) => {
    settings.syncOffsetMs = value;
    saveSettings();
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
    renderChrome();
    renderStatus();
    applyGradient();
    return;
  }

  try {
    currentMedia = await invoke<MediaState>("get_media_state");
    mediaSampledAtMs = performance.now();
    const nextTrackKey = trackKey(currentMedia);

    if (nextTrackKey && nextTrackKey !== currentTrackKey) {
      currentTrackKey = nextTrackKey;
      await loadLyrics(currentMedia);
    }

    if (!nextTrackKey) {
      currentTrackKey = "";
      lyricsLines = [];
      lyricsMode = "missing";
      invalidateLyricsRender();
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

    renderChrome();
    renderLyrics();
    renderStatus();
    applyGradient();
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
    lyricsLines = result.plainLyrics
      .split(/\r?\n/)
      .map((text) => createLyricLine(null, text.trim()))
      .filter((line) => line.text.length > 0);
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
  const wordDuration = line.hasExactWordTiming ? 380 : 460;
  return clamp(line.words.length * wordDuration, 1400, 7200);
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
  const parsed = parseWords(textWithWordTags);
  return {
    timeMs,
    endTimeMs: null,
    text: parsed.text || " ",
    words: parsed.words,
    hasExactWordTiming: parsed.hasExactWordTiming,
  };
}

function parseWords(textWithWordTags: string): {
  text: string;
  words: LyricWord[];
  hasExactWordTiming: boolean;
} {
  const wordTagPattern = /<(\d{1,2}):(\d{2})(?:[.:](\d{1,3}))?>/g;
  const matches = [...textWithWordTags.matchAll(wordTagPattern)];
  const text = textWithWordTags.replace(wordTagPattern, "").replace(/\s+/g, " ").trim();

  if (matches.length === 0) {
    return {
      text,
      words: splitWords(text).map((word) => ({
        text: word,
        startMs: null,
        endMs: null,
        exact: false,
      })),
      hasExactWordTiming: false,
    };
  }

  const words: LyricWord[] = [];

  for (let index = 0; index < matches.length; index += 1) {
    const match = matches[index];
    const nextMatch = matches[index + 1];
    const markerStart = parseTimeParts(match[1], match[2], match[3]);
    const markerEnd = nextMatch ? parseTimeParts(nextMatch[1], nextMatch[2], nextMatch[3]) : null;
    const segmentStart = (match.index ?? 0) + match[0].length;
    const segmentEnd = nextMatch?.index ?? textWithWordTags.length;
    const segmentWords = splitWords(textWithWordTags.slice(segmentStart, segmentEnd));

    if (segmentWords.length === 0) {
      continue;
    }

    if (segmentWords.length === 1 || markerEnd === null || markerEnd <= markerStart) {
      words.push({
        text: segmentWords.join(" "),
        startMs: markerStart,
        endMs: markerEnd,
        exact: true,
      });
      continue;
    }

    const duration = markerEnd - markerStart;
    for (let wordIndex = 0; wordIndex < segmentWords.length; wordIndex += 1) {
      const startMs = markerStart + (duration * wordIndex) / segmentWords.length;
      const endMs = markerStart + (duration * (wordIndex + 1)) / segmentWords.length;
      words.push({
        text: segmentWords[wordIndex],
        startMs,
        endMs,
        exact: true,
      });
    }
  }

  return {
    text,
    words,
    hasExactWordTiming: words.some((word) => word.exact),
  };
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
    assignWordTimings(line);
  }

  return lines;
}

function assignWordTimings(line: LyricLine) {
  if (line.timeMs === null || line.endTimeMs === null || line.words.length === 0) {
    return;
  }

  if (line.hasExactWordTiming) {
    for (let index = 0; index < line.words.length; index += 1) {
      const word = line.words[index];
      const nextExactStart = line.words
        .slice(index + 1)
        .find((candidate) => candidate.startMs !== null);
      if (word.startMs === null) {
        word.startMs = index === 0 ? line.timeMs : line.words[index - 1].endMs;
      }

      const inferredDuration =
        index > 0 && word.startMs !== null && line.words[index - 1].startMs !== null
          ? word.startMs - line.words[index - 1].startMs
          : 520;

      word.endMs =
        nextExactStart?.startMs ??
        word.endMs ??
        (word.startMs !== null
          ? Math.min(line.endTimeMs, word.startMs + clamp(inferredDuration, 320, 1200))
          : line.endTimeMs);
    }
    return;
  }

  const weights = line.words.map((word) => Math.max(1, Math.sqrt(word.text.length)));
  const totalWeight = weights.reduce((total, weight) => total + weight, 0);
  let cursor = line.timeMs;

  for (let index = 0; index < line.words.length; index += 1) {
    const duration = ((line.endTimeMs - line.timeMs) * weights[index]) / totalWeight;
    line.words[index].startMs = cursor;
    line.words[index].endMs = index === line.words.length - 1 ? line.endTimeMs : cursor + duration;
    cursor += duration;
  }
}

function updateActiveLine(positionMs = getSyncedPositionMs()) {
  if (lyricsLines.length === 0) {
    activeLineIndex = -1;
    activeWordIndex = -1;
    return;
  }

  const timed = lyricsLines.some((line) => line.timeMs !== null);
  if (!timed) {
    const ratio = currentMedia.durationMs
      ? clamp(positionMs / currentMedia.durationMs, 0, 0.98)
      : 0;
    activeLineIndex = Math.floor(ratio * lyricsLines.length);
    activeWordIndex = resolveUntimedActiveWord(activeLineIndex, ratio);
    return;
  }

  let nextIndex = 0;
  for (let index = 0; index < lyricsLines.length; index += 1) {
    const time = lyricsLines[index].timeMs;
    if (time !== null && time <= positionMs + 80) {
      nextIndex = index;
    }
  }
  activeLineIndex = nextIndex;
  activeWordIndex = resolveActiveWord(lyricsLines[activeLineIndex], positionMs);
}

function getSyncedPositionMs() {
  if (!tauriAvailable) {
    const duration = currentMedia.durationMs ?? 180_000;
    const elapsed = performance.now() - demoStartedAtMs;
    return (demoState.positionMs + elapsed + settings.syncOffsetMs + duration) % duration;
  }

  const rate = currentMedia.playbackRate ?? 1;
  const elapsed = currentMedia.isPlaying
    ? Math.min(performance.now() - mediaSampledAtMs, Math.max(1000, settings.pollingIntervalMs * 5))
    : 0;
  const position = currentMedia.positionMs + elapsed * rate + settings.syncOffsetMs;
  return currentMedia.durationMs
    ? clamp(position, 0, currentMedia.durationMs)
    : Math.max(0, position);
}

function resolveActiveWord(line: LyricLine | undefined, positionMs: number) {
  if (!line || line.words.length === 0) {
    return -1;
  }

  for (let index = 0; index < line.words.length; index += 1) {
    const word = line.words[index];
    if (
      word.startMs !== null &&
      word.endMs !== null &&
      word.startMs <= positionMs + 50 &&
      word.endMs >= positionMs - 20
    ) {
      return index;
    }
  }

  return -1;
}

function resolveUntimedActiveWord(lineIndex: number, songRatio: number) {
  const line = lyricsLines[lineIndex];
  if (!line || line.words.length === 0) {
    return -1;
  }

  const lineStartRatio = lineIndex / lyricsLines.length;
  const lineEndRatio = (lineIndex + 1) / lyricsLines.length;
  const lineRatio = clamp((songRatio - lineStartRatio) / (lineEndRatio - lineStartRatio), 0, 0.98);
  return Math.floor(lineRatio * line.words.length);
}

function getLineProgress(line: LyricLine | undefined, positionMs: number) {
  if (!line) {
    return 0;
  }

  if (line.timeMs !== null && line.endTimeMs !== null) {
    return clamp((positionMs - line.timeMs) / (line.endTimeMs - line.timeMs), 0, 1);
  }

  if (!currentMedia.durationMs || lyricsLines.length === 0) {
    return 0;
  }

  const lineIndex = lyricsLines.indexOf(line);
  const lineStart = (currentMedia.durationMs * lineIndex) / lyricsLines.length;
  const lineEnd = (currentMedia.durationMs * (lineIndex + 1)) / lyricsLines.length;
  return clamp((positionMs - lineStart) / (lineEnd - lineStart), 0, 1);
}

function getWordState(line: LyricLine, lineIndex: number, wordIndex: number, positionMs: number) {
  const word = line.words[wordIndex];
  if (!word) {
    return { progress: 0 };
  }

  if (word.startMs !== null && word.endMs !== null && word.endMs > word.startMs) {
    return { progress: (positionMs - word.startMs) / (word.endMs - word.startMs) };
  }

  if (!currentMedia.durationMs || lyricsLines.length === 0 || line.words.length === 0) {
    return { progress: 0 };
  }

  const lineStart = (currentMedia.durationMs * lineIndex) / lyricsLines.length;
  const lineEnd = (currentMedia.durationMs * (lineIndex + 1)) / lyricsLines.length;
  const wordStart = lineStart + ((lineEnd - lineStart) * wordIndex) / line.words.length;
  const wordEnd = lineStart + ((lineEnd - lineStart) * (wordIndex + 1)) / line.words.length;
  return { progress: (positionMs - wordStart) / (wordEnd - wordStart) };
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
      const words =
        line.words.length > 0
          ? line.words
              .map(
                (word, wordIndex) =>
                  `<span class="lyric-word" data-word-index="${wordIndex}" style="--word-progress: 0">${escapeHtml(word.text)}</span>`,
              )
              .join(" ")
          : escapeHtml(line.text);
      return `<p class="${className}" data-line-index="${index}" style="--line-progress: 0">${words}</p>`;
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
  const positionMs = getSyncedPositionMs();
  updateActiveLine(positionMs);
  updateLyricDom(positionMs);
}

function updateLyricDom(positionMs: number) {
  const list = document.querySelector<HTMLDivElement>("#lyrics-list");
  if (!list || lyricsLines.length === 0) {
    return;
  }

  const lineElements = list.querySelectorAll<HTMLElement>(".lyric-line");
  lineElements.forEach((lineElement) => {
    const lineIndex = Number(lineElement.dataset.lineIndex);
    const line = lyricsLines[lineIndex];
    const distance = Math.abs(lineIndex - activeLineIndex);

    lineElement.classList.toggle("active", lineIndex === activeLineIndex);
    lineElement.classList.toggle("near", distance === 1);
    lineElement.classList.toggle("far", distance > 4);
    lineElement.style.setProperty("--line-progress", String(getLineProgress(line, positionMs)));

    const wordElements = lineElement.querySelectorAll<HTMLElement>(".lyric-word");
    wordElements.forEach((wordElement) => {
      const wordIndex = Number(wordElement.dataset.wordIndex);
      const state = getWordState(line, lineIndex, wordIndex, positionMs);

      wordElement.classList.toggle("past", state.progress >= 1);
      wordElement.classList.toggle(
        "active",
        lineIndex === activeLineIndex && wordIndex === activeWordIndex,
      );
      wordElement.classList.toggle("upcoming", state.progress <= 0);
      wordElement.classList.toggle("exact", Boolean(line.words[wordIndex]?.exact));
      wordElement.style.setProperty("--word-progress", String(clamp(state.progress, 0, 1)));
    });
  });

  if (activeLineIndex !== lastScrolledLineIndex) {
    lastScrolledLineIndex = activeLineIndex;
    list
      .querySelector<HTMLElement>(`.lyric-line[data-line-index="${activeLineIndex}"]`)
      ?.scrollIntoView({ behavior: "smooth", block: "center" });
  }
}

function getLyricsRenderKey() {
  if (!currentMedia.hasSession) {
    return "no-session";
  }

  if (lyricsMode === "missing" || lyricsLines.length === 0) {
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
  panel.hidden = !settingsOpen;

  document.querySelector<HTMLInputElement>("#opacity")!.value = String(
    Math.round(settings.opacity * 100),
  );
  document.querySelector<HTMLInputElement>("#font-size")!.value = String(settings.fontSize);
  document.querySelector<HTMLInputElement>("#line-spacing")!.value = String(settings.lineSpacing);
  document.querySelector<HTMLInputElement>("#bg-saturation")!.value = String(settings.bgSaturation);
  document.querySelector<HTMLInputElement>("#sync-offset")!.value = String(settings.syncOffsetMs);
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
