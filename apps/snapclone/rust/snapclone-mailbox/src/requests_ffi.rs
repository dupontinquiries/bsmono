//! C FFI for the connection-request approval lifecycle (`requests::RequestMailbox`).
//!
//! The Qt unified-mailbox owns an opaque `ScRequestMailbox*`, feeds it inbound
//! requests as they arrive (`sc_reqmb_receive`), and applies the user's
//! Accept/Reject/Block decisions (`sc_reqmb_apply`). It reads back the pending
//! rows to render (`sc_reqmb_pending`) and the per-sender state to drive the
//! snapclone-menu actions (`sc_reqmb_state_of`). All state/transition rules live
//! in the tested Rust model; the C++ side is a thin renderer + dispatcher.
//!
//! Conventions mirror snapclone-menu's FFI: opaque handle, panic-safe via
//! catch_unwind, null-tolerant, caller-owned output buffers (no allocation
//! crosses the boundary for arrays — the caller passes a buffer and we fill it).
//! Sender ids are passed as NUL-terminated UTF-8 C strings.

use crate::requests::{ApplyOutcome, RequestAction, RequestMailbox, RequestState};
use std::ffi::{c_char, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};

// ── C-compatible enums (kept in sync with the header) ────────────────────────

/// RequestState as an int. -1 = unknown/no entry (state_of miss).
pub const SC_REQ_STATE_UNKNOWN: i32 = -1;
pub const SC_REQ_STATE_PENDING: i32 = 0;
pub const SC_REQ_STATE_ACCEPTED: i32 = 1;
pub const SC_REQ_STATE_REJECTED: i32 = 2;
pub const SC_REQ_STATE_BLOCKED: i32 = 3;

/// RequestAction as an int (input to sc_reqmb_apply).
pub const SC_REQ_ACTION_ACCEPT: i32 = 0;
pub const SC_REQ_ACTION_REJECT: i32 = 1;
pub const SC_REQ_ACTION_BLOCK: i32 = 2;

/// ApplyOutcome as an int (return of sc_reqmb_apply).
pub const SC_REQ_OUTCOME_NOCHANGE: i32 = 0;
pub const SC_REQ_OUTCOME_REFUSED: i32 = -1;
/// Any value >= SC_REQ_OUTCOME_CHANGED_BASE encodes Changed(state): subtract the
/// base to get the SC_REQ_STATE_* the entry changed to. Lets one return value
/// carry both "changed" and "to what" without an out-param.
pub const SC_REQ_OUTCOME_CHANGED_BASE: i32 = 100;

fn state_to_c(s: RequestState) -> i32 {
    match s {
        RequestState::Pending => SC_REQ_STATE_PENDING,
        RequestState::Accepted => SC_REQ_STATE_ACCEPTED,
        RequestState::Rejected => SC_REQ_STATE_REJECTED,
        RequestState::Blocked => SC_REQ_STATE_BLOCKED,
    }
}

fn action_from_c(a: i32) -> Option<RequestAction> {
    match a {
        SC_REQ_ACTION_ACCEPT => Some(RequestAction::Accept),
        SC_REQ_ACTION_REJECT => Some(RequestAction::Reject),
        SC_REQ_ACTION_BLOCK => Some(RequestAction::Block),
        _ => None,
    }
}

/// Safely read a C string into a Rust &str; returns None on null/bad-UTF8.
unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

// ── Handle lifecycle ─────────────────────────────────────────────────────────

/// Create a new request mailbox. Returns an opaque handle, or null on failure.
#[no_mangle]
pub extern "C" fn sc_reqmb_new() -> *mut RequestMailbox {
    let r = catch_unwind(|| Box::into_raw(Box::new(RequestMailbox::new())));
    r.unwrap_or(std::ptr::null_mut())
}

/// Free a request mailbox. Null-tolerant.
///
/// # Safety
/// `mb` must have come from `sc_reqmb_new` and not be used after this.
#[no_mangle]
pub unsafe extern "C" fn sc_reqmb_free(mb: *mut RequestMailbox) {
    if mb.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        drop(unsafe { Box::from_raw(mb) });
    }));
}

// ── Operations ───────────────────────────────────────────────────────────────

/// Record an inbound request. Returns 1 if newly admitted as Pending, 0 if
/// dropped (blocked sender) / merged into an existing pending row / on error.
///
/// # Safety
/// `mb` valid; `sender_id`/`display_name` NUL-terminated UTF-8 or null.
#[no_mangle]
pub unsafe extern "C" fn sc_reqmb_receive(
    mb: *mut RequestMailbox,
    sender_id: *const c_char,
    display_name: *const c_char,
    received_ms: i64,
) -> i32 {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mb = match unsafe { mb.as_mut() } {
            Some(m) => m,
            None => return 0,
        };
        let sid = match unsafe { cstr(sender_id) } {
            Some(s) => s,
            None => return 0,
        };
        let name = unsafe { cstr(display_name) }.unwrap_or("");
        i32::from(mb.receive(sid, name, received_ms))
    }));
    r.unwrap_or(0)
}

/// Apply Accept/Reject/Block. Returns an encoded ApplyOutcome:
///   SC_REQ_OUTCOME_NOCHANGE / SC_REQ_OUTCOME_REFUSED, or
///   SC_REQ_OUTCOME_CHANGED_BASE + new-state for Changed.
///
/// # Safety
/// `mb` valid; `sender_id` NUL-terminated UTF-8 or null.
#[no_mangle]
pub unsafe extern "C" fn sc_reqmb_apply(
    mb: *mut RequestMailbox,
    sender_id: *const c_char,
    action: i32,
) -> i32 {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mb = match unsafe { mb.as_mut() } {
            Some(m) => m,
            None => return SC_REQ_OUTCOME_NOCHANGE,
        };
        let sid = match unsafe { cstr(sender_id) } {
            Some(s) => s,
            None => return SC_REQ_OUTCOME_NOCHANGE,
        };
        let act = match action_from_c(action) {
            Some(a) => a,
            None => return SC_REQ_OUTCOME_NOCHANGE,
        };
        match mb.apply(sid, act) {
            ApplyOutcome::NoChange => SC_REQ_OUTCOME_NOCHANGE,
            ApplyOutcome::Refused => SC_REQ_OUTCOME_REFUSED,
            ApplyOutcome::Changed(s) => SC_REQ_OUTCOME_CHANGED_BASE + state_to_c(s),
        }
    }));
    r.unwrap_or(SC_REQ_OUTCOME_NOCHANGE)
}

/// Unblock a sender. Returns 1 if they were blocked (and now aren't), else 0.
///
/// # Safety
/// `mb` valid; `sender_id` NUL-terminated UTF-8 or null.
#[no_mangle]
pub unsafe extern "C" fn sc_reqmb_unblock(
    mb: *mut RequestMailbox,
    sender_id: *const c_char,
) -> i32 {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mb = match unsafe { mb.as_mut() } {
            Some(m) => m,
            None => return 0,
        };
        match unsafe { cstr(sender_id) } {
            Some(s) => i32::from(mb.unblock(s)),
            None => 0,
        }
    }));
    r.unwrap_or(0)
}

/// State of a sender's request, as SC_REQ_STATE_* (or SC_REQ_STATE_UNKNOWN if
/// no entry). The snapclone-menu side reads this to choose actions.
///
/// # Safety
/// `mb` valid; `sender_id` NUL-terminated UTF-8 or null.
#[no_mangle]
pub unsafe extern "C" fn sc_reqmb_state_of(
    mb: *const RequestMailbox,
    sender_id: *const c_char,
) -> i32 {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mb = match unsafe { mb.as_ref() } {
            Some(m) => m,
            None => return SC_REQ_STATE_UNKNOWN,
        };
        match unsafe { cstr(sender_id) } {
            Some(s) => mb.state_of(s).map(state_to_c).unwrap_or(SC_REQ_STATE_UNKNOWN),
            None => SC_REQ_STATE_UNKNOWN,
        }
    }));
    r.unwrap_or(SC_REQ_STATE_UNKNOWN)
}

/// Number of pending requests (for the mailbox badge).
///
/// # Safety
/// `mb` valid.
#[no_mangle]
pub unsafe extern "C" fn sc_reqmb_pending_count(mb: *const RequestMailbox) -> i32 {
    let r = catch_unwind(AssertUnwindSafe(|| {
        unsafe { mb.as_ref() }.map_or(0, |m| m.pending_count() as i32)
    }));
    r.unwrap_or(0)
}

/// One pending row handed back to C. `sender_id`/`display_name` point into
/// caller-provided char buffers that this call fills (NUL-terminated, truncated
/// to the given capacities). The caller owns the buffers.
#[repr(C)]
pub struct ScPendingRow {
    /// Caller-provided buffer for the sender id; filled NUL-terminated.
    pub sender_id: *mut c_char,
    pub sender_id_cap: i32,
    /// Caller-provided buffer for the display name; filled NUL-terminated.
    pub display_name: *mut c_char,
    pub display_name_cap: i32,
    /// Out: arrival timestamp (ms).
    pub received_ms: i64,
}

fn fill_cbuf(dst: *mut c_char, cap: i32, src: &str) {
    if dst.is_null() || cap <= 0 {
        return;
    }
    let cap = cap as usize;
    let bytes = src.as_bytes();
    // Reserve one byte for the NUL terminator.
    let n = bytes.len().min(cap - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, dst, n);
        *dst.add(n) = 0;
    }
}

/// Fill `rows` (an array of `cap` ScPendingRow whose buffer pointers/caps the
/// caller has set) with the pending requests, newest-first. Returns the number
/// written (<= cap). The total pending count may exceed cap; use
/// sc_reqmb_pending_count to size the buffer.
///
/// # Safety
/// `mb` valid; `rows` points to `cap` ScPendingRow with valid buffer pointers.
#[no_mangle]
pub unsafe extern "C" fn sc_reqmb_pending(
    mb: *const RequestMailbox,
    rows: *mut ScPendingRow,
    cap: i32,
) -> i32 {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let mb = match unsafe { mb.as_ref() } {
            Some(m) => m,
            None => return 0,
        };
        if rows.is_null() || cap <= 0 {
            return 0;
        }
        let pending = mb.pending();
        let n = (pending.len() as i32).min(cap);
        for i in 0..n as usize {
            let entry = pending[i];
            let row = unsafe { &mut *rows.add(i) };
            fill_cbuf(row.sender_id, row.sender_id_cap, &entry.sender_id);
            fill_cbuf(row.display_name, row.display_name_cap, &entry.display_name);
            row.received_ms = entry.received_ms;
        }
        n
    }));
    r.unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn cs(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    #[test]
    fn ffi_lifecycle_roundtrip() {
        unsafe {
            let mb = sc_reqmb_new();
            assert!(!mb.is_null());

            let alice = cs("sc-alice");
            let aname = cs("Alice");
            // Receive → newly pending.
            assert_eq!(sc_reqmb_receive(mb, alice.as_ptr(), aname.as_ptr(), 100), 1);
            assert_eq!(sc_reqmb_pending_count(mb), 1);
            assert_eq!(sc_reqmb_state_of(mb, alice.as_ptr()), SC_REQ_STATE_PENDING);

            // Accept → Changed(Accepted).
            let out = sc_reqmb_apply(mb, alice.as_ptr(), SC_REQ_ACTION_ACCEPT);
            assert_eq!(out, SC_REQ_OUTCOME_CHANGED_BASE + SC_REQ_STATE_ACCEPTED);
            assert_eq!(sc_reqmb_pending_count(mb), 0);

            sc_reqmb_free(mb);
        }
    }

    #[test]
    fn ffi_block_then_accept_refused() {
        unsafe {
            let mb = sc_reqmb_new();
            let bad = cs("sc-bad");
            let bname = cs("Spammer");
            sc_reqmb_receive(mb, bad.as_ptr(), bname.as_ptr(), 100);
            let blk = sc_reqmb_apply(mb, bad.as_ptr(), SC_REQ_ACTION_BLOCK);
            assert_eq!(blk, SC_REQ_OUTCOME_CHANGED_BASE + SC_REQ_STATE_BLOCKED);
            // Future receive dropped.
            assert_eq!(sc_reqmb_receive(mb, bad.as_ptr(), bname.as_ptr(), 200), 0);
            // Accept refused.
            assert_eq!(sc_reqmb_apply(mb, bad.as_ptr(), SC_REQ_ACTION_ACCEPT),
                       SC_REQ_OUTCOME_REFUSED);
            // Unblock works.
            assert_eq!(sc_reqmb_unblock(mb, bad.as_ptr()), 1);
            sc_reqmb_free(mb);
        }
    }

    #[test]
    fn ffi_pending_rows_filled() {
        unsafe {
            let mb = sc_reqmb_new();
            let a = cs("sc-a");
            let b = cs("sc-b");
            sc_reqmb_receive(mb, a.as_ptr(), cs("A").as_ptr(), 100);
            sc_reqmb_receive(mb, b.as_ptr(), cs("B").as_ptr(), 200);

            // Two caller-owned buffers per row.
            let mut id0 = vec![0 as c_char; 32];
            let mut nm0 = vec![0 as c_char; 32];
            let mut id1 = vec![0 as c_char; 32];
            let mut nm1 = vec![0 as c_char; 32];
            let mut rows = [
                ScPendingRow {
                    sender_id: id0.as_mut_ptr(), sender_id_cap: 32,
                    display_name: nm0.as_mut_ptr(), display_name_cap: 32,
                    received_ms: 0,
                },
                ScPendingRow {
                    sender_id: id1.as_mut_ptr(), sender_id_cap: 32,
                    display_name: nm1.as_mut_ptr(), display_name_cap: 32,
                    received_ms: 0,
                },
            ];
            let n = sc_reqmb_pending(mb, rows.as_mut_ptr(), 2);
            assert_eq!(n, 2);
            // Newest-first: sc-b (200) before sc-a (100).
            assert_eq!(CStr::from_ptr(rows[0].sender_id).to_str().unwrap(), "sc-b");
            assert_eq!(rows[0].received_ms, 200);
            assert_eq!(CStr::from_ptr(rows[1].sender_id).to_str().unwrap(), "sc-a");
            sc_reqmb_free(mb);
        }
    }

    #[test]
    fn ffi_null_tolerant() {
        unsafe {
            assert_eq!(sc_reqmb_receive(std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), 0), 0);
            assert_eq!(sc_reqmb_state_of(std::ptr::null(), std::ptr::null()), SC_REQ_STATE_UNKNOWN);
            assert_eq!(sc_reqmb_pending_count(std::ptr::null()), 0);
            assert_eq!(sc_reqmb_pending(std::ptr::null(), std::ptr::null_mut(), 0), 0);
            sc_reqmb_free(std::ptr::null_mut()); // must not crash
        }
    }

    #[test]
    fn ffi_buffer_truncation_safe() {
        unsafe {
            let mb = sc_reqmb_new();
            let long = cs("sc-a-very-long-sender-identifier-exceeding-buffer");
            sc_reqmb_receive(mb, long.as_ptr(), cs("Name").as_ptr(), 100);
            let mut id = vec![0 as c_char; 8]; // tiny buffer
            let mut nm = vec![0 as c_char; 8];
            let mut rows = [ScPendingRow {
                sender_id: id.as_mut_ptr(), sender_id_cap: 8,
                display_name: nm.as_mut_ptr(), display_name_cap: 8,
                received_ms: 0,
            }];
            let n = sc_reqmb_pending(mb, rows.as_mut_ptr(), 1);
            assert_eq!(n, 1);
            // Truncated but NUL-terminated (7 chars + NUL), no overflow.
            let s = CStr::from_ptr(rows[0].sender_id).to_str().unwrap();
            assert_eq!(s.len(), 7);
            assert!(long.to_str().unwrap().starts_with(s));
            sc_reqmb_free(mb);
        }
    }
}
