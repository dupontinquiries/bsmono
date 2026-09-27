//! entity — one graph, however many places the edges came from.
//!
//! This crate already has five ways to find code: [`crate::syntax`]'s
//! outline, [`crate::project_search`]'s grep, `workspace_symbols`' name
//! index, `route`'s scoring, and `gitspace`'s history. `aperture-lsp` adds a
//! sixth. Each one hands back its own shape of answer, and nothing relates
//! them — five greps that all land on `resolve_collision` produce five
//! unrelated hit lists, not one function that has been found five times.
//!
//! ## What this module is
//!
//! Two types and a rule.
//!
//! * [`EntityId`] names a PLACE — a file, an item in a file, a crate, a
//!   commit — canonically, so two sources that found the same place produce
//!   the same id and the graph merges them instead of double-counting.
//! * [`Edge`] is a RELATIONSHIP, and it carries the [`Site`] it was observed
//!   at. `A calls B` is not the useful fact; `A calls B at line 217` is,
//!   because that is what a person needs to walk back along.
//! * The rule: a source may only ADD. [`Graph::insert`] merges a duplicate
//!   rather than replacing it, so an approximate edge and the exact one that
//!   confirms it become one edge that knows both sources said so.
//!
//! ## What it deliberately is not
//!
//! Not an index, not a cache, not a background job. Nothing here walks the
//! workspace on its own or holds a thread. A caller expands the bits of the
//! graph it is about to show, and the graph remembers what it was told —
//! which is the same bargain [`crate::gitspace`] makes with `git`, for the
//! same reason: the moment this owns a scan, every consumer inherits a
//! concurrency story it did not ask for.
//!
//! ## Ids are derived from names, and that has a cost worth stating
//!
//! An [`EntityId::Item`] is `(file, kind, name)`. Rename the function and it
//! is a different id — every bucket entry and saved path pointing at the old
//! one is orphaned. That is deliberate and it is the lesser evil: the
//! alternative is a generated id plus a mapping table, which just moves the
//! problem to "who updates the table, and what happens when they disagree".
//! An orphaned reference is VISIBLE and a person can fix it; a stale mapping
//! is invisible and lies. See `bucket::Located::Unresolved`, which is the
//! same decision made one layer up.

use std::collections::{BTreeMap, BTreeSet};

/// Where something was observed, in a file. Lines and columns are 0-based —
/// the outline's convention, not the display's.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Site {
    pub path: String,
    pub line: u32,
    pub col: u32,
}

impl Site {
    pub fn new(path: impl Into<String>, line: u32, col: u32) -> Site {
        Site { path: path.into(), line, col }
    }
}

/// A place in (or around) the codebase.
///
/// `Ord` and `Hash` are derived and load-bearing: the graph's whole job is
/// deciding that two sources found the same thing, and that decision is this
/// type's equality.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntityId {
    /// A file, by workspace-relative path.
    File { path: String },
    /// An item inside a file: a function, struct, impl, module.
    ///
    /// `kind` is part of the identity, not decoration — a `struct Thing` and
    /// its `impl Thing` are two entities that a call graph relates
    /// differently, and collapsing them loses the distinction that makes
    /// "who implements this" answerable.
    Item { path: String, kind: String, name: String },
    /// A dependency crate. Version is optional because a `goto_definition`
    /// into a registry path does not always carry one.
    Crate { name: String, version: Option<String> },
    /// A commit or checkpoint, by oid — the same strings `gitspace` deals in.
    Commit { oid: String },
    /// A symbol outside the workspace: std, or a dependency's internals.
    /// `origin` is "std" or the crate name; `path` is the qualified symbol.
    Extern { origin: String, path: String },
    /// A frame observed in a running program.
    ///
    /// Nothing produces these yet — there is no run capture anywhere in the
    /// tree. It is here, with [`Relation::ObservedIn`], so that adding one
    /// later is a new [`EdgeSource`] and not a change to this enum, which
    /// every persisted id would have to survive.
    Frame { run: String, symbol: String },
}

impl EntityId {
    pub fn file(path: impl Into<String>) -> EntityId {
        EntityId::File { path: path.into() }
    }
    pub fn item(path: impl Into<String>, kind: impl Into<String>, name: impl Into<String>) -> EntityId {
        EntityId::Item { path: path.into(), kind: kind.into(), name: name.into() }
    }

    /// The file this lives in, when it lives in one. What a "jump there"
    /// needs, and what tells a caller whether an entity is even openable.
    pub fn path(&self) -> Option<&str> {
        match self {
            EntityId::File { path } => Some(path),
            EntityId::Item { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Short label for a status bar or a path chip.
    pub fn label(&self) -> String {
        match self {
            EntityId::File { path } => path.rsplit('/').next().unwrap_or(path).to_string(),
            EntityId::Item { name, .. } => name.clone(),
            EntityId::Crate { name, version: Some(v) } => format!("{name} {v}"),
            EntityId::Crate { name, version: None } => name.clone(),
            EntityId::Commit { oid } => oid.chars().take(8).collect(),
            EntityId::Extern { path, .. } => {
                path.rsplit("::").next().unwrap_or(path).to_string()
            }
            EntityId::Frame { symbol, .. } => symbol.clone(),
        }
    }

    /// A stable, single-line, greppable form — for a bucket entry, a saved
    /// path, a log line.
    ///
    /// Round-trips through [`EntityId::parse_key`] for every id whose fields
    /// contain no `\u{1f}`, which is the same separator and the same bargain
    /// `gitspace` makes when parsing git's output. A path containing a unit
    /// separator is not a thing that happens; a path containing `#`, `@` or
    /// `:` very much is, which is why those are not the separator.
    pub fn key(&self) -> String {
        let us = '\u{1f}';
        match self {
            EntityId::File { path } => format!("file{us}{path}"),
            EntityId::Item { path, kind, name } => format!("item{us}{path}{us}{kind}{us}{name}"),
            EntityId::Crate { name, version } => {
                format!("crate{us}{name}{us}{}", version.as_deref().unwrap_or(""))
            }
            EntityId::Commit { oid } => format!("commit{us}{oid}"),
            EntityId::Extern { origin, path } => format!("extern{us}{origin}{us}{path}"),
            EntityId::Frame { run, symbol } => format!("frame{us}{run}{us}{symbol}"),
        }
    }

    /// Total: unknown tags and short field lists give `None` rather than a
    /// half-built id. Same reasoning as `bucket::parse` — this text comes
    /// back from storage a different version of the program wrote.
    pub fn parse_key(s: &str) -> Option<EntityId> {
        let mut f = s.split('\u{1f}');
        let tag = f.next()?;
        let mut next = || f.next().filter(|x| !x.is_empty()).map(str::to_string);
        match tag {
            "file" => Some(EntityId::File { path: next()? }),
            "item" => Some(EntityId::Item { path: next()?, kind: next()?, name: next()? }),
            "crate" => Some(EntityId::Crate { name: next()?, version: next() }),
            "commit" => Some(EntityId::Commit { oid: next()? }),
            "extern" => Some(EntityId::Extern { origin: next()?, path: next()? }),
            "frame" => Some(EntityId::Frame { run: next()?, symbol: next()? }),
            _ => None,
        }
    }
}

/// How two entities relate.
///
/// Every variant has an [`inverse`](Relation::inverse), and the graph stores
/// each edge ONCE and answers the other direction by inverting. Storing both
/// halves would make "is this edge already here" a question with two answers,
/// and the first time the two disagreed nobody would know which was right.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Relation {
    /// Lexical containment: file contains item, item contains item.
    Contains,
    ContainedBy,
    Calls,
    CalledBy,
    /// A mention that is not a call — a type used, a name referenced.
    References,
    ReferencedBy,
    /// A commit touched this entity.
    TouchedBy,
    Touched,
    /// The commit that first introduced it.
    IntroducedBy,
    Introduced,
    /// A raw text hit. The weakest relation and the most honest one: it says
    /// the string appeared, not that it meant anything.
    TextMatch,
    TextMatchOf,
    /// A test exercises this.
    TestedBy,
    Tests,
    /// Seen in a running program. No source produces this yet; see
    /// [`EntityId::Frame`].
    ObservedIn,
    Observed,
    /// No relation in the code at all: you opened it from there (a file
    /// picker, a symbol search). Navigation records it so the way back
    /// exists; no [`EdgeSource`] ever produces it, because it is a fact
    /// about attention, not about the program.
    Opened,
    OpenedFrom,
}

impl Relation {
    pub fn inverse(self) -> Relation {
        use Relation::*;
        match self {
            Contains => ContainedBy,
            ContainedBy => Contains,
            Calls => CalledBy,
            CalledBy => Calls,
            References => ReferencedBy,
            ReferencedBy => References,
            TouchedBy => Touched,
            Touched => TouchedBy,
            IntroducedBy => Introduced,
            Introduced => IntroducedBy,
            TextMatch => TextMatchOf,
            TextMatchOf => TextMatch,
            TestedBy => Tests,
            Tests => TestedBy,
            ObservedIn => Observed,
            Observed => ObservedIn,
            Opened => OpenedFrom,
            OpenedFrom => Opened,
        }
    }

    /// How it reads in a navigation path: "you got to B from A **via a
    /// call**". Present tense, from the perspective of the thing you left.
    pub fn label(self) -> &'static str {
        use Relation::*;
        match self {
            Contains => "contains",
            ContainedBy => "in",
            Calls => "calls",
            CalledBy => "called by",
            References => "references",
            ReferencedBy => "referenced by",
            TouchedBy => "touched by",
            Touched => "touched",
            IntroducedBy => "introduced by",
            Introduced => "introduced",
            TextMatch => "matches",
            TextMatchOf => "matched by",
            TestedBy => "tested by",
            Tests => "tests",
            ObservedIn => "observed in",
            Observed => "observed",
            Opened => "then",
            OpenedFrom => "opened from",
        }
    }
}

/// A relationship, and where it was seen.
///
/// The `site` is what makes an edge navigable rather than merely true. "A
/// calls B" cannot be walked back along; "A calls B at src/a.rs:217:8" can.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: EntityId,
    pub rel: Relation,
    pub to: EntityId,
    pub site: Option<Site>,
    /// Which [`EdgeSource`] said so. More than one may, and all of them are
    /// kept — that is [`Graph`]'s merge rule.
    pub sources: BTreeSet<String>,
    /// True while only heuristic sources have claimed this edge. A
    /// tree-sitter identifier match is approximate; an LSP `references`
    /// answer is not, and confirming an approximate edge clears the flag.
    ///
    /// Shown distinctly rather than hidden. A guess presented as a fact is
    /// the failure mode worth designing against here — see the note in root
    /// TODO.md about `didChange` and the two tiers disagreeing.
    pub approx: bool,
}

impl Edge {
    pub fn new(from: EntityId, rel: Relation, to: EntityId, source: &str) -> Edge {
        Edge {
            from,
            rel,
            to,
            site: None,
            sources: [source.to_string()].into_iter().collect(),
            approx: false,
        }
    }
    pub fn at(mut self, site: Site) -> Edge {
        self.site = Some(site);
        self
    }
    pub fn approx(mut self) -> Edge {
        self.approx = true;
        self
    }
    /// Identity for merging: the same relationship observed at the same
    /// place. Two calls to B from DIFFERENT lines of A are two edges, and
    /// that is the point — "which callsite" is the question the path answers.
    fn key(&self) -> (EntityId, Relation, EntityId, Option<Site>) {
        (self.from.clone(), self.rel, self.to.clone(), self.site.clone())
    }
}

/// Something that can answer "what relates to this".
///
/// Blocking, and `&mut self` so an implementation may cache. Both are the
/// same call the LSP client already made and for the reason its doc gives:
/// do not build the concurrency story before a real consumer has shown which
/// calls are actually slow.
pub trait EdgeSource {
    /// Stable name, recorded as an edge's provenance. Keep it short — it
    /// shows up in a UI.
    fn name(&self) -> &str;
    /// Relationships leading AWAY from `id`.
    fn edges_from(&mut self, id: &EntityId) -> Vec<Edge>;
    /// Relationships leading TO it. Defaulted empty: several sources are
    /// naturally one-directional (an outline knows what a file contains, and
    /// nothing asks it the reverse), and the graph inverts what it has.
    fn edges_to(&mut self, id: &EntityId) -> Vec<Edge> {
        let _ = id;
        Vec::new()
    }
}

/// One entity per place, however many sources found it.
#[derive(Clone, Debug, Default)]
pub struct Graph {
    edges: Vec<Edge>,
    /// `edges` indices, by endpoint. Both directions, so a query never scans.
    out: BTreeMap<EntityId, Vec<usize>>,
    inn: BTreeMap<EntityId, Vec<usize>>,
    /// Which sources have mentioned each entity, in insertion order of first
    /// mention. This is the "remembers why it is on screen" property: five
    /// greps that find the same function leave five marks on one node.
    why: BTreeMap<EntityId, BTreeSet<String>>,
    /// Entities expanded already, so a caller can avoid asking twice.
    expanded: BTreeSet<EntityId>,
}

/// What one step of a neighbourhood walk found: the entity, and how far out
/// it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reached {
    pub id: EntityId,
    pub depth: u32,
}

impl Graph {
    pub fn new() -> Graph {
        Graph::default()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn node_count(&self) -> usize {
        self.why.len()
    }

    /// Add an edge, or merge it into the one already there.
    ///
    /// Merging is a union of sources and an AND of `approx`: the exact answer
    /// wins over the guess, which is the whole reason the cheap tree-sitter
    /// tier is allowed to speak first. Returns true if this was new.
    pub fn insert(&mut self, e: Edge) -> bool {
        for (from, src) in [(&e.from, &e.sources), (&e.to, &e.sources)] {
            self.why.entry(from.clone()).or_default().extend(src.iter().cloned());
        }
        if let Some(i) = self.edges.iter().position(|x| x.key() == e.key()) {
            let cur = &mut self.edges[i];
            cur.sources.extend(e.sources);
            cur.approx = cur.approx && e.approx;
            return false;
        }
        let i = self.edges.len();
        self.out.entry(e.from.clone()).or_default().push(i);
        self.inn.entry(e.to.clone()).or_default().push(i);
        self.edges.push(e);
        true
    }

    /// Every relationship touching `id`, INCLUDING incoming ones presented in
    /// the direction you would travel: an edge `A calls B` shows up on B as
    /// `B called-by A`. One stored edge, two navigable directions.
    pub fn neighbors(&self, id: &EntityId) -> Vec<Edge> {
        let mut out: Vec<Edge> = Vec::new();
        for i in self.out.get(id).into_iter().flatten() {
            out.push(self.edges[*i].clone());
        }
        for i in self.inn.get(id).into_iter().flatten() {
            let e = &self.edges[*i];
            out.push(Edge {
                from: e.to.clone(),
                rel: e.rel.inverse(),
                to: e.from.clone(),
                site: e.site.clone(),
                sources: e.sources.clone(),
                approx: e.approx,
            });
        }
        out
    }

    /// Only the neighbours reached by one relation — "who calls this".
    pub fn neighbors_by(&self, id: &EntityId, rel: Relation) -> Vec<Edge> {
        self.neighbors(id).into_iter().filter(|e| e.rel == rel).collect()
    }

    /// Which sources have mentioned this entity at all.
    pub fn why(&self, id: &EntityId) -> Vec<String> {
        self.why.get(id).map(|s| s.iter().cloned().collect()).unwrap_or_default()
    }

    pub fn contains(&self, id: &EntityId) -> bool {
        self.why.contains_key(id)
    }

    /// Ask a source about `id` and fold what it says in. Returns how many
    /// edges were NEW — zero is a normal and useful answer ("nothing here you
    /// did not already know"), not a failure.
    pub fn expand(&mut self, id: &EntityId, src: &mut dyn EdgeSource) -> usize {
        let mut fresh = 0;
        for e in src.edges_from(id).into_iter().chain(src.edges_to(id)) {
            if self.insert(e) {
                fresh += 1;
            }
        }
        self.expanded.insert(id.clone());
        fresh
    }

    pub fn is_expanded(&self, id: &EntityId) -> bool {
        self.expanded.contains(id)
    }

    /// Everything within `max_depth` hops of `root`, breadth-first, each with
    /// the depth it was first reached at. The root is depth 0 and always
    /// present, even in an empty graph — "show me this and its
    /// neighbourhood" must not come back with nothing when the neighbourhood
    /// is empty.
    ///
    /// This is `bucket::Bucket::depth`'s horizon made concrete: a bucket says
    /// how far attention reaches, and this is what reaches that far.
    pub fn neighborhood(&self, root: &EntityId, max_depth: u32) -> Vec<Reached> {
        self.neighborhood_via(root, max_depth, &|_| true)
    }

    /// The same walk, restricted to relations the filter accepts — "the call
    /// graph two hops out, ignoring text matches".
    pub fn neighborhood_via(
        &self,
        root: &EntityId,
        max_depth: u32,
        allow: &dyn Fn(Relation) -> bool,
    ) -> Vec<Reached> {
        let mut seen: BTreeSet<EntityId> = [root.clone()].into_iter().collect();
        let mut out = vec![Reached { id: root.clone(), depth: 0 }];
        let mut frontier = vec![root.clone()];
        for depth in 1..=max_depth {
            let mut next = Vec::new();
            for id in &frontier {
                for e in self.neighbors(id) {
                    if !allow(e.rel) || !seen.insert(e.to.clone()) {
                        continue;
                    }
                    out.push(Reached { id: e.to.clone(), depth });
                    next.push(e.to);
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> EntityId {
        EntityId::item("src/a.rs", "fn", name)
    }

    #[test]
    fn ids_round_trip_through_their_key_form() {
        let ids = [
            EntityId::file("src/physics/collision.rs"),
            EntityId::item("src/a.rs", "fn", "resolve_collision"),
            EntityId::item("src/a.rs", "impl", "Thing"),
            EntityId::Crate { name: "serde".into(), version: Some("1.0.2".into()) },
            EntityId::Crate { name: "ropey".into(), version: None },
            EntityId::Commit { oid: "5db8a71e".into() },
            EntityId::Extern { origin: "std".into(), path: "core::mem::swap".into() },
            EntityId::Frame { run: "r1".into(), symbol: "main".into() },
        ];
        for id in ids {
            assert_eq!(EntityId::parse_key(&id.key()).as_ref(), Some(&id), "{}", id.key());
        }
        // Paths with the characters a naive separator would have eaten.
        let awkward = EntityId::item("a:b@c#d.rs", "fn", "x");
        assert_eq!(EntityId::parse_key(&awkward.key()), Some(awkward));
    }

    #[test]
    fn parsing_a_key_is_total() {
        for bad in ["", "file", "nonsense\u{1f}x", "item\u{1f}only-a-path", "\u{1f}\u{1f}"] {
            assert_eq!(EntityId::parse_key(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn kind_is_part_of_identity_so_a_struct_and_its_impl_are_two_things() {
        let s = EntityId::item("a.rs", "struct", "Thing");
        let i = EntityId::item("a.rs", "impl", "Thing");
        assert_ne!(s, i);
        let mut g = Graph::new();
        g.insert(Edge::new(EntityId::file("a.rs"), Relation::Contains, s, "outline"));
        g.insert(Edge::new(EntityId::file("a.rs"), Relation::Contains, i, "outline"));
        assert_eq!(g.node_count(), 3, "the file and both items");
    }

    #[test]
    fn every_relation_inverts_and_inverts_back() {
        use Relation::*;
        let all = [
            Contains, ContainedBy, Calls, CalledBy, References, ReferencedBy, TouchedBy, Touched,
            IntroducedBy, Introduced, TextMatch, TextMatchOf, TestedBy, Tests, ObservedIn, Observed,
        ];
        // A floor, so adding a variant and forgetting it here cannot pass.
        assert!(all.len() >= 16);
        for r in all {
            assert_eq!(r.inverse().inverse(), r, "{r:?}");
            assert_ne!(r.inverse(), r, "{r:?}: no relation is its own inverse");
            assert!(!r.label().is_empty());
        }
    }

    #[test]
    fn one_stored_edge_is_navigable_from_both_ends() {
        let mut g = Graph::new();
        g.insert(
            Edge::new(item("a"), Relation::Calls, item("b"), "lsp")
                .at(Site::new("src/a.rs", 216, 8)),
        );
        assert_eq!(g.edge_count(), 1, "stored once");

        let fwd = g.neighbors(&item("a"));
        assert_eq!(fwd.len(), 1);
        assert_eq!(fwd[0].rel, Relation::Calls);
        assert_eq!(fwd[0].to, item("b"));

        let back = g.neighbors(&item("b"));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].rel, Relation::CalledBy, "presented in the direction you travel");
        assert_eq!(back[0].to, item("a"));
        assert_eq!(
            back[0].site,
            Some(Site::new("src/a.rs", 216, 8)),
            "the callsite rides along both ways — it is what you walk back to"
        );
    }

    #[test]
    fn the_same_place_found_five_times_is_one_node_that_remembers_all_five() {
        let mut g = Graph::new();
        let target = item("resolve_collision");
        for src in ["grep", "outline", "symbols", "route", "lsp"] {
            g.insert(Edge::new(EntityId::file("src/a.rs"), Relation::Contains, target.clone(), src));
        }
        assert_eq!(g.node_count(), 2, "one file, one item — not five items");
        assert_eq!(g.edge_count(), 1, "one relationship");
        assert_eq!(g.why(&target).len(), 5, "and it remembers who found it");
        assert!(g.why(&target).contains(&"route".to_string()));
    }

    #[test]
    fn an_exact_answer_clears_an_approximate_one_but_not_the_other_way() {
        let mut g = Graph::new();
        let site = Site::new("src/a.rs", 10, 0);
        let guess = Edge::new(item("a"), Relation::Calls, item("b"), "tree-sitter")
            .at(site.clone())
            .approx();
        assert!(g.insert(guess));
        assert!(g.neighbors(&item("a"))[0].approx, "a guess starts out marked");

        let exact = Edge::new(item("a"), Relation::Calls, item("b"), "lsp").at(site.clone());
        assert!(!g.insert(exact), "same relationship at the same site: merged, not added");
        let e = &g.neighbors(&item("a"))[0];
        assert!(!e.approx, "confirmation clears the flag");
        assert_eq!(e.sources.len(), 2, "and both sources are kept");

        // Another guess arriving afterwards must not un-confirm it.
        g.insert(
            Edge::new(item("a"), Relation::Calls, item("b"), "grep").at(site).approx(),
        );
        assert!(!g.neighbors(&item("a"))[0].approx, "a later guess cannot undo a confirmation");
    }

    #[test]
    fn two_callsites_in_one_function_are_two_edges() {
        // The distinction the whole `site` field exists for: "A calls B" is
        // not navigable, "A calls B at line 12 and again at line 40" is.
        let mut g = Graph::new();
        for line in [12, 40] {
            g.insert(
                Edge::new(item("a"), Relation::Calls, item("b"), "lsp")
                    .at(Site::new("src/a.rs", line, 4)),
            );
        }
        assert_eq!(g.edge_count(), 2);
        let lines: Vec<u32> = g.neighbors(&item("b")).iter().filter_map(|e| e.site.as_ref().map(|s| s.line)).collect();
        assert_eq!(lines, [12, 40]);
    }

    struct Canned {
        name: String,
        from: BTreeMap<EntityId, Vec<Edge>>,
        asked: usize,
    }
    impl EdgeSource for Canned {
        fn name(&self) -> &str {
            &self.name
        }
        fn edges_from(&mut self, id: &EntityId) -> Vec<Edge> {
            self.asked += 1;
            self.from.get(id).cloned().unwrap_or_default()
        }
    }

    fn chain_source() -> Canned {
        // a → b → c → d, plus a text match hanging off b.
        let mut from = BTreeMap::new();
        for (x, y) in [("a", "b"), ("b", "c"), ("c", "d")] {
            from.insert(
                item(x),
                vec![Edge::new(item(x), Relation::Calls, item(y), "canned")],
            );
        }
        from.get_mut(&item("b")).unwrap().push(Edge::new(
            item("b"),
            Relation::TextMatch,
            EntityId::file("notes.md"),
            "canned",
        ));
        Canned { name: "canned".into(), from, asked: 0 }
    }

    #[test]
    fn expanding_reports_only_what_was_new() {
        let mut g = Graph::new();
        let mut s = chain_source();
        assert_eq!(g.expand(&item("a"), &mut s), 1);
        assert_eq!(g.expand(&item("a"), &mut s), 0, "asking twice adds nothing");
        assert_eq!(s.asked, 2, "…but it really did ask again — caching is the caller's job");
        assert!(g.is_expanded(&item("a")));
        assert!(!g.is_expanded(&item("b")));
    }

    #[test]
    fn the_depth_horizon_bounds_the_walk_and_the_root_is_always_in_it() {
        let mut g = Graph::new();
        let mut s = chain_source();
        for x in ["a", "b", "c"] {
            g.expand(&item(x), &mut s);
        }

        let at = |d: u32| -> Vec<String> {
            g.neighborhood(&item("a"), d).iter().map(|r| r.id.label()).collect()
        };
        assert_eq!(at(0), ["a"], "depth 0 is the root alone, never empty");
        assert_eq!(at(1), ["a", "b"]);
        let two = at(2);
        assert!(two.contains(&"c".to_string()) && two.contains(&"notes.md".to_string()));
        assert_eq!(at(3).len(), 5, "a b c d notes.md");
        assert_eq!(at(99).len(), 5, "and it stops when it runs out, not when it hits the limit");

        // Monotone in depth — the property the status-bar count depends on.
        let mut last = 0;
        for d in 0..=5 {
            let n = g.neighborhood(&item("a"), d).len();
            assert!(n >= last, "depth {d} saw fewer than depth {}", d - 1);
            last = n;
        }
    }

    #[test]
    fn a_relation_filter_keeps_a_text_match_from_dragging_in_the_world() {
        let mut g = Graph::new();
        let mut s = chain_source();
        for x in ["a", "b", "c"] {
            g.expand(&item(x), &mut s);
        }
        let calls_only =
            g.neighborhood_via(&item("a"), 3, &|r| matches!(r, Relation::Calls | Relation::CalledBy));
        let labels: Vec<String> = calls_only.iter().map(|r| r.id.label()).collect();
        assert_eq!(labels, ["a", "b", "c", "d"]);
        assert!(!labels.contains(&"notes.md".to_string()), "the grep hit stayed out");
    }

    #[test]
    fn depth_is_the_shortest_hop_count_not_the_first_path_tried() {
        // a→b→c and a→c directly. `c` is depth 1, whichever order the edges
        // were inserted in — a longest-path answer would make the horizon
        // meaningless.
        let mut g = Graph::new();
        g.insert(Edge::new(item("a"), Relation::Calls, item("b"), "s"));
        g.insert(Edge::new(item("b"), Relation::Calls, item("c"), "s"));
        g.insert(Edge::new(item("a"), Relation::Calls, item("c"), "s"));
        let n = g.neighborhood(&item("a"), 3);
        let c = n.iter().find(|r| r.id == item("c")).expect("reached");
        assert_eq!(c.depth, 1);
    }

    #[test]
    fn a_cycle_terminates() {
        let mut g = Graph::new();
        g.insert(Edge::new(item("a"), Relation::Calls, item("b"), "s"));
        g.insert(Edge::new(item("b"), Relation::Calls, item("a"), "s"));
        // Depth 12, not 50, ON PURPOSE. Without the visited set this walk
        // does not merely revisit — it doubles at every hop (`b` sees `a`
        // both as `Calls` and as the inverse of `a Calls b`), so a limit of
        // 50 does not fail this test, it gets the process OOM-killed.
        // Twelve makes the same break come back as `4096 != 2`, which is a
        // test result rather than a dead runner. Verified by removing the
        // guard and watching both.
        assert_eq!(g.neighborhood(&item("a"), 12).len(), 2);
    }

    #[test]
    fn a_deep_limit_on_an_acyclic_graph_costs_nothing() {
        let mut g = Graph::new();
        let mut s = chain_source();
        for x in ["a", "b", "c"] {
            g.expand(&item(x), &mut s);
        }
        assert_eq!(g.neighborhood(&item("a"), 10_000).len(), 5, "stops when it runs out");
    }
}
