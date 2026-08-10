//! overview — the mindmap pane, from a transcript or a page.
//!
//! Both halves of this app produce **scored text with positions**: OCR gives
//! regions with corners, transcription gives segments with times. That is one
//! shape, so the overview is written once against it and serves both — which is
//! why `Transcriber` was given the same result shape as `Ocr` rather than a
//! convenient one.
//!
//! ## What an overview is for
//!
//! A wall of transcript is unreadable the moment it exceeds a screen. The pane
//! answers "what is in here" before you read any of it, which is the same job
//! voiceterm's mindmap does for a graph — and it emits a `nifty-core` `GraphDoc`
//! so the existing layout and prezi-walk code apply unchanged instead of a
//! second graph engine growing here.
//!
//! ## Topics come from repetition, not from a model
//!
//! No summariser, no weights, no network. Terms that recur across *separate
//! parts* of the material are the topics; a word repeated ten times in one
//! sentence is emphasis, not a topic. Spreading the count over buckets is what
//! separates the two, and it is why this is worth doing at all rather than
//! showing the first line of each paragraph.

use std::collections::HashMap;

/// One unit of material — a transcript segment or an OCR region.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub text: String,
    /// Position along the material: milliseconds for audio, reading order for a
    /// page. Only the ordering matters, which is what lets one type serve both.
    pub at: u64,
}

impl Item {
    pub fn new(text: &str, at: u64) -> Item { Item { text: text.into(), at } }
}

/// A topic and where it appears.
#[derive(Clone, Debug, PartialEq)]
pub struct Topic {
    pub term: String,
    /// How many separate parts of the material mention it.
    pub spread: usize,
    pub mentions: usize,
    /// `at` of the first mention, so a click can jump there.
    pub first_at: u64,
}

/// Words carrying no topic signal.
///
/// Deliberately short. A long list starts removing domain words — "record",
/// "file", "state" are stopwords in general English and topics in this
/// monorepo — so it covers only grammar.
const STOP: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "then", "so", "of", "to", "in",
    "on", "at", "for", "with", "is", "are", "was", "were", "be", "been", "it",
    "this", "that", "these", "those", "i", "you", "we", "they", "he", "she",
    "as", "by", "from", "not", "no", "yes", "do", "does", "did", "have", "has",
    "had", "will", "would", "can", "could", "should", "there", "here", "what",
    "just", "like", "know", "think", "really", "okay", "right", "well", "um",
    "uh", "yeah",
];

fn words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| w.len() >= 4)
        .map(|w| w.to_lowercase())
        .filter(|w| !STOP.contains(&w.as_str()))
        .collect()
}

/// Find the topics.
///
/// `buckets` is how many parts the material is divided into for the spread
/// measure. More buckets is stricter about what counts as recurring.
pub fn topics(items: &[Item], buckets: usize, limit: usize) -> Vec<Topic> {
    if items.is_empty() { return Vec::new(); }
    let buckets = buckets.max(1);
    let lo = items.iter().map(|i| i.at).min().unwrap_or(0);
    let hi = items.iter().map(|i| i.at).max().unwrap_or(lo);
    let span = (hi - lo).max(1);

    // term -> (bucket set, total mentions, first at)
    let mut seen: HashMap<String, (Vec<bool>, usize, u64)> = HashMap::new();
    for item in items {
        let b = (((item.at - lo) as u128 * buckets as u128) / span as u128) as usize;
        let b = b.min(buckets - 1);
        for w in words(&item.text) {
            let e = seen.entry(w).or_insert_with(|| (vec![false; buckets], 0, item.at));
            e.0[b] = true;
            e.1 += 1;
            e.2 = e.2.min(item.at);
        }
    }

    let mut out: Vec<Topic> = seen.into_iter()
        .map(|(term, (bs, mentions, first_at))| Topic {
            term,
            spread: bs.iter().filter(|b| **b).count(),
            mentions,
            first_at,
        })
        // A term confined to one bucket is emphasis, not a topic.
        .filter(|t| t.spread >= 2)
        .collect();

    // Spread first, mentions second, then the term itself — the last key breaks
    // ties deterministically so the pane does not reshuffle between frames.
    out.sort_by(|a, b| b.spread.cmp(&a.spread)
        .then(b.mentions.cmp(&a.mentions))
        .then(a.term.cmp(&b.term)));
    out.truncate(limit);
    out
}

/// Render topics as a niftyplot source.
///
/// Emitting source rather than a laid-out graph keeps this pure and lets the
/// existing parser, layout and prezi-walk apply unchanged — and the text is
/// something a user can copy into a diagram tool.
pub fn to_niftyplot(title: &str, ts: &[Topic]) -> String {
    let mut s = String::from("graph\n");
    let safe = |x: &str| x.replace(['"', '\n'], " ");
    s.push_str(&format!("title \"{}\"\n", safe(title)));
    s.push_str("root [label=\"Overview\"]\n");
    for (i, t) in ts.iter().enumerate() {
        s.push_str(&format!("t{} [label=\"{}\"]\n", i, safe(&t.term)));
        s.push_str(&format!("root --> t{}\n", i));
    }
    s
}

/// Parse that source into the shared graph document.
pub fn to_graph(title: &str, ts: &[Topic]) -> nifty_core::niftyplot::GraphDoc {
    nifty_core::niftyplot::parse(&to_niftyplot(title, ts))
}

/// Items from a transcript.
pub fn from_segments(segs: &[crate::audio::Segment]) -> Vec<Item> {
    segs.iter().map(|s| Item::new(&s.text, s.start_ms)).collect()
}

/// Items from a scanned page, in reading order.
///
/// Position is the reading index rather than a pixel coordinate: only ordering
/// matters to the spread measure, and using `y` would make a two-column page
/// look like two halves of a recording.
pub fn from_regions(regions: &[crate::Region]) -> Vec<Item> {
    crate::reading_order(regions).into_iter().enumerate()
        .map(|(i, r)| Item::new(&regions[r].text, i as u64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Segment;

    fn talk() -> Vec<Item> {
        vec![
            Item::new("welcome to the meeting about the routing table", 0),
            Item::new("the routing table needs a replacement cache", 10_000),
            Item::new("nothing much happens here", 20_000),
            Item::new("back to the routing table and its buckets", 30_000),
            Item::new("buckets again, and the replacement cache", 40_000),
        ]
    }

    #[test]
    fn a_recurring_term_becomes_a_topic() {
        let t = topics(&talk(), 4, 10);
        assert!(t.iter().any(|x| x.term == "routing"), "topics: {t:?}");
        assert!(t.iter().any(|x| x.term == "table"));
    }

    #[test]
    fn a_word_repeated_only_in_one_place_is_emphasis_not_a_topic() {
        // The distinction the whole spread measure exists for: ten mentions in
        // one sentence is somebody making a point, not a subject of the talk.
        let items = vec![
            Item::new("kubernetes kubernetes kubernetes kubernetes kubernetes", 0),
            Item::new("something else entirely", 10_000),
            Item::new("another unrelated thing", 20_000),
        ];
        let t = topics(&items, 3, 10);
        assert!(!t.iter().any(|x| x.term == "kubernetes"),
                "confined to one bucket, so not a topic: {t:?}");
    }

    #[test]
    fn topics_are_ordered_by_spread_then_mentions_and_are_stable() {
        // A pane that reshuffles between frames is a pane you mis-tap.
        let a = topics(&talk(), 4, 10);
        let b = topics(&talk(), 4, 10);
        assert_eq!(a, b);
        for w in a.windows(2) {
            assert!(w[0].spread >= w[1].spread, "spread descending: {a:?}");
        }
    }

    #[test]
    fn a_topic_remembers_where_it_first_appeared_so_a_click_can_jump() {
        let t = topics(&talk(), 4, 10);
        let routing = t.iter().find(|x| x.term == "routing").expect("routing");
        assert_eq!(routing.first_at, 0);
    }

    #[test]
    fn grammar_words_are_dropped_but_domain_words_are_not() {
        // The stoplist is deliberately short: "record", "file" and "state" are
        // stopwords in general English and topics in this monorepo.
        let items = vec![
            Item::new("the state of the record file", 0),
            Item::new("that state and this record file", 10_000),
        ];
        let t = topics(&items, 2, 10);
        let terms: Vec<&str> = t.iter().map(|x| x.term.as_str()).collect();
        assert!(terms.contains(&"state") && terms.contains(&"record"));
        assert!(!terms.contains(&"that") && !terms.contains(&"this"));
    }

    #[test]
    fn the_same_overview_code_serves_audio_and_a_scanned_page() {
        // The reason Transcriber was given Ocr's result shape rather than a
        // convenient one.
        let segs = vec![
            Segment { start_ms: 0, end_ms: 1, text: "routing table".into(), score: 0.9 },
            Segment { start_ms: 9_000, end_ms: 1, text: "routing buckets".into(), score: 0.9 },
        ];
        assert_eq!(from_segments(&segs).len(), 2);

        let regions = vec![
            crate::Region { text: "routing table".into(), score: 0.9,
                            corners: [(0.0, 0.0), (100.0, 0.0), (100.0, 20.0), (0.0, 20.0)] },
            crate::Region { text: "routing buckets".into(), score: 0.9,
                            corners: [(0.0, 40.0), (100.0, 40.0), (100.0, 60.0), (0.0, 60.0)] },
        ];
        let items = from_regions(&regions);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].at, 0, "position is reading order, not pixels");
        assert_eq!(items[1].at, 1);

        assert!(topics(&items, 2, 5).iter().any(|t| t.term == "routing"));
    }

    #[test]
    fn a_niftyplot_source_parses_into_a_graph() {
        let t = topics(&talk(), 4, 5);
        let doc = to_graph("Meeting", &t);
        assert!(doc.nodes.len() >= 2, "root plus topics: {}", doc.nodes.len());
        assert!(!doc.edges.is_empty());
    }

    #[test]
    fn quotes_in_a_topic_cannot_break_the_generated_source() {
        // A term carrying a quote would otherwise terminate the label early and
        // produce a source that parses into something else entirely.
        let ts = vec![Topic { term: "say \"what\"".into(), spread: 2, mentions: 2, first_at: 0 }];
        let src = to_niftyplot("A \"title\"", &ts);
        assert_eq!(src.matches('"').count() % 2, 0, "quotes balanced: {src}");
    }

    #[test]
    fn empty_material_produces_no_topics_rather_than_panicking() {
        assert!(topics(&[], 4, 10).is_empty());
        assert!(topics(&[Item::new("", 0)], 4, 10).is_empty());
    }

    #[test]
    fn one_item_cannot_spread_across_buckets() {
        // Everything lands in one bucket, so nothing reaches spread >= 2. That
        // is correct: a single sentence has no topics, it has words.
        assert!(topics(&[Item::new("routing table routing table", 0)], 4, 10).is_empty());
    }
}
