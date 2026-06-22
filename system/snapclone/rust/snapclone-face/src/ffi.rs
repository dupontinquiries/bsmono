//! C FFI for snapclone-face.
//!
//! Exposes the face library (Phase 1 of the face-bandwidth project)
//! to C/C++ callers. Follows the same `sc_` prefix and error-enum
//! pattern as `snapclone-crypto`'s FFI:
//!
//!   - All functions are `extern "C"`, `#[no_mangle]`
//!   - Opaque handle returned by an "open" function, freed by a
//!     matching "close" function
//!   - Errors are returned as a thin `FaceFfiError` enum, not via
//!     panicking or by-value Rust types
//!   - All input pointers are checked for null; passing null returns
//!     `InvalidInput` rather than crashing
//!   - String pointers are assumed to be `\0`-terminated UTF-8;
//!     callers are responsible for keeping them alive for the
//!     duration of the call (Rust copies what it needs)
//!
//! The C header at `include/face/snapclone_face_ffi.h` is the canonical
//! contract; this module's signatures must stay in sync. Hand-curated
//! today; could be cbindgen-generated later.
//!
//! ## Why this module relaxes the crate-wide forbid(unsafe_code)
//!
//! The crate root has `#![forbid(unsafe_code)]` as defensive hygiene
//! for the Rust-only modules (library, detector, reference, consent),
//! none of which need raw pointer manipulation. The FFI module is the
//! one necessary exception: C FFI declarations require `unsafe extern
//! "C"` and `#[no_mangle]`, and both are flagged by `forbid(unsafe_code)`.
//! We narrowly allow them here so the rest of the crate keeps its
//! safety guarantee.

#![allow(unsafe_code)]

use crate::library::{Crop, Label, Library, InMemoryLibrary};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::Arc;

/// Error codes returned across the FFI boundary. Stable wire format —
/// new variants append, existing values never change.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceFfiError {
    /// Operation succeeded.
    Ok                 = 0,
    /// A required pointer was null, or an input value was invalid.
    InvalidInput       = 1,
    /// The library is disabled (consent off); the operation refused.
    ConsentDisabled    = 2,
    /// Caller-supplied buffer was too small for the requested data.
    BufferTooSmall     = 3,
    /// A label kind value didn't match any known enum variant.
    InvalidLabel       = 4,
    /// An internal error (e.g. lock poisoning). Should never occur
    /// in normal operation; indicates a bug.
    Internal           = 99,
}

/// Label kind discriminator passed across the FFI as a single int.
///
/// C-callers pair this with a `contact_id` C-string:
///   - kind = SelfFace (0): contact_id is ignored (pass NULL)
///   - kind = Contact  (1): contact_id is the UTF-8 contact identifier
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceLabelKind {
    /// User's own face — front camera selfies.
    SelfFace = 0,
    /// A face from a snap received from a specific contact.
    Contact  = 1,
}

/// Opaque handle to a face library instance.
///
/// Cast to a `*mut FaceLibraryHandle` from C; treat as opaque.
/// Lifetime: created via `sc_face_library_open`, destroyed by
/// `sc_face_library_close`. Operations on a closed handle have
/// undefined behavior — caller responsibility.
pub struct FaceLibraryHandle {
    inner: Arc<dyn Library>,
}

/// Create a new in-memory face library.
///
/// Returns a non-null handle on success, null on allocation failure
/// (vanishingly rare). The library starts ENABLED. Caller takes
/// ownership and must eventually call `sc_face_library_close`.
///
/// `budget_bytes = 0` means "use the crate default" (currently
/// `DEFAULT_LIBRARY_BUDGET_BYTES`, see library.rs).
#[no_mangle]
pub extern "C" fn sc_face_library_open(
    budget_bytes: usize,
) -> *mut FaceLibraryHandle {
    let lib: Arc<dyn Library> = if budget_bytes == 0 {
        Arc::new(InMemoryLibrary::new())
    } else {
        Arc::new(InMemoryLibrary::with_budget(budget_bytes))
    };
    Box::into_raw(Box::new(FaceLibraryHandle { inner: lib }))
}

/// Destroy a face library handle, releasing all stored crops.
///
/// # Safety
/// `handle` must have been returned by `sc_face_library_open` and
/// not previously closed. Passing null is safe and is a no-op.
#[no_mangle]
pub unsafe extern "C" fn sc_face_library_close(handle: *mut FaceLibraryHandle) {
    if handle.is_null() { return; }
    drop(Box::from_raw(handle));
}

/// Add a face crop to the library.
///
/// # Safety
/// `handle` must be a live handle from `sc_face_library_open`.
/// `contact_id` must be either null (when label_kind == SelfFace) or
/// a `\0`-terminated UTF-8 string (when label_kind == Contact).
/// `bytes` must point to `bytes_len` readable bytes.
/// `content_hash` must point to 32 readable bytes.
///
/// Per-input numeric arguments are by-value, so no aliasing concerns.
#[no_mangle]
pub unsafe extern "C" fn sc_face_library_add(
    handle: *mut FaceLibraryHandle,
    label_kind: FaceLabelKind,
    contact_id: *const c_char,
    content_hash: *const u8,
    bytes: *const u8,
    bytes_len: usize,
    width: u32,
    height: u32,
    captured_at_ms: i64,
    quality: f32,
) -> FaceFfiError {
    if handle.is_null() || content_hash.is_null() {
        return FaceFfiError::InvalidInput;
    }
    if bytes.is_null() && bytes_len > 0 {
        return FaceFfiError::InvalidInput;
    }
    // Build the Rust Label from the kind + optional contact id.
    let label = match label_kind {
        FaceLabelKind::SelfFace => Label::SelfFace,
        FaceLabelKind::Contact  => {
            if contact_id.is_null() {
                return FaceFfiError::InvalidInput;
            }
            let cstr = CStr::from_ptr(contact_id);
            match cstr.to_str() {
                Ok(s) => Label::Contact(s.to_string()),
                Err(_) => return FaceFfiError::InvalidInput,
            }
        }
    };
    // Copy the hash and byte payload into owned buffers — once we
    // return, the C caller is free to invalidate its memory.
    let mut hash_buf = [0u8; 32];
    std::ptr::copy_nonoverlapping(content_hash, hash_buf.as_mut_ptr(), 32);
    let byte_vec: Vec<u8> = if bytes_len == 0 {
        Vec::new()
    } else {
        std::slice::from_raw_parts(bytes, bytes_len).to_vec()
    };

    let crop = Crop {
        label,
        content_hash: hash_buf,
        bytes: byte_vec,
        width, height,
        captured_at_ms,
        last_used_ms: captured_at_ms,
        quality,
    };
    let handle_ref = &*handle;
    match handle_ref.inner.add(crop) {
        Ok(())                                   => FaceFfiError::Ok,
        Err(crate::FaceError::ConsentDisabled)    => FaceFfiError::ConsentDisabled,
        Err(_)                                    => FaceFfiError::Internal,
    }
}

/// Wipe all crops from the library. Always succeeds regardless of
/// consent state — this IS the consent-withdrawal action.
///
/// Writes the number of removed crops to `*removed_out` if non-null.
///
/// # Safety
/// `handle` must be a live handle from `sc_face_library_open`.
/// `removed_out` may be null; if non-null, must point to writable
/// `usize` storage.
#[no_mangle]
pub unsafe extern "C" fn sc_face_library_wipe(
    handle: *mut FaceLibraryHandle,
    removed_out: *mut usize,
) -> FaceFfiError {
    if handle.is_null() {
        return FaceFfiError::InvalidInput;
    }
    let handle_ref = &*handle;
    let n = handle_ref.inner.wipe();
    if !removed_out.is_null() {
        *removed_out = n;
    }
    FaceFfiError::Ok
}

/// Enable or disable the library. Disabling refuses future writes and
/// makes reads return empty. Does NOT wipe stored data — call
/// `sc_face_library_wipe` separately for that.
///
/// # Safety
/// `handle` must be a live handle from `sc_face_library_open`.
#[no_mangle]
pub unsafe extern "C" fn sc_face_library_set_enabled(
    handle: *mut FaceLibraryHandle,
    enabled: bool,
) -> FaceFfiError {
    if handle.is_null() {
        return FaceFfiError::InvalidInput;
    }
    let handle_ref = &*handle;
    handle_ref.inner.set_enabled(enabled);
    FaceFfiError::Ok
}

/// Read the current enabled state.
///
/// # Safety
/// `handle` must be a live handle from `sc_face_library_open`.
/// Returns `false` for a null handle (sane default for "library
/// unavailable").
#[no_mangle]
pub unsafe extern "C" fn sc_face_library_is_enabled(
    handle: *mut FaceLibraryHandle,
) -> bool {
    if handle.is_null() { return false; }
    let handle_ref = &*handle;
    handle_ref.inner.is_enabled()
}

/// Return total bytes currently stored. Returns 0 for null handle.
///
/// # Safety
/// `handle` must be a live handle from `sc_face_library_open`.
#[no_mangle]
pub unsafe extern "C" fn sc_face_library_total_bytes(
    handle: *mut FaceLibraryHandle,
) -> usize {
    if handle.is_null() { return 0; }
    let handle_ref = &*handle;
    handle_ref.inner.total_bytes()
}

/// Return the count of crops stored for a specific label.
///
/// For SelfFace, `contact_id` is ignored. For Contact, `contact_id`
/// must be a `\0`-terminated UTF-8 string.
///
/// Writes the count to `*count_out`. Returns `InvalidInput` for null
/// handle or null count_out.
///
/// # Safety
/// See per-arg notes above. `count_out` must point to writable usize.
#[no_mangle]
pub unsafe extern "C" fn sc_face_library_label_count(
    handle: *mut FaceLibraryHandle,
    label_kind: FaceLabelKind,
    contact_id: *const c_char,
    count_out: *mut usize,
) -> FaceFfiError {
    if handle.is_null() || count_out.is_null() {
        return FaceFfiError::InvalidInput;
    }
    let label = match label_kind {
        FaceLabelKind::SelfFace => Label::SelfFace,
        FaceLabelKind::Contact  => {
            if contact_id.is_null() {
                return FaceFfiError::InvalidInput;
            }
            let cstr = CStr::from_ptr(contact_id);
            match cstr.to_str() {
                Ok(s) => Label::Contact(s.to_string()),
                Err(_) => return FaceFfiError::InvalidInput,
            }
        }
    };
    let handle_ref = &*handle;
    let counts = handle_ref.inner.label_counts();
    *count_out = counts.get(&label).copied().unwrap_or(0);
    FaceFfiError::Ok
}

// ═══════════════════════════════════════════════════════════════════════════
// ── Detector FFI ──────────────────────────────────────────────────────────
// ═══════════════════════════════════════════════════════════════════════════
//
// V32.32: real YOLO face detection exposed to C++ via a separate opaque
// handle. The Library and Detector are independent — callers can open
// one without the other. Typical usage:
//
//   1. App startup: sc_face_detector_open(model_path) once
//   2. Per frame: sc_face_detector_detect(handle, rgb, ...) — gets bboxes
//   3. Per face: crop the source frame to the bbox; pass to library_add
//   4. App shutdown: sc_face_detector_close
//
// The detector is thread-safe; multiple threads can call detect()
// concurrently against the same handle. We provide an opaque handle
// rather than process-global singleton so callers control lifetime
// (allows multiple detectors for A/B testing different model files).

/// Opaque handle to a Detector instance.
pub struct FaceDetectorHandle {
    inner: std::sync::Arc<crate::detector::Detector>,
}

/// Open a face detector by loading a YOLO ONNX model from disk.
///
/// Returns a non-null handle on success. On failure, returns null AND
/// writes the error code to `*error_out` if `error_out` is non-null
/// (callers may pass null when they don't care about the specific reason).
///
/// `model_path` must be a `\0`-terminated UTF-8 path to a YOLO-class
/// ONNX file. The model is loaded immediately (50-200ms) so call this
/// at startup, not in a per-frame hot path.
///
/// `conf_threshold` and `iou_threshold` are detector tuning. Pass
/// negative values to use the crate defaults (0.45 each).
///
/// # Safety
/// `model_path` must be a valid `\0`-terminated C string for the
/// duration of this call. `error_out` may be null; if non-null must
/// point to writable storage for FaceFfiError.
#[no_mangle]
pub unsafe extern "C" fn sc_face_detector_open(
    model_path: *const c_char,
    conf_threshold: f32,
    iou_threshold: f32,
    error_out: *mut FaceFfiError,
) -> *mut FaceDetectorHandle {
    let set_err = |e: FaceFfiError| {
        if !error_out.is_null() { *error_out = e; }
    };
    if model_path.is_null() {
        set_err(FaceFfiError::InvalidInput);
        return std::ptr::null_mut();
    }
    let path_str = match CStr::from_ptr(model_path).to_str() {
        Ok(s) => s,
        Err(_) => {
            set_err(FaceFfiError::InvalidInput);
            return std::ptr::null_mut();
        }
    };
    let config = crate::detector::DetectorConfig {
        conf_threshold: if conf_threshold < 0.0 { 0.45 } else { conf_threshold },
        iou_threshold:  if iou_threshold  < 0.0 { 0.45 } else { iou_threshold  },
    };
    match crate::detector::Detector::from_onnx(path_str, config) {
        Ok(det) => {
            set_err(FaceFfiError::Ok);
            Box::into_raw(Box::new(FaceDetectorHandle {
                inner: std::sync::Arc::new(det),
            }))
        }
        Err(_) => {
            // Specific error variant (ModelLoad vs Inference vs ...) is
            // logged by the Rust side; FFI surface only distinguishes
            // "could not construct" from "ok."
            set_err(FaceFfiError::Internal);
            std::ptr::null_mut()
        }
    }
}

/// Destroy a detector handle. Passing null is safe.
///
/// # Safety
/// `handle` must have been returned by `sc_face_detector_open` and not
/// previously closed.
#[no_mangle]
pub unsafe extern "C" fn sc_face_detector_close(handle: *mut FaceDetectorHandle) {
    if handle.is_null() { return; }
    drop(Box::from_raw(handle));
}

/// Run detection on an RGB image.
///
/// The caller pre-allocates five parallel output arrays each of length
/// `max_results`. On success, the first `*count_out` entries of each
/// are populated with the detections (sorted by confidence descending
/// per NMS). When more faces are detected than `max_results`, the top
/// `max_results` by confidence are returned and the count is clamped.
///
/// Parallel arrays rather than struct-of-bbox to avoid needing a
/// C-side struct definition that has to stay in sync with Rust. Each
/// out_* array is independent and may be allocated wherever convenient.
///
///   rgb         — width × height × 3 RGB888 bytes (tightly packed)
///   width/height — image dimensions
///   max_results — capacity of each output array
///   out_x_min, out_y_min, out_x_max, out_y_max — bbox in source pixels
///   out_confidence — model confidence in [0, 1]
///   count_out   — number of detections actually written
///
/// Returns `Ok` on success, `InvalidInput` for null/bad arguments,
/// `Internal` if inference failed at runtime.
///
/// # Safety
/// `handle` must be a live handle. `rgb` must point to at least
/// `width*height*3` readable bytes. Each output array must point to
/// at least `max_results` writable elements (or 0 if max_results=0).
/// `count_out` must point to a writable `usize`.
#[no_mangle]
pub unsafe extern "C" fn sc_face_detector_detect(
    handle: *mut FaceDetectorHandle,
    rgb: *const u8,
    width: u32,
    height: u32,
    max_results: usize,
    out_x_min: *mut f32,
    out_y_min: *mut f32,
    out_x_max: *mut f32,
    out_y_max: *mut f32,
    out_confidence: *mut f32,
    count_out: *mut usize,
) -> FaceFfiError {
    if handle.is_null() || rgb.is_null() || count_out.is_null() {
        return FaceFfiError::InvalidInput;
    }
    // If max_results > 0, the output pointers must be non-null. When
    // max_results == 0 the caller is asking "how many would there be?"
    // — we run detection but skip the copy step.
    if max_results > 0 && (
        out_x_min.is_null() || out_y_min.is_null()
        || out_x_max.is_null() || out_y_max.is_null()
        || out_confidence.is_null()
    ) {
        return FaceFfiError::InvalidInput;
    }
    let rgb_len = match (width as usize)
        .checked_mul(height as usize)
        .and_then(|v| v.checked_mul(3)) {
        Some(n) => n,
        None    => return FaceFfiError::InvalidInput,
    };
    let rgb_slice = std::slice::from_raw_parts(rgb, rgb_len);

    let handle_ref = &*handle;
    let detections = match handle_ref.inner.detect(rgb_slice, width, height) {
        Ok(d)  => d,
        Err(crate::FaceError::InvalidInput(_)) => return FaceFfiError::InvalidInput,
        Err(_) => return FaceFfiError::Internal,
    };

    let n = detections.len().min(max_results);
    for i in 0..n {
        let d = &detections[i];
        *out_x_min.add(i)      = d.x_min;
        *out_y_min.add(i)      = d.y_min;
        *out_x_max.add(i)      = d.x_max;
        *out_y_max.add(i)      = d.y_max;
        *out_confidence.add(i) = d.confidence;
    }
    *count_out = n;
    FaceFfiError::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn open_close_roundtrip() {
        let h = sc_face_library_open(0);
        assert!(!h.is_null());
        unsafe { sc_face_library_close(h); }
    }

    #[test]
    fn close_null_is_safe() {
        unsafe { sc_face_library_close(std::ptr::null_mut()); }
    }

    #[test]
    fn add_self_face_increments_count() {
        let h = sc_face_library_open(0);
        let hash = [0xABu8; 32];
        let bytes = vec![0u8; 100];
        let err = unsafe {
            sc_face_library_add(
                h,
                FaceLabelKind::SelfFace,
                std::ptr::null(),
                hash.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
                112, 112,
                1700000000000,
                0.8,
            )
        };
        assert_eq!(err, FaceFfiError::Ok);
        let mut count = 0usize;
        let err = unsafe {
            sc_face_library_label_count(
                h, FaceLabelKind::SelfFace, std::ptr::null(), &mut count,
            )
        };
        assert_eq!(err, FaceFfiError::Ok);
        assert_eq!(count, 1);
        unsafe { sc_face_library_close(h); }
    }

    #[test]
    fn add_contact_face_with_utf8_id() {
        let h = sc_face_library_open(0);
        let id = CString::new("alice-pk-abc").unwrap();
        let hash = [0xCDu8; 32];
        let bytes = vec![0u8; 50];
        let err = unsafe {
            sc_face_library_add(
                h,
                FaceLabelKind::Contact,
                id.as_ptr(),
                hash.as_ptr(),
                bytes.as_ptr(),
                bytes.len(),
                100, 100, 1700000000000, 0.5,
            )
        };
        assert_eq!(err, FaceFfiError::Ok);
        let mut count = 0usize;
        let err = unsafe {
            sc_face_library_label_count(
                h, FaceLabelKind::Contact, id.as_ptr(), &mut count,
            )
        };
        assert_eq!(err, FaceFfiError::Ok);
        assert_eq!(count, 1);
        unsafe { sc_face_library_close(h); }
    }

    #[test]
    fn add_with_null_handle_refuses() {
        let hash = [0u8; 32];
        let err = unsafe {
            sc_face_library_add(
                std::ptr::null_mut(),
                FaceLabelKind::SelfFace,
                std::ptr::null(),
                hash.as_ptr(),
                std::ptr::null(), 0,
                100, 100, 0, 0.0,
            )
        };
        assert_eq!(err, FaceFfiError::InvalidInput);
    }

    #[test]
    fn add_contact_requires_id() {
        let h = sc_face_library_open(0);
        let hash = [0u8; 32];
        let err = unsafe {
            sc_face_library_add(
                h,
                FaceLabelKind::Contact,
                std::ptr::null(),  // null id with Contact kind = error
                hash.as_ptr(),
                std::ptr::null(), 0,
                100, 100, 0, 0.0,
            )
        };
        assert_eq!(err, FaceFfiError::InvalidInput);
        unsafe { sc_face_library_close(h); }
    }

    #[test]
    fn disabled_library_refuses_add() {
        let h = sc_face_library_open(0);
        unsafe { sc_face_library_set_enabled(h, false); }
        let hash = [0u8; 32];
        let err = unsafe {
            sc_face_library_add(
                h, FaceLabelKind::SelfFace, std::ptr::null(),
                hash.as_ptr(), std::ptr::null(), 0,
                100, 100, 0, 0.0,
            )
        };
        assert_eq!(err, FaceFfiError::ConsentDisabled);
        unsafe { sc_face_library_close(h); }
    }

    #[test]
    fn wipe_clears_and_reports_count() {
        let h = sc_face_library_open(0);
        let hash1 = [1u8; 32];
        let hash2 = [2u8; 32];
        unsafe {
            sc_face_library_add(h, FaceLabelKind::SelfFace, std::ptr::null(),
                hash1.as_ptr(), std::ptr::null(), 0, 100, 100, 100, 0.0);
            sc_face_library_add(h, FaceLabelKind::SelfFace, std::ptr::null(),
                hash2.as_ptr(), std::ptr::null(), 0, 100, 100, 200, 0.0);
        }
        let mut removed = 0usize;
        let err = unsafe { sc_face_library_wipe(h, &mut removed) };
        assert_eq!(err, FaceFfiError::Ok);
        assert_eq!(removed, 2);
        let mut count = 0usize;
        unsafe {
            sc_face_library_label_count(h, FaceLabelKind::SelfFace,
                std::ptr::null(), &mut count);
        }
        assert_eq!(count, 0);
        unsafe { sc_face_library_close(h); }
    }

    #[test]
    fn null_handle_reads_are_safe() {
        unsafe {
            assert!(!sc_face_library_is_enabled(std::ptr::null_mut()));
            assert_eq!(sc_face_library_total_bytes(std::ptr::null_mut()), 0);
        }
    }

    // ── Detector FFI tests ────────────────────────────────────────────
    // We don't have a model file in this test context, so we can only
    // exercise the null-pointer / bad-argument paths. Real inference
    // is exercised by the higher-level integration tests (which need
    // a model file checked in or downloaded out-of-band).

    #[test]
    fn detector_open_null_path_returns_null() {
        let mut err = FaceFfiError::Ok;
        let h = unsafe {
            sc_face_detector_open(std::ptr::null(), -1.0, -1.0, &mut err)
        };
        assert!(h.is_null());
        assert_eq!(err, FaceFfiError::InvalidInput);
    }

    #[test]
    fn detector_open_missing_file_returns_null() {
        let p = CString::new("/no/such/model.onnx").unwrap();
        let mut err = FaceFfiError::Ok;
        let h = unsafe {
            sc_face_detector_open(p.as_ptr(), -1.0, -1.0, &mut err)
        };
        assert!(h.is_null());
        // Could be Internal (load failed) or other — we don't pin
        // the specific variant, just that it's not Ok.
        assert_ne!(err, FaceFfiError::Ok);
    }

    #[test]
    fn detector_close_null_is_safe() {
        unsafe { sc_face_detector_close(std::ptr::null_mut()); }
    }

    #[test]
    fn detector_detect_null_handle_returns_invalid_input() {
        let rgb = vec![0u8; 320 * 320 * 3];
        let mut count = 0usize;
        let err = unsafe {
            sc_face_detector_detect(
                std::ptr::null_mut(),
                rgb.as_ptr(),
                320, 320,
                0,
                std::ptr::null_mut(), std::ptr::null_mut(),
                std::ptr::null_mut(), std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut count,
            )
        };
        assert_eq!(err, FaceFfiError::InvalidInput);
    }
}
