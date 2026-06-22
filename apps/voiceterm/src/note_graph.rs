//! v0.1.670: context-aware completion ranking for the search bar AND
//! ZBracket linking — one engine, two consumers.
//!
//! The UX need: when you type in the search bar or start a `[[link`, the
//! suggestions should blend (a) name matches against local files, (b) notes
//! ADJACENT in the Zettelkasten graph (linked to/from the current note —
//! the "mind palace" neighbors), (c) files near the current one on disk
//! (folder proximity), and (d) historical terms you've used. search_suggest
//! already does history/bookmarks/files for the web bar; this adds the graph
//! adjacency + proximity dimension that note-linking needs, and is shaped so
//! the search bar can use it too.
//!
//! PURE + testable: the graph build + ranking are here; the dropdown UI and
//! async web results are the wiring layer (web suggestions merge in on top).

use std::collections::{HashMap, HashSet};

/// The note link graph: note-name → set of note-names it links to. Built
/// from each note's ZBracket targets (see zbrackets::link_targets). Names
/// are basenames-without-extension (the link key).
#[derive(Debug, Clone, Default)]
pub struct NoteGraph {
    out: HashMap<String, HashSet<String>>, // links FROM a note
    inc: HashMap<String, HashSet<String>>, // links TO a note (backlinks)
}

impl NoteGraph {
    pub fn new() -> Self { Self::default() }

    /// Add an edge: `from` links to `to`.
    pub fn add_edge(&mut self, from: &str, to: &str) {
        self.out.entry(from.to_string()).or_default().insert(to.to_string());
        self.inc.entry(to.to_string()).or_default().insert(from.to_string());
    }

    /// Build from (note-name, markdown) pairs by extracting ZBracket targets.
    pub fn build(notes: &[(String, String)]) -> Self {
        let mut g = Self::new();
        for (name, md) in notes {
            for target in crate::zbrackets::link_targets(md) {
                g.add_edge(name, &target);
            }
        }
        g
    }

    /// Direct neighbors of `note` (both out-links and back-links).
    pub fn neighbors(&self, note: &str) -> HashSet<String> {
        let mut s = HashSet::new();
        if let Some(o) = self.out.get(note) { s.extend(o.iter().cloned()); }
        if let Some(i) = self.inc.get(note) { s.extend(i.iter().cloned()); }
        s
    }

    /// Adjacency score of `candidate` relative to `current`: 1.0 if a direct
    /// neighbor, 0.4 if a two-hop neighbor (shares a neighbor), else 0.0.
    pub fn adjacency(&self, current: &str, candidate: &str) -> f32 {
        if current == candidate { return 0.0; }
        let n1 = self.neighbors(current);
        if n1.contains(candidate) { return 1.0; }
        // two-hop: candidate is a neighbor of one of current's neighbors
        for nb in &n1 {
            if self.neighbors(nb).contains(candidate) { return 0.4; }
        }
        0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestKind { LocalFile, AdjacentNote, History }

#[derive(Debug, Clone)]
pub struct Completion {
    /// the value to insert (the note name / file stem / term)
    pub text: String,
    /// where it came from (the strongest contributing source)
    pub kind: SuggestKind,
    pub score: f32,
}

/// A historical term the user typed before (a past query or link target).
#[derive(Debug, Clone)]
pub struct HistTerm {
    pub term: String,
    pub last_used: u64, // unix secs
    pub count: u32,
}

/// Name-match quality of `partial` against `candidate` (both matched on the
/// basename). Empty partial → neutral 0.5 so ranking falls to context
/// (adjacency/history/proximity) — that's the "just opened `[[`" state.
pub fn match_score(partial: &str, candidate: &str) -> f32 {
    let p = partial.trim().to_lowercase();
    let c = candidate.to_lowercase();
    if p.is_empty() { return 0.5; }
    if c == p { return 1.0; }
    if c.starts_with(&p) { return 0.85; }
    // word-boundary substring (after a space/_/-/ /)
    if c.split(|ch: char| ch == ' ' || ch == '_' || ch == '-' || ch == '/')
        .any(|w| w.starts_with(&p)) {
        return 0.65;
    }
    if c.contains(&p) { return 0.45; }
    if is_subsequence(&p, &c) { return 0.3; }
    0.0
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|nc| it.any(|hc| hc == nc))
}

/// Folder proximity of `candidate_path` to `current_path`: 1.0 in the same
/// directory, decaying with the total directory distance — both `../` hops UP
/// to the common ancestor AND directories DOWN to the candidate.
pub fn proximity(current_path: &str, candidate_path: &str) -> f32 {
    if current_path.is_empty() || candidate_path.is_empty() { return 0.0; }
    let rel = crate::zbrackets::path_difference(current_path, candidate_path);
    let up = rel.matches("../").count();
    // downward dirs = path components in the forward part minus the filename
    let fwd = rel.trim_start_matches("../");
    let down = fwd.split('/').count().saturating_sub(1); // last is the filename
    let dist = up + down;
    1.0 / (1.0 + dist as f32)
}

fn history_boost(candidate: &str, history: &[HistTerm], now: u64) -> f32 {
    let cl = candidate.to_lowercase();
    history.iter()
        .filter(|h| h.term.to_lowercase() == cl)
        .map(|h| {
            let age = now.saturating_sub(h.last_used) as f32;
            let recency = 0.5f32.powf(age / (3.0 * 86_400.0)); // ~3-day half-life
            let freq = (h.count as f32).min(10.0) / 10.0;
            0.5 * recency + 0.5 * freq
        })
        .fold(0.0, f32::max)
}

fn basename_stem(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base).to_string()
}

/// Rank completions for `partial`, blending name match with graph adjacency,
/// folder proximity, and history. `current_note` is the note being edited
/// (its name + path). `files` are local note paths; `history` past terms.
/// Returns up to `limit`, best first, deduped by text.
pub fn rank(
    partial: &str,
    current_note: &str,
    current_path: &str,
    files: &[String],
    graph: &NoteGraph,
    history: &[HistTerm],
    now: u64,
    limit: usize,
) -> Vec<Completion> {
    let mut best: HashMap<String, Completion> = HashMap::new();
    let mut consider = |text: String, path: Option<&str>, kind: SuggestKind| {
        let m = match_score(partial, &text);
        if m <= 0.0 { return; }
        let adj = graph.adjacency(current_note, &text);
        let prox = path.map(|p| proximity(current_path, p)).unwrap_or(0.0);
        let hist = history_boost(&text, history, now);
        // match relevance gates; context multiplies it up.
        let score = m * (1.0 + 0.8 * adj + 0.5 * prox + 0.6 * hist);
        // pick the strongest-source label for display
        let src = if adj > 0.0 { SuggestKind::AdjacentNote }
                  else if hist > 0.0 { SuggestKind::History }
                  else { kind };
        let e = best.entry(text.clone()).or_insert(Completion { text, kind: src, score: 0.0 });
        if score > e.score { e.score = score; e.kind = src; }
    };

    for f in files { consider(basename_stem(f), Some(f), SuggestKind::LocalFile); }
    for h in history { consider(h.term.clone(), None, SuggestKind::History); }
    // ensure graph neighbors are candidates even if not in the file list yet
    for nb in graph.neighbors(current_note) { consider(nb, None, SuggestKind::AdjacentNote); }

    let mut out: Vec<Completion> = best.into_values().collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(a.text.cmp(&b.text)));
    out.truncate(limit);
    out
}

/// Wire format for QML: "text\tkind\tscore" per line. kind ∈ file|note|hist.
pub fn to_wire(cs: &[Completion]) -> String {
    cs.iter().map(|c| {
        let k = match c.kind {
            SuggestKind::LocalFile => "file",
            SuggestKind::AdjacentNote => "note",
            SuggestKind::History => "hist",
        };
        format!("{}\t{}\t{:.3}", c.text, k, c.score)
    }).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(edges: &[(&str, &str)]) -> NoteGraph {
        let mut g = NoteGraph::new();
        for (a, b) in edges { g.add_edge(a, b); }
        g
    }

    #[test]
    fn match_tiers() {
        assert_eq!(match_score("go", "Go"), 1.0);
        assert!(match_score("go", "Golang") >= 0.85);
        assert!(match_score("str", "Data Structure") >= 0.65); // word-boundary
        assert!(match_score("ruct", "Structure") >= 0.45);     // substring
        assert!(match_score("", "anything") == 0.5);            // neutral
        assert_eq!(match_score("xyz", "Go"), 0.0);
    }

    #[test]
    fn adjacency_direct_and_two_hop() {
        let graph = g(&[("A", "B"), ("B", "C")]);
        assert_eq!(graph.adjacency("A", "B"), 1.0); // direct
        assert_eq!(graph.adjacency("A", "C"), 0.4); // two-hop via B
        assert_eq!(graph.adjacency("A", "Z"), 0.0); // unrelated
    }

    #[test]
    fn backlinks_count_as_adjacent() {
        let graph = g(&[("Other", "Current")]); // Other links TO Current
        assert_eq!(graph.adjacency("Current", "Other"), 1.0); // backlink neighbor
    }

    #[test]
    fn proximity_same_dir_beats_distant() {
        let near = proximity("/n/Notes/a.md", "/n/Notes/b.md");
        let far = proximity("/n/Notes/a.md", "/n/Notes/deep/sub/c.md");
        assert!(near > far, "near {} should beat far {}", near, far);
        assert_eq!(near, 1.0);
    }

    #[test]
    fn adjacency_outranks_equal_name_match() {
        // two files both prefix-match "De"; one is linked from the current
        // note, the other isn't. The linked one should rank first.
        let graph = g(&[("Index", "Deep")]);
        let files = vec!["/n/Deep.md".to_string(), "/n/Decoy.md".to_string()];
        let out = rank("De", "Index", "/n/Index.md", &files, &graph, &[], 1000, 10);
        assert_eq!(out[0].text, "Deep", "adjacent note wins");
        assert_eq!(out[0].kind, SuggestKind::AdjacentNote);
    }

    #[test]
    fn history_lifts_a_match() {
        let files = vec!["/n/Apple.md".to_string(), "/n/Apricot.md".to_string()];
        let hist = vec![HistTerm { term: "Apricot".into(), last_used: 1000, count: 5 }];
        let out = rank("Ap", "X", "/n/X.md", &files, &NoteGraph::new(), &hist, 1000, 10);
        assert_eq!(out[0].text, "Apricot", "recent/frequent term lifts");
    }

    #[test]
    fn empty_partial_ranks_by_context_only() {
        // just opened "[[": no typed text → adjacency/history drive it.
        let graph = g(&[("Cur", "Linked")]);
        let files = vec!["/n/Linked.md".to_string(), "/n/Random.md".to_string()];
        let out = rank("", "Cur", "/n/Cur.md", &files, &graph, &[], 1000, 10);
        assert_eq!(out[0].text, "Linked", "adjacent note surfaces first on empty");
    }

    #[test]
    fn build_graph_from_notes() {
        let notes = vec![
            ("Index".to_string(), "see [[Deep]] and [[Other]]".to_string()),
            ("Deep".to_string(), "back to [[Index]]".to_string()),
        ];
        let graph = NoteGraph::build(&notes);
        assert_eq!(graph.adjacency("Index", "Deep"), 1.0);
        assert_eq!(graph.adjacency("Index", "Other"), 1.0);
        // Deep links back to Index → Index is adjacent to Deep
        assert_eq!(graph.adjacency("Deep", "Index"), 1.0);
    }

    #[test]
    fn dedup_keeps_best_and_wire_format() {
        let graph = g(&[("Cur", "Go")]);
        let files = vec!["/n/Go.md".to_string()];
        let out = rank("Go", "Cur", "/n/Cur.md", &files, &graph, &[], 1000, 10);
        assert_eq!(out.len(), 1, "Go appears once despite file+graph sources");
        let wire = to_wire(&out);
        assert!(wire.starts_with("Go\tnote\t"), "got {}", wire);
    }
}
