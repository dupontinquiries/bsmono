#pragma once
// ─── core/TieredDeliveryPolicy.hpp ───────────────────────────────────────────
//
// Tiered media delivery: a single snap can be transmitted as multiple
// progressively-higher-quality versions, where the first version arrives
// fast and gets shown immediately, and later versions replace the preview
// once they finish transferring.
//
// The user-visible model:
//   1. Sender presses Send. Local encoder produces a Thumbnail (tiny,
//      arrives almost instantly), a Preview (low-res but watchable), and
//      a Standard or HighQuality (the "real" snap).
//   2. Each version is transmitted as a separate frame on the wire,
//      tagged with the snap ID and a tier marker.
//   3. Receiver's UI displays the highest tier it has so far. When a
//      higher tier arrives, it transparently upgrades.
//
// Why this is good UX:
//   • Latency to "something on screen" goes from ~15s (full video on
//     mobile cellular) to ~200ms (thumbnail).
//   • Recipient gets a sense of what was sent immediately and can
//      decide whether to wait for the full version or move on.
//   • Recovers gracefully on flaky networks — if the network dies after
//      the thumbnail, the recipient still has SOMETHING rather than
//      nothing.
//   • On fast LAN, the thumbnail and full version arrive within a frame
//      or two of each other; the upgrade is imperceptible.
//
// Why TIERED DELIVERY is a per-snap CHOICE, not always-on:
//   • Two devices on the same Gigabit LAN: any tiered scheme is pure
//     overhead. The full snap arrives in ~80ms; thumbnails just clog
//     the pipe.
//   • Tiny snaps (5-second 480p clip ~200 KB): the thumbnail isn't
//     meaningfully smaller than the full version. Skip it.
//   • Cellular peer where we know throughput is ~50 KB/s: thumbnail
//     + preview + full is essential or the user waits 10+ minutes.
//
// So: this module CHOOSES which tiers to send for a given snap, based on
// (a) the snap's size + duration, (b) the peer's NetworkStats profile
// (success rate per address class, observed latency), and (c) the
// DebugFlag::NoFsmTuning toggle.
//
// When NoFsmTuning is ON: bypass policy, send Standard tier only. This
// is the A/B baseline.
//
// ─────────────────────────────────────────────────────────────────────────────

#include "core/EncodeTier.hpp"
#include "core/Types.hpp"
#include <QList>
#include <QString>

namespace SnapClone::Core::TieredDelivery {

using SnapClone::Core::Media::EncodeTier;
using SnapClone::Core::Media::tierName;

/// Inputs the policy needs to decide. The caller fills this in from
/// the snap's metadata + the recipient's network profile.
struct Context {
    qint64  inputSizeBytes  = 0;   ///< raw source size (.mkv from recorder)
    int     durationMs      = 0;   ///< clip duration
    int     sourceWidth     = 0;   ///< source resolution width
    int     sourceHeight    = 0;   ///< source resolution height
    bool    isVideo         = true;///< false for image-only snaps
    // Per-peer signals (optional — pass zeros if not yet known):
    double  peerSuccessRate = -1;  ///< 0..1 from NetworkStats; -1 = unknown
    qint64  peerMedianMs    = -1;  ///< connect latency from NetworkStats
    QString peerClassName;          ///< e.g. "rfc1918", "cgnat", "public"
};

/// Plan: ordered list of tiers to encode and send for this snap.
/// First tier arrives first; receiver shows it; later tiers replace.
/// Empty list = "send the source as-is, no transcode" (very rare —
/// only for already-tiny or no-encoder-available scenarios).
using Plan = QList<EncodeTier>;

/// Decide a plan. Pure function — no I/O, no Qt signals, no global
/// state. Easy to unit-test by varying Context.
///
/// Heuristics (start simple; tune over time):
///   • Source < 200 KB:                  [Standard]  (single tier, too small to slice)
///   • Source < 1 MB and same-LAN peer:  [Standard]  (LAN is fast enough)
///   • Cellular peer or unknown:         [Thumbnail, Preview, Standard]
///   • Source > 5 MB or HQ-flagged:      [Thumbnail, Preview, Standard, HighQuality]
///
/// Returns Plan{ EncodeTier::Standard } as a safe default if Context is
/// missing or DebugFlag::NoFsmTuning is set elsewhere (caller checks).
[[nodiscard]] Plan choose(const Context &ctx);

/// Human-readable summary of a plan ("thumbnail → preview → standard").
[[nodiscard]] QString planToString(const Plan &plan);

/// One-tier shortcut when the caller knows it wants no tiering at all
/// (debug flag, settings override, simple-path test).
[[nodiscard]] inline Plan standardOnly() {
    return Plan{ EncodeTier::Standard };
}

} // namespace SnapClone::Core::TieredDelivery
