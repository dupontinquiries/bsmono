//! ctxplan.rs — project-space context assembly for the chat backend.
//!
//! A chat turn often needs files from a project space (the `/tmp/logscope/{uuid}` scratch devloop's
//! `mount` sets up), but rarely *all* of them and rarely *whole*. This plans what to pull into the model's
//! context for a turn: **loop in** a small, relevant file whole; **peek** only the relevant sections of a
//! large one; or **skip** it — greedily, highest relevance first, under a token budget. The output also
//! says exactly which byte ranges the IO layer should read, so a peek touches only the sections it needs.
//!
//! The space walk, the relevance scores (from `retrieval`/`explain`), the file reads, and tokenization are
//! all seams; the decision logic and the read plan are pure and unit-tested.
#![allow(dead_code)]

/// An interesting span of a file (a symbol body, a hunk, a match region) — byte range `[start, end)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Section { pub label: String, pub start: usize, pub end: usize, pub relevance: f32 }
impl Section { pub fn len(&self) -> usize { self.end.saturating_sub(self.start) } }

/// A candidate file in the project space, already scored by the caller.
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub bytes: usize,           // total size
    pub relevance: f32,         // whole-file relevance to the turn
    pub sections: Vec<Section>, // known interesting spans (may be empty)
}

/// What the planner decided for one file.
#[derive(Clone, Debug, PartialEq)]
pub enum Inclusion {
    Whole { path: String, est_tokens: usize },
    Peek { path: String, sections: Vec<Section>, est_tokens: usize },
    Skip { path: String, reason: &'static str },
}

/// Budget + thresholds governing the plan.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub max_tokens: usize,            // total context budget for the turn
    pub whole_file_max_tokens: usize, // files larger than this are peeked, never looped in whole
    pub min_relevance: f32,           // below this, skip
    pub bytes_per_token: usize,       // token estimator (~4 for code/English)
}
impl Default for Budget {
    fn default() -> Self { Self { max_tokens: 8000, whole_file_max_tokens: 1500, min_relevance: 0.15, bytes_per_token: 4 } }
}
impl Budget {
    /// Derive the file-context budget from the model's USABLE context window — `context_tokens` is the
    /// window after any RoPE extension (`rope::effective_context` of an NTK/YaRN plan), so lengthening the
    /// context directly widens how much project-space material a turn can pull in. Reserves `reserve_sys`
    /// for the system prompt + chat history and `reserve_out` for the response; the remainder is the file
    /// budget, with the whole-file cap a quarter of it (so one big file can't crowd out the rest).
    pub fn for_context(context_tokens: usize, reserve_sys: usize, reserve_out: usize) -> Self {
        let avail = context_tokens.saturating_sub(reserve_sys.saturating_add(reserve_out));
        Self {
            max_tokens: avail,
            whole_file_max_tokens: (avail / 4).clamp(256, 16000),
            min_relevance: 0.15,
            bytes_per_token: 4,
        }
    }
}

#[derive(Debug, Default)]
pub struct Plan { pub inclusions: Vec<Inclusion>, pub used_tokens: usize }

fn est(bytes: usize, bpt: usize) -> usize { bytes.div_ceil(bpt.max(1)) }

/// Pick the most-relevant sections that fit `budget` tokens (greedy by relevance, then file order).
fn pick_sections(sections: &[Section], budget: usize, bpt: usize) -> (Vec<Section>, usize) {
    let mut ordered: Vec<&Section> = sections.iter().collect();
    ordered.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal).then(a.start.cmp(&b.start)));
    let (mut chosen, mut used) = (Vec::new(), 0usize);
    for s in ordered {
        let t = est(s.len(), bpt);
        if used + t <= budget { used += t; chosen.push(s.clone()); }
    }
    chosen.sort_by_key(|s| s.start); // restore reading order
    (chosen, used)
}

/// Plan the turn's context over the project-space `files`. Greedy, highest relevance first.
pub fn plan(files: &[FileEntry], budget: &Budget) -> Plan {
    let bpt = budget.bytes_per_token.max(1);
    let mut order: Vec<&FileEntry> = files.iter().collect();
    order.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal).then(a.path.cmp(&b.path)));

    let mut inclusions = Vec::with_capacity(files.len());
    let mut used = 0usize;
    for f in order {
        let remaining = budget.max_tokens.saturating_sub(used);
        if f.relevance < budget.min_relevance {
            inclusions.push(Inclusion::Skip { path: f.path.clone(), reason: "below relevance threshold" });
            continue;
        }
        if remaining == 0 {
            inclusions.push(Inclusion::Skip { path: f.path.clone(), reason: "budget exhausted" });
            continue;
        }
        let whole = est(f.bytes, bpt);
        if whole <= budget.whole_file_max_tokens && whole <= remaining {
            used += whole;
            inclusions.push(Inclusion::Whole { path: f.path.clone(), est_tokens: whole });
            continue;
        }
        // too big to loop in whole (or won't fit) → peek
        let peek_budget = remaining.min(budget.whole_file_max_tokens);
        let (mut chosen, mut t) = pick_sections(&f.sections, peek_budget, bpt);
        if chosen.is_empty() {
            // no known sections → peek the head, so the model at least sees the file's shape
            let head_bytes = (peek_budget * bpt).min(f.bytes);
            if head_bytes == 0 {
                inclusions.push(Inclusion::Skip { path: f.path.clone(), reason: "no budget for peek" });
                continue;
            }
            chosen = vec![Section { label: "head".into(), start: 0, end: head_bytes, relevance: f.relevance }];
            t = est(head_bytes, bpt);
        }
        used += t;
        inclusions.push(Inclusion::Peek { path: f.path.clone(), sections: chosen, est_tokens: t });
    }
    Plan { inclusions, used_tokens: used }
}

impl Plan {
    /// Concrete read requests for the IO layer: `(path, ranges)`. An empty range list means "read the whole
    /// file" (a `Whole` inclusion); a non-empty list means "read only these `[start,end)` spans" (a `Peek`).
    /// Skips produce nothing — that's the savings.
    pub fn reads(&self) -> Vec<(String, Vec<(usize, usize)>)> {
        self.inclusions.iter().filter_map(|inc| match inc {
            Inclusion::Whole { path, .. } => Some((path.clone(), Vec::new())),
            Inclusion::Peek { path, sections, .. } => Some((path.clone(), sections.iter().map(|s| (s.start, s.end)).collect())),
            Inclusion::Skip { .. } => None,
        }).collect()
    }

    /// Assemble the actual context prefix by fetching each read via the injected IO seam. `fetch(path,
    /// ranges)` returns the bytes (whole file when `ranges` is empty, else just those spans, concatenated).
    pub fn assemble(&self, fetch: impl Fn(&str, &[(usize, usize)]) -> String) -> String {
        let mut out = String::new();
        for (path, ranges) in self.reads() {
            let body = fetch(&path, &ranges);
            out.push_str(&format!("// {path}\n{body}\n\n"));
        }
        out
    }

    pub fn looped_in(&self) -> usize { self.inclusions.iter().filter(|i| matches!(i, Inclusion::Whole { .. })).count() }
    pub fn peeked(&self) -> usize { self.inclusions.iter().filter(|i| matches!(i, Inclusion::Peek { .. })).count() }
    pub fn skipped(&self) -> usize { self.inclusions.iter().filter(|i| matches!(i, Inclusion::Skip { .. })).count() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(label: &str, start: usize, end: usize, rel: f32) -> Section {
        Section { label: label.into(), start, end, relevance: rel }
    }
    fn file(path: &str, bytes: usize, rel: f32, sections: Vec<Section>) -> FileEntry {
        FileEntry { path: path.into(), bytes, relevance: rel, sections }
    }
    // small budget for clear arithmetic: 4 bytes/token
    fn budget(max: usize, whole_max: usize) -> Budget {
        Budget { max_tokens: max, whole_file_max_tokens: whole_max, min_relevance: 0.2, bytes_per_token: 4 }
    }

    #[test]
    fn budget_for_context_scales_with_the_window() {
        // reserve 1k system + 1k response; the rest is the file budget
        let small = Budget::for_context(8_000, 1_000, 1_000);
        let big = Budget::for_context(32_768, 1_000, 1_000); // a 4× RoPE-extended window
        assert_eq!(small.max_tokens, 6_000);
        assert_eq!(big.max_tokens, 30_768);
        assert!(big.max_tokens > small.max_tokens, "extending context widens the file budget");
        assert!(big.whole_file_max_tokens >= small.whole_file_max_tokens);

        // concretely: a 20k-byte (5k-token) relevant file is peeked under the small window but fits whole
        // under the extended one
        let f = file("big.rs", 20_000, 0.9, vec![sec("body", 0, 20_000, 0.9)]);
        let sp = plan(&[f.clone()], &small);
        assert!(matches!(sp.inclusions[0], Inclusion::Peek { .. }), "peeked when the window is small");
        let bp = plan(&[f], &big);
        assert!(matches!(bp.inclusions[0], Inclusion::Whole { .. }), "looped in whole once context is extended");
    }

    #[test]
    fn small_relevant_file_is_looped_in_whole() {
        let p = plan(&[file("a.rs", 400, 0.9, vec![])], &budget(1000, 200));
        assert_eq!(p.inclusions[0], Inclusion::Whole { path: "a.rs".into(), est_tokens: 100 });
        assert_eq!(p.reads(), vec![("a.rs".into(), vec![])]); // whole-file read
    }

    #[test]
    fn large_file_is_peeked_to_its_top_sections() {
        // 40 KB file (10000 tokens) — far over the whole-file cap → peek
        let f = file("big.rs", 40_000, 0.9, vec![
            sec("hot", 1000, 1400, 0.95),   // 400 B = 100 tok
            sec("warm", 5000, 5400, 0.50),  // 100 tok
            sec("cold", 9000, 9080, 0.10),  // 20 tok
        ]);
        let p = plan(&[f], &budget(1000, 200)); // peek budget = 200 tok → fits hot+warm (200), not cold
        match &p.inclusions[0] {
            Inclusion::Peek { sections, est_tokens, .. } => {
                let labels: Vec<&str> = sections.iter().map(|s| s.label.as_str()).collect();
                assert_eq!(labels, vec!["hot", "warm"]); // top-2 by relevance, in reading order
                assert_eq!(*est_tokens, 200);
            }
            other => panic!("expected Peek, got {other:?}"),
        }
        // reads only the chosen ranges, not the whole 40 KB
        assert_eq!(p.reads(), vec![("big.rs".into(), vec![(1000, 1400), (5000, 5400)])]);
    }

    #[test]
    fn irrelevant_files_are_skipped() {
        let p = plan(&[file("junk.rs", 100, 0.05, vec![])], &budget(1000, 200));
        assert!(matches!(p.inclusions[0], Inclusion::Skip { reason: "below relevance threshold", .. }));
        assert!(p.reads().is_empty());
    }

    #[test]
    fn budget_is_spent_on_the_most_relevant_first() {
        // two whole-eligible files; budget only fits one → higher-relevance wins, other skipped
        let files = vec![
            file("low.rs", 400, 0.30, vec![]),  // 100 tok
            file("high.rs", 400, 0.95, vec![]), // 100 tok
        ];
        let p = plan(&files, &budget(100, 200)); // only 100 tokens total
        assert_eq!(p.looped_in(), 1);
        assert_eq!(p.skipped(), 1);
        // the included one is high.rs
        assert!(p.inclusions.iter().any(|i| matches!(i, Inclusion::Whole { path, .. } if path == "high.rs")));
        assert!(p.used_tokens <= 100);
    }

    #[test]
    fn large_file_without_sections_falls_back_to_a_head_peek() {
        let p = plan(&[file("opaque.bin", 40_000, 0.8, vec![])], &budget(1000, 150));
        match &p.inclusions[0] {
            Inclusion::Peek { sections, .. } => {
                assert_eq!(sections[0].label, "head");
                assert_eq!(sections[0].start, 0);
                assert_eq!(sections[0].end, 150 * 4); // peek_budget tokens × bytes_per_token
            }
            other => panic!("expected head Peek, got {other:?}"),
        }
    }

    #[test]
    fn assemble_uses_the_io_seam_for_whole_and_peek() {
        let files = vec![
            file("a.rs", 400, 0.9, vec![]),
            file("big.rs", 40_000, 0.8, vec![sec("fn_x", 100, 200, 0.9)]),
        ];
        let p = plan(&files, &budget(1000, 200));
        // mock fetch: whole → "<whole path>", ranges → "<path 100-200>"
        let ctx = p.assemble(|path, ranges| {
            if ranges.is_empty() { format!("<whole {path}>") }
            else { ranges.iter().map(|(s, e)| format!("<{path} {s}-{e}>")).collect::<Vec<_>>().join("") }
        });
        assert!(ctx.contains("<whole a.rs>"));
        assert!(ctx.contains("<big.rs 100-200>"));
        assert!(!ctx.contains("<whole big.rs>"), "the big file was peeked, not looped in whole");
    }
}
