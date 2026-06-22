//! C FFI for `snapclone-vqcodec`.
//!
//! Exposes the shared-codebook image codec to C/C++ callers. Follows the same
//! conventions as `snapclone-face`'s FFI:
//!
//!   - All functions are `extern "C"`, `#[no_mangle]`, `sc_vq_`-prefixed.
//!   - The codebook (the shared lookup table) is an opaque handle created by a
//!     `*_train` / `*_from_bytes` function and released by `sc_vq_codebook_free`.
//!   - Variable-length outputs (serialized codebook, encoded frame, decoded
//!     luma) use the two-call buffer pattern: call once with `out == NULL` to
//!     learn the required length via `*out_len`, allocate, then call again.
//!   - The codec owns ONLY the opaque codebook; every byte buffer is
//!     caller-owned, so there is nothing C-side code must free except the
//!     handle. No Rust-allocated buffers cross the boundary.
//!   - All pointers are null-checked; a null required pointer returns
//!     `InvalidInput` rather than crashing.
//!
//! The C header at `include/vqcodec/snapclone_vqcodec_ffi.h` is the canonical
//! contract; keep the two in sync by hand (cbindgen could automate later).
//!
//! ## Why this module relaxes the crate-wide unsafe lint
//!
//! The crate root is `#![deny(unsafe_code)]` (defensive hygiene for the pure
//! Rust codec). C FFI fundamentally requires `unsafe` (raw pointer reads,
//! `Box::from_raw`) and `#[no_mangle]`, both flagged by the lint. We narrowly
//! allow them here so the rest of the crate keeps its guarantee.

#![allow(unsafe_code)]

use crate::{decode, encode, train_codebook, Codebook, Encoded, GrayImage};

/// Status codes returned across the FFI boundary. Stable wire format — new
/// variants append, existing values never change.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScVqStatus {
    /// Operation succeeded.
    Ok = 0,
    /// A required pointer was null, or a dimension/length was invalid.
    InvalidInput = 1,
    /// A serialized blob (codebook or encoded frame) failed to parse.
    ParseError = 2,
    /// The caller-supplied buffer was too small; `*out_len` holds the need.
    BufferTooSmall = 3,
    /// Internal error — should not occur in normal operation.
    Internal = 99,
}

/// Opaque handle to a codebook (the shared lookup table). Created by
/// `sc_vq_codebook_train` or `sc_vq_codebook_from_bytes`; released by
/// `sc_vq_codebook_free`. Treat as opaque from C.
pub struct ScVqCodebook {
    inner: Codebook,
}

/// Train a codebook from a single luma image. `luma` is `width*height` bytes
/// (row-major). Returns null on invalid input.
#[no_mangle]
pub unsafe extern "C" fn sc_vq_codebook_train(
    luma: *const u8,
    len: usize,
    width: u32,
    height: u32,
    block: u32,
    k: u32,
    iters: u32,
    seed: u64,
) -> *mut ScVqCodebook {
    if luma.is_null() || width == 0 || height == 0 || block == 0 {
        return std::ptr::null_mut();
    }
    let need = match (width as usize).checked_mul(height as usize) {
        Some(n) => n,
        None => return std::ptr::null_mut(),
    };
    if len < need {
        return std::ptr::null_mut();
    }
    let data = std::slice::from_raw_parts(luma, need).to_vec();
    let img = GrayImage { width: width as usize, height: height as usize, data };
    let cb = train_codebook(&[img], block as usize, k as usize, iters as usize, seed);
    Box::into_raw(Box::new(ScVqCodebook { inner: cb }))
}

/// Load a codebook from a blob produced by `sc_vq_codebook_to_bytes`. Returns
/// null on a bad/short blob.
#[no_mangle]
pub unsafe extern "C" fn sc_vq_codebook_from_bytes(
    data: *const u8,
    len: usize,
) -> *mut ScVqCodebook {
    if data.is_null() || len == 0 {
        return std::ptr::null_mut();
    }
    let bytes = std::slice::from_raw_parts(data, len);
    match Codebook::from_bytes(bytes) {
        Some(cb) => Box::into_raw(Box::new(ScVqCodebook { inner: cb })),
        None => std::ptr::null_mut(),
    }
}

/// Release a codebook handle. Null is a no-op.
#[no_mangle]
pub unsafe extern "C" fn sc_vq_codebook_free(cb: *mut ScVqCodebook) {
    if !cb.is_null() {
        drop(Box::from_raw(cb));
    }
}

/// Bits per block index for this codebook (`ceil(log2(k))`). 0 if `cb` is null.
#[no_mangle]
pub unsafe extern "C" fn sc_vq_codebook_index_bits(cb: *const ScVqCodebook) -> u32 {
    if cb.is_null() {
        return 0;
    }
    (*cb).inner.index_bits() as u32
}

/// Serialize the codebook to its shareable blob (two-call pattern: pass
/// `out == NULL` to get the size in `*out_len`).
#[no_mangle]
pub unsafe extern "C" fn sc_vq_codebook_to_bytes(
    cb: *const ScVqCodebook,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> ScVqStatus {
    if cb.is_null() || out_len.is_null() {
        return ScVqStatus::InvalidInput;
    }
    let bytes = (*cb).inner.to_bytes();
    *out_len = bytes.len();
    if out.is_null() {
        return ScVqStatus::Ok; // size query
    }
    if cap < bytes.len() {
        return ScVqStatus::BufferTooSmall;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
    ScVqStatus::Ok
}

/// Encode a luma image against `cb` into a transmittable frame blob (two-call
/// pattern). `luma` is `width*height` bytes.
#[no_mangle]
pub unsafe extern "C" fn sc_vq_encode(
    cb: *const ScVqCodebook,
    luma: *const u8,
    len: usize,
    width: u32,
    height: u32,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
) -> ScVqStatus {
    if cb.is_null() || luma.is_null() || out_len.is_null() || width == 0 || height == 0 {
        return ScVqStatus::InvalidInput;
    }
    let need = match (width as usize).checked_mul(height as usize) {
        Some(n) => n,
        None => return ScVqStatus::InvalidInput,
    };
    if len < need {
        return ScVqStatus::InvalidInput;
    }
    let data = std::slice::from_raw_parts(luma, need).to_vec();
    let img = GrayImage { width: width as usize, height: height as usize, data };
    let bytes = encode(&img, &(*cb).inner).to_bytes();
    *out_len = bytes.len();
    if out.is_null() {
        return ScVqStatus::Ok; // size query
    }
    if cap < bytes.len() {
        return ScVqStatus::BufferTooSmall;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
    ScVqStatus::Ok
}

/// Decode a frame blob (from `sc_vq_encode`) against `cb` into luma (two-call
/// pattern). `out_width` / `out_height` receive the decoded dimensions and may
/// be null.
#[no_mangle]
pub unsafe extern "C" fn sc_vq_decode(
    cb: *const ScVqCodebook,
    enc: *const u8,
    enc_len: usize,
    out: *mut u8,
    cap: usize,
    out_len: *mut usize,
    out_width: *mut u32,
    out_height: *mut u32,
) -> ScVqStatus {
    if cb.is_null() || enc.is_null() || out_len.is_null() {
        return ScVqStatus::InvalidInput;
    }
    let bytes = std::slice::from_raw_parts(enc, enc_len);
    let encoded = match Encoded::from_bytes(bytes) {
        Some(e) => e,
        None => return ScVqStatus::ParseError,
    };
    let img = decode(&encoded, &(*cb).inner);
    *out_len = img.data.len();
    if !out_width.is_null() {
        *out_width = img.width as u32;
    }
    if !out_height.is_null() {
        *out_height = img.height as u32;
    }
    if out.is_null() {
        return ScVqStatus::Ok; // size query
    }
    if cap < img.data.len() {
        return ScVqStatus::BufferTooSmall;
    }
    std::ptr::copy_nonoverlapping(img.data.as_ptr(), out, img.data.len());
    ScVqStatus::Ok
}
