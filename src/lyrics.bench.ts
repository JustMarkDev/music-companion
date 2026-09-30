import { bench, describe } from "vitest";
import { buildSyncedLyrics, NOISY_METADATA, SYNCED_RESULT } from "./bench-fixtures";
import { normalizeLyricsMetadata, parseLyrics, selectLyricsDisplay } from "./lyrics";

const fullSong = buildSyncedLyrics(80);

describe("lyrics hot paths", () => {
  bench("lrc_parse_full_song", () => {
    parseLyrics(fullSong);
  });

  bench("metadata_normalize", () => {
    normalizeLyricsMetadata(NOISY_METADATA);
  });

  bench("lyrics_display_select", () => {
    selectLyricsDisplay(SYNCED_RESULT, "Bench Track", true);
  });
});
