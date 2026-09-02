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

// ── the missing half: where FileEntry.relevance comes from ───────────────────
//
// This planner was complete and tested and had NO CALLERS for its whole life,
// and the reason is visible in `FileEntry`: it needs a `relevance` per file and
// nothing in the crate produced one. So the prompt-building path grew its own
// coarse byte caps instead, and the chat workspace block ended up shipping a
// directory listing where a planner would have chosen files. Two answers to one
// question, one of them unused.
//
// Relevance for "what is this repository" is not a hard problem and does not
// need embeddings. Two signals carry most of it:
//
//   * DEPTH. A file at the root describes the project; one eight levels down
//     describes a detail. This is the same insight that made `repo_tree`
//     breadth-first — under a tight budget, shallow is what a summary needs.
//   * NAME. Every ecosystem puts its description in a small set of filenames.
//     A README or a manifest is worth more than an arbitrary source file, and
//     both are worth more than a lockfile or a generated blob.

/// Files whose name alone says they describe the project. Ordered most-telling
/// first; the score is assigned by position so adding one is a one-line change.
const DESCRIBES_PROJECT: [&str; 10] = [
    "README.md", "README", "Cargo.toml", "pyproject.toml", "package.json",
    "go.mod", "CMakeLists.txt", "Makefile", "ARCHITECTURE.md", "DESIGN.md",
];

/// Names that are large, generated, or lock-shaped: real files, but reading one
/// teaches nothing about what the project IS, and they are usually the biggest
/// thing in the tree. Skipped rather than down-ranked, because at a tight budget
/// a down-ranked lockfile still wins against a deep source file.
const NOT_WORTH_READING: [&str; 6] = [
    "Cargo.lock", "package-lock.json", "yarn.lock", "poetry.lock",
    "go.sum", "flake.lock",
];

/// Relevance in `[0, 1]` for "summarise this repository", from path shape alone.
///
/// Deliberately no content inspection: this runs over every file in a tree that
/// may be tens of thousands of entries, and opening them all to decide what to
/// open is the batch job the planner exists to avoid.
pub fn describes_project(rel_path: &str) -> f32 {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    if NOT_WORTH_READING.contains(&name) {
        return 0.0;
    }
    if let Some(i) = DESCRIBES_PROJECT.iter().position(|n| *n == name) {
        // 1.0 down to 0.55 across the list — all comfortably above any
        // ordinary source file, so the manifests always make the cut.
        return 1.0 - (i as f32) * 0.05;
    }
    let depth = rel_path.matches('/').count();
    let code = matches!(rel_path.rsplit('.').next().unwrap_or(""),
        "rs" | "py" | "c" | "h" | "cpp" | "hpp" | "go" | "ts" | "js" | "qml" | "md" | "toml");
    if !code {
        return 0.0;
    }
    // An entry point outranks a sibling at the same depth: `main.rs`/`lib.rs`
    // is where someone reading a crate starts.
    let entry = matches!(name, "lib.rs" | "main.rs" | "mod.rs" | "__init__.py" | "index.ts");
    let base = if entry { 0.50 } else { 0.35 };
    // Each level down costs a little. Floored above `min_relevance` (0.15) only
    // for the first few levels, so a deep file is skipped rather than crowding
    // out a shallow one.
    (base - depth as f32 * 0.06).max(0.0)
}

#[cfg(test)]
mod relevance_tests {
    use super::*;

    #[test]
    fn a_readme_outranks_any_source_file() {
        assert!(describes_project("README.md") > describes_project("src/lib.rs"));
        assert!(describes_project("Cargo.toml") > describes_project("src/main.rs"));
    }

    #[test]
    fn shallow_outranks_deep_at_the_same_kind() {
        assert!(describes_project("src/lib.rs")
              > describes_project("crates/a/src/lib.rs"));
    }

    #[test]
    fn an_entry_point_outranks_a_sibling() {
        assert!(describes_project("src/lib.rs") > describes_project("src/util.rs"));
    }

    #[test]
    fn a_lockfile_is_skipped_not_merely_ranked_low() {
        // At a tight budget a low-but-nonzero lockfile still beats a deep source
        // file, and it is usually the largest thing in the tree — so it has to be
        // zero, not small.
        assert_eq!(describes_project("Cargo.lock"), 0.0);
        assert_eq!(describes_project("package-lock.json"), 0.0);
    }

    #[test]
    fn binaries_and_unknown_extensions_are_skipped() {
        assert_eq!(describes_project("assets/logo.png"), 0.0);
        assert_eq!(describes_project("data/model.safetensors"), 0.0);
    }

    #[test]
    fn a_very_deep_file_falls_below_the_default_min_relevance() {
        // Budget::default().min_relevance is 0.15; a file this deep should be
        // skipped by the planner rather than competing for the budget.
        let deep = "a/b/c/d/e/f/g/deep.rs";
        assert!(describes_project(deep) < Budget::default().min_relevance,
            "got {}", describes_project(deep));
    }

    #[test]
    fn one_medium_file_cannot_eat_the_whole_budget() {
        // Found by running the wired planner over apps/switcheroo: a 1405-token
        // HOMEKIT.md took 70% of a 2000-token budget and crowded out every
        // source file, because `Budget { max_tokens, ..default() }` keeps
        // `whole_file_max_tokens` at 1500 no matter how small the budget is.
        // `for_context` caps a single whole file at a quarter of the budget.
        let files: Vec<FileEntry> = [
            ("NOTES.md", 5600usize),   // ~1400 tokens: fits the DEFAULT whole cap
            ("Cargo.toml", 600),
            ("src/main.rs", 1000),
        ].iter().map(|(p, b)| FileEntry {
            path: (*p).into(), bytes: *b,
            relevance: describes_project(p), sections: Vec::new(),
        }).collect();

        let naive = plan(&files, &Budget { max_tokens: 2000, ..Budget::default() });
        let sized = plan(&files, &Budget::for_context(2000, 0, 0));

        let whole = |p: &Plan| -> Vec<String> {
            p.inclusions.iter().filter_map(|i| match i {
                Inclusion::Whole { path, .. } => Some(path.clone()), _ => None }).collect()
        };
        assert!(whole(&naive).contains(&"NOTES.md".to_string()),
            "the naive budget is expected to swallow it — that is the bug being pinned");
        assert!(!whole(&sized).contains(&"NOTES.md".to_string()),
            "a 1400-token file must be peeked, not looped in whole, at a 2000-token budget");
        assert!(whole(&sized).contains(&"Cargo.toml".to_string())
             && whole(&sized).contains(&"src/main.rs".to_string()),
            "and the small descriptive files must survive: {:?}", whole(&sized));
    }

    #[test]
    fn the_planner_puts_the_manifests_in_first_under_a_tight_budget() {
        // End to end: the shape a summary needs is manifests and entry points,
        // not whatever the walk happened to reach first.
        let files: Vec<FileEntry> = [
            ("a/b/c/d/deep.rs", 4000usize),
            ("README.md", 2000),
            ("src/lib.rs", 3000),
            ("Cargo.lock", 200000),
        ].iter().map(|(p, b)| FileEntry {
            path: (*p).into(), bytes: *b,
            relevance: describes_project(p), sections: Vec::new(),
        }).collect();
        let budget = Budget { max_tokens: 1400, ..Budget::default() };
        let plan = plan(&files, &budget);
        let kept: Vec<String> = plan.reads().into_iter().map(|(p, _)| p).collect();
        assert!(kept.contains(&"README.md".to_string()), "{kept:?}");
        assert!(!kept.contains(&"Cargo.lock".to_string()),
            "a 200 KB lockfile must never be read for a summary: {kept:?}");
        assert!(!kept.contains(&"a/b/c/d/deep.rs".to_string()), "{kept:?}");
    }
}

/// Relevance for a SPECIFIC question, not just "what is this repository".
///
/// `describes_project` ranks by path shape alone, which is right for "summarise
/// this codebase" and wrong for everything else. A field run asked "add a new
/// theme bright pink"; the planner surfaced `Cargo.toml` (0.90) and buried
/// `snapclone-theme/src/palette.rs` (0.17), and the model — shown a manifest
/// and asked about themes — confabulated a `themes` array into it. It did
/// exactly what it was pointed at. The wrong file is worse than no file,
/// because a model given nothing says so and a model given the wrong thing
/// invents.
///
/// The boost is deliberately crude: content words from the question matched
/// against the PATH. No embeddings, no content scan — the planner exists to
/// avoid opening everything, so its ranking may not open anything either.
/// Path matching is enough here because repositories name things after what
/// they do: a question about themes finds `snapclone-theme/`, `theme.rs`,
/// `palette.rs` is found by "palette", and so on.
pub fn relevance_for_query(rel_path: &str, query: &str) -> f32 {
    let base = describes_project(rel_path);
    // A file the shape rule REFUSED stays refused. A lockfile or a PNG that
    // happens to contain the word is still a lockfile or a PNG, and letting a
    // name coincidence resurrect a 200 KB Cargo.lock into the prompt is the
    // opposite of the point.
    if base <= 0.0 {
        return 0.0;
    }
    let hay = rel_path.to_lowercase();
    // Words worth matching on. Short ones match everything ("a" is in "palette")
    // and stopwords carry no signal, so both are dropped rather than weighted
    // down — a 0.05 boost applied to every file is just noise with a cost.
    const STOP: [&str; 24] = [
        "add", "new", "the", "and", "for", "with", "make", "change", "fix", "update",
        "please", "can", "you", "how", "what", "where", "why", "does", "this", "that",
        "into", "from", "some", "file",
    ];
    let mut hits = 0usize;
    let mut terms = 0usize;
    for w in query.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
        if w.len() < 4 || STOP.contains(&w) { continue; }
        terms += 1;
        // Singular/plural without a stemmer: "themes" should find "theme".
        let stem = w.strip_suffix('s').unwrap_or(w);
        if hay.contains(stem) { hits += 1; }
    }
    if terms == 0 || hits == 0 {
        return base;
    }
    // A path that names what was asked about must DECISIVELY outrank a
    // manifest, not merely edge past it. First attempt added a ratio-scaled
    // boost to the base and lost: `snapclone-theme/src/palette.rs` scored
    // 0.50 against `Cargo.toml`'s 0.90, so the field case would have failed
    // again. The floor is what fixes it — any name match lands above the
    // manifest tier (0.90), with the ratio and the base only breaking ties
    // among matches. A README still leads at 1.0, which is right: it is
    // genuinely worth reading for almost any question.
    let ratio = (hits as f32 / terms as f32).min(1.0);
    (0.92 + 0.07 * ratio + 0.01 * base).min(1.0)
}

#[cfg(test)]
mod query_relevance_tests {
    use super::*;

    #[test]
    fn the_field_case_now_surfaces_the_palette_not_the_manifest() {
        // "add a new theme bright pink" — the exact prompt that produced a
        // confabulated `themes` array in Cargo.toml.
        let q = "add a new theme bright pink";
        let palette = relevance_for_query("crates/snapclone-theme/src/palette.rs", q);
        let manifest = relevance_for_query("Cargo.toml", q);
        assert!(palette > manifest,
            "palette {palette:.2} must outrank the root manifest {manifest:.2} for a theme task");
    }

    #[test]
    fn an_unrelated_question_leaves_the_ranking_alone() {
        // The boost must not fire on everything, or it replaces one fixed
        // ranking with a differently-fixed one.
        let q = "why is decoding slow";
        for p in ["Cargo.toml", "crates/snapclone-theme/src/palette.rs", "README.md"] {
            assert_eq!(relevance_for_query(p, q), describes_project(p), "{p} moved");
        }
    }

    #[test]
    fn a_summarise_style_question_falls_back_to_path_shape() {
        // All stopwords and short words: nothing to match on, so the
        // "what is this repository" ranking is the right answer and must survive.
        let q = "what does this do";
        assert_eq!(relevance_for_query("Cargo.toml", q), describes_project("Cargo.toml"));
    }

    #[test]
    fn plurals_match_their_singular() {
        let a = relevance_for_query("src/themes.rs", "add a theme");
        let b = relevance_for_query("src/theme.rs", "add some themes");
        assert!(a > describes_project("src/themes.rs"));
        assert!(b > describes_project("src/theme.rs"));
    }

    #[test]
    fn a_skipped_file_stays_skipped_even_when_its_name_matches() {
        // A lockfile mentioning "theme" is still a lockfile. describes_project
        // returns 0 for these and the boost must not resurrect them, or a
        // 200 KB Cargo.lock lands in the prompt for a one-word coincidence.
        assert_eq!(relevance_for_query("Cargo.lock", "add a theme"), 0.0);
        assert_eq!(relevance_for_query("assets/theme.png", "add a theme"), 0.0);
    }

    #[test]
    fn the_score_stays_in_range() {
        let q = "theme palette colour snapclone egui bright";
        let r = relevance_for_query("crates/snapclone-theme/src/palette.rs", q);
        assert!((0.0..=1.0).contains(&r), "got {r}");
    }
}
