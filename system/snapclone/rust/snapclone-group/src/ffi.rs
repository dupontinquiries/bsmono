//! ffi.rs — C ABI for the security-critical group operations.
//!
//! Design (mirrors snapclone-crypto's STATELESS FFI): the C++ group session
//! keeps the stateful `GroupState` as opaque serialized bytes it owns; the only
//! things it must call into Rust for are the two operations it cannot safely
//! reimplement — sealing+signing an outgoing group entry and verifying+opening
//! an incoming one. Those are exposed here as stateless functions over byte
//! buffers with an error enum, so nothing allocated by Rust crosses the
//! boundary persistently and there are no handles to leak.
//!
//! Variable-length output uses the standard caller-provides-buffer pattern: pass
//! a buffer + its capacity; the function writes into it and reports the bytes
//! written via an out-param, or returns `BufferTooSmall` with the required size
//! so the caller can retry with a bigger buffer.
//!
//! Every entry point is wrapped in `catch_unwind`: a Rust panic must NEVER
//! unwind across the C ABI (that's UB), so a panic becomes an `Internal` error
//! code instead.

use crate::entry::{self, GroupEntryBody, MessageKind};
use crate::delivery::DeliveryTracker;
use crate::floor::{FloorConfig, FloorControl};
use ed25519_dalek::SigningKey;
use std::os::raw::{c_uchar, c_int};
use std::panic::catch_unwind;
use std::ptr;

/// Result codes for the group FFI. 0 = success.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupFfiError {
    Ok = 0,
    NullPointer = 1,
    BufferTooSmall = 2,
    BadKind = 3,
    /// Signature verify / decrypt / parse failed (verify_and_open path).
    OpenFailed = 4,
    /// Seal/sign failed (encrypt path).
    SealFailed = 5,
    /// A panic was caught crossing the boundary, or another internal error.
    Internal = 6,
    /// Lookup found no such entry (delivery summary for an unknown entry id).
    NotFound = 7,
}

fn kind_from_u8(k: u8) -> Option<MessageKind> {
    match k {
        0 => Some(MessageKind::Text),
        1 => Some(MessageKind::MediaRef),
        2 => Some(MessageKind::Control),
        _ => None,
    }
}

fn kind_to_u8(k: MessageKind) -> u8 {
    k as u8
}

/// Seal + sign a group entry into the wire bytes that go in the DHT feed bucket.
///
/// Inputs:
///   * `epoch`, `index` — the membership epoch and the sender's ratchet index
///     for this message (the caller derives `index` from its SenderChain).
///   * `kind` — 0=Text, 1=MediaRef, 2=Control.
///   * `payload` / `payload_len` — the plaintext message body.
///   * `message_key` — 32 bytes, the per-message key from the sender ratchet.
///   * `sender_id` / `sender_id_len` — UTF-8 sc-ID string.
///   * `signing_key` — 32 bytes, the sender's Ed25519 secret key.
///   * `out` / `out_cap` — caller buffer for the wire bytes.
///   * `out_len` — receives the number of bytes written (or, on
///     `BufferTooSmall`, the required size).
///
/// The RNG is the OS CSPRNG (OsRng) internally; the caller does not supply one.
///
/// # Safety
/// All pointers must be valid for their stated lengths; `out` must be writable
/// for `out_cap` bytes; `message_key` and `signing_key` must each point to 32
/// bytes; `out_len` must be a valid `*mut usize`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_seal_and_sign(
    epoch: u64,
    index: u32,
    kind: u8,
    payload: *const c_uchar,
    payload_len: usize,
    message_key: *const c_uchar,
    sender_id: *const c_uchar,
    sender_id_len: usize,
    signing_key: *const c_uchar,
    out: *mut c_uchar,
    out_cap: usize,
    out_len: *mut usize,
) -> GroupFfiError {
    let r = catch_unwind(|| {
        if message_key.is_null() || sender_id.is_null() || signing_key.is_null()
            || out_len.is_null()
            || (payload.is_null() && payload_len != 0)
        {
            return GroupFfiError::NullPointer;
        }
        let mkind = match kind_from_u8(kind) {
            Some(k) => k,
            None => return GroupFfiError::BadKind,
        };
        let payload_slice = if payload_len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(payload, payload_len)
        };
        let mk = {
            let s = std::slice::from_raw_parts(message_key, 32);
            let mut a = [0u8; 32];
            a.copy_from_slice(s);
            a
        };
        let sk = {
            let s = std::slice::from_raw_parts(signing_key, 32);
            let mut a = [0u8; 32];
            a.copy_from_slice(s);
            SigningKey::from_bytes(&a)
        };
        let sid = {
            let s = std::slice::from_raw_parts(sender_id, sender_id_len);
            match std::str::from_utf8(s) {
                Ok(v) => v.to_owned(),
                Err(_) => return GroupFfiError::SealFailed,
            }
        };
        let body = GroupEntryBody {
            epoch,
            index,
            kind: mkind,
            payload: payload_slice.to_vec(),
        };
        let mut rng = rand_core::OsRng;
        let wire = match entry::seal_and_sign(&body, &mk, &sid, &sk, &mut rng) {
            Ok(w) => w,
            Err(_) => return GroupFfiError::SealFailed,
        };
        // Report required size and bounds-check the caller buffer.
        *out_len = wire.len();
        if wire.len() > out_cap || out.is_null() {
            return GroupFfiError::BufferTooSmall;
        }
        ptr::copy_nonoverlapping(wire.as_ptr(), out, wire.len());
        GroupFfiError::Ok
    });
    r.unwrap_or(GroupFfiError::Internal)
}

/// Verify + open a wire group entry. VERIFY-BEFORE-DECRYPT is enforced inside
/// `entry::verify_and_open`.
///
/// Inputs:
///   * `wire` / `wire_len` — the bytes fetched from the DHT feed bucket.
///   * `message_key` — 32 bytes, the per-message key the recipient derived by
///     ratcheting the sender's chain to the message index.
///
/// Outputs (all optional; pass null to skip):
///   * `out_payload` / `out_payload_cap` / `out_payload_len` — the decrypted
///     payload (same BufferTooSmall retry contract as above).
///   * `out_epoch`, `out_index`, `out_kind` — the body's metadata.
///   * `out_sender_id` / `out_sender_id_cap` / `out_sender_id_len` — the
///     authenticated sender sc-ID.
///   * `out_sender_pub` — 32 bytes, the sender's Ed25519 verifying key (caller
///     confirms it matches `out_sender_id` via the contact-key store / TOFU).
///
/// # Safety
/// Pointers must be valid for their stated lengths; 32-byte buffers where noted.
#[no_mangle]
pub unsafe extern "C" fn sc_group_verify_and_open(
    wire: *const c_uchar,
    wire_len: usize,
    message_key: *const c_uchar,
    out_payload: *mut c_uchar,
    out_payload_cap: usize,
    out_payload_len: *mut usize,
    out_epoch: *mut u64,
    out_index: *mut u32,
    out_kind: *mut u8,
    out_sender_id: *mut c_uchar,
    out_sender_id_cap: usize,
    out_sender_id_len: *mut usize,
    out_sender_pub: *mut c_uchar,
) -> GroupFfiError {
    let r = catch_unwind(|| {
        if wire.is_null() || message_key.is_null() {
            return GroupFfiError::NullPointer;
        }
        let wire_slice = std::slice::from_raw_parts(wire, wire_len);
        let mk = {
            let s = std::slice::from_raw_parts(message_key, 32);
            let mut a = [0u8; 32];
            a.copy_from_slice(s);
            a
        };
        let opened = match entry::verify_and_open(wire_slice, &mk) {
            Ok(o) => o,
            Err(_) => return GroupFfiError::OpenFailed,
        };

        // Scalar metadata.
        if !out_epoch.is_null() {
            *out_epoch = opened.body.epoch;
        }
        if !out_index.is_null() {
            *out_index = opened.body.index;
        }
        if !out_kind.is_null() {
            *out_kind = kind_to_u8(opened.body.kind);
        }
        if !out_sender_pub.is_null() {
            ptr::copy_nonoverlapping(opened.sender_pubkey.as_ptr(), out_sender_pub, 32);
        }

        // Sender id (variable length). Report required size; copy if it fits.
        if !out_sender_id_len.is_null() {
            *out_sender_id_len = opened.sender_id.len();
            if opened.sender_id.len() > out_sender_id_cap || out_sender_id.is_null() {
                return GroupFfiError::BufferTooSmall;
            }
            ptr::copy_nonoverlapping(
                opened.sender_id.as_ptr(),
                out_sender_id,
                opened.sender_id.len(),
            );
        }

        // Payload (variable length). Same contract.
        if !out_payload_len.is_null() {
            *out_payload_len = opened.body.payload.len();
            if opened.body.payload.len() > out_payload_cap || out_payload.is_null() {
                return GroupFfiError::BufferTooSmall;
            }
            ptr::copy_nonoverlapping(
                opened.body.payload.as_ptr(),
                out_payload,
                opened.body.payload.len(),
            );
        }

        GroupFfiError::Ok
    });
    r.unwrap_or(GroupFfiError::Internal)
}

/// Read the cleartext (UNTRUSTED) sender id from a wire entry without verifying
/// or decrypting — lets the recipient pick the right sender chain before having
/// a key. The result must not be trusted until `sc_group_verify_and_open`
/// succeeds. Returns the id length via `out_len` (BufferTooSmall contract);
/// returns `OpenFailed` if the bytes aren't a valid entry frame.
///
/// # Safety
/// `wire` valid for `wire_len`; `out`/`out_len` per the buffer contract.
#[no_mangle]
pub unsafe extern "C" fn sc_group_peek_sender_id(
    wire: *const c_uchar,
    wire_len: usize,
    out: *mut c_uchar,
    out_cap: usize,
    out_len: *mut usize,
) -> GroupFfiError {
    let r = catch_unwind(|| {
        if wire.is_null() || out_len.is_null() {
            return GroupFfiError::NullPointer;
        }
        let wire_slice = std::slice::from_raw_parts(wire, wire_len);
        let id = match entry::peek_sender_id(wire_slice) {
            Some(s) => s,
            None => return GroupFfiError::OpenFailed,
        };
        *out_len = id.len();
        if id.len() > out_cap || out.is_null() {
            return GroupFfiError::BufferTooSmall;
        }
        ptr::copy_nonoverlapping(id.as_ptr(), out, id.len());
        GroupFfiError::Ok
    });
    r.unwrap_or(GroupFfiError::Internal)
}

// ─── group mailbox slot FFI ──────────────────────────────────────────────────
//
// The shared-slot mailbox: serialize/parse the per-sender entry the C++ DHT
// layer PUTs/GETs under H(group_id + "/groupmsg"), plus the rollback mode flag.
// The slot READER (dedup/order/gaps) stays in Rust-tested logic; C++ forwards
// each GET value through sc_mailbox_parse_entry and orders by (sender, seq).

/// Map the rollback flag to a mode code: 0 = PerRecipient (safe rollback),
/// 1 = GroupShared. Mirrors MailboxMode::from_flag.
#[no_mangle]
pub extern "C" fn sc_mailbox_mode_from_flag(shared_enabled: c_int) -> c_int {
    match crate::mailbox::MailboxMode::from_flag(shared_enabled != 0) {
        crate::mailbox::MailboxMode::PerRecipient => 0,
        crate::mailbox::MailboxMode::GroupShared => 1,
    }
}

/// Build the DHT slot key string for a group: writes "group_id/groupmsg" into
/// `out` (NUL-terminated). Returns Ok or BufferTooSmall (with required size,
/// including the NUL, in *out_len).
///
/// # Safety
/// `group_id` points to `group_id_len` bytes; `out`/`out_len` valid.
#[no_mangle]
pub unsafe extern "C" fn sc_mailbox_slot_key(
    group_id: *const c_uchar,
    group_id_len: usize,
    out: *mut c_uchar,
    out_cap: usize,
    out_len: *mut usize,
) -> GroupFfiError {
    let r = catch_unwind(|| {
        if group_id.is_null() || out.is_null() || out_len.is_null() {
            return GroupFfiError::NullPointer;
        }
        let gid = match std::str::from_utf8(std::slice::from_raw_parts(group_id, group_id_len)) {
            Ok(v) => v,
            Err(_) => return GroupFfiError::OpenFailed,
        };
        let key = crate::mailbox::group_slot_key(gid);
        let needed = key.len() + 1; // + NUL
        *out_len = needed;
        if needed > out_cap {
            return GroupFfiError::BufferTooSmall;
        }
        let dst = std::slice::from_raw_parts_mut(out, needed);
        dst[..key.len()].copy_from_slice(key.as_bytes());
        dst[key.len()] = 0;
        GroupFfiError::Ok
    });
    r.unwrap_or(GroupFfiError::Internal)
}

/// Serialize a mailbox entry (the bytes to PUT as one dht_value). `sealed` is
/// the already-encrypted body (from sc_group_seal_and_sign or seal::encrypt).
///
/// # Safety
/// All pointers valid for their lengths; `out`/`out_len` valid.
#[no_mangle]
pub unsafe extern "C" fn sc_mailbox_build_entry(
    sender_id: *const c_uchar, sender_id_len: usize,
    group_id: *const c_uchar, group_id_len: usize,
    seq: u32,
    sealed: *const c_uchar, sealed_len: usize,
    out: *mut c_uchar, out_cap: usize, out_len: *mut usize,
) -> GroupFfiError {
    let r = catch_unwind(|| {
        if sender_id.is_null() || group_id.is_null() || out.is_null() || out_len.is_null()
            || (sealed.is_null() && sealed_len != 0) {
            return GroupFfiError::NullPointer;
        }
        let sid = match std::str::from_utf8(std::slice::from_raw_parts(sender_id, sender_id_len)) {
            Ok(v) => v.to_owned(), Err(_) => return GroupFfiError::OpenFailed,
        };
        let gid = match std::str::from_utf8(std::slice::from_raw_parts(group_id, group_id_len)) {
            Ok(v) => v.to_owned(), Err(_) => return GroupFfiError::OpenFailed,
        };
        let body = if sealed_len == 0 { Vec::new() }
                   else { std::slice::from_raw_parts(sealed, sealed_len).to_vec() };
        let entry = crate::mailbox::MailboxEntry { sender_id: sid, group_id: gid, seq, sealed: body };
        let bytes = match entry.to_bytes() {
            Ok(b) => b, Err(_) => return GroupFfiError::SealFailed,
        };
        *out_len = bytes.len();
        if bytes.len() > out_cap { return GroupFfiError::BufferTooSmall; }
        std::slice::from_raw_parts_mut(out, bytes.len()).copy_from_slice(&bytes);
        GroupFfiError::Ok
    });
    r.unwrap_or(GroupFfiError::Internal)
}

/// Parse a mailbox entry's public header from a raw DHT value. Writes the sender
/// id (NUL-terminated) into `sender_out`, the seq into *seq_out, and the sealed
/// body offset/len into *sealed_off / *sealed_len (the body is sealed[off..off+len]
/// within `raw`). Also validates the group_id matches `expect_group`. Returns
/// OpenFailed for malformed / wrong-group values (so C++ drops them like the
/// existing GET callback does).
///
/// # Safety
/// All pointers valid for their lengths.
#[no_mangle]
pub unsafe extern "C" fn sc_mailbox_parse_entry(
    raw: *const c_uchar, raw_len: usize,
    expect_group: *const c_uchar, expect_group_len: usize,
    sender_out: *mut c_uchar, sender_cap: usize,
    seq_out: *mut u32,
    sealed_off: *mut usize, sealed_len: *mut usize,
) -> GroupFfiError {
    let r = catch_unwind(|| {
        if raw.is_null() || expect_group.is_null() || sender_out.is_null()
            || seq_out.is_null() || sealed_off.is_null() || sealed_len.is_null() {
            return GroupFfiError::NullPointer;
        }
        let raw_slice = std::slice::from_raw_parts(raw, raw_len);
        let entry = match crate::mailbox::MailboxEntry::from_bytes(raw_slice) {
            Ok(e) => e, Err(_) => return GroupFfiError::OpenFailed,
        };
        let expect = match std::str::from_utf8(
            std::slice::from_raw_parts(expect_group, expect_group_len)) {
            Ok(v) => v, Err(_) => return GroupFfiError::OpenFailed,
        };
        if entry.group_id != expect {
            return GroupFfiError::OpenFailed; // wrong group → caller drops
        }
        let sbytes = entry.sender_id.as_bytes();
        if sbytes.len() + 1 > sender_cap { return GroupFfiError::BufferTooSmall; }
        let sdst = std::slice::from_raw_parts_mut(sender_out, sbytes.len() + 1);
        sdst[..sbytes.len()].copy_from_slice(sbytes);
        sdst[sbytes.len()] = 0;
        *seq_out = entry.seq;
        // sealed body is the tail: raw_len - sealed.len() .. raw_len
        *sealed_len = entry.sealed.len();
        *sealed_off = raw_len - entry.sealed.len();
        GroupFfiError::Ok
    });
    r.unwrap_or(GroupFfiError::Internal)
}

// ─── DeliveryTracker FFI (stateful, opaque handle) ───────────────────────────
//
// Group read-receipts are STATEFUL — the per-conversation tracker accumulates
// as ACKs / open-receipts arrive — so unlike the stateless seal/open functions
// above this is an opaque-handle API. The C++ inbox owns one DeliveryTracker
// per group, feeds it record_* events from the transport, and reads back the
// "k of N" summary for the delivered/seen badge. Strings are passed ptr+len
// (UTF-8), matching the byte-buffer convention used above (no NUL assumed).
// sc-IDs never contain '\n', so record_sent takes the recipient set as one
// '\n'-separated buffer. Every entry point is catch_unwind-guarded like the rest.

unsafe fn str_from<'a>(ptr: *const c_uchar, len: usize) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    std::str::from_utf8(std::slice::from_raw_parts(ptr, len)).ok()
}

/// Create an empty delivery tracker (one per group conversation). Never null.
/// Free with `sc_group_delivery_free`.
#[no_mangle]
pub extern "C" fn sc_group_delivery_new() -> *mut DeliveryTracker {
    Box::into_raw(Box::new(DeliveryTracker::new()))
}

/// Free a tracker from `sc_group_delivery_new`.
///
/// # Safety
/// `h` must be a pointer from `sc_group_delivery_new` that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn sc_group_delivery_free(h: *mut DeliveryTracker) {
    if !h.is_null() {
        drop(Box::from_raw(h));
    }
}

/// Record that entry `entry_id` was sent to the '\n'-separated `recipients`
/// (the sender `self_id` is excluded automatically). Idempotent.
///
/// # Safety
/// All pointers must be valid for their stated lengths; `h` from
/// `sc_group_delivery_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_delivery_record_sent(
    h: *mut DeliveryTracker,
    entry_id: *const c_uchar,
    entry_id_len: usize,
    recipients: *const c_uchar,
    recipients_len: usize,
    self_id: *const c_uchar,
    self_id_len: usize,
) -> GroupFfiError {
    catch_unwind(|| {
        if h.is_null() {
            return GroupFfiError::NullPointer;
        }
        let t = &mut *h;
        let entry = match str_from(entry_id, entry_id_len) {
            Some(s) => s.to_string(),
            None => return GroupFfiError::OpenFailed,
        };
        let selfid = match str_from(self_id, self_id_len) {
            Some(s) => s,
            None => return GroupFfiError::OpenFailed,
        };
        let joined = match str_from(recipients, recipients_len) {
            Some(s) => s,
            None => return GroupFfiError::OpenFailed,
        };
        let recips: Vec<String> = joined
            .split('\n')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        t.record_sent(&entry, recips, selfid);
        GroupFfiError::Ok
    })
    .unwrap_or(GroupFfiError::Internal)
}

/// Record that `member` received entry `entry_id` (a transport ACK). Monotonic:
/// never downgrades a member already marked Seen.
///
/// # Safety
/// Pointers valid for their lengths; `h` from `sc_group_delivery_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_delivery_record_delivered(
    h: *mut DeliveryTracker,
    entry_id: *const c_uchar,
    entry_id_len: usize,
    member: *const c_uchar,
    member_len: usize,
) -> GroupFfiError {
    catch_unwind(|| {
        if h.is_null() {
            return GroupFfiError::NullPointer;
        }
        let t = &mut *h;
        let entry = match str_from(entry_id, entry_id_len) {
            Some(s) => s.to_string(),
            None => return GroupFfiError::OpenFailed,
        };
        let m = match str_from(member, member_len) {
            Some(s) => s,
            None => return GroupFfiError::OpenFailed,
        };
        t.record_delivered(&entry, m);
        GroupFfiError::Ok
    })
    .unwrap_or(GroupFfiError::Internal)
}

/// Record that `member` opened entry `entry_id` (an open-receipt).
///
/// # Safety
/// Pointers valid for their lengths; `h` from `sc_group_delivery_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_delivery_record_seen(
    h: *mut DeliveryTracker,
    entry_id: *const c_uchar,
    entry_id_len: usize,
    member: *const c_uchar,
    member_len: usize,
) -> GroupFfiError {
    catch_unwind(|| {
        if h.is_null() {
            return GroupFfiError::NullPointer;
        }
        let t = &mut *h;
        let entry = match str_from(entry_id, entry_id_len) {
            Some(s) => s.to_string(),
            None => return GroupFfiError::OpenFailed,
        };
        let m = match str_from(member, member_len) {
            Some(s) => s,
            None => return GroupFfiError::OpenFailed,
        };
        t.record_seen(&entry, m);
        GroupFfiError::Ok
    })
    .unwrap_or(GroupFfiError::Internal)
}

/// Read the "k of N" summary for entry `entry_id`. Writes recipient / delivered
/// / seen counts to the out-params. Returns `NotFound` if the entry was never
/// recorded as sent (out-params left untouched).
///
/// # Safety
/// `entry_id` valid for its length; the three out pointers must be writable;
/// `h` from `sc_group_delivery_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_delivery_summary(
    h: *const DeliveryTracker,
    entry_id: *const c_uchar,
    entry_id_len: usize,
    out_recipients: *mut c_int,
    out_delivered: *mut c_int,
    out_seen: *mut c_int,
) -> GroupFfiError {
    catch_unwind(|| {
        if h.is_null()
            || out_recipients.is_null()
            || out_delivered.is_null()
            || out_seen.is_null()
        {
            return GroupFfiError::NullPointer;
        }
        let t = &*h;
        let entry = match str_from(entry_id, entry_id_len) {
            Some(s) => s.to_string(),
            None => return GroupFfiError::OpenFailed,
        };
        match t.summary(&entry) {
            Some(s) => {
                *out_recipients = s.recipients as c_int;
                *out_delivered = s.delivered as c_int;
                *out_seen = s.seen as c_int;
                GroupFfiError::Ok
            }
            None => GroupFfiError::NotFound,
        }
    })
    .unwrap_or(GroupFfiError::Internal)
}

/// Drop all state for one entry (e.g. the snap expired). Idempotent.
///
/// # Safety
/// `entry_id` valid for its length; `h` from `sc_group_delivery_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_delivery_forget(
    h: *mut DeliveryTracker,
    entry_id: *const c_uchar,
    entry_id_len: usize,
) -> GroupFfiError {
    catch_unwind(|| {
        if h.is_null() {
            return GroupFfiError::NullPointer;
        }
        let t = &mut *h;
        let entry = match str_from(entry_id, entry_id_len) {
            Some(s) => s.to_string(),
            None => return GroupFfiError::OpenFailed,
        };
        t.forget(&entry);
        GroupFfiError::Ok
    })
    .unwrap_or(GroupFfiError::Internal)
}

// ─── FloorControl FFI (stateful, opaque handle) ──────────────────────────────
//
// Half-duplex "talking stick" for a group voice channel. Like the delivery
// tracker this is stateful, so the C++ group-radio session owns one
// FloorControl per channel, feeds it local press/release and the floor
// announcements that arrive over the transport, and reads back whether it may
// transmit. Together with the snapclone-audio TransmitGate (when to send the
// mic) this is the full gate for group push-to-talk.
//
// Convention for the boolean ops below: the return is a `c_int` where a
// NEGATIVE value means an error (null handle / bad UTF-8) and 0/1 is the actual
// answer. The C++ side treats `> 0` as "yes". The void ops are null-safe no-ops.

/// Create a floor controller for member `me`. `hold_timeout_ms` is the
/// keepalive window before an idle holder is dropped; `max_hold_ms` is the
/// fairness cap on a single turn. Returns null if `me` is not valid UTF-8.
/// Free with `sc_group_floor_free`.
///
/// # Safety
/// `me` must be valid for `me_len` bytes.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_new(
    me: *const c_uchar,
    me_len: usize,
    hold_timeout_ms: u64,
    max_hold_ms: u64,
) -> *mut FloorControl {
    match str_from(me, me_len) {
        Some(s) => Box::into_raw(Box::new(FloorControl::new(
            s.to_string(),
            FloorConfig { hold_timeout_ms, max_hold_ms },
        ))),
        None => ptr::null_mut(),
    }
}

/// Free a controller from `sc_group_floor_new`.
///
/// # Safety
/// `h` must be a pointer from `sc_group_floor_new` not already freed.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_free(h: *mut FloorControl) {
    if !h.is_null() {
        drop(Box::from_raw(h));
    }
}

/// We press talk (call repeatedly while holding — it doubles as keepalive).
/// `claim_ts` is our claim timestamp for tiebreaking a simultaneous grab.
/// Returns 1 if we hold the floor and may transmit, 0 if denied (someone else
/// holds it), -1 on a null handle.
///
/// # Safety
/// `h` from `sc_group_floor_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_request(
    h: *mut FloorControl,
    claim_ts: u64,
    now_ms: u64,
) -> c_int {
    if h.is_null() {
        return -1;
    }
    if (*h).request(claim_ts, now_ms) {
        1
    } else {
        0
    }
}

/// We stop talking (releases the floor if we held it). Null-safe.
///
/// # Safety
/// `h` from `sc_group_floor_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_release(h: *mut FloorControl, now_ms: u64) {
    if !h.is_null() {
        (*h).release(now_ms);
    }
}

/// A remote member announced it is taking/keeping the floor. Applies the
/// deterministic contention tiebreak. Returns 1 if WE hold the floor afterward,
/// 0 if not, -1 on a null handle or bad UTF-8 in `holder`.
///
/// # Safety
/// `holder` valid for `holder_len`; `h` from `sc_group_floor_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_remote_assert(
    h: *mut FloorControl,
    holder: *const c_uchar,
    holder_len: usize,
    claim_ts: u64,
    now_ms: u64,
) -> c_int {
    catch_unwind(|| {
        if h.is_null() {
            return -1;
        }
        let holder = match str_from(holder, holder_len) {
            Some(s) => s.to_string(),
            None => return -1,
        };
        if (*h).remote_assert(holder, claim_ts, now_ms) {
            1
        } else {
            0
        }
    })
    .unwrap_or(-1)
}

/// A remote member announced it released the floor. Null-safe; ignores a holder
/// that isn't the one we currently believe holds it.
///
/// # Safety
/// `holder` valid for `holder_len`; `h` from `sc_group_floor_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_remote_release(
    h: *mut FloorControl,
    holder: *const c_uchar,
    holder_len: usize,
) {
    let _ = catch_unwind(|| {
        if h.is_null() {
            return;
        }
        if let Some(s) = str_from(holder, holder_len) {
            (*h).remote_release(&s.to_string());
        }
    });
}

/// Advance the local clock — reopens the floor on holder timeout or the
/// fairness cap. Call on a steady timer (e.g. every few hundred ms). Null-safe.
///
/// # Safety
/// `h` from `sc_group_floor_new`, or null.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_tick(h: *mut FloorControl, now_ms: u64) {
    if !h.is_null() {
        (*h).tick(now_ms);
    }
}

/// Do WE hold the floor right now? 1 = yes, 0 = no, -1 = null handle.
///
/// # Safety
/// `h` from `sc_group_floor_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_holds(h: *const FloorControl) -> c_int {
    if h.is_null() {
        return -1;
    }
    if (*h).holds_floor() {
        1
    } else {
        0
    }
}

/// May we transmit this instant (we both want and hold the floor)? 1/0, -1 null.
///
/// # Safety
/// `h` from `sc_group_floor_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_may_transmit(h: *const FloorControl) -> c_int {
    if h.is_null() {
        return -1;
    }
    if (*h).may_transmit() {
        1
    } else {
        0
    }
}

/// Write the current holder's sc-id into `out` (for a "Bob is talking" label).
/// Sets `out_len` to the id length. Returns `NotFound` when the floor is open,
/// `BufferTooSmall` (with the required size in `out_len`) if it doesn't fit.
///
/// # Safety
/// `out` writable for `out_cap` bytes; `out_len` a valid `*mut usize`; `h` from
/// `sc_group_floor_new`.
#[no_mangle]
pub unsafe extern "C" fn sc_group_floor_current_holder(
    h: *const FloorControl,
    out: *mut c_uchar,
    out_cap: usize,
    out_len: *mut usize,
) -> GroupFfiError {
    catch_unwind(|| {
        if h.is_null() || out_len.is_null() {
            return GroupFfiError::NullPointer;
        }
        match (*h).current_holder() {
            Some(holder) => {
                let bytes = holder.as_bytes();
                *out_len = bytes.len();
                if out.is_null() || bytes.len() > out_cap {
                    return GroupFfiError::BufferTooSmall;
                }
                ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len());
                GroupFfiError::Ok
            }
            None => GroupFfiError::NotFound, // floor is open
        }
    })
    .unwrap_or(GroupFfiError::Internal)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::ratchet::SenderChain;

    // Exercise the FFI exactly as C++ would: raw pointers, caller buffers, the
    // BufferTooSmall retry, and the full seal→open round-trip through the C ABI.

    #[test]
    fn ffi_mailbox_mode_flag() {
        assert_eq!(sc_mailbox_mode_from_flag(0), 0); // PerRecipient (rollback)
        assert_eq!(sc_mailbox_mode_from_flag(1), 1); // GroupShared
    }

    #[test]
    fn ffi_mailbox_slot_key() {
        let gid = b"grp-xyz";
        let mut out = [0u8; 64];
        let mut len = 0usize;
        let e = unsafe {
            sc_mailbox_slot_key(gid.as_ptr(), gid.len(), out.as_mut_ptr(), out.len(), &mut len)
        };
        assert_eq!(e, GroupFfiError::Ok);
        assert_eq!(&out[..len - 1], b"grp-xyz/groupmsg"); // len includes NUL
        assert_eq!(out[len - 1], 0);
    }

    #[test]
    fn ffi_mailbox_build_then_parse() {
        let sid = b"sc-alice";
        let gid = b"grp-1";
        let sealed = [1u8, 2, 3, b'\n', 250, 0, 9]; // sealed may hold newlines/NULs
        let mut wire = [0u8; 256];
        let mut wlen = 0usize;
        let e = unsafe {
            sc_mailbox_build_entry(sid.as_ptr(), sid.len(), gid.as_ptr(), gid.len(), 42,
                sealed.as_ptr(), sealed.len(), wire.as_mut_ptr(), wire.len(), &mut wlen)
        };
        assert_eq!(e, GroupFfiError::Ok);

        // Parse it back the way the C++ GET callback would.
        let mut sender_out = [0u8; 64];
        let mut seq = 0u32;
        let mut soff = 0usize;
        let mut slen = 0usize;
        let pe = unsafe {
            sc_mailbox_parse_entry(wire.as_ptr(), wlen, gid.as_ptr(), gid.len(),
                sender_out.as_mut_ptr(), sender_out.len(), &mut seq, &mut soff, &mut slen)
        };
        assert_eq!(pe, GroupFfiError::Ok);
        assert_eq!(seq, 42);
        // sender id NUL-terminated
        let send_str = std::ffi::CStr::from_bytes_until_nul(&sender_out).unwrap();
        assert_eq!(send_str.to_str().unwrap(), "sc-alice");
        // sealed body recovered exactly from the reported offset/len
        assert_eq!(&wire[soff..soff + slen], &sealed);
    }

    #[test]
    fn ffi_mailbox_parse_rejects_wrong_group() {
        let sid = b"sc-a"; let gid = b"grp-1"; let sealed = [7u8];
        let mut wire = [0u8; 128]; let mut wlen = 0usize;
        unsafe {
            sc_mailbox_build_entry(sid.as_ptr(), sid.len(), gid.as_ptr(), gid.len(), 0,
                sealed.as_ptr(), sealed.len(), wire.as_mut_ptr(), wire.len(), &mut wlen);
        }
        let other = b"grp-OTHER";
        let mut so = [0u8; 32]; let mut sq = 0u32; let mut off = 0usize; let mut ln = 0usize;
        let pe = unsafe {
            sc_mailbox_parse_entry(wire.as_ptr(), wlen, other.as_ptr(), other.len(),
                so.as_mut_ptr(), so.len(), &mut sq, &mut off, &mut ln)
        };
        assert_eq!(pe, GroupFfiError::OpenFailed); // wrong group → caller drops
    }


    #[test]
    fn ffi_seal_then_open_round_trip() {
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let mut chain = SenderChain::from_root([9u8; 32]);
        let (idx, mk) = chain.next_message_key();
        let payload = b"ffi hello";
        let sid = b"sc-ffitest0";

        // First call with a too-small buffer to learn the size.
        let mut wire_len: usize = 0;
        let mut tiny = [0u8; 4];
        let e = unsafe {
            sc_group_seal_and_sign(
                3, idx, MessageKind::Text as u8,
                payload.as_ptr(), payload.len(),
                mk.as_ptr(),
                sid.as_ptr(), sid.len(),
                signing.to_bytes().as_ptr(),
                tiny.as_mut_ptr(), tiny.len(), &mut wire_len,
            )
        };
        assert_eq!(e, GroupFfiError::BufferTooSmall);
        assert!(wire_len > 4, "should report the real required size");

        // Retry with a correctly-sized buffer.
        let mut wire = vec![0u8; wire_len];
        let mut written: usize = 0;
        let e = unsafe {
            sc_group_seal_and_sign(
                3, idx, MessageKind::Text as u8,
                payload.as_ptr(), payload.len(),
                mk.as_ptr(),
                sid.as_ptr(), sid.len(),
                signing.to_bytes().as_ptr(),
                wire.as_mut_ptr(), wire.len(), &mut written,
            )
        };
        assert_eq!(e, GroupFfiError::Ok);
        assert_eq!(written, wire_len);

        // Open it back through the FFI (receiver ratchets to the same index).
        let mut recv_chain = SenderChain::from_root([9u8; 32]);
        let recv_mk = recv_chain.message_key_for(0).unwrap();
        let _ = idx; // index 0 here

        let mut payload_out = vec![0u8; 64];
        let mut payload_len: usize = 0;
        let mut epoch: u64 = 0;
        let mut index: u32 = 0;
        let mut kind: u8 = 255;
        let mut sid_out = vec![0u8; 64];
        let mut sid_len: usize = 0;
        let mut pub_out = [0u8; 32];
        let e = unsafe {
            sc_group_verify_and_open(
                wire.as_ptr(), wire.len(),
                recv_mk.as_ptr(),
                payload_out.as_mut_ptr(), payload_out.len(), &mut payload_len,
                &mut epoch, &mut index, &mut kind,
                sid_out.as_mut_ptr(), sid_out.len(), &mut sid_len,
                pub_out.as_mut_ptr(),
            )
        };
        assert_eq!(e, GroupFfiError::Ok);
        assert_eq!(epoch, 3);
        assert_eq!(kind, MessageKind::Text as u8);
        assert_eq!(&payload_out[..payload_len], payload);
        assert_eq!(&sid_out[..sid_len], sid);
        assert_eq!(pub_out, signing.verifying_key().to_bytes());
    }

    #[test]
    fn ffi_open_rejects_wrong_key() {
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let sid = b"sc-1";
        let payload = b"x";
        let mut wire = vec![0u8; 256];
        let mut written: usize = 0;
        let mk = [1u8; 32];
        let e = unsafe {
            sc_group_seal_and_sign(
                1, 0, MessageKind::Text as u8,
                payload.as_ptr(), payload.len(),
                mk.as_ptr(),
                sid.as_ptr(), sid.len(),
                signing.to_bytes().as_ptr(),
                wire.as_mut_ptr(), wire.len(), &mut written,
            )
        };
        assert_eq!(e, GroupFfiError::Ok);
        wire.truncate(written);

        // Wrong message key → open fails (decrypt step).
        let wrong = [2u8; 32];
        let mut pl = vec![0u8; 64];
        let mut pln: usize = 0;
        let e = unsafe {
            sc_group_verify_and_open(
                wire.as_ptr(), wire.len(), wrong.as_ptr(),
                pl.as_mut_ptr(), pl.len(), &mut pln,
                ptr::null_mut(), ptr::null_mut(), ptr::null_mut(),
                ptr::null_mut(), 0, ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        assert_eq!(e, GroupFfiError::OpenFailed);
    }

    #[test]
    fn ffi_null_pointers_rejected() {
        let mut len: usize = 0;
        let e = unsafe {
            sc_group_seal_and_sign(
                0, 0, 0,
                ptr::null(), 0,
                ptr::null(), // null message key
                ptr::null(), 0,
                ptr::null(),
                ptr::null_mut(), 0, &mut len,
            )
        };
        assert_eq!(e, GroupFfiError::NullPointer);
    }

    #[test]
    fn ffi_bad_kind_rejected() {
        let mk = [1u8; 32];
        let sid = b"sc-1";
        let sk = SigningKey::from_bytes(&[5u8; 32]);
        let mut len: usize = 0;
        let mut buf = vec![0u8; 256];
        let e = unsafe {
            sc_group_seal_and_sign(
                0, 0, 99, // invalid kind
                ptr::null(), 0,
                mk.as_ptr(),
                sid.as_ptr(), sid.len(),
                sk.to_bytes().as_ptr(),
                buf.as_mut_ptr(), buf.len(), &mut len,
            )
        };
        assert_eq!(e, GroupFfiError::BadKind);
    }

    #[test]
    fn ffi_peek_sender_id() {
        let signing = SigningKey::from_bytes(&[5u8; 32]);
        let sid = b"sc-peek123";
        let mut wire = vec![0u8; 256];
        let mut written: usize = 0;
        let mk = [3u8; 32];
        unsafe {
            sc_group_seal_and_sign(
                0, 0, MessageKind::Text as u8,
                b"p".as_ptr(), 1,
                mk.as_ptr(),
                sid.as_ptr(), sid.len(),
                signing.to_bytes().as_ptr(),
                wire.as_mut_ptr(), wire.len(), &mut written,
            );
        }
        wire.truncate(written);
        let mut out = vec![0u8; 64];
        let mut out_len: usize = 0;
        let e = unsafe {
            sc_group_peek_sender_id(wire.as_ptr(), wire.len(), out.as_mut_ptr(), out.len(), &mut out_len)
        };
        assert_eq!(e, GroupFfiError::Ok);
        assert_eq!(&out[..out_len], sid);
    }

    #[test]
    fn delivery_ffi_roundtrip() {
        unsafe {
            let h = sc_group_delivery_new();
            assert!(!h.is_null());
            let entry = b"entry-1";
            let recips = b"sc-aaaa\nsc-bbbb"; // two recipients, '\n'-separated
            let me = b"sc-self";
            assert_eq!(
                sc_group_delivery_record_sent(
                    h, entry.as_ptr(), entry.len(),
                    recips.as_ptr(), recips.len(),
                    me.as_ptr(), me.len()
                ),
                GroupFfiError::Ok
            );
            let (mut r, mut d, mut s) = (0i32, 0i32, 0i32);
            assert_eq!(
                sc_group_delivery_summary(h, entry.as_ptr(), entry.len(), &mut r, &mut d, &mut s),
                GroupFfiError::Ok
            );
            assert_eq!((r, d, s), (2, 0, 0));

            // one recipient delivered
            let m1 = b"sc-aaaa";
            assert_eq!(
                sc_group_delivery_record_delivered(h, entry.as_ptr(), entry.len(), m1.as_ptr(), m1.len()),
                GroupFfiError::Ok
            );
            assert_eq!(
                sc_group_delivery_summary(h, entry.as_ptr(), entry.len(), &mut r, &mut d, &mut s),
                GroupFfiError::Ok
            );
            assert_eq!((r, d, s), (2, 1, 0));

            // same recipient opens it → Seen implies Delivered
            assert_eq!(
                sc_group_delivery_record_seen(h, entry.as_ptr(), entry.len(), m1.as_ptr(), m1.len()),
                GroupFfiError::Ok
            );
            assert_eq!(
                sc_group_delivery_summary(h, entry.as_ptr(), entry.len(), &mut r, &mut d, &mut s),
                GroupFfiError::Ok
            );
            assert_eq!((r, d, s), (2, 1, 1));

            // unknown entry → NotFound, out-params untouched
            let unk = b"no-such-entry";
            assert_eq!(
                sc_group_delivery_summary(h, unk.as_ptr(), unk.len(), &mut r, &mut d, &mut s),
                GroupFfiError::NotFound
            );

            // null handle is safe
            assert_eq!(
                sc_group_delivery_record_delivered(
                    std::ptr::null_mut(), entry.as_ptr(), entry.len(), m1.as_ptr(), m1.len()
                ),
                GroupFfiError::NullPointer
            );
            // forget drops it
            assert_eq!(
                sc_group_delivery_forget(h, entry.as_ptr(), entry.len()),
                GroupFfiError::Ok
            );
            assert_eq!(
                sc_group_delivery_summary(h, entry.as_ptr(), entry.len(), &mut r, &mut d, &mut s),
                GroupFfiError::NotFound
            );
            sc_group_delivery_free(h);
        }
    }

    #[test]
    fn floor_ffi_roundtrip() {
        unsafe {
            let me = b"alice";
            let h = sc_group_floor_new(me.as_ptr(), me.len(), 1000, 30_000);
            assert!(!h.is_null());

            // grab the open floor
            assert_eq!(sc_group_floor_request(h, 100, 100), 1);
            assert_eq!(sc_group_floor_holds(h), 1);
            assert_eq!(sc_group_floor_may_transmit(h), 1);

            let mut buf = [0u8; 64];
            let mut n: usize = 0;
            assert_eq!(
                sc_group_floor_current_holder(h, buf.as_mut_ptr(), buf.len(), &mut n),
                GroupFfiError::Ok
            );
            assert_eq!(&buf[..n], me);

            // release → open
            sc_group_floor_release(h, 200);
            assert_eq!(sc_group_floor_holds(h), 0);
            assert_eq!(
                sc_group_floor_current_holder(h, buf.as_mut_ptr(), buf.len(), &mut n),
                GroupFfiError::NotFound
            );

            // bob takes it remotely → we don't hold; holder reads "bob"
            let bob = b"bob";
            assert_eq!(sc_group_floor_remote_assert(h, bob.as_ptr(), bob.len(), 300, 300), 0);
            assert_eq!(sc_group_floor_holds(h), 0);
            assert_eq!(
                sc_group_floor_current_holder(h, buf.as_mut_ptr(), buf.len(), &mut n),
                GroupFfiError::Ok
            );
            assert_eq!(&buf[..n], bob);

            // we request while bob holds → denied
            assert_eq!(sc_group_floor_request(h, 400, 400), 0);

            // bob goes silent past hold_timeout → tick reopens the floor
            sc_group_floor_tick(h, 1400);
            assert_eq!(
                sc_group_floor_current_holder(h, buf.as_mut_ptr(), buf.len(), &mut n),
                GroupFfiError::NotFound
            );

            // bad UTF-8 me → null handle
            let bad = [0xffu8, 0xfe];
            assert!(sc_group_floor_new(bad.as_ptr(), bad.len(), 1000, 30_000).is_null());

            // null-handle ops are safe (negative / no crash)
            assert_eq!(sc_group_floor_holds(std::ptr::null()), -1);
            assert_eq!(sc_group_floor_request(std::ptr::null_mut(), 0, 0), -1);
            sc_group_floor_release(std::ptr::null_mut(), 0);
            sc_group_floor_tick(std::ptr::null_mut(), 0);

            sc_group_floor_free(h);
        }
    }
}
