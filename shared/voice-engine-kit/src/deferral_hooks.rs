//! deferral_hooks — "voice is about to take the CPU" / "voice is done",
//! for a host app that wants to back off other work while it happens.
//!
//! Distinct from `capture_hooks`'s fire-once shape: this is a matched
//! before/after PAIR around the whole transcribe call, not a per-chunk tap
//! or a single start event. Forcing it into `capture_hooks`'s shape would
//! have meant two calls to a single-purpose hook standing in for what is
//! actually "enter" and "leave" — a worse fit than a second, correctly
//! shaped mechanism.
//!
//! Replaces `listen_with_audio`'s direct
//! `crate::log_summary::VOICE_ACTIVE.store(true/false, ..)` calls — real
//! app state (a log-summary worker's own deferral flag) this crate has no
//! business naming, the same reasoning as `corpus_recorder`'s reach-back
//! into `voice.rs` that `C18_TODO.md` section 19 found and fixed. A
//! consuming app installs `set_deferral_hooks` once; the shared core calls
//! both hooks unconditionally but cheaply when nothing is installed.

use std::sync::OnceLock;

type DeferralHook = Box<dyn Fn() + Send + Sync>;

static ON_TRANSCRIBE_START: OnceLock<DeferralHook> = OnceLock::new();
static ON_TRANSCRIBE_END: OnceLock<DeferralHook> = OnceLock::new();

/// Install both hooks together — they are a pair by construction (a
/// consuming app that wants to defer on start almost certainly wants to
/// resume on end, and installing them separately would let one be set
/// without the other, an inconsistent state with no legitimate use).
/// Refuses a second install, same as `capture_hooks`, for the same reason:
/// silent replacement hides a caller's mistake rather than surfacing it.
pub fn set_deferral_hooks(
    on_start: impl Fn() + Send + Sync + 'static,
    on_end: impl Fn() + Send + Sync + 'static,
) -> Result<(), &'static str> {
    ON_TRANSCRIBE_START.set(Box::new(on_start)).map_err(|_| "on_start already set")?;
    ON_TRANSCRIBE_END.set(Box::new(on_end)).map_err(|_| "on_end already set")?;
    Ok(())
}

pub(crate) fn transcribe_starting() {
    if let Some(f) = ON_TRANSCRIBE_START.get() { f(); }
}

pub(crate) fn transcribe_ending() {
    if let Some(f) = ON_TRANSCRIBE_END.get() { f(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn hooks_are_no_ops_before_installed() {
        if ON_TRANSCRIBE_START.get().is_none() {
            transcribe_starting(); // must not panic
        }
        if ON_TRANSCRIBE_END.get().is_none() {
            transcribe_ending();
        }
    }

    #[test]
    fn installed_pair_fires_both_in_order() {
        let order = Arc::new(AtomicU32::new(0));
        let (a, b) = (order.clone(), order.clone());
        if set_deferral_hooks(
            move || { a.store(1, Ordering::SeqCst); },
            move || { b.store(2, Ordering::SeqCst); },
        ).is_ok() {
            transcribe_starting();
            assert_eq!(order.load(Ordering::SeqCst), 1);
            transcribe_ending();
            assert_eq!(order.load(Ordering::SeqCst), 2);
        }
    }
}
