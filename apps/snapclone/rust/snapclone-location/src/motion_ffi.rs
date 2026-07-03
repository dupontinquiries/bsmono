//! C FFI for the motion model.
//!
//! Two uses:
//!  - **Sender side**: `sc_motion_classify` turns an accelerometer window summary
//!    into an activity code to attach to the outbound location.
//!  - **Either side**: `sc_motion_project` projects a fix forward by the motion
//!    state and elapsed time, returning the projected lat/lon + an honest
//!    uncertainty radius the wayfinder widens the node halo by.

use crate::motion::{
    classify_activity, project, Activity, ActivityPolicy, MotionState, MotionSummary,
};
use std::os::raw::c_int;

/// Activity codes.
pub const SC_ACT_STATIONARY: c_int = 0;
pub const SC_ACT_ON_FOOT: c_int = 1;
pub const SC_ACT_VEHICLE: c_int = 2;

fn act_to_c(a: Activity) -> c_int {
    match a {
        Activity::Stationary => SC_ACT_STATIONARY,
        Activity::OnFoot => SC_ACT_ON_FOOT,
        Activity::Vehicle => SC_ACT_VEHICLE,
    }
}
fn act_from_c(c: c_int) -> Activity {
    match c {
        SC_ACT_ON_FOOT => Activity::OnFoot,
        SC_ACT_VEHICLE => Activity::Vehicle,
        _ => Activity::Stationary,
    }
}

/// Classify an accelerometer window (default policy) → SC_ACT_*.
#[no_mangle]
pub extern "C" fn sc_motion_classify(
    accel_std: f32, cadence_spm: f32, speed_mps: f32,
) -> c_int {
    let m = MotionSummary { accel_std, cadence_spm, speed_mps };
    act_to_c(classify_activity(&ActivityPolicy::default(), &m))
}

/// Project a fix forward. Inputs are the base fix (lat/lon/accuracy), the motion
/// state (activity code + heading + steps/stride for foot, speed for vehicle),
/// and elapsed seconds since the fix. Outputs via the `out_*` pointers:
/// projected lat/lon, uncertainty radius (m), and distance projected (m).
///
/// # Safety
/// All `out_*` pointers must be valid (non-null).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn sc_motion_project(
    base_lat: f64, base_lon: f64, base_accuracy_m: f32,
    activity: c_int, heading_deg: f32, steps: u32, stride_m: f32, speed_mps: f32,
    elapsed_secs: f64,
    out_lat: *mut f64, out_lon: *mut f64,
    out_uncertainty_m: *mut f32, out_projected_m: *mut f32,
) -> c_int {
    if out_lat.is_null() || out_lon.is_null()
        || out_uncertainty_m.is_null() || out_projected_m.is_null() {
        return -1;
    }
    let motion = MotionState {
        activity: act_from_c(activity),
        heading_deg, steps, stride_m, speed_mps,
    };
    let p = project(base_lat, base_lon, base_accuracy_m, &motion, elapsed_secs);
    unsafe {
        *out_lat = p.lat;
        *out_lon = p.lon;
        *out_uncertainty_m = p.uncertainty_m;
        *out_projected_m = p.projected_m;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_classify() {
        assert_eq!(sc_motion_classify(0.05, 0.0, 0.0), SC_ACT_STATIONARY);
        assert_eq!(sc_motion_classify(1.2, 110.0, 1.4), SC_ACT_ON_FOOT);
        assert_eq!(sc_motion_classify(0.4, 0.0, 15.0), SC_ACT_VEHICLE);
    }

    #[test]
    fn ffi_project_foot() {
        let (mut lat, mut lon, mut unc, mut proj) = (0.0f64, 0.0f64, 0.0f32, 0.0f32);
        let rc = unsafe {
            sc_motion_project(45.0, -73.0, 10.0,
                SC_ACT_ON_FOOT, 90.0, 100, 0.7, 0.0, 60.0,
                &mut lat, &mut lon, &mut unc, &mut proj)
        };
        assert_eq!(rc, 0);
        assert!((proj - 70.0).abs() < 0.5);
        assert!(lon > -73.0);          // moved east
        assert!(unc > 10.0);           // uncertainty grew
    }

    #[test]
    fn ffi_project_stationary_is_identity() {
        let (mut lat, mut lon, mut unc, mut proj) = (0.0f64, 0.0f64, 0.0f32, 0.0f32);
        unsafe {
            sc_motion_project(45.0, -73.0, 12.0,
                SC_ACT_STATIONARY, 0.0, 0, 0.7, 0.0, 120.0,
                &mut lat, &mut lon, &mut unc, &mut proj);
        }
        assert_eq!((lat, lon, proj), (45.0, -73.0, 0.0));
        assert_eq!(unc, 12.0);
    }

    #[test]
    fn ffi_null_out_safe() {
        let rc = unsafe {
            sc_motion_project(45.0, -73.0, 10.0, SC_ACT_ON_FOOT, 0.0, 1, 0.7, 0.0, 1.0,
                std::ptr::null_mut(), std::ptr::null_mut(),
                std::ptr::null_mut(), std::ptr::null_mut())
        };
        assert_eq!(rc, -1);
    }
}
