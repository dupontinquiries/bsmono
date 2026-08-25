//! prefixcache — don't ask the model again for something it already answered.
//!
//! The problem this solves is the one you feel while editing: the caret
//! moves, the debounce fires, and a request goes out that is byte-for-byte
//! the same context as the last one. Arrow-keying along a line, clicking
//! into a spot you just left, undo-then-redo — none of those change what
//! the model would see, but each one re-triggers a completion that takes
//! hundreds of milliseconds and burns a GPU that has other work.
//!
//! Three mechanisms, in the order they fire:
//!
//! 1. **Exact hit.** The same prefix (and language) was answered before →
//!    return it, no request. This alone kills most caret-motion churn,
//!    because moving the caret WITHOUT typing does not change the prefix
//!    at all.
//!
//! 2. **Differential / prefix-extension hit.** You typed a few characters
//!    that the previous completion had already predicted. If the cached
//!    answer for a SHORTER prefix starts with exactly what you have since
//!    typed, the remainder of that answer is still the right suggestion —
//!    return the tail. This is what makes typing along an accepted ghost
//!    free rather than one request per keystroke. It is also strictly
//!    safe: the model already committed to those characters, and you typed
//!    them, so the continuation it proposed is unchanged.
//!
//! 3. **Line-identity survival.** Deleting a line elsewhere in the file
//!    shifts every line number after it, which naively invalidates
//!    everything. But a completion is a function of the TEXT, not of line
//!    numbers, so a per-line content map lets an entry survive edits that
//!    did not touch the region it was computed from. [`LineMap`] tracks
//!    each line's hash by content so "line 40 was deleted" can be
//!    distinguished from "line 40 was rewritten", which decides whether an
//!    entry keyed near there is still trustworthy.
//!
//! ## Multiple caret positions
//!
//! [`PrefixCache::plan`] takes SEVERAL prefixes (the real caret plus the
//! other cursors in a multi-cursor edit, or speculative next positions)
//! and returns which are already answered and which still need the model.
//! That is the batching seam: the misses come back as one list, so a
//! backend that can run a batch — which is the whole point of a KV-cached
//! engine holding several sequences at once — gets one call instead of N,
//! and the hits never leave this process.
//!
//! This module is PURE. It does no I/O, holds no engine handle, and knows
//! nothing about wgpu; it decides what to ask for. That is deliberate, and
//! it is why the whole thing is unit-testable with no model present: the
//! expensive part is elsewhere, and the decision to skip the expensive
//! part is the part worth pinning with tests.

use std::collections::HashMap;

/// FNV-1a. A hash, not a cryptographic one — collisions here cost a stale
/// suggestion, not a security property, and this runs per line per edit so
/// it has to be cheap. Written out rather than pulled in because this
/// crate has no dependencies and is keeping it that way.
fn hash64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// What a cached answer is keyed on. The language matters: the same text
/// in Rust and in Markdown should not share a completion.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub prefix_hash: u64,
    pub lang: String,
}

impl Key {
    pub fn new(prefix: &str, lang: &str) -> Key {
        Key { prefix_hash: hash64(prefix), lang: lang.to_string() }
    }
}

/// One remembered answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The full prefix this was computed from, kept so a differential hit
    /// can verify by CONTENT rather than trusting the hash alone — a
    /// 64-bit collision returning someone else's completion would be a
    /// baffling bug, and the string is already in memory.
    pub prefix: String,
    pub completion: String,
    /// Monotonic counter, for eviction. Not a clock: this crate is pure
    /// and a wall clock would make its tests time-dependent.
    pub stamp: u64,
}

/// Why a lookup succeeded, so a caller (and the loop pane) can see whether
/// the cache is actually earning its keep.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    /// The identical prefix was answered before.
    Exact(String),
    /// A shorter prefix was answered, and the characters typed since are
    /// exactly what it predicted — this is the remaining tail.
    Extended { completion: String, consumed: usize },
}

/// What [`PrefixCache::plan`] decided for one requested position.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    Have(Hit),
    /// Nothing usable; this prefix has to go to the model.
    Need,
}

pub struct PrefixCache {
    entries: HashMap<Key, Entry>,
    /// Insertion order for eviction, oldest first.
    order: Vec<Key>,
    cap: usize,
    stamp: u64,
    hits: u64,
    misses: u64,
}

impl PrefixCache {
    pub fn new(cap: usize) -> PrefixCache {
        PrefixCache {
            entries: HashMap::new(),
            order: Vec::new(),
            cap: cap.max(1),
            stamp: 0,
            hits: 0,
            misses: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// (hits, misses) — the number the loop pane should show, because a
    /// cache nobody can see the hit rate of is a cache nobody can tune.
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    pub fn insert(&mut self, prefix: &str, lang: &str, completion: &str) {
        let key = Key::new(prefix, lang);
        self.stamp += 1;
        let stamp = self.stamp;
        if self.entries.contains_key(&key) {
            self.order.retain(|k| k != &key);
        }
        self.entries.insert(
            key.clone(),
            Entry { prefix: prefix.to_string(), completion: completion.to_string(), stamp },
        );
        self.order.push(key);
        while self.order.len() > self.cap {
            let oldest = self.order.remove(0);
            self.entries.remove(&oldest);
        }
    }

    /// Exact lookup only. Does not count statistics — [`plan`](Self::plan)
    /// and [`get`](Self::get) are the counted entry points, so a caller
    /// peeking does not distort the hit rate the pane reports.
    pub fn peek(&self, prefix: &str, lang: &str) -> Option<&Entry> {
        self.entries.get(&Key::new(prefix, lang)).filter(|e| e.prefix == prefix)
    }

    /// The full lookup: exact, then differential.
    pub fn get(&mut self, prefix: &str, lang: &str) -> Option<Hit> {
        if let Some(e) = self.entries.get(&Key::new(prefix, lang)) {
            // Verify by content: a hash collision must not serve someone
            // else's completion.
            if e.prefix == prefix {
                self.hits += 1;
                return Some(Hit::Exact(e.completion.clone()));
            }
        }
        // Differential: find the LONGEST cached prefix that this one
        // extends, where everything typed since was already predicted.
        // Longest wins because it consumed the most of its completion and
        // therefore leaves the most reliable tail — a shorter ancestor's
        // remaining text has had more chances to diverge.
        let mut best: Option<(usize, String, usize)> = None;
        for e in self.entries.values() {
            if e.prefix.len() >= prefix.len() || !prefix.starts_with(&e.prefix) {
                continue;
            }
            let typed = &prefix[e.prefix.len()..];
            if !e.completion.starts_with(typed) {
                continue; // you diverged from what it predicted
            }
            let tail = e.completion[typed.len()..].to_string();
            if tail.is_empty() {
                continue; // fully consumed: there is nothing left to suggest
            }
            let consumed = typed.len();
            if best.as_ref().is_none_or(|(len, _, _)| e.prefix.len() > *len) {
                best = Some((e.prefix.len(), tail, consumed));
            }
        }
        match best {
            Some((_, completion, consumed)) => {
                self.hits += 1;
                Some(Hit::Extended { completion, consumed })
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }

    /// Decide for SEVERAL positions at once — the batching seam. Returns
    /// one `Plan` per input, in the same order, so the caller can send
    /// exactly the `Need`s to the model as a single batch and serve the
    /// rest from here.
    pub fn plan(&mut self, prefixes: &[(String, String)]) -> Vec<Plan> {
        prefixes
            .iter()
            .map(|(prefix, lang)| match self.get(prefix, lang) {
                Some(h) => Plan::Have(h),
                None => Plan::Need,
            })
            .collect()
    }

    /// Drop everything: a new file, or a language change.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    /// Drop entries whose prefix contains text that no longer exists in
    /// the buffer. Called after an edit with the lines that changed
    /// identity — see [`LineMap::diff`]. An entry survives when the edit
    /// happened somewhere its prefix never covered.
    pub fn invalidate_containing(&mut self, gone: &[String]) {
        if gone.is_empty() {
            return;
        }
        let doomed: Vec<Key> = self
            .entries
            .iter()
            .filter(|(_, e)| gone.iter().any(|g| !g.trim().is_empty() && e.prefix.contains(g.as_str())))
            .map(|(k, _)| k.clone())
            .collect();
        for k in doomed {
            self.entries.remove(&k);
            self.order.retain(|o| o != &k);
        }
    }
}

/// Lines by CONTENT, so an edit can be described as "these texts left"
/// rather than "everything after line 40 shifted".
///
/// The distinction matters for a completion cache: deleting a line renumbers
/// everything below it, which would invalidate the whole file if entries
/// were keyed on position. Keyed on content, an entry computed from an
/// untouched region stays valid no matter how far it moved.
#[derive(Debug, Default, Clone)]
pub struct LineMap {
    hashes: Vec<u64>,
    texts: HashMap<u64, String>,
}

impl LineMap {
    pub fn new() -> LineMap {
        LineMap::default()
    }

    pub fn from_lines<'a>(lines: impl Iterator<Item = &'a str>) -> LineMap {
        let mut m = LineMap::new();
        m.rebuild(lines);
        m
    }

    pub fn rebuild<'a>(&mut self, lines: impl Iterator<Item = &'a str>) {
        self.hashes.clear();
        self.texts.clear();
        for l in lines {
            let h = hash64(l);
            self.hashes.push(h);
            self.texts.entry(h).or_insert_with(|| l.to_string());
        }
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    /// Which line TEXTS are in `self` but not in `next` — the content that
    /// genuinely left the buffer. A line that merely moved (because
    /// something above it was deleted) is in both and is NOT reported,
    /// which is the entire point.
    ///
    /// "Gone" means the text is absent from `next` ENTIRELY, not that it
    /// occurs fewer times. Deleting one of two identical lines leaves the
    /// text still present, so anything cached against it is still valid —
    /// reporting it gone would throw away good entries every time a
    /// duplicate line (a lone `}`, a blank line, a repeated `use`) was
    /// removed anywhere in the file, which in real code is constantly.
    /// Caught by its own test rather than reasoned about: the first
    /// version compared counts and was wrong for exactly this case.
    pub fn diff(&self, next: &LineMap) -> Vec<String> {
        let mut still: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for h in &next.hashes {
            still.insert(*h);
        }
        let mut gone = Vec::new();
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for h in &self.hashes {
            if still.contains(h) || !seen.insert(*h) {
                continue;
            }
            if let Some(t) = self.texts.get(h) {
                gone.push(t.clone());
            }
        }
        gone.sort();
        gone
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c() -> PrefixCache {
        PrefixCache::new(64)
    }

    #[test]
    fn moving_the_caret_without_typing_costs_nothing() {
        // The headline case: the same context asked twice must not reach
        // the model twice. Caret motion that does not change the prefix is
        // most of what an editor does between keystrokes.
        let mut k = c();
        k.insert("fn main() {\n    ", "rust", "println!(\"hi\");");
        assert_eq!(
            k.get("fn main() {\n    ", "rust"),
            Some(Hit::Exact("println!(\"hi\");".into()))
        );
        assert_eq!(k.stats(), (1, 0));
    }

    #[test]
    fn typing_what_was_predicted_reuses_the_tail_instead_of_asking_again() {
        // The differential case, and the one that makes typing along a
        // suggestion free: the model already committed to "println!(...)",
        // you typed "print", so the rest of ITS answer is still the answer.
        let mut k = c();
        k.insert("let x = ", "rust", "compute(42);");
        match k.get("let x = comp", "rust") {
            Some(Hit::Extended { completion, consumed }) => {
                assert_eq!(completion, "ute(42);");
                assert_eq!(consumed, 4, "the four characters typed since");
            }
            other => panic!("expected an extension hit, got {other:?}"),
        }
    }

    #[test]
    fn diverging_from_the_prediction_is_a_miss_not_a_wrong_answer() {
        // The safety property. If what you typed is NOT what it predicted,
        // the cached tail is meaningless and serving it would put text you
        // never asked for in front of you.
        let mut k = c();
        k.insert("let x = ", "rust", "compute(42);");
        assert_eq!(k.get("let x = other", "rust"), None);
        assert_eq!(k.stats(), (0, 1));
    }

    #[test]
    fn the_longest_matching_ancestor_wins() {
        // Two ancestors both predict correctly; the longer one has had
        // fewer characters' worth of chances to drift, so its tail is the
        // better suggestion.
        let mut k = c();
        k.insert("fn ", "rust", "main() {}");
        k.insert("fn main(", "rust", ") {}");
        match k.get("fn main()", "rust") {
            Some(Hit::Extended { completion, .. }) => {
                assert_eq!(completion, " {}", "from the longer ancestor, not `fn `");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn language_is_part_of_the_key() {
        let mut k = c();
        k.insert("# ", "markdown", "Heading");
        assert_eq!(k.get("# ", "rust"), None, "a Rust comment is not a Markdown heading");
        assert!(k.get("# ", "markdown").is_some());
    }

    #[test]
    fn a_fully_consumed_completion_stops_being_offered() {
        // You typed the whole suggestion. There is no tail left, and
        // offering an empty ghost would read as "the model has no idea"
        // rather than "you already wrote it".
        let mut k = c();
        k.insert("let x = ", "rust", "1;");
        assert_eq!(k.get("let x = 1;", "rust"), None);
    }

    #[test]
    fn plan_separates_the_answered_positions_from_the_ones_to_batch() {
        // The multi-cursor / batching seam: several positions in, one list
        // out, so only the Needs go to the model — as one batch.
        let mut k = c();
        k.insert("let a = ", "rust", "1;");
        let asked = vec![
            ("let a = ".to_string(), "rust".to_string()),   // exact
            ("let a = 1".to_string(), "rust".to_string()),  // extension
            ("let b = ".to_string(), "rust".to_string()),   // unknown
        ];
        let plan = k.plan(&asked);
        assert!(matches!(plan[0], Plan::Have(Hit::Exact(_))));
        assert!(matches!(plan[1], Plan::Have(Hit::Extended { .. })));
        assert_eq!(plan[2], Plan::Need);
        assert_eq!(
            plan.iter().filter(|p| **p == Plan::Need).count(),
            1,
            "one request instead of three"
        );
    }

    #[test]
    fn eviction_drops_the_oldest_and_keeps_the_cache_bounded() {
        let mut k = PrefixCache::new(2);
        k.insert("a", "rust", "1");
        k.insert("b", "rust", "2");
        k.insert("c", "rust", "3");
        assert_eq!(k.len(), 2);
        assert!(k.peek("a", "rust").is_none(), "oldest evicted");
        assert!(k.peek("c", "rust").is_some());
    }

    #[test]
    fn a_deleted_line_invalidates_only_what_actually_contained_it() {
        // The line-identity property. Deleting a line renumbers everything
        // below it; entries computed from untouched text must SURVIVE that,
        // or every edit anywhere throws the whole cache away.
        let before = LineMap::from_lines(["fn a() {}", "fn doomed() {}", "fn b() {}"].into_iter());
        let after = LineMap::from_lines(["fn a() {}", "fn b() {}"].into_iter());
        let gone = before.diff(&after);
        assert_eq!(gone, vec!["fn doomed() {}"], "only the line that truly left");

        let mut k = c();
        k.insert("fn a() {}\nfn doomed() {}\n", "rust", "X");
        k.insert("fn a() {}\n", "rust", "Y");
        k.invalidate_containing(&gone);
        assert!(k.peek("fn a() {}\nfn doomed() {}\n", "rust").is_none(), "contained the dead line");
        assert!(k.peek("fn a() {}\n", "rust").is_some(), "untouched region survives the renumber");
    }

    #[test]
    fn a_line_that_only_moved_is_not_reported_gone() {
        // Deleting the FIRST line shifts every other line's number. Keyed
        // on content, nothing else is considered lost.
        let before = LineMap::from_lines(["one", "two", "three"].into_iter());
        let after = LineMap::from_lines(["two", "three"].into_iter());
        assert_eq!(before.diff(&after), vec!["one"]);
    }

    #[test]
    fn duplicate_lines_are_counted_not_deduped() {
        // Two identical lines, one deleted: the text still exists, so
        // nothing keyed on it should be invalidated. Counting rather than
        // set-membership is what gets this right.
        let before = LineMap::from_lines(["x", "x", "y"].into_iter());
        let after = LineMap::from_lines(["x", "y"].into_iter());
        assert!(before.diff(&after).is_empty(), "one copy remains, so the text is not gone");

        let all_gone = LineMap::from_lines(["y"].into_iter());
        assert_eq!(before.diff(&all_gone), vec!["x"]);
    }
}
