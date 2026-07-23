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
    expect(formatAccelerator("Ctrl+Shift+KeyL")).toBe("Ctrl + Shift + L");
    expect(formatAccelerator("Ctrl+ArrowRight")).toBe("Ctrl + Right Arrow");
  });
});
