//! C FFI seam for `uikit-gesture`.
//!
//! This is the bridge that lets the *existing* C++ `SwipeNavigator` keep
//! driving the exact same tuned logic while snapclone's UI is being moved to
//! egui (see `apps/snapclone-egui`). The C++ side owns the toolkit bits
//! (reading wheel/touch deltas, scroll-limit checks, painting the barrier) and
//! calls these functions for the decisions — so the *feel* is identical whether
//! the front-end is Qt-today or egui-tomorrow, and there is exactly one tuned
//! implementation to maintain.
//!
//! Once the egui port fully replaces the Qt widget, this seam can be dropped;
//! until then it is the contract that keeps the two front-ends in lockstep.
//!
//! ```c
//! // C header sketch (generate with cbindgen):
//! typedef struct UkGesture UkGesture;
//! typedef enum { UK_NONE=0, UK_DETENT_FIRED=1, UK_DETENT_PROGRESS=2 } UkOutcomeKind;
//! typedef enum { UK_STAY=0, UK_PAGE_JUMP=1 } UkReleaseKind;
//! typedef struct { UkOutcomeKind kind; int dir; double fraction; } UkOutcome; // dir: +1 Next, -1 Prev
//!
//! UkGesture* uk_gesture_new_default(void);
//! UkGesture* uk_gesture_new(double page, double detent, double arm_ms,
//!                           double rate, double lock_ms, double commit_frac,
//!                           double flick_vel, double flick_min);
//! void       uk_gesture_free(UkGesture*);
//! UkOutcome  uk_gesture_on_scroll(UkGesture*, double delta, double now_ms);
//! void       uk_gesture_reset_streak(UkGesture*);
//! UkReleaseKind uk_gesture_on_release(UkGesture*, double vel, double drag, int* out_dir);
//! ```

use crate::{Direction, GestureConfig, Outcome, ReleaseOutcome, TwoTierGesture};

/// C-visible outcome kind.
#[repr(C)]
#[derive(Clone, Copy)]
pub enum UkOutcomeKind {
    None = 0,
    DetentFired = 1,
    DetentProgress = 2,
}

/// C-visible release kind.
#[repr(C)]
#[derive(Clone, Copy)]
pub enum UkReleaseKind {
    Stay = 0,
    PageJump = 1,
}

/// C-visible scroll outcome. `dir` is +1 (Next), -1 (Prev), or 0 (n/a).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UkOutcome {
    pub kind: UkOutcomeKind,
    pub dir: i32,
    pub fraction: f64,
}

fn dir_to_c(d: Direction) -> i32 { d.sign() }

/// Create a gesture with the tuned defaults (matches Theme.hpp).
#[no_mangle]
pub extern "C" fn uk_gesture_new_default() -> *mut TwoTierGesture {
    Box::into_raw(Box::new(TwoTierGesture::new(GestureConfig::default())))
}

/// Create a gesture with explicit thresholds.
#[no_mangle]
pub extern "C" fn uk_gesture_new(
    page_size: f64,
    detent_threshold: f64,
    detent_arm_window_ms: f64,
    detent_rate_per_ms: f64,
    takeover_lock_ms: f64,
    commit_fraction: f64,
    flick_velocity_px_ms: f64,
    flick_min_px: f64,
) -> *mut TwoTierGesture {
    let cfg = GestureConfig {
        page_size,
        detent_threshold,
        detent_arm_window_ms,
        detent_rate_per_ms,
        takeover_lock_ms,
        commit_fraction,
        flick_velocity_px_ms,
        flick_min_px,
    };
    Box::into_raw(Box::new(TwoTierGesture::new(cfg)))
}

/// Free a gesture created by `uk_gesture_new*`.
///
/// # Safety
/// `g` must be a pointer returned by `uk_gesture_new*` and not freed already.
#[no_mangle]
pub unsafe extern "C" fn uk_gesture_free(g: *mut TwoTierGesture) {
    if !g.is_null() {
        drop(unsafe { Box::from_raw(g) });
    }
}

/// Feed one at-limit scroll sample.
///
/// # Safety
/// `g` must be a valid pointer from `uk_gesture_new*`.
#[no_mangle]
pub unsafe extern "C" fn uk_gesture_on_scroll(
    g: *mut TwoTierGesture,
    delta: f64,
    now_ms: f64,
) -> UkOutcome {
    let g = unsafe { &mut *g };
    match g.on_scroll(delta, now_ms) {
        Outcome::None => UkOutcome { kind: UkOutcomeKind::None, dir: 0, fraction: 0.0 },
        Outcome::DetentFired { dir } => {
            UkOutcome { kind: UkOutcomeKind::DetentFired, dir: dir_to_c(dir), fraction: 1.0 }
        }
        Outcome::DetentProgress { fraction, edge } => {
            UkOutcome { kind: UkOutcomeKind::DetentProgress, dir: dir_to_c(edge), fraction }
        }
    }
}

/// Reset the detent streak (call when the inner view is not at its limit).
///
/// # Safety
/// `g` must be a valid pointer from `uk_gesture_new*`.
#[no_mangle]
pub unsafe extern "C" fn uk_gesture_reset_streak(g: *mut TwoTierGesture) {
    let g = unsafe { &mut *g };
    g.reset_streak();
}

/// Page-tier decision at release. Writes the direction (+1/-1) to `out_dir`
/// when a jump is returned; returns the kind.
///
/// # Safety
/// `g` must be valid; `out_dir` must be a valid writable pointer or null.
#[no_mangle]
pub unsafe extern "C" fn uk_gesture_on_release(
    g: *mut TwoTierGesture,
    velocity: f64,
    drag_distance: f64,
    out_dir: *mut i32,
) -> UkReleaseKind {
    let g = unsafe { &mut *g };
    match g.on_release(velocity, drag_distance) {
        ReleaseOutcome::Stay => UkReleaseKind::Stay,
        ReleaseOutcome::PageJump { dir } => {
            if !out_dir.is_null() {
                unsafe { *out_dir = dir_to_c(dir) };
            }
            UkReleaseKind::PageJump
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_seam_roundtrips() {
        unsafe {
            let g = uk_gesture_new_default();
            assert!(!g.is_null());
            // progress before fire
            let o = uk_gesture_on_scroll(g, 20.0, 0.0);
            assert!(matches!(o.kind, UkOutcomeKind::DetentProgress));
            assert_eq!(o.dir, 1);
            // release: a real flick jumps Prev
            let mut dir = 0;
            let k = uk_gesture_on_release(g, -0.6, -30.0, &mut dir);
            assert!(matches!(k, UkReleaseKind::PageJump));
            assert_eq!(dir, -1);
            uk_gesture_reset_streak(g);
            uk_gesture_free(g);
        }
    }
}
