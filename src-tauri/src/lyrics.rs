//! Lyrics lookup. lrc.red is the only provider: Apple Music lyrics, many with
//! word timing, a transliteration and a translation, served as TTML.

use crate::LATEST_LYRICS_REQUEST;
use crate::ttml::{self, Line, LineText, Lyrics, Segment};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LyricsResult {
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: Option<u64>,
    /// True when some line times its individual words.
    word_timed: bool,
    lines: Vec<Line>,
}

/// What lrc.red says a recording is, used to rank its hits against the song
/// that is playing.
#[derive(Debug, Clone)]
struct Candidate {
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    /// Seconds.
    duration: Option<f64>,
}

static HTTP_CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();

/// Total time a lookup gets for all the requests it makes (not each one), so a
/// stalled service ends in an error that is retried later instead of hanging.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(6);

type Lookup = Result<Option<LyricsResult>, String>;

/// Gives `lookup` at most `deadline`; running out of time is an error, so the
/// lookup is retried later instead of being cached as a miss.
async fn within_deadline(
    provider: &str,
    deadline: Duration,
    lookup: impl Future<Output = Lookup>,
) -> Lookup {
    tokio::time::timeout(deadline, lookup)
        .await
        .unwrap_or_else(|_| Err(provider_error(provider, "timed out")))
}

/// Tries ranked candidates in order and returns the first one that yields
/// lyrics. A candidate whose request fails does not stop the search; its
/// error is returned only when no candidate yields lyrics, so a transient
/// failure is retried instead of cached as a miss.
async fn first_found<K: Clone, T, Fetch: Future<Output = Result<Option<T>, String>>>(
    candidates: Vec<(Candidate, K)>,
    mut fetch: impl FnMut(K) -> Fetch,
) -> Result<Option<(Candidate, K, T)>, String> {
    let mut failure = None;
    for (candidate, key) in candidates {
        match fetch(key.clone()).await {
            Ok(Some(found)) => return Ok(Some((candidate, key, found))),
            Ok(None) => {}
            Err(error) => failure = Some(error),
        }
    }
    failure.map_or(Ok(None), Err)
}

/// Looks the song up on lrc.red. The lookup is dropped (cancelled) as soon as a
/// newer request supersedes it.
pub async fn fetch_lyrics(
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
    request_id: u64,
) -> Lookup {
    let client = http_client()?;
    let mut lookup = std::pin::pin!(within_deadline(
        "lrc.red",
        PROVIDER_TIMEOUT,
        fetch_lrc_red(client, title, artist, duration_ms)
    ));
    loop {
        tokio::select! {
            result = &mut lookup => return result,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if LATEST_LYRICS_REQUEST.load(std::sync::atomic::Ordering::Acquire) != request_id {
                    return Err("lyrics request superseded".to_string());
                }
            }
        }
    }
}

fn http_client() -> Result<&'static reqwest::Client, String> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .user_agent(concat!(
                    "MusicCompanion/",
                    env!("CARGO_PKG_VERSION"),
                    " (https://github.com/JustMarkDev/Music-Companion)"
                ))
                // Use Windows' TLS stack and certificate store, matching
                // the trust configuration used by the browser.
                .connect_timeout(std::time::Duration::from_secs(8))
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|char| char.is_alphanumeric() || char.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn canonical_title(value: &str, normalized_artist: &str) -> String {
    let normalized_title = normalize(value);
    if normalized_artist.is_empty() {
        return normalized_title;
    }

    normalized_title
        .strip_prefix(normalized_artist)
        .and_then(|title| title.strip_prefix(' '))
        .or_else(|| {
            normalized_title
                .strip_suffix(normalized_artist)
                .and_then(|title| title.strip_suffix(' '))
        })
        .unwrap_or(&normalized_title)
        .to_string()
}

fn score(value: Option<&str>, expected: &str) -> u8 {
    if expected.is_empty() {
        return 0;
    }

    let Some(value) = value else {
        return 0;
    };

    let value = normalize(value);
    if value == expected {
        4
    } else if value.contains(expected) || expected.contains(&value) {
        2
    } else {
        0
    }
}

fn duration_difference_ms(candidate_seconds: Option<f64>, expected_ms: Option<u64>) -> u64 {
    let Some(expected_ms) = expected_ms else {
        return 0;
    };
    let Some(candidate_seconds) = candidate_seconds.filter(|value| value.is_finite()) else {
        return u64::MAX;
    };

    let candidate_ms = (candidate_seconds.max(0.0) * 1_000.0).round() as u64;
    candidate_ms.abs_diff(expected_ms)
}

fn duration_matches(candidate_seconds: Option<f64>, expected_ms: Option<u64>) -> bool {
    const DURATION_TOLERANCE_MS: u64 = 3_000;

    expected_ms.is_none()
        || duration_difference_ms(candidate_seconds, expected_ms) <= DURATION_TOLERANCE_MS
}

/// How well a candidate's track name alone matches the playing title, 0-4.
/// Unlike the title score in `metadata_scores`, an album with that name does
/// not count.
fn track_title_score(candidate: &Candidate, normalized_title: &str, normalized_artist: &str) -> u8 {
    let track_title = candidate
        .track_name
        .as_deref()
        .map(|title| canonical_title(title, normalized_artist));
    score(track_title.as_deref(), normalized_title)
}

/// How well a candidate's title and artist match what is playing, each 0-4.
fn metadata_scores(
    candidate: &Candidate,
    normalized_title: &str,
    normalized_artist: &str,
) -> (u8, u8) {
    // The album only vouches for the title when the hit has no track name:
    // every track of an album named like the playing song (AC/DC's "Back In
    // Black") would otherwise tie with the song itself.
    let title_score = match candidate
        .track_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        Some(_) => track_title_score(candidate, normalized_title, normalized_artist),
        None => score(candidate.album_name.as_deref(), normalized_title),
    };
    let artist_score = [
        score(candidate.artist_name.as_deref(), normalized_artist),
        score(candidate.track_name.as_deref(), normalized_artist),
        score(candidate.album_name.as_deref(), normalized_artist),
    ]
    .into_iter()
    .max()
    .unwrap_or_default();
    (title_score, artist_score)
}

fn ranking_key(
    candidate: &Candidate,
    normalized_title: &str,
    normalized_artist: &str,
    duration_ms: Option<u64>,
) -> (std::cmp::Reverse<bool>, std::cmp::Reverse<u8>, u64) {
    let (title_score, artist_score) =
        metadata_scores(candidate, normalized_title, normalized_artist);
    let metadata_score = title_score * 4 + artist_score * 3;
    let metadata_matches = title_score > 0 && artist_score > 0;

    (
        std::cmp::Reverse(metadata_matches),
        std::cmp::Reverse(metadata_score),
        duration_difference_ms(candidate.duration, duration_ms),
    )
}

fn is_latin_letter(char: char) -> bool {
    char.is_ascii_alphabetic() || matches!(char, '\u{C0}'..='\u{24F}' | '\u{1E00}'..='\u{1EFF}')
}

fn has_non_latin_letters(value: &str) -> bool {
    value
        .chars()
        .any(|char| char.is_alphabetic() && !is_latin_letter(char))
}

/// True when exactly one of the two titles is written in a non-Latin script,
/// so a differing spelling says nothing about whether they are the same song.
/// A hit without a title tells nothing about its script.
fn titles_differ_in_script(candidate_title: Option<&str>, playing_title: &str) -> bool {
    candidate_title
        .is_some_and(|title| has_non_latin_letters(title) != has_non_latin_letters(playing_title))
}

/// Keeps the hits that plausibly are the playing song, best first. Search
/// results are fuzzy (remixes, covers, other artists), so a hit needs a
/// matching length and a matching title. The same song can be credited to an
/// artist written in another script, so the artist need not match; a title may
/// differ only when it is written in another script, never to pass off another
/// song by the same artist.
fn rank_matches<T>(
    mut hits: Vec<(Candidate, T)>,
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
) -> Vec<(Candidate, T)> {
    let normalized_artist = normalize(artist);
    let normalized_title = canonical_title(title, &normalized_artist);
    hits.retain(|(candidate, _)| {
        let (title_score, artist_score) =
            metadata_scores(candidate, &normalized_title, &normalized_artist);
        duration_matches(candidate.duration, duration_ms)
            && (title_score > 0
                || (artist_score > 0
                    && titles_differ_in_script(candidate.track_name.as_deref(), title)))
    });
    hits.sort_by_key(|(candidate, _)| {
        ranking_key(
            candidate,
            &normalized_title,
            &normalized_artist,
            duration_ms,
        )
    });
    hits
}

fn provider_error(provider: &str, error: impl std::fmt::Display) -> String {
    format!("{provider}: {error}")
}

fn build_result(candidate: Candidate, lyrics: Lyrics) -> LyricsResult {
    let word_timed = lyrics.is_word_timed();
    LyricsResult {
        track_name: candidate.track_name.unwrap_or_default(),
        artist_name: candidate.artist_name.unwrap_or_default(),
        album_name: candidate.album_name.unwrap_or_default(),
        duration: candidate.duration.map(|value| value.round() as u64),
        word_timed,
        lines: lyrics.lines,
    }
}

#[derive(Deserialize)]
struct LrcRedMatches {
    #[serde(default)]
    hits: Vec<LrcRedHit>,
}

#[derive(Deserialize)]
struct LrcRedHit {
    isrc: String,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration: Option<f64>,
}

/// Players often join collaborators into one artist ("Gorillaz & Del the
/// Funky Homosapien") that lrc.red cannot match, so a miss is retried with the
/// primary artist alone.
async fn fetch_lrc_red(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
) -> Lookup {
    let found = search_lrc_red(client, title, artist, duration_ms).await;
    if !matches!(found, Ok(None)) {
        return found;
    }
    match primary_artist(artist) {
        Some(primary) => search_lrc_red(client, title, primary, duration_ms).await,
        None => found,
    }
}

/// The first credited artist of a joined artist string, or `None` when it
/// names a single artist. Separators need surrounding spaces, so names such
/// as "Simon&Garfunkel" are left whole.
fn primary_artist(artist: &str) -> Option<&str> {
    const SEPARATORS: [&str; 9] = [
        " & ",
        ", ",
        "; ",
        " feat. ",
        " feat ",
        " ft. ",
        " featuring ",
        " x ",
        " / ",
    ];
    let lower = artist.to_ascii_lowercase();
    let index = SEPARATORS
        .iter()
        .filter_map(|separator| lower.find(separator))
        .min()?;
    let primary = artist[..index].trim();
    (!primary.is_empty()).then_some(primary)
}

/// One `/match.json` query: the hits for a title and artist, with the
/// duration weighed in when given.
async fn lrc_red_hits(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    duration_seconds: Option<f64>,
) -> Result<Vec<LrcRedHit>, String> {
    let mut url = format!(
        "https://lrc.red/match.json?title={}&artist={}",
        urlencoding::encode(title),
        urlencoding::encode(artist)
    );
    if let Some(duration_seconds) = duration_seconds {
        url.push_str(&format!("&duration={duration_seconds}"));
    }
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|error| provider_error("lrc.red match", error))?;
    if !response.status().is_success() {
        return Err(provider_error("lrc.red match", response.status()));
    }
    let matches = response
        .json::<LrcRedMatches>()
        .await
        .map_err(|error| provider_error("lrc.red match", error))?;
    Ok(matches.hits)
}

/// Hits of both queries without repeats, those of the title query first.
fn merge_lrc_red_hits(by_title: Vec<LrcRedHit>, by_duration: Vec<LrcRedHit>) -> Vec<LrcRedHit> {
    let mut seen = HashSet::new();
    by_title
        .into_iter()
        .chain(by_duration)
        .filter(|hit| seen.insert(hit.isrc.clone()))
        .collect()
}

/// The recordings lrc.red lists for a song that plausibly are it, best
/// first, each with its ISRC.
///
/// `/match.json` weighs the duration above the title, so with one it lists
/// other songs of about that length and can leave out the song itself. The
/// query without a duration finds the song by its name, the one with a
/// duration finds its variants of the right length; both are ranked here.
async fn lrc_red_matches(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
) -> Result<Vec<(Candidate, String)>, String> {
    let hits = match duration_ms {
        None => lrc_red_hits(client, title, artist, None).await?,
        Some(duration_ms) => {
            let (by_title, by_duration) = tokio::join!(
                lrc_red_hits(client, title, artist, None),
                lrc_red_hits(
                    client,
                    title,
                    artist,
                    Some((duration_ms as f64 / 1_000.0).round())
                )
            );
            match (by_title, by_duration) {
                (Ok(by_title), Ok(by_duration)) => merge_lrc_red_hits(by_title, by_duration),
                (Ok(hits), Err(_)) | (Err(_), Ok(hits)) => hits,
                (Err(error), Err(_)) => return Err(error),
            }
        }
    };
    let candidates = hits
        .into_iter()
        .map(|hit| {
            let candidate = Candidate {
                track_name: hit.title,
                artist_name: hit.artist,
                album_name: hit.album,
                duration: hit.duration,
            };
            (candidate, hit.isrc)
        })
        .collect();
    Ok(rank_matches(candidates, title, artist, duration_ms))
}

/// `/match.json` finds the recording, `/s/{isrc}.ttml` is its lyrics.
async fn search_lrc_red(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
) -> Lookup {
    let started_at = std::time::Instant::now();
    // A hit can lack a lyrics file, so fall through to the next best one.
    let ranked = lrc_red_matches(client, title, artist, duration_ms)
        .await?
        .into_iter()
        .take(3)
        .collect();
    let found = first_found(ranked, |isrc: String| async move {
        fetch_lrc_red_ttml(client, &isrc).await
    })
    .await?;
    let Some((candidate, isrc, lyrics)) = found else {
        println!(
            "[latency] lrc.red total={}ms no match",
            started_at.elapsed().as_millis()
        );
        return Ok(None);
    };
    println!(
        "[latency] lrc.red total={}ms isrc={isrc}",
        started_at.elapsed().as_millis()
    );
    Ok(Some(build_result(candidate, lyrics)))
}

/// The lyrics of one recording, or `None` when it has no usable file.
async fn fetch_lrc_red_ttml(
    client: &reqwest::Client,
    isrc: &str,
) -> Result<Option<Lyrics>, String> {
    let response = client
        .get(format!(
            "https://lrc.red/s/{}.ttml",
            urlencoding::encode(isrc)
        ))
        .send()
        .await
        .map_err(|error| provider_error("lrc.red lyrics", error))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(provider_error("lrc.red lyrics", response.status()));
    }
    let ttml = response
        .text()
        .await
        .map_err(|error| provider_error("lrc.red lyrics", error))?;
    Ok(ttml::parse(&ttml))
}

/// The first sync of a recording runs lrc.red's alignment model, which takes
/// seconds; later requests for it are answered from what it stored.
const WORD_SYNC_TIMEOUT: Duration = Duration::from_secs(45);
/// Longest the TTML of a synced recording may take. It is a bonus: the sync
/// itself is already enough to show the words.
const SYNCED_TTML_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct LrcRedSong {
    #[serde(default)]
    lyrics: LrcRedLyrics,
}

#[derive(Deserialize, Default)]
struct LrcRedLyrics {
    #[serde(default)]
    lines: Vec<LrcRedLine>,
}

#[derive(Deserialize)]
struct LrcRedLine {
    /// Words made of one or more timed syllables; empty for an untimed line.
    #[serde(default)]
    words: Vec<Vec<LrcRedWord>>,
}

#[derive(Deserialize)]
struct LrcRedWord {
    text: String,
    /// Seconds.
    begin: f64,
    end: f64,
}

fn seconds_to_ms(seconds: f64) -> u64 {
    (seconds * 1_000.0).round().max(0.0) as u64
}

/// The synced lines of a song, one segment per word with its syllables joined.
/// Untimed lines are left out.
fn song_to_lyrics(song: &LrcRedSong) -> Option<Lyrics> {
    let lines = song
        .lyrics
        .lines
        .iter()
        .filter_map(|line| {
            let words = line
                .words
                .iter()
                .filter(|word| !word.is_empty())
                .collect::<Vec<_>>();
            let first = words.first()?.first()?;
            let last_index = words.len() - 1;
            let segments = words
                .iter()
                .enumerate()
                .map(|(index, word)| {
                    let mut text = word
                        .iter()
                        .map(|syllable| syllable.text.as_str())
                        .collect::<String>();
                    if index != last_index {
                        text.push(' ');
                    }
                    Segment {
                        start_ms: seconds_to_ms(word[0].begin),
                        end_ms: seconds_to_ms(word[word.len() - 1].end),
                        text,
                    }
                })
                .collect::<Vec<_>>();
            let start_ms = seconds_to_ms(first.begin);
            let end_ms = segments.iter().map(|s| s.end_ms).max().unwrap_or(start_ms);
            Some(Line {
                start_ms,
                end_ms,
                voice: 0,
                text: LineText {
                    segments,
                    background: Vec::new(),
                },
                romanized: None,
                translation: None,
            })
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then_some(Lyrics { lines })
}

/// Asks lrc.red to time every word of one recording. `None` means lrc.red
/// cannot, which will not change on retrying; an error is a failure worth
/// retrying later.
async fn sync_lrc_red_words(
    client: &reqwest::Client,
    isrc: &str,
) -> Result<Option<Lyrics>, String> {
    let response = client
        .post(format!(
            "https://lrc.red/s/{}/sync",
            urlencoding::encode(isrc)
        ))
        .timeout(WORD_SYNC_TIMEOUT)
        .send()
        .await
        .map_err(|error| provider_error("lrc.red sync", error))?;
    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(provider_error("lrc.red sync", status));
    }
    if status.is_client_error() {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(provider_error("lrc.red sync", status));
    }
    let song = response
        .json::<LrcRedSong>()
        .await
        .map_err(|error| provider_error("lrc.red sync", error))?;
    Ok(song_to_lyrics(&song).filter(Lyrics::is_word_timed))
}

/// Word-timed lyrics for a song, from lrc.red's alignment model. The song is
/// found the same way as for a lookup, so a different recording of it is
/// never timed by mistake.
pub async fn sync_words(title: &str, artist: &str, duration_ms: Option<u64>) -> Lookup {
    let started_at = std::time::Instant::now();
    let client = http_client()?;
    let mut matches = lrc_red_matches(client, title, artist, duration_ms).await?;
    if matches.is_empty()
        && let Some(primary) = primary_artist(artist)
    {
        matches = lrc_red_matches(client, title, primary, duration_ms).await?;
    }
    // A recording lrc.red cannot time falls through to the next best one.
    let ranked = matches.into_iter().take(3).collect();
    let found = first_found(ranked, |isrc: String| async move {
        sync_lrc_red_words(client, &isrc).await
    })
    .await?;
    let Some((candidate, isrc, synced)) = found else {
        println!(
            "[latency] lrc.red sync total={}ms not timed",
            started_at.elapsed().as_millis()
        );
        return Ok(None);
    };
    println!(
        "[latency] lrc.red sync total={}ms isrc={isrc}",
        started_at.elapsed().as_millis()
    );
    // What the sync stored may now be served as TTML, together with the
    // transliteration and translation the sync answer does not carry.
    let lyrics =
        match tokio::time::timeout(SYNCED_TTML_TIMEOUT, fetch_lrc_red_ttml(client, &isrc)).await {
            Ok(Ok(Some(stored))) if stored.is_word_timed() => stored,
            _ => synced,
        };
    Ok(Some(build_result(candidate, lyrics)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(track_name: &str, artist_name: &str, duration: f64) -> Candidate {
        Candidate {
            track_name: Some(track_name.to_string()),
            artist_name: Some(artist_name.to_string()),
            album_name: None,
            duration: Some(duration),
        }
    }

    #[test]
    fn metadata_match_outranks_closer_duration() {
        let normalized_artist = normalize("Jace June");
        let normalized_title = canonical_title("Goodbye My Baby", &normalized_artist);
        let expected_duration_ms = Some(182_000);
        let mut results = [
            candidate("Deeper Than It Seems", "Jace June", 182.0),
            candidate("Goodbye My Baby", "Jace June", 194.0),
        ];

        results.sort_by_key(|item| {
            ranking_key(
                item,
                &normalized_title,
                &normalized_artist,
                expected_duration_ms,
            )
        });

        assert_eq!(results[0].track_name.as_deref(), Some("Goodbye My Baby"));
    }

    #[test]
    fn combined_artist_and_title_forms_have_equal_metadata_rank() {
        let normalized_artist = normalize("Jace June");
        let normalized_title = canonical_title("Goodbye My Baby", &normalized_artist);
        let expected_duration_ms = Some(194_000);
        let candidates = [
            candidate("Goodbye My Baby", "Jace June", 194.0),
            candidate("Jace June - Goodbye My Baby", "Jace June", 194.0),
            candidate("Goodbye My Baby - Jace June", "Jace June", 194.0),
        ];

        let keys = candidates.map(|item| {
            ranking_key(
                &item,
                &normalized_title,
                &normalized_artist,
                expected_duration_ms,
            )
        });

        assert_eq!(keys[0], keys[1]);
        assert_eq!(keys[1], keys[2]);
    }

    #[test]
    fn only_durations_within_three_seconds_are_eligible() {
        assert!(duration_matches(Some(177.0), Some(180_000)));
        assert!(duration_matches(Some(183.0), Some(180_000)));
        assert!(!duration_matches(Some(176.999), Some(180_000)));
        assert!(!duration_matches(Some(184.0), Some(180_000)));
        assert!(!duration_matches(Some(215.0), Some(180_000)));
        assert!(!duration_matches(None, Some(180_000)));
        assert!(duration_matches(Some(215.0), None));
    }

    #[test]
    fn rank_matches_rejects_other_lengths_and_unrelated_songs() {
        let hits = vec![
            (
                candidate("Blinding Lights", "The Weeknd", 200.0),
                "original",
            ),
            (
                candidate("Blinding Lights (Remix)", "The Weeknd", 216.0),
                "long remix",
            ),
            (candidate("Other Song", "Other Artist", 200.0), "unrelated"),
        ];

        let ranked = rank_matches(hits, "Blinding Lights", "The Weeknd", Some(200_000));

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].1, "original");
    }

    #[test]
    fn rank_matches_prefers_the_exact_title_over_a_same_length_variant() {
        let hits = vec![
            (
                candidate("Blinding Lights (Remix)", "The Weeknd", 201.0),
                "remix",
            ),
            (
                candidate("Blinding Lights", "The Weeknd", 202.0),
                "original",
            ),
        ];

        let ranked = rank_matches(hits, "Blinding Lights", "The Weeknd", Some(200_000));

        assert_eq!(ranked[0].1, "original");
    }

    #[test]
    fn rank_matches_does_not_confuse_a_song_with_others_on_the_album_named_after_it() {
        let on_album = |title: &str, duration: f64| Candidate {
            album_name: Some("Back In Black".to_string()),
            ..candidate(title, "AC/DC", duration)
        };
        let hits = vec![
            (
                on_album("Rock and Roll Ain't Noise Pollution", 255.648),
                "other track",
            ),
            (on_album("Back In Black", 256.0), "the song"),
        ];

        let ranked = rank_matches(hits, "Back In Black", "AC/DC", Some(255_000));

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].1, "the song");
    }

    #[test]
    fn rank_matches_rejects_another_song_by_the_same_artist() {
        let hits = vec![(
            candidate("Dirty Deeds Done Dirt Cheap", "AC/DC", 253.0),
            "other song",
        )];

        assert!(rank_matches(hits, "Back In Black", "AC/DC", Some(253_000)).is_empty());
    }

    #[test]
    fn rank_matches_rejects_a_hit_without_a_title_that_only_shares_the_artist() {
        let untitled = Candidate {
            track_name: None,
            artist_name: Some("AC/DC".to_string()),
            album_name: None,
            duration: Some(256.0),
        };

        assert!(
            rank_matches(
                vec![(untitled, "untitled")],
                "Back In Black",
                "AC/DC",
                Some(256_000)
            )
            .is_empty()
        );
    }

    #[test]
    fn rank_matches_accepts_the_artist_alone_when_the_titles_use_different_scripts() {
        let hits = vec![(candidate("夜曲", "周杰伦", 226.0), "hit")];

        assert_eq!(
            rank_matches(hits, "Ye Qu", "周杰伦", Some(226_000)).len(),
            1
        );
    }

    #[test]
    fn rank_matches_accepts_a_hit_when_only_the_artist_script_differs() {
        let hits = vec![(candidate("夜曲", "周杰伦", 226.0), "hit")];

        assert_eq!(
            rank_matches(hits, "夜曲", "Jay Chou", Some(226_000)).len(),
            1
        );
    }

    fn lrc_red_hit(isrc: &str, title: &str) -> LrcRedHit {
        LrcRedHit {
            isrc: isrc.to_string(),
            title: Some(title.to_string()),
            artist: None,
            album: None,
            duration: None,
        }
    }

    #[test]
    fn lrc_red_hits_of_both_queries_are_merged_without_repeats() {
        let merged = merge_lrc_red_hits(
            vec![lrc_red_hit("A", "Song"), lrc_red_hit("B", "Song (Live)")],
            vec![lrc_red_hit("C", "Other"), lrc_red_hit("A", "Song")],
        );

        let isrcs = merged
            .iter()
            .map(|hit| hit.isrc.as_str())
            .collect::<Vec<_>>();
        assert_eq!(isrcs, ["A", "B", "C"]);
    }

    #[test]
    fn primary_artist_is_the_first_credited_artist() {
        assert_eq!(
            primary_artist("Gorillaz & Del the Funky Homosapien"),
            Some("Gorillaz")
        );
        assert_eq!(primary_artist("Drake, Future"), Some("Drake"));
        assert_eq!(primary_artist("Ravyn Lenae feat. Rex"), Some("Ravyn Lenae"));
        assert_eq!(primary_artist("Gorillaz"), None);
        assert_eq!(primary_artist("Simon&Garfunkel"), None);
    }

    fn run<T>(future: impl Future<Output = T>) -> T {
        tauri::async_runtime::block_on(future)
    }

    #[test]
    fn a_provider_that_stalls_ends_in_a_timeout_error_not_a_miss() {
        let stalled = within_deadline("lrc.red", Duration::from_millis(30), async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(None)
        });
        assert_eq!(run(stalled).err().as_deref(), Some("lrc.red: timed out"));

        let quick = within_deadline("lrc.red", Duration::from_secs(5), async { Ok(None) });
        assert!(matches!(run(quick), Ok(None)));
    }

    #[test]
    fn the_deadline_covers_every_request_a_lookup_makes() {
        // Three sequential 20 ms "requests" cannot fit in 30 ms in total.
        let slow = within_deadline("lrc.red", Duration::from_millis(30), async {
            for _ in 0..3 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok(None)
        });
        assert!(run(slow).is_err());
    }

    fn ranked(keys: &[u32]) -> Vec<(Candidate, u32)> {
        keys.iter()
            .map(|key| (candidate("Song", "Artist", 200.0), *key))
            .collect()
    }

    #[test]
    fn a_failing_candidate_does_not_stop_the_next_one() {
        let found = run(first_found(ranked(&[1, 2, 3]), |key| async move {
            match key {
                1 => Err("lrc.red lyrics: 500 Internal Server Error".to_string()),
                2 => Ok(Some("lyrics")),
                _ => panic!("candidate {key} must not be tried after a hit"),
            }
        }));
        assert_eq!(
            found.unwrap().map(|(_, key, lyrics)| (key, lyrics)),
            Some((2, "lyrics"))
        );
    }

    #[test]
    fn a_candidate_error_surfaces_only_when_no_candidate_has_lyrics() {
        let failed = run(first_found(ranked(&[1, 2]), |key| async move {
            if key == 1 {
                Err::<Option<&str>, _>("500".to_string())
            } else {
                Ok(None)
            }
        }));
        assert_eq!(failed.map(|found| found.is_some()), Err("500".to_string()));

        let missing = run(first_found(ranked(&[1, 2]), |_| async {
            Ok::<Option<&str>, String>(None)
        }));
        assert_eq!(missing.map(|found| found.is_some()), Ok(false));
    }

    /// The start of what `POST /s/AUDJ02102297/sync` answered.
    const SYNCED_SONG: &str = r#"{"id":"AUDJ02102297","lyrics":{"lines":[
        {"words":[[{"text":"Some","begin":15.816,"end":16.345},{"text":"one","begin":16.345,"end":16.776}],
            [{"text":"said","begin":16.776,"end":17.296}],
            [{"text":"they","begin":17.296,"end":17.641}],
            [{"text":"left","begin":17.641,"end":18.063}],
            [{"text":"to","begin":18.063,"end":18.38},{"text":"geth","begin":18.38,"end":18.936},{"text":"er","begin":18.936,"end":19.662}]],
         "text":"Someone said they left together","timed":true},
        {"words":[[{"text":"I","begin":19.675,"end":20.129}],
            [{"text":"ran","begin":20.129,"end":20.599}],
            [{"text":"her","begin":22.695,"end":22.715}]],
         "text":"I ran her","timed":true},
        {"words":[],"text":"An untimed line","timed":false}]}}"#;

    #[test]
    fn a_synced_song_becomes_lines_of_timed_words() {
        let song = serde_json::from_str::<LrcRedSong>(SYNCED_SONG).unwrap();

        let lyrics = song_to_lyrics(&song).unwrap();

        assert!(lyrics.is_word_timed());
        // The untimed line is left out.
        assert_eq!(lyrics.lines.len(), 2);
        let first = &lyrics.lines[0];
        assert_eq!((first.start_ms, first.end_ms), (15_816, 19_662));
        let words = first
            .text
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(words, ["Someone ", "said ", "they ", "left ", "together"]);
        assert_eq!(
            first.text.segments[0],
            Segment {
                start_ms: 15_816,
                end_ms: 16_776,
                text: "Someone ".to_string(),
            }
        );
    }

    #[test]
    fn a_synced_song_without_timed_lines_has_no_lyrics() {
        let untimed =
            serde_json::from_str::<LrcRedSong>(r#"{"lyrics":{"lines":[{"words":[]}]}}"#).unwrap();
        assert!(song_to_lyrics(&untimed).is_none());

        let empty = serde_json::from_str::<LrcRedSong>("{}").unwrap();
        assert!(song_to_lyrics(&empty).is_none());
    }

    #[test]
    fn the_result_tells_whether_words_are_timed() {
        let ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="1" end="2">Line only</p></div></body></tt>"#;
        let result = build_result(
            candidate("Song", "Artist", 200.4),
            ttml::parse(ttml).unwrap(),
        );

        assert!(!result.word_timed);
        assert_eq!(result.duration, Some(200));
        assert_eq!(result.track_name, "Song");
    }

    #[test]
    #[ignore = "curriculum bench; run via bun run bench:rust"]
    fn curriculum_metric_lrc_red_rank_candidates() {
        let mut hits = (0..60)
            .map(|index| {
                (
                    candidate(
                        &format!("Different Song {index}"),
                        &format!("Different Artist {index}"),
                        181.0 + f64::from(index) * 0.01,
                    ),
                    index,
                )
            })
            .collect::<Vec<_>>();
        hits.push((candidate("Self Aware", "Temper City", 181.0), 60));
        hits.push((
            candidate("Temper City - Self Aware", "DanceHype", 181.0),
            61,
        ));

        crate::bench_support::report_ops_per_sec("lrc_red_rank_candidates", || {
            let ranked = rank_matches(hits.clone(), "Self Aware", "Temper City", Some(181_000));
            assert_eq!(ranked.len(), 2);
        });
    }
}
