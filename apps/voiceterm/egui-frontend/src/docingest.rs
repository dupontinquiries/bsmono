//! docingest — taking a captured web page into the zettelkasten.
//!
//! A browser capture ("MarkDownload" and friends) is markdown plus an assets
//! folder, and it arrives damaged in ways that are specific and detectable.
//! From the Fairphone-vs-RedMagic capture in `testdata/`, 1,535 lines:
//!
//! * **101 empty footnote refs** — `[][1]`. The link text was a chip or an
//!   image the extractor could not render, so the sentence reads *"The
//!   **completely dominates** the ."* with both subjects gone.
//! * **34 base64 asset names** — `9k=.10.jpeg`, `2Q==.8.jpeg`. Not filenames,
//!   fragments of data URIs the extractor turned into files.
//! * **44% blank lines** — the layout's whitespace, carried across as
//!   paragraph breaks.
//! * **Broken tables** — cells split across lines, so the pipe table renders
//!   as literal pipes.
//!
//! ## Diagnose, recommend, verify, offer to revert
//!
//! The flow this module implements, in that order, because each step exists to
//! stop the one before it from doing damage:
//!
//! 1. [`diagnose`] scores the document and names the LEVEL of repair it looks
//!    to need. It recommends; it does not choose. Every level stays available.
//! 2. The chosen level runs, producing a new document beside the old one.
//! 3. [`Report::verdict`] re-scores and compares. A repair that made things
//!    WORSE is reported as failed and the original is kept — this is the whole
//!    reason the score is a number rather than a feeling.
//! 4. Whatever the verdict, the original is retained until the operator says
//!    otherwise, so a bad repair is one tap from undone.
//!
//! ## Why a score and not a checklist
//!
//! A checklist says what is wrong. It cannot say whether a fix helped. The
//! step that catches a formatter making a document worse needs a
//! before-and-after comparison, and that needs a single comparable number —
//! so the checks each contribute a weighted penalty and the total is what the
//! verdict compares.

/// What is wrong with a document, and how much.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Damage {
    /// `[][1]` — a reference whose link text was dropped.
    pub empty_refs: usize,
    /// Assets named from base64 fragments rather than content.
    pub base64_assets: usize,
    /// Lines that are blank.
    pub blank_lines: usize,
    pub total_lines: usize,
    /// Table rows whose cell count disagrees with the header's.
    pub ragged_table_rows: usize,
    /// Headings, as a proxy for structure worth keeping.
    pub headings: usize,
}

impl Damage {
    /// A single comparable number, higher is worse.
    ///
    /// Weights are ratios rather than absolutes so a long document is not
    /// automatically judged worse than a short one — the Fairphone capture has
    /// 101 empty refs in 1,535 lines, and a 60-line note with 20 would be in
    /// far worse shape.
    pub fn score(&self) -> f32 {
        if self.total_lines == 0 {
            return 0.0;
        }
        let n = self.total_lines as f32;
        // An empty ref is the worst kind: it removes the subject of a sentence
        // and leaves prose that reads as complete.
        let refs = (self.empty_refs as f32 / n) * 3.0;
        let assets = (self.base64_assets as f32 / n) * 1.0;
        // Blank lines only count past the share ordinary prose has, AND only
        // in a document long enough for the ratio to mean something. A
        // five-line note is 40% blank by construction — one blank between each
        // of three paragraphs — and scoring that as damage told the operator a
        // clean note needed repair. The threshold is 40 lines because that is
        // roughly where a capture stops being a note.
        let blank = if self.total_lines >= 40 {
            ((self.blank_lines as f32 / n) - 0.35).max(0.0) * 2.0
        } else {
            0.0
        };
        let ragged = (self.ragged_table_rows as f32 / n) * 2.0;
        refs + assets + blank + ragged
    }

    pub fn is_clean(&self) -> bool {
        self.score() < 0.02
    }
}

/// How much repair to attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Change nothing. Always offered: a capture kept verbatim is a valid
    /// choice, and for a quotation it is the only correct one.
    None,
    /// Whitespace and asset names. Nothing that changes words.
    Tidy,
    /// Tidy, plus rebuilding tables and dropping dead refs.
    Structural,
    /// Structural, plus a local model summarising and re-sectioning in chunks.
    /// The only level that can invent text, which is why it is never the
    /// automatic recommendation.
    Assisted,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::None => "keep as captured",
            Level::Tidy => "tidy whitespace and asset names",
            Level::Structural => "rebuild tables and drop dead links",
            Level::Assisted => "local AI pass, in chunks",
        }
    }

    /// Does this level put words in the document that were not there?
    pub fn can_invent_text(self) -> bool {
        self == Level::Assisted
    }

    pub const ALL: [Level; 4] = [Level::None, Level::Tidy, Level::Structural, Level::Assisted];
}

/// Measure a document.
pub fn diagnose(md: &str) -> Damage {
    let lines: Vec<&str> = md.lines().collect();
    let mut d = Damage {
        total_lines: lines.len(),
        ..Default::default()
    };
    for l in &lines {
        if l.trim().is_empty() {
            d.blank_lines += 1;
        }
        if l.starts_with('#') {
            d.headings += 1;
        }
        d.empty_refs += count_empty_refs(l);
        d.base64_assets += count_base64_assets(l);
    }
    d.ragged_table_rows = ragged_rows(&lines);
    d
}

/// `[][1]` and `[]( )` — a reference with no text.
fn count_empty_refs(line: &str) -> usize {
    let b = line.as_bytes();
    let mut n = 0;
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'[' && b[i + 1] == b']' {
            // Only when something follows: a bare `[]` is a checkbox or
            // literal text, not a broken link.
            if let Some(&c) = b.get(i + 2) {
                if c == b'[' || c == b'(' {
                    n += 1;
                }
            }
        }
        i += 1;
    }
    n
}

/// Assets named from base64 fragments: `9k=.10.jpeg`, `2Q==.8.jpeg`,
/// `A5QRtBOKThn1AAAAAElFTkSuQmCC.png`.
fn count_base64_assets(line: &str) -> usize {
    line.split(|c: char| c == '(' || c == ' ' || c == '/')
        .filter(|tok| {
            let stem = tok.split('.').next().unwrap_or("");
            stem.len() >= 2
                && (stem.ends_with('=')
                    || (stem.len() >= 16
                        && stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/')
                        && stem.chars().any(|c| c.is_ascii_uppercase())
                        && stem.chars().any(|c| c.is_ascii_lowercase())
                        && stem.chars().any(|c| c.is_ascii_digit())))
        })
        .count()
}

/// Rows whose cell count differs from their table's header.
fn ragged_rows(lines: &[&str]) -> usize {
    let mut n = 0;
    let mut expect: Option<usize> = None;
    for l in lines {
        let t = l.trim();
        if !t.starts_with('|') {
            expect = None;
            continue;
        }
        let cells = t.matches('|').count();
        match expect {
            None => expect = Some(cells),
            Some(e) if cells != e => n += 1,
            _ => {}
        }
    }
    n
}

/// What to recommend, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Recommendation {
    pub level: Level,
    /// One line for the toast. Says what was found, not what will happen.
    pub because: String,
}

/// Choose a level from the damage.
///
/// Assisted is NEVER recommended automatically, however bad the document is.
/// It is the only level that can put words in that were not captured, and a
/// tool that silently reaches for a language model to fix punctuation has
/// exceeded what the operator asked for. It stays on the menu.
pub fn recommend(d: &Damage) -> Recommendation {
    let s = d.score();
    if d.is_clean() {
        return Recommendation {
            level: Level::None,
            because: "nothing obviously wrong — keeping it as captured".into(),
        };
    }
    if d.empty_refs > 0 || d.ragged_table_rows > 0 {
        return Recommendation {
            level: Level::Structural,
            because: format!(
                "{} dead link{}, {} ragged table row{}",
                d.empty_refs,
                if d.empty_refs == 1 { "" } else { "s" },
                d.ragged_table_rows,
                if d.ragged_table_rows == 1 { "" } else { "s" }
            ),
        };
    }
    let _ = s;
    Recommendation {
        level: Level::Tidy,
        because: format!(
            "{}% blank lines, {} oddly named asset{}",
            d.blank_lines * 100 / d.total_lines.max(1),
            d.base64_assets,
            if d.base64_assets == 1 { "" } else { "s" }
        ),
    }
}

/// The outcome of running a level.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub level: Level,
    pub before: Damage,
    pub after: Damage,
}

/// What the operator is told, and what happens to the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Better. The new document is offered, the original retained.
    Improved,
    /// No measurable change. Offered, because "no worse" is a legitimate
    /// outcome for a tidy pass on an already-clean document.
    Unchanged,
    /// WORSE. The repair is discarded and the original kept.
    Failed,
}

impl Report {
    pub fn verdict(&self) -> Verdict {
        let (b, a) = (self.before.score(), self.after.score());
        // A margin, because a formatter that shifts the score by a rounding
        // error has not improved anything and should not claim to.
        if a > b + 1e-4 {
            Verdict::Failed
        } else if b - a > 1e-4 {
            Verdict::Improved
        } else {
            Verdict::Unchanged
        }
    }

    /// The line under the toast.
    pub fn summary(&self) -> String {
        match self.verdict() {
            Verdict::Failed => format!(
                "{} made it worse — kept the original",
                self.level.label()
            ),
            Verdict::Unchanged => format!("{} changed nothing measurable", self.level.label()),
            Verdict::Improved => {
                let fixed = self.before.empty_refs.saturating_sub(self.after.empty_refs);
                format!(
                    "{}: {} dead link{} resolved, {} → {} lines",
                    self.level.label(),
                    fixed,
                    if fixed == 1 { "" } else { "s" },
                    self.before.total_lines,
                    self.after.total_lines
                )
            }
        }
    }

    /// Should the result be kept at all?
    pub fn keep(&self) -> bool {
        self.verdict() != Verdict::Failed
    }
}

/// Where the ingest is in its flow.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// Nothing loaded.
    Idle,
    /// Measured, waiting for the operator to pick a level.
    Diagnosed { damage: Damage, rec: Recommendation },
    /// A level is running.
    Running(Level),
    /// Done, and asking whether to keep it.
    ///
    /// This phase is the point of the whole module: the question is asked
    /// EVERY time, including on success, because a formatter that is right
    /// about the score can still be wrong about the document.
    Confirming(Report),
}

/// The ingest's state.
#[derive(Debug, Clone)]
pub struct Ingest {
    pub phase: Phase,
    /// The document as captured. Held until the operator accepts a repair, so
    /// revert is a move rather than a re-read.
    pub original: String,
    /// The repaired document, while it is being offered.
    pub candidate: Option<String>,
    /// Assets folder beside the markdown, if the capture had one.
    pub asset_count: usize,
}

impl Default for Ingest {
    fn default() -> Self {
        Ingest {
            phase: Phase::Idle,
            original: String::new(),
            candidate: None,
            asset_count: 0,
        }
    }
}

impl Ingest {
    /// Load a captured document and measure it.
    pub fn load(&mut self, md: impl Into<String>, asset_count: usize) {
        self.original = md.into();
        self.candidate = None;
        self.asset_count = asset_count;
        let damage = diagnose(&self.original);
        let rec = recommend(&damage);
        self.phase = Phase::Diagnosed { damage, rec };
    }

    /// The levels to offer, recommended one first.
    ///
    /// All of them, always. The recommendation is a default, not a gate — a
    /// capture kept verbatim is the right answer for a quotation, and the
    /// operator knows which this is and the checker does not.
    pub fn options(&self) -> Vec<Level> {
        let rec = match &self.phase {
            Phase::Diagnosed { rec, .. } => rec.level,
            _ => Level::None,
        };
        let mut v = vec![rec];
        v.extend(Level::ALL.iter().copied().filter(|l| *l != rec));
        v
    }

    /// Run a level. `apply` does the work; this handles the bookkeeping and
    /// the verdict.
    pub fn run(&mut self, level: Level, apply: impl FnOnce(&str) -> String) -> Verdict {
        let before = diagnose(&self.original);
        self.phase = Phase::Running(level);
        let out = apply(&self.original);
        let after = diagnose(&out);
        let report = Report { level, before, after };
        let verdict = report.verdict();
        // A failed repair is not offered. Keeping the candidate around would
        // invite an accept that makes the document worse, and the operator has
        // just been told it failed.
        self.candidate = report.keep().then_some(out);
        self.phase = Phase::Confirming(report);
        verdict
    }

    /// The operator said yes.
    pub fn accept(&mut self) -> Option<String> {
        let out = self.candidate.take()?;
        self.original = out.clone();
        let damage = diagnose(&self.original);
        let rec = recommend(&damage);
        self.phase = Phase::Diagnosed { damage, rec };
        Some(out)
    }

    /// The operator said no, or the repair failed.
    ///
    /// Returns to Diagnosed rather than Idle: the document is still loaded and
    /// still needs a decision, and dropping to Idle would make a rejection
    /// look like a close.
    pub fn revert(&mut self) {
        self.candidate = None;
        let damage = diagnose(&self.original);
        let rec = recommend(&damage);
        self.phase = Phase::Diagnosed { damage, rec };
    }

    /// The question under the result. Formal, because it is asked after every
    /// run and a chatty one wears out.
    pub fn confirm_prompt(&self) -> Option<String> {
        match &self.phase {
            Phase::Confirming(r) if r.keep() => {
                Some(format!("{} — keep this version?", r.summary()))
            }
            Phase::Confirming(r) => Some(r.summary()),
            _ => None,
        }
    }
}

/// Split a document for a model with a bounded context.
///
/// Chunks break on BLANK LINES, never mid-paragraph: a chunk boundary inside a
/// sentence gives the model half a thought and it will complete it wrongly and
/// confidently. A paragraph longer than the budget is emitted whole and
/// oversized rather than cut — the caller can refuse it, and a silently
/// truncated paragraph is worse than a visible one.
pub fn chunk_for_model(md: &str, budget_chars: usize) -> Vec<String> {
    if budget_chars == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    for para in md.split("\n\n") {
        if cur.is_empty() {
            cur = para.to_string();
        } else if cur.len() + 2 + para.len() <= budget_chars {
            cur.push_str("\n\n");
            cur.push_str(para);
        } else {
            out.push(std::mem::take(&mut cur));
            cur = para.to_string();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real capture: a Google AI-mode results page, 1,535 lines.
    const CAPTURE: &str = include_str!("../testdata/capture-google-search.md");

    #[test]
    fn the_real_capture_is_diagnosed_as_structurally_damaged() {
        // Not a synthetic fixture: this is what a browser capture of a search
        // page actually looks like, and the numbers are what the detectors
        // must find in it.
        let d = diagnose(CAPTURE);
        assert!(d.empty_refs >= 90, "empty refs: {}", d.empty_refs);
        assert!(d.base64_assets >= 20, "base64 assets: {}", d.base64_assets);
        assert!(d.total_lines > 1000);
        assert!(!d.is_clean());

        let r = recommend(&d);
        assert_eq!(r.level, Level::Structural, "{}", r.because);
        assert!(r.because.contains("dead link"), "{}", r.because);
    }

    #[test]
    fn an_ai_pass_is_never_recommended_on_its_own() {
        // Assisted is the only level that can put words in the document that
        // were not captured. A tool that silently reaches for a language model
        // to fix punctuation has exceeded what was asked — however bad the
        // input is.
        let awful = Damage {
            empty_refs: 500, base64_assets: 200, blank_lines: 900,
            total_lines: 1000, ragged_table_rows: 300, headings: 0,
        };
        assert_ne!(recommend(&awful).level, Level::Assisted);
        assert!(Level::Assisted.can_invent_text());
        assert!(!Level::Structural.can_invent_text());
        // But it is always offered.
        let mut i = Ingest::default();
        i.load(CAPTURE, 34);
        assert!(i.options().contains(&Level::Assisted));
        assert_eq!(i.options().len(), Level::ALL.len(), "every level stays available");
        assert_eq!(i.options()[0], Level::Structural, "the recommendation comes first");
    }

    #[test]
    fn a_repair_that_makes_it_worse_is_refused_and_the_original_kept() {
        // The step the whole flow exists for. A formatter can be confidently
        // wrong, and only a before-and-after score catches it.
        let mut i = Ingest::default();
        i.load("# Title\n\nA line with [][1] a dead ref.\n", 0);
        let before = i.original.clone();
        let v = i.run(Level::Tidy, |_| {
            // A "repair" that adds damage.
            "# Title\n\n[][1] [][2] [][3]\n\n\n\n\n\n".to_string()
        });
        assert_eq!(v, Verdict::Failed);
        assert!(i.candidate.is_none(), "a failed repair must not be offerable");
        assert_eq!(i.original, before, "the original is untouched");
        assert!(i.confirm_prompt().unwrap().contains("made it worse"));
    }

    #[test]
    fn a_real_improvement_is_offered_and_can_be_reverted() {
        let mut i = Ingest::default();
        i.load("# T\n\nsee [][1] and [][2]\n\n\n\n\n", 0);
        let original = i.original.clone();
        let v = i.run(Level::Structural, |_| "# T\n\nsee the spec\n".to_string());
        assert_eq!(v, Verdict::Improved);
        assert!(i.confirm_prompt().unwrap().contains("keep this version?"),
                "the question is asked even on success");

        // Reverting leaves the document as captured and still awaiting a
        // decision — not closed, which would read as "done".
        i.revert();
        assert_eq!(i.original, original);
        assert!(i.candidate.is_none());
        assert!(matches!(i.phase, Phase::Diagnosed { .. }));
    }

    #[test]
    fn accepting_re_diagnoses_so_a_second_pass_knows_what_is_left() {
        let mut i = Ingest::default();
        i.load("# T\n\nsee [][1]\n\n\n\n\n\n\n", 0);
        i.run(Level::Structural, |_| "# T\n\nsee the spec\n\n\n\n\n\n\n".to_string());
        let kept = i.accept().expect("an improvement is keepable");
        assert_eq!(i.original, kept);
        match &i.phase {
            Phase::Diagnosed { damage, .. } => {
                assert_eq!(damage.empty_refs, 0, "the fixed damage is gone from the new score");
            }
            p => panic!("expected Diagnosed after accept, got {p:?}"),
        }
    }

    #[test]
    fn the_score_is_a_ratio_so_length_alone_is_not_damage() {
        // 101 dead refs in 1,535 lines is bad; 20 in 60 lines is far worse,
        // and an absolute count would rank them the other way.
        let long = Damage { empty_refs: 101, total_lines: 1535, ..Default::default() };
        let short = Damage { empty_refs: 20, total_lines: 60, ..Default::default() };
        assert!(short.score() > long.score(),
                "short {} vs long {}", short.score(), long.score());
    }

    #[test]
    fn chunks_break_on_paragraphs_and_never_mid_sentence() {
        // A boundary inside a sentence gives the model half a thought, which
        // it will complete wrongly and confidently.
        let md = "para one is here.\n\npara two is longer than the first.\n\npara three.";
        let chunks = chunk_for_model(md, 40);
        assert!(chunks.len() >= 2);
        for c in &chunks {
            assert!(!c.starts_with(' '), "chunk starts mid-line: {c:?}");
            assert!(c.trim_end().ends_with('.'), "chunk cut mid-sentence: {c:?}");
        }
        // Every paragraph survives somewhere.
        let joined = chunks.join("\n\n");
        for p in ["para one", "para two", "para three"] {
            assert!(joined.contains(p), "{p} was dropped");
        }
    }

    #[test]
    fn an_oversized_paragraph_is_emitted_whole_rather_than_cut() {
        // The caller can refuse it. A silently truncated paragraph is worse
        // than a visible one, because nothing downstream can tell.
        let big = "x".repeat(500);
        let chunks = chunk_for_model(&big, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), 500);
    }

    #[test]
    fn the_real_capture_chunks_without_losing_anything() {
        let chunks = chunk_for_model(CAPTURE, 4000);
        assert!(chunks.len() > 1, "1,535 lines should not be one chunk");
        let total: usize = chunks.iter().map(|c| c.len()).sum();
        // Separators are dropped between chunks, so the sum is slightly under.
        assert!(total >= CAPTURE.len() - chunks.len() * 2,
                "content lost: {} vs {}", total, CAPTURE.len());
    }

    #[test]
    fn a_clean_document_is_left_alone() {
        // "Keep as captured" is a real answer, and for a quotation the only
        // correct one.
        let d = diagnose("# Note\n\nA clean paragraph.\n\nAnother one.\n");
        assert!(d.is_clean(), "score {}", d.score());
        assert_eq!(recommend(&d).level, Level::None);
    }
}
