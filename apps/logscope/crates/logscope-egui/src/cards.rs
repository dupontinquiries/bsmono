//! Ambient suggestion queue (pure, no egui) — scaffolding for the Batch 9 "wide doc-orchestration"
//! popups and the Batch 6 card UI.
//!
//! The load-bearing rule from the design: a suggestion must NOT inject itself into a worker's context
//! the moment it fires — that would perturb an in-flight task. Instead suggestions queue for the
//! human; only `accept` hands back a payload to act on, `dismiss` drops it AND remembers not to nag
//! again, `defer` keeps it but sinks its priority. The queue is content-keyed so the same suggestion
//! firing twice doesn't stack. Priority = a score (e.g. tree-similarity / severity); the UI shows the
//! top card. egui rendering is a thin layer over this; all decisions live here so they can be tested.

use std::collections::HashSet;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardKind {
    /// A file crossed a complexity threshold → "encourage helper libraries?"
    LocThreshold,
    /// The transcript shows novel cross-referenced reasoning → "document these discoveries?"
    Discovery,
    /// Two branches edit the same surface → "reconcile?" (the conflicting-menus case)
    Conflict,
    /// A partial-edit file arrived → "merge as a commit/branch?"
    MergeEdit,
    /// The coordinator (esp. Aggressive mode) is problem-solving and the local corpus likely holds
    /// relevant prior notes/decisions/logs → "search your database to boost context?". On accept, RAG
    /// (rag::Bm25) runs and the top hits are injected; on dismiss, we don't nag again this session.
    SearchDatabase,
    /// Generic info.
    Note,
}

#[derive(Clone, Debug)]
pub struct Card {
    pub id: u64,
    /// Stable content key: dedup + dismissal memory hang off this, NOT the id (so the same
    /// suggestion re-firing is suppressed even with a fresh id).
    pub key: String,
    pub kind: CardKind,
    pub title: String,
    pub body: String,
    pub action: String, // label for the accept button ("Generate helper-library doc", "Merge", …)
    pub score: f32,     // priority; higher = surfaced first
    deferred: bool,
}

#[derive(Default)]
pub struct CardQueue {
    pending: Vec<Card>,
    dismissed: HashSet<String>, // keys the user said no to → never re-suggest this session
    next_id: u64,
}

impl CardQueue {
    pub fn new() -> Self { Self::default() }
    pub fn len(&self) -> usize { self.pending.len() }
    pub fn is_empty(&self) -> bool { self.pending.is_empty() }

    /// Offer a suggestion. Returns the assigned id if it was queued, or None if suppressed (already
    /// dismissed, or already pending under the same key). Re-offering a pending key refreshes its
    /// score to the max of old/new (an escalating signal can raise priority) without duplicating.
    pub fn suggest(&mut self, key: &str, kind: CardKind, title: &str, body: &str, action: &str, score: f32) -> Option<u64> {
        if self.dismissed.contains(key) { return None; }
        if let Some(c) = self.pending.iter_mut().find(|c| c.key == key) {
            c.score = c.score.max(score);
            return None;
        }
        let id = self.next_id; self.next_id += 1;
        self.pending.push(Card {
            id, key: key.to_string(), kind, title: title.to_string(), body: body.to_string(),
            action: action.to_string(), score, deferred: false,
        });
        Some(id)
    }

    /// The card to show now: highest score, with deferred cards sinking below non-deferred ones.
    pub fn top(&self) -> Option<&Card> {
        self.pending.iter().max_by(|a, b| {
            a.deferred.cmp(&b.deferred).reverse() // non-deferred (false) first
                .then(a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal))
        })
    }

    /// All cards, surfaced order (for a stacked/list view).
    pub fn ordered(&self) -> Vec<&Card> {
        let mut v: Vec<&Card> = self.pending.iter().collect();
        v.sort_by(|a, b| a.deferred.cmp(&b.deferred)
            .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)));
        v
    }

    /// User accepted: remove and hand back the card so the caller can act (inject context, generate a
    /// doc, run the merge). This is the ONLY path that feeds anything into a worker.
    pub fn accept(&mut self, id: u64) -> Option<Card> {
        let i = self.pending.iter().position(|c| c.id == id)?;
        Some(self.pending.remove(i))
    }

    /// User dismissed: remove and remember the key so the same suggestion won't nag again.
    pub fn dismiss(&mut self, id: u64) {
        if let Some(i) = self.pending.iter().position(|c| c.id == id) {
            let c = self.pending.remove(i);
            self.dismissed.insert(c.key);
        }
    }

    /// Keep but sink priority (the user isn't ready but doesn't want it gone).
    pub fn defer(&mut self, id: u64) {
        if let Some(c) = self.pending.iter_mut().find(|c| c.id == id) { c.deferred = true; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_by_key_not_id() {
        let mut q = CardQueue::new();
        assert!(q.suggest("file:world.rs:loc", CardKind::LocThreshold, "1K LOC", "world.rs crossed 1000 lines", "Suggest helpers", 0.5).is_some());
        assert!(q.suggest("file:world.rs:loc", CardKind::LocThreshold, "1K LOC", "again", "Suggest helpers", 0.9).is_none(), "same key must not stack");
        assert_eq!(q.len(), 1);
        assert!((q.top().unwrap().score - 0.9).abs() < 1e-6, "re-offer escalates score to the max");
    }

    #[test]
    fn dismiss_suppresses_future() {
        let mut q = CardQueue::new();
        let id = q.suggest("k", CardKind::Discovery, "t", "b", "Document", 0.3).unwrap();
        q.dismiss(id);
        assert!(q.is_empty());
        assert!(q.suggest("k", CardKind::Discovery, "t", "b", "Document", 0.3).is_none(), "dismissed key never re-suggests");
    }

    #[test]
    fn accept_returns_and_removes() {
        let mut q = CardQueue::new();
        let id = q.suggest("k", CardKind::MergeEdit, "Merge?", "partial edit to main.rs", "Merge as branch", 0.7).unwrap();
        let c = q.accept(id).expect("accept returns the card");
        assert_eq!(c.kind, CardKind::MergeEdit);
        assert!(q.is_empty(), "accepted card leaves the queue");
        // accepting is NOT a dismissal — the same suggestion could legitimately fire again later
        assert!(q.suggest("k", CardKind::MergeEdit, "Merge?", "x", "Merge as branch", 0.7).is_some());
    }

    #[test]
    fn priority_and_defer_ordering() {
        let mut q = CardQueue::new();
        let lo = q.suggest("lo", CardKind::Note, "lo", "", "ok", 0.2).unwrap();
        let _hi = q.suggest("hi", CardKind::Note, "hi", "", "ok", 0.9).unwrap();
        assert_eq!(q.top().unwrap().key, "hi", "highest score surfaces");
        q.defer(_hi);
        assert_eq!(q.top().unwrap().key, "lo", "deferring the top sinks it below others");
        let _ = lo;
    }
}
