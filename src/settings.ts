export type AccentMode = "dynamic" | "manual";
export type BackdropMaterial = "mica" | "acrylic";
export type HotkeyAction = "pinned" | "next" | "previous" | "playPause";

export type SettingsState = {
  clickThrough: boolean;
  opacity: number;
  blurIntensity: number;
  fontSize: number;
  lineSpacing: number;
  romanizedLyrics: boolean;
  startAtLogin: boolean;
  accentMode: AccentMode;
  accentColor: string;
  backdropMaterial: BackdropMaterial;
  hotkeys: Record<HotkeyAction, string>;
};

export const DEFAULT_HOTKEYS: Record<HotkeyAction, string> = {
  pinned: "Ctrl+Shift+KeyL",
  next: "Ctrl+ArrowRight",
  previous: "Ctrl+ArrowLeft",
  playPause: "Ctrl+Shift+Space",
};

export const DEFAULT_SETTINGS: SettingsState = {
  clickThrough: false,
  opacity: 0,
  blurIntensity: 100,
  fontSize: 1,
  lineSpacing: 0.5,
  romanizedLyrics: true,
  startAtLogin: false,
  accentMode: "dynamic",
  accentColor: "#22e6c7",
  backdropMaterial: "acrylic",
  hotkeys: { ...DEFAULT_HOTKEYS },
};

export function decodeSettings(stored: string | null): SettingsState {
  try {
    const loaded: unknown = stored ? JSON.parse(stored) : {};
    const value = loaded && typeof loaded === "object" ? (loaded as Record<string, unknown>) : {};
    const hotkeys =
      value.hotkeys && typeof value.hotkeys === "object"
        ? (value.hotkeys as Record<string, unknown>)
        : {};
    return {
      clickThrough: Boolean(value.clickThrough),
      opacity: numeric(value.opacity, DEFAULT_SETTINGS.opacity, 0, 1),
      blurIntensity: numeric(value.blurIntensity, DEFAULT_SETTINGS.blurIntensity, 1, 100),
      fontSize: numeric(value.fontSize, DEFAULT_SETTINGS.fontSize, 0.5, 3),
      lineSpacing: roundStep(numeric(value.lineSpacing, DEFAULT_SETTINGS.lineSpacing, 0.1, 1.2)),
      romanizedLyrics:
        typeof value.romanizedLyrics === "boolean"
          ? value.romanizedLyrics
          : DEFAULT_SETTINGS.romanizedLyrics,
      startAtLogin:
        typeof value.startAtLogin === "boolean"
          ? value.startAtLogin
          : DEFAULT_SETTINGS.startAtLogin,
      accentMode: value.accentMode === "manual" ? "manual" : "dynamic",
      accentColor: isHexColor(value.accentColor)
        ? normalizeHexColor(value.accentColor)
        : DEFAULT_SETTINGS.accentColor,
      backdropMaterial: value.backdropMaterial === "mica" ? "mica" : "acrylic",
      hotkeys: {
        pinned: stringOr(hotkeys.pinned, DEFAULT_HOTKEYS.pinned),
        next: stringOr(hotkeys.next, DEFAULT_HOTKEYS.next),
        previous: stringOr(hotkeys.previous, DEFAULT_HOTKEYS.previous),
        playPause: stringOr(hotkeys.playPause, DEFAULT_HOTKEYS.playPause),
      },
    };
  } catch {
    return { ...DEFAULT_SETTINGS, hotkeys: { ...DEFAULT_HOTKEYS } };
  }
}

export function isHexColor(value: unknown): value is string {
  return typeof value === "string" && /^#?[\da-f]{6}$/i.test(value);
}

export function normalizeHexColor(value: string) {
  return `#${value.replace(/^#/, "").toUpperCase()}`;
}

function numeric(value: unknown, fallback: number, minimum: number, maximum: number) {
  return typeof value === "number" && Number.isFinite(value)
    ? clamp(value, minimum, maximum)
    : fallback;
}

function roundStep(value: number) {
  return Math.round(value * 20) / 20;
}

function stringOr(value: unknown, fallback: string) {
  return typeof value === "string" ? value : fallback;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(max, Math.max(min, value));
}
