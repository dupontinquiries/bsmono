/* snapclone_gstpipeline_ffi.h — C ABI for the GstLaunchBackend decision logic.
 *
 * Pure logic extracted from GstLaunchBackend: classify gst-launch stderr, and
 * drive the negotiation-failure recovery ladder (settle → relaxed → swap) with
 * correct per-session guard lifecycle — including the resume-on-same-sensor fix
 * that's locked down by a Rust regression test. No GStreamer, no Qt; this will
 * NOT touch hardware. Link libsnapclone_gstpipeline.a.
 */
#ifndef SNAPCLONE_GSTPIPELINE_FFI_H
#define SNAPCLONE_GSTPIPELINE_FFI_H

#ifdef __cplusplus
extern "C" {
#endif

/* error categories */
#define SC_GST_ERR_NEGOTIATION  0
#define SC_GST_ERR_GL_LINK      1
#define SC_GST_ERR_MISSING_NODE 2
#define SC_GST_ERR_OTHER        3

/* recovery actions */
#define SC_GST_ACT_FORCE_CPU_RETRY 0  /* force CPU colorspace, retry same node */
#define SC_GST_ACT_SETTLE_RETRY    1  /* wait settle window, retry same node, normal caps */
#define SC_GST_ACT_RELAXED_RETRY   2  /* retry same node, relaxed caps */
#define SC_GST_ACT_SWAP_NODE       3  /* try a different camera node */
#define SC_GST_ACT_GIVE_UP         4  /* nothing left to try */

typedef struct ScGstRecovery ScGstRecovery; /* opaque */

/* Classify gst-launch stderr → SC_GST_ERR_*. NULL → OTHER. */
int sc_gst_classify_error(const char *stderr_text);

/* Lifecycle. settle_ms/gpu_retry_ms <= 0 → defaults (600 / 100). */
ScGstRecovery *sc_gst_recovery_new(int settle_ms, int gpu_retry_ms);
void           sc_gst_recovery_free(ScGstRecovery *sm);

/* Model the backend's calls. */
void sc_gst_recovery_on_start(ScGstRecovery *sm, const char *node);
void sc_gst_recovery_on_stop(ScGstRecovery *sm);

/* On a failure: err = SC_GST_ERR_*, early_exit != 0 if within the early-exit
 * window. Writes the retry delay (ms) into *out_delay_ms if non-null. Returns
 * SC_GST_ACT_*. */
int sc_gst_recovery_on_failure(ScGstRecovery *sm, int err, int early_exit, int *out_delay_ms);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_GSTPIPELINE_FFI_H */
