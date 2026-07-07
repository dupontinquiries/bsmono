//! ws_registry.rs — bind temporary workspaces to conversation-graph nodes, and manage their lifecycle.
//!
//! `workspace.rs` turns a model reply into files in a temp dir; `convograph.rs` is the durable turn graph.
//! Nothing connected them: workspaces were global and orphaned, with no cleanup and no way to ask "what
//! files does THIS branch have?". This registry keys each workspace by the producing node's `uuid` (the same
//! side-table-by-uuid pattern the graph uses for image pixels), tracks bytes + timestamps, evicts by TTL and
//! a total-bytes budget, and answers graph-relative queries (the workspaces along the active branch).
//!
//! Pure: GC and the path walk are deterministic given `now_ms`; the registry returns the dirs to delete
//! rather than touching the filesystem, so the policy is unit-tested and the IO is a thin caller step.

use crate::convograph::Graph;
use std::collections::HashMap;
use std::path::PathBuf;

/// One node's workspace: where its files live, what they are, how big, and when it was made / last touched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsEntry {
    pub uuid: u128,          // the convograph Node this workspace belongs to
    pub dir: PathBuf,        // the temp directory holding the files
    pub files: Vec<String>,  // artifact relpaths within `dir`
    pub bytes: u64,          // total on-disk size
    pub created_ms: u64,
    pub last_used_ms: u64,
    pub pinned: bool,        // protected from all GC (TTL, budget, orphan) — a deliberate keep
}

#[derive(Default)]
pub struct WsRegistry {
    entries: HashMap<u128, WsEntry>,
}

impl WsRegistry {
    pub fn new() -> Self { Self { entries: HashMap::new() } }
    pub fn len(&self) -> usize { self.entries.len() }
    pub fn is_empty(&self) -> bool { self.entries.is_empty() }
    pub fn get(&self, uuid: u128) -> Option<&WsEntry> { self.entries.get(&uuid) }
    pub fn total_bytes(&self) -> u64 { self.entries.values().map(|e| e.bytes).sum() }
    /// Iterate all tracked workspaces (order unspecified). For listing/telemetry.
    pub fn iter(&self) -> impl Iterator<Item = &WsEntry> { self.entries.values() }

    /// Record (or replace) the workspace for `uuid`. Replacing returns the OLD dir to delete (a re-run of the
    /// same node supersedes its previous files), else `None`.
    pub fn register(&mut self, uuid: u128, dir: PathBuf, files: Vec<String>, bytes: u64, now_ms: u64) -> Option<PathBuf> {
        let superseded = self.entries.get(&uuid).map(|e| e.dir.clone()).filter(|d| *d != dir);
        let pinned = self.entries.get(&uuid).map(|e| e.pinned).unwrap_or(false); // a re-run keeps its pin
        self.entries.insert(uuid, WsEntry { uuid, dir, files, bytes, created_ms: now_ms, last_used_ms: now_ms, pinned });
        superseded
    }

    /// Pin/unpin a workspace: pinned survives TTL, budget, and orphan GC. No-op if absent.
    pub fn set_pinned(&mut self, uuid: u128, pinned: bool) {
        if let Some(e) = self.entries.get_mut(&uuid) { e.pinned = pinned; }
    }

    /// Reap workspaces whose node is no longer in the graph — i.e. the producing turn was DELETED (not merely
    /// pruned; pruned turns remain in the graph, so their uuids stay live). `live` is the set of uuids
    /// currently in the graph. Pinned entries are kept. Returns dirs to delete. This is the graph-coupled GC:
    /// the temp tree follows the conversation as turns come and go.
    pub fn gc_orphaned(&mut self, live: &std::collections::HashSet<u128>) -> Vec<PathBuf> {
        let orphans: Vec<u128> = self.entries.values()
            .filter(|e| !e.pinned && !live.contains(&e.uuid))
            .map(|e| e.uuid).collect();
        orphans.into_iter().filter_map(|u| self.entries.remove(&u).map(|e| e.dir)).collect()
    }

    /// Mark a workspace as used now (keeps it fresh against LRU eviction). No-op if absent.
    pub fn touch(&mut self, uuid: u128, now_ms: u64) {
        if let Some(e) = self.entries.get_mut(&uuid) { e.last_used_ms = now_ms; }
    }

    /// Evict workspaces past `ttl_ms` since last use, then — if still over `max_bytes` — the least-recently-
    /// used until under budget. Returns the directories to delete (the caller does the filesystem removal).
    /// A `ttl_ms` of 0 disables the age rule; a `max_bytes` of 0 disables the budget rule.
    pub fn gc(&mut self, now_ms: u64, ttl_ms: u64, max_bytes: u64) -> Vec<PathBuf> {
        let mut removed = Vec::new();
        if ttl_ms > 0 {
            let expired: Vec<u128> = self.entries.values()
                .filter(|e| !e.pinned && now_ms.saturating_sub(e.last_used_ms) > ttl_ms)
                .map(|e| e.uuid).collect();
            for u in expired { if let Some(e) = self.entries.remove(&u) { removed.push(e.dir); } }
        }
        if max_bytes > 0 {
            while self.total_bytes() > max_bytes {
                // evict the least-recently-used UNPINNED (oldest last_used_ms; tie-break smallest uuid)
                let victim = self.entries.values().filter(|e| !e.pinned)
                    .min_by(|a, b| a.last_used_ms.cmp(&b.last_used_ms).then(a.uuid.cmp(&b.uuid)))
                    .map(|e| e.uuid);
                match victim {
                    Some(u) => { if let Some(e) = self.entries.remove(&u) { removed.push(e.dir); } }
                    None => break, // only pinned entries remain
                }
            }
        }
        removed
    }

    /// The workspaces along the active branch from root to `leaf` (root first, leaf last) — the files "in
    /// scope" at this point in the conversation. Walks `Node::parent`, skipping nodes without a workspace.
    pub fn on_path(&self, graph: &Graph, leaf: usize) -> Vec<&WsEntry> {
        let mut chain: Vec<u128> = Vec::new();
        let mut cur = Some(leaf);
        let mut guard = 0;
        while let Some(id) = cur {
            let node = match graph.nodes.get(id) { Some(n) => n, None => break };
            chain.push(node.uuid);
            cur = node.parent;
            guard += 1;
            if guard > graph.nodes.len() + 1 { break; } // cycle guard
        }
        chain.reverse(); // root → leaf
        chain.into_iter().filter_map(|u| self.entries.get(&u)).collect()
    }

    /// The set of distinct file relpaths visible on the branch (later nodes shadow earlier ones with the same
    /// path — the newest version of a file wins, mirroring how the swarm assembles prior winners).
    pub fn files_in_scope(&self, graph: &Graph, leaf: usize) -> Vec<(String, PathBuf)> {
        let mut seen: HashMap<String, PathBuf> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for e in self.on_path(graph, leaf) {
            for f in &e.files {
                if !seen.contains_key(f) { order.push(f.clone()); }
                seen.insert(f.clone(), e.dir.join(f));
            }
        }
        order.into_iter().map(|f| { let p = seen.remove(&f).unwrap(); (f, p) }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convograph::{Graph, Turn};

    fn dir(s: &str) -> PathBuf { PathBuf::from(format!("/tmp/ws/{s}")) }

    #[test]
    fn register_get_and_total_bytes() {
        let mut r = WsRegistry::new();
        assert!(r.register(1, dir("a"), vec!["lib.rs".into()], 100, 0).is_none());
        assert!(r.register(2, dir("b"), vec!["main.rs".into()], 250, 0).is_none());
        assert_eq!(r.total_bytes(), 350);
        assert_eq!(r.get(1).unwrap().files, vec!["lib.rs".to_string()]);
        // re-register same uuid with a new dir supersedes → returns old dir to delete
        assert_eq!(r.register(1, dir("a2"), vec!["lib.rs".into()], 120, 5), Some(dir("a")));
        assert_eq!(r.total_bytes(), 370);
    }

    #[test]
    fn gc_orphaned_reaps_nodes_absent_from_graph() {
        let mut r = WsRegistry::new();
        r.register(1, dir("live"), vec![], 10, 0);
        r.register(2, dir("gone"), vec![], 10, 0);
        let live: std::collections::HashSet<u128> = [1u128].into_iter().collect(); // node 2 deleted
        let reaped = r.gc_orphaned(&live);
        assert_eq!(reaped, vec![dir("gone")]);
        assert!(r.get(2).is_none() && r.get(1).is_some());
    }

    #[test]
    fn pinned_survives_orphan_ttl_and_budget() {
        let mut r = WsRegistry::new();
        r.register(1, dir("keep"), vec![], 500, 0);
        r.set_pinned(1, true);
        // orphaned (not in live) but pinned → kept
        assert!(r.gc_orphaned(&std::collections::HashSet::new()).is_empty());
        // expired by TTL but pinned → kept
        assert!(r.gc(10_000, 1000, 0).is_empty());
        // over byte budget but only pinned remains → kept
        assert!(r.gc(0, 0, 100).is_empty());
        assert!(r.get(1).is_some(), "pinned workspace survives every GC");
    }

    #[test]
    fn pin_persists_across_reregister() {
        let mut r = WsRegistry::new();
        r.register(1, dir("a"), vec![], 10, 0);
        r.set_pinned(1, true);
        r.register(1, dir("a2"), vec![], 20, 5); // re-run of the same node
        assert!(r.get(1).unwrap().pinned, "re-registering keeps the pin");
    }

    #[test]
    fn gc_evicts_by_ttl() {
        let mut r = WsRegistry::new();
        r.register(1, dir("a"), vec![], 10, 0);
        r.register(2, dir("b"), vec![], 10, 900);
        // at t=1000, ttl=500: uuid1 (last_used 0, age 1000) expired; uuid2 (age 100) survives
        let removed = r.gc(1000, 500, 0);
        assert_eq!(removed, vec![dir("a")]);
        assert!(r.get(1).is_none() && r.get(2).is_some());
    }

    #[test]
    fn gc_evicts_lru_over_byte_budget() {
        let mut r = WsRegistry::new();
        r.register(1, dir("a"), vec![], 100, 0);   // oldest use
        r.register(2, dir("b"), vec![], 100, 10);
        r.register(3, dir("c"), vec![], 100, 20);  // newest use
        r.touch(1, 30);                            // now uuid1 is the MOST recent → should survive
        // budget 250: total 300 → evict LRU until ≤250. LRU now = uuid2 (10). evict b → 200 ≤ 250, stop.
        let removed = r.gc(100, 0, 250);
        assert_eq!(removed, vec![dir("b")]);
        assert_eq!(r.total_bytes(), 200);
        assert!(r.get(1).is_some() && r.get(3).is_some());
    }

    #[test]
    fn on_path_walks_the_active_branch() {
        // graph: 0(root) → 1 → 2 ; and a fork 0 → 3. Leaf 2's path = [0,1,2]; leaf 3's = [0,3].
        let g = Graph::from_tree(
            [(Turn::User, "root", None), (Turn::Assistant, "a", Some(0)), (Turn::User, "b", Some(1)), (Turn::Assistant, "c", Some(0))],
            |s| s.len(), Some(2));
        let mut r = WsRegistry::new();
        // give nodes 0 and 2 workspaces (node 1 has none → skipped)
        r.register(g.nodes[0].uuid, dir("n0"), vec!["root.rs".into()], 1, 0);
        r.register(g.nodes[2].uuid, dir("n2"), vec!["leaf.rs".into()], 1, 0);
        r.register(g.nodes[3].uuid, dir("n3"), vec!["fork.rs".into()], 1, 0);

        let on2: Vec<PathBuf> = r.on_path(&g, 2).iter().map(|e| e.dir.clone()).collect();
        assert_eq!(on2, vec![dir("n0"), dir("n2")], "root→leaf, node1 (no ws) skipped");
        // the fork sees n0 + n3, not n2
        let on3: Vec<PathBuf> = r.on_path(&g, 3).iter().map(|e| e.dir.clone()).collect();
        assert_eq!(on3, vec![dir("n0"), dir("n3")]);
    }

    #[test]
    fn files_in_scope_shadows_by_newest() {
        // node0 writes config.toml; node2 (deeper) rewrites config.toml + adds new.rs → newest config wins
        let g = Graph::from_tree(
            [(Turn::User, "r", None), (Turn::Assistant, "a", Some(0)), (Turn::Assistant, "b", Some(1))],
            |s| s.len(), Some(2));
        let mut r = WsRegistry::new();
        r.register(g.nodes[0].uuid, dir("n0"), vec!["config.toml".into()], 1, 0);
        r.register(g.nodes[2].uuid, dir("n2"), vec!["config.toml".into(), "new.rs".into()], 1, 0);
        let scope = r.files_in_scope(&g, 2);
        // config.toml resolves to the DEEPER node's copy; both distinct paths present
        let map: HashMap<_, _> = scope.into_iter().collect();
        assert_eq!(map.get("config.toml"), Some(&dir("n2").join("config.toml")), "newest version shadows");
        assert_eq!(map.get("new.rs"), Some(&dir("n2").join("new.rs")));
        assert_eq!(map.len(), 2);
    }
}
