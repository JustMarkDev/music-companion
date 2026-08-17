import { PLATFORM, type Platform } from "./settings";

type KeyboardShortcut = Pick<KeyboardEvent, "ctrlKey" | "shiftKey" | "altKey" | "metaKey" | "code">;

/** Modifier order matches the accelerator grammar the global-shortcut plugin parses. */
export function keyboardEventToAccelerator(event: KeyboardShortcut) {
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.shiftKey) parts.push("Shift");
  if (event.altKey) parts.push("Alt");
  if (event.metaKey) parts.push("Super");
  parts.push(event.code);
  return parts.join("+");
}

/** Modifier glyphs in the order macOS renders them in its own menus. */
const MACOS_SYMBOLS: Record<string, string> = {
  Ctrl: "⌃",
  Alt: "⌥",
  Shift: "⇧",
  Super: "⌘",
};

const MACOS_MODIFIER_ORDER = ["Ctrl", "Alt", "Shift", "Super"];

export function formatAccelerator(accelerator: string, platform: Platform = PLATFORM) {
  const parts = accelerator
    .replace(/Key([A-Z])/g, "$1")
    .replace("ArrowRight", "Right Arrow")
    .replace("ArrowLeft", "Left Arrow")
    .replace("ArrowUp", "Up Arrow")
    .replace("ArrowDown", "Down Arrow")
    .split("+");

  if (platform !== "macos") {
    return parts.join(" + ");
  }

  const modifiers = parts.filter((part) => part in MACOS_SYMBOLS);
  const keys = parts.filter((part) => !(part in MACOS_SYMBOLS));
  const symbols = MACOS_MODIFIER_ORDER.filter((modifier) => modifiers.includes(modifier))
    .map((modifier) => MACOS_SYMBOLS[modifier])
    .join("");

  return `${symbols}${keys.join(" + ")}`;
}
