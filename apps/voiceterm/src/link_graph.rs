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
}
