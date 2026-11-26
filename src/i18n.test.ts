import { afterEach, describe, expect, it } from "vite-plus/test";
import { ITALIAN, setLanguage, t } from "./i18n";

afterEach(() => setLanguage("en"));

describe("t", () => {
  it("keeps the English text by default", () => {
    expect(t("Settings")).toBe("Settings");
  });

  it("translates to Italian and shows English text that has no translation", () => {
    setLanguage("it");
    expect(t("Settings")).toBe("Impostazioni");
    expect(t("A text nobody translated")).toBe("A text nobody translated");
  });

  it("fills in the values by name, in either language", () => {
    expect(t("Use {color}", { color: "#FFF" })).toBe("Use #FFF");
    setLanguage("it");
    expect(t("Use {color}", { color: "#FFF" })).toBe("Usa #FFF");
    expect(t("Use {color}")).toBe("Usa {color}");
  });

  it("keeps every value of an English text in its translation", () => {
    for (const [english, italian] of Object.entries(ITALIAN)) {
      const names = (text: string) =>
        [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
      expect(names(italian), english).toEqual(names(english));
    }
  });
});

describe("the Italian interface", () => {
  const files = import.meta.glob<string>("./*.ts", {
    query: "?raw",
    import: "default",
    eager: true,
  });
  const sources = Object.entries(files)
    .filter(([file]) => !file.endsWith(".test.ts") && file !== "./i18n.ts")
    .map(([, source]) => source);
  // Every text the code asks `t` for with a plain string.
  const asked = new Set(
    sources.flatMap((source) =>
      [...source.matchAll(/\bt\(\s*(["`])((?:\\.|(?!\1)[^\\])*)\1/g)].map((match) => match[2]),
    ),
  );

  it("translates every text the code asks for", () => {
    expect(asked.size).toBeGreaterThan(40);
    const missing = [...asked].filter((text) => !(text in ITALIAN));
    expect(missing).toEqual([]);
  });

  it("has no translation of a text that is never asked for", () => {
    const source = sources.join("\n");
    const unused = Object.keys(ITALIAN).filter(
      (text) => !asked.has(text) && !source.includes(JSON.stringify(text)),
    );
    expect(unused).toEqual([]);
  });
});
