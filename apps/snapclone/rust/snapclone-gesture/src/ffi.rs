//! C FFI for snapclone-gesture.
//!
//! Opaque-handle ABI: an app creates a `ScGesture*`, feeds it scroll samples
//! and release events, and reads back decisions. Mirrors the crate's Rust API.
//! All entry points are panic-safe (catch_unwind → error code), accept null
//! handles defensively, and never expose Rust types across the boundary.

use crate::{Direction, GestureConfig, Outcome, ReleaseOutcome, TwoTierGesture};
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Opaque handle.
pub struct ScGesture {
    inner: TwoTierGesture,
}

/// Outcome codes for `sc_gesture_on_scroll`.
pub const SC_GESTURE_NONE: i32 = 0;
pub const SC_GESTURE_DETENT_FIRED: i32 = 1;
pub const SC_GESTURE_DETENT_PROGRESS: i32 = 2;
pub const SC_GESTURE_ERROR: i32 = -1;

/// Release codes for `sc_gesture_on_release`.
pub const SC_GESTURE_STAY: i32 = 0;
pub const SC_GESTURE_PAGE_JUMP: i32 = 1;

/// Direction codes (used in out-params).
pub const SC_GESTURE_DIR_NEXT: i32 = 1;
pub const SC_GESTURE_DIR_PREV: i32 = -1;

fn dir_code(d: Direction) -> i32 {
    d.sign()
}

/// Create a gesture machine with the tuned defaults for the given page size.
/// Returns null on allocation failure. Free with `sc_gesture_free`.
#[no_mangle]
pub extern "C" fn sc_gesture_new(page_size: f64) -> *mut ScGesture {
    let res = catch_unwind(|| {
        let cfg = GestureConfig::for_page_size(if page_size > 0.0 {
            page_size
        } else {
            GestureConfig::default().page_size
        });
        Box::into_raw(Box::new(ScGesture {
            inner: TwoTierGesture::new(cfg),
        }))
    });
    res.unwrap_or(std::ptr::null_mut())
}

/// Create with fully explicit config. Any non-positive field falls back to the
/// default for that field.
#[no_mangle]
pub extern "C" fn sc_gesture_new_config(
    page_size: f64,
    detent_threshold: f64,
    detent_arm_window_ms: f64,
    detent_rate_per_ms: f64,
    takeover_lock_ms: f64,
    commit_fraction: f64,
    flick_velocity_px_ms: f64,
    flick_min_px: f64,
) -> *mut ScGesture {
    let res = catch_unwind(|| {
        let d = GestureConfig::default();
        let pick = |v: f64, def: f64| if v > 0.0 { v } else { def };
        let cfg = GestureConfig {
            page_size: pick(page_size, d.page_size),
            detent_threshold: pick(detent_threshold, d.detent_threshold),
            detent_arm_window_ms: pick(detent_arm_window_ms, d.detent_arm_window_ms),
            detent_rate_per_ms: pick(detent_rate_per_ms, d.detent_rate_per_ms),
            takeover_lock_ms: pick(takeover_lock_ms, d.takeover_lock_ms),
            commit_fraction: pick(commit_fraction, d.commit_fraction),
            flick_velocity_px_ms: pick(flick_velocity_px_ms, d.flick_velocity_px_ms),
            flick_min_px: pick(flick_min_px, d.flick_min_px),
        };
        Box::into_raw(Box::new(ScGesture {
            inner: TwoTierGesture::new(cfg),
        }))
    });
    res.unwrap_or(std::ptr::null_mut())
}

/// Free a gesture machine. Null-safe.
///
/// # Safety
/// `g` must be a pointer returned by `sc_gesture_new*` and not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_gesture_free(g: *mut ScGesture) {
    if g.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        drop(Box::from_raw(g));
    }));
}

/// Feed one at-the-limit scroll sample. Returns an SC_GESTURE_* outcome code.
/// On DETENT_FIRED, `*out_dir` is the direction (SC_GESTURE_DIR_*).
/// On DETENT_PROGRESS, `*out_fraction` is 0..1 and `*out_dir` is the edge.
/// Out-params may be null if the caller doesn't need them.
///
/// # Safety
/// `g` must be a valid handle; `out_fraction`/`out_dir` null or valid pointers.
#[no_mangle]
pub unsafe extern "C" fn sc_gesture_on_scroll(
    g: *mut ScGesture,
    delta: f64,
    now_ms: f64,
    out_fraction: *mut f64,
    out_dir: *mut i32,
) -> i32 {
    let res = catch_unwind(AssertUnwindSafe(|| {
        let g = match unsafe { g.as_mut() } {
            Some(g) => g,
            None => return SC_GESTURE_ERROR,
        };
        match g.inner.on_scroll(delta, now_ms) {
            Outcome::None => SC_GESTURE_NONE,
            Outcome::DetentFired { dir } => {
                if let Some(d) = unsafe { out_dir.as_mut() } {
                    *d = dir_code(dir);
                }
                SC_GESTURE_DETENT_FIRED
            }
            Outcome::DetentProgress { fraction, edge } => {
                if let Some(f) = unsafe { out_fraction.as_mut() } {
                    *f = fraction;
                }
                if let Some(d) = unsafe { out_dir.as_mut() } {
                    *d = dir_code(edge);
                }
                SC_GESTURE_DETENT_PROGRESS
            }
        }
    }));
    res.unwrap_or(SC_GESTURE_ERROR)
}

/// Reset the detent streak (call when the inner view is NOT at its limit).
///
/// # Safety
/// `g` must be a valid handle.
#[no_mangle]
pub unsafe extern "C" fn sc_gesture_reset_streak(g: *mut ScGesture) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if let Some(g) = unsafe { g.as_mut() } {
            g.inner.reset_streak();
        }
    }));
}

/// Page-tier decision at release. Returns SC_GESTURE_STAY or
/// SC_GESTURE_PAGE_JUMP; on PAGE_JUMP, `*out_dir` is the direction.
///
/// # Safety
/// `g` must be a valid handle; `out_dir` null or a valid pointer.
#[no_mangle]
pub unsafe extern "C" fn sc_gesture_on_release(
    g: *mut ScGesture,
    velocity: f64,
    drag_distance: f64,
    out_dir: *mut i32,
) -> i32 {
    let res = catch_unwind(AssertUnwindSafe(|| {
        let g = match unsafe { g.as_mut() } {
            Some(g) => g,
            None => return SC_GESTURE_ERROR,
        };
        match g.inner.on_release(velocity, drag_distance) {
            ReleaseOutcome::Stay => SC_GESTURE_STAY,
            ReleaseOutcome::PageJump { dir } => {
                if let Some(d) = unsafe { out_dir.as_mut() } {
                    *d = dir_code(dir);
                }
                SC_GESTURE_PAGE_JUMP
            }
        }
    }));
    res.unwrap_or(SC_GESTURE_ERROR)
}

/// True (1) if the post-fire takeover lock is currently holding, else 0.
///
/// # Safety
/// `g` must be a valid handle.
#[no_mangle]
pub unsafe extern "C" fn sc_gesture_locked(g: *const ScGesture, now_ms: f64) -> i32 {
    let res = catch_unwind(AssertUnwindSafe(|| {
        match unsafe { g.as_ref() } {
            Some(g) => i32::from(g.inner.locked(now_ms)),
            None => 0,
        }
    }));
    res.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_lifecycle_and_detent() {
        let g = sc_gesture_new(390.0);
        assert!(!g.is_null());
        let mut fired = 0;
        let mut t = 0.0;
        for _ in 0..40 {
            let mut frac = 0.0;
            let mut dir = 0;
            let code = unsafe { sc_gesture_on_scroll(g, 12.0, t, &mut frac, &mut dir) };
            if code == SC_GESTURE_DETENT_FIRED {
                assert_eq!(dir, SC_GESTURE_DIR_NEXT);
                fired += 1;
            }
            t += 16.0;
        }
        assert_eq!(fired, 1);
        unsafe { sc_gesture_free(g) };
    }

    #[test]
    fn ffi_release_page_jump() {
        let g = sc_gesture_new(390.0);
        let mut dir = 0;
        let code = unsafe { sc_gesture_on_release(g, 0.5, 30.0, &mut dir) };
        assert_eq!(code, SC_GESTURE_PAGE_JUMP);
        assert_eq!(dir, SC_GESTURE_DIR_NEXT);
        unsafe { sc_gesture_free(g) };
    }

    #[test]
    fn ffi_null_handle_is_safe() {
        let mut dir = 0;
        assert_eq!(
            unsafe { sc_gesture_on_scroll(std::ptr::null_mut(), 1.0, 0.0, std::ptr::null_mut(), &mut dir) },
            SC_GESTURE_ERROR
        );
        assert_eq!(
            unsafe { sc_gesture_on_release(std::ptr::null_mut(), 0.5, 30.0, &mut dir) },
            SC_GESTURE_ERROR
        );
        unsafe { sc_gesture_free(std::ptr::null_mut()) }; // no crash
    }

    #[test]
    fn ffi_null_outparams_ok() {
        let g = sc_gesture_new(390.0);
        // Passing null out-params must not crash.
        let _ = unsafe {
            sc_gesture_on_scroll(g, 12.0, 0.0, std::ptr::null_mut(), std::ptr::null_mut())
        };
        unsafe { sc_gesture_free(g) };
    }
}
