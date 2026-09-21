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
    let cut: Vec<char> = clean.chars().take(PREVIEW_CHARS).collect();
    // CHARACTERS COMPARED TO CHARACTERS. This used `cut.rfind(' ')`, which
    // returns a BYTE index, and compared it with `PREVIEW_CHARS / 2`, a
    // CHARACTER count. For ASCII they agree. For accented text they do not: in
    // "é"*40 + " " + a long URL, the space is at character 40 but byte 80, so
    // with a halfway mark of 60 it was wrongly taken as a good cut -- and the
    // preview kept 40 characters instead of 120. A note title in French
    // followed by a link lost two thirds of its preview.
    match cut.iter().rposition(|&c| c == ' ') {
        Some(i) if i > PREVIEW_CHARS / 2 => format!("{}…", cut[..i].iter().collect::<String>()),
        _ => format!("{}…", cut.iter().collect::<String>()),
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

/// The query surface over a built graph.
///
/// The port carried the SHAPE — node identity, the edge vocabulary, pruning —
/// and stopped there, so the frontend could build a graph and not ask it
/// anything. `connections.rs` has `GraphSnapshot` with `find_node`,
/// `edges_for_node`, `nodes_with_tag`, `related_to` and `pair_weight`, and an
/// Obsidian-style view is mostly those questions: the graph is the index, the
/// answers are the feature.
///
/// Owns nothing it does not need to. Scanning stays with the shell, exactly as
/// the module header says; this is a borrowed view over slices the caller
/// already holds.
pub struct Graph<'a> {
    pub nodes: &'a [Node],
    pub edges: &'a [Edge],
}

impl<'a> Graph<'a> {
    pub fn new(nodes: &'a [Node], edges: &'a [Edge]) -> Graph<'a> {
        Graph { nodes, edges }
    }

    pub fn find_node(&self, id: &str) -> Option<&'a Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Every edge touching `id`, in either direction.
    pub fn edges_for(&self, id: &str) -> Vec<&'a Edge> {
        self.edges.iter().filter(|e| e.from == id || e.to == id).collect()
    }

    /// The `n` nodes most connected to `id`, strongest first.
    ///
    /// Adapted rather than copied: the original sums `u32` weights, and this
    /// port's `EdgeReason::weight` returns `f32` because similarity edges
    /// carry a continuous score. The SUMMING is the part that matters and is
    /// kept — two pages sharing both a domain and a tag are more related than
    /// two sharing either alone, and taking the strongest single edge would
    /// throw that away.
    ///
    /// Ties break on id so the order is stable across runs. A "related" panel
    /// that reshuffles on every rebuild cannot be read.
    pub fn related_to(&self, id: &str, n: usize) -> Vec<(String, f32)> {
        if n == 0 || id.is_empty() {
            return Vec::new();
        }
        let mut scores: std::collections::HashMap<&str, f32> = Default::default();
        for e in self.edges {
            let other = if e.from == id {
                e.to.as_str()
            } else if e.to == id {
                e.from.as_str()
            } else {
                continue;
            };
            // A self-edge would otherwise rank a node against itself at the
            // top of its own related list.
            if other == id {
                continue;
            }
            *scores.entry(other).or_insert(0.0) += e.reason.weight();
        }
        let mut v: Vec<(String, f32)> =
            scores.into_iter().map(|(k, s)| (k.to_string(), s)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    /// Total weight between two nodes, summed across parallel edges.
    ///
    /// What a canvas uses for line thickness.
    pub fn pair_weight(&self, a: &str, b: &str) -> f32 {
        self.edges.iter()
            .filter(|e| (e.from == a && e.to == b) || (e.from == b && e.to == a))
            .map(|e| e.reason.weight())
            .sum()
    }

    /// Nodes carrying `tag`, case-insensitively.
    pub fn nodes_with_tag(&self, tag: &str) -> Vec<&'a Node> {
        let t = tag.trim().to_lowercase();
        self.nodes.iter()
            .filter(|n| n.tags.iter().any(|x| x.trim().to_lowercase() == t))
            .collect()
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

// ── building a graph from what this app holds ───────────────────────────────

/// One bookmark, as the graph needs it.
///
/// A pair rather than the browser's own type: this module should not
/// depend on the bookmark store to know that a bookmark is a title and a
/// url, and a caller with a different source can use it.
#[derive(Debug, Clone, PartialEq)]
pub struct PageIn {
    pub url: String,
    pub title: String,
}

/// One mail thread, likewise.
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadIn {
    pub conversation_id: String,
    pub subject: String,
    pub participants: Vec<String>,
}

/// The host part of a URL, or empty when there is not one.
///
/// Deliberately not a URL parser: this needs the authority, and pulling in
/// a crate to find one `/` would be the tail wagging the dog.
pub fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split('/').next().unwrap_or("").to_lowercase()
}

/// Build the graph.
///
/// This was seventy lines inside the binary, where nothing could reach it.
/// The decisions in it are the interesting part and none of them were
/// checkable: which node kinds are produced, which are deliberately NOT,
/// and which edges the data can honestly support.
pub fn build(pages: &[PageIn], threads: &[ThreadIn]) -> (Vec<Node>, Vec<Edge>) {
    let mut nodes = Vec::new();
    for b in pages {
        nodes.push(Node {
            id: node_id(NodeKind::Page, &b.url),
            kind: NodeKind::Page,
            title: b.title.clone(),
            subtitle: b.url.clone(),
            // `preview` truncates on a word boundary, so a long title does
            // not end mid-word. Built from the title because a bookmark is
            // all the app stored — an empty preview would be honest and
            // leaves the node unreadable at a glance.
            preview: preview(&b.title),
            // No tags: the app records none, and inventing one ("web",
            // "saved") would be a facet nobody can filter on meaningfully.
            tags: Vec::new(),
        });
    }
    for t in threads {
        nodes.push(Node {
            id: node_id(NodeKind::ChatThread, &t.conversation_id),
            kind: NodeKind::ChatThread,
            title: t.subject.clone(),
            subtitle: t.participants.join(", "),
            preview: preview(&t.subject),
            tags: Vec::new(),
        });
    }

    // SameDomain is the one edge this data can honestly produce: two
    // bookmarks on one host are related, weakly. The others need a link
    // the app does not record — a recording tagged with the page it was
    // captured on, a chat citing a URL — so producing them would be
    // asserting a connection nobody established.
    let mut edges = Vec::new();
    let hosts: Vec<(&str, String)> = pages.iter()
        .map(|b| (b.url.as_str(), host_of(&b.url)))
        .collect();
    for (i, (ua, ha)) in hosts.iter().enumerate() {
        for (ub, hb) in hosts.iter().skip(i + 1) {
            if !ha.is_empty() && ha == hb {
                edges.push(Edge {
                    from: node_id(NodeKind::Page, ua),
                    to: node_id(NodeKind::Page, ub),
                    reason: EdgeReason::SameDomain,
                });
            }
        }
    }
    (nodes, prune(edges))
}

#[cfg(test)]
mod build_tests {
    use super::*;

    fn page(url: &str, title: &str) -> PageIn {
        PageIn { url: url.into(), title: title.into() }
    }

    #[test]
    fn two_bookmarks_on_one_host_are_joined_and_others_are_not() {
        let (nodes, edges) = build(&[
            page("https://a.test/one", "One"),
            page("https://a.test/two", "Two"),
            page("https://b.test/three", "Three"),
        ], &[]);
        assert_eq!(nodes.len(), 3);
        assert_eq!(edges.len(), 1, "{edges:?}");
        assert_eq!(edges[0].reason, EdgeReason::SameDomain);
    }

    #[test]
    fn a_url_with_no_host_joins_nothing() {
        // Two `file:///…` bookmarks are not "the same site". An empty host
        // matching an empty host would join every local file to every
        // other one.
        let (_, edges) = build(&[page("notaurl", "A"), page("alsonot", "B")], &[]);
        assert!(edges.is_empty(), "{edges:?}");
    }

    #[test]
    fn the_host_is_compared_without_case_or_scheme() {
        assert_eq!(host_of("HTTPS://Example.COM/x"), "example.com");
        assert_eq!(host_of("example.com/x"), "example.com");
        let (_, edges) = build(&[page("https://A.test/1", "a"),
                                 page("http://a.TEST/2", "b")], &[]);
        assert_eq!(edges.len(), 1, "case or scheme split one host in two");
    }

    #[test]
    fn mail_threads_become_nodes_but_never_edges() {
        // The app records no link between a thread and anything else.
        // Producing one would assert a connection nobody established.
        let (nodes, edges) = build(&[], &[ThreadIn {
            conversation_id: "c1".into(),
            subject: "Engines".into(),
            participants: vec!["Ada".into(), "Bo".into()],
        }]);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, NodeKind::ChatThread);
        assert_eq!(nodes[0].subtitle, "Ada, Bo");
        assert!(edges.is_empty());
    }

    #[test]
    fn nothing_in_yields_nothing_out() {
        let (nodes, edges) = build(&[], &[]);
        assert!(nodes.is_empty() && edges.is_empty());
    }

    fn node(id: &str, tags: &[&str]) -> Node {
        Node { id: id.into(), kind: NodeKind::Page, title: id.into(),
               subtitle: String::new(), preview: String::new(),
               tags: tags.iter().map(|t| t.to_string()).collect() }
    }

    /// PARALLEL EDGES SUM. Two pages sharing both a domain and a tag are more
    /// related than two sharing either alone; taking the strongest single edge
    /// would throw that away. This is the rule the original states and the
    /// reason related_to is not just a max.
    #[test]
    fn related_to_sums_parallel_edges() {
        let nodes = vec![node("a", &[]), node("b", &[]), node("c", &[])];
        let edges = vec![
            Edge::undirected("a", "b", EdgeReason::SameDomain),
            Edge::undirected("a", "b", EdgeReason::SharedTag("t".into())),
            Edge::undirected("a", "c", EdgeReason::SameDomain),
        ];
        let g = Graph::new(&nodes, &edges);
        let r = g.related_to("a", 5);
        assert_eq!(r[0].0, "b", "two edges beat one: {r:?}");
        assert!(r[0].1 > r[1].1);
        assert_eq!(g.pair_weight("a", "b"), r[0].1, "pair_weight agrees");
    }

    /// Direction must not matter -- these relations are undirected, and
    /// `Edge::undirected` canonicalises the endpoints, so a query for either
    /// end has to find the other.
    #[test]
    fn related_to_finds_edges_from_either_end() {
        let nodes = vec![node("zzz", &[]), node("aaa", &[])];
        // Canonical order puts "aaa" first regardless of how it is written.
        let edges = vec![Edge::undirected("zzz", "aaa", EdgeReason::SameDomain)];
        let g = Graph::new(&nodes, &edges);
        assert_eq!(g.related_to("zzz", 5)[0].0, "aaa");
        assert_eq!(g.related_to("aaa", 5)[0].0, "zzz");
    }

    /// A related panel that reshuffles on every rebuild cannot be read.
    #[test]
    fn equal_scores_break_ties_on_id_so_the_order_is_stable() {
        let nodes = vec![node("x", &[]), node("b", &[]), node("a", &[])];
        let edges = vec![
            Edge::undirected("x", "b", EdgeReason::SameDomain),
            Edge::undirected("x", "a", EdgeReason::SameDomain),
        ];
        let g = Graph::new(&nodes, &edges);
        let ids: Vec<String> = g.related_to("x", 5).into_iter().map(|(i, _)| i).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    /// A node must not top its own related list.
    #[test]
    fn a_self_edge_does_not_rank_a_node_against_itself() {
        let nodes = vec![node("a", &[])];
        let edges = vec![Edge { from: "a".into(), to: "a".into(), reason: EdgeReason::SameDomain }];
        assert!(Graph::new(&nodes, &edges).related_to("a", 5).is_empty());
    }

    #[test]
    fn an_unknown_or_unconnected_node_relates_to_nothing() {
        let nodes = vec![node("a", &[]), node("b", &[])];
        let edges = vec![Edge::undirected("a", "b", EdgeReason::SameDomain)];
        let g = Graph::new(&nodes, &edges);
        assert!(g.related_to("ghost", 5).is_empty());
        assert!(g.related_to("a", 0).is_empty(), "n=0 asks for nothing");
        assert!(g.related_to("", 5).is_empty());
        assert_eq!(g.pair_weight("a", "ghost"), 0.0);
    }

    #[test]
    fn nodes_are_found_by_id_and_by_tag_case_insensitively() {
        let nodes = vec![node("a", &["Rust", "cli"]), node("b", &["rust"])];
        let g = Graph::new(&nodes, &[]);
        assert_eq!(g.find_node("a").unwrap().id, "a");
        assert!(g.find_node("nope").is_none());
        assert_eq!(g.nodes_with_tag("RUST").len(), 2, "tags match regardless of case");
        assert_eq!(g.nodes_with_tag(" cli ").len(), 1, "and are trimmed");
    }

    #[test]
    fn edges_for_returns_both_directions() {
        let nodes = vec![node("a", &[]), node("b", &[]), node("c", &[])];
        let edges = vec![
            Edge::undirected("a", "b", EdgeReason::SameDomain),
            Edge::undirected("b", "c", EdgeReason::SharedTag("t".into())),
        ];
        assert_eq!(Graph::new(&nodes, &edges).edges_for("b").len(), 2);
    }

#[cfg(test)]
mod preview_units_tests {
    use super::*;

    /// THE MISMATCH. Accented text then a long unbroken token: the space is
    /// early in characters but late in bytes, and a byte index compared to a
    /// character count cut the preview to a third of its length.
    #[test]
    fn accented_text_before_a_long_token_keeps_its_full_preview() {
        let t = format!("{} {}", "é".repeat(40), "x".repeat(200));
        let p = preview(&t);
        assert_eq!(p.chars().count(), PREVIEW_CHARS + 1, "{} chars: {p}", p.chars().count());
    }

    /// A space past halfway is still used, so a preview does not end mid-word.
    #[test]
    fn a_late_space_is_still_a_clean_cut() {
        let t = "word ".repeat(40);
        let p = preview(&t);
        assert!(p.ends_with('…'));
        assert!(!p.trim_end_matches('…').ends_with(' '), "{p:?}");
    }

    /// Short text and CJK are untouched or cut safely.
    #[test]
    fn short_and_multibyte_text_is_safe() {
        assert_eq!(preview("hi"), "hi");
        let cjk = "日本語".repeat(80);
        assert!(preview(&cjk).chars().count() <= PREVIEW_CHARS + 1);
    }
}
}
