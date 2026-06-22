//! convograph.rs — the conversation "spine": a durable, append-only graph of turns that lives OUTSIDE
//! the model's context window. The model's actual context is a SLICE of this graph (the newest turns
//! within a token budget), so the full history is retained losslessly while what's fed stays minimal.
//! This is the substrate the whole design hangs off — "the session is not the context window."
//!
//! Branches (multiple-solution forks) are representable via `parent` links. Today `spine()` builds a
//! linear chain from the message log; the same model + renderer handle branches once forking is wired.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Turn { User, Assistant }

#[derive(Clone, Debug)]
pub struct Node {
    pub id: usize,
    pub parent: Option<usize>,
    pub turn: Turn,
    pub preview: String,
    pub tokens: usize,
    pub in_context: bool, // currently inside the model's window (the retrieval slice)
    pub keywords: Vec<String>, // content words (topic signal for relevance/segmentation)
    pub looks_back: bool,      // this turn references something earlier ("undo", "the first version"…)
    pub pruned: bool,          // human-pruned: kept in the log but removed from the model's context
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub head: Option<usize>, // the current leaf
}

impl Graph {
    /// Build a linear spine from the turn log — each turn a node whose parent is the previous turn.
    pub fn spine<'a>(turns: impl IntoIterator<Item = (Turn, &'a str)>, est: impl Fn(&str) -> usize) -> Graph {
        let mut nodes: Vec<Node> = Vec::new();
        let mut parent = None;
        for (turn, text) in turns {
            let id = nodes.len();
            nodes.push(Node {
                id, parent, turn,
                preview: preview(text),
                tokens: est(text),
                in_context: false,
                keywords: keywords(text),
                looks_back: cue(text).is_some(),
                pruned: false,
            });
            parent = Some(id);
        }
        let head = nodes.last().map(|n| n.id);
        Graph { nodes, head }
    }

    /// Build a branching tree from turns that each name their parent (None = a root). `head` is the
    /// active leaf whose lineage is the "live" branch; if absent/out of range it falls back to the
    /// last node. Unlike `spine`, two turns may share a parent — a real fork. ids = insertion order.
    pub fn from_tree<'a>(items: impl IntoIterator<Item = (Turn, &'a str, Option<usize>)>, est: impl Fn(&str) -> usize, head: Option<usize>) -> Graph {
        let mut nodes: Vec<Node> = Vec::new();
        for (turn, text, parent) in items {
            let id = nodes.len();
            let parent = parent.filter(|&p| p < id); // parents must precede (guards against bad input/cycles)
            nodes.push(Node {
                id, parent, turn,
                preview: preview(text), tokens: est(text),
                in_context: false, keywords: keywords(text),
                looks_back: cue(text).is_some(), pruned: false,
            });
        }
        let n = nodes.len();
        let head = head.filter(|&h| h < n).or(if n > 0 { Some(n - 1) } else { None });
        Graph { nodes, head }
    }

    /// The lineage root→head of the current leaf.
    pub fn active_path(&self) -> Vec<usize> {
        let mut path = Vec::new();
        let mut cur = self.head;
        while let Some(id) = cur {
            path.push(id);
            cur = self.nodes.get(id).and_then(|n| n.parent);
        }
        path.reverse();
        path
    }

    /// Mark the retrieval slice: walk back from head along the active path, keeping nodes in_context
    /// until the next would exceed `budget` tokens. Newest turns are kept; older ones stay stored but
    /// dimmed. The head is always kept (so a single over-budget turn still shows as in-context).
    /// Returns (nodes_in_context, tokens_used).
    pub fn mark_window(&mut self, budget: usize) -> (usize, usize) {
        self.mark_window_ex(budget, 0, 0, usize::MAX)
    }

    /// Prompt-accurate slice: like `mark_window` but reserving `sys_tokens` for the system prompt
    /// (the caller folds any template-framing margin into this), charging `per_msg` overhead per kept
    /// turn (role markers), and capping at `max_turns`. Mirrors the prompt builder exactly, so the
    /// visual slice == what's fed. Returns (turns_in_context, total_tokens_charged_incl_reservations).
    pub fn mark_window_ex(&mut self, budget: usize, sys_tokens: usize, per_msg: usize, max_turns: usize) -> (usize, usize) {
        for n in &mut self.nodes { n.in_context = false; }
        let path = self.active_path();
        let mut used = sys_tokens;
        let mut count = 0usize;
        for &id in path.iter().rev() {
            if self.nodes[id].pruned { continue; } // human-pruned: out of context, frees budget for others
            let t = self.nodes[id].tokens + per_msg;
            if (count > 0 && used + t > budget) || count >= max_turns { break; }
            self.nodes[id].in_context = true;
            used += t;
            count += 1;
        }
        (count, used)
    }

    /// The ids of in-context nodes in chronological order (oldest→newest) — the turns to feed.
    pub fn context_ids(&self) -> Vec<usize> {
        self.nodes.iter().filter(|n| n.in_context).map(|n| n.id).collect()
    }

    fn overlap(a: &[String], b: &[String]) -> usize { a.iter().filter(|w| b.contains(w)).count() }

    /// REFERENCE-MATCH term. If the head turn looks backward (a back-reference cue), the earlier turns
    /// most relevant to it by keyword overlap — to be PULLED INTO context even if they fell out of the
    /// recency window. Returns up to `top` node ids (chronological), most-overlapping first chosen.
    /// Empty when the head isn't looking back or nothing overlaps. This is "pull from the front on a
    /// keyword like undo": the cue flips retrieval from recent to referenced.
    pub fn back_reference_pins(&self, top: usize) -> Vec<usize> {
        let head = match self.head { Some(h) => h, None => return Vec::new() };
        if !self.nodes[head].looks_back { return Vec::new(); }
        let hk = &self.nodes[head].keywords;
        let mut scored: Vec<(usize, usize)> = self.nodes.iter()
            .filter(|n| n.id != head)
            .map(|n| (n.id, Self::overlap(hk, &n.keywords)))
            .filter(|&(_, s)| s > 0)
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0))); // most overlap, then oldest
        let mut ids: Vec<usize> = scored.into_iter().take(top).map(|(id, _)| id).collect();
        ids.sort();
        ids
    }

    /// SUPERSESSION term, half of it: rough topic segmentation. Contiguous runs of turns that share a
    /// keyword with their predecessor. A run that is FOLLOWED by a topic shift is a completed segment
    /// (an iteration that's done); its last turn is the outcome to keep, the earlier turns in the run
    /// are the churn to collapse/drop first under budget. Returns inclusive (start,end) id ranges.
    pub fn segments(&self) -> Vec<(usize, usize)> {
        if self.nodes.is_empty() { return Vec::new(); }
        let mut segs = Vec::new();
        let mut start = 0usize;
        for i in 1..self.nodes.len() {
            if Self::overlap(&self.nodes[i].keywords, &self.nodes[i - 1].keywords) == 0 {
                segs.push((start, i - 1));
                start = i;
            }
        }
        segs.push((start, self.nodes.len() - 1));
        segs
    }

    /// The "collapsible churn": every turn that is NOT the last of its segment. Dropping these first
    /// (keeping each segment's outcome) frees budget without losing conclusions — the "iteration is
    /// done, keep the end" move. Ids in ascending order.
    pub fn superseded(&self) -> Vec<usize> {
        self.segments().into_iter().flat_map(|(s, e)| s..e).collect() // s..e excludes e (the outcome)
    }

    /// Index of the oldest node still in context (the slice boundary), if any.
    pub fn window_start(&self) -> Option<usize> {
        self.active_path().into_iter().find(|&id| self.nodes[id].in_context)
    }

    pub fn total_tokens(&self) -> usize { self.nodes.iter().map(|n| n.tokens).sum() }
    pub fn context_tokens(&self) -> usize { self.nodes.iter().filter(|n| n.in_context).map(|n| n.tokens).sum() }
}

fn preview(s: &str) -> String {
    let t = s.trim().split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = t.chars().take(42).collect();
    if t.chars().count() > 42 { out.push('…'); }
    out
}

/// Content words of a turn (lowercased, stopwords + short tokens dropped, deduped) — the cheap topic
/// signal for relevance scoring and segmentation. Not linguistics, just a useful overlap signal.
pub fn keywords(text: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the","a","an","and","or","but","to","of","in","on","for","is","it","this","that","with","as",
        "be","are","was","were","i","you","we","he","she","they","my","your","our","can","do","does",
        "please","now","so","if","at","by","from","up","out","not","no","yes","ok","okay","just","then",
        "make","made","write","rewrite","use","using","add","change","fix","like","want","need","get",
    ];
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for w in text.split(|c: char| !c.is_alphanumeric()) {
        let w = w.to_lowercase();
        if w.len() >= 3 && !STOP.contains(&w.as_str()) && seen.insert(w.clone()) { out.push(w); }
    }
    out
}

/// A cue that the relevant context is NOT the recent turns but something earlier.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cue { LookBack }

/// Detect a back-reference cue in a turn — "undo", "go back", "earlier", "the first version",
/// "remember", "you said", "revert", "original". Drives `back_reference_pins`.
pub fn cue(text: &str) -> Option<Cue> {
    let t = text.to_lowercase();
    const BACK: &[&str] = &[
        "undo","revert","roll back","rollback","go back","back to","earlier","previous","prior",
        "the first","first version","original","remember","you said","you wrote","before that",
        "like before","as before","restore","that earlier","the old","initial version",
    ];
    if BACK.iter().any(|k| t.contains(k)) { Some(Cue::LookBack) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn est(s: &str) -> usize { (s.len() / 4).max(1) } // ~4 chars/token, like the app's estimator

    #[test]
    fn from_tree_active_path_follows_the_branch() {
        // 0:q → 1:question ; branch A 2→3 ; branch B 4→5, both forking off the question (node 1)
        let items = vec![
            (Turn::User, "q0", None),
            (Turn::Assistant, "what do you see?", Some(0)),
            (Turn::User, "answer A", Some(1)),
            (Turn::Assistant, "reply A", Some(2)),
            (Turn::User, "answer B", Some(1)),
            (Turn::Assistant, "reply B", Some(4)),
        ];
        let g = Graph::from_tree(items.iter().cloned(), est, Some(5)); // active leaf = branch B tip
        assert_eq!(g.active_path(), vec![0, 1, 4, 5], "active path follows branch B");
        let pa = g.active_path();
        assert!(!pa.contains(&2) && !pa.contains(&3), "branch A is off the active path");
        let kids: Vec<usize> = g.nodes.iter().filter(|n| n.parent == Some(1)).map(|n| n.id).collect();
        assert_eq!(kids, vec![2, 4], "the question node forks into two answers (a real branch)");
        // switching the head to branch A's tip switches the live lineage
        let g2 = Graph::from_tree(items.iter().cloned(), est, Some(3));
        assert_eq!(g2.active_path(), vec![0, 1, 2, 3], "head=3 makes branch A live");
    }

    #[test]
    fn pruning_a_turn_removes_it_from_context_and_frees_budget() {
        let big = "here is a long answer ".repeat(20);
        let turns: Vec<(Turn, &str)> = vec![
            (Turn::User, "first question about vectors and sorting"),
            (Turn::Assistant, big.as_str()),
            (Turn::User, "now in french please"),
            (Turn::Assistant, "voici la version francaise"),
        ];
        let mut g = Graph::spine(turns.iter().cloned(), est);
        g.nodes[1].pruned = true; // human prunes the big assistant turn
        let (_n, used) = g.mark_window_ex(40, 0, 0, usize::MAX);
        assert!(!g.nodes[1].in_context, "pruned turn is out of context");
        assert!(!g.context_ids().contains(&1), "pruned id excluded from the fed turns");
        assert!(used <= 40, "pruned turn frees budget (used {used})");
    }

    fn convo() -> Graph {
        Graph::spine(
            [(Turn::User, "first question about rope scaling"),
             (Turn::Assistant, "a long answer ".repeat(40).leak() as &str),
             (Turn::User, "follow-up"),
             (Turn::Assistant, "short reply")],
            est)
    }

    #[test]
    fn spine_chains_parents_and_sets_head() {
        let g = convo();
        assert_eq!(g.nodes.len(), 4);
        assert_eq!(g.nodes[0].parent, None);
        assert_eq!(g.nodes[1].parent, Some(0));
        assert_eq!(g.nodes[3].parent, Some(2));
        assert_eq!(g.head, Some(3));
    }

    #[test]
    fn active_path_is_root_to_head() {
        assert_eq!(convo().active_path(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn window_keeps_newest_within_budget_and_dims_older() {
        let mut g = convo();
        // budget large enough for the tail but not the big node[1]
        let budget = g.nodes[2].tokens + g.nodes[3].tokens + 2;
        let (count, used) = g.mark_window(budget);
        assert!(used <= budget, "slice fits the budget");
        assert!(g.nodes[3].in_context && g.nodes[2].in_context, "newest turns kept");
        assert!(!g.nodes[1].in_context, "older big turn falls out of the window");
        assert!(!g.nodes[0].in_context, "oldest turn falls out");
        assert_eq!(count, 2);
        assert_eq!(g.window_start(), Some(2), "slice boundary is the oldest in-context node");
    }

    #[test]
    fn head_is_kept_even_if_alone_over_budget() {
        let mut g = convo();
        let (count, _used) = g.mark_window(1); // absurdly small
        assert!(count >= 1, "head always kept");
        assert!(g.nodes[3].in_context, "the head node is the kept one");
        assert!(!g.nodes[2].in_context);
    }

    #[test]
    fn token_accounting() {
        let mut g = convo();
        g.mark_window(usize::MAX);
        assert_eq!(g.context_tokens(), g.total_tokens(), "everything fits ⇒ context == total");
    }

    #[test]
    fn ex_respects_turn_cap() {
        let mut g = convo();
        let (count, _) = g.mark_window_ex(usize::MAX, 0, 0, 2); // cap to 2 turns
        assert_eq!(count, 2, "turn cap honored");
        assert_eq!(g.context_ids(), vec![2, 3], "the 2 newest turns, chronological");
    }

    #[test]
    fn ex_reserves_system_budget() {
        let mut g = convo();
        // sys reservation eats into the budget so fewer turns fit than with mark_window
        let budget = g.total_tokens() + 20;
        let (loose, _) = { let mut h = g.clone(); h.mark_window(budget); (h.context_ids().len(), 0) };
        let (tight, _) = g.mark_window_ex(budget, 1000, 8, usize::MAX);
        assert!(tight <= loose, "reserving sys tokens keeps no more turns than the unreserved slice");
        assert!(g.nodes[3].in_context, "head still kept under reservation");
    }

    // ── relevance primitives (the "context beyond recency" avenues) ──
    fn topical() -> Graph {
        Graph::spine(
            [(Turn::User, "write a quicksort for a vec of ints"),
             (Turn::Assistant, "here is a quicksort implementation with a pivot"),
             (Turn::User, "the pivot choice is wrong, fix the quicksort partition"),
             (Turn::Assistant, "fixed the quicksort partition and pivot"),
             (Turn::User, "now write a haiku about the ocean"),
             (Turn::Assistant, "ocean waves crash softly")],
            est)
    }

    #[test]
    fn keywords_drop_stopwords_and_dedup() {
        let k = keywords("Please fix the quicksort partition, the quicksort pivot");
        assert!(k.contains(&"quicksort".to_string()) && k.contains(&"partition".to_string()) && k.contains(&"pivot".to_string()));
        assert!(!k.contains(&"the".to_string()) && !k.contains(&"fix".to_string()), "stopwords dropped");
        assert_eq!(k.iter().filter(|w| *w == "quicksort").count(), 1, "deduped");
    }

    #[test]
    fn cue_detects_lookback_only_when_present() {
        assert_eq!(cue("undo that last change"), Some(Cue::LookBack));
        assert_eq!(cue("go back to the first version"), Some(Cue::LookBack));
        assert_eq!(cue("remember the original sort?"), Some(Cue::LookBack));
        assert_eq!(cue("write a new function"), None);
    }

    #[test]
    fn back_reference_pulls_the_referenced_earlier_turn() {
        // head looks back AND shares keywords with the early quicksort turns, not the recent haiku
        let g = Graph::spine(
            [(Turn::User, "write a quicksort for ints"),
             (Turn::Assistant, "here is a quicksort"),
             (Turn::User, "now a haiku about the ocean"),
             (Turn::Assistant, "ocean waves crash"),
             (Turn::User, "go back to the quicksort and make the pivot random")],
            est);
        let pins = g.back_reference_pins(2);
        assert!(pins.contains(&0) || pins.contains(&1), "pulls a quicksort turn forward: {pins:?}");
        assert!(!pins.contains(&4), "never pins the head itself");
        // no cue ⇒ no pins
        let g2 = Graph::spine([(Turn::User, "write a quicksort"), (Turn::Assistant, "ok")], est);
        assert!(g2.back_reference_pins(2).is_empty(), "no back-cue ⇒ no pins");
    }

    #[test]
    fn segmentation_splits_on_topic_shift_and_marks_churn() {
        let g = topical();
        let segs = g.segments();
        // quicksort run [0..3] then the ocean/haiku run [4..5]
        assert!(segs.iter().any(|&(s, e)| s == 0 && e >= 2), "quicksort segment found: {segs:?}");
        assert!(segs.len() >= 2, "topic shift to haiku starts a new segment: {segs:?}");
        let churn = g.superseded();
        assert!(churn.contains(&0) && !churn.contains(&3), "outcome (end of quicksort run) kept, churn before it collapsible: {churn:?}");
    }
}
