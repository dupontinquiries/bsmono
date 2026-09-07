//! loggroup — the engine feed as sections you can fold and copy.
//!
//! The Engine panel prints every diagnostic in one flat list. `diagfeed` already folds a
//! line that repeats itself into one entry with a count, which handles a chatty line — but
//! not a chatty SUBJECT. A load produces a run of preflight notes, then a run of tile
//! overrides, then a run of dispatch levers, and each of those is one thing said in six
//! lines. Twenty-three lines of that is a wall, and the interesting one — the failure — is
//! somewhere in the middle of it.
//!
//! This groups by subject: a header with a count you can fold, and text you can copy. The
//! copy button matters more than the fold; the reason anyone reads this panel is to paste it
//! into a bug report or a message, and the alternative today is a screenshot.
//!
//! ## Grouping by subject, not by adjacency
//!
//! A run of lines about the same subject is not necessarily contiguous — a warning can land
//! in the middle of the preflight notes. Grouping by adjacency would split preflight in two
//! and put the halves in different places, so the subject is derived from the line's own
//! text and lines keep their original order WITHIN a group. Group order follows first
//! appearance, so the panel still reads chronologically at the top level.
//!
//! ## What is never folded
//!
//! Failures and warnings stay ungrouped and in place. A folded failure is a failure you did
//! not see, and the entire value of this panel is that a refusal is visible before the first
//! token. `Kind::Failure` and `Kind::Warning` therefore each get their own single-line
//! "group" of one, expanded, rather than being tucked under a subject header with a count.

use crate::diagfeed::{Entry, Kind};

/// A named run of related lines.
// `Entry` is not `PartialEq`, so neither is a Group. Tests compare the fields they care
// about rather than whole groups, which is the more precise assertion anyway.
#[derive(Debug, Clone)]
pub struct Group {
    /// The subject, as a panel header: "preflight", "tile overrides", "load".
    pub label: String,
    pub entries: Vec<Entry>,
    /// Start expanded? Failures and warnings do; bulk context does not.
    pub open: bool,
}

impl Group {
    /// Total lines, counting a folded repeat once per occurrence — a group holding one
    /// entry seen twelve times is twelve lines of output, and saying "1" would undersell
    /// what was hidden.
    pub fn line_count(&self) -> usize {
        self.entries.iter().map(|e| e.count.max(1)).sum()
    }

    /// The group as plain text, for the clipboard. One line per entry, with the repeat
    /// count kept — someone reading the paste needs to know a line happened forty times.
    pub fn to_clipboard(&self) -> String {
        let mut s = String::new();
        for e in &self.entries {
            s.push_str(&e.text);
            if e.count > 1 {
                s.push_str(&format!("  ×{}", e.count));
            }
            s.push('\n');
        }
        s
    }
}

/// The subject a line belongs to.
///
/// Prefix-based and deliberately shallow: the engine writes these lines and their opening
/// words are stable, whereas anything cleverer would silently re-categorise when a message
/// is reworded. An unrecognised line goes to "engine", which is honest — it is engine
/// output and nothing more specific is known.
pub fn subject(text: &str) -> &'static str {
    let t = text.trim_start_matches("wgpu-engine: ").trim();
    let low = t.to_ascii_lowercase();
    const TABLE: &[(&str, &str)] = &[
        ("preflight", "preflight"),
        ("load path", "load"),
        ("loading ", "load"),
        ("worker: loading", "load"),
        ("released page cache", "load"),
        ("adapter =", "adapter"),
        ("tile overrides", "tiles"),
        ("tile ", "tiles"),
        ("dispatch levers", "dispatch"),
        ("dispatch ", "dispatch"),
        ("submit batch", "dispatch"),
        ("fast-decode levers", "dispatch"),
        ("kv prefix tree", "kv cache"),
        ("kv ", "kv cache"),
        ("phase ", "timing"),
        ("decode tile", "timing"),
        ("roofline", "timing"),
    ];
    for (needle, label) in TABLE {
        if low.starts_with(needle) {
            return label;
        }
    }
    "engine"
}

/// Group a feed snapshot.
///
/// `min_group` is how many entries a subject needs before it is worth folding; below it the
/// lines are left loose, because a header hiding one line costs a click and saves nothing.
pub fn group(entries: &[Entry], min_group: usize) -> Vec<Group> {
    let mut order: Vec<&'static str> = Vec::new();
    let mut buckets: std::collections::HashMap<&'static str, Vec<Entry>> = Default::default();
    let mut out: Vec<Group> = Vec::new();

    for e in entries {
        // NEVER FOLDED: a failure or a warning goes straight out, in place, expanded. A
        // folded failure is a failure you did not see.
        if matches!(e.kind, Kind::Failure | Kind::Warning) {
            // Flush what is buffered so ordering around the failure is preserved.
            for label in order.drain(..) {
                if let Some(v) = buckets.remove(label) {
                    out.push(finish(label, v, min_group));
                }
            }
            out.push(Group { label: e.kind.short_label().to_string(), entries: vec![e.clone()], open: true });
            continue;
        }
        let s = subject(&e.text);
        if !buckets.contains_key(s) {
            order.push(s);
        }
        buckets.entry(s).or_default().push(e.clone());
    }
    for label in order {
        if let Some(v) = buckets.remove(label) {
            out.push(finish(label, v, min_group));
        }
    }
    out
}

fn finish(label: &'static str, entries: Vec<Entry>, min_group: usize) -> Group {
    // Below the threshold a group is not worth a fold: leave it open so it reads as the
    // loose lines it effectively is.
    let open = entries.len() < min_group.max(1);
    Group { label: label.to_string(), entries, open }
}

/// Every group as one block of text, with headers. What "copy all" produces.
pub fn all_to_clipboard(groups: &[Group]) -> String {
    let mut s = String::new();
    for g in groups {
        s.push_str(&format!("── {} ({} line(s))\n", g.label, g.line_count()));
        s.push_str(&g.to_clipboard());
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(kind: Kind, text: &str) -> Entry {
        Entry { kind, text: text.to_string(), count: 1 }
    }

    fn feed() -> Vec<Entry> {
        vec![
            e(Kind::Context, "wgpu-engine: load path — 89.3% of weights sliced from the file directly"),
            e(Kind::Context, "wgpu-engine: preflight — Asahi/Honeykrisp Vulkan detected → subgroup matvec OFF"),
            e(Kind::Context, "wgpu-engine: preflight — env LOGSCOPE_FLUSH_EVERY=4 (active)"),
            e(Kind::Context, "wgpu-engine: preflight — decode window submits bounded"),
            e(Kind::Context, "wgpu-engine: loading /home/k/models/llama3/Llama-3.2-1B-Instruct-Q4.gguf"),
            e(Kind::Context, "wgpu-engine: tile overrides — LOGSCOPE_DECODE_TILE / LOGSCOPE_GEMM_TILE pin"),
            e(Kind::Context, "wgpu-engine: tile 64×64"),
            e(Kind::Performance, "wgpu-engine: submit batch = 16 passes/submit"),
        ]
    }

    /// The panel's problem: six preflight lines and six tile lines are two things said
    /// twelve times, and the interesting line is somewhere in the middle.
    #[test]
    fn related_lines_collapse_under_one_header() {
        let gs = group(&feed(), 2);
        let by = |l: &str| gs.iter().find(|g| g.label == l).unwrap_or_else(|| panic!("no {l} group"));
        assert_eq!(by("preflight").entries.len(), 3);
        assert_eq!(by("load").entries.len(), 2, "the two load lines belong together");
        assert_eq!(by("tiles").entries.len(), 2);
        // A group big enough to fold starts folded; a lone line does not, because a header
        // hiding one line costs a click and saves nothing.
        assert!(!by("preflight").open);
        assert!(by("dispatch").open, "a single-line subject should stay loose");
    }

    /// Group order follows FIRST APPEARANCE, so the panel still reads chronologically even
    /// though membership is by subject rather than adjacency.
    #[test]
    fn a_subject_split_by_an_interruption_is_still_one_group() {
        let mut f = feed();
        f.insert(2, e(Kind::Context, "wgpu-engine: tile early"));
        let gs = group(&f, 2);
        let labels: Vec<&str> = gs.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels.iter().filter(|l| **l == "tiles").count(), 1, "tiles was split: {labels:?}");
        assert_eq!(gs.iter().find(|g| g.label == "tiles").unwrap().entries.len(), 3);
        // and the first group is still the first thing that happened
        assert_eq!(labels[0], "load");
    }

    /// A folded failure is a failure you did not see. Failures and warnings stay in place
    /// and expanded, and they flush whatever was buffering so the order around them holds.
    #[test]
    fn failures_and_warnings_are_never_folded_away() {
        let mut f = feed();
        f.insert(3, e(Kind::Failure, "wgpu-engine: preflight — device lost during load"));
        let gs = group(&f, 2);
        let fail = gs.iter().position(|g| g.entries.iter().any(|x| x.kind == Kind::Failure)).expect("kept");
        assert!(gs[fail].open, "a failure was folded shut");
        assert_eq!(gs[fail].entries.len(), 1, "a failure was grouped with unrelated lines");
        // It sits between the lines that preceded and followed it, not at the end.
        assert!(fail > 0 && fail < gs.len() - 1, "position {fail} of {}", gs.len());
        // Its text is NOT also inside the preflight group, despite starting with "preflight".
        let pre: Vec<&Group> = gs.iter().filter(|g| g.label == "preflight").collect();
        assert!(pre.iter().all(|g| g.entries.iter().all(|x| x.kind != Kind::Failure)));
    }

    /// The copy button is the point: the reason anyone reads this panel is to paste it
    /// somewhere, and the alternative today is a screenshot.
    #[test]
    fn a_group_copies_as_text_and_keeps_its_repeat_counts() {
        let mut g = group(&feed(), 2);
        let pre = g.iter_mut().find(|g| g.label == "preflight").unwrap();
        pre.entries[0].count = 12;
        let txt = pre.to_clipboard();
        assert_eq!(txt.lines().count(), 3);
        assert!(txt.contains("×12"), "a repeat count was lost in the paste: {txt}");
        assert!(txt.starts_with("wgpu-engine: preflight"));
        assert_eq!(pre.line_count(), 14, "12 + 1 + 1");

        let all = all_to_clipboard(&g);
        assert!(all.contains("── preflight (14 line(s))"), "{all}");
        assert!(all.contains("── load ("));
        // every line of the feed survives the round trip
        for e in feed() {
            assert!(all.contains(&e.text), "{:?} was lost", e.text);
        }
    }

    /// Subjects come from the line's opening words, which the engine controls and which are
    /// stable. Anything unrecognised is "engine" rather than being forced somewhere.
    #[test]
    fn an_unknown_line_is_not_forced_into_a_subject() {
        assert_eq!(subject("wgpu-engine: preflight — anything"), "preflight");
        assert_eq!(subject("preflight — without the prefix"), "preflight");
        assert_eq!(subject("wgpu-engine: KV prefix tree ON"), "kv cache");
        assert_eq!(subject("wgpu-engine: something nobody has written yet"), "engine");
        assert_eq!(subject(""), "engine");
        // An empty feed is an empty list, not a group of nothing.
        assert!(group(&[], 2).is_empty());
    }
}
