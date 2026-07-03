/* snapclone_gesture_ffi.h — C ABI for the reusable two-tier gesture model.
 *
 * Small sustained scroll past a limit -> a one-shot sub-page detent.
 * Big fast swipe -> a whole-page jump.
 *
 * Platform-free: you feed it signed deltas + monotonic timestamps (ms) and read
 * back decisions. YOUR app owns reading wheel/touch events, knowing whether the
 * inner view is scrolled to its limit, rendering the progress "barrier", and
 * actually moving to the next page / sub-page.
 *
 * Link against libsnapclone_gesture.a. All functions are panic-safe and
 * null-tolerant.
 *
 * Typical wiring:
 *   ScGesture *g = sc_gesture_new(page_width_px);
 *   // per at-the-limit scroll sample:
 *   double frac; int dir;
 *   int o = sc_gesture_on_scroll(g, signed_delta_px, now_ms, &frac, &dir);
 *   if (o == SC_GESTURE_DETENT_FIRED)   advance_subpage(dir);
 *   else if (o == SC_GESTURE_DETENT_PROGRESS) render_barrier(frac, dir);
 *   // when the inner view is NOT at its limit (normal scrolling):
 *   sc_gesture_reset_streak(g);
 *   // on finger/trackpad release:
 *   int rdir; int r = sc_gesture_on_release(g, release_vel_px_ms, drag_px, &rdir);
 *   if (r == SC_GESTURE_PAGE_JUMP) jump_page(rdir);
 *   // teardown:
 *   sc_gesture_free(g);
 */
#ifndef SNAPCLONE_GESTURE_FFI_H
#define SNAPCLONE_GESTURE_FFI_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ScGesture ScGesture;

/* on_scroll outcome codes */
#define SC_GESTURE_NONE              0
#define SC_GESTURE_DETENT_FIRED      1
#define SC_GESTURE_DETENT_PROGRESS   2
#define SC_GESTURE_ERROR            (-1)

/* on_release outcome codes */
#define SC_GESTURE_STAY              0
#define SC_GESTURE_PAGE_JUMP         1

/* direction out-param values */
#define SC_GESTURE_DIR_NEXT          1
#define SC_GESTURE_DIR_PREV        (-1)

/* Create with tuned defaults for a given page size (px). NULL on failure. */
ScGesture *sc_gesture_new(double page_size);

/* Create with explicit config. Any non-positive field uses that field's
 * default. Fields mirror GestureConfig:
 *   detent_threshold       raw px past the limit to fire a sub-page detent
 *   detent_arm_window_ms   ms the detent stays armed before re-arming
 *   detent_rate_per_ms     max raw units/ms accepted (refresh-rate normalizer)
 *   takeover_lock_ms       post-fire cooldown that stops runaway multi-fires
 *   commit_fraction        fraction of page_size a drag must cover for a jump
 *   flick_velocity_px_ms   min release velocity for a flick
 *   flick_min_px           min drag distance for a flick to count
 */
ScGesture *sc_gesture_new_config(double page_size,
                                 double detent_threshold,
                                 double detent_arm_window_ms,
                                 double detent_rate_per_ms,
                                 double takeover_lock_ms,
                                 double commit_fraction,
                                 double flick_velocity_px_ms,
                                 double flick_min_px);

/* Free. Null-safe. */
void sc_gesture_free(ScGesture *g);

/* Feed one AT-THE-LIMIT scroll sample. Returns an SC_GESTURE_* outcome.
 * out_fraction (0..1) is set on DETENT_PROGRESS; out_dir is set on
 * DETENT_FIRED (advance direction) and DETENT_PROGRESS (pulled edge).
 * out_fraction / out_dir may be NULL. */
int sc_gesture_on_scroll(ScGesture *g, double delta, double now_ms,
                         double *out_fraction, int *out_dir);

/* Call when the inner view is NOT at its limit, so the next limit-hit starts a
 * fresh detent streak. */
void sc_gesture_reset_streak(ScGesture *g);

/* Page-tier decision at release. Returns SC_GESTURE_STAY or
 * SC_GESTURE_PAGE_JUMP; out_dir (may be NULL) is set on PAGE_JUMP. */
int sc_gesture_on_release(ScGesture *g, double velocity, double drag_distance,
                          int *out_dir);

/* 1 if the post-fire takeover lock currently holds, else 0. */
int sc_gesture_locked(const ScGesture *g, double now_ms);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_GESTURE_FFI_H */
