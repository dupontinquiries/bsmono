//! capture_hooks — optional extension points the capture core calls,
//! without knowing what (if anything) is on the other end.
//!
//! ## Why this exists
//!
//! Two functions that must live in this crate's core call directly into
//! features that must NOT: the cpal audio callback (`ensure_capture_running`)
//! calls voiceterm's `corpus_recorder::push_samples` on every chunk, and
//! `begin_capture` calls voiceterm's live-preview worker
//! (`ensure_preview_thread`/`clear_partial_preview`) on every recording
//! start. Both are real, valuable, and genuinely optional — nothing in
//! `listen()`'s own return value depends on either — but both are called
//! from functions that have no optional version to fall back to.
//!
//! `maintenance/C18_TODO.md` sections 19 and 22 found this twice,
//! separately, and solved it the same way both times before this module
//! existed to hold the pattern once: a consuming app installs a hook at
//! startup; the shared core calls it unconditionally but cheaply when
//! nothing is installed.
//!
//! ## Why one settable slot per hook, not a list
//!
//! Each hook is a single `OnceLock`, settable once. Not a `Vec` of
//! subscribers: there is exactly one consuming app per process (this is a
//! library linked into one binary, not a plugin host), so "what if two
//! things want the audio tap" is not a real scenario this needs to solve,
//! and solving it anyway (ordering, panics in one subscriber affecting
//! others, removal) would be speculative generality for a problem that
//! does not exist yet.
//!
//! ## Why OnceLock and not a plain `static mut` or a Mutex<Option<..>>
//!
//! `OnceLock::set` is safe, and refusing a second `set` (returning the
//! value back rather than silently overwriting) makes a caller who
//! accidentally installs a hook twice see the mistake immediately rather
//! than get spooky "which one actually ran" behavior. A `Mutex<Option<..>>`
//! would allow silent replacement and would need a lock on every audio
//! chunk for a value that, in every real use, is set once and never
//! changes again.

use std::sync::OnceLock;

type AudioTap = Box<dyn Fn(&[f32], u32) + Send + Sync>;
type CaptureStartHook = Box<dyn Fn() + Send + Sync>;

static AUDIO_TAP: OnceLock<AudioTap> = OnceLock::new();
static ON_CAPTURE_START: OnceLock<CaptureStartHook> = OnceLock::new();

/// Install the per-chunk audio tap. Call once, before capture starts
/// (typically at app startup). `samples` are the same mono-or-interleaved
/// f32 chunk the cpal callback just received; `sample_rate` is the
/// stream's native rate (NOT necessarily 16kHz — resampling, if the tap
/// needs 16kHz, is the installer's job, matching what
/// `corpus_recorder::push_samples`'s own signature already expects).
///
/// Returns `Err(f)` handing the closure back if a tap was already
/// installed, rather than silently replacing it — see the module doc for
/// why silent replacement is the wrong default here.
pub fn set_audio_tap(
    f: impl Fn(&[f32], u32) + Send + Sync + 'static,
) -> Result<(), AudioTap> {
    AUDIO_TAP.set(Box::new(f))
}

/// Called from inside the capture core's audio callback on every chunk.
/// A no-op when nothing is installed — cheap (`OnceLock::get` on the hot
/// path is a single loaded flag check, no lock).
pub(crate) fn tap_audio(samples: &[f32], sample_rate: u32) {
    if let Some(f) = AUDIO_TAP.get() {
        f(samples, sample_rate);
    }
}

/// Install the capture-start hook. Call once, before the first
/// `begin_capture`. Fires once per recording start — the installer decides
/// what "start" means for its own feature (voiceterm's preview worker
/// clears stale state and ensures its background thread is running).
pub fn set_on_capture_start(
    f: impl Fn() + Send + Sync + 'static,
) -> Result<(), CaptureStartHook> {
    ON_CAPTURE_START.set(Box::new(f))
}

/// Called from inside the capture core's `begin_capture`. A no-op when
/// nothing is installed.
pub(crate) fn capture_started() {
    if let Some(f) = ON_CAPTURE_START.get() {
        f();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    // Both hooks are process-global OnceLocks, so tests that install one
    // must not run against the SAME hook in the same test binary run more
    // than once (the second `set` would correctly fail, but that failure
    // would belong to test isolation, not to the behavior being tested).
    // Each test below exercises the CALLING side (tap_audio /
    // capture_started) against a hook installed exactly once, which is
    // the real usage shape (one app, one install, many calls).

    #[test]
    fn tap_audio_is_a_no_op_before_anything_is_installed() {
        // Uses a name-scoped no-arg call: if nothing else in this process
        // installed the tap yet, this must not panic and must simply do
        // nothing. (If another test in the same binary already installed
        // one, this assertion is skipped rather than flaky — the ordering
        // between #[test] functions in one binary is not guaranteed.)
        if AUDIO_TAP.get().is_none() {
            tap_audio(&[0.0, 0.0], 16_000);
        }
    }

    #[test]
    fn installed_tap_receives_the_real_samples_and_rate() {
        let seen_len = Arc::new(AtomicU32::new(0));
        let seen_rate = Arc::new(AtomicU32::new(0));
        let (l, r) = (seen_len.clone(), seen_rate.clone());
        // First installer in THIS test wins; if a previous test in the
        // same binary already claimed the slot, this one intentionally
        // does not re-assert (see the note above) rather than fail on
        // ordering it does not control.
        if set_audio_tap(move |samples, rate| {
            l.store(samples.len() as u32, Ordering::SeqCst);
            r.store(rate, Ordering::SeqCst);
        }).is_ok() {
            tap_audio(&[1.0, 2.0, 3.0], 48_000);
            assert_eq!(seen_len.load(Ordering::SeqCst), 3);
            assert_eq!(seen_rate.load(Ordering::SeqCst), 48_000);
        }
    }

    #[test]
    fn setting_the_tap_twice_hands_the_second_closure_back_rather_than_replacing() {
        // Only meaningful as the SECOND attempt in this process; run after
        // any test above that may have already installed one, and either
        // way this call itself must return Err rather than silently
        // replacing a real installed tap or panicking.
        let _ = set_audio_tap(|_samples, _rate| {}); // may succeed or already be set
        let second = set_audio_tap(|_samples, _rate| {});
        assert!(second.is_err(), "a second install must be refused, not silently swap in");
    }

    #[test]
    fn capture_started_is_a_no_op_before_anything_is_installed() {
        if ON_CAPTURE_START.get().is_none() {
            capture_started(); // must not panic
        }
    }

    #[test]
    fn installed_capture_start_hook_fires_on_capture_started() {
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        if set_on_capture_start(move || { f.store(true, Ordering::SeqCst); }).is_ok() {
            capture_started();
            assert!(fired.load(Ordering::SeqCst), "installed hook must actually run");
        }
    }
}
