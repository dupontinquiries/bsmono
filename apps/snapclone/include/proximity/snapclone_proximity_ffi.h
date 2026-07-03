#ifndef SNAPCLONE_PROXIMITY_FFI_H
#define SNAPCLONE_PROXIMITY_FFI_H

// ─── snapclone_proximity_ffi.h ───────────────────────────────────────────────
//
// C ABI for the Rust snapclone-proximity crate. Available when CMake is built
// with -DSNAPCLONE_RUST_PROXIMITY=ON (default ON).
//
// RSSI ranging + gradient-following bearing estimation for the BLE peer
// "pointing / triangulation" feature (see docs/TODO.md "Bluetooth / ad-hoc
// pointing triangulation"). Phones lack angle-of-arrival hardware, so bearing
// is derived by correlating RSSI change with compass heading as the user
// moves — "warmer/colder pointing" rather than true trilateration. Distance
// comes from a log-distance path-loss model, smoothed, and presented as a
// coarse band (never a precise meter, since BLE RSSI is too noisy for that).
//
// Pure-Rust, no deps; a few microseconds per sample on FP5.
//
// ─────────────────────────────────────────────────────────────────────────────

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/// Coarse proximity band for display. Matches the Rust ProximityBand enum.
typedef enum {
    SC_PROX_UNKNOWN    = 0, // not enough data
    SC_PROX_VERY_CLOSE = 1, // < ~2 m
    SC_PROX_CLOSE      = 2, // ~2-5 m
    SC_PROX_NEAR       = 3, // ~5-15 m
    SC_PROX_FAR        = 4, // > ~15 m (still in BLE range)
} ScProximityBand;

/// Estimate returned by sc_proximity_update. By value.
typedef struct {
    /// Smoothed distance estimate in metres (coarse; prefer `band`).
    float distance_m;
    /// Coarse band for display (ScProximityBand).
    int32_t band;
    /// Estimated bearing to the peer, degrees [0,360), 0=N, 90=E.
    /// -1.0 means "not enough motion to estimate yet".
    float bearing_deg;
    /// Confidence in the bearing, [0,1].
    float bearing_confidence;
} ScProximityEstimate;

/// Create an estimator. `tx_power_dbm` = expected RSSI at 1 m for the peer's
/// radio (pass 0 for the default ~-59 dBm); `path_loss_n` = environmental
/// path-loss exponent (pass 0 for the default ~2.5, indoor-ish). Returns an
/// opaque handle; free with sc_proximity_free.
typedef struct ProximityEstimator ProximityEstimator;
ProximityEstimator *sc_proximity_new(float tx_power_dbm, float path_loss_n);

/// Feed one RSSI sample (dBm, negative) + the user's current compass heading
/// (degrees, 0=N). Set `has_heading` to 0 when no compass reading is
/// available this sample (distance still updates; bearing won't). Returns the
/// current estimate. Null handle → empty estimate.
ScProximityEstimate sc_proximity_update(ProximityEstimator *handle,
                                        float rssi_dbm, float heading_deg,
                                        int has_heading);

/// Free an estimator. NULL is a safe no-op.
void sc_proximity_free(ProximityEstimator *handle);

/// ABI version probe; returns 1.
int sc_proximity_abi_version(void);

#ifdef __cplusplus
} // extern "C"
#endif

#endif // SNAPCLONE_PROXIMITY_FFI_H
