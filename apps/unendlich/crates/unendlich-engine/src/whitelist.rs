//! whitelist.rs — pure ID allow-list.
//!
//! A deterministic set of string IDs (player names / mod IDs) with add / remove
//! / contains, plus parsing a newline-or-comma list and serializing back. Backed
//! by a `BTreeSet` so iteration/serialization order is deterministic (R11). Pure
//! and panic-free (R1/R21).

use std::collections::BTreeSet;

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Whitelist {
    ids: BTreeSet<String>,
}

/// Normalize a raw token into a canonical ID, or `None` if it is empty after
/// trimming. (Surrounding whitespace is dropped; case is preserved, since IDs
/// may be case-sensitive.)
fn normalize(token: &str) -> Option<String> {
    let t = token.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

impl Whitelist {
    pub fn new() -> Self {
        Whitelist { ids: BTreeSet::new() }
    }

    /// Add an ID (after normalization). Returns true if it was newly inserted.
    /// Adding the same ID again is a no-op (idempotent).
    pub fn add(&mut self, id: &str) -> bool {
        match normalize(id) {
            Some(n) => self.ids.insert(n),
            None => false,
        }
    }

    /// Remove an ID. Returns true if it was present.
    pub fn remove(&mut self, id: &str) -> bool {
        match normalize(id) {
            Some(n) => self.ids.remove(&n),
            None => false,
        }
    }

    pub fn contains(&self, id: &str) -> bool {
        match normalize(id) {
            Some(n) => self.ids.contains(&n),
            None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// All IDs in deterministic (sorted) order.
    pub fn entries(&self) -> Vec<String> {
        self.ids.iter().cloned().collect()
    }

    /// Parse a list separated by newlines and/or commas, normalizing and
    /// deduping. Blank tokens are skipped. `#` begins a line comment.
    pub fn parse(text: &str) -> Self {
        let mut wl = Whitelist::new();
        for raw_line in text.lines() {
            let line = match raw_line.split('#').next() {
                Some(l) => l,
                None => raw_line,
            };
            for token in line.split(',') {
                wl.add(token);
            }
        }
        wl
    }

    /// Serialize to a newline-separated list in sorted order. `parse` of this
    /// output reproduces the same set.
    pub fn serialize(&self) -> String {
        self.entries().join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_semantics() {
        let mut wl = Whitelist::new();
        assert!(!wl.contains("alice"));
        assert!(wl.add("alice"));
        assert!(wl.contains("alice"));
        assert!(wl.remove("alice"));
        assert!(!wl.contains("alice"));
        assert!(!wl.remove("alice"), "removing absent id returns false");
    }

    #[test]
    fn add_is_idempotent() {
        let mut wl = Whitelist::new();
        assert!(wl.add("bob"));
        assert!(!wl.add("bob"), "second add is a no-op");
        assert!(!wl.add("  bob  "), "whitespace-variant normalizes to same id");
        assert_eq!(wl.len(), 1);
    }

    #[test]
    fn parse_handles_commas_newlines_blanks_comments() {
        let wl = Whitelist::parse("alice, bob\ncarol\n\n  \n# a comment\ndave # inline\n,,eve,");
        let e = wl.entries();
        assert_eq!(e, vec!["alice", "bob", "carol", "dave", "eve"]);
    }

    #[test]
    fn prop_parse_serialize_round_trip() {
        let inputs = [
            "alice\nbob\ncarol",
            "zeta, alpha, mike",
            "x,x,x,y", // dupes collapse
            "",
            "  spaced  ,  names  ",
        ];
        for s in inputs {
            let a = Whitelist::parse(s);
            let b = Whitelist::parse(&a.serialize());
            assert_eq!(a, b, "round-trip changed the set for {:?}", s);
            // serialize is sorted + deterministic
            assert_eq!(a.serialize(), a.serialize());
        }
    }

    #[test]
    fn entries_are_sorted_and_deduped() {
        let wl = Whitelist::parse("mike,alpha,zeta,alpha");
        assert_eq!(wl.entries(), vec!["alpha", "mike", "zeta"]);
    }

    #[test]
    fn prop_deterministic() {
        let wl = Whitelist::parse("g,a,f,b,a,c");
        assert_eq!(wl.serialize(), wl.serialize());
        assert_eq!(Whitelist::parse("g,a,f,b,a,c"), Whitelist::parse("g,a,f,b,a,c"));
    }
}
