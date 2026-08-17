import { describe, expect, it } from "vitest";
import { decodeSettings, defaultHotkeysFor, DEFAULT_SETTINGS, detectPlatform } from "./settings";

describe("platform defaults", () => {
  it("detects the host platform from either supported webview", () => {
    expect(
      detectPlatform(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)",
      ),
    ).toBe("macos");
    expect(
      detectPlatform(
        "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Edg/120",
      ),
    ).toBe("windows");
  });

  it("keeps macOS transport hotkeys clear of Mission Control", () => {
    const macos = defaultHotkeysFor("macos");

    expect(macos.next).toBe("Ctrl+Super+ArrowRight");
    expect(macos.previous).toBe("Ctrl+Super+ArrowLeft");
    for (const accelerator of Object.values(macos)) {
      expect(accelerator).toContain("Super");
    }
  });

  it("leaves the Windows defaults unchanged", () => {
    expect(defaultHotkeysFor("windows")).toEqual({
      pinned: "Ctrl+Shift+KeyL",
      next: "Ctrl+ArrowRight",
      previous: "Ctrl+ArrowLeft",
      playPause: "Ctrl+Shift+Space",
    });
  });

  it("hands out independent copies", () => {
    const first = defaultHotkeysFor("macos");
    first.next = "Changed";

    expect(defaultHotkeysFor("macos").next).toBe("Ctrl+Super+ArrowRight");
  });
});

describe("decodeSettings", () => {
  it("returns independent defaults for malformed data", () => {
    const first = decodeSettings("not-json");
    first.hotkeys.next = "Changed";
    expect(decodeSettings("not-json")).toEqual(DEFAULT_SETTINGS);
  });

  it("clamps modern numeric settings", () => {
    expect(
      decodeSettings(
        JSON.stringify({ opacity: 2, blurIntensity: 0, fontSize: 4, lineSpacing: 0.01 }),
      ),
    ).toMatchObject({ opacity: 1, blurIntensity: 1, fontSize: 3, lineSpacing: 0.1 });
  });

  it("normalizes valid colors and rejects invalid values", () => {
    expect(decodeSettings(JSON.stringify({ accentColor: "a1b2c3" })).accentColor).toBe("#A1B2C3");
    expect(decodeSettings(JSON.stringify({ accentColor: "red" })).accentColor).toBe(
      DEFAULT_SETTINGS.accentColor,
    );
  });

  it("accepts only typed booleans and string hotkeys", () => {
    expect(
      decodeSettings(
        JSON.stringify({
          romanizedLyrics: false,
          startAtLogin: "yes",
          hotkeys: { next: "Alt+KeyN", previous: 42 },
        }),
      ),
    ).toMatchObject({
      romanizedLyrics: false,
      startAtLogin: DEFAULT_SETTINGS.startAtLogin,
      hotkeys: {
        next: "Alt+KeyN",
        previous: DEFAULT_SETTINGS.hotkeys.previous,
      },
    });
  });
});
