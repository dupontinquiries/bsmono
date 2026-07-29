//! liaison_relay.rs — the Liaison talking to the Thinker directly, and folding its answer back.
//!
//! Two jobs that look like one:
//!
//! 1. **Relay.** The Liaison forwards a question to the Thinker without the operator retyping it, and
//!    without the Liaison paraphrasing it — a small model rewriting a request before a large one sees it
//!    is a lossy step nobody asked for. The relay carries the text VERBATIM and adds only context the
//!    Thinker cannot otherwise have.
//! 2. **Append.** The Thinker answers with changes; the Liaison folds them into the file by LINE NUMBER
//!    rather than by regenerating it. A large model asked to reprint a 400-line file to change three
//!    lines will get some of the other 397 wrong, and the diff will not show which because everything
//!    moved.
//!
//! ## Why line numbers rather than a diff
//!
//! A unified diff needs matching context, and a model that misremembers the context produces a hunk that
//! fails to apply — which `patchmode` already handles, but only by rejecting it. An explicit line number
//! is unambiguous: it either exists or it does not, and the failure is immediate and local rather than a
//! fuzzy match that lands in the wrong place.

/// One change the Thinker asked for.
#[derive(Clone, PartialEq, Debug)]
pub enum Edit {
    /// Replace line `at` (1-based) with `text`.
    Replace { at: usize, text: String },
    /// Insert `text` AFTER line `at`. `at == 0` means before the first line.
    InsertAfter { at: usize, text: String },
    /// Delete line `at`.
    Delete { at: usize },
}

impl Edit {
    pub fn line(&self) -> usize {
        match self { Edit::Replace { at, .. } | Edit::InsertAfter { at, .. } | Edit::Delete { at } => *at }
    }
}

/// Number a file for the Thinker to refer to.
///
/// One-based, because every editor and every compiler error is one-based, and an off-by-one between what
/// the model reads and what the human reads is the worst kind of confusion to debug.
pub fn number(src: &str) -> String {
    src.lines().enumerate().map(|(i, l)| format!("{:>4} | {l}", i + 1)).collect::<Vec<_>>().join("\n")
}

/// Parse the Thinker's reply into edits.
///
/// Format, stated in `APPEND_PROTOCOL` and deliberately terse:
/// ```text
/// @@12 replace
/// let x = 1;
/// @@40 after
/// fn extra() {}
/// @@7 delete
/// ```
pub fn parse_edits(reply: &str) -> Vec<Edit> {
    let mut out = Vec::new();
    let mut cur: Option<(usize, String)> = None;   // (line, kind)
    let mut buf: Vec<String> = Vec::new();
    let flush = |out: &mut Vec<Edit>, cur: &Option<(usize, String)>, buf: &mut Vec<String>| {
        let Some((at, kind)) = cur else { buf.clear(); return };
        let text = buf.join("\n");
        match kind.as_str() {
            "replace" => out.push(Edit::Replace { at: *at, text }),
            "after" => out.push(Edit::InsertAfter { at: *at, text }),
            "delete" => out.push(Edit::Delete { at: *at }),
            _ => {}
        }
        buf.clear();
    };
    for line in reply.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("@@") {
            flush(&mut out, &cur, &mut buf);
            let mut it = rest.split_whitespace();
            let at = it.next().and_then(|v| v.parse::<usize>().ok());
            let kind = it.next().unwrap_or("replace").to_ascii_lowercase();
            cur = at.map(|a| (a, kind));
            continue;
        }
        if cur.is_some() { buf.push(line.to_string()); }
    }
    flush(&mut out, &cur, &mut buf);
    out
}

/// Apply edits to a file.
///
/// Applied from the BOTTOM UP, so earlier line numbers stay valid as later ones shift. Applying top-down
/// means every edit after the first refers to a file that no longer exists — the classic way a batch of
/// line-numbered edits lands progressively further from where it was meant to.
///
/// Returns the new text, or an error naming the first out-of-range line. Out of range is a hard failure
/// rather than a clamp: a model that asked to change line 900 of a 400-line file has misunderstood
/// something, and silently editing line 400 instead would hide that.
pub fn apply(src: &str, edits: &[Edit]) -> Result<String, String> {
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    let n = lines.len();
    for e in edits {
        let at = e.line();
        let ok = match e {
            Edit::InsertAfter { .. } => at <= n,       // 0 = before the first line
            _ => at >= 1 && at <= n,
        };
        if !ok {
            return Err(format!("edit refers to line {at}, but the file has {n} lines — the model is \
working from a different version, or miscounted; applying it anywhere else would hide that"));
        }
    }
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.line()));
    for e in sorted {
        match e {
            Edit::Replace { at, text } => { lines[at - 1] = text.clone(); }
            Edit::Delete { at } => { lines.remove(at - 1); }
            Edit::InsertAfter { at, text } => {
                let idx = *at;                          // insert AFTER line `at` = index `at`
                lines.insert(idx.min(lines.len()), text.clone());
            }
        }
    }
    Ok(lines.join("\n"))
}

/// A short stand-in for a long Thinker reply, shown when the message is collapsed.
///
/// The linear view fills up with 400-line answers that are relevant for one turn and then are scenery.
/// Collapsing them by default once they are no longer the latest message keeps the conversation
/// readable — but a collapsed message must say enough that a reader knows whether to open it, or they
/// will open all of them and the collapse has bought nothing.
#[derive(Clone, PartialEq, Debug)]
pub struct CliffsNotes {
    /// One line: what the reply did.
    pub gist: String,
    /// Files touched and how many lines each, since that is what a reader scans for.
    pub touched: Vec<(String, usize)>,
    pub total_lines: usize,
}

impl CliffsNotes {
    /// Should this message be collapsed by default?
    ///
    /// Only when it is BOTH long and not the latest. A short reply gains nothing from collapsing, and
    /// collapsing the message someone is currently reading is actively hostile.
    pub fn should_collapse(total_lines: usize, is_latest: bool) -> bool {
        !is_latest && total_lines > COLLAPSE_ABOVE_LINES
    }

    /// The chevron label.
    pub fn label(&self) -> String {
        if self.touched.is_empty() {
            return format!("{} — {} lines", self.gist, self.total_lines);
        }
        let files: Vec<String> = self.touched.iter().map(|(f, n)| format!("{f} ({n})")).collect();
        format!("{} — {}", self.gist, files.join(", "))
    }
}

/// Longer than this, and a non-latest message collapses.
pub const COLLAPSE_ABOVE_LINES: usize = 12;

/// Summarize a reply for the collapsed view.
///
/// Derived from the EDITS where there are any, because "changed 3 lines in loader.rs" is what a reader
/// scans for — not the first sentence of the prose, which is usually "Sure, here's how to do that" and
/// tells them nothing.
pub fn cliffsnotes(reply: &str, edits: &[Edit], file: Option<&str>) -> CliffsNotes {
    let total_lines = reply.lines().count();
    if edits.is_empty() {
        // Fall back to the first non-empty, non-preamble line.
        let gist = reply.lines().map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("```"))
            .unwrap_or("(empty reply)").chars().take(80).collect::<String>();
        return CliffsNotes { gist, touched: Vec::new(), total_lines };
    }
    let (mut rep, mut ins, mut del) = (0, 0, 0);
    for e in edits {
        match e { Edit::Replace { .. } => rep += 1, Edit::InsertAfter { .. } => ins += 1,
                  Edit::Delete { .. } => del += 1 }
    }
    let mut parts = Vec::new();
    if rep > 0 { parts.push(format!("{rep} changed")); }
    if ins > 0 { parts.push(format!("{ins} added")); }
    if del > 0 { parts.push(format!("{del} removed")); }
    CliffsNotes {
        gist: parts.join(", "),
        touched: file.map(|f| vec![(f.to_string(), edits.len())]).unwrap_or_default(),
        total_lines,
    }
}

/// The instruction given to the Thinker.
pub const APPEND_PROTOCOL: &str = "\
The file below is line-numbered. Reply ONLY with edits, in this form:

    @@<line> replace
    <the new text for that line>

    @@<line> after
    <text to insert after that line>

    @@<line> delete

Rules:
- Refer to the line numbers shown. Do not reprint the file.
- One `@@` header per change; the lines under it are its text.
- If you need to change nothing, reply with no `@@` lines at all.
Reprinting the whole file to change a few lines is how the untouched parts get quietly altered, which is \
why this format does not allow it.";

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "one\ntwo\nthree\nfour";

    #[test]
    fn numbering_is_ONE_based_like_every_compiler_error() {
        // An off-by-one between what the model reads and what the human reads is the worst kind of
        // confusion to debug, because both parties are certain and neither is looking at the same line.
        let n = number(SRC);
        assert!(n.starts_with("   1 | one"));
        assert!(n.contains("   4 | four"));
    }

    #[test]
    fn edits_apply_BOTTOM_UP_so_earlier_numbers_stay_valid() {
        // Top-down application means every edit after the first refers to a file that no longer exists.
        // Here: deleting line 2 first would make "line 4" point at the wrong text.
        let edits = vec![Edit::Delete { at: 2 }, Edit::Replace { at: 4, text: "FOUR".into() }];
        assert_eq!(apply(SRC, &edits).unwrap(), "one\nthree\nFOUR");
    }

    #[test]
    fn an_out_of_range_line_FAILS_rather_than_clamping() {
        // A model asking to change line 900 of a 400-line file has misunderstood something. Editing line
        // 400 instead would apply a change the model never asked for, in a place it never looked.
        let e = apply(SRC, &[Edit::Replace { at: 90, text: "x".into() }]).unwrap_err();
        assert!(e.contains("line 90") && e.contains("4 lines"));
        assert!(e.contains("different version"), "the message must suggest the likely cause");
    }

    #[test]
    fn insert_after_zero_puts_text_at_the_top() {
        assert_eq!(apply(SRC, &[Edit::InsertAfter { at: 0, text: "zero".into() }]).unwrap(),
                   "zero\none\ntwo\nthree\nfour");
    }

    #[test]
    fn the_reply_parser_handles_all_three_kinds_and_multi_line_text() {
        let reply = "@@2 replace\nTWO\n@@4 after\nfive\nsix\n@@1 delete\n";
        let e = parse_edits(reply);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0], Edit::Replace { at: 2, text: "TWO".into() });
        assert_eq!(e[1], Edit::InsertAfter { at: 4, text: "five\nsix".into() });
        assert_eq!(e[2], Edit::Delete { at: 1 });
    }

    #[test]
    fn prose_before_the_first_header_is_ignored() {
        // Models preamble. Treating "Sure, here are the changes:" as edit text would corrupt the file
        // with the model's own chatter.
        let e = parse_edits("Sure, here are the changes:\n@@1 replace\nONE\n");
        assert_eq!(e, vec![Edit::Replace { at: 1, text: "ONE".into() }]);
    }

    #[test]
    fn a_reply_with_no_headers_is_no_edits_not_an_error() {
        // "Nothing needs changing" is a legitimate answer, and the protocol says to reply with no
        // headers. Treating that as a parse failure would make the correct response look broken.
        assert!(parse_edits("Looks fine to me.").is_empty());
        assert_eq!(apply(SRC, &[]).unwrap(), SRC);
    }

    #[test]
    fn a_collapsed_message_says_enough_to_decide_whether_to_open_it() {
        // A chevron labelled "Assistant reply" buys nothing — the reader opens all of them and the
        // collapse has cost a click without saving attention.
        let edits = vec![Edit::Replace { at: 1, text: "a".into() }, Edit::Replace { at: 2, text: "b".into() },
                         Edit::InsertAfter { at: 9, text: "c".into() }];
        let c = cliffsnotes("...long reply...", &edits, Some("loader.rs"));
        let l = c.label();
        assert!(l.contains("2 changed") && l.contains("1 added"), "{l}");
        assert!(l.contains("loader.rs"), "the file is what a reader scans for: {l}");
    }

    #[test]
    fn the_gist_skips_the_models_preamble() {
        // "Sure, here's how to do that" is the most common first line and says nothing. With edits
        // present the summary is derived from THEM instead.
        let c = cliffsnotes("Sure, here's how to do that:
@@1 replace
x",
                            &[Edit::Replace { at: 1, text: "x".into() }], Some("f.rs"));
        assert!(!c.gist.contains("Sure"), "{}", c.gist);
        assert_eq!(c.gist, "1 changed");
    }

    #[test]
    fn only_LONG_and_NOT_LATEST_messages_collapse() {
        // Collapsing the message someone is currently reading is actively hostile, and collapsing a
        // three-line answer saves nothing.
        assert!(CliffsNotes::should_collapse(400, false));
        assert!(!CliffsNotes::should_collapse(400, true), "never collapse the latest");
        assert!(!CliffsNotes::should_collapse(3, false), "short replies gain nothing");
    }

    #[test]
    fn a_reply_with_no_edits_still_gets_a_usable_gist() {
        let c = cliffsnotes("```rust
fn main() {}
```", &[], None);
        assert!(!c.gist.starts_with("```"), "the fence is not a summary: {}", c.gist);
        assert_eq!(c.gist, "fn main() {}");
    }

    #[test]
    fn the_protocol_forbids_reprinting_and_says_why() {
        assert!(APPEND_PROTOCOL.contains("Do not reprint the file"));
        assert!(APPEND_PROTOCOL.contains("quietly altered"),
            "a rule without its reason gets optimised away by the next model");
    }
}
