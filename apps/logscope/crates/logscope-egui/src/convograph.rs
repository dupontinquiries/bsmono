//! convograph.rs — the conversation "spine": a durable, append-only graph of turns that lives OUTSIDE
//! the model's context window. The model's actual context is a SLICE of this graph (the newest turns
//! within a token budget), so the full history is retained losslessly while what's fed stays minimal.
//! This is the substrate the whole design hangs off — "the session is not the context window."
//!
//! Branches (multiple-solution forks) are representable via `parent` links. Today `spine()` builds a
//! linear chain from the message log; the same model + renderer handle branches once forking is wired.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Turn { User, Assistant }

/// What a node *is*. A text turn matches on its body; an IMAGE node matches on its NAME + generation prompt;
/// a SHOT node animates a keyframe image (its `refers` points at that image's uuid) and matches on its motion
/// prompt + the keyframe's own keywords. All three are keyword bags to the matcher — retrieval never branches
/// on kind. Pixels/clips live in a side-table keyed by [`Node::uuid`], not here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text, Image, Shot, Patch, File,
    /// A liaison line: the backend narrating itself. Not a turn (nobody said it) and not an artifact
    /// (nothing was produced) — a third category, which is why it was invisible when the graph only had
    /// two.
    Liaison,
    /// A FAN-OUT zone: one task sent to several models at once. Marks where the timeline stops being a
    /// line, so a reader can see that three sibling nodes are alternatives rather than a sequence.
    FanOut,
    /// Where a fan-out was resolved back to one answer.
    Merge,
}

impl Kind {
    /// Is this an ARTIFACT — something the conversation produced — rather than a turn of the conversation?
    ///
    /// The distinction is what the graph is for: a reader scanning it wants to see "here is where a patch
    /// was applied" without reading every message. Artifacts and turns therefore render differently
    /// (shape, colour, edge style) rather than being one undifferentiated stream of boxes.
    pub fn is_artifact(self) -> bool { matches!(self, Kind::Patch | Kind::File | Kind::Image | Kind::Shot) }

    /// Does this node mark a change in the timeline's SHAPE rather than carrying content?
    ///
    /// Fan-out and merge are structural: they say "the next three nodes are alternatives" and "they came
    /// back together here". Rendering them like content nodes is what made a fan look identical to three
    /// consecutive replies.
    pub fn is_structural(self) -> bool { matches!(self, Kind::FanOut | Kind::Merge) }

    /// Backend narration — neither a turn nor an artifact.
    pub fn is_narration(self) -> bool { matches!(self, Kind::Liaison) }

    /// Short glyph for the node badge.
    pub fn glyph(self) -> &'static str {
        match self {
            Kind::Text => "", Kind::Image => "🖼", Kind::Shot => "🎬", Kind::Patch => "⑂", Kind::File => "🗎",
            Kind::Liaison => "◈", Kind::FanOut => "⋔", Kind::Merge => "⋏",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Text => "message", Kind::Image => "image", Kind::Shot => "shot",
            Kind::Patch => "patch", Kind::File => "file",
            Kind::Liaison => "backend", Kind::FanOut => "fan-out", Kind::Merge => "merge",
        }
    }
}

/// Stable, session-independent identity — what the chat refers to a node by ("that galaxy image") and what
/// an image's pixel payload is keyed on. Distinct from `id`, a positional index into this graph. Derived
/// deterministically from content + index (swap for a random UUIDv4 later without changing the type).
fn gen_uuid(a: &str, b: &str, salt: u64) -> u128 {
    use std::hash::{Hash, Hasher};
    let mut h1 = std::collections::hash_map::DefaultHasher::new();
    (a, b, salt, 0x9E37_79B9_7F4A_7C15u64).hash(&mut h1);
    let mut h2 = std::collections::hash_map::DefaultHasher::new();
    (salt, b, a, 0xD1B5_4A32_D192_ED03u64).hash(&mut h2);
    ((h1.finish() as u128) << 64) | (h2.finish() as u128)
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: usize,
    pub uuid: u128,            // stable identity (chat references + image payload key); id is only positional
    pub kind: Kind,            // Text turn vs Image node vs Shot — retrieval treats all as keyword bags
    pub refers: Option<u128>,  // a SEMANTIC edge by uuid (a shot → its keyframe, an edit → its source); not the tree parent
    pub parent: Option<usize>,
    pub turn: Turn,
    pub preview: String,
    pub tokens: usize,
    pub in_context: bool, // currently inside the model's window (the retrieval slice)
    pub keywords: Vec<String>, // content words (topic signal for relevance/segmentation)
    pub looks_back: bool,      // this turn references something earlier ("undo", "the first version"…)
    pub pruned: bool,          // human-pruned: kept in the log but removed from the model's context
}

/// Bytes in the shortest form that stays unambiguous.
fn human_bytes(b: u64) -> String {
    const K: u64 = 1024;
    if b < K { format!("{b} B") }
    else if b < K * K { format!("{:.1} KiB", b as f64 / K as f64) }
    else if b < K * K * K { format!("{:.1} MiB", b as f64 / (K * K) as f64) }
    else { format!("{:.2} GiB", b as f64 / (K * K * K) as f64) }
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
                id, uuid: gen_uuid(text, "", id as u64), kind: Kind::Text, refers: None, parent, turn,
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
                id, uuid: gen_uuid(text, "", id as u64), kind: Kind::Text, refers: None, parent, turn,
                preview: preview(text), tokens: est(text),
                in_context: false, keywords: keywords(text),
                looks_back: cue(text).is_some(), pruned: false,
            });
        }
        let n = nodes.len();
        let head = head.filter(|&h| h < n).or(if n > 0 { Some(n - 1) } else { None });
        Graph { nodes, head }
    }

    /// Append a live TEXT turn (parent defaults to the current head) and return its uuid — the incremental
    /// counterpart to the batch `spine`/`from_tree` builders, so the graph can grow a turn at a time.
    pub fn push_turn(&mut self, turn: Turn, text: &str, parent: Option<usize>, est: impl Fn(&str) -> usize) -> u128 {
        let id = self.nodes.len();
        let parent = parent.or(self.head).filter(|&p| p < id);
        let uuid = gen_uuid(text, "", id as u64);
        self.nodes.push(Node {
            id, uuid, kind: Kind::Text, refers: None, parent, turn,
            preview: preview(text), tokens: est(text),
            in_context: false, keywords: keywords(text),
            looks_back: cue(text).is_some(), pruned: false,
        });
        self.head = Some(id);
        uuid
    }

    /// Append an IMAGE node to the graph and return its stable [`Node::uuid`]. The image's matchable surface
    /// is its `name` plus the generation `prompt` — those become its `keywords`, so it's retrieved by the very
    /// same keyword-overlap machinery ([`back_reference_pins`], [`segments`]) as any text turn; a later turn
    /// that mentions the image's name pulls it up from the front just like referencing an earlier reply. The
    /// pixel payload is stored elsewhere, keyed by the returned uuid. `est` charges its (small) token cost —
    /// an image in context costs only its name/caption tokens, not the pixels.
    pub fn push_image(&mut self, name: &str, prompt: &str, parent: Option<usize>, est: impl Fn(&str) -> usize) -> u128 {
        let id = self.nodes.len();
        let parent = parent.or(self.head).filter(|&p| p < id); // chain off the current head by default
        let uuid = gen_uuid(name, prompt, id as u64);
        let mut kw = keywords(name);
        for w in keywords(prompt) { if !kw.contains(&w) { kw.push(w); } } // name + prompt terms = its surface
        self.nodes.push(Node {
            id, uuid, kind: Kind::Image, refers: None, parent, turn: Turn::Assistant,
            preview: name.to_string(), tokens: est(name).max(1),
            in_context: false, keywords: kw,
            looks_back: false, pruned: false,
        });
        self.head = Some(id);
        uuid
    }

    /// Resolve a stable uuid back to its node (how a named/referenced image is looked up).
    /// A PATCH node: a diff the assistant produced, attached to the turn that produced it.
    ///
    /// `applied` distinguishes a proposed patch from a landed one, because "the model suggested a change"
    /// and "the file on disk changed" are very different facts to see in a history.
    pub fn push_patch(&mut self, path: &str, hunks: usize, applied: bool, parent: Option<usize>,
                      est: impl Fn(&str) -> usize) -> u128 {
        let preview = format!("{}{} · {} hunk{}", if applied { "" } else { "proposed " },
            path, hunks, if hunks == 1 { "" } else { "s" });
        self.push_artifact(Kind::Patch, &preview, path, parent, est)
    }

    /// A FILE node: something written to the workspace.
    pub fn push_file(&mut self, path: &str, bytes: u64, parent: Option<usize>,
                     est: impl Fn(&str) -> usize) -> u128 {
        let preview = format!("{path} · {}", human_bytes(bytes));
        self.push_artifact(Kind::File, &preview, path, parent, est)
    }

    /// Shared construction for artifact nodes.
    ///
    /// Artifacts are ASSISTANT-side and start OUT of context: a produced file is not automatically part of
    /// what the model is looking at, and marking it in-context would overstate the window. Keywords come
    /// from the path so retrieval can find "that patch to backend.rs" the same way it finds a message.
    fn push_artifact(&mut self, kind: Kind, preview: &str, path: &str, parent: Option<usize>,
                     est: impl Fn(&str) -> usize) -> u128 {
        let id = self.nodes.len();
        let uuid = gen_uuid(preview, path, id as u64);
        let keywords = path.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2).map(|w| w.to_ascii_lowercase()).collect();
        self.nodes.push(Node {
            id, uuid, kind, refers: None, parent: parent.or(self.head), turn: Turn::Assistant,
            preview: preview.to_string(), tokens: est(preview), in_context: false,
            keywords, looks_back: false, pruned: false,
        });
        self.head = Some(id);
        uuid
    }

    /// Artifact nodes in order, for a filtered view ("show me what this conversation produced").
    pub fn artifacts(&self) -> Vec<&Node> {
        self.nodes.iter().filter(|n| n.kind.is_artifact()).collect()
    }

    pub fn node_by_uuid(&self, uuid: u128) -> Option<&Node> { self.nodes.iter().find(|n| n.uuid == uuid) }

    /// Every image node's (uuid, name), newest last — for the payload side-table / a picker.
    pub fn image_nodes(&self) -> Vec<(u128, &str)> {
        self.nodes.iter().filter(|n| n.kind == Kind::Image).map(|n| (n.uuid, n.preview.as_str())).collect()
    }

    /// Append a SHOT node that animates the image `keyframe_uuid` with `motion_prompt`, returning its uuid.
    /// This is the video layer of the hybrid: an image owns a shot's LOOK, the shot (rendered later by an
    /// image-to-video model) owns its MOTION. The shot's matchable surface is the motion prompt PLUS the
    /// keyframe's own keywords, so "the shot where the locket spins" is retrieved by the same overlap
    /// machinery. `refers` records the keyframe edge; the rendered clip is stored in the side-table by uuid.
    pub fn push_shot(&mut self, keyframe_uuid: u128, motion_prompt: &str, parent: Option<usize>, est: impl Fn(&str) -> usize) -> u128 {
        let id = self.nodes.len();
        let parent = parent.or(self.head).filter(|&p| p < id);
        let uuid = gen_uuid(motion_prompt, "shot", id as u64);
        let mut kw = keywords(motion_prompt);
        if let Some(kf) = self.node_by_uuid(keyframe_uuid) {      // inherit the keyframe's subject terms
            for w in kf.keywords.clone() { if !kw.contains(&w) { kw.push(w); } }
        }
        self.nodes.push(Node {
            id, uuid, kind: Kind::Shot, refers: Some(keyframe_uuid), parent, turn: Turn::Assistant,
            preview: motion_prompt.to_string(), tokens: est(motion_prompt).max(1),
            in_context: false, keywords: kw,
            looks_back: false, pruned: false,
        });
        self.head = Some(id);
        uuid
    }

    /// The keyframe an image-edit / shot node derives from, if any (follows the `refers` edge).
    pub fn keyframe_of(&self, uuid: u128) -> Option<u128> { self.node_by_uuid(uuid).and_then(|n| n.refers) }

    /// The render TIMELINE: shot node ids along the active branch in play order (root→head). Concatenate the
    /// clips in this order and you have the movie; a cut falls between two shots that sit in different topic
    /// segments (see [`cuts`]).
    pub fn timeline(&self) -> Vec<usize> {
        self.active_path().into_iter().filter(|&id| self.nodes[id].kind == Kind::Shot).collect()
    }

    /// Hard-CUT boundaries within the [`timeline`]: the index (into the timeline vec) AFTER which the next
    /// shot begins a new topic segment — a scene change, where a straight cut reads best and no cross-shot
    /// interpolation should ever be attempted. Reuses the same segmentation as context compaction.
    pub fn cuts(&self) -> Vec<usize> {
        let tl = self.timeline();
        let mut out = Vec::new();
        for w in 1..tl.len() {
            if Self::overlap(&self.nodes[tl[w]].keywords, &self.nodes[tl[w - 1]].keywords) == 0 {
                out.push(w - 1); // a segment break between timeline[w-1] and timeline[w]
            }
        }
        out
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
mod artifact_tests {
    use super::*;
    fn est(s: &str) -> usize { s.len() / 4 + 1 }

    #[test]
    fn artifacts_attach_to_the_turn_that_produced_them() {
        let mut g = Graph::default();
        g.push_turn(Turn::User, "fix the loader", None, est);
        let a = g.push_turn(Turn::Assistant, "here is a patch", None, est);
        let ai = g.node_by_uuid(a).unwrap().id;
        g.push_patch("src/lib.rs", 3, true, None, est);
        let p = g.nodes.last().unwrap();
        assert_eq!(p.kind, Kind::Patch);
        assert_eq!(p.parent, Some(ai), "a patch hangs off the turn that produced it");
        assert_eq!(p.turn, Turn::Assistant);
        assert!(p.preview.contains("3 hunks") && p.preview.contains("src/lib.rs"));
    }

    #[test]
    fn a_proposed_patch_reads_differently_from_an_applied_one() {
        // "the model suggested a change" and "the file on disk changed" are different facts, and a
        // history that renders them identically is misleading about what actually happened.
        let mut g = Graph::default();
        g.push_patch("a.rs", 1, true, None, est);
        g.push_patch("b.rs", 1, false, None, est);
        assert!(!g.nodes[0].preview.starts_with("proposed"));
        assert!(g.nodes[1].preview.starts_with("proposed"));
        assert!(g.nodes[0].preview.contains("1 hunk") && !g.nodes[0].preview.contains("hunks"));
    }

    #[test]
    fn artifacts_start_out_of_context() {
        // A produced file is not automatically in the model's window; marking it in-context would
        // overstate what the model is looking at.
        let mut g = Graph::default();
        g.push_file("out/report.md", 4096, None, est);
        assert!(!g.nodes[0].in_context);
        assert!(g.nodes[0].preview.contains("4.0 KiB"));
    }

    #[test]
    fn the_graph_has_THREE_categories_not_two() {
        // Liaison narration is neither a turn (nobody said it) nor an artifact (nothing was produced),
        // and fan/merge are neither — they mark a change in the timeline's SHAPE. With only
        // turn-vs-artifact, all three rendered as ordinary nodes and a fan-out looked exactly like three
        // consecutive replies.
        assert!(Kind::Liaison.is_narration());
        assert!(!Kind::Liaison.is_artifact() && !Kind::Liaison.is_structural());
        for k in [Kind::FanOut, Kind::Merge] {
            assert!(k.is_structural(), "{k:?} marks shape, not content");
            assert!(!k.is_artifact() && !k.is_narration());
        }
        assert!(!Kind::Text.is_narration() && !Kind::Text.is_structural());
        // every non-text kind carries a distinct badge, so they are separable without colour
        let g: Vec<&str> = [Kind::Image, Kind::Shot, Kind::Patch, Kind::File, Kind::Liaison,
                            Kind::FanOut, Kind::Merge].iter().map(|k| k.glyph()).collect();
        let mut u = g.clone(); u.sort(); u.dedup();
        assert_eq!(u.len(), g.len(), "badges must be distinct: {g:?}");
    }

    #[test]
    fn artifact_kinds_are_distinguishable_from_turns() {
        assert!(!Kind::Text.is_artifact());
        for k in [Kind::Patch, Kind::File, Kind::Image, Kind::Shot] {
            assert!(k.is_artifact(), "{k:?} is an artifact");
            assert!(!k.glyph().is_empty(), "{k:?} needs a badge glyph");
        }
        assert!(Kind::Text.glyph().is_empty(), "a plain message carries no badge");
    }

    #[test]
    fn artifacts_are_retrievable_by_path_words() {
        // Retrieval treats every node as a keyword bag; an artifact whose path contributed nothing would
        // be unfindable by the name the user actually refers to it by.
        let mut g = Graph::default();
        g.push_patch("crates/logscope-egui/src/backend.rs", 2, true, None, est);
        let kw = &g.nodes[0].keywords;
        assert!(kw.iter().any(|w| w == "backend"), "{kw:?}");
        assert!(kw.iter().any(|w| w == "logscope"), "{kw:?}");
        assert!(!kw.iter().any(|w| w == "rs"), "two-letter noise is dropped: {kw:?}");
    }

    #[test]
    fn the_artifact_filter_returns_only_produced_things() {
        let mut g = Graph::default();
        g.push_turn(Turn::User, "hello", None, est);
        g.push_turn(Turn::Assistant, "hi", None, est);
        g.push_patch("x.rs", 1, true, None, est);
        g.push_file("y.md", 10, None, est);
        let a = g.artifacts();
        assert_eq!(a.len(), 2);
        assert!(a.iter().all(|n| n.kind.is_artifact()));
    }

    #[test]
    fn byte_sizes_stay_readable_across_magnitudes() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(4096), "4.0 KiB");
        assert!(human_bytes(5 << 20).starts_with("5.0 MiB"));
        assert!(human_bytes(3 << 30).starts_with("3.00 GiB"));
    }
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

    #[test]
    fn image_node_is_pulled_up_by_name_like_text() {
        // an image enters the chat with a name; many unrelated turns pass; then a later turn references it
        // by name with a back-cue — and the SAME keyword-overlap retrieval pulls the image node forward.
        let mut g = Graph::spine(
            [(Turn::User, "let's talk about database indexing"),
             (Turn::Assistant, "b-trees keep lookups logarithmic")].into_iter(),
            |t| t.split_whitespace().count());
        let img = g.push_image("spiral galaxy over mountains", "a photoreal spiral galaxy above jagged peaks at night", None, |t| t.split_whitespace().count());
        // unrelated churn so the image falls out of the recency window
        g.push_turn(Turn::User, "now explain rust lifetimes", None, |t| t.split_whitespace().count());
        g.push_turn(Turn::Assistant, "a lifetime bounds how long a borrow is valid", None, |t| t.split_whitespace().count());
        // the referencing turn: a back-cue ("remember") + the image's name keyword ("galaxy")
        g.push_turn(Turn::User, "remember that galaxy image? bring it back", None, |t| t.split_whitespace().count());

        let img_id = g.node_by_uuid(img).unwrap().id;
        let pins = g.back_reference_pins(3);
        assert!(pins.contains(&img_id), "the image node should be pulled up by name overlap, got {pins:?}");
        // it's registered as an image, discoverable by uuid + in the image list
        assert_eq!(g.node_by_uuid(img).unwrap().kind, Kind::Image);
        assert!(g.image_nodes().iter().any(|&(u, name)| u == img && name == "spiral galaxy over mountains"));
    }

    #[test]
    fn uuids_are_stable_and_distinct() {
        let mut g = Graph::spine([(Turn::User, "hello")].into_iter(), |_| 1);
        let a = g.push_image("cat", "a cat", None, |_| 1);
        let b = g.push_image("cat", "a cat", None, |_| 1); // same content, different node → different uuid
        assert_ne!(a, b, "identical content at different indices must not collide");
        assert_eq!(g.node_by_uuid(a).unwrap().preview, "cat");
    }

    #[test]
    fn shots_animate_keyframes_and_assemble_into_a_timeline() {
        let words = |t: &str| t.split_whitespace().count();
        let mut g = Graph::spine([(Turn::User, "storyboard the opening")].into_iter(), words);
        // two scenes, each a keyframe + a shot that animates it
        let kf1 = g.push_image("harbor at dawn", "a quiet harbor at dawn, boats still", None, words);
        let s1 = g.push_shot(kf1, "slow push in on the harbor as gulls lift off", None, words);
        let kf2 = g.push_image("city street chase", "a crowded neon city street at night", None, words);
        let s2 = g.push_shot(kf2, "handheld chase down the city street", None, words);

        // the shot links back to its keyframe, and inherits the keyframe's subject terms for retrieval
        assert_eq!(g.keyframe_of(s1), Some(kf1));
        assert!(g.node_by_uuid(s1).unwrap().keywords.iter().any(|w| w == "harbor"));

        // the timeline is the shots in play order; a cut falls at the scene change (harbor → city)
        let tl = g.timeline();
        let (s1_id, s2_id) = (g.node_by_uuid(s1).unwrap().id, g.node_by_uuid(s2).unwrap().id);
        assert_eq!(tl, vec![s1_id, s2_id]);
        assert_eq!(g.cuts(), vec![0], "harbor shot and city shot share no keywords → a hard cut between them");

        // and a shot is still retrievable by the same fuzzy matcher: a later 'remember the harbor shot' pulls it
        g.push_turn(Turn::User, "remember that harbor shot? use it again", None, words);
        assert!(g.back_reference_pins(3).contains(&s1_id));
    }
}
