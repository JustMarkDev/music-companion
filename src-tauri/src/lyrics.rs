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

/// True when two normalized titles have the same words in another order, as
/// "Song (Edit) (feat. X)" and "Song (feat. X) [Edit]" do.
fn same_words(left: &str, right: &str) -> bool {
    fn sorted(value: &str) -> Vec<&str> {
        let mut words = value.split_whitespace().collect::<Vec<_>>();
        words.sort_unstable();
        words
    }
    sorted(left) == sorted(right)
}

fn score(value: Option<&str>, expected: &str) -> u8 {
    if expected.is_empty() {
        return 0;
    }

    let Some(value) = value else {
        return 0;
    };

    let value = normalize(value);
    if value == expected || same_words(&value, expected) {
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
    // lrc.red credits some artists in another script ("Paolo Conte" is
    // "帕羅康提"), which says nothing against the playing artist. It only
    // counts for a hit whose title matches, or any song in that script would
    // pass for the playing one.
    let other_script = title_score > 0
        && !normalized_artist.is_empty()
        && candidate
            .artist_name
            .as_deref()
            .is_some_and(has_non_latin_letters)
        && !has_non_latin_letters(normalized_artist);
    let artist_score = [
        score(candidate.artist_name.as_deref(), normalized_artist),
        score(candidate.track_name.as_deref(), normalized_artist),
        score(candidate.album_name.as_deref(), normalized_artist),
        if other_script { 2 } else { 0 },
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
) -> (std::cmp::Reverse<bool>, std::cmp::Reverse<(u8, u8)>, u64) {
    let (title_score, artist_score) =
        metadata_scores(candidate, normalized_title, normalized_artist);
    let metadata_matches = title_score > 0 && artist_score > 0;

    (
        std::cmp::Reverse(metadata_matches),
        // The title decides before the artist: "Via con me" by "帕羅康提" is the
        // song, "Via con me (Live)" by "Paolo Conte" another recording of it.
        std::cmp::Reverse((title_score, artist_score)),
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

/// True when a hit plausibly is the playing song: its title matches, or the
/// artist does and the titles are written in different scripts. The second case
/// needs `anchored`, a length that tells the hit apart from the artist's other
/// songs; without one, any song of the artist in another script would pass.
fn is_same_song(
    candidate: &Candidate,
    title: &str,
    normalized_title: &str,
    normalized_artist: &str,
    anchored: bool,
) -> bool {
    let (title_score, artist_score) =
        metadata_scores(candidate, normalized_title, normalized_artist);
    title_score > 0
        || (anchored
            && artist_score > 0
            && titles_differ_in_script(candidate.track_name.as_deref(), title))
}

/// Keeps the hits that plausibly are the playing song, best first. Search
/// results are fuzzy (remixes, covers, other artists), so a hit needs a
/// matching length and a matching title. The same song can be credited to an
/// artist written in another script, so the artist need not match; a title may
/// differ only when it is written in another script and the length is known,
/// never to pass off another song by the same artist.
fn rank_matches<T>(
    mut hits: Vec<(Candidate, T)>,
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
) -> Vec<(Candidate, T)> {
    let normalized_artist = normalize(artist);
    let normalized_title = canonical_title(title, &normalized_artist);
    hits.retain(|(candidate, _)| {
        duration_matches(candidate.duration, duration_ms)
            && is_same_song(
                candidate,
                title,
                &normalized_title,
                &normalized_artist,
                duration_ms.is_some(),
            )
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

/// Like `rank_matches` for an edit of a song (a TV size or short version): the
/// recording lrc.red has is the full one, so it may be any length from the
/// edit's up, the closest first.
///
/// A length that only has to be at least the edit's cannot tell the song from
/// the artist's others, so a title in another script is taken only from the
/// first hit, the one lrc.red itself ranks highest. `hits` come in that order.
fn rank_edit_matches<T>(
    hits: Vec<(Candidate, T)>,
    title: &str,
    artist: &str,
    edit_ms: u64,
) -> Vec<(Candidate, T)> {
    // Room for the rounding of durations; a recording shorter than the edit is not its full version.
    const LENGTH_TOLERANCE_MS: u64 = 1_000;
    let normalized_artist = normalize(artist);
    let normalized_title = canonical_title(title, &normalized_artist);
    let mut hits = hits
        .into_iter()
        .enumerate()
        .filter(|(index, (candidate, _))| {
            candidate
                .duration
                .filter(|seconds| seconds.is_finite())
                .is_some_and(|seconds| (seconds * 1_000.0) as u64 + LENGTH_TOLERANCE_MS >= edit_ms)
                && is_same_song(
                    candidate,
                    title,
                    &normalized_title,
                    &normalized_artist,
                    *index == 0,
                )
        })
        .map(|(_, hit)| hit)
        .collect::<Vec<_>>();
    hits.sort_by_key(|(candidate, _)| {
        ranking_key(
            candidate,
            &normalized_title,
            &normalized_artist,
            Some(edit_ms),
        )
    });
    hits
}

/// The title without a trailing marker of a shortened edit ("(TV Size)",
/// "<TV. Size Version>", "- Short Ver."), or `None` when it has none.
fn strip_edit_marker(title: &str) -> Option<String> {
    let title = title.trim_end();
    let is_marker = |label: &str| {
        let label = normalize(label);
        [
            "tv size",
            "tv ver",
            "tv edit",
            "tv cut",
            "short ver",
            "short size",
            "short edit",
            "short cut",
            "anime size",
            "anime ver",
            "anime edit",
        ]
        .iter()
        .any(|marker| label.contains(marker))
    };

    let grouped = title
        .chars()
        .last()
        .and_then(|closing| match closing {
            ')' => title.rfind('('),
            ']' => title.rfind('['),
            '>' => title.rfind('<'),
            _ => None,
        })
        .filter(|open| is_marker(&title[open + 1..title.len() - 1]))
        .map(|open| title[..open].trim_end());
    // The rightmost dash, so a dash inside the title is kept whatever kind it is.
    let dashed = [" - ", " \u{2013} ", " \u{2014} "]
        .iter()
        .filter_map(|dash| title.rsplit_once(dash))
        .filter(|(_, label)| is_marker(label))
        .max_by_key(|(rest, _)| rest.len())
        .map(|(rest, _)| rest.trim_end());
    grouped
        .or(dashed)
        .filter(|stripped| !stripped.is_empty())
        .map(str::to_string)
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
    let mut found = search_lrc_red(client, title, artist, duration_ms).await;
    if matches!(found, Ok(None))
        && let Some(primary) = primary_artist(artist)
    {
        found = search_lrc_red(client, title, primary, duration_ms).await;
    }
    // A TV size or short version is not on lrc.red, but its full version is.
    if matches!(found, Ok(None))
        && let (Some(stripped), Some(edit_ms)) = (strip_edit_marker(title), duration_ms)
    {
        found = search_lrc_red_edit(client, &stripped, artist, edit_ms).await;
    }
    found
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

/// The free text `/search.json` is asked. It finds nothing for a word with an
/// apostrophe ("L'orchestrina") but does for the words around it.
fn search_query(title: &str, artist: &str) -> String {
    format!("{title} {artist}").replace(['\'', '’'], " ")
}

/// One `/search.json` query: the hits for a title and artist written as free
/// text. It finds songs `/match.json` cannot when lrc.red credits the artist
/// differently (IRIS OUT is by "米津玄师", not "Kenshi Yonezu").
async fn lrc_red_text_hits(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> Result<Vec<LrcRedHit>, String> {
    let response = client
        .get(format!(
            "https://lrc.red/search.json?q={}",
            urlencoding::encode(&search_query(title, artist))
        ))
        .send()
        .await
        .map_err(|error| provider_error("lrc.red search", error))?;
    if !response.status().is_success() {
        return Err(provider_error("lrc.red search", response.status()));
    }
    let matches = response
        .json::<LrcRedMatches>()
        .await
        .map_err(|error| provider_error("lrc.red search", error))?;
    Ok(matches.hits)
}

/// The recordings lrc.red lists for a song that plausibly are it, best
/// first, each with its ISRC. `/match.json` is asked first; the text search,
/// the halves of a dual-script title, and the primary artist of a joined one
/// follow only while no hit is named exactly like the playing song, so a song
/// costs extra requests only while it would otherwise be missed or covered.
async fn lrc_red_matches(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
) -> Result<Vec<(Candidate, String)>, String> {
    let mut matched = lrc_red_match_candidates(client, title, artist, duration_ms).await?;
    if has_exact_title(&matched, title, artist, duration_ms) {
        return Ok(matched);
    }
    // `/match.json` only knows the live version of "Via con me"; the studio
    // one is credited to "帕羅康提" and found by the text search.
    match lrc_red_text_hits(client, title, artist).await {
        Ok(hits) => matched = merge_text_hits(matched, hits),
        Err(_) if !matched.is_empty() => return Ok(matched),
        Err(error) => return Err(error),
    }
    // A title in two scripts ("クスシキ - KUSUSHIKI") finds nothing as free
    // text, while either half alone names the song.
    if !has_exact_title(&matched, title, artist, duration_ms) && !title_halves(title).is_empty() {
        matched = merge_half_hits(client, matched, title, artist).await;
    }
    // A duet ("Kenshi Yonezu & Hikaru Utada") finds nothing as free text when
    // lrc.red credits one singer in another script ("米津玄師, Utada"); the
    // first singer alone finds the song.
    if !has_exact_title(&matched, title, artist, duration_ms)
        && let Some(primary) = primary_artist(artist)
        && let Ok(hits) = lrc_red_text_hits(client, title, primary).await
    {
        matched = merge_text_hits(matched, hits);
    }
    Ok(rank_matches(matched, title, artist, duration_ms))
}

/// Adds text-search hits to ranked candidates, skipping ISRCs already known.
/// Ranking happens later, once every fallback query has answered.
fn merge_text_hits(
    mut matched: Vec<(Candidate, String)>,
    hits: Vec<LrcRedHit>,
) -> Vec<(Candidate, String)> {
    let known = matched
        .iter()
        .map(|(_, isrc)| isrc.clone())
        .collect::<HashSet<_>>();
    matched.extend(
        hit_candidates(hits)
            .into_iter()
            .filter(|(_, isrc)| !known.contains(isrc)),
    );
    matched
}

/// Merges the text-search hits of each half of a dual-script title. The halves
/// are asked together; a failed half only loses its own hits.
async fn merge_half_hits(
    client: &reqwest::Client,
    mut matched: Vec<(Candidate, String)>,
    title: &str,
    artist: &str,
) -> Vec<(Candidate, String)> {
    let halves = title_halves(title);
    let Some(first) = halves.first() else {
        return matched;
    };
    let (first_hits, second_hits) = tokio::join!(lrc_red_text_hits(client, first, artist), async {
        match halves.get(1) {
            Some(second) => lrc_red_text_hits(client, second, artist).await,
            None => Ok(Vec::new()),
        }
    });
    for hits in [first_hits, second_hits].into_iter().flatten() {
        matched = merge_text_hits(matched, hits);
    }
    matched
}

/// The halves of a title naming a song twice ("クスシキ - KUSUSHIKI"), so each
/// half can be searched on its own. Only a dash with spaces around it splits,
/// so "KICK BACK -ANIME edit" stays whole.
fn title_halves(title: &str) -> Vec<String> {
    const DASHES: [&str; 3] = [" - ", " \u{2013} ", " \u{2014} "];
    // The rightmost dash, so a dash inside the title is kept whatever kind it is.
    DASHES
        .iter()
        .filter_map(|dash| title.rsplit_once(dash))
        .max_by_key(|(rest, _)| rest.len())
        .map(|(first, second)| vec![first.trim().to_string(), second.trim().to_string()])
        .unwrap_or_default()
        .into_iter()
        .filter(|half| !half.is_empty())
        .collect()
}

/// True when some hit is named exactly like the playing song, is not
/// credited to another artist, and matches the known length. A wrong-length
/// exact hit must not suppress the fallbacks: `rank_matches` would filter it
/// out afterwards and the lookup would miss.
fn has_exact_title(
    hits: &[(Candidate, String)],
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
) -> bool {
    let normalized_artist = normalize(artist);
    let normalized_title = canonical_title(title, &normalized_artist);
    hits.iter().any(|(candidate, _)| {
        duration_matches(candidate.duration, duration_ms)
            && track_title_score(candidate, &normalized_title, &normalized_artist) == 4
            && metadata_scores(candidate, &normalized_title, &normalized_artist).1 > 0
    })
}

/// The recordings `/match.json` lists for a song that plausibly are it.
///
/// `/match.json` weighs the duration above the title, so with one it lists
/// other songs of about that length and can leave out the song itself. The
/// query without a duration finds the song by its name, the one with a
/// duration finds its variants of the right length; both are ranked here.
async fn lrc_red_match_candidates(
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
            // A single failed query must not become a cached miss: when the
            // surviving query has nothing, the failure is returned so the
            // lookup is retried later instead of sticking until the next track.
            let (hits, failure) = match (by_title, by_duration) {
                (Ok(by_title), Ok(by_duration)) => {
                    (merge_lrc_red_hits(by_title, by_duration), None)
                }
                (Ok(hits), Err(error)) | (Err(error), Ok(hits)) => (hits, Some(error)),
                (Err(error), Err(_)) => return Err(error),
            };
            let ranked = rank_matches(hit_candidates(hits), title, artist, Some(duration_ms));
            if ranked.is_empty()
                && let Some(error) = failure
            {
                return Err(error);
            }
            return Ok(ranked);
        }
    };
    Ok(rank_matches(
        hit_candidates(hits),
        title,
        artist,
        duration_ms,
    ))
}

fn hit_candidates(hits: Vec<LrcRedHit>) -> Vec<(Candidate, String)> {
    hits.into_iter()
        .map(|hit| {
            let candidate = Candidate {
                track_name: hit.title,
                artist_name: hit.artist,
                album_name: hit.album,
                duration: hit.duration,
            };
            (candidate, hit.isrc)
        })
        .collect()
}

/// `/match.json` finds the recording, `/s/{isrc}.ttml` is its lyrics.
async fn search_lrc_red(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
) -> Lookup {
    // A hit can lack a lyrics file, so fall through to the next best one.
    let ranked = lrc_red_matches(client, title, artist, duration_ms)
        .await?
        .into_iter()
        .take(3)
        .collect();
    first_lyrics(client, ranked).await
}

/// Searches for the full version of an edit by its title without the marker.
/// Only the best hit is used: its length cannot confirm the song, so a second
/// guess would be a worse one.
async fn search_lrc_red_edit(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    edit_ms: u64,
) -> Lookup {
    let hits = lrc_red_hits(client, title, artist, None).await?;
    let mut ranked = rank_edit_matches(hit_candidates(hits), title, artist, edit_ms);
    if ranked.is_empty() {
        // As for any song, the text search finds what `/match.json` cannot.
        let hits = lrc_red_text_hits(client, title, artist).await?;
        ranked = rank_edit_matches(hit_candidates(hits), title, artist, edit_ms);
    }
    first_lyrics(client, ranked.into_iter().take(1).collect()).await
}

/// The lyrics of the first ranked recording that has any.
async fn first_lyrics(client: &reqwest::Client, ranked: Vec<(Candidate, String)>) -> Lookup {
    let started_at = std::time::Instant::now();
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
///
/// The answer has no transliteration or translation, and neither has the TTML
/// lrc.red serves for the recording afterwards (a cached copy of the old one
/// for five minutes), so the caller keeps those from the lyrics it had.
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
    Ok(Some(build_result(candidate, synced)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_studio_recording_credited_in_another_script_beats_a_live_one() {
        let hits = vec![
            (candidate("Via con me (Live)", "Paolo Conte", 168.3), 1),
            (candidate("Via Con Me", "帕羅康提", 166.5), 2),
            (candidate("Via Con Me", "Fiorello", 237.2), 3),
        ];

        let ranked = rank_matches(hits, "Via con me", "Paolo Conte", Some(166_500));

        assert_eq!(ranked.iter().map(|(_, id)| *id).collect::<Vec<_>>(), [2, 1]);
    }

    #[test]
    fn another_script_credit_only_vouches_for_a_matching_title() {
        let hits = vec![
            (candidate("感電", "周杰伦", 166.0), 1),
            (candidate("Via Con Me", "帕羅康提", 166.0), 2),
        ];

        let ranked = rank_matches(hits, "Via con me", "Paolo Conte", Some(166_500));
        assert_eq!(ranked.iter().map(|(_, id)| *id).collect::<Vec<_>>(), [2]);

        // Without a playing artist, no credit is preferred over another.
        let scores = |artist| metadata_scores(&candidate("Song", artist, 100.0), "song", "");
        assert_eq!(scores("アーティスト"), scores("Artist"));
    }

    #[test]
    fn a_cover_by_another_artist_is_not_an_exact_title_hit() {
        let cover = vec![(candidate("Via Con Me", "Fiorello", 166.2), "a".to_string())];
        let studio = vec![(candidate("Via Con Me", "帕羅康提", 166.5), "b".to_string())];

        assert!(!has_exact_title(&cover, "Via con me", "Paolo Conte", None));
        assert!(has_exact_title(&studio, "Via con me", "Paolo Conte", None));
    }

    #[test]
    fn a_wrong_length_exact_hit_does_not_suppress_the_fallbacks() {
        let hit = vec![(
            candidate("JANE DOE", "Kenshi Yonezu", 180.0),
            "cover".to_string(),
        )];

        assert!(has_exact_title(&hit, "JANE DOE", "Kenshi Yonezu", None));
        assert!(!has_exact_title(
            &hit,
            "JANE DOE",
            "Kenshi Yonezu",
            Some(236_000),
        ));
    }

    #[test]
    fn the_text_search_has_no_apostrophes() {
        assert_eq!(
            search_query("L'orchestrina", "Paolo Conte"),
            "L orchestrina Paolo Conte"
        );
        assert_eq!(search_query("Don’t Stop", "Journey"), "Don t Stop Journey");
    }

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

    #[test]
    fn rank_matches_does_not_pass_off_another_song_in_another_script_without_a_length() {
        // "IRIS OUT" must not get the lyrics of 感電 just because Kenshi Yonezu sings both.
        let hits = vec![(candidate("感電", "Kenshi Yonezu", 264.5), "other song")];

        assert!(rank_matches(hits.clone(), "IRIS OUT", "Kenshi Yonezu", None).is_empty());
        // Titles of one script still match without a length.
        let same = vec![(candidate("IRIS OUT", "Kenshi Yonezu", 153.0), "song")];
        assert_eq!(
            rank_matches(same, "IRIS OUT", "Kenshi Yonezu", None).len(),
            1
        );
    }

    #[test]
    fn a_title_with_its_credits_in_another_order_is_the_same_song() {
        // The player says "(Edit) (feat. X)", lrc.red "(feat. X) [Edit]".
        let hits = vec![
            (
                candidate(
                    "Just the Two of Us (feat. Bill Withers) [Edit]",
                    "Grover Washington, Jr.",
                    237.493,
                ),
                "edit",
            ),
            (
                candidate("Just the Two of Us", "Grover Washington, Jr.", 443.773),
                "album version",
            ),
        ];

        let playing = "Just the Two of Us (Edit) (feat. Bill Withers)";
        let known = rank_matches(
            hits.clone(),
            playing,
            "Grover Washington, Jr.",
            Some(237_381),
        );
        assert_eq!(known.iter().map(|hit| hit.1).collect::<Vec<_>>(), ["edit"]);

        // Without a length, the exact edit is still preferred to the album version.
        let unknown = rank_matches(hits, playing, "Grover Washington, Jr.", None);
        assert_eq!(unknown[0].1, "edit");
    }

    #[test]
    fn the_marker_of_a_shortened_edit_is_stripped_from_its_title() {
        let stripped = |title: &str| strip_edit_marker(title);
        let expected = Some("The Cruel Angel's Thesis".to_string());
        assert_eq!(
            stripped("The Cruel Angel's Thesis <TV. Size Version>"),
            expected
        );
        assert_eq!(stripped("The Cruel Angel's Thesis (TV Size)"), expected);
        assert_eq!(stripped("The Cruel Angel's Thesis [Short Ver.]"), expected);
        assert_eq!(stripped("The Cruel Angel's Thesis - TV Size"), expected);
        assert_eq!(stripped("The Cruel Angel's Thesis – TV Size"), expected);
        assert_eq!(stripped("The Cruel Angel's Thesis — TV Size"), expected);
        // A dash of another kind inside the title stays in it.
        assert_eq!(
            stripped("My Song - Remix – TV Size"),
            Some("My Song - Remix".to_string())
        );
        assert_eq!(stripped("The Cruel Angel's Thesis"), None);
        assert_eq!(stripped("Song (Live)"), None);
        assert_eq!(stripped("(TV Size)"), None);
    }

    #[test]
    fn a_song_credited_to_the_artist_in_another_script_is_ranked_above_its_covers() {
        // What `/search.json?q=IRIS OUT Kenshi Yonezu` lists, with the song first.
        let hits = vec![
            (candidate("IRIS OUT", "米津玄师", 151.573), "official"),
            (candidate("IRIS OUT", "Trickle", 146.694), "other length"),
            (
                candidate("IRIS OUT (Reze ver.cover)", "CODE:D 6TH", 153.205),
                "cover",
            ),
            (
                candidate("Out of Control", "LEE GI KWANG", 203.307),
                "unrelated",
            ),
        ];

        let ranked = rank_matches(hits, "IRIS OUT", "Kenshi Yonezu", Some(153_181));

        let order = ranked.iter().map(|(_, id)| *id).collect::<Vec<_>>();
        assert_eq!(order, ["official", "cover"]);
    }

    #[test]
    fn an_edit_is_matched_with_a_full_version_that_is_at_least_as_long() {
        let full = candidate("残酷な天使のテーゼ", "Yoko Takahashi", 245.9);
        let shorter = candidate("残酷な天使のテーゼ", "Yoko Takahashi", 60.0);
        let other_artist = candidate("残酷な天使のテーゼ", "Someone Else", 245.9);
        // lrc.red lists the song it ranks highest first.
        let hits = vec![
            (full, "full"),
            (shorter, "shorter"),
            (other_artist, "other"),
        ];

        let ranked = rank_edit_matches(hits, "The Cruel Angel's Thesis", "Yoko Takahashi", 93_200);

        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].1, "full");
    }

    #[test]
    fn an_edit_does_not_take_another_song_of_the_artist_in_another_script() {
        // "IRIS OUT (TV Size)": lrc.red's own best hit is another song, and 感電 is longer.
        let hits = vec![
            (candidate("KICK BACK", "Kenshi Yonezu", 193.5), "kick back"),
            (candidate("感電", "Kenshi Yonezu", 264.5), "another song"),
        ];

        assert!(rank_edit_matches(hits, "IRIS OUT", "Kenshi Yonezu", 93_000).is_empty());
    }

    #[test]
    fn an_edit_with_the_same_title_is_matched_wherever_lrc_red_lists_it() {
        let hits = vec![
            (candidate("Different Tune", "Artist", 200.0), "other"),
            (candidate("Song", "Artist", 200.0), "full"),
        ];

        let ranked = rank_edit_matches(hits, "Song", "Artist", 90_000);

        assert_eq!(ranked.iter().map(|hit| hit.1).collect::<Vec<_>>(), ["full"]);
    }

    #[test]
    fn an_edit_prefers_the_full_version_to_a_recording_shorter_than_itself() {
        let hits = vec![
            (candidate("Song", "Artist", 91.0), "slightly shorter"),
            (candidate("Song", "Artist", 245.9), "full"),
        ];

        let ranked = rank_edit_matches(hits, "Song", "Artist", 93_200);

        assert_eq!(ranked.iter().map(|hit| hit.1).collect::<Vec<_>>(), ["full"]);
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

    #[test]
    fn primary_artist_of_a_duet_is_the_first_singer() {
        assert_eq!(
            primary_artist("Kenshi Yonezu & Hikaru Utada"),
            Some("Kenshi Yonezu")
        );
    }

    #[test]
    fn a_dual_script_title_splits_into_searchable_halves() {
        assert_eq!(
            title_halves("クスシキ - KUSUSHIKI"),
            ["クスシキ", "KUSUSHIKI"]
        );
        assert_eq!(
            title_halves("革命道中 – On The Way"),
            ["革命道中", "On The Way"]
        );
        assert_eq!(
            title_halves("革命道中 — On The Way"),
            ["革命道中", "On The Way"]
        );
        // A dash without spaces around it is part of the title.
        assert!(title_halves("KICK BACK -ANIME edit").is_empty());
        assert!(title_halves("JANE DOE").is_empty());
        // An empty half is dropped, the other still searches.
        assert_eq!(title_halves(" - KUSUSHIKI"), ["KUSUSHIKI"]);
        // A repeated dash splits at the rightmost one.
        assert_eq!(title_halves("A - B - C"), ["A - B", "C"]);
    }

    #[test]
    fn merged_text_hits_skip_recordings_already_known() {
        let matched = vec![(candidate("Song", "Artist", 200.0), "A".to_string())];
        let hits = vec![lrc_red_hit("A", "Song"), lrc_red_hit("B", "Song (Live)")];

        let merged = merge_text_hits(matched, hits);

        let isrcs = merged
            .iter()
            .map(|(_, isrc)| isrc.as_str())
            .collect::<Vec<_>>();
        assert_eq!(isrcs, ["A", "B"]);
    }

    #[test]
    fn the_official_duet_beats_a_cover_the_joined_search_misses() {
        // What the merged fallbacks list for "JANE DOE" by
        // "Kenshi Yonezu & Hikaru Utada": the cover from the joined-artist
        // queries, whose file holds a few placeholder lines, and the official
        // recording from the primary-artist text search, credited to
        // "米津玄師, Utada".
        let cover = candidate("JANE DOE", "vally.exe", 234.44);

        // The cover alone never counts as exact: its artist says nothing
        // about the duet, so the fallbacks run.
        assert!(!has_exact_title(
            &[(cover.clone(), "cover".to_string())],
            "JANE DOE",
            "Kenshi Yonezu & Hikaru Utada",
            Some(236_000),
        ));

        let ranked = rank_matches(
            vec![
                (cover, "cover".to_string()),
                (
                    candidate("JANE DOE", "米津玄師, Utada", 235.947),
                    "official".to_string(),
                ),
            ],
            "JANE DOE",
            "Kenshi Yonezu & Hikaru Utada",
            Some(236_000),
        );

        assert_eq!(
            ranked.iter().map(|(_, id)| id.as_str()).collect::<Vec<_>>(),
            ["official", "cover"]
        );
    }

    #[test]
    fn a_dual_script_title_prefers_the_half_matching_recording() {
        // What the half query "KUSUSHIKI Mrs. GREEN APPLE" lists: the official
        // recording and a cover. The full title only ever scores a partial
        // match, so the half fallbacks run.
        let hits = vec![
            (
                candidate("KUSUSHIKI", "Mrs. GREEN APPLE", 189.427),
                "official",
            ),
            (candidate("Kusushiki", "Olwen Mari", 91.698), "cover"),
            (
                candidate("Inferno", "Mrs. GREEN APPLE", 212.547),
                "other song",
            ),
        ];
        assert!(!has_exact_title(
            &hits
                .iter()
                .map(|(candidate, id)| (candidate.clone(), id.to_string()))
                .collect::<Vec<_>>(),
            "クスシキ - KUSUSHIKI",
            "Mrs. GREEN APPLE",
            Some(189_000),
        ));

        let ranked = rank_matches(
            hits,
            "クスシキ - KUSUSHIKI",
            "Mrs. GREEN APPLE",
            Some(189_000),
        );

        assert_eq!(
            ranked.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
            ["official"]
        );
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
