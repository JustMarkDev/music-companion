import { describe, expect, it } from "vitest";
import { decodeSettings, DEFAULT_SETTINGS } from "./settings";

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
