/* snapclone_motion_ffi.h — motion classification + position projection.
 *
 * SENDER side: classify your accelerometer window into an activity to attach to
 * your outbound location. EITHER side: project a fix forward by motion + elapsed
 * time so the wayfinder can show roughly where a peer is NOW, with an honest
 * uncertainty radius. Link libsnapclone_location.a.
 *
 * NOTE: dead reckoning is SELF-tracking — a receiver cannot sense a remote
 * peer's motion. The sender computes its own activity/heading/steps and ships
 * them with its position; projection is then possible on either end.
 */
#ifndef SNAPCLONE_MOTION_FFI_H
#define SNAPCLONE_MOTION_FFI_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* activity codes */
#define SC_ACT_STATIONARY 0
#define SC_ACT_ON_FOOT    1
#define SC_ACT_VEHICLE    2

/* Classify an accelerometer window summary (default policy) → SC_ACT_*.
 * accel_std = std-dev of linear-accel magnitude (m/s^2); cadence_spm = footfall
 * steps/min from autocorrelation (0 if none); speed_mps = mean speed (<0 unknown). */
int sc_motion_classify(float accel_std, float cadence_spm, float speed_mps);

/* Project a fix forward. For SC_ACT_ON_FOOT: distance = steps * stride_m along
 * heading. For SC_ACT_VEHICLE: distance = speed_mps * elapsed_secs along heading.
 * SC_ACT_STATIONARY: no movement. Writes projected lat/lon, the 1-sigma
 * uncertainty radius (m, grows with distance), and distance projected (m).
 * Returns 0 on success, -1 if any out pointer is null. */
int sc_motion_project(double base_lat, double base_lon, float base_accuracy_m,
                      int activity, float heading_deg, uint32_t steps,
                      float stride_m, float speed_mps, double elapsed_secs,
                      double *out_lat, double *out_lon,
                      float *out_uncertainty_m, float *out_projected_m);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_MOTION_FFI_H */
