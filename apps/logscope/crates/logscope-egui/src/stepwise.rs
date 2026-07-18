//! Stepwise generation: prompt the model to work in NUMBERED STEPS and pause at each boundary with a
//! `⏸ STEP k/n — summary` marker line. The stream watcher halts generation at the marker; the harness
//! auto-continues on the user's behalf (warm-KV resume via the prefix tree, so a pause costs ~nothing) —
//! and each pause is an injection point where fan-out results (a small model's side-task output, compile
//! errors, etc.) can be spliced in before the next step. The UI collects the markers into a checklist so
//! a many-pause generation reads as one coherent, guided flow.
//!
//! Pure logic (parsing, policy, checklist extraction) lives here and is unit-tested; the chat layer wires
//! it to the token stream and rendering.

/// The instruction block appended to the system prompt when stepwise mode is on. Kept short and
/// mechanical — models follow format contracts better than prose requests.
pub const STEPWISE_PROMPT: &str = "\n\nSTEPWISE PROTOCOL: Structure your work as a short numbered plan (2-9 steps), state the plan first, \
then do ONE step at a time. After completing each step, output on its own line exactly:\n\
⏸ STEP <k>/<n> — <one-line summary of what you just did>\n\
and then STOP. You will be told to continue. Do not do more than one step between markers. \
On the final step, write your marker line and then TASK_COMPLETE.";

/// A parsed step marker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepMark { pub k: u32, pub n: u32, pub summary: String }

/// Find a completed step marker in `text` (scans from the end — the freshest marker wins).
/// Marker grammar: a line `⏸ STEP k/n — summary` (also tolerates `STEP k/n:` and ASCII dashes,
/// since models drift on punctuation).
pub fn last_step_mark(text: &str) -> Option<StepMark> {
    for line in text.lines().rev() {
        let l = line.trim();
        let body = l.strip_prefix('⏸').map(str::trim).unwrap_or(l);
        let rest = match body.strip_prefix("STEP ").or_else(|| body.strip_prefix("Step ")) {
            Some(r) => r,
            None => continue,
        };
        // k/n then a separator (— , - , :) then summary
        let (kn, tail) = match rest.split_once(|c: char| c == '—' || c == '-' || c == ':') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (rest.trim(), ""),
        };
        let (ks, ns) = match kn.split_once('/') { Some(p) => p, None => continue };
        let (Ok(k), Ok(n)) = (ks.trim().parse::<u32>(), ns.trim().parse::<u32>()) else { continue };
        if k == 0 || n == 0 || k > n || n > 50 { continue; }
        return Some(StepMark { k, n, summary: tail.trim_start_matches('—').trim().to_string() });
    }
    None
}

/// Should the harness pause the stream now? True when the tail of the streamed text has a marker for a
/// step we haven't already paused on (dedup by `last_paused` step number).
pub fn should_pause(text: &str, last_paused: u32) -> Option<StepMark> {
    let m = last_step_mark(text)?;
    if m.k > last_paused { Some(m) } else { None }
}

/// Auto-continue policy after a pause: continue while steps remain, under a hard cap so a confused model
/// can't loop the harness forever. Final step (k==n) or TASK_COMPLETE in the text ends the run.
pub fn should_continue(mark: &StepMark, text: &str, continues_so_far: u32, cap: u32) -> bool {
    mark.k < mark.n && continues_so_far < cap && !text.contains("TASK_COMPLETE")
}

/// Extract the checklist for rendering: every step marker in order, plus whether the run is finished.
/// The UI shows these as a compact steps guide; the marker lines can then be de-emphasized in the body.
pub fn checklist(text: &str) -> (Vec<StepMark>, bool) {
    let mut steps: Vec<StepMark> = Vec::new();
    for line in text.lines() {
        if let Some(m) = last_step_mark(line) {
            if steps.last().map_or(true, |p| m.k > p.k) { steps.push(m); }
        }
    }
    let done = text.contains("TASK_COMPLETE")
        || steps.last().map_or(false, |m| m.k == m.n);
    (steps, done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_marker_variants() {
        let m = last_step_mark("did things\n⏸ STEP 2/5 — wrote the parser\nmore").unwrap();
        assert_eq!((m.k, m.n), (2, 5));
        assert_eq!(m.summary, "wrote the parser");
        // punctuation drift: colon, ascii dash, no pause glyph
        assert_eq!(last_step_mark("STEP 3/4: ran tests").unwrap().k, 3);
        assert_eq!(last_step_mark("Step 1/9 - scaffolded crate").unwrap().n, 9);
        // rejects nonsense
        assert!(last_step_mark("STEP 7/3 — impossible").is_none());
        assert!(last_step_mark("STEP x/y — nope").is_none());
        assert!(last_step_mark("no markers here").is_none());
    }

    #[test]
    fn pause_dedups_and_policy_caps() {
        let text = "plan...\n⏸ STEP 1/3 — set up crate\n";
        let m = should_pause(text, 0).unwrap();
        assert_eq!(m.k, 1);
        assert!(should_pause(text, 1).is_none(), "already paused on step 1");
        assert!(should_continue(&m, text, 0, 12), "steps remain → continue");
        let last = StepMark { k: 3, n: 3, summary: String::new() };
        assert!(!should_continue(&last, text, 0, 12), "final step → stop");
        assert!(!should_continue(&m, text, 12, 12), "cap reached → stop");
        assert!(!should_continue(&m, "…TASK_COMPLETE", 0, 12), "explicit completion → stop");
    }

    #[test]
    fn checklist_collects_in_order() {
        let text = "plan\n⏸ STEP 1/3 — a\nbody\n⏸ STEP 2/3 — b\nbody\n⏸ STEP 3/3 — c\nTASK_COMPLETE";
        let (steps, done) = checklist(text);
        assert_eq!(steps.len(), 3);
        assert!(done);
        assert_eq!(steps[1].summary, "b");
    }
}
