//! patchmode.rs — let a model edit code by emitting PATCHES (unified diffs) instead of rewriting whole
//! files. Token-efficient for edits (only the changed lines travel), and it lands naturally in the gitmind
//! checkpoint flow: parsed hunks apply to the working copy, then `gitmind_session::reapply` commits the
//! result exactly as the full-file path does. Complements `merge.rs` (item-block lazy edits) and
//! `workspace_exec`'s WriteFile (full overwrite) with a THIRD, diff-based edit mode.
//!
//! Format: git-style unified diff. Per file, a header naming the path, then one or more `@@` hunks:
//!
//! ```text
//! *** path: src/main.rs
//! @@
//!  context line (space prefix = unchanged)
//! -removed line
//! +added line
//!  trailing context
//! ```
//!
//! We accept both the `*** path:` header (robust for local models that mangle `--- / +++`) AND standard
//! `--- a/… / +++ b/…`. Application is line-based and CONTEXT-ANCHORED: a hunk locates its position by
//! matching its context/removed lines against the current file, so hunks apply even when line numbers in
//! an `@@ -l,c +l,c @@` header are wrong (local models get those wrong constantly). Anything that can't be
//! matched confidently is REPORTED, never guessed — the human resolves it, same discipline as merge.rs.

/// The instruction block appended to the system prompt when patch mode is on. Mechanical, format-first.
///
/// The last two rules carry the Liaison discipline into generation: a hunk's context lines are a CLAIM
/// about the file's current contents, and application is context-anchored, so invented context does not
/// fail loudly — it either misapplies or silently fails to match. Asking rather than guessing is therefore
/// the cheaper behaviour, exactly as the Liaison reports absence rather than fabricating a summary.
pub const PATCH_MODE_PROMPT: &str = "\n\nPATCH MODE: When you change existing files, emit a UNIFIED DIFF patch \
instead of rewriting the whole file. For each file, write a header line:\n\
*** path: <relative/path>\n\
then one or more hunks. In a hunk, prefix each line with a space (unchanged context), '-' (remove), or \
'+' (add). Include 2-3 unchanged context lines around each change so the patch can be located. Do NOT \
reprint unchanged parts of the file. For a brand-new file, use the header then all '+' lines. Keep each \
patch minimal — only the lines that actually change.\n\
Context lines must be copied EXACTLY from the file you were given — they are how the patch is located, \
so approximate or remembered context will not apply. If you have not been shown the part of the file you \
need to change, say so and ask for it; do not guess at the surrounding lines. If a change is too \
pervasive to express as hunks, say that and emit the full file instead — an honest full rewrite is better \
than a patch built on invented context.";

/// One file's changes: the target path and its ordered hunks.
#[derive(Clone, Debug, PartialEq)]
pub struct FilePatch {
    pub path: String,
    pub hunks: Vec<Hunk>,
    /// True when every hunk line is an addition (no context, no removals) → a whole new file.
    pub is_new_file: bool,
}

/// One hunk: the ordered lines with their op. `Ctx` must match the original, `Del` is removed, `Add` added.
#[derive(Clone, Debug, PartialEq)]
pub enum Line { Ctx(String), Del(String), Add(String) }

#[derive(Clone, Debug, PartialEq)]
pub struct Hunk { pub lines: Vec<Line> }

/// A hunk whose context could not be located, kept with enough detail to act on.
///
/// A bare failure COUNT is not actionable: it tells the model something went wrong but not what to look
/// at, so the retry is a guess. Carrying the expected context back means the next attempt can be told
/// exactly which lines were not found.
#[derive(Debug, Clone, PartialEq)]
pub struct FailedHunk {
    /// 0-based index of the hunk within this file's patch.
    pub index: usize,
    /// The "before" image the hunk expected: context + removed lines, in order.
    pub expected: Vec<String>,
}

/// Result of applying a file's patch.
///
/// `text` is the PARTIALLY applied result when some hunks failed. That is deliberate — a caller may want
/// to inspect it — but writing it out unconditionally silently half-patches the file, which is why
/// [`is_clean`] exists and [`apply_file_patch_strict`] refuses instead.
#[derive(Debug, PartialEq)]
pub struct Applied {
    pub text: String,
    pub hunks_applied: usize,
    pub hunks_failed: usize,   // hunks whose context couldn't be located (reported, not force-applied)
    /// Which hunks failed, and what each expected to find.
    pub failures: Vec<FailedHunk>,
}

impl Applied {
    /// Did every hunk land? Only then is `text` safe to write without further thought.
    pub fn is_clean(&self) -> bool { self.hunks_failed == 0 }

    /// Operator-facing account of what did not apply — the Liaison rule: name the observable state, do
    /// not summarise it away. Empty when clean.
    pub fn report(&self) -> Vec<String> {
        self.failures.iter().map(|f| {
            let head = f.expected.iter().find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string()).unwrap_or_else(|| "(blank context)".into());
            format!("hunk {} did not apply — could not find {} line(s) starting: {}",
                    f.index + 1, f.expected.len(), head)
        }).collect()
    }
}

/// Apply only if EVERY hunk lands; otherwise return the original untouched alongside the failures.
///
/// The default is partial application, which is right for a UI that shows a diff and lets a human judge.
/// It is wrong for anything automatic: a half-patched file compiles differently, may still compile, and
/// the damage is discovered later. This is the variant an autonomous loop should use.
pub fn apply_file_patch_strict(original: &str, fp: &FilePatch) -> Applied {
    let out = apply_file_patch(original, fp);
    if out.is_clean() { return out; }
    Applied { text: original.to_string(), ..out }
}

fn strip_path(raw: &str) -> String {
    let p = raw.trim();
    // tolerate `a/…` `b/…` diff prefixes
    p.strip_prefix("a/").or_else(|| p.strip_prefix("b/")).unwrap_or(p).trim().to_string()
}

/// Parse a model reply into per-file patches. Recognizes `*** path:` and `--- a/… / +++ b/…` file headers,
/// and `@@` hunk separators. Lines outside any file header are ignored (prose between patches is fine).
pub fn parse_patches(text: &str) -> Vec<FilePatch> {
    let mut files: Vec<FilePatch> = Vec::new();
    let mut cur: Option<FilePatch> = None;
    let mut hunk: Option<Hunk> = None;

    let flush_hunk = |cur: &mut Option<FilePatch>, hunk: &mut Option<Hunk>| {
        if let (Some(f), Some(h)) = (cur.as_mut(), hunk.take()) {
            if !h.lines.is_empty() { f.hunks.push(h); }
        }
    };
    let flush_file = |files: &mut Vec<FilePatch>, cur: &mut Option<FilePatch>, hunk: &mut Option<Hunk>| {
        if let (Some(f), Some(h)) = (cur.as_mut(), hunk.take()) {
            if !h.lines.is_empty() { f.hunks.push(h); }
        }
        if let Some(mut f) = cur.take() {
            f.is_new_file = !f.hunks.is_empty()
                && f.hunks.iter().all(|h| h.lines.iter().all(|l| matches!(l, Line::Add(_))));
            if !f.hunks.is_empty() { files.push(f); }
        }
    };

    for line in text.lines() {
        // file headers
        if let Some(p) = line.strip_prefix("*** path:").or_else(|| line.strip_prefix("*** Path:")) {
            flush_file(&mut files, &mut cur, &mut hunk);
            cur = Some(FilePatch { path: strip_path(p), hunks: Vec::new(), is_new_file: false });
            continue;
        }
        if let Some(p) = line.strip_prefix("+++ ") {
            // standard unified-diff target header opens/retitles the current file
            let path = strip_path(p);
            if path != "/dev/null" {
                if cur.is_none() { cur = Some(FilePatch { path, hunks: Vec::new(), is_new_file: false }); }
                else if let Some(f) = cur.as_mut() { if f.path.is_empty() { f.path = path; } }
            }
            continue;
        }
        if line.starts_with("--- ") { continue; } // source header — path comes from +++/*** ; skip
        if line.starts_with("@@") {
            flush_hunk(&mut cur, &mut hunk);
            if cur.is_some() { hunk = Some(Hunk { lines: Vec::new() }); }
            continue;
        }
        // hunk body lines (only when inside a file+hunk)
        if let Some(h) = hunk.as_mut() {
            match line.as_bytes().first() {
                Some(b' ') => h.lines.push(Line::Ctx(line[1..].to_string())),
                Some(b'-') => h.lines.push(Line::Del(line[1..].to_string())),
                Some(b'+') => h.lines.push(Line::Add(line[1..].to_string())),
                _ => { /* blank/other between hunks — ignore */ }
            }
        }
    }
    flush_file(&mut files, &mut cur, &mut hunk);
    files
}

/// Apply one file's hunks to `original`, returning the patched text plus how many hunks applied/failed.
/// Context-anchored: each hunk is located by matching its context+removed lines against the file, so wrong
/// `@@` line numbers don't matter. A hunk that can't be located is skipped and counted as failed (the
/// caller surfaces that to the human rather than corrupting the file).
pub fn apply_file_patch(original: &str, fp: &FilePatch) -> Applied {
    if fp.is_new_file {
        let text: String = fp.hunks.iter().flat_map(|h| h.lines.iter()).filter_map(|l| match l {
            Line::Add(s) => Some(s.clone()), _ => None,
        }).collect::<Vec<_>>().join("\n");
        return Applied { text, hunks_applied: fp.hunks.len(), hunks_failed: 0, failures: Vec::new() };
    }
    let mut lines: Vec<String> = original.lines().map(|s| s.to_string()).collect();
    let (mut applied, mut failed) = (0usize, 0usize);
    let mut failures: Vec<FailedHunk> = Vec::new();
    let mut search_from = 0usize;
    for (hi, h) in fp.hunks.iter().enumerate() {
        // The "before" image this hunk expects: context + removed lines, in order.
        let before: Vec<&String> = h.lines.iter().filter_map(|l| match l {
            Line::Ctx(s) | Line::Del(s) => Some(s), Line::Add(_) => None,
        }).collect();
        // The replacement: context + added lines (removed dropped), in order.
        let after: Vec<String> = h.lines.iter().filter_map(|l| match l {
            Line::Ctx(s) | Line::Add(s) => Some(s.clone()), Line::Del(_) => None,
        }).collect();
        if before.is_empty() {
            // pure-insertion hunk with no anchor: append at the current cursor
            for (i, s) in after.iter().enumerate() { lines.insert((search_from + i).min(lines.len()), s.clone()); }
            search_from += after.len(); applied += 1; continue;
        }
        match locate(&lines, &before, search_from) {
            Some(at) => {
                lines.splice(at..at + before.len(), after.iter().cloned());
                search_from = at + after.len();
                applied += 1;
            }
            None => {
                // context not found → report WHAT was expected, don't guess and don't force
                failed += 1;
                failures.push(FailedHunk {
                    index: hi,
                    expected: before.iter().map(|s| (*s).clone()).collect(),
                });
            }
        }
    }
    Applied { text: lines.join("\n"), hunks_applied: applied, hunks_failed: failed, failures }
}

/// Find `needle` as a contiguous run in `hay` at or after `from`; if not found there, retry from 0 (hunks
/// are usually in order, but a model may emit them out of order). Returns the start index.
fn locate(hay: &[String], needle: &[&String], from: usize) -> Option<usize> {
    let find = |start: usize| -> Option<usize> {
        if needle.len() > hay.len() { return None; }
        (start..=hay.len() - needle.len()).find(|&i| {
            hay[i..i + needle.len()].iter().zip(needle).all(|(a, b)| a == *b)
        })
    };
    find(from).or_else(|| if from > 0 { find(0) } else { None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_star_path_header_and_ops() {
        let t = "*** path: src/main.rs\n@@\n ctx\n-old\n+new\n more\n";
        let fps = parse_patches(t);
        assert_eq!(fps.len(), 1);
        assert_eq!(fps[0].path, "src/main.rs");
        assert_eq!(fps[0].hunks.len(), 1);
        assert_eq!(fps[0].hunks[0].lines, vec![
            Line::Ctx("ctx".into()), Line::Del("old".into()), Line::Add("new".into()), Line::Ctx("more".into())]);
        assert!(!fps[0].is_new_file);
    }

    #[test]
    fn applies_a_simple_edit_context_anchored() {
        let orig = "fn a() {}\nfn b() {\n    old();\n}\nfn c() {}\n";
        let patch = "*** path: x.rs\n@@\n fn b() {\n-    old();\n+    new();\n }\n";
        let fp = &parse_patches(patch)[0];
        let out = apply_file_patch(orig, fp);
        assert_eq!(out.hunks_applied, 1);
        assert_eq!(out.hunks_failed, 0);
        assert!(out.text.contains("    new();"));
        assert!(!out.text.contains("old();"));
        assert!(out.text.contains("fn a() {}") && out.text.contains("fn c() {}"), "rest intact");
    }

    #[test]
    fn wrong_line_numbers_still_apply_via_context() {
        // @@ header has bogus line numbers; context matching must still find the site.
        let orig = "one\ntwo\nthree\nfour\n";
        let patch = "*** path: x\n@@ -999,3 +999,3 @@\n two\n-three\n+THREE\n four\n";
        let out = apply_file_patch(orig, &parse_patches(patch)[0]);
        assert_eq!((out.hunks_applied, out.hunks_failed), (1, 0));
        assert_eq!(out.text, "one\ntwo\nTHREE\nfour");
    }

    #[test]
    fn new_file_is_all_additions() {
        let patch = "*** path: src/new.rs\n@@\n+pub fn hi() {}\n+// done\n";
        let fp = &parse_patches(patch)[0];
        assert!(fp.is_new_file);
        let out = apply_file_patch("", fp);
        assert_eq!(out.text, "pub fn hi() {}\n// done");
        assert_eq!(out.hunks_failed, 0);
    }

    #[test]
    fn unlocatable_hunk_is_reported_not_guessed() {
        let orig = "alpha\nbeta\n";
        let patch = "*** path: x\n@@\n nonexistent context\n-gone\n+added\n";
        let out = apply_file_patch(orig, &parse_patches(patch)[0]);
        assert_eq!(out.hunks_failed, 1);
        assert_eq!(out.hunks_applied, 0);
        assert_eq!(out.text, "alpha\nbeta", "file untouched when a hunk can't be placed");
    }

    #[test]
    fn standard_unified_diff_headers_parse() {
        let t = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n use x;\n-old\n+new\n";
        let fps = parse_patches(t);
        assert_eq!(fps.len(), 1);
        assert_eq!(fps[0].path, "src/lib.rs");
    }

    #[test]
    fn multiple_files_in_one_reply() {
        let t = "prose here\n*** path: a.rs\n@@\n x\n-1\n+2\nblah\n*** path: b.rs\n@@\n+new file\n";
        let fps = parse_patches(t);
        assert_eq!(fps.len(), 2);
        assert_eq!(fps[0].path, "a.rs");
        assert_eq!(fps[1].path, "b.rs");
        assert!(fps[1].is_new_file);
    }

    #[test]
    fn a_failed_hunk_reports_what_it_expected_not_just_a_count() {
        // A bare failure count tells the model something broke but not what to look at, so the retry is a
        // guess. The expected context has to come back.
        let original = "fn a() {}\nfn b() {}\n";
        let patch = "*** path: src/x.rs\n@@\n fn NOPE() {}\n-fn gone() {}\n+fn added() {}\n";
        let fps = parse_patches(patch);
        let out = apply_file_patch(original, &fps[0]);
        assert_eq!(out.hunks_failed, 1);
        assert!(!out.is_clean());
        assert_eq!(out.failures.len(), 1);
        assert_eq!(out.failures[0].index, 0);
        assert!(out.failures[0].expected.iter().any(|l| l.contains("NOPE")),
                "the unmatched context is carried back: {:?}", out.failures[0].expected);
        let r = out.report();
        assert_eq!(r.len(), 1);
        assert!(r[0].contains("hunk 1 did not apply"), "{}", r[0]);
    }

    #[test]
    fn strict_apply_refuses_to_half_patch_a_file() {
        // The dangerous case: one hunk lands, one does not. Partial application leaves a file that may
        // still COMPILE and is wrong — discovered much later. Strict mode returns the original untouched.
        let original = "one\ntwo\nthree\n";
        let patch = "*** path: x\n@@\n one\n+inserted\n@@\n missing-context\n+never\n";
        let fps = parse_patches(patch);
        let loose = apply_file_patch(original, &fps[0]);
        assert!(loose.hunks_applied >= 1 && loose.hunks_failed >= 1, "the setup must be a PARTIAL apply");
        assert_ne!(loose.text, original, "loose mode does modify the file");

        let strict = apply_file_patch_strict(original, &fps[0]);
        assert_eq!(strict.text, original, "strict mode leaves the file untouched when any hunk fails");
        assert_eq!(strict.hunks_failed, loose.hunks_failed, "but still reports what failed");
        assert!(!strict.failures.is_empty());
    }

    #[test]
    fn a_clean_apply_is_marked_clean_and_reports_nothing() {
        let original = "alpha\nbeta\n";
        let patch = "*** path: x\n@@\n alpha\n-beta\n+gamma\n";
        let fps = parse_patches(patch);
        let out = apply_file_patch(original, &fps[0]);
        assert!(out.is_clean());
        assert!(out.report().is_empty(), "nothing to say when everything landed");
        assert!(out.text.contains("gamma"));
        // and strict agrees with loose when clean
        assert_eq!(apply_file_patch_strict(original, &fps[0]).text, out.text);
    }

    #[test]
    fn two_hunks_apply_in_order() {
        let orig = "a\nb\nc\nd\ne\nf\n";
        let patch = "*** path: x\n@@\n a\n-b\n+B\n@@\n e\n-f\n+F\n";
        let out = apply_file_patch(orig, &parse_patches(patch)[0]);
        assert_eq!((out.hunks_applied, out.hunks_failed), (2, 0));
        assert_eq!(out.text, "a\nB\nc\nd\ne\nF");
    }
}
