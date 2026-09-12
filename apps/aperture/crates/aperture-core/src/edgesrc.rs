//! edgesrc — the sources that actually feed [`crate::entity::Graph`].
//!
//! The graph shipped with one [`EdgeSource`](crate::entity::EdgeSource), in
//! its own tests. These are the real ones, built from what this crate can
//! already see: an outline, a project search, and git.
//!
//! ## Why these take DATA and not the modules that produce it
//!
//! `Syntax::outline` lives behind the `tree-sitter` feature. A source that
//! called it directly would be feature-gated, and so would every test of it
//! — which means the interesting logic (span nesting, which item a hit falls
//! inside, how a commit attaches to a function) would only be exercised in
//! one of the two builds this crate ships.
//!
//! So each source here takes the plain rows it needs and the wiring that
//! fetches those rows stays with the caller. [`OutlineRow`] is structurally
//! `syntax::OutlineItem` minus `sig`, and [`OutlineRow::from_items`] does
//! that conversion in one place, under the feature. Same shape as
//! `bucket::locate`, and for the same reason.
//!
//! ## What each one is worth
//!
//! * [`OutlineEdges`] — containment. The skeleton: every other source
//!   attaches to items it has already established.
//! * [`SearchEdges`] — the weakest and most honest relation. A grep hit says
//!   a string appeared, and attributing it to the enclosing item is a guess
//!   the reader can check.
//! * [`GitEdges`] — time. Which commit touched an item, and which one
//!   introduced it.
//!
//! Callers (`Calls`/`CalledBy`) are NOT here: the exact answer needs
//! `textDocument/references` from `aperture-lsp`, and the approximate tier
//! needs a tokenizer. Both belong with the crates that own those, and both
//! produce [`Edge`]s that merge into the same graph.

use crate::entity::{Edge, EdgeSource, EntityId, Relation, Site};
use std::collections::BTreeMap;

/// One item in a file's outline: what [`OutlineEdges`] needs, and nothing
/// that would tie it to a parser.
///
/// `end_line` is INCLUSIVE, matching `syntax::OutlineItem`. Getting that
/// wrong makes nesting off by one item at every boundary, which looks like
/// a subtle mis-parse rather than an off-by-one, so it is worth stating
/// twice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineRow {
    pub kind: String,
    pub name: String,
    pub line: usize,
    pub end_line: usize,
}

impl OutlineRow {
    pub fn new(kind: &str, name: &str, line: usize, end_line: usize) -> OutlineRow {
        OutlineRow { kind: kind.into(), name: name.into(), line, end_line }
    }

    /// The one place `OutlineKind` becomes a string. Kept here rather than
    /// as a method on the enum because the enum is defined twice — once by
    /// `ts`, once by the featureless stub — and a method would have to be
    /// written twice and kept in step.
    pub fn from_items(items: &[crate::syntax::OutlineItem]) -> Vec<OutlineRow> {
        use crate::syntax::OutlineKind as K;
        items
            .iter()
            .map(|i| OutlineRow {
                kind: match i.kind {
                    K::Fn => "fn",
                    K::Struct => "struct",
                    K::Enum => "enum",
                    K::Trait => "trait",
                    K::Impl => "impl",
                    K::Mod => "mod",
                    K::Const => "const",
                }
                .to_string(),
                name: i.name.clone(),
                line: i.line,
                end_line: i.end_line,
            })
            .collect()
    }

    fn id(&self, path: &str) -> EntityId {
        EntityId::item(path, &self.kind, &self.name)
    }

    fn contains(&self, other: &OutlineRow) -> bool {
        // Enclosing AND strictly larger. The `self != other` version of this
        // is the obvious one and it is wrong: two DISTINCT items with
        // identical spans — which a mis-parse produces, an `impl` and its
        // single method reported at the same extent — are not equal, so each
        // would contain the other and the containment tree would have a
        // cycle in it. Caught by the test below, which was written expecting
        // the guard to work and did not pass.
        //
        // Equal spans therefore nest under neither: both fall back to the
        // file. Coarser than the truth if the parse was right, and safe if
        // it was not.
        let encloses = self.line <= other.line && other.end_line <= self.end_line;
        let strictly_larger = self.line < other.line || other.end_line < self.end_line;
        encloses && strictly_larger
    }

    fn span(&self) -> usize {
        self.end_line.saturating_sub(self.line)
    }
}

/// A file's outline, as containment edges.
///
/// Produces `File Contains Item` for top-level items and `Item Contains
/// Item` for nesting — a method inside an `impl`, a function inside a `mod`.
/// The parent chosen is the SMALLEST enclosing item, not the first found:
/// with `mod` wrapping `impl` wrapping `fn`, attaching the fn to the mod
/// would flatten exactly the structure this is here to capture.
pub struct OutlineEdges {
    by_file: BTreeMap<String, Vec<OutlineRow>>,
}

impl OutlineEdges {
    pub fn new() -> OutlineEdges {
        OutlineEdges { by_file: BTreeMap::new() }
    }

    /// Add (or replace) one file's outline. Replacing matters: this gets
    /// re-fed after an edit, and two versions of the same file's items in
    /// one source would produce edges for items that no longer exist.
    pub fn add_file(&mut self, path: impl Into<String>, rows: Vec<OutlineRow>) {
        self.by_file.insert(path.into(), rows);
    }

    /// Everything, for seeding a graph in one go — the walk `Graph::expand`
    /// does per entity is for interactive use, not for the initial fill.
    pub fn all_edges(&self) -> Vec<Edge> {
        self.by_file.keys().flat_map(|p| self.file_edges(p)).collect()
    }

    fn file_edges(&self, path: &str) -> Vec<Edge> {
        let Some(rows) = self.by_file.get(path) else { return Vec::new() };
        let file = EntityId::file(path);
        let mut out = Vec::new();
        for r in rows {
            let parent = rows
                .iter()
                .filter(|p| p.contains(r))
                .min_by_key(|p| p.span())
                .map(|p| p.id(path))
                .unwrap_or_else(|| file.clone());
            out.push(
                Edge::new(parent, Relation::Contains, r.id(path), self.name())
                    .at(Site::new(path, r.line as u32, 0)),
            );
        }
        out
    }
}

impl Default for OutlineEdges {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeSource for OutlineEdges {
    fn name(&self) -> &str {
        "outline"
    }

    fn edges_from(&mut self, id: &EntityId) -> Vec<Edge> {
        // Asking about a file gives its whole outline; asking about an item
        // gives the file's edges too, and the graph keeps only the ones that
        // touch what the caller wanted. Filtering here instead would mean
        // recomputing the parent chain per item for no gain — the outline is
        // already in memory.
        match id.path() {
            Some(p) => self.file_edges(p),
            None => Vec::new(),
        }
    }
}

/// Project-search hits, as `TextMatch` edges.
///
/// A hit is attributed to the smallest outline item whose span contains its
/// line, falling back to the file. That attribution is a GUESS — the item's
/// span says where the text is, not that the item means anything by it — and
/// `TextMatch` is the weakest relation for exactly that reason. It is still
/// worth making: "this string appears in `resolve_collision`" is a fact a
/// reader can check at a glance, where "this string appears at
/// physics.rs:412" is one they have to go and look up.
pub struct SearchEdges {
    query: String,
    outlines: BTreeMap<String, Vec<OutlineRow>>,
    hits: Vec<(String, usize, usize)>, // (rel path, line, col)
}

impl SearchEdges {
    /// `query` becomes the target entity, so every hit for one search points
    /// at one node — which is what makes "these forty places are the same
    /// search" visible in the graph rather than forty unrelated marks.
    pub fn new(query: impl Into<String>) -> SearchEdges {
        SearchEdges { query: query.into(), outlines: BTreeMap::new(), hits: Vec::new() }
    }

    /// Optional context. Without it every hit attaches to its file, which is
    /// correct, just coarser.
    pub fn with_outline(&mut self, path: impl Into<String>, rows: Vec<OutlineRow>) {
        self.outlines.insert(path.into(), rows);
    }

    pub fn add_hit(&mut self, rel: impl Into<String>, line: usize, col: usize) {
        self.hits.push((rel.into(), line, col));
    }

    /// Fold in a `project_search` result set directly.
    pub fn add_hits(&mut self, hits: &[crate::project_search::Hit]) {
        for h in hits {
            self.add_hit(h.rel.clone(), h.line, h.col);
        }
    }

    /// The entity every hit in this search points at.
    pub fn target(&self) -> EntityId {
        EntityId::Extern { origin: "search".into(), path: self.query.clone() }
    }

    fn owner(&self, path: &str, line: usize) -> EntityId {
        self.outlines
            .get(path)
            .and_then(|rows| {
                rows.iter()
                    .filter(|r| r.line <= line && line <= r.end_line)
                    .min_by_key(|r| r.span())
                    .map(|r| r.id(path))
            })
            .unwrap_or_else(|| EntityId::file(path))
    }

    pub fn all_edges(&self) -> Vec<Edge> {
        let target = self.target();
        self.hits
            .iter()
            .map(|(path, line, col)| {
                Edge::new(self.owner(path, *line), Relation::TextMatch, target.clone(), "search")
                    .at(Site::new(path, *line as u32, *col as u32))
                    .approx()
            })
            .collect()
    }
}

impl EdgeSource for SearchEdges {
    fn name(&self) -> &str {
        "search"
    }
    fn edges_from(&mut self, id: &EntityId) -> Vec<Edge> {
        self.all_edges().into_iter().filter(|e| &e.from == id).collect()
    }
    fn edges_to(&mut self, id: &EntityId) -> Vec<Edge> {
        if *id == self.target() {
            self.all_edges()
        } else {
            Vec::new()
        }
    }
}

/// Git history, as `Touched` and `Introduced` edges.
///
/// Blame times per line get folded up to the item that contains the line, by
/// MAXIMUM — the same rule `aperture-scene`'s room heat already uses, and
/// for the same reason: a function is as recently touched as its most
/// recently touched line, not as its average, which would make a one-line
/// fix to an old function look old.
pub struct GitEdges {
    /// path → (commit oid, the lines it last touched)
    touches: Vec<(String, String, Vec<usize>)>,
    outlines: BTreeMap<String, Vec<OutlineRow>>,
    introduced: Vec<(String, String)>, // (path, oid)
}

impl GitEdges {
    pub fn new() -> GitEdges {
        GitEdges { touches: Vec::new(), outlines: BTreeMap::new(), introduced: Vec::new() }
    }

    pub fn with_outline(&mut self, path: impl Into<String>, rows: Vec<OutlineRow>) {
        self.outlines.insert(path.into(), rows);
    }

    /// A commit and the 0-based lines of `path` it is the latest author of.
    pub fn add_touch(&mut self, path: impl Into<String>, oid: impl Into<String>, lines: Vec<usize>) {
        self.touches.push((path.into(), oid.into(), lines));
    }

    /// The commit a file first appeared in.
    ///
    /// Per FILE, not per item: git's rename and content tracking can tell you
    /// when a file appeared, but "when did this function first exist" needs a
    /// pickaxe search over the whole history, which is a different cost class
    /// and a different feature. Claiming it per item from data that cannot
    /// support it would be the kind of confident wrongness this graph's
    /// `approx` flag exists to avoid.
    pub fn add_introduced(&mut self, path: impl Into<String>, oid: impl Into<String>) {
        self.introduced.push((path.into(), oid.into()));
    }

    fn owners(&self, path: &str, lines: &[usize]) -> Vec<(EntityId, usize)> {
        let mut best: BTreeMap<EntityId, usize> = BTreeMap::new();
        for line in lines {
            let owner = self
                .outlines
                .get(path)
                .and_then(|rows| {
                    rows.iter()
                        .filter(|r| r.line <= *line && *line <= r.end_line)
                        .min_by_key(|r| r.span())
                        .map(|r| r.id(path))
                })
                .unwrap_or_else(|| EntityId::file(path));
            let e = best.entry(owner).or_insert(*line);
            *e = (*e).max(*line);
        }
        best.into_iter().collect()
    }

    pub fn all_edges(&self) -> Vec<Edge> {
        let mut out = Vec::new();
        for (path, oid, lines) in &self.touches {
            for (owner, line) in self.owners(path, lines) {
                out.push(
                    Edge::new(
                        owner,
                        Relation::TouchedBy,
                        EntityId::Commit { oid: oid.clone() },
                        "git",
                    )
                    .at(Site::new(path, line as u32, 0)),
                );
            }
        }
        for (path, oid) in &self.introduced {
            out.push(Edge::new(
                EntityId::file(path),
                Relation::IntroducedBy,
                EntityId::Commit { oid: oid.clone() },
                "git",
            ));
        }
        out
    }
}

impl Default for GitEdges {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeSource for GitEdges {
    fn name(&self) -> &str {
        "git"
    }
    fn edges_from(&mut self, id: &EntityId) -> Vec<Edge> {
        self.all_edges().into_iter().filter(|e| &e.from == id).collect()
    }
    fn edges_to(&mut self, id: &EntityId) -> Vec<Edge> {
        self.all_edges().into_iter().filter(|e| &e.to == id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Graph;

    /// mod outer { impl Thing { fn method } }  plus a top-level fn.
    fn nested() -> Vec<OutlineRow> {
        vec![
            OutlineRow::new("mod", "outer", 0, 40),
            OutlineRow::new("impl", "Thing", 5, 30),
            OutlineRow::new("fn", "Thing::method", 10, 20),
            OutlineRow::new("fn", "loose", 50, 60),
        ]
    }

    #[test]
    fn nesting_attaches_to_the_smallest_enclosing_item() {
        let mut src = OutlineEdges::new();
        src.add_file("src/a.rs", nested());
        let edges = src.all_edges();
        let parent = |name: &str| -> String {
            edges
                .iter()
                .find(|e| e.to.label() == name)
                .map(|e| e.from.label())
                .unwrap_or_else(|| panic!("no edge to {name}"))
        };
        assert_eq!(parent("outer"), "a.rs", "top-level items hang off the file");
        assert_eq!(parent("loose"), "a.rs");
        assert_eq!(parent("Thing"), "outer");
        assert_eq!(
            parent("Thing::method"),
            "Thing",
            "the impl, not the mod — flattening here loses the structure this exists for"
        );
        assert_eq!(edges.len(), 4, "one edge per item, no more");
    }

    #[test]
    fn an_item_does_not_contain_itself_and_equal_spans_do_not_nest() {
        // A mis-parse can hand back two items with identical spans. If each
        // "contained" the other the containment walk would cycle.
        let mut src = OutlineEdges::new();
        src.add_file("a.rs", vec![OutlineRow::new("impl", "T", 3, 9), OutlineRow::new("fn", "T::f", 3, 9)]);
        let edges = src.all_edges();
        assert_eq!(edges.len(), 2);
        for e in &edges {
            assert_eq!(e.from.label(), "a.rs", "both fell back to the file: {e:?}");
            assert_ne!(e.from, e.to);
        }
    }

    #[test]
    fn re_adding_a_file_replaces_its_outline() {
        let mut src = OutlineEdges::new();
        src.add_file("a.rs", vec![OutlineRow::new("fn", "old", 0, 5)]);
        src.add_file("a.rs", vec![OutlineRow::new("fn", "new", 0, 5)]);
        let labels: Vec<String> = src.all_edges().iter().map(|e| e.to.label()).collect();
        assert_eq!(labels, ["new"], "an edit must not leave the deleted item behind");
    }

    #[test]
    fn the_outline_becomes_a_navigable_containment_tree() {
        let mut g = Graph::new();
        let mut src = OutlineEdges::new();
        src.add_file("src/a.rs", nested());
        g.expand(&EntityId::file("src/a.rs"), &mut src);

        // Down: what does the impl hold?
        let inside = g.neighbors_by(&EntityId::item("src/a.rs", "impl", "Thing"), Relation::Contains);
        assert_eq!(inside.len(), 1);
        assert_eq!(inside[0].to.label(), "Thing::method");

        // Up: the inverse comes for free, and reaches the file in two hops.
        let up = g.neighbors_by(&EntityId::item("src/a.rs", "fn", "Thing::method"), Relation::ContainedBy);
        assert_eq!(up[0].to.label(), "Thing");
        let all = g.neighborhood(&EntityId::file("src/a.rs"), 3);
        assert_eq!(all.len(), 5, "the file and its four items");
    }

    #[test]
    fn search_hits_attribute_to_the_enclosing_item_and_share_one_target() {
        let mut src = SearchEdges::new("velocity");
        src.with_outline("src/a.rs", nested());
        src.add_hit("src/a.rs", 12, 4); // inside Thing::method
        src.add_hit("src/a.rs", 55, 0); // inside loose
        src.add_hit("src/b.rs", 3, 0); // no outline for b.rs
        let edges = src.all_edges();
        assert_eq!(edges.len(), 3);

        let froms: Vec<String> = edges.iter().map(|e| e.from.label()).collect();
        assert_eq!(froms, ["Thing::method", "loose", "b.rs"], "falls back to the file");
        assert!(edges.iter().all(|e| e.approx), "attribution is a guess and says so");
        assert!(edges.iter().all(|e| e.to == src.target()), "one query, one node");

        let mut g = Graph::new();
        for e in edges {
            g.insert(e);
        }
        assert_eq!(
            g.neighbors_by(&src.target(), Relation::TextMatchOf).len(),
            3,
            "and the query node lists every place it was found"
        );
    }

    #[test]
    fn a_hit_outside_every_span_falls_back_to_the_file_rather_than_guessing() {
        let mut src = SearchEdges::new("q");
        src.with_outline("a.rs", nested());
        src.add_hit("a.rs", 45, 0); // between `outer` (ends 40) and `loose` (starts 50)
        assert_eq!(src.all_edges()[0].from, EntityId::file("a.rs"));
    }

    #[test]
    fn a_commit_touching_several_lines_of_one_item_is_one_edge_at_the_last_line() {
        let mut src = GitEdges::new();
        src.with_outline("src/a.rs", nested());
        src.add_touch("src/a.rs", "abc123", vec![11, 12, 19]);
        let edges = src.all_edges();
        assert_eq!(edges.len(), 1, "one item touched, not three lines");
        assert_eq!(edges[0].from.label(), "Thing::method");
        assert_eq!(edges[0].to, EntityId::Commit { oid: "abc123".into() });
        assert_eq!(
            edges[0].site.as_ref().unwrap().line,
            19,
            "the most recent line wins, the same max-over-span rule the room heat uses"
        );
    }

    #[test]
    fn one_commit_spanning_two_items_produces_two_edges() {
        let mut src = GitEdges::new();
        src.with_outline("src/a.rs", nested());
        src.add_touch("src/a.rs", "abc", vec![12, 55]);
        let mut owners: Vec<String> = src.all_edges().iter().map(|e| e.from.label()).collect();
        owners.sort();
        assert_eq!(owners, ["Thing::method", "loose"]);
    }

    #[test]
    fn introduction_is_recorded_per_file_because_that_is_what_git_can_answer() {
        let mut src = GitEdges::new();
        src.with_outline("src/a.rs", nested());
        src.add_introduced("src/a.rs", "root1");
        let e = &src.all_edges()[0];
        assert_eq!(e.from, EntityId::file("src/a.rs"), "not an item — the data cannot support that");
        assert_eq!(e.rel, Relation::IntroducedBy);
    }

    #[test]
    fn three_sources_merge_into_one_graph_that_remembers_all_three() {
        let mut g = Graph::new();
        let file = EntityId::file("src/a.rs");
        let method = EntityId::item("src/a.rs", "fn", "Thing::method");

        let mut outline = OutlineEdges::new();
        outline.add_file("src/a.rs", nested());
        let mut search = SearchEdges::new("velocity");
        search.with_outline("src/a.rs", nested());
        search.add_hit("src/a.rs", 12, 4);
        let mut git = GitEdges::new();
        git.with_outline("src/a.rs", nested());
        git.add_touch("src/a.rs", "abc123", vec![19]);
        git.add_introduced("src/a.rs", "root1");

        g.expand(&file, &mut outline);
        g.expand(&method, &mut search);
        g.expand(&method, &mut git);
        g.expand(&file, &mut git);

        // The method is one node that three different tools found.
        let why = g.why(&method);
        assert!(why.contains(&"outline".to_string()), "{why:?}");
        assert!(why.contains(&"search".to_string()), "{why:?}");
        assert!(why.contains(&"git".to_string()), "{why:?}");

        // And from it you can reach its container, the query and the commit.
        let rels: Vec<Relation> = g.neighbors(&method).iter().map(|e| e.rel).collect();
        assert!(rels.contains(&Relation::ContainedBy));
        assert!(rels.contains(&Relation::TextMatch));
        assert!(rels.contains(&Relation::TouchedBy));
        assert!(g.edge_count() >= 6, "a floor, so an empty scan cannot pass: {}", g.edge_count());
    }

    #[test]
    fn asking_a_source_about_something_it_has_never_heard_of_is_empty_not_a_panic() {
        let stranger = EntityId::Crate { name: "serde".into(), version: None };
        let mut outline = OutlineEdges::new();
        outline.add_file("a.rs", nested());
        assert!(outline.edges_from(&stranger).is_empty());
        assert!(SearchEdges::new("q").edges_from(&stranger).is_empty());
        assert!(GitEdges::new().edges_to(&stranger).is_empty());
        // And an unknown FILE, which has a path and so takes a different route.
        assert!(outline.edges_from(&EntityId::file("never-seen.rs")).is_empty());
    }
}
