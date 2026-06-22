//! snapclone-proximity — classical RSSI ranging + gradient-following bearing
//! estimation for the BLE peer "pointing / triangulation" feature.
//!
//! No ML, no extra hardware. Pure f32 stdlib. Mirrors the house FFI pattern
//! used by snapclone-saliency / snapclone-audio: a stateful opaque handle,
//! `#[no_mangle]` C entry points, and a calibration test suite.
//!
//! ## What it does
//!
//! Phones can't expose per-antenna phase, so true angle-of-arrival is out
//! without BLE 5.1 direction-finding hardware (FP5 likely lacks it). Instead
//! we estimate:
//!
//!   1. **Distance from RSSI** via the log-distance path-loss model:
//!        d = 10 ^ ((txPower - rssi) / (10 * n))
//!      where `txPower` is the RSSI expected at 1 m and `n` is the
//!      environmental path-loss exponent (~2.0 free space, ~3-4 indoors).
//!      Noisy and multipath-prone, so we smooth (below) and only ever
//!      present coarse distance BANDS to the user, never a precise meter.
//!
//!   2. **Smoothed RSSI / distance** via an EMA. BLE RSSI is jumpy
//!      (±6 dB frame-to-frame is normal); without smoothing the distance
//!      estimate would flicker uselessly.
//!
//!   3. **Bearing by gradient following.** As the user walks/turns, we
//!      correlate the change in (smoothed) RSSI with the compass heading
//!      they were facing when the change happened. RSSI rising while
//!      facing ~040° ⇒ the contact is roughly NE. A few seconds of motion
//!      converges on a bearing. This is "pointing" by warmer/colder, not
//!      trilateration — but it needs only one peer and no GPS.
//!
//! The intended consumer is `core/ProximitySource` (Qt/BlueZ side), which
//! feeds raw RSSI samples + the current compass heading in, and reads back
//! a smoothed distance band + a bearing estimate with a confidence.

use std::os::raw::c_int;

// ─── Distance bands (what the UI actually shows) ─────────────────────────────

/// Coarse proximity band. RSSI distance is too noisy to show meters, so the
/// UI shows one of these instead.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProximityBand {
    Unknown   = 0, // not enough data
    VeryClose = 1, // < ~2 m
    Close     = 2, // ~2-5 m
    Near      = 3, // ~5-15 m
    Far       = 4, // > ~15 m (still in BLE range, ~ up to 30-50m)
}

fn band_for(distance_m: f32) -> ProximityBand {
    if distance_m <= 0.0 {
        ProximityBand::Unknown
    } else if distance_m < 2.0 {
        ProximityBand::VeryClose
    } else if distance_m < 5.0 {
        ProximityBand::Close
    } else if distance_m < 15.0 {
        ProximityBand::Near
    } else {
        ProximityBand::Far
    }
}

// ─── Result struct returned to C ─────────────────────────────────────────────

/// Snapshot returned by `sc_proximity_update`. By value, like SalRect.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProximityEstimate {
    /// Smoothed distance estimate in metres (coarse; prefer `band`).
    pub distance_m: f32,
    /// Coarse band for display.
    pub band: ProximityBand,
    /// Estimated bearing to the peer in degrees [0,360), 0 = North,
    /// 90 = East. -1.0 means "not enough motion to estimate yet".
    pub bearing_deg: f32,
    /// Confidence in the bearing estimate, [0,1]. Rises as more
    /// correlated motion samples accumulate.
    pub bearing_confidence: f32,
}

impl ProximityEstimate {
    fn empty() -> Self {
        ProximityEstimate {
            distance_m: -1.0,
            band: ProximityBand::Unknown,
            bearing_deg: -1.0,
            bearing_confidence: 0.0,
        }
    }
}

// ─── Tunables ────────────────────────────────────────────────────────────────

/// Default RSSI at 1 m (dBm). Per-device; this is a reasonable BLE default.
const DEFAULT_TX_POWER: f32 = -59.0;
/// Default path-loss exponent (indoor-ish).
const DEFAULT_PATH_LOSS_N: f32 = 2.5;
/// EMA smoothing factor for RSSI (0..1; higher = more responsive, noisier).
const RSSI_EMA_ALPHA: f32 = 0.25;
/// Number of directional bins for the gradient bearing estimator (15° each).
const BEARING_BINS: usize = 24;

// ─── Stateful estimator ──────────────────────────────────────────────────────

pub struct ProximityEstimator {
    tx_power: f32,
    path_loss_n: f32,
    smoothed_rssi: f32,
    have_rssi: bool,
    /// True once we've seen at least one prior RSSI sample, so a delta is
    /// meaningful for bearing credit.
    have_rssi_prev: bool,
    /// Accumulated RSSI improvement per heading bin. When the user faces a
    /// heading and RSSI improves, we add the delta to that bin; when it
    /// worsens, we subtract. The bin with the highest accumulated value
    /// points (roughly) toward the peer. Decays slowly so stale directions
    /// fade as the user moves around.
    bin_score: [f32; BEARING_BINS],
    /// Total absolute evidence accumulated, for confidence scaling.
    total_evidence: f32,
    last_heading_deg: f32,
    have_heading: bool,
}

impl ProximityEstimator {
    fn new(tx_power: f32, path_loss_n: f32) -> Self {
        ProximityEstimator {
            tx_power,
            path_loss_n: if path_loss_n > 0.5 { path_loss_n } else { DEFAULT_PATH_LOSS_N },
            smoothed_rssi: 0.0,
            have_rssi: false,
            have_rssi_prev: false,
            bin_score: [0.0; BEARING_BINS],
            total_evidence: 0.0,
            last_heading_deg: 0.0,
            have_heading: false,
        }
    }

    fn distance_from_rssi(&self, rssi: f32) -> f32 {
        // d = 10 ^ ((txPower - rssi) / (10 * n))
        let exp = (self.tx_power - rssi) / (10.0 * self.path_loss_n);
        10f32.powf(exp)
    }

    fn update(&mut self, rssi: f32, heading_deg: f32, has_heading: bool) -> ProximityEstimate {
        // Smooth RSSI.
        let prev = self.smoothed_rssi;
        if !self.have_rssi {
            self.smoothed_rssi = rssi;
            self.have_rssi = true;
        } else {
            self.smoothed_rssi += RSSI_EMA_ALPHA * (rssi - self.smoothed_rssi);
        }

        // Bearing: if we have a heading and a previous RSSI to compare,
        // attribute the RSSI change to the heading the user is facing NOW
        // (the direction they just moved toward). RSSI rising (less
        // negative) while facing H ⇒ peer is toward H.
        //
        // We credit the CURRENT heading, not a midpoint with the previous
        // sample's heading: the RSSI delta reflects motion that ended at
        // the current heading/position, and consecutive samples can face
        // opposite ways (the user turning), whose circular midpoint would
        // be meaningless (mean of 90° and 270° is 0° — a direction nobody
        // faced).
        if has_heading && self.have_rssi_prev {
            let delta = self.smoothed_rssi - prev; // + = getting closer
            // Decay all bins slightly so old evidence fades.
            for b in self.bin_score.iter_mut() {
                *b *= 0.98;
            }
            let bin = heading_to_bin(heading_deg);
            self.bin_score[bin] += delta;
            // Spread a fraction to adjacent bins (heading is fuzzy).
            let left = (bin + BEARING_BINS - 1) % BEARING_BINS;
            let right = (bin + 1) % BEARING_BINS;
            self.bin_score[left] += delta * 0.4;
            self.bin_score[right] += delta * 0.4;
            self.total_evidence += delta.abs();
        }
        self.have_rssi_prev = true;
        if has_heading {
            self.last_heading_deg = heading_deg;
            self.have_heading = true;
        }

        // Pick the best bin as the bearing.
        let mut best_bin = 0usize;
        let mut best_val = f32::MIN;
        for (i, &v) in self.bin_score.iter().enumerate() {
            if v > best_val {
                best_val = v;
                best_bin = i;
            }
        }
        let dist = self.distance_from_rssi(self.smoothed_rssi);
        let mut est = ProximityEstimate {
            distance_m: dist,
            band: band_for(dist),
            bearing_deg: -1.0,
            bearing_confidence: 0.0,
        };
        // Only report a bearing once there's positive evidence in the best
        // bin AND enough total accumulated movement to trust it.
        if best_val > 0.0 && self.total_evidence > 4.0 {
            est.bearing_deg = bin_to_heading(best_bin);
            // Confidence: best bin's share of total positive evidence,
            // scaled by how much total evidence we have (saturating).
            let positive_sum: f32 =
                self.bin_score.iter().filter(|&&v| v > 0.0).sum();
            let share = if positive_sum > 0.0 { best_val / positive_sum } else { 0.0 };
            let evidence_factor = (self.total_evidence / 20.0).clamp(0.0, 1.0);
            est.bearing_confidence = (share * evidence_factor).clamp(0.0, 1.0);
        }
        est
    }
}

// ─── Angle helpers ───────────────────────────────────────────────────────────

fn heading_to_bin(heading_deg: f32) -> usize {
    let h = heading_deg.rem_euclid(360.0);
    let bin = (h / (360.0 / BEARING_BINS as f32)) as usize;
    bin.min(BEARING_BINS - 1)
}

fn bin_to_heading(bin: usize) -> f32 {
    let step = 360.0 / BEARING_BINS as f32;
    (bin as f32 + 0.5) * step
}

/// Mean of two headings on the circle (handles 350°+10°=0° correctly).
/// Currently exercised only by tests; retained because heading fusion
/// (e.g. averaging compass + step-derived heading) will need it when the
/// ProximitySource feeds both sensors. Marked allow(dead_code) so the
/// non-test build doesn't warn until that consumer lands.
#[allow(dead_code)]
fn circular_mean(a_deg: f32, b_deg: f32) -> f32 {
    let a = a_deg.to_radians();
    let b = b_deg.to_radians();
    let x = a.cos() + b.cos();
    let y = a.sin() + b.sin();
    let mean = y.atan2(x).to_degrees();
    mean.rem_euclid(360.0)
}

// ─── C FFI ───────────────────────────────────────────────────────────────────

/// Create a proximity estimator. `tx_power_dbm` is the expected RSSI at 1 m
/// for the peer's radio (pass 0 to use the default), `path_loss_n` is the
/// environmental exponent (pass 0 to use the default ~2.5). Opaque handle;
/// free with `sc_proximity_free`.
#[no_mangle]
pub extern "C" fn sc_proximity_new(tx_power_dbm: f32, path_loss_n: f32) -> *mut ProximityEstimator {
    let tx = if tx_power_dbm < 0.0 { tx_power_dbm } else { DEFAULT_TX_POWER };
    let n = if path_loss_n > 0.5 { path_loss_n } else { DEFAULT_PATH_LOSS_N };
    Box::into_raw(Box::new(ProximityEstimator::new(tx, n)))
}

/// Feed one RSSI sample (dBm, negative) plus the user's current compass
/// heading (degrees, 0=N). Set `has_heading` to 0 if no compass reading is
/// available this sample (distance still updates; bearing won't). Returns
/// the current estimate. On a null handle returns an empty estimate.
///
/// # Safety
/// `handle` must be a live pointer from `sc_proximity_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_proximity_update(
    handle: *mut ProximityEstimator,
    rssi_dbm: f32,
    heading_deg: f32,
    has_heading: c_int,
) -> ProximityEstimate {
    if handle.is_null() {
        return ProximityEstimate::empty();
    }
    let est = &mut *handle;
    est.update(rssi_dbm, heading_deg, has_heading != 0)
}

/// Free a proximity estimator. NULL is a safe no-op.
///
/// # Safety
/// `handle` must be from `sc_proximity_new` and not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_proximity_free(handle: *mut ProximityEstimator) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// ABI version probe; returns 1.
#[no_mangle]
pub extern "C" fn sc_proximity_abi_version() -> c_int {
    1
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_handle_safe() {
        let e = unsafe { sc_proximity_update(std::ptr::null_mut(), -60.0, 0.0, 0) };
        assert_eq!(e.band, ProximityBand::Unknown);
        assert!(e.distance_m < 0.0);
    }

    #[test]
    fn stronger_rssi_means_closer() {
        // -50 dBm (strong) should yield a smaller distance than -90 (weak).
        let est = ProximityEstimator::new(DEFAULT_TX_POWER, DEFAULT_PATH_LOSS_N);
        let d_strong = est.distance_from_rssi(-50.0);
        let d_weak = est.distance_from_rssi(-90.0);
        assert!(d_strong < d_weak, "strong {d_strong} should be < weak {d_weak}");
    }

    #[test]
    fn bands_are_ordered() {
        assert_eq!(band_for(1.0), ProximityBand::VeryClose);
        assert_eq!(band_for(3.0), ProximityBand::Close);
        assert_eq!(band_for(10.0), ProximityBand::Near);
        assert_eq!(band_for(25.0), ProximityBand::Far);
        assert_eq!(band_for(-1.0), ProximityBand::Unknown);
    }

    #[test]
    fn at_one_metre_distance_is_about_one() {
        // At RSSI == txPower, the model gives exactly 1 m.
        let est = ProximityEstimator::new(DEFAULT_TX_POWER, DEFAULT_PATH_LOSS_N);
        let d = est.distance_from_rssi(DEFAULT_TX_POWER);
        assert!((d - 1.0).abs() < 0.01, "expected ~1m, got {d}");
    }

    #[test]
    fn circular_mean_wraps() {
        // Mean of 350 and 10 should be ~0 (360), not 180.
        let m = circular_mean(350.0, 10.0);
        assert!(m < 5.0 || m > 355.0, "circular mean wrapped wrong: {m}");
        // Mean of 80 and 100 = 90.
        let m2 = circular_mean(80.0, 100.0);
        assert!((m2 - 90.0).abs() < 1.0, "expected ~90, got {m2}");
    }

    #[test]
    fn bearing_converges_toward_improving_heading() {
        // Simulate walking: RSSI improves (gets less negative) whenever the
        // user faces ~90° (East), worsens otherwise. The estimator should
        // converge a bearing in the eastern quadrant.
        let h = sc_proximity_new(0.0, 0.0);
        assert!(!h.is_null());
        // Feed alternating samples: facing East improves, facing West worsens.
        let mut rssi = -80.0f32;
        let mut last = ProximityEstimate::empty();
        for i in 0..40 {
            let facing_east = i % 2 == 0;
            if facing_east {
                rssi += 1.5; // closer
            } else {
                rssi -= 0.5; // slightly away
            }
            rssi = rssi.clamp(-95.0, -45.0);
            let heading = if facing_east { 90.0 } else { 270.0 };
            last = unsafe { sc_proximity_update(h, rssi, heading, 1) };
        }
        unsafe { sc_proximity_free(h) };
        assert!(last.bearing_deg >= 0.0, "should have produced a bearing");
        // Bearing should land in the eastern half (roughly 45..135).
        assert!(
            last.bearing_deg > 30.0 && last.bearing_deg < 150.0,
            "expected eastern bearing, got {}",
            last.bearing_deg
        );
        assert!(last.bearing_confidence > 0.0);
    }

    #[test]
    fn distance_smooths_noisy_rssi() {
        // Feed a noisy RSSI around -70; smoothed distance shouldn't swing
        // as wildly as the per-sample distance would.
        let h = sc_proximity_new(0.0, 0.0);
        let samples = [-70.0, -64.0, -76.0, -70.0, -66.0, -74.0, -70.0];
        let mut last_d = 0.0f32;
        for &r in &samples {
            let e = unsafe { sc_proximity_update(h, r, 0.0, 0) };
            last_d = e.distance_m;
        }
        unsafe { sc_proximity_free(h) };
        // The smoothed distance should be a sane positive number in a
        // plausible band, not a wild outlier from the last noisy sample.
        assert!(last_d > 0.0 && last_d < 50.0, "smoothed distance {last_d}");
    }

    #[test]
    fn abi_version_is_one() {
        assert_eq!(sc_proximity_abi_version(), 1);
    }
}
