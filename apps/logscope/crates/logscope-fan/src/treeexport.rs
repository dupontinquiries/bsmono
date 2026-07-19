//! treeexport.rs — render the conversation TREE to text. The existing exporter walks a single linear
//! path; these show the whole branching structure. Three formats, all pure + testable:
//!   • ascii    — an indented tree with ├─/└─ connectors (glanceable shape)
//!   • markdown — nested sections per branch with the full turn text (shareable / archival)
//!   • git_script — a bash script that reconstructs the tree as a real git history: one commit per
//!     turn, one branch per fork (so you can `git log --graph` your conversation). Each commit is
//!     tagged n<id> so a child can check out its parent before committing.

#[derive(Clone, Debug)]
pub struct ExportNode { pub id: usize, pub parent: Option<usize>, pub role: String, pub text: String }

fn children_of(nodes: &[ExportNode]) -> Vec<Vec<usize>> {
    let mut kids = vec![Vec::new(); nodes.len()];
    for n in nodes { if let Some(p) = n.parent { if p < kids.len() { kids[p].push(n.id); } } }
    kids
}
fn roots(nodes: &[ExportNode]) -> Vec<usize> { nodes.iter().filter(|n| n.parent.is_none()).map(|n| n.id).collect() }
fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    let t: String = line.chars().take(max).collect();
    if line.chars().count() > max { format!("{t}…") } else { t }
}

/// Indented ASCII tree with box-drawing connectors.
pub fn ascii(nodes: &[ExportNode]) -> String {
    let kids = children_of(nodes);
    let mut out = String::from("conversation\n");
    fn walk(id: usize, prefix: &str, last: bool, nodes: &[ExportNode], kids: &[Vec<usize>], out: &mut String) {
        let n = &nodes[id];
        let branch = if last { "└─ " } else { "├─ " };
        out.push_str(&format!("{prefix}{branch}[{}] {} ({}t? — turn {})\n", n.role, first_line(&n.text, 60), n.text.split_whitespace().count(), id + 1));
        let next_prefix = format!("{prefix}{}", if last { "   " } else { "│  " });
        let cs = &kids[id];
        for (i, &c) in cs.iter().enumerate() { walk(c, &next_prefix, i + 1 == cs.len(), nodes, kids, out); }
    }
    let rs = roots(nodes);
    for (i, &r) in rs.iter().enumerate() { walk(r, "", i + 1 == rs.len(), nodes, &kids, &mut out); }
    out
}

/// Nested Markdown: each turn a bullet with its text; forks become indented sub-lists labelled by the
/// branch point. (Whole tree, not just the active path.)
pub fn markdown(nodes: &[ExportNode]) -> String {
    let kids = children_of(nodes);
    let mut out = String::from("# Conversation tree\n\n");
    fn walk(id: usize, depth: usize, nodes: &[ExportNode], kids: &[Vec<usize>], out: &mut String) {
        let n = &nodes[id];
        let indent = "  ".repeat(depth);
        out.push_str(&format!("{indent}- **{} · turn {}**\n", n.role, id + 1));
        for line in n.text.lines() { out.push_str(&format!("{indent}  {line}\n")); }
        let cs = &kids[id];
        if cs.len() > 1 { // a fork — call it out
            for &c in cs { out.push_str(&format!("{indent}  - _branch →_\n")); walk(c, depth + 2, nodes, kids, out); }
        } else {
            for &c in cs { walk(c, depth + 1, nodes, kids, out); }
        }
    }
    for &r in &roots(nodes) { walk(r, 0, nodes, &kids, &mut out); }
    out
}

/// A bash script reconstructing the tree as a git history (commit per turn, branch per fork). The turn
/// text is written via a quoted heredoc (no shell interpolation); each commit is tagged n<id>.
pub fn git_script(nodes: &[ExportNode]) -> String {
    let mut out = String::from("#!/usr/bin/env bash\nset -euo pipefail\ngit init -q\n");
    // emit in id order so every parent is committed (and tagged) before its children
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_unstable();
    let kids = children_of(nodes);
    let mut emitted = vec![false; nodes.len()];
    for &id in &order {
        let n = &nodes[id];
        let file = format!("turn-{id}.md");
        // position HEAD: roots start fresh; a child checks out its parent's tag; a fork (parent already
        // has an emitted child) starts a new branch off the parent.
        if let Some(p) = n.parent {
            let parent_forked = kids[p].iter().filter(|&&c| emitted[c]).count() >= 1;
            out.push_str(&format!("git checkout -q n{p}\n"));
            if parent_forked { out.push_str(&format!("git checkout -q -b branch-{id}\n")); }
        }
        out.push_str(&format!("cat > {file} <<'LOGSCOPE_EOF'\n{}\nLOGSCOPE_EOF\n", n.text));
        let subject = first_line(&n.text, 50).replace('\'', "");
        out.push_str(&format!("git add -A && git commit -q -m '{} · turn {}: {}' && git tag n{id}\n", n.role, id + 1, subject));
        emitted[id] = true;
    }
    out.push_str("echo 'done — try: git log --graph --oneline --all'\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn n(id: usize, parent: Option<usize>, role: &str, text: &str) -> ExportNode {
        ExportNode { id, parent, role: role.into(), text: text.into() }
    }
    // 0:you → 1:asst ; fork at 1 → 2:you (branch A) and 3:you (branch B)
    fn tree() -> Vec<ExportNode> {
        vec![
            n(0, None, "you", "sort a vec by weight"),
            n(1, Some(0), "assistant", "use sort_by_key"),
            n(2, Some(1), "you", "now reverse it"),
            n(3, Some(1), "you", "make it stable"),
        ]
    }

    #[test]
    fn ascii_shows_connectors_and_the_fork() {
        let s = ascii(&tree());
        assert!(s.contains("├─") || s.contains("└─"), "has tree connectors");
        assert!(s.contains("sort a vec by weight") && s.contains("now reverse it"));
        // node 1 has two children → both branches present under it
        assert!(s.contains("make it stable"), "the second branch is rendered too");
    }

    #[test]
    fn markdown_includes_all_turns_and_marks_branches() {
        let s = markdown(&tree());
        assert!(s.contains("# Conversation tree"));
        for t in ["sort a vec by weight", "use sort_by_key", "now reverse it", "make it stable"] {
            assert!(s.contains(t), "markdown includes '{t}'");
        }
        assert!(s.contains("branch"), "fork is labelled");
    }

    #[test]
    fn git_script_commits_each_turn_and_forks() {
        let s = git_script(&tree());
        assert!(s.starts_with("#!/usr/bin/env bash"));
        for id in 0..4 { assert!(s.contains(&format!("git tag n{id}")), "commit+tag for turn {id}"); }
        assert!(s.contains("git checkout -q n1"), "children of the fork check out their parent");
        assert!(s.contains("git checkout -q -b branch-3"), "the second child of the fork starts a branch");
        assert!(s.contains("LOGSCOPE_EOF"), "turn text written via a safe heredoc");
    }

    #[test]
    fn empty_tree_is_safe() {
        assert_eq!(ascii(&[]), "conversation\n");
        assert!(markdown(&[]).contains("# Conversation tree"));
        assert!(git_script(&[]).contains("git init"));
    }
}
