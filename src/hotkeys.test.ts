import { describe, expect, it } from "vitest";
import { formatAccelerator, keyboardEventToAccelerator } from "./hotkeys";

describe("hotkey accelerators", () => {
  it("creates a stable accelerator in modifier order", () => {
    expect(
      keyboardEventToAccelerator({
        ctrlKey: true,
        shiftKey: true,
        altKey: true,
        metaKey: true,
        code: "KeyL",
      }),
    ).toBe("Ctrl+Shift+Alt+Super+KeyL");
  });

  it("keeps media and non-letter key codes intact", () => {
    expect(
      keyboardEventToAccelerator({
        ctrlKey: false,
        shiftKey: false,
        altKey: false,
        metaKey: false,
        code: "MediaTrackNext",
      }),
    ).toBe("MediaTrackNext");
  });

  it("formats letters and arrow keys for display", () => {
    expect(formatAccelerator("Ctrl+Shift+KeyL", "windows")).toBe("Ctrl + Shift + L");
    expect(formatAccelerator("Ctrl+ArrowRight", "windows")).toBe("Ctrl + Right Arrow");
  });

  it("renders macOS modifiers as glyphs in system order", () => {
    expect(formatAccelerator("Shift+Super+KeyL", "macos")).toBe("⇧⌘L");
    expect(formatAccelerator("Ctrl+Super+ArrowRight", "macos")).toBe("⌃⌘Right Arrow");
    expect(formatAccelerator("Ctrl+Shift+Alt+Super+KeyL", "macos")).toBe("⌃⌥⇧⌘L");
  });

  it("keeps unmodified macOS keys readable", () => {
    expect(formatAccelerator("MediaTrackNext", "macos")).toBe("MediaTrackNext");
  });
});
