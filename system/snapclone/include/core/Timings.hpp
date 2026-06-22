#pragma once
// ─── SnapClone timing constants ───────────────────────────────────────────────
//
// All timing values in one place so energy audits and tuning are easy.
// Earth-day note: every timer that fires wakes the CPU and radio. We use
// the longest intervals that still give good UX.
//
// V32.33 audit pass: walked every interval against the goal "use less
// background battery without making the app feel slower." Findings:
//
//   - kMailboxCheckMs (90s): already conservative. The 30s startup
//     checks the user sees in the log are kReconnectFastMs and the
//     t+30s second-check, not this periodic. No change.
//   - kGossipTickMs (3s): already at the comfortable lower bound.
//     Frequent gossip JOIN log messages come from kGossipFailureMs
//     interacting with the backoff system, not from this tick. The
//     tick itself fires the heartbeat. No change.
//   - kDhtReannounceMs (55 min): already very slow. No change.
//   - kPunchMaintenanceMs: bumped 20s → 22s. Per-peer keepalive while
//     a connection is up. 10% slower waking the radio in steady-state
//     while staying well clear of the aggressive consumer-router NAT
//     timeout floor (~30s). Real battery win for users with several
//     active peers. Safe because most NATs are 60-120s timeout.
//
// The bigger known steady-state cost is `kPunchMaintenanceMs` firing
// per peer. If you have N connected peers, you wake the radio every
// 20-22s × N times per minute. That's the leverage point. The fix
// isn't a global shorter/longer here — it's smarter per-peer
// timing based on observed NAT behavior. Future work; not in scope
// for this audit.

namespace SnapClone::Timings {

// ── Network polling ───────────────────────────────────────────────────────────
inline constexpr int kNetworkPollMs         = 10'000; // SSID change detection
                                                       // 10s: detects IP change within 1 heartbeat
                                                       // (was 3s — too aggressive, wasted CPU on iface scan)
inline constexpr int kMailboxCheckMs        = 90'000; // DHT mailbox poll
inline constexpr int kDhtCachePurgeMs       = 60 * 60'000; // purge stale DHT cache

// ── STUN ──────────────────────────────────────────────────────────────────────
inline constexpr int kStunTimeoutMs         = 4'000;  // per-server UDP timeout
inline constexpr int kStunRetryBaseMs       = 30'000; // initial retry interval
inline constexpr int kStunRetryMaxMs        = 5 * 60'000; // cap at 5 min
inline constexpr int kStunRetryMaxAttempts  = 20;     // then stop until network change

// ── NAT traversal ─────────────────────────────────────────────────────────────
inline constexpr int kPunchBurstIntervalMs  = 150;    // initial burst packet interval
inline constexpr int kPunchBurstCount       = 60;     // 60 × 150ms = 9s burst window
inline constexpr int kPunchMaintenanceMs    = 22'000; // keep NAT hole open (CGNAT ~30-120s)
                                                       // V32.33: 20s → 22s. Per-peer keepalive
                                                       // is the main steady-state radio cost
                                                       // when multiple peers are connected.
                                                       // 22s stays well clear of the 30s
                                                       // aggressive NAT timeout floor.
inline constexpr int kCgnatKeepaliveMs      = 25'000; // UDP keepalive to STUN server
                                                       // Only active while peers are connecting (see AppController)

// ── Connection management ──────────────────────────────────────────────────────
inline constexpr int kReconnectFastMs       = 8'000;  // first reconnect pass
inline constexpr int kReconnectSlowMs       = 20'000; // second reconnect pass
inline constexpr int kMailboxDecryptRetryMs = 3'000;  // retry if key not ready yet
inline constexpr int kConnectKickDebounceMs = 4'000;  // min gap between send-triggered connect attempts per peer
inline constexpr int kSprayCooldownMs       = 20'000; // wait before spray-punch
inline constexpr int kDiscoverySuppressMs   = 120'000;// suppress after symmetric NAT
inline constexpr int kDhtLookupTimeoutMs    = 60'000; // per DHT lookup attempt
inline constexpr int kRelayConnectMs        = 30'000; // relay TCP connect timeout

// ── Gossip (LAN discovery) ─────────────────────────────────────────────────────
inline constexpr int kGossipTickMs          = 3'000;  // send heartbeat
inline constexpr int kGossipPruneMs         = 8'000;  // check stale peers
inline constexpr int kGossipFailureMs       = 15'000; // declare peer lost
inline constexpr int kGossipBackoffTicks    = 5;      // empty ticks before back-off
inline constexpr int kGossipBackoffDivisor  = 10;     // fire 1-in-N ticks when idle

// ── DHT / announce ────────────────────────────────────────────────────────────
inline constexpr int kDhtReannounceMs       = 55 * 60'000; // re-announce (DHT TTL ~1h)
inline constexpr int kDhtAnnounceCoalesceMs = 5'000;  // batch multiple addr updates

// ── UI ────────────────────────────────────────────────────────────────────────
inline constexpr int kStatRefreshMs         = 3'000;  // netstat label fade-in
inline constexpr int kBeaconPruneMs         = 5'000;  // prune stale nearby peers
inline constexpr int kSpinnerFrameMs        = 125;    // 8fps braille spinner
inline constexpr int kStatusFadeSuccessMs   = 3'000;  // connected status auto-hide
inline constexpr int kStatusFadeFailMs      = 8'000;  // failed status auto-hide

} // namespace SnapClone::Timings
