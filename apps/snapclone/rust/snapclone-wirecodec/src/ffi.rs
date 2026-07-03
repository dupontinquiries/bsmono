//! C FFI for snapclone-wirecodec.
//!
//! SnapRouter calls `sc_wire_encode` on the media bytes before serialization and
//! `sc_wire_decode` after deserialization. Both use the caller-buffer +
//! out_len + BufferTooSmall-retry convention used elsewhere in the project.

use crate::{decode, encode, Content, DecodeError};
use std::os::raw::c_int;
use std::panic::catch_unwind;

/// Result codes.
pub const SC_WIRE_OK: c_int = 0;
pub const SC_WIRE_NULL: c_int = 1;
pub const SC_WIRE_BUFFER_TOO_SMALL: c_int = 2;
pub const SC_WIRE_DECODE_FAILED: c_int = 3;
pub const SC_WIRE_INTERNAL: c_int = 4;

/// Content classes (mirror Core::MediaType; -1 = Unknown for transport callers).
pub const SC_WIRE_CONTENT_UNKNOWN: c_int = -1;
pub const SC_WIRE_CONTENT_IMAGE: c_int = 0;
pub const SC_WIRE_CONTENT_VIDEO: c_int = 1;
pub const SC_WIRE_CONTENT_TEXT: c_int = 2;
pub const SC_WIRE_CONTENT_LINK: c_int = 3;
pub const SC_WIRE_CONTENT_LOCATION: c_int = 4;

fn content_from_c(c: c_int) -> Content {
    if c < 0 { Content::Unknown } else { Content::from_media_type(c) }
}

/// Encode `data` for the wire (content-aware compression + 1-byte tag). Writes
/// into `out` (capacity `out_cap`); sets `*out_len` to the bytes produced. If
/// `out_cap` is too small, returns SC_WIRE_BUFFER_TOO_SMALL with the required
/// size in `*out_len` (retry with a buffer that big). Worst case is
/// `data_len + 1`.
///
/// # Safety
/// `data` valid for `data_len`; `out` valid for `out_cap`; `out_len` non-null.
#[no_mangle]
pub unsafe extern "C" fn sc_wire_encode(
    content: c_int,
    data: *const u8, data_len: usize,
    out: *mut u8, out_cap: usize, out_len: *mut usize,
) -> c_int {
    let r = catch_unwind(|| {
        if out_len.is_null() || (data.is_null() && data_len != 0) {
            return SC_WIRE_NULL;
        }
        let input = if data_len == 0 { &[][..] }
                    else { std::slice::from_raw_parts(data, data_len) };
        let wire = encode(content_from_c(content), input);
        *out_len = wire.len();
        if wire.len() > out_cap || out.is_null() {
            return SC_WIRE_BUFFER_TOO_SMALL;
        }
        std::slice::from_raw_parts_mut(out, wire.len()).copy_from_slice(&wire);
        SC_WIRE_OK
    });
    r.unwrap_or(SC_WIRE_INTERNAL)
}

/// Decode a wire payload back to the original bytes. Same buffer contract.
/// Note: the decoded size isn't known until inflate runs, so on
/// BufferTooSmall the caller should retry with a generous buffer (e.g. a few ×
/// the wire size, or the known max snap size).
///
/// # Safety
/// `wire` valid for `wire_len`; `out` valid for `out_cap`; `out_len` non-null.
#[no_mangle]
pub unsafe extern "C" fn sc_wire_decode(
    wire: *const u8, wire_len: usize,
    out: *mut u8, out_cap: usize, out_len: *mut usize,
) -> c_int {
    let r = catch_unwind(|| {
        if out_len.is_null() || (wire.is_null() && wire_len != 0) {
            return SC_WIRE_NULL;
        }
        let input = if wire_len == 0 { &[][..] }
                    else { std::slice::from_raw_parts(wire, wire_len) };
        match decode(input) {
            Ok(bytes) => {
                *out_len = bytes.len();
                if bytes.len() > out_cap || out.is_null() {
                    return SC_WIRE_BUFFER_TOO_SMALL;
                }
                std::slice::from_raw_parts_mut(out, bytes.len()).copy_from_slice(&bytes);
                SC_WIRE_OK
            }
            Err(DecodeError::Empty) | Err(DecodeError::UnknownCodec(_))
            | Err(DecodeError::InflateFailed) => SC_WIRE_DECODE_FAILED,
        }
    });
    r.unwrap_or(SC_WIRE_INTERNAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_encode_decode_round_trip() {
        let data = b"Hey everyone! Meeting at 3. ".repeat(8);
        let mut wire = vec![0u8; data.len() + 16];
        let mut wlen = 0usize;
        let e = unsafe {
            sc_wire_encode(SC_WIRE_CONTENT_TEXT, data.as_ptr(), data.len(),
                           wire.as_mut_ptr(), wire.len(), &mut wlen)
        };
        assert_eq!(e, SC_WIRE_OK);
        assert!(wlen < data.len(), "compressible text should shrink");

        let mut back = vec![0u8; data.len() + 16];
        let mut blen = 0usize;
        let d = unsafe {
            sc_wire_decode(wire.as_ptr(), wlen, back.as_mut_ptr(), back.len(), &mut blen)
        };
        assert_eq!(d, SC_WIRE_OK);
        assert_eq!(&back[..blen], &data[..]);
    }

    #[test]
    fn ffi_buffer_too_small_reports_size() {
        let data = b"some payload bytes that are clearly bigger than four".to_vec();
        let mut tiny = [0u8; 4];
        let mut need = 0usize;
        let e = unsafe {
            sc_wire_encode(SC_WIRE_CONTENT_UNKNOWN, data.as_ptr(), data.len(),
                           tiny.as_mut_ptr(), tiny.len(), &mut need)
        };
        assert_eq!(e, SC_WIRE_BUFFER_TOO_SMALL);
        assert!(need >= 1 && need <= data.len() + 1);
    }

    #[test]
    fn ffi_decode_garbage_fails_cleanly() {
        let bad = [0x7Fu8, 1, 2, 3];
        let mut out = [0u8; 64];
        let mut olen = 0usize;
        let d = unsafe {
            sc_wire_decode(bad.as_ptr(), bad.len(), out.as_mut_ptr(), out.len(), &mut olen)
        };
        assert_eq!(d, SC_WIRE_DECODE_FAILED);
    }

    #[test]
    fn ffi_null_safe() {
        let mut olen = 0usize;
        let mut out = [0u8; 4];
        assert_eq!(unsafe {
            sc_wire_encode(SC_WIRE_CONTENT_TEXT, std::ptr::null(), 5,
                           out.as_mut_ptr(), out.len(), &mut olen)
        }, SC_WIRE_NULL);
    }
}
