//! Study features layered on top of the reader: highlights with margin
//! notes, a lightweight spaced-repetition flashcard system, cloze quiz
//! generation from highlights, citation string generation, and export to
//! Markdown/JSON.

use crate::content::{BookContent, Locus, LocusRange};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HighlightColor {
    Yellow,
    Green,
    Blue,
    Pink,
    Purple,
}

impl HighlightColor {
    pub fn all() -> &'static [HighlightColor] {
        &[
            HighlightColor::Yellow,
            HighlightColor::Green,
            HighlightColor::Blue,
            HighlightColor::Pink,
            HighlightColor::Purple,
        ]
    }
    pub fn rgb(&self) -> (u8, u8, u8) {
        match self {
            HighlightColor::Yellow => (250, 224, 108),
            HighlightColor::Green => (149, 213, 148),
            HighlightColor::Blue => (137, 190, 240),
            HighlightColor::Pink => (240, 160, 197),
            HighlightColor::Purple => (191, 158, 235),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Highlight {
    pub id: u64,
    pub range: LocusRange,
    pub color: HighlightColor,
    pub note: String,
    pub excerpt: String,
    pub created_unix: i64,
    pub tags: Vec<String>,
}

impl Highlight {
    pub fn new(id: u64, range: LocusRange, color: HighlightColor, excerpt: String) -> Self {
        Self {
            id,
            range,
            color,
            note: String::new(),
            excerpt,
            created_unix: crate::util::now_unix(),
            tags: Vec::new(),
        }
    }
}

/// The SM-2 spaced-repetition algorithm, simplified: no separate
/// "learning steps" phase, just ease-factor + interval growth. Good
/// enough for a reader's built-in flashcards without pulling in a full
/// SRS crate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Flashcard {
    pub id: u64,
    pub front: String,
    pub back: String,
    pub source_locus: Option<Locus>,
    pub ease: f32,
    pub interval_days: f32,
    pub repetitions: u32,
    pub due_unix: i64,
    pub last_reviewed_unix: Option<i64>,
}

impl Flashcard {
    pub fn new(id: u64, front: String, back: String, source_locus: Option<Locus>) -> Self {
        Self {
            id,
            front,
            back,
            source_locus,
            ease: 2.5,
            interval_days: 0.0,
            repetitions: 0,
            due_unix: crate::util::now_unix(),
            last_reviewed_unix: None,
        }
    }

    pub fn is_due(&self, now_unix: i64) -> bool {
        self.due_unix <= now_unix
    }

    /// Grade 0..=5 per the classic SM-2 scale (0 = total blackout, 5 =
    /// perfect recall). Updates ease/interval/due date in place.
    pub fn review(&mut self, grade: u8) {
        let grade = grade.min(5);
        let now = crate::util::now_unix();
        self.last_reviewed_unix = Some(now);

        if grade < 3 {
            self.repetitions = 0;
            self.interval_days = 1.0;
        } else {
            self.repetitions += 1;
            self.interval_days = match self.repetitions {
                1 => 1.0,
                2 => 6.0,
                _ => self.interval_days * self.ease,
            };
        }

        let g = grade as f32;
        self.ease = (self.ease + (0.1 - (5.0 - g) * (0.08 + (5.0 - g) * 0.02))).max(1.3);
        self.due_unix = now + (self.interval_days * 86_400.0) as i64;
    }
}

/// Deck of flashcards with simple querying helpers used by the study UI.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Deck {
    pub cards: Vec<Flashcard>,
}

impl Deck {
    pub fn due_cards(&self, now_unix: i64) -> Vec<&Flashcard> {
        self.cards.iter().filter(|c| c.is_due(now_unix)).collect()
    }
    pub fn due_count(&self, now_unix: i64) -> usize {
        self.cards.iter().filter(|c| c.is_due(now_unix)).count()
    }
    pub fn add(&mut self, card: Flashcard) {
        self.cards.push(card);
    }
    pub fn remove(&mut self, id: u64) {
        self.cards.retain(|c| c.id != id);
    }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut Flashcard> {
        self.cards.iter_mut().find(|c| c.id == id)
    }
}

/// Generate a cloze-deletion quiz item from a highlight's excerpt: picks
/// a salient word (longest non-stopword) and blanks it out.
pub fn cloze_from_excerpt(excerpt: &str) -> Option<(String, String)> {
    const STOPWORDS: &[&str] = &[
        "the", "and", "that", "with", "this", "from", "have", "which", "were",
        "their", "would", "there", "about", "these", "those", "into", "when",
        "your", "will", "such", "than", "them", "then",
    ];
    let words: Vec<&str> = excerpt.split_whitespace().collect();
    let mut best: Option<(usize, &str)> = None;
    for (i, w) in words.iter().enumerate() {
        let clean: String = w.chars().filter(|c| c.is_alphanumeric()).collect();
        if clean.len() < 4 {
            continue;
        }
        if STOPWORDS.contains(&clean.to_lowercase().as_str()) {
            continue;
        }
        if best.map(|(_, bw)| clean.len() > bw.len()).unwrap_or(true) {
            best = Some((i, w));
        }
    }
    let (idx, answer) = best?;
    let front = words
        .iter()
        .enumerate()
        .map(|(i, w)| if i == idx { "_____" } else { *w })
        .collect::<Vec<_>>()
        .join(" ");
    Some((front, answer.to_string()))
}

/// Turn a batch of highlights into flashcards via cloze generation,
/// skipping any excerpt too short to produce a good blank.
pub fn flashcards_from_highlights(highlights: &[Highlight], next_id: &mut u64) -> Vec<Flashcard> {
    let mut out = Vec::new();
    for h in highlights {
        if let Some((front, back)) = cloze_from_excerpt(&h.excerpt) {
            let id = *next_id;
            *next_id += 1;
            out.push(Flashcard::new(id, front, back, Some(h.range.from)));
        }
    }
    out
}

/// Citation style for bibliography generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CitationStyle {
    Apa,
    Mla,
    BibTex,
    Chicago,
}

pub struct CitationInfo<'a> {
    pub title: &'a str,
    pub authors: &'a [String],
    pub year: Option<i32>,
    pub publisher: Option<&'a str>,
}

/// Format a citation string. This is a reasonable approximation of each
/// style's rules for a whole-book citation, not a full CSL engine.
pub fn format_citation(info: &CitationInfo, style: CitationStyle) -> String {
    let author_str_apa = |authors: &[String]| -> String {
        match authors.len() {
            0 => "Unknown".to_string(),
            1 => last_name_first(&authors[0]),
            2 => format!("{} & {}", last_name_first(&authors[0]), last_name_first(&authors[1])),
            _ => format!("{} et al.", last_name_first(&authors[0])),
        }
    };
    match style {
        CitationStyle::Apa => {
            let year = info.year.map(|y| y.to_string()).unwrap_or_else(|| "n.d.".into());
            format!(
                "{} ({}). {}. {}.",
                author_str_apa(info.authors),
                year,
                info.title,
                info.publisher.unwrap_or("")
            )
        }
        CitationStyle::Mla => {
            let author = info
                .authors
                .first()
                .map(|a| last_name_first(a))
                .unwrap_or_else(|| "Unknown".into());
            format!(
                "{}. {}. {}, {}.",
                author,
                info.title,
                info.publisher.unwrap_or(""),
                info.year.map(|y| y.to_string()).unwrap_or_default()
            )
        }
        CitationStyle::Chicago => {
            let author = info
                .authors
                .first()
                .map(|a| last_name_first(a))
                .unwrap_or_else(|| "Unknown".into());
            format!(
                "{}. {}. {}: {}, {}.",
                author,
                info.title,
                info.publisher.unwrap_or(""),
                info.publisher.unwrap_or(""),
                info.year.map(|y| y.to_string()).unwrap_or_default()
            )
        }
        CitationStyle::BibTex => {
            let key = format!(
                "{}{}",
                info.authors
                    .first()
                    .map(|a| crate::util::slugify(a))
                    .unwrap_or_else(|| "unknown".into()),
                info.year.unwrap_or(0)
            );
            let authors_bib = info.authors.join(" and ");
            format!(
                "@book{{{key},\n  title = {{{}}},\n  author = {{{}}},\n  publisher = {{{}}},\n  year = {{{}}}\n}}",
                info.title,
                authors_bib,
                info.publisher.unwrap_or(""),
                info.year.map(|y| y.to_string()).unwrap_or_default()
            )
        }
    }
}

fn last_name_first(full: &str) -> String {
    let parts: Vec<&str> = full.split_whitespace().collect();
    if parts.len() < 2 {
        return full.to_string();
    }
    let last = parts.last().unwrap();
    let rest = parts[..parts.len() - 1].join(" ");
    format!("{last}, {rest}")
}

/// Export highlights + notes as Markdown, grouped in document order.
pub fn export_highlights_markdown(book_title: &str, highlights: &[Highlight]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Highlights — {book_title}\n\n"));
    let mut sorted = highlights.to_vec();
    sorted.sort_by_key(|h| (h.range.from.block, h.range.from.ch));
    for h in &sorted {
        out.push_str(&format!("> {}\n", h.excerpt.trim()));
        if !h.note.is_empty() {
            out.push_str(&format!("\n**Note:** {}\n", h.note));
        }
        if !h.tags.is_empty() {
            out.push_str(&format!("\n_Tags: {}_\n", h.tags.join(", ")));
        }
        out.push_str("\n---\n\n");
    }
    out
}

/// Export highlights + notes as JSON (round-trippable).
pub fn export_highlights_json(highlights: &[Highlight]) -> String {
    serde_json::to_string_pretty(highlights).unwrap_or_else(|_| "[]".to_string())
}

/// A pomodoro-style focus timer for study sessions. Pure state machine —
/// the UI layer is responsible for polling `tick` each frame.
#[derive(Clone, Debug)]
pub struct PomodoroTimer {
    pub focus_secs: u32,
    pub break_secs: u32,
    pub remaining_secs: f32,
    pub on_break: bool,
    pub running: bool,
    pub completed_sessions: u32,
}

impl Default for PomodoroTimer {
    fn default() -> Self {
        Self {
            focus_secs: 25 * 60,
            break_secs: 5 * 60,
            remaining_secs: (25 * 60) as f32,
            on_break: false,
            running: false,
            completed_sessions: 0,
        }
    }
}

impl PomodoroTimer {
    pub fn tick(&mut self, dt: f32) -> bool {
        if !self.running {
            return false;
        }
        self.remaining_secs -= dt;
        if self.remaining_secs <= 0.0 {
            if self.on_break {
                self.on_break = false;
                self.remaining_secs = self.focus_secs as f32;
            } else {
                self.completed_sessions += 1;
                self.on_break = true;
                self.remaining_secs = self.break_secs as f32;
            }
            return true; // phase changed
        }
        false
    }

    pub fn label(&self) -> String {
        let secs = self.remaining_secs.max(0.0) as u32;
        format!("{:02}:{:02}", secs / 60, secs % 60)
    }
}

/// Look up the excerpt text for a highlight range from live content, used
/// when creating a highlight so the stored excerpt matches what's on
/// screen even after future repagination.
pub fn excerpt_for_range(content: &BookContent, range: LocusRange) -> String {
    content.snippet(range, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sm2_review_grows_interval_on_success() {
        let mut card = Flashcard::new(1, "front".into(), "back".into(), None);
        card.review(5);
        let after_first = card.interval_days;
        card.review(5);
        let after_second = card.interval_days;
        card.review(5);
        let after_third = card.interval_days;
        assert!(after_second >= after_first);
        assert!(after_third > after_second);
    }

    #[test]
    fn sm2_review_resets_on_failure() {
        let mut card = Flashcard::new(1, "front".into(), "back".into(), None);
        card.review(5);
        card.review(5);
        card.review(1);
        assert_eq!(card.repetitions, 0);
        assert_eq!(card.interval_days, 1.0);
    }

    #[test]
    fn cloze_picks_longest_content_word() {
        let (front, back) = cloze_from_excerpt("The mitochondria is the powerhouse of the cell").unwrap();
        assert_eq!(back, "mitochondria");
        assert!(front.contains("_____"));
    }

    #[test]
    fn citation_apa_format() {
        let info = CitationInfo {
            title: "Example Book",
            authors: &["Jane Smith".to_string()],
            year: Some(2020),
            publisher: Some("Acme Press"),
        };
        let apa = format_citation(&info, CitationStyle::Apa);
        assert!(apa.contains("Smith, Jane"));
        assert!(apa.contains("2020"));
    }

    #[test]
    fn pomodoro_switches_phase() {
        let mut timer = PomodoroTimer {
            remaining_secs: 1.0,
            running: true,
            ..PomodoroTimer::default()
        };
        let changed = timer.tick(2.0);
        assert!(changed);
        assert!(timer.on_break);
    }
}
