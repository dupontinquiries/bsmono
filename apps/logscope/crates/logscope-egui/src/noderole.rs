//! noderole — what a node IS, in one word, for the label above its text.
//!
//! ## Read off the reference
//!
//! Every node card in the mansion-garden and atlantis references carries a TYPE above its content —
//! ROOT PROMPT, HYPOTHESES, EVIDENCE, CORRELATED, INSIGHTS, CONCLUSION — and the content line
//! underneath. Our labels are the content line alone. The type is what makes a graph readable at a
//! glance: you can see the shape of an investigation without reading any of it.
//!
//! ## Derived, not invented
//!
//! The reference's exact vocabulary belongs to a worked example, not to this data. Inventing
//! "HYPOTHESES" and "CORRELATED" would mean classifying prose by guessing at its rhetorical
//! function, which is a model's job and an unreliable one.
//!
//! What IS available is the graph itself, and it carries more than it shows: who spoke, whether a
//! node forks, whether it is a leaf, whether it refers back to something earlier, whether it was
//! pruned out of context, and what kind of artifact it is. Every role below is a fact about the
//! node, so a label can never be wrong about it — the worst case is that it is uninformative.
//!
//! ## Precedence is the whole design
//!
//! A node is usually several of these at once: a pruned fork that also looks back. The order below
//! is what a reader needs to know FIRST. Pruned outranks everything because a pruned node is not in
//! the model's context and nothing else about it matters as much; a fork outranks a plain turn
//! because it is where the conversation's shape changed.

use crate::convograph::{Graph, Kind, Node, Turn};

/// One word for what a node is. Short by construction: this sits above a label that is already
/// tight, and a role that needs eight characters to say "assistant" has spent them badly.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// Human-pruned: kept in the log, removed from the model's context.
    Pruned,
    /// The conversation's first turn.
    Root,
    /// A turn with more than one child — where the conversation forked.
    Fork,
    /// Refers back to something earlier ("undo", "the first version").
    Loopback,
    /// An artifact rather than a turn: a patch, a file, an image, a shot.
    Artifact,
    /// The backend narrating itself — nobody said it and nothing was produced.
    Liaison,
    /// A fan-out zone: one task sent to several models.
    Fan,
    /// The end of a branch.
    Leaf,
    /// A person's turn.
    Ask,
    /// A model's turn.
    Reply,
}

impl Role {
    /// The word shown above the label. Upper case to match the reference's card headers, and
    /// because a lower-case tag beside lower-case content does not read as a category.
    pub fn label(self) -> &'static str {
        match self {
            Role::Pruned => "PRUNED",
            Role::Root => "ROOT",
            Role::Fork => "FORK",
            Role::Loopback => "LOOPBACK",
            Role::Artifact => "ARTIFACT",
            Role::Liaison => "SYSTEM",
            Role::Fan => "FAN",
            Role::Leaf => "LEAF",
            Role::Ask => "ASK",
            Role::Reply => "REPLY",
        }
    }

    /// Whether this role is worth the space. `Reply` is the majority of any conversation, so
    /// tagging every one of them adds noise and no information — the label is for what stands out.
    pub fn worth_showing(self) -> bool { !matches!(self, Role::Reply) }
}

/// Classify a node against its graph.
///
/// `children` is how many nodes name this one as parent — passed in rather than recomputed per node
/// so labelling a whole graph stays linear.
pub fn role_of(node: &Node, children: usize, is_root: bool) -> Role {
    // Precedence, most-load-bearing first. See the module note: a node is usually several of these.
    if node.pruned { return Role::Pruned; }
    match node.kind {
        Kind::Liaison => return Role::Liaison,
        Kind::Patch | Kind::File | Kind::Image | Kind::Shot => return Role::Artifact,
        _ => {}
    }
    if is_root { return Role::Root; }
    if children > 1 { return Role::Fork; }
    if node.looks_back { return Role::Loopback; }
    if children == 0 {
        return Role::Leaf;
    }
    match node.turn { Turn::User => Role::Ask, Turn::Assistant => Role::Reply }
}

/// Roles for a whole graph, indexed by node id.
///
/// One pass for the child counts, one for the classification. The root is the node with no parent
/// rather than id 0: a graph that has been pruned or rebuilt need not have its first node first.
pub fn roles(g: &Graph) -> Vec<Role> {
    let n = g.nodes.len();
    let mut children = vec![0usize; n];
    for nd in &g.nodes {
        if let Some(p) = nd.parent {
            if p < n { children[p] += 1; }
        }
    }
    g.nodes.iter().enumerate()
        .map(|(i, nd)| role_of(nd, children.get(i).copied().unwrap_or(0), nd.parent.is_none()))
        .collect()
}

/// The label line for a node: `ROLE · text`, or just the text when the role is not worth the space.
///
/// `max` is the whole line's budget, so the role is charged against the text rather than added to
/// it — a tag that pushes the content off the end has made the label worse.
pub fn line(role: Role, preview: &str, max: usize) -> String {
    if !role.worth_showing() {
        return crate::nodelabel::elide_words(preview, max);
    }
    let tag = role.label();
    // If the tag plus a separator would leave nothing to read, the text wins: the content is the
    // point and the role is the annotation.
    let room = max.saturating_sub(tag.chars().count() + 3);
    if room < 6 { return crate::nodelabel::elide_words(preview, max); }
    let body = crate::nodelabel::elide_words(preview, room);
    if body.is_empty() { tag.to_string() } else { format!("{tag} · {body}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convograph::{Graph, Kind, Turn};

    fn g_from(edges: &[(usize, usize)], n: usize) -> Graph {
        let mut g = Graph::default();
        for i in 0..n {
            g.nodes.push(Node {
                id: i, uuid: i as u128, kind: Kind::Text, refers: None, parent: None,
                turn: if i % 2 == 0 { Turn::User } else { Turn::Assistant },
                preview: format!("turn number {i} with some text"), tokens: 10,
                in_context: true, keywords: Vec::new(), looks_back: false, pruned: false,
            });
        }
        for &(p, c) in edges { g.nodes[c].parent = Some(p); }
        g
    }

    #[test]
    fn the_root_is_the_node_with_no_parent_not_id_zero() {
        let mut g = g_from(&[(1, 0), (1, 2)], 3);   // node 1 is the root here
        g.nodes[1].parent = None;
        let r = roles(&g);
        // A ROOT THAT FORKS IS THE ROOT. There is exactly one root and forks are common, so the
        // rarer fact is the one worth the label — which is why `is_root` sits above the child count
        // in the precedence chain. My first version of this test asserted Fork, from a comment I
        // had written without checking it against the code beneath it.
        assert_eq!(r[1], Role::Root, "the conversation's start outranks the fact that it forks");
        // and a non-root fork is still a fork
        let g2 = g_from(&[(0, 1), (1, 2), (1, 3)], 4);
        assert_eq!(roles(&g2)[1], Role::Fork);
        assert_eq!(roles(&g2)[0], Role::Root);
    }

    #[test]
    fn a_fork_is_where_the_conversation_changed_shape() {
        let g = g_from(&[(0, 1), (1, 2), (1, 3)], 4);
        let r = roles(&g);
        assert_eq!(r[1], Role::Fork);
        assert_eq!(r[2], Role::Leaf);
        assert_eq!(r[3], Role::Leaf);
    }

    /// Precedence is the design, so it gets a test rather than a comment.
    #[test]
    fn pruned_outranks_everything_else_a_node_might_be() {
        let mut g = g_from(&[(0, 1), (1, 2), (1, 3)], 4);
        g.nodes[1].pruned = true;      // also a fork
        g.nodes[1].looks_back = true;  // also a loopback
        assert_eq!(roles(&g)[1], Role::Pruned,
                   "a node outside the model's context is that, before it is anything else");
    }

    #[test]
    fn an_artifact_is_not_a_turn() {
        let mut g = g_from(&[(0, 1)], 2);
        g.nodes[1].kind = Kind::Patch;
        assert_eq!(roles(&g)[1], Role::Artifact);
        g.nodes[1].kind = Kind::Liaison;
        assert_eq!(roles(&g)[1], Role::Liaison, "the backend narrating itself is neither");
    }

    #[test]
    fn a_loopback_is_reported_when_nothing_louder_applies() {
        let mut g = g_from(&[(0, 1), (1, 2)], 3);
        g.nodes[1].looks_back = true;
        assert_eq!(roles(&g)[1], Role::Loopback);
        // but a fork that also looks back is a fork: the shape change matters more
        let mut g2 = g_from(&[(0, 1), (1, 2), (1, 3)], 4);
        g2.nodes[1].looks_back = true;
        assert_eq!(roles(&g2)[1], Role::Fork);
    }

    /// The majority role earns no tag. A label that says REPLY on two thirds of the graph has spent
    /// its space on the one thing the reader could already assume.
    #[test]
    fn the_common_case_is_not_tagged() {
        assert!(!Role::Reply.worth_showing());
        for r in [Role::Pruned, Role::Root, Role::Fork, Role::Loopback, Role::Artifact,
                  Role::Liaison, Role::Fan, Role::Leaf, Role::Ask] {
            assert!(r.worth_showing(), "{r:?}");
        }
        // 24 chars exactly, so nothing is elided — the point is only that no tag was prepended.
        assert_eq!(line(Role::Reply, "the model said something", 24), "the model said something");
        assert_eq!(line(Role::Reply, "the model said something at length", 24), "the model said…");
    }

    #[test]
    fn the_tag_is_charged_against_the_text_not_added_to_it() {
        let out = line(Role::Fork, "why is this better than the other one", 26);
        assert!(out.chars().count() <= 26, "{out:?} overflowed the budget");
        assert!(out.starts_with("FORK · "), "{out:?}");
    }

    /// A tag that pushes the content off the end has made the label worse, so below a floor the
    /// text wins outright.
    #[test]
    fn a_budget_too_small_for_both_gives_the_text() {
        let out = line(Role::Loopback, "revert that change", 12);
        assert!(!out.starts_with("LOOPBACK"), "{out:?} spent the whole line on the tag");
        assert!(out.starts_with("revert"), "{out:?}");
    }

    #[test]
    fn a_node_with_no_text_still_shows_what_it_is() {
        assert_eq!(line(Role::Pruned, "   ", 20), "PRUNED");
    }

    #[test]
    fn every_role_has_a_short_upper_case_label() {
        for r in [Role::Pruned, Role::Root, Role::Fork, Role::Loopback, Role::Artifact,
                  Role::Liaison, Role::Fan, Role::Leaf, Role::Ask, Role::Reply] {
            let l = r.label();
            assert!(l.chars().count() <= 8, "{r:?} label {l:?} is too long for a label line");
            assert_eq!(l, l.to_uppercase(), "{r:?} does not read as a category");
        }
    }

    #[test]
    fn an_empty_graph_classifies_to_nothing() {
        assert!(roles(&Graph::default()).is_empty());
    }
}
