// src/user_tags.rs
//
// v0.1.324 (Mermaid+/cards arc 2.5.3): user-assigned tags.
//
// Each Node in the connections graph carries auto-derived tags
// (domain, speaker, kind, etc) — those come from the data itself.
// This module owns the separate axis of USER-assigned tags: the
// labels the user attaches by hand or by voice.
//
// Storage:
//   ~/.config/web2qml/user_tags.toml
//
// Format: per-id sections with a `tags` array. Hand-rolled (not
// toml-rs) to stay zero-dep:
//
//     ["page_a1b2c3"]
//     tags = ["important", "reading-list"]
//
//     ["voice_4d5e6f"]
//     tags = ["q4-planning"]
//
// Why per-id rather than per-tag: most reads are "what user tags
// does this node have?" (rendering the preview, mergeing into
// fuzzy_filter haystack). A few reads are "what nodes carry this
// tag?" (the future "filter to @work" path); those can scan the
// whole map linearly — it's <100KB at the user's expected scale.
//
// Concurrency: load-on-first-access, in-memory mutex, write-
// through on every mutation. Same trade-off as card_pins —
// drag rate is low.

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

fn state() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| {
        let mut s = Store::default();
        s.load();
        Mutex::new(s)
    })
}

#[derive(Default)]
struct Store {
    /// id → set of tags. Vec ordered by insertion (consistent
    /// display order).
    map: HashMap<String, Vec<String>>,
}

impl Store {
    fn load(&mut self) {
        let path = match user_tags_path() {
            Some(p) => p,
            None => return,
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => return,
        };
        // Tiny parser: looks for `["<id>"]` section headers,
        // then `tags = ["a", "b", "c"]` arrays.
        let mut current_id: Option<String> = None;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            // Section header: starts with [", ends with "].
            if line.starts_with("[\"") && line.ends_with("\"]") {
                let inner = &line[2..line.len() - 2];
                current_id = Some(inner.to_string());
                continue;
            }
            // tags = [ "a", "b" ]
            if let Some(rest) = line.strip_prefix("tags") {
                let rest = rest.trim_start().trim_start_matches('=').trim();
                let rest = rest.trim_start_matches('[').trim_end_matches(']');
                let tags: Vec<String> = rest.split(',')
                    .map(|s| s.trim().trim_matches('"').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if let Some(id) = &current_id {
                    if !tags.is_empty() {
                        self.map.insert(id.clone(), tags);
                    }
                }
            }
        }
        crate::clog!("user_tags", "loaded {} entries from {:?}", self.map.len(), path);
    }

    fn save(&self) {
        let path = match user_tags_path() {
            Some(p) => p,
            None => {
                crate::clog!("user_tags", "save: no HOME, skipping");
                return;
            }
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut content = String::with_capacity(self.map.len() * 64);
        content.push_str("# web2qml user-assigned tags. Hand-edit at your own risk —\n");
        content.push_str("# the cards canvas writes this when you tag/untag.\n\n");
        let mut keys: Vec<&String> = self.map.keys().collect();
        keys.sort();
        for k in keys {
            if k.contains('"') || k.contains('\n') { continue; }
            let tags = &self.map[k];
            if tags.is_empty() { continue; }
            content.push_str(&format!("[\"{}\"]\n", k));
            content.push_str("tags = [");
            for (i, t) in tags.iter().enumerate() {
                if i > 0 { content.push_str(", "); }
                // Defensive sanitization on tag content too.
                let sanitized: String = t.chars()
                    .filter(|c| *c != '"' && *c != '\n' && *c != '\r')
                    .collect();
                content.push_str(&format!("\"{}\"", sanitized));
            }
            content.push_str("]\n\n");
        }
        if let Err(e) = std::fs::write(&path, content) {
            crate::clog!("user_tags", "save failed: {}", e);
        }
    }
}

fn user_tags_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config/web2qml/user_tags.toml"))
}

/// Normalize a tag for storage: trim, lowercase, strip spaces.
/// "Reading List" → "reading-list". Keeps tags consistent so
/// "Reading List" and "reading list" don't pile up as
/// duplicates.
fn normalize_tag(tag: &str) -> String {
    let mut out = String::with_capacity(tag.len());
    let mut prev_dash = false;
    for c in tag.trim().chars() {
        if c.is_whitespace() {
            if !prev_dash && !out.is_empty() {
                out.push('-');
                prev_dash = true;
            }
        } else if c.is_alphanumeric() || c == '-' || c == '_' || c == '@' || c == '/' {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        }
        // Other punctuation: dropped silently.
    }
    // Strip trailing dash that could happen with "foo "
    while out.ends_with('-') { out.pop(); }
    out
}

/// Add a tag to a node. Returns true if newly added (false if
/// already present). Empty/invalid tags rejected. Persists.
pub fn add(id: &str, tag: &str) -> bool {
    if id.is_empty() { return false; }
    let norm = normalize_tag(tag);
    if norm.is_empty() { return false; }
    if let Ok(mut g) = state().lock() {
        let entry = g.map.entry(id.to_string()).or_default();
        if entry.iter().any(|t| t == &norm) {
            return false;
        }
        entry.push(norm);
        g.save();
        return true;
    }
    false
}

/// Remove a tag from a node. Returns true if removed.
pub fn remove(id: &str, tag: &str) -> bool {
    if id.is_empty() { return false; }
    let norm = normalize_tag(tag);
    if norm.is_empty() { return false; }
    if let Ok(mut g) = state().lock() {
        let entry = match g.map.get_mut(id) {
            Some(e) => e,
            None => return false,
        };
        let before = entry.len();
        entry.retain(|t| t != &norm);
        let removed = entry.len() < before;
        // Clean up empty entries so the file stays small.
        if entry.is_empty() {
            g.map.remove(id);
        }
        if removed { g.save(); }
        return removed;
    }
    false
}

/// Get all user tags for a node. Cloned so callers don't hold
/// the lock.
pub fn get(id: &str) -> Vec<String> {
    state().lock().ok()
        .and_then(|g| g.map.get(id).cloned())
        .unwrap_or_default()
}

/// Get all IDs that carry the given tag (normalized
/// internally). For "filter to <tag>" voice path.
pub fn nodes_with_tag(tag: &str) -> Vec<String> {
    let norm = normalize_tag(tag);
    if norm.is_empty() { return Vec::new(); }
    let g = match state().lock() { Ok(g) => g, Err(_) => return Vec::new() };
    g.map.iter()
        .filter(|(_, tags)| tags.iter().any(|t| t == &norm))
        .map(|(id, _)| id.clone())
        .collect()
}

/// Clear all tags for a node. Returns count cleared.
pub fn clear_all(id: &str) -> usize {
    if id.is_empty() { return 0; }
    if let Ok(mut g) = state().lock() {
        let n = g.map.remove(id).map(|v| v.len()).unwrap_or(0);
        if n > 0 { g.save(); }
        return n;
    }
    0
}

/// Total count of (id, tag) pairs in the store. For diagnostics.
pub fn pair_count() -> usize {
    state().lock().map(|g| g.map.values().map(|v| v.len()).sum()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: these tests use the global state singleton (same
    // pattern as card_pins). Use distinct ids per test to avoid
    // cross-contamination.

    #[test]
    fn normalize_basic() {
        assert_eq!(normalize_tag("hello"), "hello");
        assert_eq!(normalize_tag("Hello World"), "hello-world");
        assert_eq!(normalize_tag("  spaces  "), "spaces");
    }

    #[test]
    fn normalize_strips_punctuation() {
        assert_eq!(normalize_tag("test!?."), "test");
        assert_eq!(normalize_tag("a.b.c"), "abc");
    }

    #[test]
    fn normalize_preserves_special() {
        // These chars stay.
        assert_eq!(normalize_tag("@work"), "@work");
        assert_eq!(normalize_tag("reading-list"), "reading-list");
        assert_eq!(normalize_tag("v1_release"), "v1_release");
        assert_eq!(normalize_tag("path/to/thing"), "path/to/thing");
    }

    #[test]
    fn normalize_empty_returns_empty() {
        assert_eq!(normalize_tag(""), "");
        assert_eq!(normalize_tag("   "), "");
        assert_eq!(normalize_tag("!?!"), "");
    }

    #[test]
    fn add_and_get_roundtrip() {
        let id = "test_user_tags_add_get_xyz";
        clear_all(id); // ensure clean
        assert!(add(id, "important"));
        let tags = get(id);
        assert_eq!(tags, vec!["important"]);
        clear_all(id);
    }

    #[test]
    fn add_duplicate_returns_false() {
        let id = "test_user_tags_dup_xyz";
        clear_all(id);
        assert!(add(id, "tag1"));
        assert!(!add(id, "tag1"), "second add should return false");
        assert!(!add(id, "TAG1"), "normalized duplicate should also return false");
        clear_all(id);
    }

    #[test]
    fn add_normalizes() {
        let id = "test_user_tags_norm_xyz";
        clear_all(id);
        add(id, "Reading List");
        let tags = get(id);
        assert_eq!(tags, vec!["reading-list"]);
        clear_all(id);
    }

    #[test]
    fn remove_present_returns_true() {
        let id = "test_user_tags_rm_xyz";
        clear_all(id);
        add(id, "tag1");
        assert!(remove(id, "tag1"));
        assert!(get(id).is_empty());
        // Removing again returns false.
        assert!(!remove(id, "tag1"));
        clear_all(id);
    }

    #[test]
    fn remove_normalizes() {
        let id = "test_user_tags_rmnorm_xyz";
        clear_all(id);
        add(id, "Reading List");
        // Remove using a differently-cased input.
        assert!(remove(id, "READING LIST"));
        assert!(get(id).is_empty());
        clear_all(id);
    }

    #[test]
    fn add_empty_tag_rejected() {
        let id = "test_user_tags_empty_xyz";
        clear_all(id);
        assert!(!add(id, ""));
        assert!(!add(id, "   "));
        assert!(!add(id, "!?!"));
        assert!(get(id).is_empty());
    }

    #[test]
    fn nodes_with_tag_basic() {
        let id_a = "test_user_tags_filter_a_xyz";
        let id_b = "test_user_tags_filter_b_xyz";
        clear_all(id_a); clear_all(id_b);
        add(id_a, "important");
        add(id_b, "important");
        add(id_b, "other");
        let mut found = nodes_with_tag("important");
        found.sort();
        let mut expected = vec![id_a.to_string(), id_b.to_string()];
        expected.sort();
        assert_eq!(found, expected);
        clear_all(id_a); clear_all(id_b);
    }

    #[test]
    fn nodes_with_tag_normalizes_query() {
        let id = "test_user_tags_filtnorm_xyz";
        clear_all(id);
        add(id, "Reading List");
        // Query with different casing/spacing.
        let found = nodes_with_tag("READING list");
        assert_eq!(found, vec![id.to_string()]);
        clear_all(id);
    }

    #[test]
    fn clear_all_empties_node() {
        let id = "test_user_tags_clearall_xyz";
        clear_all(id);
        add(id, "a");
        add(id, "b");
        add(id, "c");
        let n = clear_all(id);
        assert_eq!(n, 3);
        assert!(get(id).is_empty());
    }
}
