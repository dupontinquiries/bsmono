//! blackboard.rs — the swarm's shared scratchpad, so discoveries compound instead of each branch
//! re-discovering (or re-failing) in isolation.
//!
//! Right now every branch is a sealed lineage; a finding in one doesn't reach the others, so N branches
//! redundantly explore the same ground and repeat the same dead ends. The blackboard fixes that: a branch
//! POSTS what it learned (a fact, a dead end, a produced artifact, a promoted tool), and other branches
//! QUERY relevant entries or pull a DIGEST into their context. The two highest-value reads are `deadends()`
//! — so nobody wastes a cycle on an approach already known to fail — and tag-matched `query()`.
//!
//! Retrieval here is keyword/tag-based (pure + testable, works anywhere). The embedding-similarity version
//! layers on top via the existing Recall lattice when wired — same interface, richer matching.

/// A single shared finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    Fact(String),                       // something established true (e.g. "the 70B needs q4 to fit")
    Deadend(String),                    // an approach that FAILED — post it so others skip it
    Artifact { path: String, summary: String }, // a file produced (points into the workspace)
    Tool(String),                       // a tool promoted to the registry (its name)
}
impl Finding {
    pub fn text(&self) -> &str {
        match self {
            Finding::Fact(s) | Finding::Deadend(s) | Finding::Tool(s) => s,
            Finding::Artifact { summary, .. } => summary,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self { Finding::Fact(_) => "fact", Finding::Deadend(_) => "deadend", Finding::Artifact { .. } => "artifact", Finding::Tool(_) => "tool" }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub branch: String,    // which branch posted it
    pub finding: Finding,
    pub tags: Vec<String>, // lowercased keywords for retrieval
    pub tick: u64,         // monotonic post order (a branch remembers the last tick it read)
}

#[derive(Default, Clone, Debug)]
pub struct Blackboard {
    entries: Vec<Entry>,
    next_id: u64,
    clock: u64,
}

impl Blackboard {
    pub fn new() -> Self { Self::default() }

    /// Post a finding from `branch`, tagged with keywords (lowercased). Returns the entry id.
    pub fn post(&mut self, branch: impl Into<String>, finding: Finding, tags: &[&str]) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.clock += 1;
        self.entries.push(Entry {
            id, branch: branch.into(), finding,
            tags: tags.iter().map(|t| t.to_ascii_lowercase()).collect(),
            tick: self.clock,
        });
        id
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn now(&self) -> u64 { self.clock }

    /// Entries matching ANY of `tags` (or matching in the finding text), most recent first, capped at
    /// `limit`. The retrieval a branch runs to pull in what's already known about its subproblem.
    /// All entries in post order (for observability/telemetry — the swarm view renders findings as nodes).
    pub fn entries(&self) -> &[Entry] { &self.entries }

    pub fn query(&self, tags: &[&str], limit: usize) -> Vec<&Entry> {
        let needles: Vec<String> = tags.iter().map(|t| t.to_ascii_lowercase()).collect();
        let mut hits: Vec<&Entry> = self.entries.iter().filter(|e| {
            needles.iter().any(|n| e.tags.iter().any(|t| t == n) || e.finding.text().to_ascii_lowercase().contains(n.as_str()))
        }).collect();
        hits.sort_by(|a, b| b.tick.cmp(&a.tick)); // newest first
        hits.truncate(limit);
        hits
    }

    /// Every known dead end — the highest-value read: a branch checks this before committing to an approach
    /// so the swarm never pays twice for the same failure.
    pub fn deadends(&self) -> Vec<&str> {
        self.entries.iter().filter_map(|e| match &e.finding { Finding::Deadend(s) => Some(s.as_str()), _ => None }).collect()
    }

    /// New entries a branch hasn't seen (posted after `last_tick`), oldest first — the incremental sync a
    /// long-running branch polls so it picks up siblings' discoveries mid-flight.
    pub fn since(&self, last_tick: u64) -> Vec<&Entry> {
        let mut new: Vec<&Entry> = self.entries.iter().filter(|e| e.tick > last_tick).collect();
        new.sort_by_key(|e| e.tick);
        new
    }

    /// A compact, context-injectable summary of shared knowledge (facts + dead ends first — the actionable
    /// bits), capped at `limit` lines. This is what gets prepended to a branch's prompt so it stands on the
    /// swarm's accumulated findings instead of starting cold.
    pub fn digest(&self, limit: usize) -> String {
        if self.entries.is_empty() { return "(shared board empty)".into(); }
        let mut ordered: Vec<&Entry> = self.entries.iter().collect();
        // dead ends and facts are the most actionable; surface them ahead of artifacts/tools, newest first
        ordered.sort_by(|a, b| kind_rank(&a.finding).cmp(&kind_rank(&b.finding)).then(b.tick.cmp(&a.tick)));
        ordered.iter().take(limit)
            .map(|e| format!("[{}] {}: {}", e.finding.kind(), e.branch, e.finding.text()))
            .collect::<Vec<_>>().join("\n")
    }
}

fn kind_rank(f: &Finding) -> u8 {
    match f { Finding::Deadend(_) => 0, Finding::Fact(_) => 1, Finding::Tool(_) => 2, Finding::Artifact { .. } => 3 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_and_query_by_tag_and_text() {
        let mut bb = Blackboard::new();
        bb.post("branch-a", Finding::Fact("the 70B fits at q4 but not int8".into()), &["70b", "quant", "memory"]);
        bb.post("branch-b", Finding::Fact("naga validates WGSL in-process".into()), &["wgsl", "validation"]);
        // tag match
        let q = bb.query(&["quant"], 5);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].branch, "branch-a");
        // text match (no such tag, but the word is in the finding)
        assert_eq!(bb.query(&["int8"], 5).len(), 1);
        // no match
        assert!(bb.query(&["zig"], 5).is_empty());
    }

    #[test]
    fn deadends_prevent_repeated_failures() {
        let mut bb = Blackboard::new();
        bb.post("a", Finding::Deadend("uploading int8 weights OOMs the driver".into()), &["70b", "upload"]);
        bb.post("b", Finding::Fact("q4 residency works".into()), &["70b"]);
        bb.post("c", Finding::Deadend("streaming below 2 layers thrashes".into()), &["streaming"]);
        let dead = bb.deadends();
        assert_eq!(dead.len(), 2);
        assert!(dead.iter().any(|d| d.contains("OOMs")));
        assert!(dead.iter().any(|d| d.contains("thrashes")));
    }

    #[test]
    fn since_gives_incremental_sync() {
        let mut bb = Blackboard::new();
        bb.post("a", Finding::Fact("one".into()), &[]);
        let checkpoint = bb.now();
        bb.post("b", Finding::Fact("two".into()), &[]);
        bb.post("c", Finding::Fact("three".into()), &[]);
        let new = bb.since(checkpoint);
        assert_eq!(new.len(), 2, "only entries after the checkpoint");
        assert_eq!(new[0].finding.text(), "two"); // oldest-first
        assert_eq!(new[1].finding.text(), "three");
        assert!(bb.since(bb.now()).is_empty(), "nothing new after the latest tick");
    }

    #[test]
    fn digest_surfaces_actionable_findings_first() {
        let mut bb = Blackboard::new();
        bb.post("a", Finding::Artifact { path: "src/x.rs".into(), summary: "wrote x".into() }, &[]);
        bb.post("b", Finding::Deadend("approach Z fails".into()), &[]);
        bb.post("c", Finding::Fact("approach Y works".into()), &[]);
        let d = bb.digest(10);
        let dead_pos = d.find("deadend").unwrap();
        let art_pos = d.find("artifact").unwrap();
        assert!(dead_pos < art_pos, "dead ends surface before artifacts:\n{d}");
        assert!(d.contains("approach Z fails") && d.contains("approach Y works"));
    }
}
