//! A live fuzzer for the lyrics lookup. It sends real songs, as a player would
//! name them and as it would mangle them, through the same lrc.red and LRCLIB
//! code the app runs, and reports every answer that is wrong, missing, not
//! synced or not romanized.
//!
//! The songs are `fixtures/lyrics-corpus.json`: real recordings with the title,
//! artist and length each iTunes storefront gives them (a Japanese storefront
//! says "米津玄師", an American one "Kenshi Yonezu"). Each variant is asked for as
//! it is, and the first one again with a few typical player quirks applied
//! (`Mutation`), where the answer must not change.
//!
//! It needs the network and is slow, so it is ignored by default:
//!
//! ```text
//! bun run fuzz:lyrics
//! LYRICS_FUZZ_FILTER=yonezu bun run fuzz:lyrics
//! ```
//!
//! Environment: `LYRICS_FUZZ_FILTER` (substring of a title or artist),
//! `LYRICS_FUZZ_MUTANTS` (quirked queries per song, default 2),
//! `LYRICS_FUZZ_CONCURRENCY` (default 3), `LYRICS_FUZZ_CATEGORY` (only songs of
//! that category), `LYRICS_FUZZ_SAMPLE` (every n-th song only, from the
//! `LYRICS_FUZZ_OFFSET`-th), `LYRICS_FUZZ_OUT` (report directory,
//! default `target/lyrics-fuzz`) and `LYRICS_FUZZ_STRICT=1` (fail when the app
//! would show lyrics of another song).

use super::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const CORPUS: &str = include_str!("../../fixtures/lyrics-corpus.json");

/// Within this a recording is the playing one (lrc.red's own tolerance).
const LENGTH_TOLERANCE_MS: u64 = 3_000;
/// Fewer lines than this is a stub, not a synced song.
const MIN_LINES: usize = 4;
/// The last line must start in the last half of the song for it to be whole.
const MIN_COVERAGE: f64 = 0.5;
/// Share of the Japanese lines that must have a romanization.
const MIN_ROMANIZED: f64 = 0.8;
/// Below this, two providers' lyrics are not the same song.
const MIN_AGREEMENT: f64 = 0.25;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Song {
    category: String,
    duration_ms: u64,
    /// Overrides what the category says about the lyrics being Japanese.
    #[serde(default)]
    expect_japanese: Option<bool>,
    variants: Vec<Variant>,
    /// Set for a query that asks for a shortened edit, whose lyrics are the full
    /// recording's and so are longer than the length asked for.
    #[serde(skip)]
    edited: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct Variant {
    title: String,
    artist: String,
}

impl Song {
    fn expects_japanese(&self) -> Option<bool> {
        self.expect_japanese.or(match self.category.as_str() {
            c if c.starts_with("jp-") => Some(true),
            "it-title" | "en-feat" => Some(false),
            _ => None,
        })
    }
}

fn corpus() -> Vec<Song> {
    serde_json::from_str(CORPUS).expect("lyrics corpus parses")
}

/// A way a player or a library tags a song that is not how the catalog names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Mutation {
    AsIs,
    /// "Title (feat. Guest Artist)".
    FeatTitle,
    /// "Title - Remastered 2011".
    RemasterTitle,
    /// "Title (TV Size)": the full recording is what lrc.red has.
    TvSizeTitle,
    /// "Artist & Guest Artist".
    JoinedArtist,
    /// "Artist feat. Guest Artist".
    FeatArtist,
    /// "ARTIST" and "TITLE".
    Shouting,
    /// "Artist - Title" with a channel as the artist, as video sites file songs.
    ArtistInTitle,
    /// Accents written as a letter plus a combining mark, as macOS files do.
    DecomposedAccents,
    /// Voiced kana written as the plain kana plus a combining mark, as macOS files do.
    DecomposedKana,
    /// ’ where the catalog has '.
    CurlyApostrophe,
    /// The player's length a second longer than the catalog's.
    LongerBy1s,
    /// And a second shorter.
    ShorterBy1s,
    /// Spaces the player pads the tags with.
    PaddedTags,
    /// The player's length two seconds longer, the edge of what LRCLIB accepts.
    LongerBy2s,
    /// And two seconds shorter.
    ShorterBy2s,
    /// Letters written full-width ("Ｌｅｍｏｎ"), as Japanese tag editors do.
    FullWidth,
    /// "Title (Official Video)" or "Title [Lyric Video]", as video sites name songs.
    VideoTitle,
    /// "Artist - Topic", the channel YouTube Music files a song under.
    TopicArtist,
    /// "Yonezu Kenshi" for "Kenshi Yonezu": the family name first.
    ReversedArtist,
    /// The title of one storefront with the artist of another ("Lemon" by "米津玄師").
    CrossVariant,
}

const MUTATIONS: [Mutation; 20] = [
    Mutation::FeatTitle,
    Mutation::RemasterTitle,
    Mutation::TvSizeTitle,
    Mutation::JoinedArtist,
    Mutation::FeatArtist,
    Mutation::Shouting,
    Mutation::ArtistInTitle,
    Mutation::DecomposedAccents,
    Mutation::DecomposedKana,
    Mutation::CurlyApostrophe,
    Mutation::LongerBy1s,
    Mutation::ShorterBy1s,
    Mutation::PaddedTags,
    Mutation::LongerBy2s,
    Mutation::ShorterBy2s,
    Mutation::FullWidth,
    Mutation::VideoTitle,
    Mutation::TopicArtist,
    Mutation::ReversedArtist,
    Mutation::CrossVariant,
];

/// Precomposed accented letters with their base letter and combining mark.
const DECOMPOSITIONS: [(char, char, char); 24] = [
    ('à', 'a', '\u{300}'),
    ('è', 'e', '\u{300}'),
    ('ì', 'i', '\u{300}'),
    ('ò', 'o', '\u{300}'),
    ('ù', 'u', '\u{300}'),
    ('á', 'a', '\u{301}'),
    ('é', 'e', '\u{301}'),
    ('í', 'i', '\u{301}'),
    ('ó', 'o', '\u{301}'),
    ('ú', 'u', '\u{301}'),
    ('â', 'a', '\u{302}'),
    ('ê', 'e', '\u{302}'),
    ('ô', 'o', '\u{302}'),
    ('ä', 'a', '\u{308}'),
    ('ë', 'e', '\u{308}'),
    ('ï', 'i', '\u{308}'),
    ('ö', 'o', '\u{308}'),
    ('ü', 'u', '\u{308}'),
    ('å', 'a', '\u{30A}'),
    ('ñ', 'n', '\u{303}'),
    ('ç', 'c', '\u{327}'),
    ('É', 'E', '\u{301}'),
    ('È', 'E', '\u{300}'),
    ('Å', 'A', '\u{30A}'),
];

fn decompose(text: &str) -> String {
    text.chars()
        .flat_map(|char| {
            DECOMPOSITIONS
                .iter()
                .find(|(composed, _, _)| *composed == char)
                .map_or_else(|| vec![char], |(_, base, mark)| vec![*base, *mark])
        })
        .collect()
}

/// `text` with each voiced or semi-voiced kana split into its plain kana and the
/// combining mark (が is か and U+3099), the way macOS spells file names.
fn decompose_kana(text: &str) -> String {
    text.chars()
        .flat_map(|char| {
            let code = char as u32;
            // Hiragana and katakana: the voiced form of か is か+1, of は is は+1 and the
            // semi-voiced は+2; つ and う have their own places.
            let plain_and_mark = match char {
                'ゔ' => Some(('う', '\u{3099}')),
                'ヴ' => Some(('ウ', '\u{3099}')),
                _ => {
                    let hiragana = (0x3042..=0x3093).contains(&code);
                    let katakana = (0x30A2..=0x30F3).contains(&code);
                    let offset = if katakana { 0x60 } else { 0 };
                    let base = code - offset;
                    let voiced = (hiragana || katakana)
                        && matches!(
                            base,
                            0x304C
                                | 0x304E
                                | 0x3050
                                | 0x3052
                                | 0x3054
                                | 0x3056
                                | 0x3058
                                | 0x305A
                                | 0x305C
                                | 0x305E
                                | 0x3060
                                | 0x3062
                                | 0x3065
                                | 0x3067
                                | 0x3069
                                | 0x3070
                                | 0x3073
                                | 0x3076
                                | 0x3079
                                | 0x307C
                        );
                    let semi = (hiragana || katakana)
                        && matches!(base, 0x3071 | 0x3074 | 0x3077 | 0x307A | 0x307D);
                    if voiced {
                        char::from_u32(code - 1).map(|plain| (plain, '\u{3099}'))
                    } else if semi {
                        char::from_u32(code - 2).map(|plain| (plain, '\u{309A}'))
                    } else {
                        None
                    }
                }
            };
            match plain_and_mark {
                Some((plain, mark)) => vec![plain, mark],
                None => vec![char],
            }
        })
        .collect()
}

/// ASCII letters, digits and punctuation as their full-width forms, and the space.
fn full_width(text: &str) -> String {
    text.chars()
        .map(|char| match char {
            ' ' => '\u{3000}',
            '!'..='~' => char::from_u32(char as u32 - 0x21 + 0xFF01).unwrap_or(char),
            other => other,
        })
        .collect()
}

/// The query a player would send for `variant` under `mutation`, or `None` when
/// the quirk does not apply to the song. `others` are the song's other variants.
fn mutate(
    mutation: Mutation,
    variant: &Variant,
    others: &[Variant],
    duration_ms: u64,
) -> Option<Query> {
    let (title, artist) = (variant.title.as_str(), variant.artist.as_str());
    let mut query = Query {
        song: 0,
        variant: 0,
        mutation,
        title: title.to_string(),
        artist: artist.to_string(),
        duration_ms,
    };
    match mutation {
        Mutation::AsIs => {}
        Mutation::FeatTitle => query.title = format!("{title} (feat. Guest Artist)"),
        Mutation::RemasterTitle => query.title = format!("{title} - Remastered 2011"),
        Mutation::TvSizeTitle => {
            query.title = format!("{title} (TV Size)");
            query.duration_ms = duration_ms * 2 / 5;
        }
        Mutation::JoinedArtist => query.artist = format!("{artist} & Guest Artist"),
        Mutation::FeatArtist => query.artist = format!("{artist} feat. Guest Artist"),
        Mutation::Shouting => {
            query.title = title.to_uppercase();
            query.artist = artist.to_uppercase();
            if query.title == title && query.artist == artist {
                return None;
            }
        }
        Mutation::ArtistInTitle => {
            query.title = format!("{artist} - {title}");
            query.artist = "Unknown Channel".to_string();
        }
        Mutation::DecomposedAccents => {
            query.title = decompose(title);
            query.artist = decompose(artist);
            if query.title == title && query.artist == artist {
                return None;
            }
        }
        Mutation::DecomposedKana => {
            query.title = decompose_kana(title);
            query.artist = decompose_kana(artist);
            if query.title == title && query.artist == artist {
                return None;
            }
        }
        Mutation::CurlyApostrophe => {
            query.title = title.replace('\'', "\u{2019}");
            query.artist = artist.replace('\'', "\u{2019}");
            if query.title == title && query.artist == artist {
                return None;
            }
        }
        Mutation::LongerBy2s => query.duration_ms = duration_ms + 2_000,
        Mutation::ShorterBy2s => query.duration_ms = duration_ms.saturating_sub(2_000),
        Mutation::FullWidth => {
            query.title = full_width(title);
            query.artist = full_width(artist);
            if query.title == title && query.artist == artist {
                return None;
            }
        }
        Mutation::VideoTitle => {
            query.title = if title.chars().count() % 2 == 0 {
                format!("{title} (Official Video)")
            } else {
                format!("{title} [Lyric Video]")
            };
        }
        Mutation::TopicArtist => query.artist = format!("{artist} - Topic"),
        Mutation::ReversedArtist => {
            // Only a name of two Latin words has a family name to put first.
            let words = artist.split(' ').collect::<Vec<_>>();
            if words.len() != 2 || !artist.is_ascii() {
                return None;
            }
            query.artist = format!("{} {}", words[1], words[0]);
        }
        Mutation::CrossVariant => {
            // The title of another storefront's listing, if it differs.
            let other = others
                .iter()
                .find(|other| other.title != title && other.artist != artist)?;
            query.title = other.title.clone();
        }
        Mutation::LongerBy1s => query.duration_ms = duration_ms + 1_000,
        Mutation::ShorterBy1s => query.duration_ms = duration_ms.saturating_sub(1_000),
        Mutation::PaddedTags => {
            query.title = format!("  {title} ");
            query.artist = format!(" {artist}  ");
        }
    }
    Some(query)
}

#[derive(Debug, Clone)]
struct Query {
    song: usize,
    variant: usize,
    mutation: Mutation,
    title: String,
    artist: String,
    duration_ms: u64,
}

/// Every query to run: each variant of each song as it is, and the first one
/// under `mutants` quirks that apply. The quirks go to the songs they apply to
/// least often first, so every quirk is exercised across the corpus even when
/// few songs have accents or apostrophes.
fn plan(songs: &[Song], filter: Option<&str>, mutants: usize) -> Vec<Query> {
    let mut queries = Vec::new();
    let mut used = [0usize; MUTATIONS.len()];
    let category = std::env::var("LYRICS_FUZZ_CATEGORY").ok();
    let sample = env_number("LYRICS_FUZZ_SAMPLE", 1).max(1);
    let offset = env_number("LYRICS_FUZZ_OFFSET", 0);
    for (index, song) in songs.iter().enumerate() {
        if index % sample != offset % sample
            || category
                .as_deref()
                .is_some_and(|name| name != song.category)
        {
            continue;
        }
        if let Some(filter) = filter {
            let filter = filter.to_lowercase();
            if !song.variants.iter().any(|variant| {
                format!("{} {}", variant.title, variant.artist)
                    .to_lowercase()
                    .contains(&filter)
            }) {
                continue;
            }
        }
        for (variant_index, variant) in song.variants.iter().enumerate() {
            let mut chosen = vec![Mutation::AsIs];
            if variant_index == 0 {
                let mut applicable = MUTATIONS
                    .iter()
                    .enumerate()
                    .filter(|(_, mutation)| {
                        mutate(**mutation, variant, &song.variants, song.duration_ms).is_some()
                    })
                    .map(|(slot, mutation)| (slot, *mutation))
                    .collect::<Vec<_>>();
                // The least used first; the song's position breaks ties differently per song.
                applicable.sort_by_key(|(slot, _)| {
                    (
                        used[*slot],
                        (slot + MUTATIONS.len() - index % MUTATIONS.len()) % MUTATIONS.len(),
                    )
                });
                for (slot, mutation) in applicable.into_iter().take(mutants) {
                    used[slot] += 1;
                    chosen.push(mutation);
                }
            }
            for mutation in chosen {
                if let Some(mut query) = mutate(mutation, variant, &song.variants, song.duration_ms)
                {
                    query.song = index;
                    query.variant = variant_index;
                    queries.push(query);
                }
            }
        }
    }
    queries
}

fn has_japanese(text: &str) -> bool {
    has_japanese_script(text)
}

fn line_original(line: &Line) -> String {
    line.text
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>()
        .trim()
        .to_string()
}

fn line_romanized(line: &Line) -> Option<String> {
    line.romanized.as_ref().map(|romanized| {
        romanized
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>()
            .trim()
            .to_string()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TitleMatch {
    /// The playing title, or the playing title without its labels.
    Exact,
    /// One title contains the other: a live version, a remix, a longer song.
    Loose,
    /// Written in a script no spelling of the song has, so there is nothing to compare with.
    Unverified,
    Unrelated,
}

/// How the title lrc.red or LRCLIB answered with relates to the song. Judged
/// independently of the app's own ranking, against every spelling the catalog
/// gives the song.
fn title_match(got: &str, song: &Song) -> TitleMatch {
    let mut best = TitleMatch::Unrelated;
    for variant in &song.variants {
        let artist = normalize(&variant.artist);
        let spellings = |title: &str| {
            [Some(title.to_string()), base_title(title)]
                .into_iter()
                .flatten()
                .map(|title| canonical_title(&title, &artist))
                .filter(|title| !title.is_empty())
                .collect::<Vec<_>>()
        };
        let (wanted, answered) = (spellings(&variant.title), spellings(got));
        let found = if wanted.iter().any(|w| answered.contains(w)) {
            TitleMatch::Exact
        } else if wanted.iter().any(|w| {
            answered
                .iter()
                .any(|a| a.contains(w.as_str()) || w.contains(a.as_str()))
        }) {
            TitleMatch::Loose
        } else if titles_differ_in_script(Some(got), &variant.title) {
            TitleMatch::Unverified
        } else {
            TitleMatch::Unrelated
        };
        best = best.min(found);
    }
    // A title in another script is only unverifiable when no spelling of the song
    // is in its script: otherwise the song has a spelling to compare with, and
    // this is not it.
    if best == TitleMatch::Unverified
        && song
            .variants
            .iter()
            .any(|variant| !titles_differ_in_script(Some(got), &variant.title))
    {
        return TitleMatch::Unrelated;
    }
    best
}

/// True when the artist an answer is credited to can be the song's: it names
/// the same artist as some variant, or is written in another script than that
/// variant, where nothing can be compared.
fn artist_compatible(got: &str, song: &Song) -> bool {
    let got_normalized = normalize(got);
    // An answer that names no artist says nothing against the song.
    if got_normalized.is_empty() {
        return true;
    }
    song.variants.iter().any(|variant| {
        let wanted = normalize(&variant.artist);
        let names_match = !got_normalized.is_empty()
            && !wanted.is_empty()
            && (got_normalized.contains(&wanted) || wanted.contains(&got_normalized));
        names_match || has_non_latin_letters(got) != has_non_latin_letters(&variant.artist)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    // The lyrics are probably another song's.
    WrongLength,
    WrongTitle,
    WrongLanguage,
    Disagrees,
    // The song is right but the lookup is poor.
    NotSynced,
    ShortSync,
    NoRomanization,
    PartialRomanization,
    DirtyRomanization,
    LooseTitle,
    OddArtist,
    UnverifiedTitle,
    LatinLyrics,
}

impl Kind {
    fn is_wrong_song(self) -> bool {
        matches!(
            self,
            Kind::WrongLength | Kind::WrongTitle | Kind::WrongLanguage
        )
    }

    fn label(self) -> &'static str {
        match self {
            Kind::WrongLength => "wrong-length",
            Kind::WrongTitle => "wrong-title",
            Kind::WrongLanguage => "wrong-language",
            Kind::Disagrees => "providers-disagree",
            Kind::NotSynced => "not-synced",
            Kind::ShortSync => "short-sync",
            Kind::NoRomanization => "no-romanization",
            Kind::PartialRomanization => "partial-romanization",
            Kind::DirtyRomanization => "dirty-romanization",
            Kind::LooseTitle => "loose-title",
            Kind::OddArtist => "odd-artist",
            Kind::UnverifiedTitle => "unverified-title",
            Kind::LatinLyrics => "latin-lyrics",
        }
    }
}

#[derive(Debug, Clone)]
struct Problem {
    kind: Kind,
    detail: String,
}

/// What a provider answered, and how it measures up against the song.
#[derive(Debug, Clone)]
struct Found {
    track: String,
    artist: String,
    album: String,
    duration_s: Option<u64>,
    lines: usize,
    word_timed: bool,
    japanese_lines: usize,
    romanized_lines: usize,
    /// The lyric text, lowercase and without punctuation, to compare providers.
    text: String,
    problems: Vec<Problem>,
    /// What the provider sent, to choose between the providers as the app does.
    raw: Box<LyricsResult>,
}

impl Found {
    fn wrong_song(&self) -> bool {
        self.problems
            .iter()
            .any(|problem| problem.kind.is_wrong_song())
    }

    fn has(&self, kind: Kind) -> bool {
        self.problems.iter().any(|problem| problem.kind == kind)
    }
}

#[derive(Debug, Clone)]
enum Outcome {
    Found(Box<Found>),
    Miss,
    Error(String),
}

fn analyze(song: &Song, result: &LyricsResult) -> Found {
    let mut problems = Vec::new();
    let mut problem = |kind, detail: String| problems.push(Problem { kind, detail });

    if let Some(duration) = result.duration.filter(|_| !song.edited) {
        let difference = (duration * 1_000).abs_diff(song.duration_ms);
        // lrc.red rounds the length to whole seconds.
        if difference > LENGTH_TOLERANCE_MS + 500 {
            problem(
                Kind::WrongLength,
                format!("{duration}s against {}s", song.duration_ms / 1_000),
            );
        }
    }
    // An artist credited in another script cannot be compared, so it vouches for nothing.
    let artist_fits = artist_compatible(&result.artist_name, song);
    match title_match(&result.track_name, song) {
        TitleMatch::Exact if !artist_fits => {
            problem(Kind::OddArtist, format!("by \"{}\"", result.artist_name))
        }
        TitleMatch::Exact => {}
        // A title in another script is only the song when its artist fits too.
        TitleMatch::Unverified if artist_fits => problem(
            Kind::UnverifiedTitle,
            format!(
                "\"{}\" is in a script the song has no title in",
                result.track_name
            ),
        ),
        TitleMatch::Unverified | TitleMatch::Unrelated => problem(
            Kind::WrongTitle,
            format!(
                "\"{}\" by \"{}\" is not the song",
                result.track_name, result.artist_name
            ),
        ),
        TitleMatch::Loose => problem(
            Kind::LooseTitle,
            format!("\"{}\" is only like the title", result.track_name),
        ),
    }

    let lines = &result.lines;
    let texts = lines.iter().map(line_original).collect::<Vec<_>>();
    let japanese_lines = texts.iter().filter(|text| has_japanese(text)).count();
    let share = japanese_lines as f64 / lines.len().max(1) as f64;
    match song.expects_japanese() {
        // Latin lyrics of a Japanese song are its romanization or its English
        // version under the same name: the right song, not the one the user wants.
        Some(true) if share < 0.1 => problem(
            Kind::LatinLyrics,
            format!("{japanese_lines} of {} lines are Japanese", lines.len()),
        ),
        Some(false) if japanese_lines > 0 && share >= 0.1 => problem(
            Kind::WrongLanguage,
            format!("{japanese_lines} of {} lines are Japanese", lines.len()),
        ),
        _ => {}
    }

    // A song is synced when its lines are many, in order, and run most of the way.
    let last_start = lines.iter().map(|line| line.start_ms).max().unwrap_or(0);
    if lines.len() < MIN_LINES {
        problem(Kind::NotSynced, format!("{} lines", lines.len()));
    } else if !song.edited
        && ((last_start as f64) < song.duration_ms as f64 * MIN_COVERAGE
            || last_start > song.duration_ms + LENGTH_TOLERANCE_MS)
    {
        problem(
            Kind::ShortSync,
            format!(
                "last line at {}s of a {}s song",
                last_start / 1_000,
                song.duration_ms / 1_000
            ),
        );
    }

    // Romanization is only expected of a Japanese song, and only lrc.red has it.
    let (mut romanized_lines, mut dirty) = (0, 0);
    for line in lines {
        let original = line_original(line);
        if !has_japanese(&original) {
            continue;
        }
        if let Some(romanized) = line_romanized(line)
            && !romanized.is_empty()
            && romanized != original
        {
            romanized_lines += 1;
            if has_japanese(&romanized) {
                dirty += 1;
            }
        }
    }
    if song.expects_japanese() == Some(true) && japanese_lines > 0 {
        let share = romanized_lines as f64 / japanese_lines as f64;
        if romanized_lines == 0 {
            problem(
                Kind::NoRomanization,
                format!("0 of {japanese_lines} Japanese lines"),
            );
        } else if share < MIN_ROMANIZED {
            problem(
                Kind::PartialRomanization,
                format!("{romanized_lines} of {japanese_lines} Japanese lines"),
            );
        }
        if dirty > 0 {
            problem(
                Kind::DirtyRomanization,
                format!("{dirty} romanized lines still hold kana or kanji"),
            );
        }
    }

    Found {
        track: result.track_name.clone(),
        artist: result.artist_name.clone(),
        album: result.album_name.clone(),
        duration_s: result.duration,
        lines: lines.len(),
        word_timed: result.word_timed,
        japanese_lines,
        romanized_lines,
        text: texts
            .iter()
            .flat_map(|text| text.chars())
            .filter(|char| char.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect(),
        problems,
        raw: Box::new(result.clone()),
    }
}

/// Character-pair overlap of two lyrics, 0 to 1.
fn agreement(left: &str, right: &str) -> f64 {
    text_overlap(left, right)
}

#[derive(Debug, Clone)]
struct Row {
    query: Query,
    lrc_red: Outcome,
    lrclib: Outcome,
    /// What the app shows, chosen between the two as `fetch_lyrics` does.
    app: Outcome,
    /// How much the two providers' lyrics overlap, when both answered.
    agreement: Option<f64>,
}

impl Row {
    fn app(&self) -> &Outcome {
        &self.app
    }

    /// lrc.red, or LRCLIB when lrc.red has nothing, or when its artist credit does
    /// not name the playing one and LRCLIB's lyrics for the song are others.
    fn choose(query: &Query, lrc_red: &Outcome, lrclib: &Outcome) -> Outcome {
        match (lrc_red, lrclib) {
            (Outcome::Miss, lrclib) => lrclib.clone(),
            (Outcome::Found(red), Outcome::Found(lib))
                if !confirms_song(&red.raw, &query.title, &query.artist)
                    && lrclib_beats(&red.raw, &lib.raw, &query.title, &query.artist) =>
            {
                lrclib.clone()
            }
            (found, _) => found.clone(),
        }
    }
}

async fn lookup(
    song: &Song,
    provider: &str,
    attempt: impl Fn() -> std::pin::Pin<Box<dyn Future<Output = Lookup> + Send>>,
) -> Outcome {
    // The fuzzer asks far more often than the app, so a rate limit ("429") is
    // waited out; any other failure gets one retry, to tell a flaky service from
    // a systematic failure.
    let mut result = attempt().await;
    for round in 1..=4 {
        let Err(error) = &result else { break };
        if round > 1 && !error.contains("429") {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2 * round)).await;
        result = attempt().await;
    }
    match result {
        Ok(Some(lyrics)) => Outcome::Found(Box::new(analyze(song, &lyrics))),
        Ok(None) => Outcome::Miss,
        Err(error) => Outcome::Error(format!("{provider}: {error}")),
    }
}

/// LRCLIB limits how fast it is asked, and a lookup there makes several requests.
static LRCLIB_SLOTS: Semaphore = Semaphore::const_new(3);

async fn run_query(client: &'static reqwest::Client, mut song: Song, query: Query) -> Row {
    let (title, artist, duration_ms) =
        (query.title.clone(), query.artist.clone(), query.duration_ms);
    // The lyrics are judged against the length the player reported.
    match query.mutation {
        Mutation::TvSizeTitle => song.edited = true,
        Mutation::LongerBy1s
        | Mutation::ShorterBy1s
        | Mutation::LongerBy2s
        | Mutation::ShorterBy2s => song.duration_ms = duration_ms,
        _ => {}
    }
    let (lrc_red, lrclib) = tokio::join!(
        lookup(&song, "lrc.red", || {
            let (title, artist) = (title.clone(), artist.clone());
            Box::pin(async move {
                within_deadline(
                    "lrc.red",
                    LRC_RED_TIMEOUT,
                    fetch_lrc_red(client, &title, &artist, Some(duration_ms)),
                )
                .await
            })
        }),
        lookup(&song, "LRCLIB", || {
            let (title, artist) = (title.clone(), artist.clone());
            Box::pin(async move {
                let _slot = LRCLIB_SLOTS.acquire().await.expect("semaphore");
                within_deadline(
                    "LRCLIB",
                    LRCLIB_TIMEOUT,
                    fetch_lrclib(client, &title, &artist, Some(duration_ms)),
                )
                .await
            })
        }),
    );
    let app = Row::choose(&query, &lrc_red, &lrclib);
    let mut row = Row {
        query,
        lrc_red,
        lrclib,
        app,
        agreement: None,
    };
    if let (Outcome::Found(a), Outcome::Found(b)) = (&row.lrc_red, &row.lrclib) {
        // Lyrics in different scripts (one provider has the romaji) cannot be compared.
        let japanese = |found: &Found| found.japanese_lines * 10 >= found.lines;
        let overlap = agreement(&a.text, &b.text);
        if japanese(a) == japanese(b) {
            row.agreement = Some(overlap);
        }
        if row.agreement.is_some_and(|overlap| overlap < MIN_AGREEMENT) {
            let problem = Problem {
                kind: Kind::Disagrees,
                detail: format!("{:.0}% of their lyrics overlap", overlap * 100.0),
            };
            // The same song on both with different words means one is another song.
            for outcome in [&mut row.lrc_red, &mut row.lrclib] {
                if let Outcome::Found(found) = outcome {
                    found.problems.push(problem.clone());
                }
            }
        }
    }
    row
}

fn env_number(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

// ---- Report ----

#[derive(Default, Clone, Copy)]
struct Tally {
    total: usize,
    good: usize,
    wrong: usize,
    miss: usize,
    error: usize,
    synced: usize,
    word_timed: usize,
    /// Japanese songs that were found, and those fully romanized.
    japanese: usize,
    romanized: usize,
}

impl Tally {
    fn add(&mut self, outcome: &Outcome, expects_japanese: bool) {
        self.total += 1;
        match outcome {
            Outcome::Miss => self.miss += 1,
            Outcome::Error(_) => self.error += 1,
            Outcome::Found(found) if found.wrong_song() => self.wrong += 1,
            Outcome::Found(found) => {
                self.good += 1;
                if !found.has(Kind::NotSynced) && !found.has(Kind::ShortSync) {
                    self.synced += 1;
                }
                if found.word_timed {
                    self.word_timed += 1;
                }
                if expects_japanese {
                    self.japanese += 1;
                    if !found.has(Kind::NoRomanization) && !found.has(Kind::PartialRomanization) {
                        self.romanized += 1;
                    }
                }
            }
        }
    }

    fn cells(&self) -> String {
        format!(
            "{} | {} | {} | {} | {} | {}",
            self.total, self.good, self.wrong, self.miss, self.error, self.synced
        )
    }
}

fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Miss => "miss".to_string(),
        Outcome::Error(error) => format!("error ({error})"),
        Outcome::Found(found) => {
            let problems = found
                .problems
                .iter()
                .map(|problem| format!("{}: {}", problem.kind.label(), problem.detail))
                .collect::<Vec<_>>()
                .join("; ");
            format!(
                "\"{}\" by {} ({}s, {} lines{}){}",
                found.track,
                found.artist,
                found.duration_s.map_or("?".to_string(), |s| s.to_string()),
                found.lines,
                if found.word_timed { ", word-timed" } else { "" },
                if problems.is_empty() {
                    String::new()
                } else {
                    format!(" <- {problems}")
                },
            )
        }
    }
}

fn query_label(song: &Song, query: &Query) -> String {
    format!(
        "`{}` / `{}` ({}s) [{}, {:?}]",
        query.title.trim(),
        query.artist.trim(),
        query.duration_ms / 1_000,
        song.category,
        query.mutation
    )
}

fn row_json(songs: &[Song], row: &Row) -> Value {
    let provider = |outcome: &Outcome| match outcome {
        Outcome::Miss => json!({ "status": "miss" }),
        Outcome::Error(error) => json!({ "status": "error", "error": error }),
        Outcome::Found(found) => json!({
            "status": "found",
            "track": found.track,
            "artist": found.artist,
            "album": found.album,
            "durationS": found.duration_s,
            "lines": found.lines,
            "wordTimed": found.word_timed,
            "japaneseLines": found.japanese_lines,
            "romanizedLines": found.romanized_lines,
            "problems": found.problems.iter()
                .map(|problem| format!("{}: {}", problem.kind.label(), problem.detail))
                .collect::<Vec<_>>(),
        }),
    };
    json!({
        "category": songs[row.query.song].category,
        "mutation": format!("{:?}", row.query.mutation),
        "title": row.query.title,
        "artist": row.query.artist,
        "durationMs": row.query.duration_ms,
        "agreement": row.agreement,
        "lrcRed": provider(&row.lrc_red),
        "lrclib": provider(&row.lrclib),
    })
}

fn report(songs: &[Song], rows: &[Row]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Lyrics fuzz report\n");
    let _ = writeln!(
        out,
        "{} lookups of {} songs. `good` = the lyrics are the song's; `wrong` = they probably are not; `synced` = good and timed line by line over the whole song.\n",
        rows.len(),
        rows.iter()
            .map(|row| row.query.song)
            .collect::<HashSet<_>>()
            .len()
    );

    // Coverage by category, from the queries as the catalog names the song.
    let _ = writeln!(out, "## Coverage (catalog spellings)\n");
    let _ = writeln!(
        out,
        "| category | provider | n | good | wrong | miss | error | synced | word-timed | romanized |\n|---|---|---|---|---|---|---|---|---|---|"
    );
    let mut tallies: BTreeMap<(String, &str), Tally> = BTreeMap::new();
    for row in rows
        .iter()
        .filter(|row| row.query.mutation == Mutation::AsIs)
    {
        let song = &songs[row.query.song];
        let japanese = song.expects_japanese() == Some(true);
        for (provider, outcome) in [
            ("lrc.red", &row.lrc_red),
            ("LRCLIB", &row.lrclib),
            ("app", row.app()),
        ] {
            tallies
                .entry((song.category.clone(), provider))
                .or_default()
                .add(outcome, japanese);
        }
    }
    for ((category, provider), tally) in &tallies {
        let romanized = if tally.japanese > 0 {
            format!("{}/{}", tally.romanized, tally.japanese)
        } else {
            "-".to_string()
        };
        let _ = writeln!(
            out,
            "| {category} | {provider} | {} | {} | {romanized} |",
            tally.cells(),
            tally.word_timed,
        );
    }

    // Quirks: the answer must not change from the catalog spelling's.
    let _ = writeln!(
        out,
        "\n## Player quirks (answer must match the plain query)\n"
    );
    let _ = writeln!(
        out,
        "| quirk | n | same song | lost it | other song | wrong |\n|---|---|---|---|---|---|"
    );
    let mut quirks: BTreeMap<Mutation, [usize; 5]> = BTreeMap::new();
    let mut divergences = Vec::new();
    for row in rows
        .iter()
        .filter(|row| row.query.mutation != Mutation::AsIs)
    {
        let base = rows.iter().find(|base| {
            base.query.song == row.query.song
                && base.query.variant == row.query.variant
                && base.query.mutation == Mutation::AsIs
        });
        let Some(base) = base else { continue };
        let slot = quirks.entry(row.query.mutation).or_default();
        slot[0] += 1;
        match (base.app(), row.app()) {
            (Outcome::Found(_), Outcome::Found(b)) if b.wrong_song() => {
                slot[4] += 1;
                divergences.push((row, format!("wrong song: {}", describe(row.app()))));
            }
            (Outcome::Found(a), Outcome::Found(b)) => {
                // The TV size and the length quirks may legitimately differ in length.
                let same_text = !a.text.is_empty()
                    && agreement(&a.text, &b.text) >= MIN_AGREEMENT
                    && (a.japanese_lines * 10 >= a.lines) == (b.japanese_lines * 10 >= b.lines);
                if (a.track == b.track && a.artist == b.artist) || same_text {
                    slot[1] += 1;
                } else {
                    slot[3] += 1;
                    divergences.push((
                        row,
                        format!(
                            "plain query gave {}, this gave {}",
                            describe(base.app()),
                            describe(row.app())
                        ),
                    ));
                }
            }
            (Outcome::Found(_), Outcome::Miss) => {
                slot[2] += 1;
                divergences.push((
                    row,
                    format!("lost: plain query gave {}", describe(base.app())),
                ));
            }
            (Outcome::Miss, Outcome::Found(b)) if b.wrong_song() => slot[4] += 1,
            _ => slot[1] += 1,
        }
    }
    for (mutation, [n, same, lost, other, wrong]) in &quirks {
        // `same` also counts the cases where nothing was found either way.
        let _ = writeln!(
            out,
            "| {mutation:?} | {n} | {same} | {lost} | {other} | {wrong} |"
        );
    }

    let section = |out: &mut String, title: &str, items: Vec<String>| {
        let _ = writeln!(out, "\n## {title} ({})\n", items.len());
        for item in items {
            let _ = writeln!(out, "- {item}");
        }
    };

    let app_rows = |keep: &dyn Fn(&Row, &Outcome) -> bool,
                    label: &dyn Fn(&Row, &Outcome) -> String| {
        rows.iter()
            .filter(|row| keep(row, row.app()))
            .map(|row| label(row, row.app()))
            .collect::<Vec<_>>()
    };

    section(
        &mut out,
        "App shows another song's lyrics",
        app_rows(
            &|_, outcome| matches!(outcome, Outcome::Found(found) if found.wrong_song()),
            &|row, outcome| {
                format!(
                    "{} -> {}",
                    query_label(&songs[row.query.song], &row.query),
                    describe(outcome)
                )
            },
        ),
    );
    section(
        &mut out,
        "App finds no lyrics",
        app_rows(&|_, outcome| matches!(outcome, Outcome::Miss), &|row, _| {
            format!(
                "{} (lrc.red: {}; LRCLIB: {})",
                query_label(&songs[row.query.song], &row.query),
                describe(&row.lrc_red),
                describe(&row.lrclib)
            )
        }),
    );
    section(
        &mut out,
        "Japanese songs without a full romanization",
        app_rows(
            &|row, outcome| {
                songs[row.query.song].expects_japanese() == Some(true)
                    && matches!(outcome, Outcome::Found(found)
                        if !found.wrong_song()
                            && (found.has(Kind::NoRomanization)
                                || found.has(Kind::PartialRomanization)
                                || found.has(Kind::DirtyRomanization)))
            },
            &|row, outcome| {
                format!(
                    "{} -> {}",
                    query_label(&songs[row.query.song], &row.query),
                    describe(outcome)
                )
            },
        ),
    );
    section(
        &mut out,
        "Songs found but not well synced",
        app_rows(
            &|_, outcome| {
                matches!(outcome, Outcome::Found(found)
                    if !found.wrong_song() && (found.has(Kind::NotSynced) || found.has(Kind::ShortSync)))
            },
            &|row, outcome| {
                format!(
                    "{} -> {}",
                    query_label(&songs[row.query.song], &row.query),
                    describe(outcome)
                )
            },
        ),
    );
    section(
        &mut out,
        "Lookups that errored (after a retry)",
        rows.iter()
            .flat_map(|row| {
                [("lrc.red", &row.lrc_red), ("LRCLIB", &row.lrclib)]
                    .into_iter()
                    .filter(|(_, outcome)| matches!(outcome, Outcome::Error(_)))
                    .map(move |(_, outcome)| {
                        format!(
                            "{} -> {}",
                            query_label(&songs[row.query.song], &row.query),
                            describe(outcome)
                        )
                    })
            })
            .collect(),
    );
    section(
        &mut out,
        "A quirk changed the answer",
        divergences
            .into_iter()
            .map(|(row, what)| {
                format!(
                    "{}: {what}",
                    query_label(&songs[row.query.song], &row.query)
                )
            })
            .collect(),
    );
    section(
        &mut out,
        "Plain-query answers worth a look (only like the title, or providers disagree)",
        rows.iter()
            .filter(|row| row.query.mutation == Mutation::AsIs)
            .flat_map(|row| {
                [("lrc.red", &row.lrc_red), ("LRCLIB", &row.lrclib)]
                    .into_iter()
                    .filter(|(_, outcome)| {
                        matches!(outcome, Outcome::Found(found)
                        if found.has(Kind::LooseTitle)
                            || found.has(Kind::Disagrees)
                            || found.has(Kind::OddArtist)
                            || found.has(Kind::UnverifiedTitle)
                            || found.has(Kind::LatinLyrics))
                    })
                    .map(move |(provider, outcome)| {
                        format!(
                            "{provider}: {} -> {}",
                            query_label(&songs[row.query.song], &row.query),
                            describe(outcome)
                        )
                    })
            })
            .collect(),
    );
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "queries lrc.red and LRCLIB for hundreds of songs; run with `bun run fuzz:lyrics`"]
async fn live_fuzz() {
    let songs = corpus();
    let filter = std::env::var("LYRICS_FUZZ_FILTER").ok();
    let queries = plan(
        &songs,
        filter.as_deref(),
        env_number("LYRICS_FUZZ_MUTANTS", 2),
    );
    assert!(!queries.is_empty(), "no song matches the filter");
    let client = http_client().expect("http client");
    let permits = Arc::new(Semaphore::new(env_number("LYRICS_FUZZ_CONCURRENCY", 3)));

    eprintln!("fuzzing {} lookups", queries.len());
    let total = queries.len();
    let mut tasks = JoinSet::new();
    for query in queries {
        let permits = permits.clone();
        let song = songs[query.song].clone();
        tasks.spawn(async move {
            let _permit = permits.acquire_owned().await.expect("semaphore");
            run_query(client, song, query).await
        });
    }
    let mut rows = Vec::with_capacity(total);
    while let Some(row) = tasks.join_next().await {
        rows.push(row.expect("lookup task"));
        if rows.len() % 25 == 0 {
            eprintln!("  {}/{total}", rows.len());
        }
    }
    rows.sort_by_key(|row| (row.query.song, row.query.variant, row.query.mutation));

    let report = report(&songs, &rows);
    let directory = std::path::PathBuf::from(
        std::env::var("LYRICS_FUZZ_OUT").unwrap_or_else(|_| "target/lyrics-fuzz".to_string()),
    );
    std::fs::create_dir_all(&directory).expect("report directory");
    std::fs::write(directory.join("report.md"), &report).expect("write report");
    std::fs::write(
        directory.join("rows.json"),
        serde_json::to_string_pretty(
            &rows
                .iter()
                .map(|row| row_json(&songs, row))
                .collect::<Vec<_>>(),
        )
        .expect("rows serialize"),
    )
    .expect("write rows");
    println!("{report}");
    println!("report written to {}", directory.display());

    if std::env::var("LYRICS_FUZZ_STRICT").is_ok_and(|value| value == "1") {
        let wrong = rows
            .iter()
            .filter(|row| matches!(row.app(), Outcome::Found(found) if found.wrong_song()))
            .count();
        assert_eq!(wrong, 0, "{wrong} lookups would show another song's lyrics");
    }
}

/// Looks one song up and prints what each provider's search finds and what the
/// app would pick: `LYRICS_PROBE='Title|Artist|256000' bun run probe:lyrics`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "queries lrc.red and LRCLIB for one song; run with `bun run probe:lyrics`"]
async fn probe() {
    let Ok(spec) = std::env::var("LYRICS_PROBE") else {
        eprintln!("set LYRICS_PROBE='Title|Artist|durationMs'");
        return;
    };
    let mut parts = spec.split('|');
    let title = parts.next().unwrap_or_default();
    let artist = parts.next().unwrap_or_default();
    let duration_ms = parts.next().and_then(|ms| ms.trim().parse::<u64>().ok());
    let client = http_client().expect("http client");
    let describe = |found: &Lookup| match found {
        Ok(Some(lyrics)) => format!(
            "{} - {} ({} lines, {} romanized, word-timed: {})",
            lyrics.track_name,
            lyrics.artist_name,
            lyrics.lines.len(),
            lyrics
                .lines
                .iter()
                .filter(|line| line.romanized.is_some())
                .count(),
            lyrics.word_timed,
        ),
        Ok(None) => "nothing".to_string(),
        Err(error) => format!("error: {error}"),
    };
    println!("lrc.red candidates:");
    match lrc_red_matches(client, title, artist, duration_ms).await {
        Ok(ranked) => {
            for (candidate, isrc) in ranked.iter().take(8) {
                println!(
                    "  {isrc} {:?} / {:?} ({:?}s)",
                    candidate.track_name, candidate.artist_name, candidate.duration
                );
            }
        }
        Err(error) => println!("  error: {error}"),
    }
    println!(
        "lrc.red answer: {}",
        describe(&fetch_lrc_red(client, title, artist, duration_ms).await)
    );
    println!(
        "LRCLIB answer: {}",
        describe(&fetch_lrclib(client, title, artist, duration_ms).await)
    );
    let request_id = 1_000_000;
    crate::LATEST_LYRICS_REQUEST.store(request_id, std::sync::atomic::Ordering::Release);
    println!(
        "app answer: {}",
        describe(&fetch_lyrics(title, artist, duration_ms, request_id).await)
    );
}

#[cfg(test)]
mod offline {
    use super::*;

    fn song(category: &str, title: &str, artist: &str, duration_ms: u64) -> Song {
        Song {
            category: category.to_string(),
            duration_ms,
            expect_japanese: None,
            edited: false,
            variants: vec![Variant {
                title: title.to_string(),
                artist: artist.to_string(),
            }],
        }
    }

    fn line(start_ms: u64, text: &str, romanized: Option<&str>) -> Line {
        let segment = |text: &str| Segment {
            start_ms,
            end_ms: start_ms + 1_000,
            text: text.to_string(),
        };
        Line {
            start_ms,
            end_ms: start_ms + 1_000,
            voice: 0,
            text: LineText {
                segments: vec![segment(text)],
                background: Vec::new(),
            },
            romanized: romanized.map(|text| LineText {
                segments: vec![segment(text)],
                background: Vec::new(),
            }),
            translation: None,
        }
    }

    fn result(track: &str, duration: u64, lines: Vec<Line>) -> LyricsResult {
        LyricsResult {
            track_name: track.to_string(),
            artist_name: String::new(),
            album_name: String::new(),
            duration: Some(duration),
            word_timed: false,
            lines,
        }
    }

    #[test]
    fn the_corpus_is_wellformed() {
        let songs = corpus();
        assert!(songs.len() >= 100, "a corpus this small finds little");
        for song in &songs {
            assert!(!song.variants.is_empty());
            assert!((30_000..900_000).contains(&song.duration_ms), "{song:?}");
            assert!(
                matches!(
                    song.category.as_str(),
                    "jp-kanji"
                        | "jp-latin"
                        | "jp-feat"
                        | "jp-tv"
                        | "it-title"
                        | "en-feat"
                        | "other-lang"
                ),
                "{song:?}"
            );
            for variant in &song.variants {
                assert!(!variant.title.trim().is_empty() && !variant.artist.trim().is_empty());
            }
        }
        for category in ["jp-kanji", "jp-latin", "jp-feat", "it-title", "en-feat"] {
            assert!(
                songs.iter().any(|song| song.category == category),
                "{category}"
            );
        }
    }

    #[test]
    fn the_plan_covers_every_quirk_and_every_variant() {
        let songs = corpus();
        let queries = plan(&songs, None, 2);
        let variants = songs.iter().map(|song| song.variants.len()).sum::<usize>();
        let plain = queries
            .iter()
            .filter(|query| query.mutation == Mutation::AsIs)
            .count();
        assert_eq!(plain, variants);
        for mutation in MUTATIONS {
            assert!(
                queries.iter().any(|query| query.mutation == mutation),
                "{mutation:?} is never exercised"
            );
        }
        assert!(plan(&songs, None, 0).len() == variants);
        assert!(plan(&songs, Some("no such song anywhere"), 2).is_empty());
    }

    #[test]
    fn quirks_rewrite_the_tags_the_way_players_do() {
        let variant = Variant {
            title: "Felicità".to_string(),
            artist: "Al Bano & Romina Power".to_string(),
        };
        let query = |mutation| mutate(mutation, &variant, &[], 193_000).expect("applies");

        assert_eq!(
            query(Mutation::FeatTitle).title,
            "Felicità (feat. Guest Artist)"
        );
        assert_eq!(
            query(Mutation::JoinedArtist).artist,
            "Al Bano & Romina Power & Guest Artist"
        );
        assert_eq!(
            query(Mutation::ArtistInTitle).title,
            "Al Bano & Romina Power - Felicità"
        );
        assert_eq!(query(Mutation::TvSizeTitle).duration_ms, 77_200);
        assert_eq!(query(Mutation::LongerBy1s).duration_ms, 194_000);
        assert_eq!(query(Mutation::DecomposedAccents).title, "Felicita\u{300}");
        assert!(mutate(Mutation::CurlyApostrophe, &variant, &[], 1).is_none());
        let japanese = Variant {
            title: "夜に駆ける".to_string(),
            artist: "YOASOBI".to_string(),
        };
        assert!(mutate(Mutation::DecomposedAccents, &japanese, &[], 1).is_none());
        assert!(mutate(Mutation::DecomposedKana, &japanese, &[], 1).is_none());
        let voiced = Variant {
            title: "ドライフラワー".to_string(),
            artist: "ぱぴぷ".to_string(),
        };
        let kana = mutate(Mutation::DecomposedKana, &voiced, &[], 1).unwrap();
        assert_eq!(kana.title, "ト\u{3099}ライフラワー");
        assert_eq!(kana.artist, "は\u{309A}ひ\u{309A}ふ\u{309A}");
    }

    #[test]
    fn titles_are_judged_against_every_spelling_of_the_song() {
        let mut yonezu = song("jp-latin", "Lemon", "Kenshi Yonezu", 256_000);
        yonezu.variants.push(Variant {
            title: "Lemon".to_string(),
            artist: "米津玄師".to_string(),
        });
        let one_percent = song(
            "jp-feat",
            "1% Low Battery Girl (feat. ChiliChill)",
            "Sān-Z & HOYO-MiX",
            209_000,
        );

        assert_eq!(title_match("Lemon", &yonezu), TitleMatch::Exact);
        assert_eq!(title_match("Lemon (Live)", &yonezu), TitleMatch::Loose);
        assert_eq!(title_match("KICK BACK", &yonezu), TitleMatch::Unrelated);
        assert_eq!(title_match("檸檬", &yonezu), TitleMatch::Unverified);
        assert_eq!(
            title_match("1% Low Battery Girl", &one_percent),
            TitleMatch::Exact
        );
    }

    #[test]
    fn a_song_is_judged_on_length_title_language_sync_and_romanization() {
        let japanese = song("jp-kanji", "夜に駆ける", "YOASOBI", 261_000);
        let lines = (0..10)
            .map(|index| {
                line(
                    index * 25_000,
                    "沈むように溶けてゆくように",
                    Some("shizumu you ni tokete yuku you ni"),
                )
            })
            .collect::<Vec<_>>();

        let good = analyze(&japanese, &result("夜に駆ける", 261, lines.clone()));
        assert!(good.problems.is_empty(), "{:?}", good.problems);
        assert_eq!((good.japanese_lines, good.romanized_lines), (10, 10));

        let bad_guy = LyricsResult {
            artist_name: "Billie Eilish".to_string(),
            ..result("Bad Guy", 194, lines.clone())
        };
        let other_song = analyze(&japanese, &bad_guy);
        assert!(other_song.has(Kind::WrongLength) && other_song.has(Kind::WrongTitle));
        assert!(other_song.wrong_song());

        let bare = lines
            .iter()
            .map(|line| Line {
                romanized: None,
                ..line.clone()
            })
            .collect::<Vec<_>>();
        let unromanized = analyze(&japanese, &result("夜に駆ける", 261, bare));
        assert!(unromanized.has(Kind::NoRomanization) && !unromanized.wrong_song());

        let italian = song("it-title", "Soldi", "Mahmood", 195_000);
        let wrong_language = analyze(&italian, &result("Soldi", 195, lines));
        assert!(wrong_language.has(Kind::WrongLanguage));

        let short = analyze(
            &italian,
            &result(
                "Soldi",
                195,
                vec![line(1_000, "Ciao", None), line(5_000, "Come stai", None)],
            ),
        );
        assert!(short.has(Kind::NotSynced));
    }

    #[test]
    fn leftover_kana_in_a_romanization_is_dirty() {
        let japanese = song("jp-kanji", "夜に駆ける", "YOASOBI", 40_000);
        let lines = (0..5)
            .map(|index| line(index * 9_000, "沈むように", Some("shizumu よう ni")))
            .collect::<Vec<_>>();
        assert!(analyze(&japanese, &result("夜に駆ける", 40, lines)).has(Kind::DirtyRomanization));
    }

    #[test]
    fn lyrics_of_one_song_overlap_and_those_of_two_songs_do_not() {
        let one = "shizumuyounitoketeyukuyouni";
        assert!(agreement(one, one) > 0.99);
        assert!(agreement(one, "ilovetodancewithyoutonight") < MIN_AGREEMENT);
        assert_eq!(agreement("", ""), 0.0);
    }
}
