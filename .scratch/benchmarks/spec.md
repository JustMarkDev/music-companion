# App benchmarks (curriculum metrics)

Runnable microbenchmarks for Music Companion's hot paths. These are engineering
metrics for coursework writeups, not AI or agent evals.

## Commands

```bash
bun run bench
bun run bench:rust
```

Frontend benches use Vitest/Tinybench. Rust benches are `#[ignore]` release tests
so they stay out of normal `cargo test` / CI.

## Metric catalog

| Metric id                  | Layer      | What it measures                                                   | Why it matters                                                    |
| -------------------------- | ---------- | ------------------------------------------------------------------ | ----------------------------------------------------------------- |
| `lrc_parse_full_song`      | TypeScript | Throughput of `parseLyrics` on an ~80-line synced LRC body         | Overlay must re-parse when a track or romanization toggle changes |
| `metadata_normalize`       | TypeScript | Throughput of `normalizeLyricsMetadata` on noisy browser titles    | Runs on every media poll before cache lookup and LRCLIB search    |
| `lyrics_display_select`    | TypeScript | Throughput of `selectLyricsDisplay` for synced + romanized results | Decides mode/notice on every lyrics update                        |
| `playback_clock_apply`     | TypeScript | Throughput of `PlaybackClock.apply` across play/pause samples      | Anchors lyric scroll; must stay cheap on the poll loop            |
| `playback_clock_estimate`  | TypeScript | Throughput of `PlaybackClock.estimate`                             | Called every animation/render tick while playing                  |
| `lyrics_cache_get_hot`     | TypeScript | Throughput of `LyricsCache.get` against a filled in-memory cache   | Cache hits avoid network; lookup cost must stay near-constant     |
| `lyrics_cache_put_persist` | TypeScript | Throughput of `putIfCurrent` with JSON persistence                 | Persisting after fetch must not stall the overlay for long        |
| `romanize_japanese_lrc`    | Rust       | Steady-state ops/sec for Japanese LRC romanization                 | Heaviest CPU path on lyric fetch when romanization is enabled     |
| `romanize_chinese_lrc`     | Rust       | Steady-state ops/sec for Chinese LRC romanization                  | Same path for Han lyrics                                          |
| `romanize_korean_lrc`      | Rust       | Steady-state ops/sec for Hangul LRC romanization                   | Same path for Korean lyrics                                       |
| `lrclib_rank_candidates`   | Rust       | Throughput of ranking/sorting a mixed LRCLIB candidate list        | Runs after every successful search before caching                 |

## Reporting

- Vitest prints hz / mean ms per bench. Prefer **hz** (ops/sec) in curriculum tables.
- Rust benches print `curriculum_metric name=... unit=ops_per_sec value=...`.
- Record machine OS, CPU, and whether the run was debug or release. Rust benches
  require `--release` via `bun run bench:rust`.
- Do not treat single-run numbers as SLAs. Use them for relative comparison and
  for showing that hot paths are measured.

## Status

Status: ready-for-agent

## Comments

- AI / agent benchmarks are out of scope for this catalog. Do not add or run them
  here.
