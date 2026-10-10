import { describe, test } from "vite-plus/test";
import { buildLyricsResult, NOISY_METADATA, SYNCED_RESULT } from "./bench-fixtures";
import { normalizeLyricsMetadata, selectLyricsDisplay } from "./lyrics";

const fullSong = buildLyricsResult(80);

describe("lyrics hot paths", () => {
  test("lyrics hot paths", async ({ bench }) => {
    await bench.compare(
      bench("lyrics_build_full_song", () => {
        selectLyricsDisplay(fullSong, "Bench Track", "original");
      }),
      bench("metadata_normalize", () => {
        normalizeLyricsMetadata(NOISY_METADATA);
      }),
      bench("lyrics_display_select", () => {
        selectLyricsDisplay(SYNCED_RESULT, "Bench Track", "romanized");
      }),
    );
  });
});
