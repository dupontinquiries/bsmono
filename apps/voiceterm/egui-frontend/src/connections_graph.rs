//! connections_graph — `connections.rs`'s node/edge spine, ported.
//!
//! An Obsidian-style view over everything voiceterm already stores: archived
//! pages, voice recordings, chat threads, workflows, speakers, local files.
//! *"A unified Node abstraction over the existing data systems... so later UI
//! ships have a stable shape to render against."*
//!
//! The scanning is filesystem work and belongs to the shell. What ports is the
//! **shape**: the identity scheme, the edge vocabulary, and the two decisions
//! that make the graph usable rather than merely present.
//!
//! ## Ids are stable across re-scans
//!
//! *"Stable ID: `<kind-prefix>_<hash>`. Stable across re-scans for the same
//! underlying entity."* Without that, every rescan produces a new node, the
//! layout jumps, and any selection or pin the user made points at nothing. It
//! is the difference between a graph and a screensaver.
//!
//! ## The similarity edge carries milli-units, not a float
//!
//! *"The u32 carries the similarity ×1000 (milli-units) so the enum keeps its
//! Eq/Hash derive (f32 isn't Eq/Hash)."* That is the right call and easy to
//! undo by accident: an `f32` in the reason makes `EdgeReason` unhashable, and
//! deduplicating edges is exactly what you need a `HashSet` for.

/// What a node represents.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NodeKind { Page, VoiceRecording, ChatThread, Workflow, Speaker, LocalFile }

impl NodeKind {
    pub const ALL: [NodeKind; 6] = [
        NodeKind::Page, NodeKind::VoiceRecording, NodeKind::ChatThread,
        NodeKind::Workflow, NodeKind::Speaker, NodeKind::LocalFile,
    ];

    /// One character, prefixed onto every id of this kind.
    pub fn id_prefix(self) -> &'static str {
        match self {
            NodeKind::Page => "p", NodeKind::VoiceRecording => "v",
            NodeKind::ChatThread => "c", NodeKind::Workflow => "w",
            NodeKind::Speaker => "s", NodeKind::LocalFile => "f",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NodeKind::Page => "Page", NodeKind::VoiceRecording => "Voice",
            NodeKind::ChatThread => "Chat", NodeKind::Workflow => "Workflow",
            NodeKind::Speaker => "Speaker", NodeKind::LocalFile => "File",
        }
    }
}

/// Longest preview the UI will render.
pub const PREVIEW_CHARS: usize = 120;

/// A node in the graph.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    pub title: String,
    /// URL, speaker, model, command count, path — empty when not applicable.
    pub subtitle: String,
    pub preview: String,
    pub tags: Vec<String>,
}

/// Build a stable id for an entity.
///
/// The key is whatever identifies the entity in its own store — a URL, a file
/// path, a speaker name. Hashed rather than embedded so an id is a fixed
/// length and safe in a filename or a URL fragment, and prefixed so two kinds
/// sharing a key cannot collide.
pub fn node_id(kind: NodeKind, key: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01B3);
    }
    format!("{}_{:016x}", kind.id_prefix(), h)
}

/// Trim a preview to the render budget.
///
/// Cut at a character boundary and on whitespace where possible: slicing a
/// UTF-8 string mid-codepoint panics, and a preview ending mid-word reads as
/// corrupted text rather than as a truncation.
pub fn preview(text: &str) -> String {
    let clean = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= PREVIEW_CHARS { return clean; }
    let cut: String = clean.chars().take(PREVIEW_CHARS).collect();
    match cut.rfind(' ') {
        Some(i) if i > PREVIEW_CHARS / 2 => format!("{}…", &cut[..i]),
        _ => format!("{cut}…"),
    }
}

/// Why two nodes are connected.
///
/// `Eq + Hash` throughout, which is why the similarity edge carries an integer.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum EdgeReason {
    /// Voice captured while reading this page. The corpus pipeline does not
    /// tag recordings with a source URL yet, so nothing produces this one —
    /// kept because the vocabulary is the stable part.
    VoiceFromPage,
    VoiceBySpeaker,
    ChatReferencesPage,
    /// Same domain — *"weak but useful"*.
    SameDomain,
    SharedTag(String),
    /// Cosine similarity ×1000, so the enum stays hashable.
    Similar(u32),
}

impl EdgeReason {
    /// Similarity as a fraction, for display.
    pub fn similarity(&self) -> Option<f32> {
        match self { EdgeReason::Similar(m) => Some(*m as f32 / 1000.0), _ => None }
    }

    /// Build one from a fraction.
    pub fn from_similarity(v: f32) -> EdgeReason {
        EdgeReason::Similar((v.clamp(0.0, 1.0) * 1000.0).round() as u32)
    }

    /// How strongly to draw it.
    ///
    /// Same-domain is deliberately weakest: it connects everything on a busy
    /// site, and drawn at full strength it drowns the edges that mean
    /// something.
    pub fn weight(&self) -> f32 {
        match self {
            EdgeReason::VoiceBySpeaker | EdgeReason::VoiceFromPage => 1.0,
            EdgeReason::ChatReferencesPage => 0.9,
            EdgeReason::SharedTag(_) => 0.6,
            EdgeReason::Similar(m) => (*m as f32 / 1000.0).clamp(0.0, 1.0),
            EdgeReason::SameDomain => 0.25,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub reason: EdgeReason,
}

impl Edge {
    /// Build an edge with endpoints in a canonical order.
    ///
    /// These relations are undirected — A shares a tag with B exactly when B
    /// shares one with A — so ordering the endpoints means the deduplicating
    /// set sees one edge rather than two.
    pub fn undirected(a: &str, b: &str, reason: EdgeReason) -> Edge {
        let (from, to) = if a <= b { (a, b) } else { (b, a) };
        Edge { from: from.into(), to: to.into(), reason }
    }
}

/// Similarity below which an edge is not worth drawing.
///
/// A graph where everything connects to everything shows nothing.
pub const SIMILAR_THRESHOLD: f32 = 0.35;

/// Drop edges too weak to earn their ink, and deduplicate.
pub fn prune(edges: Vec<Edge>) -> Vec<Edge> {
    let mut seen = std::collections::HashSet::new();
    edges.into_iter()
        .filter(|e| match &e.reason {
            EdgeReason::Similar(m) => *m as f32 / 1000.0 >= SIMILAR_THRESHOLD,
            _ => true,
        })
        // Self-edges come free from a naive pairwise scan and render as a dot.
        .filter(|e| e.from != e.to)
        .filter(|e| seen.insert(e.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_stable_across_rescans() {
        // Without this every rescan produces a new node, the layout jumps, and
        // any selection or pin the user made points at nothing.
        let a = node_id(NodeKind::Page, "https://example.com/x");
        let b = node_id(NodeKind::Page, "https://example.com/x");
        assert_eq!(a, b);
    }

    #[test]
    fn two_kinds_sharing_a_key_do_not_collide() {
        // A speaker named "notes" and a workflow named "notes" are different
        // nodes; without the prefix they would merge into one.
        assert_ne!(node_id(NodeKind::Speaker, "notes"), node_id(NodeKind::Workflow, "notes"));
    }

    #[test]
    fn kind_prefixes_are_unique() {
        // The original tests this too, and it is what the collision guarantee
        // rests on.
        let mut p: Vec<&str> = NodeKind::ALL.iter().map(|k| k.id_prefix()).collect();
        p.sort_unstable();
        let before = p.len();
        p.dedup();
        assert_eq!(p.len(), before);
    }

    #[test]
    fn an_id_is_a_fixed_length_whatever_the_key() {
        // Hashed rather than embedded so it is safe in a filename or a URL
        // fragment, and a 2KB URL does not become a 2KB id.
        let short = node_id(NodeKind::Page, "a");
        let long = node_id(NodeKind::Page, &"x".repeat(4000));
        assert_eq!(short.len(), long.len());
        assert!(short.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }

    #[test]
    fn the_similarity_edge_stays_hashable() {
        // "The u32 carries the similarity ×1000 so the enum keeps its Eq/Hash
        // derive (f32 isn't Eq/Hash)" — and deduplicating edges is exactly what
        // a HashSet is for.
        let mut set = std::collections::HashSet::new();
        set.insert(EdgeReason::from_similarity(0.87));
        set.insert(EdgeReason::from_similarity(0.87));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn similarity_round_trips_through_milli_units() {
        let r = EdgeReason::from_similarity(0.873);
        assert!((r.similarity().unwrap() - 0.873).abs() < 0.001);
        assert_eq!(EdgeReason::from_similarity(5.0).similarity(), Some(1.0), "clamped");
        assert_eq!(EdgeReason::from_similarity(-1.0).similarity(), Some(0.0));
    }

    #[test]
    fn same_domain_is_the_weakest_edge() {
        // "Weak but useful": it connects everything on a busy site, and at full
        // strength it drowns the edges that mean something.
        assert!(EdgeReason::SameDomain.weight() < EdgeReason::SharedTag("x".into()).weight());
        assert!(EdgeReason::SharedTag("x".into()).weight()
                < EdgeReason::ChatReferencesPage.weight());
    }

    #[test]
    fn an_undirected_edge_is_the_same_edge_either_way_round() {
        // A shares a tag with B exactly when B shares one with A, so the set
        // must see one edge rather than two.
        let a = Edge::undirected("p_1", "p_2", EdgeReason::SameDomain);
        let b = Edge::undirected("p_2", "p_1", EdgeReason::SameDomain);
        assert_eq!(a, b);
        assert_eq!(prune(vec![a, b]).len(), 1);
    }

    #[test]
    fn weak_similarity_edges_are_pruned() {
        // A graph where everything connects to everything shows nothing.
        let strong = Edge::undirected("p_1", "p_2", EdgeReason::from_similarity(0.8));
        let weak = Edge::undirected("p_3", "p_4", EdgeReason::from_similarity(0.1));
        let kept = prune(vec![strong.clone(), weak]);
        assert_eq!(kept, vec![strong]);
    }

    #[test]
    fn a_non_similarity_edge_is_never_pruned_for_weakness() {
        // Same-domain is weak to DRAW but it is a fact, not a guess — pruning
        // it would hide a real relationship rather than a speculative one.
        let e = Edge::undirected("p_1", "p_2", EdgeReason::SameDomain);
        assert_eq!(prune(vec![e.clone()]), vec![e]);
    }

    #[test]
    fn a_self_edge_is_dropped() {
        // A naive pairwise scan produces these for free, and they render as a
        // dot on top of the node.
        let e = Edge::undirected("p_1", "p_1", EdgeReason::SameDomain);
        assert!(prune(vec![e]).is_empty());
    }

    #[test]
    fn a_preview_never_cuts_a_character_in_half() {
        // Slicing a UTF-8 string mid-codepoint panics — and previews come from
        // arbitrary page text.
        let text = "日本語のテキスト ".repeat(40);
        let p = preview(&text);
        assert!(p.chars().count() <= PREVIEW_CHARS + 1);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn a_preview_prefers_to_break_on_a_word() {
        // Ending mid-word reads as corrupted text rather than as a truncation.
        let text = "the quick brown fox ".repeat(20);
        let p = preview(&text);
        assert!(p.ends_with('…'));
        assert!(!p.contains("  "), "whitespace collapsed too");
    }

    #[test]
    fn a_short_preview_is_left_alone() {
        assert_eq!(preview("a short line"), "a short line");
        assert_eq!(preview(""), "");
    }

    #[test]
    fn newlines_and_runs_of_space_collapse() {
        // Page text arrives with arbitrary whitespace, and a preview holding a
        // newline breaks a single-line row.
        assert_eq!(preview("one\n\ntwo   three\t"), "one two three");
    }
}
