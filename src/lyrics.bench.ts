import { describe, test } from "vite-plus/test";
import { buildSyncedLyrics, NOISY_METADATA, SYNCED_RESULT } from "./bench-fixtures";
import { normalizeLyricsMetadata, parseLyrics, selectLyricsDisplay } from "./lyrics";

const fullSong = buildSyncedLyrics(80);

describe("lyrics hot paths", () => {
  test("lyrics hot paths", async ({ bench }) => {
    await bench.compare(
      bench("lrc_parse_full_song", () => {
        parseLyrics(fullSong);
      }),
      bench("metadata_normalize", () => {
        normalizeLyricsMetadata(NOISY_METADATA);
      }),
      bench("lyrics_display_select", () => {
        selectLyricsDisplay(SYNCED_RESULT, "Bench Track", true);
      }),
    );
  });
});
