//! route_ai — the AI-assisted second tier of "go to via route", on top of
//! [`crate::Provider`]/[`crate::engine::Engine`] the same way ghost
//! completion and [`crate::edit`] are. `aperture_core::route`'s
//! deterministic keyword-scored candidates are tier one (instant, always
//! available); this module builds the prompt that asks a model to pick and
//! describe the best few of them in plain language, and caches the answer.
//!
//! Architecture is a direct port of logscope's `filesummary.rs` two-tier
//! design (own doc: "a structural digest — pure, instant, deterministic"
//! plus "a model summary — generated in the background... cached by
//! content hash so a file is summarized once"; servicing seam is
//! `take_pending`/`fulfill`, drained by whatever backend is configured).
//! Same shape here: [`request_route_summary`] queues a prompt keyed by a
//! hash of the query + candidate set, [`take_pending`] is what a worker
//! drains, [`fulfill`] delivers the answer, [`route_summary`]/[`in_flight`]
//! are what the UI polls. No worker thread lives in this crate — same
//! reason `edit.rs` only builds the request: standing up and verifying a
//! live model call needs a running provider this crate can't supply or
//! test itself. `aperture-egui` is where a real worker would be wired,
//! against the same `Engine`/`Provider` ghost completion already uses.
//!
//! Deliberately decoupled from `aperture_core::route::RouteCandidate`
//! (this crate has zero dependencies by design — see the crate doc) via
//! [`CandidateBrief`], a plain data shape the shell converts into.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// The minimum a candidate needs to be worth describing to a model — not
/// `aperture_core::route::RouteCandidate` itself, to keep this crate
/// dependency-free (see the module doc).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CandidateBrief {
    pub label: String,
    pub rel: String,
    pub line: usize,
    pub snippet: String,
}

/// The prompt a small model gets: the user's natural-language query, the
/// deterministic tier's top candidates, and a tight, parseable output
/// shape — same posture as logscope's `summary_prompt` ("tight task spec;
/// small models follow a shape, not a persona").
pub fn route_prompt(query: &str, candidates: &[CandidateBrief]) -> String {
    let mut listing = String::new();
    for (i, c) in candidates.iter().enumerate() {
        listing.push_str(&format!("{}. {}:{} — `{}`", i + 1, c.rel, c.line + 1, c.label));
        if !c.snippet.is_empty() {
            listing.push_str(&format!(" ({})", c.snippet));
        }
        listing.push('\n');
    }
    format!(
        "A developer asked to go to: \"{query}\"\n\n\
         Here are candidate locations found by keyword search, numbered:\n\n\
         {listing}\n\
         Pick the 1-3 candidates that best match what the developer is looking for. \
         For each, reply with its number and ONE short sentence explaining why it's a match. \
         No preamble, no other candidates, no code blocks — just the numbered picks."
    )
}

/// Content key for the cache: a hash of the query plus the exact candidate
/// set offered — same role as `filesummary::content_key`, but over the
/// query+candidates instead of a file's body, since that's what the prompt
/// actually depends on.
pub fn route_key(query: &str, candidates: &[CandidateBrief]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    query.hash(&mut h);
    candidates.hash(&mut h);
    h.finish()
}

struct Store {
    summaries: HashMap<u64, String>,
    pending: Vec<(u64, String)>,
    requested: HashMap<u64, ()>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(|| Store { summaries: HashMap::new(), pending: Vec::new(), requested: HashMap::new() }))
}

/// The cached model answer for a key, if one has landed.
pub fn route_summary(key: u64) -> Option<String> {
    with_store(|s| s.summaries.get(&key).cloned())
}

/// Ask for a model answer (idempotent per key — a repeat call while one is
/// already queued/in-flight for this exact query+candidate set is a no-op).
pub fn request_route_summary(query: &str, candidates: &[CandidateBrief]) -> u64 {
    let key = route_key(query, candidates);
    with_store(|s| {
        if !s.summaries.contains_key(&key) && !s.requested.contains_key(&key) {
            s.requested.insert(key, ());
            s.pending.push((key, route_prompt(query, candidates)));
        }
    });
    key
}

/// Take one queued request — the servicing seam a worker (e.g. an
/// `Engine` wired to a real `Provider`, or logscope's own ollama executor
/// pattern) drains.
pub fn take_pending() -> Option<(u64, String)> {
    with_store(|s| s.pending.pop())
}

/// Deliver a finished answer.
pub fn fulfill(key: u64, text: String) {
    with_store(|s| {
        s.summaries.insert(key, text);
    });
}

/// True when a request is queued or being generated for `key`, but hasn't
/// landed yet.
pub fn in_flight(key: u64) -> bool {
    with_store(|s| s.requested.contains_key(&key) && !s.summaries.contains_key(&key))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses distinct query text so keys never collide across
    // tests sharing the process-wide STORE.

    fn brief(label: &str, rel: &str, line: usize) -> CandidateBrief {
        CandidateBrief { label: label.to_string(), rel: rel.to_string(), line, snippet: String::new() }
    }

    #[test]
    fn prompt_lists_every_candidate_numbered_with_location() {
        let cands = vec![brief("account_setup_wizard", "wizard.rs", 12), brief("Button::new(\"Set up\")", "ui.rs", 40)];
        let p = route_prompt("find the setup wizard button", &cands);
        assert!(p.contains("1. wizard.rs:13"));
        assert!(p.contains("account_setup_wizard"));
        assert!(p.contains("2. ui.rs:41"));
        assert!(p.contains("find the setup wizard button"));
    }

    #[test]
    fn request_then_take_then_fulfill_round_trips() {
        let cands = vec![brief("thing_a", "a.rs", 1)];
        let key = request_route_summary("round-trip-test query alpha", &cands);
        assert!(in_flight(key));
        assert!(route_summary(key).is_none());

        let (taken_key, _prompt) = take_pending().expect("a request was queued");
        assert_eq!(taken_key, key);

        fulfill(key, "1. a.rs — matches because it's the only candidate.".to_string());
        assert!(!in_flight(key));
        assert_eq!(route_summary(key).as_deref(), Some("1. a.rs — matches because it's the only candidate."));
    }

    #[test]
    fn a_repeat_request_for_the_same_key_does_not_queue_twice() {
        let cands = vec![brief("thing_b", "b.rs", 2)];
        let k1 = request_route_summary("dedup-test query beta", &cands);
        let k2 = request_route_summary("dedup-test query beta", &cands);
        assert_eq!(k1, k2);
        // exactly one item queued despite two requests
        let first = take_pending();
        assert!(first.is_some());
        // the second call was a no-op, so there's nothing else queued for this key
        while let Some((k, _)) = take_pending() {
            assert_ne!(k, k1, "should not have queued a duplicate for the same key");
        }
    }

    #[test]
    fn different_candidate_sets_for_the_same_query_get_different_keys() {
        let cands_a = vec![brief("thing_c", "c.rs", 3)];
        let cands_b = vec![brief("thing_d", "d.rs", 4)];
        let ka = route_key("same query gamma", &cands_a);
        let kb = route_key("same query gamma", &cands_b);
        assert_ne!(ka, kb);
    }
}
