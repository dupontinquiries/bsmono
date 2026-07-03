#ifndef SNAPCLONE_GROUP_FFI_H
#define SNAPCLONE_GROUP_FFI_H

/*
 * snapclone_group_ffi.h — C ABI for the Rust snapclone-group crate.
 *
 * Usage: #include "group/snapclone_group_ffi.h" inside an extern "C" {} in C++.
 * Link against libsnapclone_group.a (built with crate-type = ["staticlib"]).
 *
 * Design: STATELESS, mirroring snapclone_crypto_ffi.h. The C++ group session
 * owns the stateful GroupState as opaque serialized bytes; it only calls into
 * Rust for the two operations it cannot safely reimplement — sealing+signing an
 * outgoing group entry and verifying+opening an incoming one (verify happens
 * BEFORE decrypt inside the Rust impl). Nothing allocated by Rust persists
 * across the boundary; variable-length outputs use the caller-provides-buffer
 * pattern: pass a buffer + capacity, the function writes into it and reports the
 * bytes written via an out-param, or returns BufferTooSmall with the required
 * size so the caller can retry. Every entry point catches Rust panics and
 * reports them as Internal rather than unwinding across the ABI.
 *
 * Available when CMake is built with -DSNAPCLONE_RUST_GROUP=ON.
 */

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Result codes; 0 = success. Matches the Rust GroupFfiError repr(C) enum. */
typedef enum GroupFfiError {
    GROUP_FFI_OK               = 0,
    GROUP_FFI_NULL_POINTER     = 1,
    GROUP_FFI_BUFFER_TOO_SMALL = 2,
    GROUP_FFI_BAD_KIND         = 3,
    GROUP_FFI_OPEN_FAILED      = 4,  /* verify/decrypt/parse failed */
    GROUP_FFI_SEAL_FAILED      = 5,  /* seal/sign failed */
    GROUP_FFI_INTERNAL         = 6,  /* caught panic / internal error */
    GROUP_FFI_NOT_FOUND        = 7   /* lookup found no such entry (delivery summary) */
} GroupFfiError;

/* Message kinds (matches Rust MessageKind). */
#define SC_GROUP_KIND_TEXT      0
#define SC_GROUP_KIND_MEDIA_REF 1
#define SC_GROUP_KIND_CONTROL   2

/*
 * Seal + sign a group entry into the wire bytes for the DHT feed bucket.
 *
 *   epoch, index   membership epoch + sender ratchet index for this message
 *   kind           SC_GROUP_KIND_*
 *   payload        plaintext body (may be NULL iff payload_len == 0)
 *   message_key    32 bytes, per-message key from the sender ratchet
 *   sender_id      UTF-8 sc-ID string
 *   signing_key    32 bytes, sender's Ed25519 secret key
 *   out            caller buffer for the wire bytes
 *   out_cap        capacity of `out`
 *   out_len        receives bytes written, or required size on BufferTooSmall
 *
 * Uses the OS CSPRNG internally; the caller does not supply an RNG. To size the
 * buffer, call once with out_cap = 0 (out may be NULL): returns BufferTooSmall
 * with the required size in *out_len, then retry.
 */
GroupFfiError sc_group_seal_and_sign(
    uint64_t       epoch,
    uint32_t       index,
    uint8_t        kind,
    const uint8_t *payload,
    size_t         payload_len,
    const uint8_t *message_key,   /* 32 bytes */
    const uint8_t *sender_id,
    size_t         sender_id_len,
    const uint8_t *signing_key,   /* 32 bytes */
    uint8_t       *out,
    size_t         out_cap,
    size_t        *out_len);

/*
 * Verify + open a wire group entry (verify-before-decrypt enforced internally).
 *
 *   wire / wire_len   bytes fetched from the DHT feed bucket
 *   message_key       32 bytes, per-message key derived by ratcheting to index
 *
 * All outputs are optional — pass NULL to skip any of them:
 *   out_payload / cap / len   decrypted payload (BufferTooSmall retry contract)
 *   out_epoch, out_index      body metadata
 *   out_kind                  SC_GROUP_KIND_*
 *   out_sender_id / cap / len  authenticated sender sc-ID
 *   out_sender_pub            32 bytes, sender Ed25519 verifying key (confirm it
 *                             matches out_sender_id via your contact-key store)
 */
GroupFfiError sc_group_verify_and_open(
    const uint8_t *wire,
    size_t         wire_len,
    const uint8_t *message_key,   /* 32 bytes */
    uint8_t       *out_payload,
    size_t         out_payload_cap,
    size_t        *out_payload_len,
    uint64_t      *out_epoch,
    uint32_t      *out_index,
    uint8_t       *out_kind,
    uint8_t       *out_sender_id,
    size_t         out_sender_id_cap,
    size_t        *out_sender_id_len,
    uint8_t       *out_sender_pub /* 32 bytes */);

/*
 * Read the cleartext (UNTRUSTED) sender id without verifying or decrypting, so
 * a recipient can pick the right sender chain before having a key. Do NOT trust
 * the result until sc_group_verify_and_open succeeds. BufferTooSmall retry
 * contract; OPEN_FAILED if the bytes aren't a valid entry frame.
 */
GroupFfiError sc_group_peek_sender_id(
    const uint8_t *wire,
    size_t         wire_len,
    uint8_t       *out,
    size_t         out_cap,
    size_t        *out_len);

/* ── Group mailbox slot ("share the slot, not the key") ─────────────────────
 *
 * A single shared DHT key, H(group_id + "/groupmsg"), holds one per-sender
 * sealed entry per member. The DHT stores distinct values side-by-side, so N
 * senders coexist (same multi-writer mechanism the connreq slot uses). This
 * batches group traffic into ONE lookup. Each entry is sealed per-sender (no
 * shared group key) so forward secrecy + revocation survive.
 *
 * ROLLBACK: gated by config [network] groupMailboxShared (default OFF →
 * PerRecipient, the shipping behavior). sc_mailbox_mode_from_flag maps it.
 */

/* 0 = PerRecipient (rollback target), 1 = GroupShared. */
int sc_mailbox_mode_from_flag(int shared_enabled);

/* Build the slot key string "group_id/groupmsg" (NUL-terminated) into out. */
GroupFfiError sc_mailbox_slot_key(
    const uint8_t *group_id, size_t group_id_len,
    uint8_t *out, size_t out_cap, size_t *out_len);

/* Serialize one mailbox entry (the bytes to PUT as a dht_value). `sealed` is the
 * already-encrypted body. BufferTooSmall retry contract. */
GroupFfiError sc_mailbox_build_entry(
    const uint8_t *sender_id, size_t sender_id_len,
    const uint8_t *group_id,  size_t group_id_len,
    uint32_t       seq,
    const uint8_t *sealed,    size_t sealed_len,
    uint8_t *out, size_t out_cap, size_t *out_len);

/* Parse a raw DHT value: validates group == expect_group, writes sender id
 * (NUL-terminated) + seq, and reports the sealed body as a [offset,len) slice
 * within `raw` (no copy). OPEN_FAILED for malformed / wrong-group values, so the
 * caller drops them exactly like the existing GET callback. */
GroupFfiError sc_mailbox_parse_entry(
    const uint8_t *raw, size_t raw_len,
    const uint8_t *expect_group, size_t expect_group_len,
    uint8_t *sender_out, size_t sender_cap,
    uint32_t *seq_out,
    size_t *sealed_off, size_t *sealed_len);

/* ─── DeliveryTracker: stateful per-group read-receipts ─────────────────────
 *
 * STATEFUL, unlike the seal/open functions above. The C++ inbox owns one
 * tracker per group conversation (opaque handle), feeds it record_* events from
 * the transport (ACK -> delivered, open-receipt -> seen), and reads back the
 * "k of N" summary for the delivered/seen badge. Strings are UTF-8 ptr+len (no
 * NUL assumed); record_sent takes the recipient set as one '\n'-separated buffer
 * (sc-IDs never contain newlines). Free with sc_group_delivery_free. */
typedef struct ScGroupDeliveryTracker ScGroupDeliveryTracker;

ScGroupDeliveryTracker *sc_group_delivery_new(void);
void sc_group_delivery_free(ScGroupDeliveryTracker *h);

/* Record an entry was sent to '\n'-separated recipients (self_id excluded). */
GroupFfiError sc_group_delivery_record_sent(
    ScGroupDeliveryTracker *h,
    const uint8_t *entry_id, size_t entry_id_len,
    const uint8_t *recipients, size_t recipients_len,
    const uint8_t *self_id, size_t self_id_len);

/* member received the entry (monotonic; never downgrades a Seen member). */
GroupFfiError sc_group_delivery_record_delivered(
    ScGroupDeliveryTracker *h,
    const uint8_t *entry_id, size_t entry_id_len,
    const uint8_t *member, size_t member_len);

/* member opened the entry. */
GroupFfiError sc_group_delivery_record_seen(
    ScGroupDeliveryTracker *h,
    const uint8_t *entry_id, size_t entry_id_len,
    const uint8_t *member, size_t member_len);

/* "k of N" counts into the out-params. NOT_FOUND if the entry is unknown. */
GroupFfiError sc_group_delivery_summary(
    const ScGroupDeliveryTracker *h,
    const uint8_t *entry_id, size_t entry_id_len,
    int *out_recipients, int *out_delivered, int *out_seen);

/* Drop all state for one entry (e.g. the snap expired). Idempotent. */
GroupFfiError sc_group_delivery_forget(
    ScGroupDeliveryTracker *h,
    const uint8_t *entry_id, size_t entry_id_len);

/* ─── FloorControl: half-duplex "talking stick" for group voice ─────────────
 *
 * STATEFUL. The C++ group-radio session owns one per channel (opaque handle),
 * feeds it local press/release and the floor announcements arriving over the
 * transport, and reads back whether it may transmit. Pair with the audio
 * TransmitGate (snapclone_audio_ffi.h) for the full group push-to-talk gate.
 *
 * Boolean ops return int: NEGATIVE = error (null handle / bad UTF-8), else 0/1
 * is the answer (treat > 0 as "yes"). The void ops are null-safe no-ops.
 * `claim_ts` is a comparable timestamp used only to break a simultaneous-grab
 * tie (earlier wins, ties to the smaller id); liveness uses `now_ms` (local). */
typedef struct ScGroupFloorControl ScGroupFloorControl;

/* NULL if `me` is not valid UTF-8. */
ScGroupFloorControl *sc_group_floor_new(
    const uint8_t *me, size_t me_len,
    uint64_t hold_timeout_ms, uint64_t max_hold_ms);
void sc_group_floor_free(ScGroupFloorControl *h);

/* press/keepalive: 1 = we hold & may transmit, 0 = denied, -1 = null. */
int  sc_group_floor_request(ScGroupFloorControl *h, uint64_t claim_ts, uint64_t now_ms);
/* stop talking (null-safe). */
void sc_group_floor_release(ScGroupFloorControl *h, uint64_t now_ms);
/* a peer is taking/keeping the floor: 1 = we hold after, 0 = not, -1 = null/bad. */
int  sc_group_floor_remote_assert(
    ScGroupFloorControl *h,
    const uint8_t *holder, size_t holder_len,
    uint64_t claim_ts, uint64_t now_ms);
/* a peer released (null-safe; ignored unless it's the believed holder). */
void sc_group_floor_remote_release(
    ScGroupFloorControl *h, const uint8_t *holder, size_t holder_len);
/* advance the local clock: reopens on holder timeout / fairness cap (null-safe). */
void sc_group_floor_tick(ScGroupFloorControl *h, uint64_t now_ms);
/* do we hold it? 1/0, -1 = null. */
int  sc_group_floor_holds(const ScGroupFloorControl *h);
/* may we transmit now (want AND hold)? 1/0, -1 = null. */
int  sc_group_floor_may_transmit(const ScGroupFloorControl *h);
/* current holder's sc-id into out; NOT_FOUND when open, BUFFER_TOO_SMALL if it
 * doesn't fit (required size in out_len). */
GroupFfiError sc_group_floor_current_holder(
    const ScGroupFloorControl *h,
    uint8_t *out, size_t out_cap, size_t *out_len);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_GROUP_FFI_H */
