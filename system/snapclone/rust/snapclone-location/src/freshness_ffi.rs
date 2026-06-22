//! C FFI for the position-freshness model.
//!
//! The wayfinder, per node, calls `sc_loc_confidence(age_secs, accuracy_m)` to
//! grade the fix, then reads `sc_loc_presentation` / `sc_loc_opacity` to decide
//! how to draw it, and `sc_loc_last_seen_label` for the "N ago" text. Pure
//! value-in / value-out — no handles, no allocation across the boundary except
//! the label, which writes into a caller buffer.

use crate::freshness::{
    confidence, last_seen_label, opacity, presentation, Confidence, FreshnessPolicy,
    PresentationTier,
};
use std::os::raw::{c_char, c_int};

/// Confidence codes.
pub const SC_LOC_CONF_LIVE: c_int = 0;
pub const SC_LOC_CONF_RECENT: c_int = 1;
pub const SC_LOC_CONF_STALE: c_int = 2;
pub const SC_LOC_CONF_LAST_KNOWN: c_int = 3;

/// Presentation tiers.
pub const SC_LOC_TIER_SOLID_ARROW: c_int = 0;
pub const SC_LOC_TIER_FADED_ARROW: c_int = 1;
pub const SC_LOC_TIER_LAST_SEEN_LABEL: c_int = 2;

fn conf_to_c(c: Confidence) -> c_int {
    match c {
        Confidence::Live => SC_LOC_CONF_LIVE,
        Confidence::Recent => SC_LOC_CONF_RECENT,
        Confidence::Stale => SC_LOC_CONF_STALE,
        Confidence::LastKnown => SC_LOC_CONF_LAST_KNOWN,
    }
}
fn conf_from_c(c: c_int) -> Confidence {
    match c {
        SC_LOC_CONF_RECENT => Confidence::Recent,
        SC_LOC_CONF_STALE => Confidence::Stale,
        SC_LOC_CONF_LAST_KNOWN => Confidence::LastKnown,
        _ => Confidence::Live,
    }
}

/// Grade a fix → SC_LOC_CONF_*. Uses the default policy. `age_secs` is how long
/// ago the fix was recorded; `accuracy_m` its error radius (0 = unknown).
#[no_mangle]
pub extern "C" fn sc_loc_confidence(age_secs: i64, accuracy_m: f32) -> c_int {
    conf_to_c(confidence(&FreshnessPolicy::default(), age_secs, accuracy_m))
}

/// Grade a fix with EXPLICIT policy thresholds (for tuning from config).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn sc_loc_confidence_ex(
    age_secs: i64, accuracy_m: f32,
    live_age: i64, recent_age: i64, stale_age: i64,
    precise_acc: f32, coarse_acc: f32,
) -> c_int {
    let p = FreshnessPolicy {
        live_age_secs: live_age,
        recent_age_secs: recent_age,
        stale_age_secs: stale_age,
        precise_accuracy_m: precise_acc,
        coarse_accuracy_m: coarse_acc,
    };
    conf_to_c(confidence(&p, age_secs, accuracy_m))
}

/// Map a confidence code → presentation tier (SC_LOC_TIER_*).
#[no_mangle]
pub extern "C" fn sc_loc_presentation(conf: c_int) -> c_int {
    match presentation(conf_from_c(conf)) {
        PresentationTier::SolidArrow => SC_LOC_TIER_SOLID_ARROW,
        PresentationTier::FadedArrow => SC_LOC_TIER_FADED_ARROW,
        PresentationTier::LastSeenLabel => SC_LOC_TIER_LAST_SEEN_LABEL,
    }
}

/// Opacity multiplier (0..1) for a confidence code.
#[no_mangle]
pub extern "C" fn sc_loc_opacity(conf: c_int) -> f32 {
    opacity(conf_from_c(conf))
}

/// Write a compact "N ago" label (NUL-terminated) into `out`. Returns the
/// number of bytes written (excluding NUL), or -1 if the buffer is too small or
/// `out` is null. 16 bytes is always enough.
///
/// # Safety
/// `out` must be valid for `out_cap` bytes.
#[no_mangle]
pub unsafe extern "C" fn sc_loc_last_seen_label(
    age_secs: i64, out: *mut c_char, out_cap: usize,
) -> c_int {
    if out.is_null() {
        return -1;
    }
    let s = last_seen_label(age_secs);
    let bytes = s.as_bytes();
    if bytes.len() + 1 > out_cap {
        return -1;
    }
    let dst = unsafe { std::slice::from_raw_parts_mut(out as *mut u8, bytes.len() + 1) };
    dst[..bytes.len()].copy_from_slice(bytes);
    dst[bytes.len()] = 0;
    bytes.len() as c_int
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_confidence_tiers() {
        assert_eq!(sc_loc_confidence(10, 20.0), SC_LOC_CONF_LIVE);
        assert_eq!(sc_loc_confidence(300, 10.0), SC_LOC_CONF_RECENT);
        assert_eq!(sc_loc_confidence(1800, 10.0), SC_LOC_CONF_STALE);
        assert_eq!(sc_loc_confidence(7200, 10.0), SC_LOC_CONF_LAST_KNOWN);
        // coarse fresh fix capped
        assert_eq!(sc_loc_confidence(5, 5000.0), SC_LOC_CONF_STALE);
    }

    #[test]
    fn ffi_presentation_and_opacity() {
        assert_eq!(sc_loc_presentation(SC_LOC_CONF_LIVE), SC_LOC_TIER_SOLID_ARROW);
        assert_eq!(sc_loc_presentation(SC_LOC_CONF_LAST_KNOWN), SC_LOC_TIER_LAST_SEEN_LABEL);
        assert!(sc_loc_opacity(SC_LOC_CONF_LIVE) > sc_loc_opacity(SC_LOC_CONF_STALE));
    }

    #[test]
    fn ffi_explicit_policy() {
        // With a tight 10s live window, a 30s fix is no longer Live.
        assert_eq!(sc_loc_confidence_ex(30, 10.0, 10, 60, 300, 50.0, 1000.0),
                   SC_LOC_CONF_RECENT);
    }

    #[test]
    fn ffi_last_seen_label() {
        let mut buf = [0i8; 16];
        let n = unsafe { sc_loc_last_seen_label(7200, buf.as_mut_ptr(), buf.len()) };
        assert!(n > 0);
        let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_str().unwrap();
        assert_eq!(s, "2h ago");
        // too-small buffer → -1
        let mut tiny = [0i8; 2];
        assert_eq!(unsafe { sc_loc_last_seen_label(7200, tiny.as_mut_ptr(), tiny.len()) }, -1);
    }
}
