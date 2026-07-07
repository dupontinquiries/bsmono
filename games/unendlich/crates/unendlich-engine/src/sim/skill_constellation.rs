//! sim::skill_constellation — the bending-discipline skill tree.
//!
//! From `character_panel_reference §4`: the character panel's left third is a
//! **constellation** of technique nodes in a branching tributary structure —
//! central nodes split into lateral branches, some gated by world conditions
//! ("Water + high-R", a season). The engine already has the individual
//! techniques (`sim::bending_*`); what's missing is the *tree* that says which
//! are unlocked, what each requires, and how spending points opens new branches.
//! This is that pure structure.
//!
//! Each node maps to a technique/perk by id; prerequisites and an optional
//! **environmental gate** (a caller-supplied condition, e.g. a season or a
//! resonance threshold from the world/time system) control availability. The
//! gate is the integration seam with the world's time-bending state: the caller
//! decides whether a node's condition holds this moment and passes it in.
//! Deterministic (BTreeMap/BTreeSet ordering), std-only.

use std::collections::{BTreeMap, BTreeSet};

/// Identifies a node (and the technique/perk it grants).
pub type NodeId = u32;

/// One constellation node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillNode {
    pub id: NodeId,
    /// All must be unlocked before this node becomes available.
    pub prerequisites: Vec<NodeId>,
    /// Skill points to unlock.
    pub cost: u32,
    /// If true, the node also needs a world/time condition (supplied at unlock
    /// time) — a season, a high-resonance environment, a cross-discipline state.
    pub environmental: bool,
}

/// Why an unlock attempt failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlockError {
    UnknownNode,
    AlreadyUnlocked,
    PrerequisitesNotMet,
    EnvironmentLocked,
    InsufficientPoints,
}

/// The tree plus which nodes the player has unlocked.
#[derive(Clone, Debug, Default)]
pub struct Constellation {
    nodes: BTreeMap<NodeId, SkillNode>,
    unlocked: BTreeSet<NodeId>,
}

impl Constellation {
    pub fn new() -> Constellation {
        Constellation { nodes: BTreeMap::new(), unlocked: BTreeSet::new() }
    }

    /// Define a node. Re-adding an id replaces its definition.
    pub fn add_node(&mut self, id: NodeId, prerequisites: Vec<NodeId>, cost: u32, environmental: bool) {
        self.nodes.insert(id, SkillNode { id, prerequisites, cost, environmental });
    }

    pub fn get(&self, id: NodeId) -> Option<&SkillNode> {
        self.nodes.get(&id)
    }
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn is_unlocked(&self, id: NodeId) -> bool {
        self.unlocked.contains(&id)
    }

    /// Are all of `id`'s prerequisites unlocked? (True for a root node with no
    /// prerequisites.)
    pub fn prerequisites_met(&self, id: NodeId) -> bool {
        match self.nodes.get(&id) {
            Some(n) => n.prerequisites.iter().all(|p| self.unlocked.contains(p)),
            None => false,
        }
    }

    /// Can `id` be unlocked right now? Exists, not already unlocked, prerequisites
    /// met, and — if it's environmental — the caller's `env_ok` condition holds.
    pub fn is_available(&self, id: NodeId, env_ok: bool) -> bool {
        match self.nodes.get(&id) {
            Some(n) => {
                !self.unlocked.contains(&id)
                    && self.prerequisites_met(id)
                    && (!n.environmental || env_ok)
            }
            None => false,
        }
    }

    /// Attempt to unlock `id`, spending points. `env_ok` is the world/time
    /// condition for environmental nodes (ignored for non-environmental ones).
    /// On success returns the points spent.
    pub fn unlock(&mut self, id: NodeId, points_available: u32, env_ok: bool) -> Result<u32, UnlockError> {
        let node = self.nodes.get(&id).ok_or(UnlockError::UnknownNode)?;
        if self.unlocked.contains(&id) {
            return Err(UnlockError::AlreadyUnlocked);
        }
        if !self.prerequisites_met(id) {
            return Err(UnlockError::PrerequisitesNotMet);
        }
        if node.environmental && !env_ok {
            return Err(UnlockError::EnvironmentLocked);
        }
        if points_available < node.cost {
            return Err(UnlockError::InsufficientPoints);
        }
        let cost = node.cost;
        self.unlocked.insert(id);
        Ok(cost)
    }

    /// Total points spent on unlocked nodes.
    pub fn total_spent(&self) -> u32 {
        self.unlocked.iter().filter_map(|id| self.nodes.get(id)).map(|n| n.cost).sum()
    }

    pub fn unlocked_count(&self) -> usize {
        self.unlocked.len()
    }

    /// Respec: forget all unlocks (refunds are the caller's concern — see
    /// [`total_spent`] before calling).
    pub fn reset(&mut self) {
        self.unlocked.clear();
    }

    /// Could `id` ever be unlocked in principle — every transitive prerequisite
    /// exists and there's no dependency cycle reaching it? (Ignores points and
    /// the environmental gate; a pure structural check.)
    pub fn can_eventually_reach(&self, id: NodeId) -> bool {
        let mut seen = BTreeSet::new();
        self.reachable(id, &mut seen)
    }

    fn reachable(&self, id: NodeId, seen: &mut BTreeSet<NodeId>) -> bool {
        if !seen.insert(id) {
            return false; // cycle
        }
        let ok = match self.nodes.get(&id) {
            Some(n) => n.prerequisites.iter().all(|&p| self.reachable(p, seen)),
            None => false,
        };
        seen.remove(&id);
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A small delta tree: 1 (root) → 2, 3; {2,3} → 4 (diamond); 5 is env-gated off 2.
    fn tree() -> Constellation {
        let mut c = Constellation::new();
        c.add_node(1, vec![], 1, false);
        c.add_node(2, vec![1], 2, false);
        c.add_node(3, vec![1], 2, false);
        c.add_node(4, vec![2, 3], 3, false);
        c.add_node(5, vec![2], 2, true); // environmental
        c
    }

    #[test]
    fn a_root_node_is_available_and_children_are_not() {
        let c = tree();
        assert!(c.is_available(1, false), "root has no prereqs");
        assert!(!c.is_available(2, false), "child locked until root unlocked");
        assert!(!c.is_available(99, false), "unknown node");
    }

    #[test]
    fn unlocking_opens_the_next_branch() {
        let mut c = tree();
        assert_eq!(c.unlock(1, 10, false), Ok(1));
        assert!(c.is_unlocked(1));
        assert!(c.is_available(2, false), "root unlocked ⇒ child available");
        assert!(c.is_available(3, false));
    }

    #[test]
    fn a_diamond_requires_both_prerequisites() {
        let mut c = tree();
        c.unlock(1, 10, false).unwrap();
        c.unlock(2, 10, false).unwrap();
        assert!(!c.is_available(4, false), "needs 3 as well");
        assert_eq!(c.unlock(4, 10, false), Err(UnlockError::PrerequisitesNotMet));
        c.unlock(3, 10, false).unwrap();
        assert!(c.is_available(4, false));
        assert_eq!(c.unlock(4, 10, false), Ok(3));
    }

    #[test]
    fn environmental_nodes_need_the_world_condition() {
        let mut c = tree();
        c.unlock(1, 10, false).unwrap();
        c.unlock(2, 10, false).unwrap();
        assert!(!c.is_available(5, false), "env gate closed");
        assert_eq!(c.unlock(5, 10, false), Err(UnlockError::EnvironmentLocked));
        assert!(c.is_available(5, true), "env gate open (e.g. right season)");
        assert_eq!(c.unlock(5, 10, true), Ok(2));
    }

    #[test]
    fn insufficient_points_blocks_unlock() {
        let mut c = tree();
        assert_eq!(c.unlock(1, 0, false), Err(UnlockError::InsufficientPoints));
        c.unlock(1, 1, false).unwrap();
        assert_eq!(c.unlock(2, 1, false), Err(UnlockError::InsufficientPoints), "node 2 costs 2");
    }

    #[test]
    fn double_unlock_and_unknown_are_errors() {
        let mut c = tree();
        c.unlock(1, 10, false).unwrap();
        assert_eq!(c.unlock(1, 10, false), Err(UnlockError::AlreadyUnlocked));
        assert_eq!(c.unlock(42, 10, false), Err(UnlockError::UnknownNode));
    }

    #[test]
    fn spent_and_reset_track_the_loadout() {
        let mut c = tree();
        c.unlock(1, 10, false).unwrap();
        c.unlock(2, 10, false).unwrap();
        assert_eq!(c.total_spent(), 3); // 1 + 2
        assert_eq!(c.unlocked_count(), 2);
        c.reset();
        assert_eq!(c.total_spent(), 0);
        assert_eq!(c.unlocked_count(), 0);
        assert!(!c.is_available(2, false), "back to needing the root");
    }

    #[test]
    fn reachability_detects_missing_prereqs_and_cycles() {
        let mut c = Constellation::new();
        c.add_node(10, vec![11], 1, false); // 11 doesn't exist
        assert!(!c.can_eventually_reach(10), "missing prerequisite ⇒ unreachable");

        let mut d = Constellation::new();
        d.add_node(20, vec![21], 1, false);
        d.add_node(21, vec![20], 1, false); // cycle
        assert!(!d.can_eventually_reach(20));

        assert!(tree().can_eventually_reach(4), "the diamond is reachable");
    }

    #[test]
    fn structure_is_deterministic() {
        let mut a = tree();
        let mut b = tree();
        a.unlock(1, 10, false).unwrap();
        b.unlock(1, 10, false).unwrap();
        assert_eq!(a.total_spent(), b.total_spent());
        assert_eq!(a.is_available(2, false), b.is_available(2, false));
    }
}
