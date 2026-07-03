//! Knowledge-web link graph: extract `[[wikilinks]]` from a corpus of
//! documents and build a navigable graph — forward links ("what does
//! this doc point to?") AND backlinks ("what points HERE?").
//!
//! Backlinks are the thing that turns one-way links into a WEB: in
//! the user's own notes, `[[reVos]]` appears in a lecture note AND
//! the reVos page benefits from knowing that note references it. The
//! existing renderer (manpages::detect_wikilink) handles displaying a
//! single link inline; this module is the corpus-level graph on top.
//!
//! Pure + standalone (no IO, no crate deps) so it's fully testable.
//! Slug normalization mirrors manpages::slugify so a `[[Target]]`
//! resolves to the same slug whether rendered inline or graphed here.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// Normalize a wikilink target to a slug. MUST match
/// manpages::slugify so inline rendering and the graph agree on what
/// `[[ECSE-2660]]` resolves to. (Lowercase; spaces → '-'; keep
/// [a-z0-9_-]; drop everything else.)
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let lc = c.to_ascii_lowercase();
        if lc.is_ascii_alphanumeric() || lc == '-' || lc == '_' {
            out.push(lc);
        } else if c.is_whitespace() {
            out.push('-');
        }
    }
    out
}

/// Extract all wikilink target slugs from a document body, in order
/// of first appearance, de-duplicated. Handles `[[Target]]` and
/// `[[Target|Display]]`; a link broken by a newline before its `]]`
/// is not a link (matches the renderer's rule).
pub fn extract_links(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some((slug, after)) = parse_link_at(body, i) {
                if !slug.is_empty() && seen.insert(slug.clone()) {
                    out.push(slug);
                }
                i = after;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Parse a `[[...]]` starting at `start` (first '['). Returns the
/// target slug and the byte index just past the closing `]]`.
/// Mirrors manpages::detect_wikilink's rules (newline cancels).
fn parse_link_at(s: &str, start: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if start + 1 >= bytes.len() { return None; }
    if bytes[start] != b'[' || bytes[start + 1] != b'[' { return None; }
    let body_start = start + 2;
    let mut i = body_start;
    while i + 1 < bytes.len() {
        if bytes[i] == b']' && bytes[i + 1] == b']' {
            let body = &s[body_start..i];
            let target_raw = match body.find('|') {
                Some(pipe) => &body[..pipe],
                None => body,
            };
            return Some((slugify(target_raw), i + 2));
        }
        if bytes[i] == b'\n' { return None; }
        i += 1;
    }
    None
}

/// The corpus link graph: forward links per doc + reverse (backlink)
/// index. Docs are keyed by slug (the same slug a `[[link]]` to them
/// would produce), so links resolve across the corpus.
#[derive(Debug, Default, Clone)]
pub struct LinkGraph {
    /// doc-slug → set of slugs it links TO (forward links).
    forward: BTreeMap<String, BTreeSet<String>>,
    /// doc-slug → set of slugs that link TO it (backlinks).
    backward: BTreeMap<String, BTreeSet<String>>,
    /// All known doc slugs (so we can distinguish a link to an
    /// existing doc from a "dangling" link to a not-yet-created one).
    known: BTreeSet<String>,
}

impl LinkGraph {
    pub fn new() -> Self {
        LinkGraph::default()
    }

    /// Add a document to the graph. `title` is slugified to the doc's
    /// own slug; `body` is scanned for outgoing wikilinks. Calling
    /// twice for the same slug replaces that doc's forward links (and
    /// updates backlinks accordingly) — so re-indexing an edited doc
    /// is correct, not additive.
    pub fn add_document(&mut self, title: &str, body: &str) {
        let slug = slugify(title);
        if slug.is_empty() { return; }
        self.known.insert(slug.clone());

        // If this doc was already indexed, remove its old forward
        // links from the backward index first (so edits don't leave
        // stale backlinks).
        if let Some(old) = self.forward.get(&slug) {
            for target in old.clone() {
                if let Some(set) = self.backward.get_mut(&target) {
                    set.remove(&slug);
                }
            }
        }

        let links: BTreeSet<String> = extract_links(body).into_iter().collect();
        // Update backward index for the new links.
        for target in &links {
            self.backward.entry(target.clone()).or_default().insert(slug.clone());
        }
        self.forward.insert(slug, links);
    }

    /// Forward links from a doc (slugs it points to), sorted.
    pub fn links_from(&self, title: &str) -> Vec<String> {
        let slug = slugify(title);
        self.forward.get(&slug)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Backlinks to a doc (slugs that point to it), sorted. THIS is
    /// the knowledge-web payoff: "what references this?"
    pub fn backlinks(&self, title: &str) -> Vec<String> {
        let slug = slugify(title);
        self.backward.get(&slug)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Links that point to a slug with NO corresponding document —
    /// "dangling" links (a [[Target]] you've referenced but not
    /// written yet). Useful as a "notes to create" list, exactly the
    /// learn-by-connecting workflow the user's notes embody.
    pub fn dangling_targets(&self) -> Vec<String> {
        let mut out: Vec<String> = self.backward.keys()
            .filter(|target| !self.known.contains(*target))
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Number of indexed documents.
    pub fn doc_count(&self) -> usize {
        self.known.len()
    }

    /// JSON for QML: {"slug":..,"forward":[..],"backlinks":[..]} for
    /// one doc. Empty arrays if the doc isn't indexed.
    pub fn doc_json(&self, title: &str) -> String {
        let slug = slugify(title);
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let arr = |v: &[String]| -> String {
            let items: Vec<String> = v.iter()
                .map(|s| format!("\"{}\"", esc(s))).collect();
            format!("[{}]", items.join(","))
        };
        format!("{{\"slug\":\"{}\",\"forward\":{},\"backlinks\":{}}}",
            esc(&slug),
            arr(&self.links_from(title)),
            arr(&self.backlinks(title)))
    }

    /// v0.1.894: degree of a slug — outgoing wikilinks + incoming backlinks.
    /// Edges to/from not-yet-created (dangling) targets still count: a note
    /// that links out to many things is a hub of intent even before those
    /// notes exist, which is what "connectedness" means in a zettelkasten.
    fn degree(&self, slug: &str) -> usize {
        self.forward.get(slug).map(|s| s.len()).unwrap_or(0)
            + self.backward.get(slug).map(|s| s.len()).unwrap_or(0)
    }

    /// v0.1.894: degree centrality of one doc by title. Unknown docs score 0.
    /// The simplest centrality that answers "is this a hub?" — and the one the
    /// graph view, the walkthrough, and a "show me the key notes" voice query
    /// all want first. (Eigenvector/PageRank can layer on later; degree is the
    /// honest, cheap baseline and needs no iteration.)
    pub fn degree_centrality(&self, title: &str) -> usize {
        self.degree(&slugify(title))
    }

    /// v0.1.894: every indexed doc ranked by degree centrality, most-connected
    /// first; ties broken by slug so the order is stable. The "where are the
    /// hubs" view of the corpus — and the seed order for [`walkthrough`].
    pub fn centrality_ranking(&self) -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = self.known.iter()
            .map(|slug| (slug.clone(), self.degree(slug)))
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    }

    /// Known docs directly linked to `slug` in either direction (forward or
    /// back), excluding self-loops. Dangling (uncreated) targets are skipped —
    /// the walk can only step to notes that actually exist.
    fn known_neighbors(&self, slug: &str) -> Vec<String> {
        let mut set: BTreeSet<String> = BTreeSet::new();
        if let Some(f) = self.forward.get(slug) {
            for t in f { if self.known.contains(t) { set.insert(t.clone()); } }
        }
        if let Some(b) = self.backward.get(slug) {
            for t in b { if self.known.contains(t) { set.insert(t.clone()); } }
        }
        set.remove(slug);
        set.into_iter().collect()
    }

    /// v0.1.894: a guided visiting order over all indexed docs — the spine of a
    /// "graph walkthrough" (a voice tour, a replay, guided reading). Starts at
    /// the most-central hub and walks its connected neighbourhood breadth-first
    /// in centrality order, so the tour follows the link web outward from its
    /// densest point. When a connected component is exhausted it jumps to the
    /// most-central not-yet-visited doc, so disconnected clusters are still
    /// covered, each entered at its own hub. Only KNOWN docs are visited;
    /// dangling targets surface separately via [`dangling_targets`].
    /// Deterministic: BTree iteration + explicit centrality/slug tiebreaks.
    ///
    /// Parser-agnostic by construction — anything that feeds (node, links)
    /// into the graph gets a walkthrough for free, so the same call orders a
    /// zettelkasten note web AND an email attachment graph.
    pub fn walkthrough(&self) -> Vec<String> {
        use std::collections::VecDeque;
        let ranking = self.centrality_ranking();
        let mut visited: BTreeSet<String> = BTreeSet::new();
        let mut order: Vec<String> = Vec::new();
        for (seed, _) in &ranking {
            if visited.contains(seed) { continue; }
            let mut queue: VecDeque<String> = VecDeque::new();
            queue.push_back(seed.clone());
            visited.insert(seed.clone());
            while let Some(cur) = queue.pop_front() {
                order.push(cur.clone());
                let mut neigh = self.known_neighbors(&cur);
                // hub-first within the frontier: higher degree, then slug.
                neigh.sort_by(|a, b| self.degree(b).cmp(&self.degree(a)).then_with(|| a.cmp(b)));
                for n in neigh {
                    if visited.insert(n.clone()) {
                        queue.push_back(n);
                    }
                }
            }
        }
        order
    }

    /// v0.1.894: the walkthrough order as a JSON array of slugs, for QML to
    /// drive a guided tour / replay control. Mirrors [`doc_json`]'s escaping.
    pub fn walkthrough_json(&self) -> String {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let items: Vec<String> = self.walkthrough().iter()
            .map(|s| format!("\"{}\"", esc(s))).collect();
        format!("[{}]", items.join(","))
    }

    /// v0.1.895: build a link graph from a niftyplot graph source — the adapter
    /// that lets the centrality + walkthrough machinery tour ANY niftyplot
    /// graph: an inbox sender / attachment graph from `mail_panes`, a fenced
    /// niftyplot block in a note, a hand-drawn diagram. Node ids become doc
    /// slugs (the SAME slugify a `[[link]]` uses, so it composes with the note
    /// web); each directed edge `from -> to` becomes one forward link. Edge
    /// labels/styles are ignored — centrality is about connectivity, not weight.
    /// Nodes with no outgoing edges are still registered so they appear in the
    /// walk, and edge endpoints that were never declared as nodes are registered
    /// too. Reuses `add_document` verbatim (via a synthesized wikilink body) so
    /// the forward/back/known/re-index logic is exactly the corpus path.
    pub fn from_niftyplot(source: &str) -> LinkGraph {
        let doc = crate::niftyplot::parse(source);
        // The niftyplot edge grammar can leave a trailing `:` / whitespace on a
        // target id when an edge carries a `: "label"` suffix (e.g. the inbox
        // sender graph's `alice -> You : "5"` yields a target of `You :`).
        // Normalize endpoints so that target matches its declared node — trim
        // whitespace and any trailing colon. Safe: niftyplot ids never end in
        // a colon (it's the label separator).
        let clean = |raw: &str| raw.trim().trim_end_matches(':').trim().to_string();
        // Outgoing-target lists per node id (declared nodes seed empty lists;
        // edges append and also register their endpoints).
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for n in &doc.nodes {
            let id = clean(&n.id);
            if !id.is_empty() { out.entry(id).or_default(); }
        }
        for e in &doc.edges {
            let (from, to) = (clean(&e.from), clean(&e.to));
            if from.is_empty() || to.is_empty() { continue; }
            out.entry(from).or_default().push(to.clone());
            out.entry(to).or_default();
        }
        let mut g = LinkGraph::new();
        for (id, targets) in &out {
            let body: String = targets.iter().map(|t| format!("[[{}]] ", t)).collect();
            g.add_document(id, &body);
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_matches_renderer_rules() {
        assert_eq!(slugify("ECSE-2660"), "ecse-2660");
        assert_eq!(slugify("My Notes"), "my-notes");
        assert_eq!(slugify("reVos"), "revos");
        // punctuation dropped
        assert_eq!(slugify("foo, bar!"), "foo-bar");
    }

    #[test]
    fn extract_basic_and_aliased() {
        let body = "see [[ECSE-2660]] and [[reVos|the video tool]] today";
        assert_eq!(extract_links(body), vec!["ecse-2660", "revos"]);
    }

    #[test]
    fn extract_dedups_in_order() {
        let body = "[[a]] then [[b]] then [[a]] again";
        assert_eq!(extract_links(body), vec!["a", "b"]);
    }

    #[test]
    fn newline_cancels_link() {
        // a [[ broken by a newline before ]] is not a link
        let body = "[[unclosed\nstill text]] no";
        assert_eq!(extract_links(body), Vec::<String>::new());
    }

    #[test]
    fn backlinks_form_a_web() {
        let mut g = LinkGraph::new();
        g.add_document("L09", "lecture, saved time with [[reVos]] and [[ECSE-2660]]");
        g.add_document("reVos", "the video tool, see [[kss]]");
        // forward
        assert_eq!(g.links_from("L09"), vec!["ecse-2660", "revos"]);
        assert_eq!(g.links_from("reVos"), vec!["kss"]);
        // backlinks: reVos is referenced by L09
        assert_eq!(g.backlinks("reVos"), vec!["l09"]);
        // kss is referenced by reVos
        assert_eq!(g.backlinks("kss"), vec!["revos"]);
        // ECSE-2660 referenced by L09
        assert_eq!(g.backlinks("ECSE-2660"), vec!["l09"]);
    }

    #[test]
    fn reindex_replaces_not_appends() {
        let mut g = LinkGraph::new();
        g.add_document("note", "links to [[alpha]]");
        assert_eq!(g.backlinks("alpha"), vec!["note"]);
        // edit the note: now it links to beta, not alpha
        g.add_document("note", "links to [[beta]] now");
        // alpha should no longer have note as a backlink (stale removed)
        assert_eq!(g.backlinks("alpha"), Vec::<String>::new());
        assert_eq!(g.backlinks("beta"), vec!["note"]);
        assert_eq!(g.links_from("note"), vec!["beta"]);
    }

    #[test]
    fn dangling_targets_are_uncreated_notes() {
        let mut g = LinkGraph::new();
        g.add_document("index", "see [[written]] and [[not-yet]]");
        g.add_document("written", "I exist");
        // "not-yet" is referenced but has no document
        assert_eq!(g.dangling_targets(), vec!["not-yet"]);
        // once created, it's no longer dangling
        g.add_document("not-yet", "now I exist");
        assert_eq!(g.dangling_targets(), Vec::<String>::new());
    }

    #[test]
    fn doc_count_tracks_known() {
        let mut g = LinkGraph::new();
        assert_eq!(g.doc_count(), 0);
        g.add_document("a", "[[b]]");  // b is only referenced, not a doc
        assert_eq!(g.doc_count(), 1);
        g.add_document("b", "hi");
        assert_eq!(g.doc_count(), 2);
    }

    #[test]
    fn doc_json_wellformed() {
        let mut g = LinkGraph::new();
        g.add_document("L09", "[[reVos]] [[kss]]");
        g.add_document("reVos", "x");
        let j = g.doc_json("L09");
        assert!(j.contains("\"slug\":\"l09\""));
        assert!(j.contains("\"revos\""));
        assert!(j.contains("\"kss\""));
        // balanced braces/brackets
        assert_eq!(j.matches('{').count(), j.matches('}').count());
        assert_eq!(j.matches('[').count(), j.matches(']').count());
    }

    #[test]
    fn empty_title_ignored() {
        let mut g = LinkGraph::new();
        g.add_document("", "[[x]]");
        assert_eq!(g.doc_count(), 0);
    }

    // v0.1.894: degree centrality ranks hubs first.
    #[test]
    fn centrality_ranks_hubs_first() {
        let mut g = LinkGraph::new();
        g.add_document("a", "[[hub]]");
        g.add_document("b", "[[hub]]");
        g.add_document("c", "[[hub]]");
        g.add_document("hub", "[[x]]");
        g.add_document("x", "leaf");
        // hub: forward {x}=1 + backlinks {a,b,c}=3 → 4
        assert_eq!(g.degree_centrality("hub"), 4);
        assert_eq!(g.degree_centrality("a"), 1);
        assert_eq!(g.degree_centrality("x"), 1);
        assert_eq!(g.degree_centrality("missing"), 0);
        let rank = g.centrality_ranking();
        assert_eq!(rank[0], ("hub".to_string(), 4));
        assert_eq!(rank.len(), 5); // a,b,c,hub,x
        // degree-1 docs tie → stable slug order among the tail
        let tail: Vec<&str> = rank[1..].iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(tail, vec!["a", "b", "c", "x"]);
    }

    // v0.1.894: the walkthrough starts at the hub and visits every known doc once.
    #[test]
    fn walkthrough_starts_at_hub_and_covers_all() {
        let mut g = LinkGraph::new();
        g.add_document("a", "[[hub]]");
        g.add_document("b", "[[hub]]");
        g.add_document("hub", "[[x]]");
        g.add_document("x", "leaf");
        let w = g.walkthrough();
        assert_eq!(w[0], "hub", "densest node first");
        assert_eq!(w.len(), 4, "every known doc once");
        let mut uniq = w.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq, vec!["a", "b", "hub", "x"]);
    }

    // v0.1.894: disconnected clusters are each entered at their own hub, the
    // denser component first, with no interleaving.
    #[test]
    fn walkthrough_covers_disconnected_components() {
        let mut g = LinkGraph::new();
        g.add_document("q", "[[p]] [[r]]"); // q is the degree-2 hub of cluster 1
        g.add_document("p", "leaf");
        g.add_document("r", "leaf");
        g.add_document("s", "[[t]]"); // cluster 2: s → t
        g.add_document("t", "leaf");
        let w = g.walkthrough();
        assert_eq!(w[0], "q", "overall densest node first");
        assert_eq!(w.len(), 5);
        let pos = |k: &str| w.iter().position(|x| x == k).unwrap();
        // cluster 1 (q,p,r) fully precedes cluster 2 (s,t)
        assert!(pos("p") < pos("s") && pos("r") < pos("s"));
    }

    #[test]
    fn walkthrough_json_and_empty() {
        let g = LinkGraph::new();
        assert_eq!(g.walkthrough(), Vec::<String>::new());
        assert_eq!(g.walkthrough_json(), "[]");
        let mut g = LinkGraph::new();
        g.add_document("hub", "[[x]]");
        g.add_document("x", "leaf");
        assert_eq!(g.walkthrough_json(), "[\"hub\",\"x\"]");
    }

    // v0.1.895: the niftyplot adapter — an inbox sender graph (every sender
    // points at "You") tours from the hub outward. This is "graph walkthrough
    // email integration" exercised end-to-end on the real DSL format.
    #[test]
    #[test]
    fn from_niftyplot_inbox_graph_walks_from_you() {
        let src = "%kind: graph\n%title: \"Inbox senders\"\n\
                   You: circle \"You\"\n\
                   alice => You : \"5\"\n\
                   bob -> You : \"1\"\n\
                   carol -> You : \"2\"\n";
        let g = LinkGraph::from_niftyplot(src);
        assert_eq!(g.doc_count(), 4); // You + 3 senders
        // "You" is the hub: 3 senders point in.
        assert_eq!(g.degree_centrality("you"), 3);
        assert_eq!(g.degree_centrality("alice"), 1);
        assert_eq!(g.backlinks("you"), vec!["alice", "bob", "carol"]);
        let w = g.walkthrough();
        assert_eq!(w[0], "you", "tour starts at the inbox hub");
        assert_eq!(w.len(), 4);
    }
}
