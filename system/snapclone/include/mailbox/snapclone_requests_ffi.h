/*
 * snapclone_requests_ffi.h — C ABI for the connection-request approval
 * lifecycle (Rust: snapclone-mailbox::requests::RequestMailbox).
 *
 * The Qt unified-mailbox owns one ScRequestMailbox*, feeds it inbound requests
 * as they arrive, applies the user's Accept/Reject/Block decisions, and reads
 * back the pending rows to render. All state/transition rules live in the
 * tested Rust model — the C++ side is a thin renderer + dispatcher.
 *
 * Link against libsnapclone_mailbox.a (staticlib).
 *
 * Ownership/lifetime:
 *   • sc_reqmb_new() returns a handle you must sc_reqmb_free() exactly once.
 *   • All functions are null-tolerant and panic-safe (a Rust panic is caught
 *     and turned into a benign default return, never unwinds across the ABI).
 *   • For sc_reqmb_pending the CALLER owns the char buffers; the call fills them
 *     NUL-terminated and truncates to the given capacities.
 */
#ifndef SNAPCLONE_REQUESTS_FFI_H
#define SNAPCLONE_REQUESTS_FFI_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ── RequestState (sc_reqmb_state_of return; Changed-outcome payload) ──────── */
#define SC_REQ_STATE_UNKNOWN  (-1)
#define SC_REQ_STATE_PENDING   (0)
#define SC_REQ_STATE_ACCEPTED  (1)
#define SC_REQ_STATE_REJECTED  (2)
#define SC_REQ_STATE_BLOCKED   (3)

/* ── RequestAction (sc_reqmb_apply input) ─────────────────────────────────── */
#define SC_REQ_ACTION_ACCEPT   (0)
#define SC_REQ_ACTION_REJECT   (1)
#define SC_REQ_ACTION_BLOCK    (2)

/* ── ApplyOutcome (sc_reqmb_apply return) ─────────────────────────────────── */
#define SC_REQ_OUTCOME_NOCHANGE     (0)
#define SC_REQ_OUTCOME_REFUSED      (-1)
/* return >= CHANGED_BASE means Changed(state): subtract base for SC_REQ_STATE_* */
#define SC_REQ_OUTCOME_CHANGED_BASE (100)

/* Opaque handle. */
typedef struct ScRequestMailbox ScRequestMailbox;

/* One pending row. The caller sets the buffer pointers + caps before calling
 * sc_reqmb_pending; the call fills them (NUL-terminated, truncated to cap) and
 * sets received_ms. The caller owns the buffers. */
typedef struct ScPendingRow {
    char*   sender_id;          /* caller-owned buffer, filled NUL-terminated   */
    int32_t sender_id_cap;      /* capacity of sender_id in bytes               */
    char*   display_name;       /* caller-owned buffer, filled NUL-terminated   */
    int32_t display_name_cap;   /* capacity of display_name in bytes            */
    int64_t received_ms;        /* out: arrival timestamp (ms)                  */
} ScPendingRow;

/* Lifecycle. */
ScRequestMailbox* sc_reqmb_new(void);
void              sc_reqmb_free(ScRequestMailbox* mb);

/* Record an inbound request. Returns 1 if newly admitted Pending, else 0
 * (blocked sender / merged into existing pending / error). */
int32_t sc_reqmb_receive(ScRequestMailbox* mb,
                         const char* sender_id,
                         const char* display_name,
                         int64_t received_ms);

/* Apply Accept/Reject/Block. Returns SC_REQ_OUTCOME_NOCHANGE / _REFUSED, or
 * SC_REQ_OUTCOME_CHANGED_BASE + new SC_REQ_STATE_* for a real transition. */
int32_t sc_reqmb_apply(ScRequestMailbox* mb,
                       const char* sender_id,
                       int32_t action);

/* Unblock a sender. Returns 1 if they were blocked (now aren't), else 0. */
int32_t sc_reqmb_unblock(ScRequestMailbox* mb, const char* sender_id);

/* State of a sender's request as SC_REQ_STATE_* (UNKNOWN if no entry). */
int32_t sc_reqmb_state_of(const ScRequestMailbox* mb, const char* sender_id);

/* Count of pending requests (for the mailbox badge). */
int32_t sc_reqmb_pending_count(const ScRequestMailbox* mb);

/* Fill `rows` (array of `cap` ScPendingRow with caller-set buffers) with the
 * pending requests newest-first. Returns count written (<= cap). Size the
 * buffer with sc_reqmb_pending_count. */
int32_t sc_reqmb_pending(const ScRequestMailbox* mb,
                        ScPendingRow* rows,
                        int32_t cap);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* SNAPCLONE_REQUESTS_FFI_H */
