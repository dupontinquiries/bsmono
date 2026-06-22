//! Tunable constants. Centralized here so callers don't have to dig
//! through the chain or storage modules to find the defaults.
//!
//! Why these specific values:
//!
//! - **MAX_HOP_DEPTH = 5**: friend-graph diameter studies consistently
//!   land at 4-6 for arbitrary-pair reachability. 5 is the middle of
//!   that range. Below 3 the offline-gap-fill benefit is marginal;
//!   above 7 the chain verification cost grows fast (linear in hops,
//!   each verification is an ed25519 sig check).
//!
//! - **DEFAULT_STORAGE_BUDGET_BYTES = 100MB**: chosen so a moderately-
//!   active relay holds ~50-200 snaps in transit at typical 0.5-2MB
//!   sizes. Users on storage-constrained devices can lower this; users
//!   running relay nodes can raise it.
//!
//! - **DEFAULT_BLOB_TTL_HOURS = 168 (1 week)**: matches the maximum
//!   snap TTL in the existing wire format. After a week, the original
//!   sender has either rotated keys or moved on, so undelivered snaps
//!   should be dropped to free space.

/// Maximum chain length (number of hops). See module docs for rationale.
///
/// Const rather than configurable because the limit also affects the
/// recipient's verification cost — making this dynamic per-peer would
/// open a DoS vector where a misbehaving sender claims 50 hops.
pub const MAX_HOP_DEPTH: usize = 5;

/// Default storage budget in bytes (100 MB). Implementations should
/// allow this to be overridden by the user via a settings slider but
/// default to this value out-of-the-box.
pub const DEFAULT_STORAGE_BUDGET_BYTES: usize = 100 * 1024 * 1024;

/// Default time-to-live for piggyback blobs (in hours).
///
/// Past this point, the blob is dropped even if it hasn't been
/// forwarded — the recipient probably isn't coming back, and the
/// sender's keys may have rotated.
pub const DEFAULT_BLOB_TTL_HOURS: u32 = 168;  // one week

/// Maximum bytes-per-second a relay may use forwarding piggyback
/// blobs. Soft cap — implementations should backpressure rather than
/// drop, but if forwarding queue grows beyond `MAX_FORWARD_BACKLOG`
/// the relay should refuse new accept-for-relay requests.
///
/// 50 KB/s ≈ ~5MB/min, fine for a background activity on a phone.
/// Tunable per-peer in a future revision.
pub const DEFAULT_FORWARD_BANDWIDTH_BPS: usize = 50 * 1024;

/// Hard cap on pending forward queue size. Beyond this, the relay
/// refuses additional accept-for-relay requests until the backlog
/// drains.
pub const MAX_FORWARD_BACKLOG: usize = 100;
