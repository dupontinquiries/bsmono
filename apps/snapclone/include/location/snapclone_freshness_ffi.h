/* snapclone_freshness_ffi.h — position freshness/confidence for the wayfinder.
 *
 * Grade each peer's location fix by age + accuracy so the wayfinder shows a
 * SOLID arrow only for fresh/accurate fixes, a FADED arrow for older/coarser
 * ones, and a "last seen N ago" label when a fix is too old to navigate by.
 * Link libsnapclone_location.a.
 */
#ifndef SNAPCLONE_FRESHNESS_FFI_H
#define SNAPCLONE_FRESHNESS_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* confidence codes */
#define SC_LOC_CONF_LIVE        0
#define SC_LOC_CONF_RECENT      1
#define SC_LOC_CONF_STALE       2
#define SC_LOC_CONF_LAST_KNOWN  3

/* presentation tiers */
#define SC_LOC_TIER_SOLID_ARROW      0
#define SC_LOC_TIER_FADED_ARROW      1
#define SC_LOC_TIER_LAST_SEEN_LABEL  2

/* Grade a fix (default policy) → SC_LOC_CONF_*. age_secs = seconds since the fix
 * was recorded; accuracy_m = error radius (0 = unknown/precise). */
int sc_loc_confidence(int64_t age_secs, float accuracy_m);

/* Same, with explicit policy thresholds (for config-driven tuning). */
int sc_loc_confidence_ex(int64_t age_secs, float accuracy_m,
                         int64_t live_age, int64_t recent_age, int64_t stale_age,
                         float precise_acc, float coarse_acc);

/* confidence code → presentation tier (SC_LOC_TIER_*). */
int sc_loc_presentation(int conf);

/* confidence code → opacity multiplier (0..1) for the arrow/avatar. */
float sc_loc_opacity(int conf);

/* Write a compact "N ago" label (NUL-terminated) into out. Returns bytes
 * written (excl NUL), or -1 if out is null / too small. 16 bytes always fits. */
int sc_loc_last_seen_label(int64_t age_secs, char *out, size_t out_cap);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_FRESHNESS_FFI_H */
