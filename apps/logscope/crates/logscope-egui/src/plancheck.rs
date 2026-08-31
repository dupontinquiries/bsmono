//! plancheck — decide whether a plan step actually HAPPENED, from what the run wrote.
//!
//! ## The bug this replaces
//!
//! The plan was advanced by `Plan::advance()`, which marked the next not-done step done at the end
//! of every turn that made any tool call at all. Three read-only turns against a three-step plan
//! reported 3/3. That number is shown in the HUD, and since transcript compaction it is also
//! written into the digest that replaces elided turns — so a fabricated completion count became
//! "fact" in the model's own context, and the run could be told it had finished work it had not
//! started.
//!
//! ## The rule
//!
//! A step is done when a file it NAMES was successfully written or edited. Nothing else counts —
//! not a read of that file, not the passage of a turn.
//!
//! A step that names no file cannot be confirmed this way, and the honest answer is to say so
//! rather than to guess in either direction: those become [`StepState::Unverifiable`], reported
//! separately from done and pending. "2 done, 1 pending, 2 unverifiable" is a useful thing to show
//! an operator; "5/5" was not.
//!
//! ## Layering
//!
//! Pure and evidence-shaped: `&str` in, decisions out. No workspace, no model, no egui — the same
//! seam `phase_guide` uses, and for the same reason: the matching rules will change (symbols,
//! renames, a step naming a test) and the loop that feeds them should not have to.

/// What is known about one plan step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    /// Names a target, and nothing has written it yet.
    Pending,
    /// A file this step names was written or edited.
    Done,
    /// Names no file, so no write can confirm it. NOT a failure — a plan step like "work out why
    /// the test hangs" is legitimate and simply cannot be checked by this rule.
    Unverifiable,
}

/// Path-like tokens a plan step names.
///
/// Deliberately conservative: a token counts only if it has a path separator or a known source
/// extension. Bare words are how a plan step reads ("update the parser"), and treating "parser" as
/// a path would match `src/parser.rs` by luck and `docs/parser-notes.md` by accident.
pub fn step_targets(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split_whitespace() {
        let t = raw.trim_matches(|c: char| !(c.is_alphanumeric() || c == '/' || c == '.' || c == '_' || c == '-'));
        if t.is_empty() || t.len() < 3 { continue; }
        let has_sep = t.contains('/');
        let has_ext = [".rs", ".toml", ".md", ".wgsl", ".py", ".json", ".yaml", ".yml", ".sh", ".c", ".h", ".cpp", ".js", ".ts"]
            .iter().any(|e| t.ends_with(e));
        if (has_sep || has_ext) && !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// Does `written` (a workspace-relative path a tool actually wrote) satisfy a target named in a
/// plan step?
///
/// Matched from the RIGHT, on whole path components. A plan says "update src/lib.rs" while the tool
/// writes "src/lib.rs", but a plan may also say "crates/foo/src/lib.rs" for the same file, or just
/// "lib.rs". Suffix matching on components handles all three without matching "mylib.rs" against
/// "lib.rs", which a plain `ends_with` would.
pub fn target_matches(target: &str, written: &str) -> bool {
    let norm = |s: &str| -> Vec<String> {
        s.replace('\\', "/").split('/')
            .filter(|c| !c.is_empty() && *c != ".")
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    let (t, w) = (norm(target), norm(written));
    if t.is_empty() || w.is_empty() { return false; }
    // The shorter one must be a component-wise suffix of the longer.
    let (short, long) = if t.len() <= w.len() { (&t, &w) } else { (&w, &t) };
    long[long.len() - short.len()..] == short[..]
}

/// Classify one step against every path the run has written so far.
pub fn state_of(step_text: &str, written: &[String]) -> StepState {
    let targets = step_targets(step_text);
    if targets.is_empty() { return StepState::Unverifiable; }
    if targets.iter().any(|t| written.iter().any(|w| target_matches(t, w))) {
        StepState::Done
    } else {
        StepState::Pending
    }
}

/// A one-line summary for the HUD and the compaction digest: done / pending / unverifiable.
///
/// Reports all three because collapsing them is what produced the original lie. An operator reading
/// "2/5" wants to know whether the other three are outstanding or merely uncheckable.
pub fn summarize(states: &[StepState]) -> String {
    let done = states.iter().filter(|s| **s == StepState::Done).count();
    let pending = states.iter().filter(|s| **s == StepState::Pending).count();
    let unver = states.iter().filter(|s| **s == StepState::Unverifiable).count();
    let mut s = format!("{done}/{} verified", states.len());
    if pending > 0 { s.push_str(&format!(", {pending} pending")); }
    if unver > 0 { s.push_str(&format!(", {unver} unverifiable")); }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_paths_not_every_noun() {
        assert_eq!(step_targets("rewrite src/a.rs and update Cargo.toml"),
                   vec!["src/a.rs".to_string(), "Cargo.toml".to_string()]);
        // bare words are not paths, however file-ish they sound
        assert!(step_targets("update the parser and the renderer").is_empty());
        assert!(step_targets("figure out why the test hangs").is_empty());
        // punctuation around a path does not hide it
        assert_eq!(step_targets("1. edit `src/lib.rs`, then stop"), vec!["src/lib.rs".to_string()]);
    }

    #[test]
    fn matching_is_component_wise_from_the_right() {
        assert!(target_matches("src/lib.rs", "src/lib.rs"));
        assert!(target_matches("lib.rs", "crates/foo/src/lib.rs"), "a bare filename matches its file");
        assert!(target_matches("crates/foo/src/lib.rs", "src/lib.rs"), "and the long form matches the short");
        assert!(!target_matches("lib.rs", "src/mylib.rs"), "component-wise, so mylib.rs is not lib.rs");
        assert!(!target_matches("src/a.rs", "src/b.rs"));
        assert!(!target_matches("foo/lib.rs", "bar/lib.rs"), "the parent component still has to agree");
    }

    #[test]
    fn a_step_is_done_only_when_something_it_names_was_written() {
        let written = vec!["src/a.rs".to_string()];
        assert_eq!(state_of("rewrite src/a.rs", &written), StepState::Done);
        assert_eq!(state_of("rewrite src/b.rs", &written), StepState::Pending);
        // reading is not writing: `written` is the write feed, so a read simply never appears here
        assert_eq!(state_of("read src/b.rs and understand it", &written), StepState::Pending);
        assert_eq!(state_of("work out why the build hangs", &written), StepState::Unverifiable);
    }

    #[test]
    fn the_summary_keeps_the_three_states_apart() {
        let s = summarize(&[StepState::Done, StepState::Pending, StepState::Unverifiable, StepState::Done]);
        assert_eq!(s, "2/4 verified, 1 pending, 1 unverifiable");
        assert_eq!(summarize(&[StepState::Done]), "1/1 verified");
        assert_eq!(summarize(&[]), "0/0 verified");
    }
}
