//! Lyrics lookup. lrc.red is the main provider: Apple Music lyrics, many with
//! word timing, a transliteration and a translation, served as TTML. LRCLIB is
//! asked only when lrc.red has nothing, for line-timed lyrics of the same recording.

use crate::LATEST_LYRICS_REQUEST;
use crate::ttml::{self, Line, LineText, Lyrics, Segment};
use reqwest::Url;
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
/// A song lrc.red lacks walks through up to a dozen fallback queries one after
/// another, and an error there never reaches LRCLIB, so lrc.red gets more time.
const LRC_RED_TIMEOUT: Duration = Duration::from_secs(12);
const LRCLIB_TIMEOUT: Duration = Duration::from_secs(6);

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

/// Looks the song up on lrc.red, then on LRCLIB when lrc.red has nothing. The
/// lookup is dropped (cancelled) as soon as a newer request supersedes it.
pub async fn fetch_lyrics(
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
    request_id: u64,
) -> Lookup {
    let client = http_client()?;
    let mut lookup = std::pin::pin!(async {
        let found = within_deadline(
            "lrc.red",
            LRC_RED_TIMEOUT,
            fetch_lrc_red(client, title, artist, duration_ms),
        )
        .await;
        let lrclib = || {
            within_deadline(
                "LRCLIB",
                LRCLIB_TIMEOUT,
                fetch_lrclib(client, title, artist, duration_ms),
            )
        };
        match found {
            // Only a clean miss moves on: an error is retried against lrc.red.
            Ok(None) => lrclib().await,
            // lrc.red lists the song under an artist that cannot be told from the
            // playing one (another script, another band's cover of a title).
            // LRCLIB lists artists the way players do, so it is asked too.
            Ok(Some(lyrics)) if !confirms_song(&lyrics, title, artist) => {
                let other = lrclib().await.ok().flatten();
                Ok(Some(match other {
                    Some(other) if lrclib_beats(&lyrics, &other, title, artist) => other,
                    _ => lyrics,
                }))
            }
            found => found,
        }
    });
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

/// True when the lyrics' own title and artist credit are the playing song's, as
/// it is named or as a channel put it into one title ("Artist - Song"), so it is
/// known to be that song and not one that only has its length, or its title and
/// another artist, or its artist and a title in another script.
fn confirms_song(lyrics: &LyricsResult, title: &str, artist: &str) -> bool {
    let candidate = Candidate {
        track_name: Some(lyrics.track_name.clone()),
        artist_name: Some(lyrics.artist_name.clone()),
        album_name: None,
        duration: None,
    };
    let mut asked = vec![(title.to_string(), artist.to_string())];
    asked.extend(dash_title_queries(title));
    asked.extend(
        asked
            .clone()
            .into_iter()
            .flat_map(|(title, artist)| {
                [
                    base_title(&title).map(|base| (base, artist.clone())),
                    strip_edit_marker(&title).map(|full| (full, artist.clone())),
                    primary_artist(&artist).map(|primary| (title.clone(), primary.to_string())),
                ]
            })
            .flatten(),
    );
    asked.into_iter().any(|(title, artist)| {
        let artist = normalize(&artist);
        let title = canonical_title(&title, &artist);
        !artist.is_empty()
            && track_title_score(&candidate, &title, &artist) == 4
            && !artist_unverified(&candidate, &artist)
    })
}

/// The text of the lyrics as the singer says it, for comparing two providers'.
fn lyrics_text(lyrics: &LyricsResult) -> String {
    lyrics
        .lines
        .iter()
        .map(|line| {
            line.text
                .segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How much of two lyrics' character pairs they share, 0 to 1.
fn text_overlap(left: &str, right: &str) -> f64 {
    fn pairs(text: &str) -> HashSet<(char, char)> {
        let chars = normalize(text).chars().collect::<Vec<_>>();
        chars.windows(2).map(|pair| (pair[0], pair[1])).collect()
    }
    let (left, right) = (pairs(left), pairs(right));
    let union = left.union(&right).count();
    if union == 0 {
        return 0.0;
    }
    left.intersection(&right).count() as f64 / union as f64
}

/// Below this, two lyrics in the same script are not of the same song.
const SAME_SONG_OVERLAP: f64 = 0.25;

/// Whether LRCLIB's lyrics are to be shown where lrc.red's credit does not
/// confirm the playing song: when LRCLIB has the song under the playing artist
/// and title with other words, lrc.red's is a song that only shares a name. Lyrics in
/// different scripts (the Japanese and its romaji) cannot be compared.
fn lrclib_beats(lrc_red: &LyricsResult, lrclib: &LyricsResult, title: &str, artist: &str) -> bool {
    let (red_text, lib_text) = (lyrics_text(lrc_red), lyrics_text(lrclib));
    confirms_song(lrclib, title, artist)
        && has_non_latin_letters(&red_text) == has_non_latin_letters(&lib_text)
        && text_overlap(&red_text, &lib_text) < SAME_SONG_OVERLAP
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

/// The base letter of an accented Latin letter, so a title typed with accents
/// ("Più", "Felicità") equals the same one without or with the accent written as
/// a letter plus a combining mark, as macOS files have it.
fn unaccented(char: char) -> char {
    match char {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'ĉ' | 'č' => 'c',
        'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' => 'g',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' => 'i',
        'ĺ' | 'ľ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' => 's',
        'ţ' | 'ť' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ý' | 'ÿ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

/// The kana a plain kana and a combining (semi-)voiced mark make together: か
/// and U+3099 is が, は and U+309A is ぱ. macOS spells file names this way, and
/// the mark is not a letter, so it would otherwise vanish and leave か.
fn composed_kana(plain: char, mark: char) -> Option<char> {
    // The hiragana that have a voiced form right after them (か→が), and the は
    // row, which has a semi-voiced one after that (は→ば→ぱ).
    const VOICEABLE: [u32; 20] = [
        0x304B, 0x304D, 0x304F, 0x3051, 0x3053, 0x3055, 0x3057, 0x3059, 0x305B, 0x305D, 0x305F,
        0x3061, 0x3064, 0x3066, 0x3068, 0x306F, 0x3072, 0x3075, 0x3078, 0x307B,
    ];
    const HA_ROW: [u32; 5] = [0x306F, 0x3072, 0x3075, 0x3078, 0x307B];
    let code = plain as u32;
    let katakana = (0x30A1..=0x30F6).contains(&code);
    let base = if katakana { code - 0x60 } else { code };
    let composed = match mark {
        '\u{3099}' if base == 0x3046 => 0x3094,
        '\u{3099}' if VOICEABLE.contains(&base) => base + 1,
        '\u{309A}' if HA_ROW.contains(&base) => base + 2,
        _ => return None,
    };
    char::from_u32(if katakana { composed + 0x60 } else { composed })
}

/// A tag as the providers write it: kana and their combining (semi-)voiced marks
/// as the one kana they make (a player on macOS may report a title the decomposed
/// way), and full-width Latin letters, digits and punctuation as plain ASCII,
/// as Japanese tag editors type them. Neither provider matches the other forms.
fn clean_tag(value: &str) -> String {
    compose_kana(
        &value
            .chars()
            .map(|char| match char {
                '\u{3000}' => ' ',
                '\u{FF01}'..='\u{FF5E}' => {
                    char::from_u32(char as u32 - 0xFF01 + 0x21).unwrap_or(char)
                }
                other => other,
            })
            .collect::<String>(),
    )
}

/// A player's artist without the " - Topic" of the channel YouTube Music files
/// an artist's songs under.
fn clean_artist(value: &str) -> String {
    let artist = clean_tag(value);
    artist
        .strip_suffix(" - Topic")
        .map_or(artist.clone(), |name| name.trim().to_string())
}

/// The kana of `value` with each combining (semi-)voiced mark joined to its kana.
fn compose_kana(value: &str) -> String {
    let mut composed = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(char) = chars.next() {
        let char = match chars.peek().copied() {
            Some(mark @ ('\u{3099}' | '\u{309A}')) => match composed_kana(char, mark) {
                Some(joined) => {
                    chars.next();
                    joined
                }
                None => char,
            },
            _ => char,
        };
        composed.push(char);
    }
    composed
}

fn normalize(value: &str) -> String {
    clean_tag(value)
        .chars()
        .flat_map(|char| char.to_lowercase().map(unaccented))
        .collect::<String>()
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
    // Text that is all punctuation normalizes to "", which every string contains.
    if value.is_empty() {
        return 0;
    }
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

/// How far a hit may be from the playing length when only its artist vouches
/// for it, the title being written in another script. A title that says nothing
/// needs a closer length than one that matches.
const CROSS_SCRIPT_TOLERANCE_MS: u64 = 300;

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

/// The name a single is released under: "1% Battery Girl - Single" is the
/// single "1% Battery Girl".
fn single_name(album: &str) -> Option<&str> {
    const SUFFIX: &str = " - single";
    let album = album.trim_end();
    let name = album.get(..album.len().checked_sub(SUFFIX.len())?)?;
    album
        .get(name.len()..)
        .filter(|suffix| suffix.eq_ignore_ascii_case(SUFFIX))
        .map(|_| name.trim_end())
}

/// A normalized title without its feature credit: "song feat x" is "song".
fn without_credit(normalized_title: &str) -> String {
    let words = normalized_title.split_whitespace().collect::<Vec<_>>();
    let end = words
        .iter()
        .skip(1)
        .position(|word| matches!(*word, "feat" | "ft" | "featuring"))
        .map_or(words.len(), |index| index + 1);
    words[..end].join(" ")
}

/// 2 when the hit is the song's own track on a single named like it, else 0.
/// lrc.red files "1% Battery Girl (feat. ChiliChill)" as "1% Low Battery Girl
/// (feat. ChiliChill)" on the single "1% Battery Girl", so the track name alone
/// does not match.
///
/// A single holds other recordings too, each timed differently, so the track
/// must be the song and nothing else: its name holds the single's words, no
/// qualifier of its own ("(Instrumental)",
/// "- Remix", "(Live)"), and a feature credit when the playing title has one,
/// or the version without the feature would pass. The names are compared without
/// feature credits, which a player and a single's name write differently. Only a
/// single counts: every track of an album named like the song would tie with it.
/// The score stays below an exact track name's.
fn single_title_score(candidate: &Candidate, normalized_title: &str) -> u8 {
    let (Some(track), Some(single)) = (
        candidate.track_name.as_deref(),
        candidate.album_name.as_deref().and_then(single_name),
    ) else {
        return 0;
    };
    let single = without_credit(&normalize(single));
    let normalized_track = normalize(track);
    let has_credit = |normalized: &str| without_credit(normalized) != normalized;
    let bare_track = base_title(track).unwrap_or_else(|| track.to_string());
    // Every word of the single's name is in the track's: "1% Low Battery Girl"
    // is the song "1% Battery Girl", a B-side with a title of its own is not.
    // The single's name keeps its release labels ("(Remastered)"), so the words
    // are looked for in the whole track name, not in the one without labels.
    let names_the_song = single.split_whitespace().all(|word| {
        normalized_track
            .split_whitespace()
            .any(|other| other == word)
    });
    let is_the_song = !single.is_empty()
        && single == without_credit(normalized_title)
        && names_the_song
        && strip_trailing_label(&bare_track, |_| true).is_none()
        && (has_credit(&normalized_track) || !has_credit(normalized_title));
    if is_the_song { 2 } else { 0 }
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
        Some(_) => track_title_score(candidate, normalized_title, normalized_artist)
            .max(single_title_score(candidate, normalized_title)),
        None => score(candidate.album_name.as_deref(), normalized_title),
    };
    // lrc.red credits some artists in another script ("Paolo Conte" is
    // "帕羅康提"), and a player may credit one in another script than lrc.red
    // ("米津玄師" for "Kenshi Yonezu"). That says nothing against the playing
    // artist, but it only counts for a hit named exactly like the song: a title
    // that merely contains it ("Lemon·柠檬气息" by 花僮 for "Lemon") is another song.
    let other_script = title_score == 4
        && !normalized_artist.is_empty()
        && candidate.artist_name.as_deref().is_some_and(|credit| {
            has_non_latin_letters(credit) != has_non_latin_letters(normalized_artist)
        });
    let artist_score =
        named_artist_score(candidate, normalized_artist).max(if other_script { 2 } else { 0 });
    (title_score, artist_score)
}

/// How well the names a hit carries (its artist, and its title and album, where
/// a "(feat. X)" may name the artist) match the playing artist, 0-4.
fn named_artist_score(candidate: &Candidate, normalized_artist: &str) -> u8 {
    [
        score(candidate.artist_name.as_deref(), normalized_artist),
        score(candidate.track_name.as_deref(), normalized_artist),
        score(candidate.album_name.as_deref(), normalized_artist),
    ]
    .into_iter()
    .max()
    .unwrap_or_default()
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
/// A hit without a title tells nothing about its script. Credits and release
/// labels do not count: "As the sugar cube floats (feat. 阿兰·达瓦卓玛)" is a
/// Latin title, not "1% Battery Girl" written in Chinese.
fn titles_differ_in_script(candidate_title: Option<&str>, playing_title: &str) -> bool {
    fn base(title: &str) -> String {
        base_title(title).unwrap_or_else(|| title.to_string())
    }
    fn non_latin(title: &str) -> bool {
        has_non_latin_letters(&base(title))
    }
    // A title in both scripts ("Winner and Loser / 勝者と敗者") is its own song,
    // not the playing one written in another script.
    fn mixed(title: &str) -> bool {
        let base = base(title);
        has_non_latin_letters(&base) && base.chars().any(is_latin_letter)
    }
    candidate_title
        .is_some_and(|title| !mixed(title) && non_latin(title) != non_latin(playing_title))
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
    loose_title: bool,
) -> bool {
    let (title_score, artist_score) =
        metadata_scores(candidate, normalized_title, normalized_artist);
    // A title that only contains the playing one ("Lemon·柠檬气息" for "Lemon") is
    // another song unless its artist vouches for it. One named exactly like it is
    // the song unless it is credited to another artist in the same script, since
    // artists are credited in other scripts. A shortened edit has no such leeway
    // (`loose_title` false): its full version is named like it, or it is not it.
    (title_score == 4 && !artist_conflicts(candidate, normalized_artist))
        || (loose_title && title_score > 0 && artist_score > 0)
        || (anchored
            && credits_artist(candidate, normalized_artist)
            && titles_differ_in_script(candidate.track_name.as_deref(), title))
}

/// How an artist's name is written: only in Latin letters, with none, or both
/// ("Official髭男dism").
#[derive(PartialEq)]
enum Script {
    Latin,
    Other,
    Mixed,
}

fn script_of(name: &str) -> Script {
    match (
        name.chars().any(is_latin_letter),
        has_non_latin_letters(name),
    ) {
        (true, true) => Script::Mixed,
        (false, true) => Script::Other,
        _ => Script::Latin,
    }
}

/// True when the hit is credited to another artist than the playing one for
/// certain: a different name in the same script. A name in another script ("米津玄師"
/// for "Kenshi Yonezu", "イーグルス" for "Eagles") or one that mixes both proves
/// nothing, since nothing here translates names.
fn artist_conflicts(candidate: &Candidate, normalized_artist: &str) -> bool {
    let Some(credit) = candidate
        .artist_name
        .as_deref()
        .filter(|credit| !normalize(credit).is_empty())
    else {
        return false;
    };
    if normalized_artist.is_empty() {
        return false;
    }
    if named_artist_score(candidate, normalized_artist) > 0 {
        return false;
    }
    let (credit_script, artist_script) = (script_of(credit), script_of(normalized_artist));
    match (credit_script, artist_script) {
        (Script::Latin, Script::Latin) | (Script::Other, Script::Other) => true,
        // "Official髭男dism" is "Official HIGE DANdism" in Latin letters: the
        // Latin words one name has must be in the other.
        (Script::Mixed, Script::Latin | Script::Mixed) | (Script::Latin, Script::Mixed) => {
            let (credit, artist) = (normalize(credit), normalized_artist);
            let words_of = |name: &str| latin_words(name);
            let (credit_words, artist_words) = (words_of(&credit), words_of(artist));
            let shares = |words: &[String], other: &str| words.iter().any(|w| other.contains(w));
            !(shares(&credit_words, artist) || shares(&artist_words, &credit))
                && !(credit_words.is_empty() && artist_words.is_empty())
        }
        _ => false,
    }
}

/// The runs of Latin letters of at least three in a name ("official" and "dism"
/// in "official髭男dism").
fn latin_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut run = String::new();
    for char in name.chars().chain(std::iter::once(' ')) {
        if is_latin_letter(char) {
            run.push(char);
        } else {
            if run.chars().count() >= 3 {
                words.push(std::mem::take(&mut run));
            }
            run.clear();
        }
    }
    words
}

/// True when the hit's own artist credit names the playing artist, as one of
/// several or alone. Unlike the artist score, a name found inside the track
/// title ("Perfume Genius" in "(feat. Perfume Genius)") or only contained in
/// another name ("Perfume" in "Perfume Genius") does not count.
fn credits_artist(candidate: &Candidate, normalized_artist: &str) -> bool {
    const SEPARATORS: [&str; 10] = [
        ",",
        ";",
        "&",
        "/",
        "\u{d7}",
        "+",
        " x ",
        " feat. ",
        " feat ",
        " featuring ",
    ];
    fn names(value: &str) -> Vec<String> {
        let mut parts = vec![value.to_lowercase()];
        for separator in SEPARATORS {
            parts = parts
                .iter()
                .flat_map(|part| {
                    part.split(separator)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .collect();
        }
        parts
            .iter()
            .map(|part| normalize(part))
            .filter(|part| !part.is_empty())
            .collect()
    }
    let Some(credit) = candidate.artist_name.as_deref() else {
        return false;
    };
    let wanted = names(normalized_artist);
    !normalized_artist.is_empty()
        && (normalize(credit) == normalized_artist
            || names(credit).iter().any(|name| wanted.contains(name)))
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
    // A song listed under the playing title by the playing artist, whatever its
    // length, is the song itself: another song of the artist in another script is
    // not a transliteration of it, only a song of about the same length.
    let listed_by_title = hits.iter().any(|(candidate, _)| {
        (track_title_score(candidate, &normalized_title, &normalized_artist) == 4
            || single_title_score(candidate, &normalized_title) > 0)
            && metadata_scores(candidate, &normalized_title, &normalized_artist).1 > 0
    });
    hits.retain(|(candidate, _)| {
        duration_matches(candidate.duration, duration_ms)
            && is_same_song(
                candidate,
                title,
                &normalized_title,
                &normalized_artist,
                duration_ms.is_some()
                    && !listed_by_title
                    && duration_difference_ms(candidate.duration, duration_ms)
                        <= CROSS_SCRIPT_TOLERANCE_MS,
                true,
            )
    });
    drop_ambiguous(&mut hits, |(candidate, _)| candidate, &normalized_artist);
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

/// Whether nothing but its script says a hit is by the playing artist: its credit
/// shares no name with it ("Eagles" for "イーグルス").
fn artist_unverified(candidate: &Candidate, normalized_artist: &str) -> bool {
    !normalized_artist.is_empty()
        && named_artist_score(candidate, normalized_artist) == 0
        && !credits_artist(candidate, normalized_artist)
}

/// How many single-letter edits turn one text into the other.
fn edit_distance(left: &str, right: &str) -> usize {
    let right = right.chars().collect::<Vec<_>>();
    let mut row = (0..=right.len()).collect::<Vec<_>>();
    for (i, l) in left.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, r) in right.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(l != *r))
                .min(row[j] + 1)
                .min(above + 1);
            diagonal = above;
        }
    }
    row[right.len()]
}

/// Removes the hits whose artist cannot be verified when they are credited to
/// several unrelated artists: a title and a length that unrelated artists share
/// do not say which of them is the playing song ("Albachiara" by "Vasco Rossi"
/// and by "Mauro Martins" for "ヴァスコ・ロッシ"). A credit that most of those
/// hits agree on, misspellings and "The" aside, is the song's: it is kept alone.
fn drop_ambiguous<T>(
    hits: &mut Vec<T>,
    candidate: impl Fn(&T) -> &Candidate,
    normalized_artist: &str,
) {
    let credit = |hit: &T| {
        let name = normalize(candidate(hit).artist_name.as_deref().unwrap_or_default());
        name.strip_prefix("the ")
            .map(str::to_string)
            .unwrap_or(name)
    };
    // Credits that are one name spelled two ways, grouped with how many hits hold them.
    let mut groups: Vec<(String, usize)> = Vec::new();
    for hit in hits
        .iter()
        .filter(|hit| artist_unverified(candidate(hit), normalized_artist))
    {
        let name = credit(hit);
        match groups
            .iter_mut()
            .find(|(known, _)| edit_distance(known, &name) <= name.chars().count().max(8) / 8)
        {
            Some((_, count)) => *count += 1,
            None => groups.push((name, 1)),
        }
    }
    if groups.len() < 2 {
        return;
    }
    groups.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let others = groups[1..].iter().map(|(_, count)| count).sum::<usize>();
    let dominant = (groups[0].1 >= 2 && groups[0].1 >= 3 * others).then(|| groups[0].0.clone());
    hits.retain(|hit| {
        !artist_unverified(candidate(hit), normalized_artist)
            || dominant.as_deref().is_some_and(|name| {
                edit_distance(name, &credit(hit)) <= name.chars().count().max(8) / 8
            })
    });
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
                // The edit's length cannot confirm the song, so its artist must.
                && !artist_unverified(candidate, &normalized_artist)
                && is_same_song(
                    candidate,
                    title,
                    &normalized_title,
                    &normalized_artist,
                    *index == 0,
                    false,
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
    strip_trailing_label(title, |label| {
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
    })
}

/// The title without a trailing group or dash label that `is_label` accepts
/// ("Song (TV Size)", "Song - Short Ver."), or `None` when it has none.
fn strip_trailing_label(title: &str, is_label: impl Fn(&str) -> bool) -> Option<String> {
    let title = title.trim_end();
    let grouped = title
        .chars()
        .last()
        .and_then(|closing| match closing {
            ')' => title.rfind('('),
            ']' => title.rfind('['),
            '>' => title.rfind('<'),
            _ => None,
        })
        .filter(|open| is_label(&title[open + 1..title.len() - 1]))
        .map(|open| title[..open].trim_end());
    // The rightmost dash, so a dash inside the title is kept whatever kind it is.
    let dashed = [" - ", " \u{2013} ", " \u{2014} "]
        .iter()
        .filter_map(|dash| title.rsplit_once(dash))
        .filter(|(_, label)| is_label(label))
        .max_by_key(|(rest, _)| rest.len())
        .map(|(rest, _)| rest.trim_end());
    grouped
        .or(dashed)
        .filter(|stripped| !stripped.is_empty())
        .map(str::to_string)
}

/// True for the trailing labels that name the same recording, remastered or
/// credited differently by the player and by lrc.red: "(feat. X)", "(2011
/// Remaster)", "- Mono Version", "[Deluxe]".
fn is_release_label(label: &str) -> bool {
    let label = normalize(label);
    let mut words = label.split_whitespace();
    // "(Official Video)", "[Lyric Video]": how a video site names a song, which
    // is not part of its title.
    if !label.is_empty()
        && label.split_whitespace().all(|word| {
            matches!(
                word,
                "official"
                    | "music"
                    | "lyric"
                    | "lyrics"
                    | "video"
                    | "audio"
                    | "visualizer"
                    | "mv"
                    | "hd"
                    | "hq"
                    | "4k"
            )
        })
    {
        return true;
    }
    matches!(
        words.next(),
        Some("feat" | "ft" | "featuring" | "with" | "prod")
    ) || label
        .split_whitespace()
        .any(|word| word.starts_with("remaster") || matches!(word, "mono" | "stereo" | "deluxe"))
        || [" bonus track ", " radio edit "]
            .iter()
            .any(|marker| format!(" {label} ").contains(marker))
        || ["single version", "album version", "explicit", "clean"]
            .iter()
            .any(|marker| label == *marker)
}

/// The title without its release labels and a trailing "ft. X" credit, or
/// `None` when there is nothing to strip. lrc.red lists "Hotel California (2013
/// Remaster)" where a player says "Hotel California - Remastered 2011".
fn base_title(title: &str) -> Option<String> {
    let mut base = title.trim().to_string();
    loop {
        let lower = base.to_ascii_lowercase();
        let credit = [" feat. ", " feat ", " ft. ", " ft ", " featuring "]
            .iter()
            .filter_map(|marker| lower.find(marker))
            .min();
        let next = strip_trailing_label(&base, is_release_label)
            .or_else(|| credit.map(|index| base[..index].trim_end().to_string()))
            .filter(|stripped| !stripped.is_empty());
        match next {
            Some(next) => base = next,
            None => break,
        }
    }
    (base != title.trim()).then_some(base)
}

/// "Artist - Song" as a title, the way a video channel that is not the artist
/// names it, and the same pair the other way round: the (title, artist) forms
/// to ask for when the title as it is finds nothing.
fn dash_title_queries(title: &str) -> Vec<(String, String)> {
    const DASHES: [&str; 3] = [" - ", " \u{2013} ", " \u{2014} "];
    // The leftmost dash: an artist's name rarely has one, a song's title may.
    let Some((first, second)) = DASHES
        .iter()
        .filter_map(|dash| title.split_once(dash))
        .min_by_key(|(first, _)| first.len())
    else {
        return Vec::new();
    };
    let (first, second) = (first.trim(), second.trim());
    if first.is_empty() || second.is_empty() {
        return Vec::new();
    }
    vec![
        (second.to_string(), first.to_string()),
        (first.to_string(), second.to_string()),
    ]
}

fn provider_error(provider: &str, error: impl std::fmt::Display) -> String {
    format!("{provider}: {error}")
}

/// `base` with `params` as its query string, every value escaped.
fn request_url(provider: &str, base: &str, params: &[(&str, &str)]) -> Result<Url, String> {
    Url::parse_with_params(base, params).map_err(|error| provider_error(provider, error))
}

/// `https://lrc.red/s/<segments>`, each segment escaped so an odd ISRC from the
/// service cannot reach another path.
fn lrc_red_song_url(provider: &str, segments: &[&str]) -> Result<Url, String> {
    let mut url =
        Url::parse("https://lrc.red/s/").map_err(|error| provider_error(provider, error))?;
    url.path_segments_mut()
        .map_err(|()| provider_error(provider, "not a base URL"))?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
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
    let (title, artist) = (&clean_tag(title), &clean_artist(artist));
    let mut found = search_lrc_red(client, title, artist, duration_ms).await;
    if matches!(found, Ok(None))
        && let Some(primary) = primary_artist(artist)
    {
        found = search_lrc_red(client, title, primary, duration_ms).await;
    }
    for (alternate_title, alternate_artist) in fallback_queries(title, artist, duration_ms) {
        if !matches!(found, Ok(None)) {
            break;
        }
        found = search_lrc_red(client, &alternate_title, &alternate_artist, duration_ms).await;
    }
    // A TV size or short version is not on lrc.red, but its full version is.
    if matches!(found, Ok(None))
        && let (Some(stripped), Some(edit_ms)) = (strip_edit_marker(title), duration_ms)
    {
        found = search_lrc_red_edit(client, &stripped, artist, edit_ms).await;
    }
    found
}

/// What to ask for when the song as the player names it finds nothing: the
/// title without its release labels, and a title that holds the artist (a
/// channel that is not the artist uploads "Artist - Song") split into the two.
/// A split needs the length, which keeps a title with a dash from passing for
/// another song of that name.
fn fallback_queries(title: &str, artist: &str, duration_ms: Option<u64>) -> Vec<(String, String)> {
    let mut queries = Vec::new();
    if let Some(base) = base_title(title) {
        queries.push((base, artist.to_string()));
    }
    if duration_ms.is_some() {
        queries.extend(dash_title_queries(title));
    }
    queries
}

/// The first credited artist of a joined artist string, or `None` when it
/// names a single artist. Separators need surrounding spaces, so names such
/// as "Simon&Garfunkel" are left whole.
fn primary_artist(artist: &str) -> Option<&str> {
    const SEPARATORS: [&str; 10] = [
        " (",
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
    let mut url = request_url(
        "lrc.red match",
        "https://lrc.red/match.json",
        &[("title", title), ("artist", artist)],
    )?;
    if let Some(duration_seconds) = duration_seconds {
        url.query_pairs_mut()
            .append_pair("duration", &duration_seconds.to_string());
    }
    let response = client
        .get(url)
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
    format!("{title} {artist}")
        .replace(['\'', '’'], " ")
        .trim()
        .to_string()
}

/// One `/search.json` query: the hits for a title and artist written as free
/// text. It finds songs `/match.json` cannot when lrc.red credits the artist
/// differently (IRIS OUT is by "米津玄师", not "Kenshi Yonezu").
async fn lrc_red_text_hits(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
) -> Result<Vec<LrcRedHit>, String> {
    let url = request_url(
        "lrc.red search",
        "https://lrc.red/search.json",
        &[("q", &search_query(title, artist))],
    )?;
    let response = client
        .get(url)
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
    // An artist credited in a script the provider does not use ("イーグルス" for
    // "Eagles") finds nothing with the artist in the query, while the title alone
    // lists the recording; its length and its own artist credit pick it.
    if !has_exact_title(&matched, title, artist, duration_ms)
        && !normalize(artist).is_empty()
        && has_non_latin_letters(artist)
        && let Ok(hits) = lrc_red_text_hits(client, title, "").await
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
    let halves = DASHES
        .iter()
        .filter_map(|dash| title.rsplit_once(dash))
        .max_by_key(|(rest, _)| rest.len())
        .map(|(first, second)| vec![first.trim().to_string(), second.trim().to_string()])
        .unwrap_or_default()
        .into_iter()
        .filter(|half| !half.is_empty())
        .collect::<Vec<_>>();
    if !halves.is_empty() {
        return halves;
    }
    // "紅(KURENAI)": the second name in brackets, when it is in another script. A
    // release label such as "(feat. X)" is in the same script and is not one.
    let trimmed = title.trim_end();
    let Some(open) = trimmed
        .strip_suffix(')')
        .and_then(|rest| rest.rfind('(').map(|open| (rest, open)))
    else {
        return Vec::new();
    };
    let (outer, inner) = (open.0[..open.1].trim(), open.0[open.1 + 1..].trim());
    if outer.is_empty()
        || inner.is_empty()
        || has_non_latin_letters(outer) == has_non_latin_letters(inner)
    {
        return Vec::new();
    }
    vec![outer.to_string(), inner.to_string()]
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
    if ranked.is_empty() && !normalize(artist).is_empty() && has_non_latin_letters(artist) {
        // An artist credited in a script lrc.red does not use: the title alone.
        let hits = lrc_red_text_hits(client, title, "").await?;
        ranked = rank_edit_matches(hit_candidates(hits), title, artist, edit_ms);
    }
    first_lyrics(client, ranked.into_iter().take(1).collect()).await
}

/// The lyrics of the first ranked recording that has any.
async fn first_lyrics(client: &reqwest::Client, ranked: Vec<(Candidate, String)>) -> Lookup {
    let started_at = std::time::Instant::now();
    let found = first_found(ranked.clone(), |isrc: String| async move {
        fetch_lrc_red_ttml(client, &isrc).await
    })
    .await?;
    let found = match found {
        // lrc.red lists a song more than once (single, album, compilation) and
        // only some of its files carry the romanization: when the best one has
        // none, a later one that has it is the better choice.
        Some((candidate, isrc, lyrics)) if lacks_romanization(&lyrics) => {
            let later = ranked
                .iter()
                .skip_while(|(_, other)| *other != isrc)
                .skip(1)
                .cloned()
                .collect::<Vec<_>>();
            let romanized = first_found(later, |isrc: String| async move {
                Ok(fetch_lrc_red_ttml(client, &isrc)
                    .await?
                    .filter(|lyrics| !lacks_romanization(lyrics)))
            })
            .await
            .ok()
            .flatten();
            romanized.or(Some((candidate, isrc, lyrics)))
        }
        found => found,
    };
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

/// True when some line is Japanese and none has a romanization.
fn lacks_romanization(lyrics: &Lyrics) -> bool {
    lyrics.lines.iter().all(|line| line.romanized.is_none()) && lyrics_have_japanese(lyrics)
}

/// The lyrics of one recording, or `None` when it has no usable file.
async fn fetch_lrc_red_ttml(
    client: &reqwest::Client,
    isrc: &str,
) -> Result<Option<Lyrics>, String> {
    let response = client
        .get(lrc_red_song_url(
            "lrc.red lyrics",
            &[&format!("{isrc}.ttml")],
        )?)
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
    let mut lines = song
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
    // The active line is the last one that has started, which assumes time order.
    lines.sort_by_key(|line| line.start_ms);
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
        .post(lrc_red_song_url("lrc.red sync", &[isrc, "sync"])?)
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
    let (title, artist) = (&clean_tag(title), &clean_artist(artist));
    let mut matches = lrc_red_matches(client, title, artist, duration_ms).await?;
    if matches.is_empty()
        && let Some(primary) = primary_artist(artist)
    {
        matches = lrc_red_matches(client, title, primary, duration_ms).await?;
    }
    if matches.is_empty() {
        for (alternate_title, alternate_artist) in fallback_queries(title, artist, duration_ms) {
            matches =
                lrc_red_matches(client, &alternate_title, &alternate_artist, duration_ms).await?;
            if !matches.is_empty() {
                break;
            }
        }
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

/// How far an LRCLIB recording may be from the playing length. Line times are
/// only right for the recording they were made against, and lyrics that run
/// off the audio are worse than none, so this is closer than lrc.red's.
const LRCLIB_DURATION_TOLERANCE_MS: u64 = 2_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LrclibTrack {
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    /// Seconds.
    duration: Option<f64>,
    synced_lyrics: Option<String>,
}

/// One `/api/search` query: the tracks LRCLIB lists for a title and artist.
async fn lrclib_search(
    client: &reqwest::Client,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<LrclibTrack>, String> {
    let mut params = vec![("track_name", title)];
    params.extend(artist.map(|artist| ("artist_name", artist)));
    let url = request_url("LRCLIB search", "https://lrclib.net/api/search", &params)?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| provider_error("LRCLIB search", error))?;
    if !response.status().is_success() {
        return Err(provider_error("LRCLIB search", response.status()));
    }
    response
        .json::<Vec<LrclibTrack>>()
        .await
        .map_err(|error| provider_error("LRCLIB search", error))
}

/// The line-timed lyrics of the LRCLIB recording that is the playing song, or
/// `None`. Strict on purpose: the title must be the playing one (or the playing
/// one without its release labels), the artist must match and the length must
/// be within `LRCLIB_DURATION_TOLERANCE_MS`, since the lines are timed against
/// one recording and any other drifts. The closest in length wins.
fn pick_lrclib_lyrics(
    tracks: Vec<LrclibTrack>,
    title: &str,
    artist: &str,
    length: Length,
) -> Option<(Candidate, Lyrics)> {
    let normalized_artist = normalize(artist);
    let titles = [Some(title.to_string()), base_title(title)]
        .into_iter()
        .flatten()
        .map(|title| canonical_title(&title, &normalized_artist))
        .collect::<Vec<_>>();
    let matching = tracks
        .into_iter()
        .map(|track| {
            let candidate = Candidate {
                track_name: track.track_name,
                artist_name: track.artist_name,
                album_name: track.album_name,
                duration: track.duration,
            };
            (candidate, track.synced_lyrics)
        })
        .filter(|(candidate, synced)| {
            synced.as_deref().is_some_and(|lrc| !lrc.trim().is_empty())
                && length.admits(candidate.duration)
                && titles.iter().any(|title| {
                    track_title_score(candidate, title, &normalized_artist) == 4
                        && metadata_scores(candidate, title, &normalized_artist).1 > 0
                })
                // The length of an edit cannot confirm the song, so its artist must.
                && (matches!(length, Length::Exactly(_))
                    || !artist_unverified(candidate, &normalized_artist))
        })
        .collect::<Vec<_>>();
    let mut matching = matching;
    drop_ambiguous(
        &mut matching,
        |(candidate, _)| candidate,
        &normalized_artist,
    );
    // LRCLIB holds the same recording more than once, some with the lyrics in
    // romaji or in English. For a song named in Japanese the ones in Japanese
    // come first: they are the lyrics, and what a romanization is made from.
    let japanese_song = has_japanese_script(title) || has_japanese_script(artist);
    let mut parsed = matching
        .into_iter()
        .filter_map(|(candidate, synced)| {
            let lyrics = parse_lrc(&synced?, candidate.duration)?;
            Some((candidate, lyrics))
        })
        .collect::<Vec<_>>();
    parsed.sort_by_key(|(candidate, lyrics)| {
        (
            japanese_song && !lyrics_have_japanese(lyrics),
            // Some files hold only the first verse; a whole song is better.
            !covers_song(lyrics, candidate.duration),
            duration_difference_ms(candidate.duration, Some(length.target_ms())),
        )
    });
    parsed.into_iter().next()
}

/// True when the last line starts in the last half of the song, or the song's
/// length is not known.
fn covers_song(lyrics: &Lyrics, duration_seconds: Option<f64>) -> bool {
    let (Some(last), Some(seconds)) = (lyrics.lines.last(), duration_seconds) else {
        return true;
    };
    last.start_ms as f64 * 2.0 >= seconds * 1_000.0
}

fn is_japanese_char(char: char) -> bool {
    matches!(char,
        '\u{3040}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{4E00}'..='\u{9FFF}'
        | '\u{3400}'..='\u{4DBF}' | '\u{FF66}'..='\u{FF9F}')
}

fn has_japanese_script(text: &str) -> bool {
    text.chars().any(is_japanese_char)
}

fn lyrics_have_japanese(lyrics: &Lyrics) -> bool {
    lyrics.lines.iter().any(|line| {
        line.text
            .segments
            .iter()
            .any(|segment| has_japanese_script(&segment.text))
    })
}

/// The length a recording must have to be the playing one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Length {
    /// Within `LRCLIB_DURATION_TOLERANCE_MS` of this.
    Exactly(u64),
    /// This long or longer: the playing recording is a shortened edit and the
    /// one with lyrics is its full version.
    AtLeast(u64),
}

impl Length {
    fn target_ms(self) -> u64 {
        match self {
            Length::Exactly(ms) | Length::AtLeast(ms) => ms,
        }
    }

    fn admits(self, candidate_seconds: Option<f64>) -> bool {
        match self {
            Length::Exactly(ms) => {
                duration_difference_ms(candidate_seconds, Some(ms)) <= LRCLIB_DURATION_TOLERANCE_MS
            }
            // Room for the rounding of durations, as for lrc.red's.
            Length::AtLeast(ms) => candidate_seconds
                .filter(|seconds| seconds.is_finite())
                .is_some_and(|seconds| (seconds * 1_000.0) as u64 + 1_000 >= ms),
        }
    }
}

/// One LRCLIB search and the song its tracks must be to count.
#[derive(Debug, Clone, PartialEq)]
struct LrclibQuery {
    /// The title and artist asked for; without an artist, the title alone.
    search: (String, Option<String>),
    /// The title and artist a track must carry.
    title: String,
    artist: String,
    /// The playing recording is a shortened edit, and the track its full version.
    edit: bool,
}

/// The searches LRCLIB is asked, in order: as playing, with the primary artist,
/// without the title's release labels, and both. LRCLIB files "1% Battery Girl
/// (feat. ChiliChill)" by "Sān-Z & HOYO-MiX" with its lyrics only as "1% Battery
/// Girl" by "Sān-Z". Then the title and artist a channel put into one title
/// ("Artist - Song"), and for an artist credited in a script LRCLIB does not
/// list ("イーグルス" for "Eagles") the title alone. Last, the same for the full
/// version of an edit ("Song (TV Size)").
fn lrclib_queries(title: &str, artist: &str) -> Vec<LrclibQuery> {
    fn searches(title: &str, artist: &str) -> Vec<(String, Option<String>)> {
        let primary = primary_artist(artist);
        let base = base_title(title);
        let mut searches = vec![(title.to_string(), Some(artist.to_string()))];
        if let Some(primary) = primary {
            searches.push((title.to_string(), Some(primary.to_string())));
        }
        if let Some(base) = &base {
            searches.push((base.clone(), Some(artist.to_string())));
        }
        if let (Some(base), Some(primary)) = (base.clone(), primary) {
            searches.push((base, Some(primary.to_string())));
        }
        if !normalize(artist).is_empty() && has_non_latin_letters(artist) {
            searches.push((title.to_string(), None));
            searches.extend(base.map(|base| (base, None)));
        }
        searches
    }

    let mut queries = Vec::new();
    let mut add = |search: (String, Option<String>), title: &str, artist: &str, edit: bool| {
        let query = LrclibQuery {
            search,
            title: title.to_string(),
            artist: artist.to_string(),
            edit,
        };
        if !queries.contains(&query) {
            queries.push(query);
        }
    };
    for search in searches(title, artist) {
        add(search, title, artist, false);
    }
    for (split_title, split_artist) in dash_title_queries(title) {
        for search in searches(&split_title, &split_artist) {
            add(search, &split_title, &split_artist, false);
        }
    }
    for half in title_halves(title) {
        for search in searches(&half, artist) {
            add(search, &half, artist, false);
        }
    }
    if let Some(full) = strip_edit_marker(title) {
        for search in searches(&full, artist) {
            add(search, &full, artist, true);
        }
    }
    queries
}

/// Line-timed lyrics from LRCLIB, for a song lrc.red does not have. Without the
/// playing length there is nothing to tell the recording the lines were timed
/// against from another, so such a song is not looked up.
async fn fetch_lrclib(
    client: &reqwest::Client,
    title: &str,
    artist: &str,
    duration_ms: Option<u64>,
) -> Lookup {
    let Some(duration_ms) = duration_ms else {
        return Ok(None);
    };
    let (title, artist) = (clean_tag(title), clean_artist(artist));
    let started_at = std::time::Instant::now();
    let mut failure = None;
    for query in lrclib_queries(&title, &artist) {
        let (search_title, search_artist) = &query.search;
        match lrclib_search(client, search_title, search_artist.as_deref()).await {
            Ok(tracks) => {
                let length = if query.edit {
                    Length::AtLeast(duration_ms)
                } else {
                    Length::Exactly(duration_ms)
                };
                if let Some((candidate, lyrics)) =
                    pick_lrclib_lyrics(tracks, &query.title, &query.artist, length)
                {
                    println!(
                        "[latency] lrclib total={}ms track={:?}",
                        started_at.elapsed().as_millis(),
                        candidate.track_name
                    );
                    return Ok(Some(build_result(candidate, lyrics)));
                }
            }
            // Asking on only makes the limit last: retry the lookup later.
            Err(error) if error.contains("429") => return Err(error),
            Err(error) => failure = Some(error),
        }
    }
    println!(
        "[latency] lrclib total={}ms no match",
        started_at.elapsed().as_millis()
    );
    failure.map_or(Ok(None), Err)
}

/// Parses LRC text: `[mm:ss.xx]` stamps (a line may carry several) before the
/// text. Tags such as `[ar:Artist]` and word stamps such as `<00:10.50>` are
/// dropped. Each line ends where the next stamped one starts, and an empty
/// stamped line only ends the one before it. `None` when no line has text.
fn parse_lrc(lrc: &str, duration_seconds: Option<f64>) -> Option<Lyrics> {
    let mut entries: Vec<(u64, String)> = Vec::new();
    for raw in lrc.lines() {
        let mut rest = raw.trim_start();
        let mut stamps = Vec::new();
        while let Some(after) = rest.strip_prefix('[') {
            let Some((tag, tail)) = after.split_once(']') else {
                break;
            };
            let Some(stamp_ms) = parse_lrc_stamp(tag) else {
                break;
            };
            stamps.push(stamp_ms);
            rest = tail.trim_start();
        }
        let text = strip_lrc_word_stamps(rest);
        entries.extend(stamps.into_iter().map(|stamp_ms| (stamp_ms, text.clone())));
    }
    entries.sort_by_key(|(start_ms, _)| *start_ms);

    let last_end_ms = duration_seconds
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .map(|seconds| (seconds * 1_000.0).round() as u64);
    let lines = entries
        .iter()
        .enumerate()
        .filter(|(_, (_, text))| !text.is_empty())
        .map(|(index, (start_ms, text))| {
            let end_ms = entries
                .get(index + 1)
                .map(|(next_ms, _)| *next_ms)
                .or(last_end_ms)
                .unwrap_or(start_ms + 5_000)
                .max(*start_ms);
            Line {
                start_ms: *start_ms,
                end_ms,
                voice: 0,
                text: LineText {
                    segments: vec![Segment {
                        start_ms: *start_ms,
                        end_ms,
                        text: text.clone(),
                    }],
                    background: Vec::new(),
                },
                romanized: None,
                translation: None,
            }
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then_some(Lyrics { lines })
}

/// The milliseconds of an LRC stamp's inside: "01:23.45", "01:23" or "1:23.456".
/// `None` for a tag such as "ar:Artist".
fn parse_lrc_stamp(tag: &str) -> Option<u64> {
    let (minutes, seconds) = tag.split_once(':')?;
    if minutes.is_empty() || !minutes.chars().all(|char| char.is_ascii_digit()) {
        return None;
    }
    if seconds.is_empty()
        || !seconds
            .chars()
            .all(|char| char.is_ascii_digit() || char == '.')
    {
        return None;
    }
    let minutes = minutes.parse::<u64>().ok()?;
    let seconds = seconds.parse::<f64>().ok()?;
    Some(
        minutes
            .saturating_mul(60_000)
            .saturating_add((seconds * 1_000.0).round() as u64),
    )
}

/// The text of an LRC line without the `<mm:ss.xx>` stamps of enhanced LRC.
fn strip_lrc_word_stamps(text: &str) -> String {
    let mut stripped = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        let (before, from_open) = rest.split_at(open);
        match from_open
            .split_once('>')
            .filter(|(tag, _)| parse_lrc_stamp(&tag[1..]).is_some())
        {
            Some((_, after)) => {
                stripped.push_str(before);
                rest = after;
            }
            None => {
                stripped.push_str(before);
                stripped.push('<');
                rest = &from_open[1..];
            }
        }
    }
    stripped.push_str(rest);
    stripped.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod live;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_cleaned_the_way_the_providers_write_them() {
        // Full-width letters, as a Japanese tag editor types them.
        assert_eq!(
            clean_tag("\u{FF2C}\u{FF45}\u{FF4D}\u{FF4F}\u{FF4E}\u{3000}\u{FF01}"),
            "Lemon !"
        );
        // A voiced kana as the plain kana and its combining mark.
        assert_eq!(clean_tag("ゼ"), "ゼ");
        assert_eq!(clean_tag("セ\u{3099}"), "ゼ");
        // YouTube Music's channel for an artist.
        assert_eq!(clean_artist("Kenshi Yonezu - Topic"), "Kenshi Yonezu");
        assert_eq!(clean_artist("Topic"), "Topic");
        assert_eq!(normalize("\u{FF2C}ＥＭＯＮ"), "lemon");
    }

    #[test]
    fn a_video_label_is_not_part_of_the_title() {
        assert_eq!(
            base_title("Lemon (Official Video)").as_deref(),
            Some("Lemon")
        );
        assert_eq!(base_title("Lemon [Lyric Video]").as_deref(), Some("Lemon"));
        assert_eq!(
            base_title("Lemon - Official Audio").as_deref(),
            Some("Lemon")
        );
        assert_eq!(base_title("Video Killed the Radio Star"), None);
        assert_eq!(base_title("Lemon (Live)"), None);
    }

    #[test]
    fn lrclib_lyrics_in_japanese_come_before_romaji_for_a_japanese_song() {
        let romaji = Some("[00:01.00] Yume nara ba dore hodo yokatta deshou");
        let japanese = Some("[00:01.00] 夢ならばどれほどよかったでしょう");
        let tracks = vec![
            // The romaji is 0.3 s closer in length.
            lrclib_track("Lemon", "米津玄師", Some(256.0), romaji),
            lrclib_track("Lemon", "米津玄師", Some(256.3), japanese),
        ];
        let (_, lyrics) =
            pick_lrclib_lyrics(tracks, "Lemon", "米津玄師", Length::Exactly(256_000)).unwrap();
        assert!(lyrics_have_japanese(&lyrics));
        // A song named in Latin only keeps the closest in length.
        let tracks = vec![
            lrclib_track("Lemon", "Kenshi Yonezu", Some(256.0), romaji),
            lrclib_track("Lemon", "Kenshi Yonezu", Some(256.3), japanese),
        ];
        let (_, lyrics) =
            pick_lrclib_lyrics(tracks, "Lemon", "Kenshi Yonezu", Length::Exactly(256_000)).unwrap();
        assert!(!lyrics_have_japanese(&lyrics));
    }

    #[test]
    fn japanese_lyrics_without_a_romanization_are_told_from_those_with_one() {
        let lyrics = parse_lrc("[00:01.00] 夢ならば", Some(5.0)).unwrap();
        assert!(lacks_romanization(&lyrics));
        let latin = parse_lrc("[00:01.00] Yume nara ba", Some(5.0)).unwrap();
        assert!(!lacks_romanization(&latin));
    }

    fn result_of(title: &str, artist: &str) -> LyricsResult {
        LyricsResult {
            track_name: title.to_string(),
            artist_name: artist.to_string(),
            album_name: String::new(),
            duration: None,
            word_timed: false,
            lines: Vec::new(),
        }
    }

    #[test]
    fn lyrics_are_confirmed_only_by_the_playing_title_and_artist() {
        let confirms =
            |title: &str, artist: &str, found: LyricsResult| confirms_song(&found, title, artist);
        assert!(confirms(
            "Lemon",
            "Kenshi Yonezu",
            result_of("Lemon", "Kenshi Yonezu")
        ));
        assert!(confirms(
            "Lemon (TV Size)",
            "Kenshi Yonezu",
            result_of("Lemon", "Kenshi Yonezu")
        ));
        assert!(confirms(
            "Lemon",
            "Kenshi Yonezu & X",
            result_of("Lemon", "Kenshi Yonezu")
        ));
        assert!(confirms(
            "秦 基博 - ひまわりの約束",
            "Unknown Channel",
            result_of("ひまわりの約束", "秦 基博")
        ));
        // The same name by an artist whose name cannot be compared.
        assert!(!confirms(
            "Lemon",
            "米津玄師",
            result_of("Lemon", "Kenshi Yonezu")
        ));
        // The artist with a title in another script.
        assert!(!confirms(
            "うっせぇわ",
            "Ado",
            result_of("BINKUSUNO SAKE", "Ado")
        ));
        assert!(!confirms("うっせぇわ", "Ado", result_of("Usseewa", "Ado")));
    }

    #[test]
    fn two_unrelated_artists_of_the_katakana_credited_song_make_it_ambiguous() {
        // "Albachiara" as the Japanese storefront lists it: by "ヴァスコ・ロッシ".
        let hits = || {
            vec![
                (
                    candidate("Albachiara", "Mauro Martins, Rubinho", 246.0),
                    "cover",
                ),
                (candidate("Albachiara", "Vasco Rossi", 245.0), "original"),
            ]
        };
        assert!(rank_matches(hits(), "Albachiara", "ヴァスコ・ロッシ", Some(245_000)).is_empty());
        // One artist alone, even in different spellings, is the song.
        let one = vec![
            (candidate("Albachiara", "The Vasco Rossi", 245.0), "a"),
            (candidate("Albachiara", "Vasco Rossi", 244.0), "b"),
        ];
        assert_eq!(
            rank_matches(one, "Albachiara", "ヴァスコ・ロッシ", Some(245_000)).len(),
            2
        );
        // A hit that names the playing artist is never dropped, and comes first.
        let named = vec![
            (candidate("Albachiara", "Mauro Martins", 246.0), "cover"),
            (candidate("Albachiara", "ヴァスコ・ロッシ", 245.0), "named"),
        ];
        let ranked = rank_matches(named, "Albachiara", "ヴァスコ・ロッシ", Some(245_000));
        assert_eq!(ranked[0].1, "named");
    }

    #[test]
    fn an_artist_mixing_scripts_is_told_from_one_that_shares_none_of_its_words() {
        let credit = |artist: &str| candidate("Cry Baby", artist, 240.0);
        let playing = normalize("Official髭男dism");
        assert!(artist_conflicts(
            &credit("Clean Bandit, Anne-Marie, David Guetta"),
            &playing
        ));
        assert!(!artist_conflicts(
            &credit("Official HIGE DANdism"),
            &playing
        ));
        assert!(!artist_conflicts(&credit("Official胡子男dism"), &playing));
        assert!(!artist_conflicts(
            &credit("オフィシャル髭男ディズム"),
            &playing
        ));
    }

    #[test]
    fn an_edit_is_the_full_version_only_when_its_artist_is_the_playing_one() {
        // "Lemon (TV Size)" by "米津玄師": the length cannot tell the song from
        // another "Lemon" that is longer than the edit.
        let hits = vec![
            (candidate("Lemon", "N.E.R.D, Rihanna", 220.0), "other"),
            (candidate("Lemon", "Kenshi Yonezu", 256.0), "song"),
        ];
        assert!(rank_edit_matches(hits.clone(), "Lemon", "米津玄師", 102_000).is_empty());
        let ranked = rank_edit_matches(hits, "Lemon", "Kenshi Yonezu", 102_000);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].1, "song");
    }

    #[test]
    fn the_credit_most_hits_agree_on_is_the_katakana_credited_songs_even_misspelled() {
        // Laura Pausini's "Strani amori" as LRCLIB lists it, once with a typo.
        let hits = vec![
            (candidate("Strani amori", "Laura Pausini", 256.0), 1),
            (candidate("Strani amori", "Laura Pausini", 256.0), 2),
            (candidate("Strani amori", "Laura Pausino", 255.0), 3),
            (candidate("Strani amori", "Someone Else", 255.0), 4),
        ];
        let ranked = rank_matches(hits, "Strani Amori", "ラウラ・パウジーニ", Some(255_000));
        assert_eq!(
            ranked.iter().map(|(_, id)| *id).collect::<Vec<_>>().len(),
            3
        );
        assert!(
            ranked
                .iter()
                .all(|(hit, _)| hit.artist_name.as_deref() != Some("Someone Else"))
        );
    }

    #[test]
    fn a_title_naming_the_song_twice_in_brackets_is_searched_by_each_name() {
        assert_eq!(title_halves("紅(KURENAI)"), ["紅", "KURENAI"]);
        assert_eq!(title_halves("Lemon (feat. X)"), Vec::<String>::new());
        assert_eq!(title_halves("Song (Live)"), Vec::<String>::new());
        assert_eq!(primary_artist("X JAPAN (X)"), Some("X JAPAN"));
        assert_eq!(edit_distance("pausini", "pausino"), 1);
    }

    #[test]
    fn a_title_in_both_scripts_is_not_the_playing_song_in_another_script() {
        // Found by the live fuzzer: "Haruka Kanata" matched the same band's
        // "Winner and Loser / 勝者と敗者" because both run 242s.
        assert!(!titles_differ_in_script(
            Some("Winner and Loser / 勝者と敗者"),
            "Haruka Kanata"
        ));
        assert!(titles_differ_in_script(Some("遥か彼方"), "Haruka Kanata"));
        assert!(titles_differ_in_script(Some("Haruka Kanata"), "遥か彼方"));
    }

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
    fn a_song_listed_under_the_playing_title_is_not_replaced_by_another_in_another_script() {
        // AIZO (237.8 s here) is on lrc.red at 215.9 s; ねっこ by the same artist is 239.7 s.
        // The length rejects AIZO, and ねっこ must not pass off for it.
        let hits = vec![
            (candidate("AIZO", "King Gnu", 215.858), "aizo"),
            (candidate("ねっこ", "King Gnu", 239.729), "nekko"),
        ];

        assert!(rank_matches(hits, "AIZO", "King Gnu", Some(237_761)).is_empty());
    }

    #[test]
    fn lrclib_is_also_asked_for_the_bare_title_by_the_primary_artist() {
        let searches = |title: &str, artist: &str| {
            lrclib_queries(title, artist)
                .into_iter()
                .map(|query| query.search)
                .collect::<Vec<_>>()
        };
        let pair = |title: &str, artist: &str| (title.to_string(), Some(artist.to_string()));
        assert_eq!(
            searches("1% Battery Girl (feat. ChiliChill)", "Sān-Z & HOYO-MiX"),
            [
                pair("1% Battery Girl (feat. ChiliChill)", "Sān-Z & HOYO-MiX"),
                pair("1% Battery Girl (feat. ChiliChill)", "Sān-Z"),
                pair("1% Battery Girl", "Sān-Z & HOYO-MiX"),
                pair("1% Battery Girl", "Sān-Z"),
            ]
        );
        assert_eq!(searches("Song", "Artist"), [pair("Song", "Artist")]);
    }

    #[test]
    fn lrclib_is_asked_for_the_title_alone_when_the_artist_is_credited_in_katakana() {
        // A Japanese storefront lists "Hotel California" by "イーグルス"; LRCLIB
        // files it under "Eagles", so the artist cannot be part of the search.
        let queries = lrclib_queries("Hotel California", "イーグルス");
        assert!(
            queries
                .iter()
                .any(|query| query.search == ("Hotel California".to_string(), None))
        );
        // A Latin artist is never dropped from the search.
        assert!(
            lrclib_queries("Hotel California", "Eagles")
                .iter()
                .all(|query| query.search.1.is_some())
        );
    }

    #[test]
    fn lrclib_is_asked_for_the_song_a_channel_put_into_a_title_with_its_artist() {
        let queries = lrclib_queries("秦 基博 - ひまわりの約束", "Unknown Channel");
        let asks = |title: &str, artist: &str| {
            queries.iter().any(|query| {
                query.search == (title.to_string(), Some(artist.to_string()))
                    && query.title == title
                    && query.artist == artist
                    && !query.edit
            })
        };
        assert!(asks("ひまわりの約束", "秦 基博"));
    }

    #[test]
    fn lrclib_is_asked_for_the_full_version_of_an_edit() {
        let queries = lrclib_queries("Pale Blue (TV Size)", "Kenshi Yonezu");
        assert!(queries.iter().any(|query| query.edit
            && query.title == "Pale Blue"
            && query.search == ("Pale Blue".to_string(), Some("Kenshi Yonezu".to_string()))));
        // The edit itself is asked for first, in case LRCLIB lists it.
        assert!(!queries[0].edit);
    }

    #[test]
    fn the_full_version_of_an_edit_is_any_exactly_named_track_at_least_as_long() {
        let synced = Some("[00:01.00] A line");
        let tracks = || {
            vec![
                // Another recording named like it, but shorter than the edit.
                lrclib_track("Pale Blue", "Kenshi Yonezu", Some(60.0), synced),
                // A live recording and a cover are other songs.
                lrclib_track("Pale Blue (Live)", "Kenshi Yonezu", Some(300.0), synced),
                lrclib_track("Pale Blue", "Someone Else", Some(300.0), synced),
                lrclib_track("Pale Blue", "Kenshi Yonezu", Some(260.0), synced),
                lrclib_track("Pale Blue", "Kenshi Yonezu", Some(290.0), synced),
            ]
        };
        let picked = pick_lrclib_lyrics(
            tracks(),
            "Pale Blue",
            "Kenshi Yonezu",
            Length::AtLeast(118_000),
        );
        // The closest to the edit's length that is at least as long.
        assert_eq!(
            picked.map(|(candidate, _)| candidate.duration),
            Some(Some(260.0))
        );
    }

    #[test]
    fn a_track_by_another_artist_in_the_same_script_is_not_the_katakana_credited_song() {
        // イーグルス could be "Eagles"; "Hotel California" by Gipsy Kings is
        // as unrelated as any other Latin name, and so is "Eagles of Death Metal"
        // only because it is a different credit. A katakana credit proves nothing
        // about a Latin one, so only the length and the exact title decide.
        let candidate = |artist: &str| Candidate {
            track_name: Some("Hotel California".to_string()),
            artist_name: Some(artist.to_string()),
            album_name: None,
            duration: Some(391.0),
        };
        assert!(!artist_conflicts(&candidate("Eagles"), "イーグルス"));
        // A different name in the same script is a different artist.
        assert!(artist_conflicts(&candidate("Gipsy Kings"), "eagles"));
        assert!(!artist_conflicts(&candidate("Eagles"), "eagles"));
        assert!(artist_conflicts(&candidate("花僮"), "米津玄師"));
        // A name that mixes both scripts cannot be told from the playing one.
        assert!(!artist_conflicts(
            &candidate("Official胡子男dism"),
            "official髭男dism"
        ));
    }

    fn on_album(mut hit: Candidate, album: &str) -> Candidate {
        hit.album_name = Some(album.to_string());
        hit
    }

    #[test]
    fn a_single_named_like_the_song_stands_for_a_title_lrc_red_spells_differently() {
        // lrc.red files "1% Battery Girl" as "1% Low Battery Girl" on the single
        // of that name. Another song of the artist, in Japanese, is 1 s off the
        // playing length (210 s), which used to make it pass as a transliteration.
        for playing_ms in [208_547, 209_507, 210_000, 210_596, 211_000] {
            let hits = vec![
                (
                    on_album(
                        candidate("『暁の空に、輝ける誓い』", "Sān-Z, HOYO-MiX", 210.596),
                        "『暁の空に、輝ける誓い』 - Single",
                    ),
                    "japanese",
                ),
                (
                    on_album(
                        candidate(
                            "1% Low Battery Girl (feat. ChiliChill)",
                            "Sān-Z & HOYO-MiX",
                            208.547,
                        ),
                        "1% Battery Girl - Single",
                    ),
                    "single",
                ),
            ];

            let ranked = rank_matches(
                hits,
                "1% Battery Girl (feat. ChiliChill)",
                "Sān-Z & HOYO-MiX",
                Some(playing_ms),
            );

            assert_eq!(
                ranked.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
                ["single"],
                "playing length {playing_ms} ms"
            );
        }
    }

    #[test]
    fn only_a_single_vouches_for_the_title_not_an_album() {
        let on_single =
            |track: &str, album: &str| on_album(candidate(track, "AC/DC", 317.0), album);
        let title = canonical_title("Back In Black", &normalize("AC/DC"));

        // Every track of an album named like the song is not the song.
        assert_eq!(
            single_title_score(&on_single("Shoot to Thrill", "Back In Black"), &title),
            0
        );
        assert_eq!(
            single_title_score(
                &on_single("Back In Black", "Back In Black - Single"),
                &title
            ),
            2
        );
        // A name that is only part of the title is not the song.
        assert_eq!(
            single_title_score(&on_single("Back In Black", "Black - Single"), &title),
            0
        );
        assert_eq!(
            single_title_score(
                &on_single("Back In Black", "Back In Black - SINGLE"),
                "back in black part two"
            ),
            0
        );
        assert_eq!(single_name("Back In Black - SINGLE"), Some("Back In Black"));
        assert_eq!(single_name("Single"), None);
        assert_eq!(single_name(" - Single"), Some(""));
    }

    #[test]
    fn a_single_vouches_for_its_song_not_for_the_other_recordings_on_it() {
        let on_single = |track: &str| {
            on_album(
                candidate(track, "Sān-Z & HOYO-MiX", 208.5),
                "1% Battery Girl - Single",
            )
        };
        let featured = "1 battery girl feat chilichill";
        let plain = "1 battery girl";

        // The song, with the feature credit on either side or none.
        assert_eq!(
            single_title_score(
                &on_single("1% Low Battery Girl (feat. ChiliChill)"),
                featured
            ),
            2
        );
        assert_eq!(
            single_title_score(&on_single("1% Low Battery Girl (feat. ChiliChill)"), plain),
            2
        );
        assert_eq!(
            single_title_score(&on_single("1% Low Battery Girl"), plain),
            2
        );
        // The version without the feature is not the featured song.
        assert_eq!(
            single_title_score(&on_single("1% Low Battery Girl"), featured),
            0
        );
        // A B-side with a title of its own is another song.
        assert_eq!(single_title_score(&on_single("Dead Battery"), featured), 0);
        assert_eq!(single_title_score(&on_single("Dead Battery"), plain), 0);
        // Nor are its instrumental, remix or live recordings.
        for other in [
            "1% Low Battery Girl (Instrumental)",
            "1% Low Battery Girl - Instrumental",
            "1% Low Battery Girl (feat. ChiliChill) [Remix]",
            "1% Low Battery Girl (Live)",
        ] {
            assert_eq!(
                single_title_score(&on_single(other), featured),
                0,
                "{other}"
            );
        }
    }

    #[test]
    fn a_b_side_on_the_single_is_not_the_song() {
        // Same artist, same length, on the single of the playing song, but
        // another song: it must neither pass nor hide the cross-script match.
        let hits = vec![(
            on_album(
                candidate("Dead Battery", "Sān-Z & HOYO-MiX", 208.0),
                "1% Battery Girl - Single",
            ),
            "b-side",
        )];

        assert!(
            rank_matches(
                hits,
                "1% Battery Girl (feat. ChiliChill)",
                "Sān-Z & HOYO-MiX",
                Some(208_547)
            )
            .is_empty()
        );
    }

    #[test]
    fn a_remastered_single_still_stands_for_its_song() {
        let hits = vec![
            (
                on_album(
                    candidate("『暁の空に、輝ける誓い』", "Sān-Z, HOYO-MiX", 208.6),
                    "『暁の空に、輝ける誓い』 - Single",
                ),
                "japanese",
            ),
            (
                on_album(
                    candidate(
                        "1% Low Battery Girl (Remastered) (feat. ChiliChill)",
                        "Sān-Z & HOYO-MiX",
                        208.547,
                    ),
                    "1% Battery Girl (Remastered) - Single",
                ),
                "single",
            ),
        ];

        let ranked = rank_matches(
            hits,
            "1% Battery Girl (Remastered) (feat. ChiliChill)",
            "Sān-Z & HOYO-MiX",
            Some(208_547),
        );

        assert_eq!(
            ranked.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
            ["single"]
        );
    }

    #[test]
    fn a_credit_in_another_script_does_not_make_another_song_a_transliteration() {
        // "1% Battery Girl" is not on lrc.red; this other song of the artist is
        // 1 s longer and only its featured singer is written in Chinese.
        let hits = vec![(
            candidate(
                "As the sugar cube floats, fleeting in time (feat. 阿兰·达瓦卓玛)",
                "Sān-Z, HOYO-MiX",
                209.507,
            ),
            "sugar cube",
        )];

        assert!(
            rank_matches(
                hits,
                "1% Battery Girl (feat. ChiliChill)",
                "Sān-Z & HOYO-MiX",
                Some(208_547)
            )
            .is_empty()
        );
    }

    #[test]
    fn a_title_in_another_script_needs_a_closer_length_than_a_matching_one() {
        let hits = vec![(candidate("ねっこ", "King Gnu", 239.729), "nekko")];

        // 2 s away: another song of about that length.
        assert!(rank_matches(hits.clone(), "AIZO", "King Gnu", Some(237_761)).is_empty());
        // Half a second away is still a coincidence: the fuzzer found "Hello Song" passing
        // for 恋 and "Girl A" for ワタリドリ at 0.5-0.7 s. One catalog's copy of a recording
        // differs from another's by a few milliseconds, not by half a second.
        assert!(rank_matches(hits.clone(), "AIZO", "King Gnu", Some(239_200)).is_empty());
        // Within a fraction of that: the same recording under its title in another script.
        assert_eq!(
            rank_matches(hits, "AIZO", "King Gnu", Some(239_650)).len(),
            1
        );
    }

    #[test]
    fn a_title_that_only_contains_the_playing_one_is_not_vouched_for_by_an_artist_in_another_script()
     {
        // "Lemon" by 米津玄師 came back as a Chinese song "Lemon·柠檬气息" by 花僮, 1 s longer.
        let hits = vec![(candidate("Lemon·柠檬气息", "花僮", 257.4), "chinese")];
        assert!(rank_matches(hits, "Lemon", "米津玄師", Some(256_000)).is_empty());

        // A hit named exactly like the song still is, whatever script its artist is in.
        let hits = vec![(candidate("Lemon", "Kenshi Yonezu", 255.8), "song")];
        assert_eq!(
            rank_matches(hits, "Lemon", "米津玄師", Some(256_000)).len(),
            1
        );
    }

    #[test]
    fn a_name_inside_another_name_or_a_track_title_is_not_the_artists_credit() {
        let spitting = candidate(
            "Spitting Off the Edge of the World (feat. Perfume Genius)",
            "Yeah Yeah Yeahs",
            258.0,
        );
        let multiple = candidate("Palloncini", "ATARDE, ゆず", 131.0);
        let both = candidate("打上花火", "DAOKO×米津玄師", 289.0);

        assert!(!credits_artist(&spitting, "perfume"));
        assert!(!credits_artist(
            &candidate("Song", "Perfume Genius", 1.0),
            "perfume"
        ));
        assert!(credits_artist(&multiple, "ゆず"));
        assert!(credits_artist(&both, "米津玄師"));
        assert!(credits_artist(
            &candidate("Song", "Perfume", 1.0),
            "perfume"
        ));
        assert!(!credits_artist(&candidate("Song", "Perfume", 1.0), ""));
    }

    #[test]
    fn an_edit_is_not_passed_off_by_a_song_that_only_features_the_artist() {
        let hits = vec![(
            candidate(
                "Spitting Off the Edge of the World (feat. Perfume Genius)",
                "Yeah Yeah Yeahs",
                258.0,
            ),
            "other",
        )];

        assert!(rank_edit_matches(hits, "ポリリズム", "Perfume", 100_000).is_empty());
    }

    #[test]
    fn accents_and_combining_marks_do_not_change_a_title() {
        // macOS files spell "ò" as "o" and U+0300, and lrc.red has "Partiro" without one.
        assert_eq!(
            normalize("Con te partiro\u{300}"),
            normalize("Con te partirò")
        );
        assert_eq!(normalize("Con te partirò"), normalize("Con Te Partiro"));
        assert_eq!(normalize("Più Bella Cosa"), normalize("Piu' bella cosa"));
        assert_eq!(normalize("Felicita\u{300}"), "felicita");
        // は and a combining U+309A is ぱ, か and U+3099 is が, and カ is ガ likewise.
        assert_eq!(normalize("は\u{309A}"), "ぱ");
        assert_eq!(
            normalize("ドライフラワー"),
            normalize("ト\u{3099}ライフラワー")
        );
        assert_eq!(normalize("ガ"), normalize("カ\u{3099}"));
        assert_eq!(normalize("う\u{3099}"), "ゔ");
        // A mark that makes no kana is dropped, as before, and plain kana are kept.
        assert_eq!(normalize("あ\u{3099}"), "あ");
        assert_ne!(normalize("かな"), normalize("がな"));
    }

    fn lrclib_track(
        title: &str,
        artist: &str,
        duration: Option<f64>,
        lrc: Option<&str>,
    ) -> LrclibTrack {
        LrclibTrack {
            track_name: Some(title.to_string()),
            artist_name: Some(artist.to_string()),
            album_name: None,
            duration,
            synced_lyrics: lrc.map(str::to_string),
        }
    }

    const LRC: &str = "[ar:King Gnu]\n[00:01.50] First line\n[00:04.00][00:10.50]Chorus <00:04.50>line\n[00:07.00]\n[00:09.10] Last line";

    #[test]
    fn lrc_lines_are_timed_to_where_the_next_one_starts() {
        let lyrics = parse_lrc(LRC, Some(12.0)).unwrap();
        let timed = lyrics
            .lines
            .iter()
            .map(|line| {
                (
                    line.start_ms,
                    line.end_ms,
                    line.text.segments[0].text.as_str(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            timed,
            [
                (1_500, 4_000, "First line"),
                // The empty line at 7 s only ends this one.
                (4_000, 7_000, "Chorus line"),
                (9_100, 10_500, "Last line"),
                // The last line ends with the song.
                (10_500, 12_000, "Chorus line"),
            ]
        );
        // A line is timed as a whole, so the song is shown line by line.
        assert!(!lyrics.is_word_timed());
    }

    #[test]
    fn lrc_without_text_has_no_lyrics() {
        assert!(parse_lrc("[ar:Someone]\n[00:01.00]\n", None).is_none());
        assert!(parse_lrc("just words", None).is_none());
    }

    #[test]
    fn lrclib_lyrics_are_taken_for_the_same_recording_only() {
        let pick = |tracks: Vec<LrclibTrack>, duration_ms| {
            pick_lrclib_lyrics(tracks, "AIZO", "King Gnu", Length::Exactly(duration_ms))
                .map(|(candidate, _)| candidate.duration)
        };
        let synced = Some("[00:01.00] A line");
        let tracks = || {
            vec![
                // Another recording of it.
                lrclib_track("AIZO", "King Gnu", Some(215.8), synced),
                // Another artist's version of the same length.
                lrclib_track("AIZO", "Danie Green", Some(237.9), synced),
                // Another song.
                lrclib_track("ねっこ", "King Gnu", Some(237.9), synced),
                // Plain lyrics only, and no length.
                lrclib_track("AIZO", "King Gnu", Some(237.9), None),
                lrclib_track("AIZO", "King Gnu", None, synced),
            ]
        };

        assert_eq!(pick(tracks(), 237_761), None);

        let mut with_match = tracks();
        with_match.push(lrclib_track("AIZO", "King Gnu", Some(239.5), synced));
        with_match.push(lrclib_track("aizo", "king gnu", Some(238.0), synced));
        // Within 2 s, the closest in length.
        assert_eq!(pick(with_match, 237_761), Some(Some(238.0)));
    }

    #[test]
    fn a_release_label_does_not_stop_an_lrclib_match() {
        let tracks = vec![lrclib_track(
            "Hotel California",
            "Eagles",
            Some(391.0),
            Some("[00:01.00] On a dark desert highway"),
        )];

        assert!(
            pick_lrclib_lyrics(
                tracks,
                "Hotel California (2013 Remaster)",
                "Eagles",
                Length::Exactly(391_500)
            )
            .is_some()
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
    fn a_song_credited_to_the_artist_in_another_script_is_kept_and_its_covers_are_not() {
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

        // The cover only contains the title and is by another artist, so it is not offered at all.
        let order = ranked.iter().map(|(_, id)| *id).collect::<Vec<_>>();
        assert_eq!(order, ["official"]);
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

    #[test]
    fn release_labels_and_feature_credits_are_stripped_from_a_title() {
        for (title, base) in [
            ("Hotel California - Remastered 2011", "Hotel California"),
            ("Hotel California (2013 Remaster)", "Hotel California"),
            ("Despacito ft. Daddy Yankee", "Despacito"),
            ("Uptown Funk (feat. Bruno Mars) - Radio Edit", "Uptown Funk"),
            ("Song [Deluxe] (feat. X)", "Song"),
        ] {
            assert_eq!(base_title(title).as_deref(), Some(base), "{title}");
        }
        for title in [
            "Song",
            "Song (Cover)",
            "Song (Monologue)",
            "Song - Kimono",
            "Song - Live",
            "(feat. X)",
            "Remastered",
        ] {
            assert_eq!(base_title(title), None, "{title}");
        }
    }

    #[test]
    fn an_artist_and_title_in_one_title_are_asked_both_ways_round() {
        assert_eq!(
            dash_title_queries("Matt Dusk - Charlie's Inferno - 60's Version"),
            vec![
                (
                    "Charlie's Inferno - 60's Version".to_string(),
                    "Matt Dusk".to_string()
                ),
                (
                    "Matt Dusk".to_string(),
                    "Charlie's Inferno - 60's Version".to_string()
                ),
            ]
        );
        assert!(dash_title_queries("Song").is_empty());
        assert!(dash_title_queries("Self-Aware").is_empty());
    }

    #[test]
    fn a_split_title_needs_a_known_length() {
        assert!(fallback_queries("Love - Hate", "Someone", None).is_empty());
        assert_eq!(
            fallback_queries("Love - Hate", "Someone", Some(200_000)).len(),
            2
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

        // The cover is credited to another artist in the same script, so it is
        // not offered at all, not even as a second choice.
        assert_eq!(
            ranked.iter().map(|(_, id)| id.as_str()).collect::<Vec<_>>(),
            ["official"]
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
    fn synced_lines_come_out_in_time_order() {
        let song = serde_json::from_str::<LrcRedSong>(
            r#"{"lyrics":{"lines":[
                {"words":[[{"text":"late","begin":30.0,"end":31.0}],[{"text":"line","begin":31.0,"end":32.0}]]},
                {"words":[[{"text":"early","begin":2.0,"end":3.0}],[{"text":"line","begin":3.0,"end":4.0}]]}]}}"#,
        )
        .unwrap();

        let starts = song_to_lyrics(&song)
            .unwrap()
            .lines
            .iter()
            .map(|line| line.start_ms)
            .collect::<Vec<_>>();

        assert_eq!(starts, [2_000, 30_000]);
    }

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
    fn builds_escaped_request_urls() {
        let url = request_url(
            "test",
            "https://lrc.red/match.json",
            &[("title", "Rock & Roll #1"), ("artist", "AC/DC")],
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://lrc.red/match.json?title=Rock+%26+Roll+%231&artist=AC%2FDC"
        );
        assert_eq!(
            lrc_red_song_url("test", &["../x?y", "sync"])
                .unwrap()
                .as_str(),
            "https://lrc.red/s/..%2Fx%3Fy/sync"
        );
    }

    #[test]
    fn text_that_normalizes_to_nothing_matches_no_title_or_artist() {
        assert_eq!(score(Some("\u{266a}"), "clint eastwood"), 0);
        assert_eq!(score(Some(""), "gorillaz"), 0);
        assert_eq!(score(Some("Gorillaz"), "gorillaz"), 4);
    }

    #[test]
    fn a_huge_lrc_minute_value_saturates_instead_of_overflowing() {
        assert_eq!(parse_lrc_stamp("99999999999999999:00"), Some(u64::MAX));
        assert_eq!(parse_lrc_stamp("01:23.45"), Some(83_450));
    }
}
