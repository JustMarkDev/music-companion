//! The lyrics model the overlay displays, and the parser that builds it from
//! the TTML files lrc.red serves.

use roxmltree::{Document, Node};
use serde::Serialize;
use std::collections::HashMap;

/// One timed piece of a line: a word, or a syllable when the source splits words.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Includes the space that follows it, so segments concatenate to the line.
    pub text: String,
}

/// A line written in one script: its lead vocal and any background vocals.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineText {
    pub segments: Vec<Segment>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub background: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub start_ms: u64,
    pub end_ms: u64,
    /// 0 for the lead singer, 1 for any other singer of a duet.
    pub voice: u8,
    #[serde(flatten)]
    pub text: LineText,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub romanized: Option<LineText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translation: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
}

impl Lyrics {
    /// True when some line times two or more separate words. A song whose lines
    /// are only timed as a whole is shown line by line.
    pub fn is_word_timed(&self) -> bool {
        self.lines.iter().any(|line| line.text.segments.len() >= 2)
    }
}

/// One timed span of a TTML line. Adjacent spans without whitespace between
/// them are syllables of the same word.
struct Span {
    begin_ms: u64,
    end_ms: Option<u64>,
    text: String,
    space_after: bool,
    background: bool,
}

/// Parses an lrc.red TTML file. The transliteration and translation tracks of
/// its metadata are attached to the lines they belong to. `None` when the file
/// is not TTML or has no lyrics.
pub fn parse(ttml: &str) -> Option<Lyrics> {
    let document = Document::parse(ttml).ok()?;
    let transliterations = tracks(&document, "transliteration");
    let translations = tracks(&document, "translation");
    let mut singers = Singers::new(&document);

    let mut lines = document
        .descendants()
        .filter(|node| node.has_tag_name("p"))
        .filter_map(|paragraph| {
            let start_ms = paragraph.attribute("begin").and_then(parse_time)?;
            let end_ms = paragraph.attribute("end").and_then(parse_time);
            let mut text = line_text(paragraph, start_ms, end_ms);
            if text.segments.is_empty() && text.background.is_empty() {
                return None;
            }
            // A line of background vocals only is still a lead line.
            if text.segments.is_empty() {
                std::mem::swap(&mut text.segments, &mut text.background);
            }
            let end_ms = end_ms
                .into_iter()
                .chain(
                    text.segments
                        .iter()
                        .chain(&text.background)
                        .map(|s| s.end_ms),
                )
                .max()
                .unwrap_or(start_ms);

            let key = attribute_named(paragraph, "key");
            let romanized = key
                .and_then(|key| transliterations.get(key))
                .map(|node| line_text(*node, start_ms, Some(end_ms)))
                .filter(|text| !text.segments.is_empty())
                .map(capitalize_first_letter);
            let translation = key
                .and_then(|key| translations.get(key))
                .map(|node| collapse_whitespace(&all_text(*node)))
                .filter(|text| !text.is_empty());

            let agent = paragraph
                .ancestors()
                .find_map(|node| attribute_named(node, "agent"));
            let line = Line {
                start_ms,
                end_ms,
                voice: 0,
                text,
                romanized,
                translation,
            };
            Some((line, agent))
        })
        .collect::<Vec<_>>();

    // Voices are given in time order, so that the singer heard first leads even
    // when the file lists its lines in another order.
    lines.sort_by_key(|(line, _)| line.start_ms);
    let lines = lines
        .into_iter()
        .map(|(mut line, agent)| {
            line.voice = singers.voice(agent);
            line
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then_some(Lyrics { lines })
}

/// The `<text for="…">` entries of a metadata track (`transliteration` or
/// `translation`), by the key of the line each belongs to. Only the first track
/// is read.
fn tracks<'a, 'input>(
    document: &'a Document<'input>,
    name: &str,
) -> HashMap<&'a str, Node<'a, 'input>> {
    document
        .descendants()
        .find(|node| node.has_tag_name(name))
        .into_iter()
        .flat_map(|track| track.children())
        .filter(|node| node.has_tag_name("text"))
        .filter_map(|node| Some((node.attribute("for")?, node)))
        .collect()
}

/// The value of the attribute called `name` whatever its namespace prefix
/// (`lrc:key`, `ttm:agent`, …).
fn attribute_named<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|attribute| attribute.name() == name)
        .map(|attribute| attribute.value())
}

/// Tells which singer of a duet a line belongs to.
struct Singers<'a> {
    groups: Vec<&'a str>,
    lead: Option<&'a str>,
}

impl<'a> Singers<'a> {
    /// The lead singer is the first person the file declares. Groups sing along
    /// with everyone, so they are never a second voice.
    fn new(document: &'a Document) -> Self {
        let declared = document
            .descendants()
            .filter(|node| node.has_tag_name("agent"))
            .filter_map(|node| Some((attribute_named(node, "id")?, attribute_named(node, "type"))))
            .collect::<Vec<_>>();
        Self {
            groups: declared
                .iter()
                .filter(|(_, kind)| *kind == Some("group"))
                .map(|(id, _)| *id)
                .collect(),
            lead: declared
                .iter()
                .find(|(_, kind)| *kind != Some("group"))
                .map(|(id, _)| *id),
        }
    }

    /// The voice of a line sung by `agent`, which the line sets or inherits from
    /// its section or the body. Lines must come in time order.
    fn voice(&mut self, agent: Option<&'a str>) -> u8 {
        let Some(agent) = agent else {
            return 0;
        };
        if self.groups.contains(&agent) {
            return 0;
        }
        // Without declarations the first singer heard is the lead.
        let lead = *self.lead.get_or_insert(agent);
        u8::from(agent != lead)
    }
}

/// The timed words of `node` (a line, or one entry of a transliteration) and its
/// background vocals. A node without timed spans is one segment spanning the
/// line.
fn line_text(node: Node, start_ms: u64, end_ms: Option<u64>) -> LineText {
    let mut spans = Vec::new();
    collect_spans(node, false, &mut spans);

    if spans.is_empty() {
        let text = collapse_whitespace(&all_text(node));
        let segments = if text.is_empty() {
            Vec::new()
        } else {
            vec![Segment {
                start_ms,
                end_ms: end_ms.unwrap_or(start_ms).max(start_ms),
                text,
            }]
        };
        return LineText {
            segments,
            background: Vec::new(),
        };
    }

    let line_end = end_ms.unwrap_or_else(|| {
        spans
            .iter()
            .map(|span| span.end_ms.unwrap_or(span.begin_ms))
            .max()
            .unwrap_or(start_ms)
    });
    let (background, main): (Vec<_>, Vec<_>) = spans.into_iter().partition(|span| span.background);
    LineText {
        segments: segments(main, line_end),
        background: segments(background, line_end),
    }
}

fn segments(spans: Vec<Span>, line_end_ms: u64) -> Vec<Segment> {
    let starts = spans.iter().map(|span| span.begin_ms).collect::<Vec<_>>();
    let last = spans.len().saturating_sub(1);
    spans
        .into_iter()
        .enumerate()
        .map(|(index, span)| {
            let end_ms = span
                .end_ms
                .or_else(|| starts.get(index + 1).copied())
                .unwrap_or(line_end_ms)
                .max(span.begin_ms);
            let mut text = span.text;
            if span.space_after && index != last {
                text.push(' ');
            }
            Segment {
                start_ms: span.begin_ms,
                end_ms,
                text,
            }
        })
        .collect()
}

/// Collects the timed spans below `node`, descending into spans that wrap
/// others and marking those inside background vocals.
fn collect_spans(node: Node, background: bool, spans: &mut Vec<Span>) {
    for child in node.children() {
        if child.is_element() {
            let background = background || attribute_named(child, "role") == Some("x-bg");
            if child.children().any(|grandchild| grandchild.is_element()) {
                collect_spans(child, background, spans);
            } else if let Some(begin_ms) = child.attribute("begin").and_then(parse_time) {
                let text = child.text().unwrap_or_default();
                if text.starts_with(char::is_whitespace)
                    && let Some(previous) = spans.last_mut()
                {
                    previous.space_after = true;
                }
                if !text.trim().is_empty() {
                    spans.push(Span {
                        begin_ms,
                        end_ms: child.attribute("end").and_then(parse_time),
                        text: collapse_whitespace(text),
                        space_after: text.ends_with(char::is_whitespace),
                        background,
                    });
                }
            }
        } else if child
            .text()
            .is_some_and(|text| text.contains(char::is_whitespace))
            && let Some(previous) = spans.last_mut()
        {
            previous.space_after = true;
        }
    }
}

fn all_text(node: Node) -> String {
    node.descendants()
        .filter(|node| node.is_text())
        .filter_map(|node| node.text())
        .collect()
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Milliseconds from a TTML clock value such as `29.188`, `1:00.249` or
/// `0:01:02.5`.
fn parse_time(value: &str) -> Option<u64> {
    let seconds = value
        .trim()
        .trim_end_matches('s')
        .split(':')
        .try_fold(0.0, |total, part| {
            Some(total * 60.0 + part.parse::<f64>().ok()?)
        })?;
    (seconds.is_finite() && seconds >= 0.0).then(|| (seconds * 1_000.0).round() as u64)
}

/// Capitalizes the first letter of a romanized line, the way a sentence starts.
fn capitalize_first_letter(mut text: LineText) -> LineText {
    if let Some(first) = text.segments.first_mut()
        && let Some((index, character)) = first
            .text
            .char_indices()
            .find(|(_, character)| character.is_alphabetic())
    {
        let uppercase = character.to_uppercase().collect::<String>();
        first
            .text
            .replace_range(index..index + character.len_utf8(), &uppercase);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:lrc="http://lrc.red/lyric-ttml-internal" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">"#;

    fn segment(start_ms: u64, end_ms: u64, text: &str) -> Segment {
        Segment {
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    fn texts(segments: &[Segment]) -> Vec<&str> {
        segments.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn word_timed_lines_become_segments_with_their_spacing() {
        let ttml = format!(
            r#"{HEAD}<body><div><p begin="1.5" end="4" lrc:key="L1"><span begin="1.5" end="2">Some</span><span begin="2" end="2.5">one</span> <span begin="2.5" end="3.2">said</span></p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        assert!(lyrics.is_word_timed());
        let line = &lyrics.lines[0];
        assert_eq!((line.start_ms, line.end_ms), (1_500, 4_000));
        assert_eq!(texts(&line.text.segments), ["Some", "one ", "said"]);
        assert_eq!(line.text.segments[1], segment(2_000, 2_500, "one "));
    }

    #[test]
    fn line_timed_lyrics_are_one_segment_per_line() {
        let ttml = format!(
            r#"{HEAD}<body><div><p begin="0:01.0" end="0:03.5">Hello   there</p><p begin="1:00.25" end="1:02">Bye</p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        assert!(!lyrics.is_word_timed());
        assert_eq!(
            lyrics.lines[0].text.segments,
            [segment(1_000, 3_500, "Hello there")]
        );
        assert_eq!(lyrics.lines[1].start_ms, 60_250);
    }

    #[test]
    fn a_span_without_an_end_ends_where_the_next_one_starts() {
        let ttml = format!(
            r#"{HEAD}<body><div><p begin="1" end="5"><span begin="1">A</span> <span begin="2">B</span></p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        let segments = &lyrics.lines[0].text.segments;
        assert_eq!((segments[0].end_ms, segments[1].end_ms), (2_000, 5_000));
    }

    #[test]
    fn the_transliteration_keeps_its_own_word_timing_and_starts_with_a_capital() {
        let ttml = format!(
            r#"{HEAD}<head><metadata><transliterations><transliteration xml:lang="ja-Latn"><text for="L1"><span begin="1" end="2" xmlns="http://www.w3.org/ns/ttml">yume</span> <span begin="2" end="3" xmlns="http://www.w3.org/ns/ttml">nara</span></text></transliteration></transliterations></metadata></head><body><div><p begin="1" end="3" lrc:key="L1"><span begin="1" end="2">夢</span><span begin="2" end="3">なら</span></p><p begin="4" end="6" lrc:key="L2"><span begin="4" end="6">Hello</span></p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        let romanized = lyrics.lines[0].romanized.as_ref().unwrap();
        assert_eq!(texts(&romanized.segments), ["Yume ", "nara"]);
        assert_eq!(romanized.segments[1].start_ms, 2_000);
        assert_eq!(texts(&lyrics.lines[0].text.segments), ["夢", "なら"]);
        // A line the track skips has no romanization.
        assert!(lyrics.lines[1].romanized.is_none());
    }

    #[test]
    fn a_plain_transliteration_spans_the_whole_line() {
        let ttml = format!(
            r#"{HEAD}<head><metadata><transliterations><transliteration xml:lang="ko-Latn"><text for="L1">annyeong</text></transliteration></transliterations></metadata></head><body><div><p begin="2.5" end="4" lrc:key="L1">안녕</p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        assert_eq!(
            lyrics.lines[0].romanized.as_ref().unwrap().segments,
            [segment(2_500, 4_000, "Annyeong")]
        );
    }

    #[test]
    fn translations_are_attached_by_line_key() {
        let ttml = format!(
            r#"{HEAD}<head><metadata><translations><translation type="subtitle" xml:lang="en-US"><text for="L2">  Second   line </text></translation></translations></metadata></head><body><div><p begin="1" end="2" lrc:key="L1">Uno</p><p begin="3" end="4" lrc:key="L2">Dos</p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        assert_eq!(lyrics.lines[0].translation, None);
        assert_eq!(lyrics.lines[1].translation.as_deref(), Some("Second line"));
    }

    #[test]
    fn background_vocals_are_kept_apart_from_the_lead_line() {
        let ttml = format!(
            r#"{HEAD}<body><div><p begin="1" end="6"><span begin="1" end="2">Lead</span> <span begin="2" end="3">line</span> <span ttm:role="x-bg"><span begin="3" end="4">(Sube,</span> <span begin="4" end="6">sube)</span></span></p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        let line = &lyrics.lines[0];
        assert_eq!(texts(&line.text.segments), ["Lead ", "line"]);
        assert_eq!(texts(&line.text.background), ["(Sube, ", "sube)"]);
        assert_eq!(line.text.background[0].start_ms, 3_000);
        assert_eq!(line.end_ms, 6_000);
    }

    #[test]
    fn a_line_of_background_vocals_only_is_shown_as_the_line() {
        let ttml = format!(
            r#"{HEAD}<body><div><p begin="1" end="3"><span ttm:role="x-bg"><span begin="1" end="2">Oh</span> <span begin="2" end="3">yeah</span></span></p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        assert_eq!(texts(&lyrics.lines[0].text.segments), ["Oh ", "yeah"]);
        assert!(lyrics.lines[0].text.background.is_empty());
    }

    #[test]
    fn the_second_singer_of_a_duet_has_the_other_voice() {
        let ttml = format!(
            r#"{HEAD}<head><metadata><ttm:agent type="person" xml:id="v1"/><ttm:agent type="person" xml:id="v2"/><ttm:agent type="group" xml:id="v1000"/></metadata></head><body ttm:agent="v1000"><div ttm:agent="v1"><p begin="1" end="2" ttm:agent="v1">One</p><p begin="3" end="4" ttm:agent="v2">Two</p><p begin="5" end="6" ttm:agent="v1000">Both</p><p begin="7" end="8">Inherited</p></div></body></tt>"#
        );

        let voices = parse(&ttml)
            .unwrap()
            .lines
            .iter()
            .map(|line| line.voice)
            .collect::<Vec<_>>();

        assert_eq!(voices, [0, 1, 0, 0]);
    }

    #[test]
    fn without_declared_singers_the_first_one_heard_leads() {
        let ttml = format!(
            r#"{HEAD}<body><div><p begin="1" end="2" ttm:agent="v2">One</p><p begin="3" end="4" ttm:agent="v3">Two</p></div></body></tt>"#
        );

        let voices = parse(&ttml)
            .unwrap()
            .lines
            .iter()
            .map(|line| line.voice)
            .collect::<Vec<_>>();

        assert_eq!(voices, [0, 1]);
    }

    #[test]
    fn the_singer_heard_first_leads_even_when_the_file_lists_lines_out_of_order() {
        let ttml = format!(
            r#"{HEAD}<body><div><p begin="3" end="4" ttm:agent="v3">Second</p><p begin="1" end="2" ttm:agent="v2">First</p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        assert_eq!(texts(&lyrics.lines[0].text.segments), ["First"]);
        assert_eq!(
            lyrics
                .lines
                .iter()
                .map(|line| line.voice)
                .collect::<Vec<_>>(),
            [0, 1]
        );
    }

    #[test]
    fn files_without_lyrics_are_rejected() {
        assert!(parse("not xml").is_none());
        assert!(parse(&format!(r#"{HEAD}<body><div></div></body></tt>"#)).is_none());
        assert!(
            parse(&format!(
                r#"{HEAD}<body><div><p begin="1" end="2">  </p></div></body></tt>"#
            ))
            .is_none()
        );
    }

    #[test]
    fn lines_are_ordered_by_their_start() {
        let ttml = format!(
            r#"{HEAD}<body><div><p begin="5" end="6">Later</p><p begin="1" end="2">Sooner</p></div></body></tt>"#
        );

        let lyrics = parse(&ttml).unwrap();

        assert_eq!(texts(&lyrics.lines[0].text.segments), ["Sooner"]);
    }

    #[test]
    fn clock_values_are_read_in_every_form() {
        assert_eq!(parse_time("29.188"), Some(29_188));
        assert_eq!(parse_time("1:00.249"), Some(60_249));
        assert_eq!(parse_time("0:01:02.5"), Some(62_500));
        assert_eq!(parse_time("12s"), Some(12_000));
        assert_eq!(parse_time("soon"), None);
    }

    /// A song of `count` word-timed lines with a transliteration and a translation.
    fn sample_ttml(count: usize) -> String {
        let words = ["Line", "with", "a", "few", "more", "words", "to", "sing"];
        let spans = |offset: f64| {
            words
                .iter()
                .enumerate()
                .map(|(index, word)| {
                    let begin = offset + index as f64 * 0.3;
                    format!(
                        r#"<span begin="{begin:.3}" end="{:.3}">{word}</span> "#,
                        begin + 0.3
                    )
                })
                .collect::<String>()
        };
        let mut metadata = String::new();
        let mut body = String::new();
        for index in 0..count {
            let offset = index as f64 * 2.4;
            metadata.push_str(&format!(r#"<text for="L{index}">{}</text>"#, spans(offset)));
            body.push_str(&format!(
                r#"<p begin="{offset:.3}" end="{:.3}" lrc:key="L{index}" ttm:agent="v{}">{}</p>"#,
                offset + 2.4,
                index % 2 + 1,
                spans(offset)
            ));
        }
        format!(
            r#"{HEAD}<head><metadata><transliterations><transliteration xml:lang="ja-Latn">{metadata}</transliteration></transliterations></metadata></head><body><div>{body}</div></body></tt>"#
        )
    }

    #[test]
    #[ignore = "curriculum bench; run via bun run bench:rust"]
    fn curriculum_metric_parse_ttml_full_song() {
        let ttml = sample_ttml(80);
        crate::bench_support::report_ops_per_sec("ttml_parse_full_song", || {
            assert_eq!(parse(&ttml).unwrap().lines.len(), 80);
        });
    }
}
