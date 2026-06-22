//! snapclone-gstpipeline — the pure decision logic of GstLaunchBackend.
//!
//! The Qt `GstLaunchBackend` runs `gst-launch` as a subprocess and reacts to
//! its stderr and exit. The *reactions* — classify the error, decide the next
//! recovery step, manage the per-session retry guards — are pure logic that was
//! tangled up with `QProcess`/`QTimer`. This crate extracts that logic so it can
//! be unit-tested, and in particular so the FP5 "not-negotiated on resume" bug
//! (a per-session guard reset under the wrong condition) is locked down by a
//! test and can't silently come back.
//!
//! It deliberately models NO GStreamer or hardware: real pipewire/libcamera
//! negotiation is a device interaction, and a fake of it would only prove what
//! we told it to. What's faithfully modelled is the C++ control flow:
//!   * [`classify_error`] — stderr text → [`GstError`] category.
//!   * [`RecoverySm`] — the negotiation-failure ladder (settle → relax → swap)
//!     and the session-guard lifecycle, including the resume-reset fix.

#![forbid(unsafe_op_in_unsafe_fn)]

pub mod ffi;

/// The category of a gst-launch failure, derived from its stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GstError {
    /// pipewiresrc/libcamera couldn't agree a format — transient device-state
    /// on the FP5 (the same node streams fine moments before/after).
    Negotiation,
    /// The GL colorspace pipeline failed to link — this device's GL stack can't
    /// do the path; the fix is to force the CPU path, not swap cameras.
    GlLink,
    /// The requested source node doesn't exist / wasn't found.
    MissingNode,
    /// Anything else.
    Other,
}

/// Classify gst-launch stderr. Order matters: GL-link and missing-node are more
/// specific than the generic negotiation strings, so they're checked the way
/// the C++ checks them (GL handled before the negotiation ladder; missing-node
/// is a distinct case in the early-exit branch).
pub fn classify_error(stderr: &str) -> GstError {
    let has = |needle: &str| stderr.contains(needle);
    if has("could not link gldownload")
        || has("could not link glupload")
        || has("could not link glcolorconvert")
        || has("could not link glcolorscale")
    {
        return GstError::GlLink;
    }
    if has("not-negotiated")
        || has("Could not negotiate")
        || has("Format negotiation failed")
    {
        return GstError::Negotiation;
    }
    if has("no element named")
        || has("No matching node")
        || has("Failed to find target")
        || has("could not be found")
    {
        return GstError::MissingNode;
    }
    GstError::Other
}

/// What the backend should do next after a failure. The Qt layer turns these
/// into real actions (schedule a timed `start()`, flip a caps flag, pick
/// another node, or give up).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Force the CPU colorspace path and retry the SAME node after `delay_ms`.
    ForceCpuAndRetry { delay_ms: u32 },
    /// Wait a settle window, retry the SAME node with NORMAL caps.
    SettleRetry { delay_ms: u32 },
    /// Retry the SAME node with RELAXED caps (no width/height pin).
    RelaxedRetry { delay_ms: u32 },
    /// V32.37+: keep retrying the SAME node with a growing backoff (the sensor's
    /// node hasn't settled yet but demonstrably comes good with more time).
    /// `attempt` is 1-based for logging; `delay_ms` is the (capped) wait.
    BackoffRetry { delay_ms: u32, attempt: u32 },
    /// Swap to a different node.
    SwapNode,
    /// Nothing left to try — surface the failure.
    GiveUp,
}

/// Tunables (mirror the C++ constants).
#[derive(Debug, Clone, Copy)]
pub struct RecoveryConfig {
    pub settle_ms: u32,
    pub gpu_retry_ms: u32,
    /// V32.37+: after the settle + relaxed retries fail, how many MORE times to
    /// retry the SAME node with a growing backoff before giving up / swapping.
    /// Device logs (FP5 camera_29) show the sensor's pipewire node sometimes
    /// needs several seconds to settle after a cold start — three rapid attempts
    /// in ~1.5s all fail, but the same node works ~14s later. A bounded backoff
    /// bridges that gap instead of prematurely swapping to the wrong camera.
    pub backoff_max_retries: u32,
    /// Base delay for the first backoff retry (ms); each retry multiplies by 2,
    /// capped at backoff_cap_ms.
    pub backoff_base_ms: u32,
    pub backoff_cap_ms: u32,
}
impl Default for RecoveryConfig {
    fn default() -> Self {
        RecoveryConfig {
            settle_ms: 600,
            gpu_retry_ms: 100,
            backoff_max_retries: 4,    // ~ 1.2 + 2.4 + 4.8 + 4.8 ≈ 13s of bridging
            backoff_base_ms: 1200,
            backoff_cap_ms: 4800,
        }
    }
}

/// The per-session recovery state machine. One instance models one
/// `GstLaunchBackend`'s guard flags + active node. The Qt code's scattered
/// member bools (`m_negotiationSettleRetried`, `m_retriedRelaxed`, `m_relaxCaps`,
/// `m_gpuConvertFailed`, `m_isRecoveryRestart`, `m_currentNode`, `m_active`) map
/// 1:1 onto this struct, so the logic can be checked without Qt.
#[derive(Debug, Clone)]
pub struct RecoverySm {
    cfg: RecoveryConfig,
    active: bool,
    current_node: Option<String>,
    // per-session guards
    settle_retried: bool,
    relaxed_retried: bool,
    relax_caps: bool,
    gpu_failed: bool,
    /// V32.37+: how many backoff retries we've already issued this session.
    backoff_count: u32,
    // one-shot: "the next start() is our own recovery relaunch, not fresh"
    recovery_restart: bool,
}

impl Default for RecoverySm {
    fn default() -> Self {
        Self::new(RecoveryConfig::default())
    }
}

impl RecoverySm {
    pub fn new(cfg: RecoveryConfig) -> Self {
        RecoverySm {
            cfg,
            active: false,
            current_node: None,
            settle_retried: false,
            relaxed_retried: false,
            relax_caps: false,
            gpu_failed: false,
            backoff_count: 0,
            recovery_restart: false,
        }
    }

    // ── observable state (for tests + FFI) ──────────────────────────────────
    pub fn relax_caps(&self) -> bool { self.relax_caps }
    pub fn gpu_failed(&self) -> bool { self.gpu_failed }
    pub fn settle_retried(&self) -> bool { self.settle_retried }
    pub fn relaxed_retried(&self) -> bool { self.relaxed_retried }
    pub fn is_active(&self) -> bool { self.active }

    /// Model of `start(node)`. Mirrors the C++ ordering: consume the
    /// recovery-restart one-shot, then — for a FRESH session (start while
    /// inactive and not our own recovery relaunch) — reset the per-session
    /// guards. THIS reset is the fix: it fires on a fresh session regardless of
    /// whether the node changed, so resuming the same sensor no longer inherits
    /// a stale `settle_retried` that would suppress the settle-retry.
    pub fn on_start(&mut self, node: &str) {
        let recovery_restart = self.recovery_restart;
        self.recovery_restart = false;

        let node_changed = self.current_node.as_deref() != Some(node);

        // node-change resets (gpu latch + settle guard), as in the C++.
        if node_changed {
            self.gpu_failed = false;
            self.settle_retried = false;
            self.backoff_count = 0;
        }
        // FRESH-session reset (the fix): fires for a genuine new session even on
        // the same node, but NOT during our own recovery relaunches.
        if !self.active && !recovery_restart {
            self.settle_retried = false;
            self.relaxed_retried = false;
            self.relax_caps = false;
            self.backoff_count = 0;
        }

        // relaxed-caps bookkeeping: a fresh start (not our own relaxed re-entry)
        // begins un-relaxed. Our relaxed retry sets recovery_restart + relax_caps
        // and keeps relax_caps true through here.
        if !(self.relax_caps && recovery_restart) {
            // leave relax_caps as set by the reset logic above
        }

        self.current_node = Some(node.to_string());
        self.active = true;
    }

    /// Model of `stop()`. Note: like the C++, this does NOT clear
    /// `current_node` — that asymmetry is exactly why the resume bug existed and
    /// why the fresh-session reset in `on_start` (not a node-change reset) is
    /// what fixes it.
    pub fn on_stop(&mut self) {
        self.active = false;
    }

    /// Model of the finished-handler decision. Given the failure category and
    /// whether this was an early exit (within the early-exit window), return the
    /// action and mutate the guards exactly as the C++ does. The caller then
    /// performs the action (and for the retry actions, the subsequent `on_start`
    /// will see `recovery_restart == true`).
    pub fn on_failure(&mut self, err: GstError, early_exit: bool) -> RecoveryAction {
        self.active = false;

        // GPU-link failure is handled before the negotiation ladder and is not
        // gated on early_exit (it can fail at link time either way).
        if err == GstError::GlLink && !self.gpu_failed {
            self.gpu_failed = true;
            self.recovery_restart = true;
            return RecoveryAction::ForceCpuAndRetry { delay_ms: self.cfg.gpu_retry_ms };
        }

        // The negotiation ladder lives in the early-exit branch in the C++.
        if early_exit && err == GstError::Negotiation {
            if !self.settle_retried {
                self.settle_retried = true;
                self.recovery_restart = true;
                return RecoveryAction::SettleRetry { delay_ms: self.cfg.settle_ms };
            }
            if !self.relax_caps && !self.relaxed_retried {
                self.relaxed_retried = true;
                self.relax_caps = true;
                self.recovery_restart = true;
                return RecoveryAction::RelaxedRetry { delay_ms: self.cfg.settle_ms };
            }
            // V32.37+: settle + relaxed both failed. Before swapping to another
            // camera (which on a 2-sensor phone means showing the WRONG one),
            // keep retrying the SAME node with a growing backoff — the device log
            // shows the node settles within ~10-15s and then works first try.
            if self.backoff_count < self.cfg.backoff_max_retries {
                self.backoff_count += 1;
                self.recovery_restart = true;
                // exponential: base * 2^(n-1), capped.
                let mult = 1u32.checked_shl(self.backoff_count - 1).unwrap_or(u32::MAX);
                let delay = self.cfg.backoff_base_ms
                    .saturating_mul(mult)
                    .min(self.cfg.backoff_cap_ms);
                return RecoveryAction::BackoffRetry { delay_ms: delay, attempt: self.backoff_count };
            }
            return RecoveryAction::SwapNode;
        }

        // Any other early exit → try another node (e.g. missing node).
        if early_exit {
            return RecoveryAction::SwapNode;
        }

        RecoveryAction::GiveUp
    }
}

/// Frame coalescing decision (mirrors GstLaunchBackend::onStdoutData).
///
/// fdsink writes raw frames back-to-back; if the GUI thread falls behind, more
/// than one whole frame buffers up. A live preview only wants the NEWEST, so we
/// skip the stale ones. Given the current buffer size and the per-frame slice
/// size, returns how many leading frames to DROP without decoding, and how many
/// remain to process (always 0 or 1 whole frame to actually render, plus a
/// possible partial that stays buffered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoalescePlan {
    /// Whole frames to discard cheaply (no decode/emit).
    pub skip: u64,
    /// Whole frames to actually decode + emit (0 or 1).
    pub process: u64,
    /// Bytes of a trailing partial frame left in the buffer for next time.
    pub remainder: u64,
}

/// Compute the coalescing plan. `slice_bytes` must be > 0 (a known frame size);
/// if it's 0 (frame geometry not yet known) we can't slice, so nothing is done.
pub fn coalesce_plan(buf_len: u64, slice_bytes: u64) -> CoalescePlan {
    if slice_bytes == 0 {
        return CoalescePlan { skip: 0, process: 0, remainder: buf_len };
    }
    let whole = buf_len / slice_bytes;
    let remainder = buf_len % slice_bytes;
    match whole {
        0 => CoalescePlan { skip: 0, process: 0, remainder },
        1 => CoalescePlan { skip: 0, process: 1, remainder },
        n => CoalescePlan { skip: n - 1, process: 1, remainder },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── error classification ────────────────────────────────────────────────

    #[test]
    fn classify_negotiation() {
        assert_eq!(classify_error(
            "streaming stopped, reason not-negotiated (-4)"), GstError::Negotiation);
        assert_eq!(classify_error("Format negotiation failed"), GstError::Negotiation);
        assert_eq!(classify_error("Could not negotiate format"), GstError::Negotiation);
    }

    #[test]
    fn classify_gl_link() {
        assert_eq!(classify_error(
            "could not link glupload0 to glcolorconvert0"), GstError::GlLink);
    }

    #[test]
    fn classify_missing_node() {
        assert_eq!(classify_error("Failed to find target object"), GstError::MissingNode);
        assert_eq!(classify_error("no element named pipewiresrc"), GstError::MissingNode);
    }

    #[test]
    fn gl_link_beats_negotiation_when_both_present() {
        // GL is the more specific cause; the C++ checks it first.
        let s = "could not link glcolorscale0 ... not-negotiated (-4)";
        assert_eq!(classify_error(s), GstError::GlLink);
    }

    #[test]
    fn classify_other() {
        assert_eq!(classify_error("some unrelated warning"), GstError::Other);
    }

    // ── the recovery ladder ──────────────────────────────────────────────────

    fn started() -> RecoverySm {
        let mut sm = RecoverySm::default();
        sm.on_start("camera_29");
        sm
    }

    #[test]
    fn first_negotiation_failure_settles() {
        let mut sm = started();
        assert_eq!(sm.on_failure(GstError::Negotiation, true),
                   RecoveryAction::SettleRetry { delay_ms: 600 });
        assert!(sm.settle_retried());
    }

    #[test]
    fn ladder_settle_then_relax_then_backoff() {
        let mut sm = started();
        // 1st: settle
        assert_eq!(sm.on_failure(GstError::Negotiation, true),
                   RecoveryAction::SettleRetry { delay_ms: 600 });
        // the settle relaunch is our own recovery restart
        sm.on_start("camera_29");
        // 2nd: relaxed
        assert_eq!(sm.on_failure(GstError::Negotiation, true),
                   RecoveryAction::RelaxedRetry { delay_ms: 600 });
        assert!(sm.relax_caps());
        sm.on_start("camera_29");
        // 3rd: V32.37+ now a bounded backoff retry (was straight to swap) —
        // bridges the sensor-settle gap before abandoning the node.
        assert_eq!(sm.on_failure(GstError::Negotiation, true),
                   RecoveryAction::BackoffRetry { delay_ms: 1200, attempt: 1 });
    }

    #[test]
    fn gl_link_forces_cpu_and_retries_same_node() {
        let mut sm = started();
        assert_eq!(sm.on_failure(GstError::GlLink, true),
                   RecoveryAction::ForceCpuAndRetry { delay_ms: 100 });
        assert!(sm.gpu_failed());
        // a second GL failure won't loop forever — the latch is set.
        sm.on_start("camera_29");
        assert_ne!(sm.on_failure(GstError::GlLink, true),
                   RecoveryAction::ForceCpuAndRetry { delay_ms: 100 });
    }

    #[test]
    fn non_early_exit_gives_up() {
        let mut sm = started();
        assert_eq!(sm.on_failure(GstError::Negotiation, false), RecoveryAction::GiveUp);
    }

    // ── THE REGRESSION TEST: resume on the same sensor must settle-retry ──────

    #[test]
    fn resume_same_sensor_can_still_settle_retry() {
        // Reproduces the FP5 bug: a session uses up its settle-retry, the camera
        // is stopped (current_node NOT cleared, mirroring the C++), then the SAME
        // sensor is resumed. The resumed session must get a FRESH settle-retry —
        // before the fix, the guard stayed set and the retry was skipped, turning
        // a transient not-negotiated into a terminal failure.
        let mut sm = RecoverySm::default();
        sm.on_start("camera_29");
        // session 1 burns its settle-retry
        assert_eq!(sm.on_failure(GstError::Negotiation, true),
                   RecoveryAction::SettleRetry { delay_ms: 600 });
        assert!(sm.settle_retried());
        // ... eventually recovers / user navigates away
        sm.on_start("camera_29"); // the settle relaunch
        sm.on_stop();             // leave the camera page (current_node stays!)

        // RESUME the same sensor — fresh session.
        sm.on_start("camera_29");
        assert!(!sm.settle_retried(), "fresh session must reset the settle guard");
        // and a negotiation failure now must settle-retry again, not give up/swap.
        assert_eq!(sm.on_failure(GstError::Negotiation, true),
                   RecoveryAction::SettleRetry { delay_ms: 600 });
    }

    #[test]
    fn our_own_recovery_relaunch_does_not_reset_guards() {
        // The flip side: during our OWN settle/relax relaunch, the guards must
        // NOT reset (or we'd loop forever). on_failure sets recovery_restart;
        // the subsequent on_start must preserve settle_retried.
        let mut sm = started();
        sm.on_failure(GstError::Negotiation, true); // sets settle_retried + recovery_restart
        assert!(sm.settle_retried());
        sm.on_start("camera_29"); // our own relaunch — must NOT clear settle_retried
        assert!(sm.settle_retried(), "recovery relaunch must preserve the guard");
        // so the next failure escalates to relaxed, not another settle.
        assert_eq!(sm.on_failure(GstError::Negotiation, true),
                   RecoveryAction::RelaxedRetry { delay_ms: 600 });
    }

    #[test]
    fn node_change_also_resets_guards() {
        // Switching to the other camera (flip) is a fresh node → fresh guards.
        let mut sm = started();
        sm.on_failure(GstError::Negotiation, true);
        assert!(sm.settle_retried());
        sm.on_stop();
        sm.on_start("camera_10"); // different sensor
        assert!(!sm.settle_retried());
    }

    #[test]
    fn relax_caps_clears_on_fresh_session() {
        let mut sm = started();
        sm.on_failure(GstError::Negotiation, true); // settle
        sm.on_start("camera_29");
        sm.on_failure(GstError::Negotiation, true); // relax → relax_caps = true
        assert!(sm.relax_caps());
        sm.on_start("camera_29"); // recovery relaunch keeps relax
        sm.on_stop();
        sm.on_start("camera_29"); // fresh resume → relax cleared
        assert!(!sm.relax_caps(), "fresh session starts un-relaxed");
    }

    // ── negotiation backoff ladder (V32.37+) ────────────────────────────────

    #[test]
    fn ladder_settle_then_relaxed_then_backoff_then_swap() {
        let mut sm = RecoverySm::default();
        sm.on_start("camera_29");
        let a1 = sm.on_failure(GstError::Negotiation, true);
        assert!(matches!(a1, RecoveryAction::SettleRetry { .. }));
        sm.on_start("camera_29");
        let a2 = sm.on_failure(GstError::Negotiation, true);
        assert!(matches!(a2, RecoveryAction::RelaxedRetry { .. }));
        let mut delays = vec![];
        for n in 1..=4 {
            sm.on_start("camera_29");
            match sm.on_failure(GstError::Negotiation, true) {
                RecoveryAction::BackoffRetry { delay_ms, attempt } => {
                    assert_eq!(attempt, n);
                    delays.push(delay_ms);
                }
                other => panic!("expected BackoffRetry at {}, got {:?}", n, other),
            }
        }
        assert_eq!(delays, vec![1200, 2400, 4800, 4800]); // exp growth, capped
        sm.on_start("camera_29");
        assert_eq!(sm.on_failure(GstError::Negotiation, true), RecoveryAction::SwapNode);
    }

    #[test]
    fn backoff_resets_on_fresh_session() {
        let mut sm = RecoverySm::default();
        sm.on_start("camera_29");
        sm.on_failure(GstError::Negotiation, true); // settle
        sm.on_start("camera_29");
        sm.on_failure(GstError::Negotiation, true); // relaxed
        sm.on_start("camera_29");
        let a = sm.on_failure(GstError::Negotiation, true);
        assert!(matches!(a, RecoveryAction::BackoffRetry { attempt: 1, .. }));
        // the backoff relaunch fires on_start (consumes the recovery_restart
        // one-shot), the attempt fails again or the user navigates away…
        sm.on_start("camera_29");   // recovery relaunch
        // …then a GENUINE fresh session (stop + start, not a recovery relaunch)
        // resets the whole ladder back to the top.
        sm.on_stop();
        sm.on_start("camera_29");
        let a2 = sm.on_failure(GstError::Negotiation, true);
        assert!(matches!(a2, RecoveryAction::SettleRetry { .. }));
    }

    #[test]
    fn backoff_resets_on_node_change() {
        let mut sm = RecoverySm::default();
        sm.on_start("camera_29");
        sm.on_failure(GstError::Negotiation, true);
        sm.on_start("camera_29");
        sm.on_failure(GstError::Negotiation, true);
        sm.on_start("camera_29");
        sm.on_failure(GstError::Negotiation, true); // backoff 1 on camera_29
        sm.on_start("camera_10");                   // node change resets
        let a = sm.on_failure(GstError::Negotiation, true);
        assert!(matches!(a, RecoveryAction::SettleRetry { .. }));
    }

    #[test]
    fn device_log_scenario_cold_fail_then_fresh_success() {
        // Mirrors the FP5 camera_29 log: a cold start fails negotiation through
        // the whole ladder (settle → relaxed → backoff×N → swap-or-give-up), then
        // a genuine fresh session (the probe cycle / page revisit ~14s later)
        // starts at the TOP of the ladder again — so the next attempt gets a
        // clean settle-retry, exactly as observed when the sensor finally worked.
        let mut sm = RecoverySm::default();
        sm.on_start("camera_29");
        // walk the full ladder to exhaustion
        let mut acts = vec![sm.on_failure(GstError::Negotiation, true)];
        loop {
            sm.on_start("camera_29");
            let a = sm.on_failure(GstError::Negotiation, true);
            let done = matches!(a, RecoveryAction::SwapNode | RecoveryAction::GiveUp);
            acts.push(a);
            if done { break; }
        }
        // ladder shape: settle, relaxed, then 4 backoffs, then swap
        assert!(matches!(acts[0], RecoveryAction::SettleRetry { .. }));
        assert!(matches!(acts[1], RecoveryAction::RelaxedRetry { .. }));
        let backoffs = acts.iter().filter(|a| matches!(a, RecoveryAction::BackoffRetry { .. })).count();
        assert_eq!(backoffs, 4);
        assert!(matches!(acts.last().unwrap(), RecoveryAction::SwapNode));
        // now a genuine fresh session (stop + start, not a recovery relaunch)
        sm.on_stop();
        sm.on_start("camera_29");
        assert!(matches!(sm.on_failure(GstError::Negotiation, true),
                         RecoveryAction::SettleRetry { .. }),
                "after a real restart the ladder must begin fresh");
    }

    // ── frame coalescing ────────────────────────────────────────────────────

    #[test]
    fn coalesce_no_whole_frame_keeps_partial() {
        // half a frame buffered → nothing to process, all remains
        let p = coalesce_plan(500, 1000);
        assert_eq!(p, CoalescePlan { skip: 0, process: 0, remainder: 500 });
    }

    #[test]
    fn coalesce_exactly_one_frame_processes_it() {
        let p = coalesce_plan(1000, 1000);
        assert_eq!(p, CoalescePlan { skip: 0, process: 1, remainder: 0 });
    }

    #[test]
    fn coalesce_multiple_frames_skips_stale() {
        // 3 whole frames + a bit → skip 2 stale, process newest, keep partial
        let p = coalesce_plan(3 * 1000 + 250, 1000);
        assert_eq!(p, CoalescePlan { skip: 2, process: 1, remainder: 250 });
    }

    #[test]
    fn coalesce_always_processes_at_most_one() {
        // No matter how deep the backlog, only ONE frame is ever rendered.
        for whole in 1..50u64 {
            let p = coalesce_plan(whole * 4096, 4096);
            assert_eq!(p.process, 1);
            assert_eq!(p.skip, whole - 1);
            assert_eq!(p.remainder, 0);
        }
    }

    #[test]
    fn coalesce_skip_plus_process_accounts_all_whole_frames() {
        // skip + process must equal the total whole frames (no frame lost or
        // double-counted; the remainder is strictly the sub-frame tail).
        let slice = 1280 * 720 * 4;
        for buf in [0u64, slice/2, slice, slice*2, slice*5 + 17] {
            let p = coalesce_plan(buf, slice);
            let whole = buf / slice;
            assert_eq!(p.skip + p.process, whole, "buf={}", buf);
            assert_eq!(p.remainder, buf % slice);
        }
    }

    #[test]
    fn coalesce_zero_slice_is_safe() {
        // frame geometry unknown → don't slice, keep everything buffered.
        let p = coalesce_plan(9999, 0);
        assert_eq!(p, CoalescePlan { skip: 0, process: 0, remainder: 9999 });
    }
}
