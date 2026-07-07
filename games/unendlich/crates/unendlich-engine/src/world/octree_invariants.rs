//! `octree_invariants` — audit checker: octree is well-formed AND minimal.
//!
//! Worker-03 / WORLD pure core, round 13 (test-buildout). A pure validator for
//! octree nodes: every branch's subtrees are themselves valid, and no branch is
//! "non-minimal" — i.e. eight uniform leaves that should have been collapsed by
//! `octree_remerge`. Used in tests/audits to catch edit paths that grow nodes
//! without re-merging.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// A minimal owned octree node (mirrors `octree_remerge::Node`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Leaf(u16),
    Branch([Box<Node>; 8]),
}

/// Check that `n` is well-formed and minimal. `Err(reason)` on the first
/// violation found (deterministic: children are checked in index order before
/// the branch's own minimality).
pub fn check(n: &Node) -> Result<(), &'static str> {
    match n {
        Node::Leaf(_) => Ok(()),
        Node::Branch(children) => {
            for c in children.iter() {
                check(c)?;
            }
            if let Some(v0) = leaf_val(&children[0]) {
                if children.iter().all(|c| leaf_val(c) == Some(v0)) {
                    return Err("non-minimal: 8 uniform leaves should be merged");
                }
            }
            Ok(())
        }
    }
}

fn leaf_val(n: &Node) -> Option<u16> {
    match n {
        Node::Leaf(v) => Some(*v),
        Node::Branch(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(v: u16) -> Box<Node> {
        Box::new(Node::Leaf(v))
    }

    fn branch(vs: [u16; 8]) -> Node {
        Node::Branch([
            leaf(vs[0]),
            leaf(vs[1]),
            leaf(vs[2]),
            leaf(vs[3]),
            leaf(vs[4]),
            leaf(vs[5]),
            leaf(vs[6]),
            leaf(vs[7]),
        ])
    }

    #[test]
    fn single_leaf_passes() {
        assert!(check(&Node::Leaf(3)).is_ok());
    }

    #[test]
    fn valid_mixed_branch_passes() {
        assert!(check(&branch([1, 2, 1, 2, 1, 2, 1, 2])).is_ok());
    }

    #[test]
    fn eight_uniform_leaves_flagged() {
        let r = check(&branch([5, 5, 5, 5, 5, 5, 5, 5]));
        assert!(r.is_err());
    }

    #[test]
    fn well_formed_nesting_passes() {
        // A nested non-uniform branch inside a mixed parent.
        let inner = Box::new(branch([1, 2, 3, 4, 5, 6, 7, 8]));
        let parent = Node::Branch([
            inner,
            leaf(0),
            leaf(1),
            leaf(0),
            leaf(1),
            leaf(0),
            leaf(1),
            leaf(0),
        ]);
        assert!(check(&parent).is_ok());
    }

    #[test]
    fn nested_uniform_branch_is_flagged() {
        // Inner branch is 8 uniform leaves → the audit catches it on recursion.
        let inner = Box::new(branch([9, 9, 9, 9, 9, 9, 9, 9]));
        let parent = Node::Branch([
            inner,
            leaf(0),
            leaf(1),
            leaf(0),
            leaf(1),
            leaf(0),
            leaf(1),
            leaf(0),
        ]);
        assert!(check(&parent).is_err());
    }

    #[test]
    fn deep_valid_tree_passes() {
        // Depth-2 tree where each branch is non-uniform.
        let mk = || Box::new(branch([1, 2, 3, 4, 5, 6, 7, 8]));
        let parent = Node::Branch([mk(), mk(), mk(), mk(), mk(), mk(), mk(), mk()]);
        assert!(check(&parent).is_ok());
    }

    #[test]
    fn deterministic() {
        let t = branch([1, 1, 1, 1, 1, 1, 1, 1]);
        assert_eq!(check(&t), check(&t));
        let ok = branch([1, 2, 1, 2, 1, 2, 1, 2]);
        assert_eq!(check(&ok), check(&ok));
    }
}
