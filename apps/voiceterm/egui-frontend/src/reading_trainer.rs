//! reading_trainer — paced reading over a manpage, with a pace that learns.
//!
//! voiceterm-qt has a reading overlay: paragraphs are selectable, each is
//! given a duration from its length at the current words-per-minute, and
//! code blocks run at a 0.70 multiplier (`qml_gen::maybe_wrap_extended`,
//! `ContentType::Code`). This is that pacing as a pure model the egui
//! manpage view can drive, plus the training loop the Qt side describes
//! but never closed: keep up and the pace rises, fall behind and it eases.
//!
//! * [`Pacer`] — the current paragraph, its budget, elapsed time.
//! * [`Trainer`] — the words-per-minute, adjusted from what happened.
//!
//! Prose is split on blank lines; a paragraph fenced with ``` or indented
//! four spaces is code. The view highlights `Pacer::current` and advances
//! on `tick`; the person advances early (they finished) with `next`, which
//! is what the trainer reads as "kept up".

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType { Prose, Code }

/// Code reads slower: the Qt overlay's multiplier.
pub const CODE_MULTIPLIER: f32 = 0.70;

#[derive(Debug, Clone, PartialEq)]
pub struct Paragraph {
    pub text: String,
    pub kind: ContentType,
}

impl Paragraph {
    pub fn words(&self) -> usize { self.text.split_whitespace().count().max(1) }

    /// Seconds to read at `wpm`.
    pub fn budget_secs(&self, wpm: f32) -> f32 {
        let rate = match self.kind { ContentType::Prose => wpm, ContentType::Code => wpm * CODE_MULTIPLIER };
        (self.words() as f32 / rate.max(1.0)) * 60.0
    }
}

/// Split a manpage's markdown into paragraphs.
pub fn paragraphs(md: &str) -> Vec<Paragraph> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_fence = false;
    let flush = |cur: &mut String, out: &mut Vec<Paragraph>, kind: ContentType| {
        let t = cur.trim();
        if !t.is_empty() { out.push(Paragraph { text: t.to_string(), kind }); }
        cur.clear();
    };
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            if in_fence { flush(&mut cur, &mut out, ContentType::Code); } else { flush(&mut cur, &mut out, ContentType::Prose); }
            in_fence = !in_fence;
            continue;
        }
        if in_fence { cur.push_str(line); cur.push('\n'); continue; }
        if line.trim().is_empty() {
            let kind = if cur.starts_with("    ") { ContentType::Code } else { ContentType::Prose };
            flush(&mut cur, &mut out, kind);
            continue;
        }
        if line.starts_with("    ") {
            // indented code: its own paragraph kind, contiguous lines merge
            if !cur.is_empty() && !cur.starts_with("    ") { flush(&mut cur, &mut out, ContentType::Prose); }
            cur.push_str(line); cur.push('\n');
            continue;
        }
        if cur.starts_with("    ") { flush(&mut cur, &mut out, ContentType::Code); }
        cur.push_str(line.trim()); cur.push(' ');
    }
    let last_kind = if in_fence || cur.starts_with("    ") { ContentType::Code } else { ContentType::Prose };
    flush(&mut cur, &mut out, last_kind);
    out
}

/// Paragraphs from an ALREADY-PARSED html document — what the manpage view
/// holds. `Doc` keeps html, not the markdown `paragraphs` expects, and
/// re-serialising html back to markdown just to re-split it would be two
/// lossy conversions to reach text the parser already has in hand.
///
/// Headings and rules are skipped: a heading is a signpost, not something
/// you read at a pace, and pacing one would spend a paragraph's budget on
/// three words. Code blocks and quotes keep their slower rate.
pub fn paragraphs_from_html(doc: &uikit_web::RichDoc) -> Vec<Paragraph> {
    let mut out = Vec::new();
    for b in &doc.blocks {
        let kind = match b.kind {
            uikit_web::BlockKind::Code => ContentType::Code,
            uikit_web::BlockKind::Heading(_) | uikit_web::BlockKind::Rule => continue,
            _ => ContentType::Prose,
        };
        let text: String = b.inlines.iter().map(|i| i.text.as_str()).collect::<Vec<_>>().join("");
        let text = text.trim().to_string();
        if text.is_empty() { continue; }
        out.push(Paragraph { text, kind });
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pacer {
    pub paragraphs: Vec<Paragraph>,
    pub current: usize,
    pub elapsed: f32,
    pub running: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome { KeptUp, FellBehind }

impl Pacer {
    pub fn new(md: &str) -> Pacer { Pacer { paragraphs: paragraphs(md), current: 0, elapsed: 0.0, running: false } }

    /// The manpage view's path: it already parsed the html to render it.
    pub fn from_html(doc: &uikit_web::RichDoc) -> Pacer {
        Pacer { paragraphs: paragraphs_from_html(doc), current: 0, elapsed: 0.0, running: false }
    }

    pub fn done(&self) -> bool { self.current >= self.paragraphs.len() }

    /// Fraction of the current paragraph's budget used.
    pub fn progress(&self, wpm: f32) -> f32 {
        match self.paragraphs.get(self.current) {
            Some(p) => (self.elapsed / p.budget_secs(wpm)).clamp(0.0, 1.0),
            None => 1.0,
        }
    }

    /// Time passes. When the budget runs out the paragraph advances on its
    /// own and that counts as falling behind.
    pub fn tick(&mut self, dt: f32, wpm: f32) -> Option<Outcome> {
        if !self.running || self.done() { return None; }
        self.elapsed += dt;
        let over = self.elapsed >= self.paragraphs[self.current].budget_secs(wpm);
        if over { self.current += 1; self.elapsed = 0.0; Some(Outcome::FellBehind) } else { None }
    }

    /// The reader finished early.
    pub fn next(&mut self) -> Option<Outcome> {
        if self.done() { return None; }
        self.current += 1;
        self.elapsed = 0.0;
        Some(Outcome::KeptUp)
    }

    pub fn back(&mut self) { self.current = self.current.saturating_sub(1); self.elapsed = 0.0; }
}

/// The pace, and how it moves. Up 5% after three kept-up paragraphs in a
/// row, down 10% on any fall — asymmetric on purpose: a pace that climbs
/// as fast as it drops oscillates around the ceiling instead of settling
/// under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Trainer {
    pub wpm: f32,
    pub streak: u32,
    pub best: f32,
}

/// Where a reader starts before the trainer has learned anything: roughly
/// average adult prose reading speed, so the first page is neither a crawl
/// nor a blur and the adjustment has somewhere to go in both directions.
pub const START_WPM: f32 = 240.0;
pub const MIN_WPM: f32 = 80.0;
pub const MAX_WPM: f32 = 1200.0;

impl Default for Trainer {
    fn default() -> Self { Trainer::new(START_WPM) }
}

impl Trainer {
    pub fn new(wpm: f32) -> Trainer { Trainer { wpm: wpm.clamp(MIN_WPM, MAX_WPM), streak: 0, best: 0.0 } }

    pub fn record(&mut self, o: Outcome) {
        match o {
            Outcome::KeptUp => {
                self.streak += 1;
                if self.streak >= 3 {
                    self.streak = 0;
                    self.wpm = (self.wpm * 1.05).min(MAX_WPM);
                    self.best = self.best.max(self.wpm);
                }
            }
            Outcome::FellBehind => {
                self.streak = 0;
                self.wpm = (self.wpm * 0.9).max(MIN_WPM);
            }
        }
    }

    /// `wpm|best` — one line for the settings sidecar.
    pub fn to_line(&self) -> String { format!("{:.0}|{:.0}", self.wpm, self.best) }
    pub fn from_line(s: &str) -> Option<Trainer> {
        let (w, b) = s.trim().split_once('|')?;
        let mut t = Trainer::new(w.parse().ok()?);
        t.best = b.parse().ok()?;
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# ls\n\nList directory contents. Lists files\nand folders.\n\n```\nls -la\n```\n\n    indented code\n    more code\n\nLast words here.";

    #[test]
    fn paragraphs_split_on_blank_lines_and_code_is_marked() {
        let ps = paragraphs(MD);
        let kinds: Vec<ContentType> = ps.iter().map(|p| p.kind).collect();
        assert_eq!(kinds, vec![ContentType::Prose, ContentType::Prose, ContentType::Code, ContentType::Code, ContentType::Prose]);
        assert_eq!(ps[1].text, "List directory contents. Lists files and folders.");
        assert_eq!(ps[1].words(), 7);
        assert_eq!(ps[3].text, "indented code\n    more code");
    }

    #[test]
    fn code_reads_slower_by_the_qt_multiplier() {
        let prose = Paragraph { text: "one two three four five six seven eight nine ten".into(), kind: ContentType::Prose };
        let code = Paragraph { text: prose.text.clone(), kind: ContentType::Code };
        assert!((prose.budget_secs(300.0) - 2.0).abs() < 1e-4);
        assert!((code.budget_secs(300.0) - 2.0 / CODE_MULTIPLIER).abs() < 1e-3);
    }

    #[test]
    fn the_pacer_advances_on_budget_or_on_the_reader_and_reports_which() {
        let mut p = Pacer::new("a b c d e f g h i j\n\nk l");
        p.running = true;
        assert_eq!(p.tick(1.0, 600.0), Some(Outcome::FellBehind), "10 words at 600 wpm = 1 s; the budget ran out");
        assert_eq!(p.current, 1);
        assert_eq!(p.next(), Some(Outcome::KeptUp));
        assert!(p.done());
        assert_eq!(p.tick(1.0, 600.0), None);
        let mut q = Pacer::new("a b c d e f g h i j");
        q.running = true;
        assert_eq!(q.tick(0.5, 600.0), None);
        assert!((q.progress(600.0) - 0.5).abs() < 1e-4);
        assert_eq!(q.tick(0.6, 600.0), Some(Outcome::FellBehind));
    }

    #[test]
    fn the_trainer_climbs_slowly_and_drops_fast_within_bounds() {
        let mut t = Trainer::new(300.0);
        t.record(Outcome::KeptUp); t.record(Outcome::KeptUp);
        assert_eq!(t.wpm, 300.0, "two in a row is not yet a rise");
        t.record(Outcome::KeptUp);
        assert!((t.wpm - 315.0).abs() < 1e-3);
        assert_eq!(t.streak, 0);
        t.record(Outcome::FellBehind);
        assert!((t.wpm - 283.5).abs() < 1e-3);
        let mut lo = Trainer::new(MIN_WPM);
        lo.record(Outcome::FellBehind);
        assert_eq!(lo.wpm, MIN_WPM);
        assert_eq!(Trainer::from_line(&t.to_line()).unwrap().wpm.round(), 284.0);
        assert!(Trainer::from_line("junk").is_none());
    }
    #[test]
    fn html_blocks_become_paced_paragraphs_and_headings_are_skipped() {
        let doc = uikit_web::parse_html(
            "<h1>ls</h1><p>List directory contents.</p><pre>ls -la</pre><hr><p>Last words.</p>");
        let ps = paragraphs_from_html(&doc);
        let kinds: Vec<ContentType> = ps.iter().map(|p| p.kind).collect();
        assert_eq!(kinds, vec![ContentType::Prose, ContentType::Code, ContentType::Prose],
                   "{:?}", ps.iter().map(|p| &p.text).collect::<Vec<_>>());
        assert_eq!(ps[0].text, "List directory contents.");
        assert!(ps.iter().all(|p| !p.text.is_empty()));
        // Code still reads slower than prose of the same length.
        let a = Paragraph { text: "one two three".into(), kind: ContentType::Prose };
        let b = Paragraph { text: "one two three".into(), kind: ContentType::Code };
        assert!(b.budget_secs(300.0) > a.budget_secs(300.0));
    }

    #[test]
    fn a_pacer_from_html_runs_the_same_way_as_one_from_markdown() {
        let doc = uikit_web::parse_html("<p>a b c d e f g h i j</p><p>k l</p>");
        let mut p = Pacer::from_html(&doc);
        assert_eq!(p.paragraphs.len(), 2);
        assert!(!p.done());
        p.running = true;
        assert_eq!(p.tick(1.0, 600.0), Some(Outcome::FellBehind), "10 words at 600 wpm is 1 s");
        assert_eq!(p.current, 1);
        assert_eq!(p.next(), Some(Outcome::KeptUp));
        assert!(p.done());
    }

    #[test]
    fn a_document_with_nothing_readable_is_an_empty_pacer_not_a_panic() {
        let doc = uikit_web::parse_html("<h1>only a heading</h1><hr>");
        let p = Pacer::from_html(&doc);
        assert!(p.paragraphs.is_empty());
        assert!(p.done(), "an empty pacer is finished, not stuck");
        assert_eq!(p.progress(300.0), 1.0);
    }

}
