//! C FFI for snapclone-gstpipeline.
//!
//! Exposes the error classifier and the recovery state machine so GstLaunchBackend
//! could drive its decisions from this tested logic instead of re-deriving them
//! inline. The state machine is an opaque handle (one per backend instance).

use crate::{classify_error, GstError, RecoveryAction, RecoveryConfig, RecoverySm};
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::panic::catch_unwind;

/// Error categories (mirror GstError).
pub const SC_GST_ERR_NEGOTIATION: c_int = 0;
pub const SC_GST_ERR_GL_LINK: c_int = 1;
pub const SC_GST_ERR_MISSING_NODE: c_int = 2;
pub const SC_GST_ERR_OTHER: c_int = 3;

/// Recovery actions (mirror RecoveryAction); delay returned via out-param.
pub const SC_GST_ACT_FORCE_CPU_RETRY: c_int = 0;
pub const SC_GST_ACT_SETTLE_RETRY: c_int = 1;
pub const SC_GST_ACT_RELAXED_RETRY: c_int = 2;
pub const SC_GST_ACT_SWAP_NODE: c_int = 3;
pub const SC_GST_ACT_GIVE_UP: c_int = 4;
/// V32.37+: keep retrying the SAME node with a growing backoff (sensor settling).
pub const SC_GST_ACT_BACKOFF_RETRY: c_int = 5;

fn err_to_c(e: GstError) -> c_int {
    match e {
        GstError::Negotiation => SC_GST_ERR_NEGOTIATION,
        GstError::GlLink => SC_GST_ERR_GL_LINK,
        GstError::MissingNode => SC_GST_ERR_MISSING_NODE,
        GstError::Other => SC_GST_ERR_OTHER,
    }
}

fn err_from_c(c: c_int) -> GstError {
    match c {
        SC_GST_ERR_GL_LINK => GstError::GlLink,
        SC_GST_ERR_MISSING_NODE => GstError::MissingNode,
        SC_GST_ERR_OTHER => GstError::Other,
        _ => GstError::Negotiation,
    }
}

/// Classify a NUL-terminated stderr string. Returns SC_GST_ERR_*.
///
/// # Safety
/// `stderr` must be a valid NUL-terminated C string, or null (→ OTHER).
#[no_mangle]
pub unsafe extern "C" fn sc_gst_classify_error(stderr: *const c_char) -> c_int {
    let res = catch_unwind(|| {
        if stderr.is_null() {
            return SC_GST_ERR_OTHER;
        }
        let s = unsafe { CStr::from_ptr(stderr) }.to_string_lossy();
        err_to_c(classify_error(&s))
    });
    res.unwrap_or(SC_GST_ERR_OTHER)
}

/// Create a recovery state machine. `settle_ms`/`gpu_retry_ms` <= 0 → defaults.
#[no_mangle]
pub extern "C" fn sc_gst_recovery_new(settle_ms: c_int, gpu_retry_ms: c_int) -> *mut RecoverySm {
    let mut cfg = RecoveryConfig::default();
    if settle_ms > 0 { cfg.settle_ms = settle_ms as u32; }
    if gpu_retry_ms > 0 { cfg.gpu_retry_ms = gpu_retry_ms as u32; }
    Box::into_raw(Box::new(RecoverySm::new(cfg)))
}

/// Free a recovery state machine.
///
/// # Safety
/// `sm` must be a pointer from `sc_gst_recovery_new` (or null).
#[no_mangle]
pub unsafe extern "C" fn sc_gst_recovery_free(sm: *mut RecoverySm) {
    if !sm.is_null() {
        drop(unsafe { Box::from_raw(sm) });
    }
}

/// Model `start(node)`.
///
/// # Safety
/// `sm` from `sc_gst_recovery_new`; `node` a valid NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn sc_gst_recovery_on_start(sm: *mut RecoverySm, node: *const c_char) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| {
        if sm.is_null() || node.is_null() {
            return;
        }
        let sm = unsafe { &mut *sm };
        let node = unsafe { CStr::from_ptr(node) }.to_string_lossy().into_owned();
        sm.on_start(&node);
    }));
}

/// Model `stop()`.
///
/// # Safety
/// `sm` from `sc_gst_recovery_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_gst_recovery_on_stop(sm: *mut RecoverySm) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !sm.is_null() {
            unsafe { &mut *sm }.on_stop();
        }
    }));
}

/// Model a failure. `err` is SC_GST_ERR_*; `early_exit` != 0 if within the
/// early-exit window. Writes the delay (ms) for retry actions into `out_delay_ms`
/// if non-null. Returns SC_GST_ACT_*.
///
/// # Safety
/// `sm` from `sc_gst_recovery_new`; `out_delay_ms` null or valid.
#[no_mangle]
pub unsafe extern "C" fn sc_gst_recovery_on_failure(
    sm: *mut RecoverySm,
    err: c_int,
    early_exit: c_int,
    out_delay_ms: *mut c_int,
) -> c_int {
    let res = catch_unwind(std::panic::AssertUnwindSafe(|| {
        if sm.is_null() {
            return SC_GST_ACT_GIVE_UP;
        }
        let sm = unsafe { &mut *sm };
        let action = sm.on_failure(err_from_c(err), early_exit != 0);
        let (code, delay) = match action {
            RecoveryAction::ForceCpuAndRetry { delay_ms } => (SC_GST_ACT_FORCE_CPU_RETRY, delay_ms),
            RecoveryAction::SettleRetry { delay_ms } => (SC_GST_ACT_SETTLE_RETRY, delay_ms),
            RecoveryAction::RelaxedRetry { delay_ms } => (SC_GST_ACT_RELAXED_RETRY, delay_ms),
            RecoveryAction::BackoffRetry { delay_ms, .. } => (SC_GST_ACT_BACKOFF_RETRY, delay_ms),
            RecoveryAction::SwapNode => (SC_GST_ACT_SWAP_NODE, 0),
            RecoveryAction::GiveUp => (SC_GST_ACT_GIVE_UP, 0),
        };
        if !out_delay_ms.is_null() {
            unsafe { *out_delay_ms = delay as c_int; }
        }
        code
    }));
    res.unwrap_or(SC_GST_ACT_GIVE_UP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn ffi_classify() {
        let s = CString::new("reason not-negotiated (-4)").unwrap();
        assert_eq!(unsafe { sc_gst_classify_error(s.as_ptr()) }, SC_GST_ERR_NEGOTIATION);
        assert_eq!(unsafe { sc_gst_classify_error(std::ptr::null()) }, SC_GST_ERR_OTHER);
    }

    #[test]
    fn ffi_resume_regression() {
        let node = CString::new("camera_29").unwrap();
        let sm = sc_gst_recovery_new(0, 0);
        unsafe {
            sc_gst_recovery_on_start(sm, node.as_ptr());
            let mut delay = 0;
            // burn the settle retry
            assert_eq!(sc_gst_recovery_on_failure(sm, SC_GST_ERR_NEGOTIATION, 1, &mut delay),
                       SC_GST_ACT_SETTLE_RETRY);
            assert_eq!(delay, 600);
            sc_gst_recovery_on_start(sm, node.as_ptr()); // relaunch
            sc_gst_recovery_on_stop(sm);                 // leave page
            sc_gst_recovery_on_start(sm, node.as_ptr()); // RESUME same sensor
            // must settle-retry again, not swap/give up
            assert_eq!(sc_gst_recovery_on_failure(sm, SC_GST_ERR_NEGOTIATION, 1, &mut delay),
                       SC_GST_ACT_SETTLE_RETRY);
            sc_gst_recovery_free(sm);
        }
    }

    #[test]
    fn ffi_null_sm_safe() {
        let mut delay = -1;
        assert_eq!(unsafe {
            sc_gst_recovery_on_failure(std::ptr::null_mut(), SC_GST_ERR_NEGOTIATION, 1, &mut delay)
        }, SC_GST_ACT_GIVE_UP);
        unsafe { sc_gst_recovery_free(std::ptr::null_mut()); } // must not crash
    }
}
