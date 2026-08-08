//! framediag.rs — one binary that answers every open question about submit structure.
//!
//! Built after a correction worth recording: the operator's cost is the ROUND TRIP — download,
//! unpack, run, read — and mine is nothing. Shipping one careful print per turn optimises my
//! resource and spends theirs. A batch of independently-failing probes costs one download and
//! answers several questions, and the ones that fail do not stop the ones that do not.
//!
//! The live question: on a dense resident Qwen3-4B, 39% of submits ran with NO FRAME OPEN even
//! though `decode_step_q8` calls `begin_frame()` on its first line. So the frame is dropped mid-step.
//! These probes find where, without another round trip per hypothesis.

use std::sync::atomic::{AtomicU64, AtomicBool, Ordering::Relaxed};

/// Per-callsite tally of frameless dispatches.
///
/// The counter that already exists says 6821 happened; it does not say WHERE. A tally by callsite
/// name turns "the frame is dropped somewhere" into a list, in one run.
pub static FRAMELESS_BY_SITE: std::sync::Mutex<Option<std::collections::BTreeMap<&'static str, u64>>>
    = std::sync::Mutex::new(None);

/// Armed by `--frame-diag` or `LOGSCOPE_FRAME_DIAG=1`. Off by default because the tally takes a lock
/// on the dispatch path, which is exactly the kind of instrumentation that must not be always-on.
pub static ARMED: AtomicBool = AtomicBool::new(false);

/// How many times the frame was opened, closed, and closed while already closed.
///
/// **The third counter is the interesting one.** A close with no frame open means something is
/// calling `end_frame` speculatively, and every dispatch after it runs frameless until the next
/// `begin_frame` that is not similarly wasted.
pub static FRAME_OPENED: AtomicU64 = AtomicU64::new(0);
pub static FRAME_CLOSED: AtomicU64 = AtomicU64::new(0);
pub static FRAME_CLOSED_WHILE_CLOSED: AtomicU64 = AtomicU64::new(0);
/// Reopen attempts that DECLINED because `had_frame` was false.
pub static REOPEN_DECLINED: AtomicU64 = AtomicU64::new(0);

pub fn arm() {
    ARMED.store(true, Relaxed);
    *FRAMELESS_BY_SITE.lock().unwrap() = Some(Default::default());
    *BY_PTR.lock().unwrap() = Some(Default::default());
}

/// Record a frameless dispatch by pipeline pointer.
///
/// Stable per kernel for a run, which is all a "which site dominates" question needs — and it avoids
/// threading a label through every dispatch call site. The SHAPE of the distribution is the answer;
/// the labels would only make it prettier.
pub fn note_frameless_ptr(p: usize) {
    if !ARMED.load(Relaxed) { return }
    if let Ok(mut g) = BY_PTR.lock() {
        if let Some(m) = g.as_mut() { *m.entry(p).or_insert(0) += 1; }
    }
}

pub static BY_PTR: std::sync::Mutex<Option<std::collections::BTreeMap<usize, u64>>>
    = std::sync::Mutex::new(None);

/// Record a frameless dispatch by name, for call sites that have one.
pub fn note_frameless(site: &'static str) {
    if !ARMED.load(Relaxed) { return }
    if let Ok(mut g) = FRAMELESS_BY_SITE.lock() {
        if let Some(m) = g.as_mut() { *m.entry(site).or_insert(0) += 1; }
    }
}

/// The report.
///
/// Ordered by count, because the question is which site dominates — a long tail of ones is a
/// different diagnosis from one site with six thousand.
pub fn report() -> String {
    let g = FRAMELESS_BY_SITE.lock().ok();
    let mut lines = vec![format!(
        "frames: {} opened, {} closed, {} closed-while-already-closed, {} reopens declined",
        FRAME_OPENED.load(Relaxed), FRAME_CLOSED.load(Relaxed),
        FRAME_CLOSED_WHILE_CLOSED.load(Relaxed), REOPEN_DECLINED.load(Relaxed))];

    // A close while already closed means something calls end_frame speculatively, and every dispatch
    // after it is frameless until a begin_frame that is not itself wasted. That is a different bug
    // from a frame that is never opened, and the counts distinguish them without a theory.
    if FRAME_CLOSED_WHILE_CLOSED.load(Relaxed) > 0 {
        lines.push("  a frame was closed while already closed — something calls end_frame \
speculatively, and everything after it runs frameless until the next begin_frame".into());
    }
    if REOPEN_DECLINED.load(Relaxed) > 0 {
        lines.push(format!("  {} reopens declined because had_frame was false — if decode's frame \
was already closed by something earlier, download's reopen correctly refuses and every subsequent \
dispatch is frameless. That would make download the SYMPTOM rather than the cause",
            REOPEN_DECLINED.load(Relaxed)));
    }
    if let Ok(bp) = BY_PTR.lock() {
        if let Some(m) = bp.as_ref() {
            if !m.is_empty() {
                let mut v: Vec<_> = m.values().copied().collect();
                v.sort_unstable_by(|a, b| b.cmp(a));
                let total: u64 = v.iter().sum();
                lines.push(format!("frameless dispatches: {total} across {} distinct pipelines; \
top three {:?}", v.len(), &v[..3.min(v.len())]));
                if v[0] * 2 > total {
                    lines.push("  ONE PIPELINE DOMINATES — that kernel's dispatch path is the one \
running outside a frame".into());
                }
            }
        }
    }
    match g.as_deref().and_then(|o| o.as_ref()) {
        Some(m) if !m.is_empty() => {
            let mut v: Vec<_> = m.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1));
            lines.push("frameless dispatches by site:".into());
            for (site, n) in v.iter().take(12) { lines.push(format!("  {n:>7}  {site}")); }
            // A long tail of ones is a different diagnosis from one site with six thousand, and the
            // shape says which without anyone forming a hypothesis first.
            let top = *v[0].1;
            let total: u64 = v.iter().map(|(_, n)| **n).sum();
            lines.push(if top * 2 > total {
                format!("  ONE SITE DOMINATES ({top} of {total}) — fix that one")
            } else {
                format!("  spread across {} sites ({total} total) — the frame is not being held at \
all, rather than dropped at one place", v.len())
            });
        }
        // Only claim the frame held if BOTH tallies are empty. The first version printed "one
        // pipeline dominates" and "the frame held for the whole run" in the same report, which is
        // the self-contradicting output this project has spent a session learning to distrust.
        Some(_) => {
            let ptr_clean = BY_PTR.lock().ok()
                .and_then(|g| g.as_ref().map(|m| m.is_empty())).unwrap_or(true);
            if ptr_clean {
                lines.push("no frameless dispatches recorded — the frame held for the whole run".into());
            }
        }
        None => lines.push("not armed; pass --frame-diag or set LOGSCOPE_FRAME_DIAG=1".into()),
    }
    lines.join("\n")
}

pub fn reset() {
    for c in [&FRAME_OPENED, &FRAME_CLOSED, &FRAME_CLOSED_WHILE_CLOSED, &REOPEN_DECLINED] {
        c.store(0, Relaxed);
    }
    if let Ok(mut g) = FRAMELESS_BY_SITE.lock() { if let Some(m) = g.as_mut() { m.clear(); } }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serialises the tests in this module.
    ///
    /// **They share process-global state** — the counters and both tallies are statics — and cargo
    /// runs tests in parallel by default, so one test's `reset()` can land inside another's
    /// assertions. That produced exactly one failure in ten full-suite runs while writing this,
    /// which is the worst frequency for a flake: often enough to be real, rare enough to be
    /// dismissed as noise the next time it appears.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn fresh() -> std::sync::MutexGuard<'static, ()> {
        // Poisoning is ignored: a panicking test already failed, and refusing to run the others
        // because of it turns one failure into eight.
        let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        arm();
        g
    }

    #[test]
    fn a_DOMINANT_site_is_called_out() {
        // The shape of the answer decides the fix, and the report says which shape it is rather than
        // leaving a reader to eyeball a list.
        let _serial = fresh();
        for _ in 0..100 { note_frameless("moe_ffn_v1 readback"); }
        note_frameless("sample_at");
        assert!(report().contains("ONE SITE DOMINATES"));
    }

    #[test]
    fn a_SPREAD_reads_as_the_frame_never_being_held() {
        // A different diagnosis from one bad site: if every site contributes, the frame is not being
        // held at all and fixing any one of them changes nothing.
        let _serial = fresh();
        for s in ["a", "b", "c", "d"] { for _ in 0..10 { note_frameless(s); } }
        let r = report();
        assert!(r.contains("not being held at all"), "{r}");
    }

    #[test]
    fn CLOSED_WHILE_CLOSED_is_explained_rather_than_just_counted() {
        // The counter most likely to name the bug: a speculative end_frame makes everything after it
        // frameless, which is a different fault from a frame never opened.
        let _serial = fresh();
        FRAME_CLOSED_WHILE_CLOSED.store(3, Relaxed);
        assert!(report().contains("calls end_frame speculatively"));
    }

    #[test]
    fn a_DECLINED_REOPEN_points_at_download_as_the_SYMPTOM() {
        // If decode's frame was already closed by something earlier, download's `had_frame` guard
        // correctly refuses to reopen — which would make download look guilty and not be.
        let _serial = fresh();
        REOPEN_DECLINED.store(7, Relaxed);
        assert!(report().contains("SYMPTOM rather than the cause"));
    }

    #[test]
    fn UNARMED_says_how_to_arm_rather_than_reporting_zero() {
        // Reporting "0 frameless dispatches" when the tally was never enabled is the false-green
        // shape this project keeps hitting.
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        *FRAMELESS_BY_SITE.lock().unwrap() = None;
        *BY_PTR.lock().unwrap() = None;
        ARMED.store(false, Relaxed);
        assert!(report().contains("not armed"));
    }

    #[test]
    fn armed_with_NO_hits_is_distinct_from_unarmed() {
        // "The frame held for the whole run" is a real result and must not be confused with "nobody
        // measured".
        let _serial = fresh();
        assert!(report().contains("held for the whole run"));
    }
}

#[cfg(test)]
mod flush_reopen_tests {
    /// The frame leak that produced decode's 67% no-frame, pinned so it cannot silently return.
    ///
    /// `flush_and_wait` did `self.frame.lock().unwrap().take()` and nothing put a frame back. It is
    /// called from `stream_expert_lins`, which runs PER LAYER — so every dispatch after the first
    /// expert stream in a decode step ran outside a frame, and each became its own submit.
    ///
    /// The measurements either side of the diagnosis:
    ///
    /// * Scout (streams experts): **67%** no-frame
    /// * dense Qwen3-4B (does not): **39%**, from downloads alone
    /// * a frame held deliberately, on hardware: **0**
    ///
    /// The third is what ruled out `begin_frame` being broken and left "something takes it and does
    /// not give it back" as the only remaining shape.
    #[test]
    fn flush_and_wait_REOPENS_the_frame_it_took() {
        let src = include_str!("lib.rs");
        let i = src.find("fn flush_and_wait(&self)").expect("flush_and_wait moved");
        let body = &src[i..(i + 1800).min(src.len())];
        assert!(body.contains("self.begin_frame();"),
            "flush_and_wait must restore the frame it took, or every dispatch after an expert \
stream runs frameless");
        assert!(body.contains("had_frame"),
            "and only when there WAS one — a caller that did not have a frame must not acquire one \
behind its back, which is the same rule download follows");
    }

    #[test]
    fn the_reopen_can_be_DISABLED_to_measure_the_difference() {
        // The fix is a behaviour change on the hottest path, and being able to turn it off is how
        // its effect gets measured rather than asserted.
        let src = include_str!("lib.rs");
        assert!(src.contains("LOGSCOPE_NO_FRAME_REOPEN"));
    }
}


#[cfg(test)]
mod second_leak_tests {
    /// The dense fix did not fix Scout, and the measurement said so in one run.
    ///
    /// ```text
    /// dense 1B    39% -> 0%    flush_and_wait fixed it
    /// Scout       67% -> 71%   unchanged
    /// ```
    ///
    /// Scout's per-layer path is `moe_ffn_v1` downloading router logits as u32, and
    /// `download_u32` took the frame without restoring it while `download` grew a reopen. Two
    /// siblings, one fixed — the fifth time this codebase has produced that shape.
    #[test]
    fn download_u32_REOPENS_the_frame_like_its_sibling() {
        let src = include_str!("lib.rs");
        let i = src.find("pub fn download_u32").expect("moved");
        // To the NEXT fn, not a fixed window: download_u32 is longer than 4000 chars and my first
        // version searched past its start and stopped before its end, which reported a reopen that
        // is there as absent.
        let body = &src[i..i + src[i + 10..].find("\n    pub fn ").map(|n| n + 10).unwrap_or(4000)];
        assert!(body.contains("had_frame_u32"), "download_u32 must record whether a frame existed");
        assert!(body.contains("self.begin_frame();"),
            "and restore it — Scout's 71% no-frame came from this function alone");
    }

    #[test]
    fn BOTH_download_paths_are_symmetric() {
        // The failure mode is one path growing a fix the other does not. Asserting both have it is
        // cheaper than remembering to check.
        let src = include_str!("lib.rs");
        for f in ["pub fn download_u32", "fn flush_and_wait(&self)"] {
            let i = src.find(f).unwrap_or_else(|| panic!("{f} moved"));
            let end = i + src[i + 10..].find("\n    pub fn ").map(|n| n + 10).unwrap_or(4000);
            assert!(src[i..end.min(src.len())].contains("begin_frame"), "{f} does not reopen");
        }
    }
}

#[cfg(test)]
mod leak_guard {
    /// Every `flush_and_wait` on a hot path must reopen the frame it closed.
    ///
    /// Two leaks were found by reading, a week apart in effect: the dims-ring wrap, and expert
    /// streaming at 480 calls per token. Both were `flush_and_wait()` with no reopen, both cost
    /// two thirds of all submits, and both were invisible because **the frame was correctly opened
    /// at the top of the decode step** — so every obvious check passed.
    ///
    /// A grep is a crude test and it is the right shape here: the failure is a call site MISSING a
    /// neighbouring line, which no type or trait can express.
    #[test]
    fn every_flush_and_wait_is_followed_by_a_conditional_reopen() {
        let src = include_str!("lib.rs");
        let mut missing = Vec::new();
        for (n, line) in src.lines().enumerate() {
            if !line.trim().starts_with("self.flush_and_wait();") { continue }
            // The reopen must be within a few lines after, and guarded by had_frame.
            let after: String = src.lines().skip(n + 1).take(4).collect::<Vec<_>>().join("\n");
            if !(after.contains("had_frame") && after.contains("begin_frame")) {
                missing.push(n + 1);
            }
        }
        assert!(missing.is_empty(),
            "flush_and_wait at line(s) {missing:?} ends the frame without reopening it. Everything \
dispatched after runs on its own encoder until the next decode step — which is how 67% of submits \
came to have no frame open");
    }

    /// The reopen must be CONDITIONAL, not unconditional.
    ///
    /// A caller that was not in a frame must not acquire one behind its back — `download`'s comment
    /// establishes that and both new sites follow it. An unconditional `begin_frame` would leave a
    /// frame open across a caller that never wanted one, and the symptom would be work batched into
    /// a frame nobody flushes.
    #[test]
    fn the_reopens_are_guarded_by_had_frame() {
        let src = include_str!("lib.rs");
        let guarded = src.matches("if had_frame && std::env::var_os(\"LOGSCOPE_NO_FRAME_REOPEN\")").count();
        assert!(guarded >= 2, "expected both reopen sites to be guarded, found {guarded}");
    }
}
