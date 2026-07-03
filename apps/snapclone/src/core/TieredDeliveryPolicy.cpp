#include "core/TieredDeliveryPolicy.hpp"
#include "debug/DebugFlags.hpp"
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcTiered, "snapclone.tiered")

namespace SnapClone::Core::TieredDelivery {

Plan choose(const Context &ctx)
{
    // Debug toggle: NoFsmTuning bypasses ALL policy and goes back to
    // sending Standard only. This is the A/B baseline — useful for
    // measuring whether tiering actually helps.
    if (Debug::isDebug(Debug::DebugFlag::NoFsmTuning)) {
        qCDebug(lcTiered) << "NoFsmTuning ON — returning Standard-only plan";
        return standardOnly();
    }

    // Image snaps don't benefit from multi-tier video delivery.
    // A future change could add image thumbnails (a 32×32 JPEG arrives in
    // ~5 KB; the 1080p original follows) but that's separate from this
    // video-tiering policy.
    if (!ctx.isVideo) return standardOnly();

    // Very small source: tiering is pure overhead. The full snap probably
    // fits in a single UDP datagram or a few WS frames.
    // 200 KB is roughly where the overhead of three encodes outweighs
    // the latency win of a quick thumbnail.
    if (ctx.inputSizeBytes > 0 && ctx.inputSizeBytes < 200 * 1024) {
        qCDebug(lcTiered) << "Source size" << ctx.inputSizeBytes
                          << "bytes — too small to tier; Standard only";
        return standardOnly();
    }

    // Same-LAN peer with good history: skip tiering, just send Standard.
    // Same-LAN gigabit transfers a few-MB clip in well under a second;
    // thumbnail-first is wasted work.
    const bool isSameLanPeer = ctx.peerClassName == QLatin1String("rfc1918")
                            || ctx.peerClassName == QLatin1String("rfc1918-same-lan");
    const bool peerIsFast = ctx.peerSuccessRate > 0.8
                         && ctx.peerMedianMs >= 0
                         && ctx.peerMedianMs < 200;
    if (isSameLanPeer && peerIsFast && ctx.inputSizeBytes < 2 * 1024 * 1024) {
        qCDebug(lcTiered) << "Same-LAN fast peer + <2MB source — Standard only";
        return standardOnly();
    }

    // Large source: full tiered chain ending in HighQuality.
    // 5 MB threshold is roughly where the user really cares about quality
    // (longer clips, more content worth preserving) AND where a thumbnail
    // is most beneficial (full transfer takes 10+ seconds on cellular).
    if (ctx.inputSizeBytes > 5 * 1024 * 1024) {
        qCDebug(lcTiered) << "Large source (" << ctx.inputSizeBytes
                          << "bytes) — full tier chain";
        return Plan{ EncodeTier::Thumbnail,
                     EncodeTier::Preview,
                     EncodeTier::Standard,
                     EncodeTier::HighQuality };
    }

    // Cellular / CGNAT / unknown peer: tier without HighQuality. The user
    // is likely on a slow link; chasing pristine quality wastes their
    // bandwidth and our encode time.
    const bool cellularLike =
        ctx.peerClassName == QLatin1String("cgnat") ||
        ctx.peerSuccessRate < 0.5 ||  // unreliable peer
        (ctx.peerMedianMs > 0 && ctx.peerMedianMs > 2000);
    if (cellularLike) {
        qCDebug(lcTiered) << "Cellular-like peer — Thumbnail+Preview+Standard";
        return Plan{ EncodeTier::Thumbnail,
                     EncodeTier::Preview,
                     EncodeTier::Standard };
    }

    // Default: Preview + Standard. Skips the tiny-thumbnail step on
    // medium-quality connections — Preview itself arrives in ~1-3s on
    // most internet links, which is fast enough for "something on screen
    // quickly" UX without spending the encode budget on three tiers.
    qCDebug(lcTiered) << "Default plan: Preview+Standard";
    return Plan{ EncodeTier::Preview, EncodeTier::Standard };
}

QString planToString(const Plan &plan)
{
    if (plan.isEmpty()) return QStringLiteral("(empty — source pass-through)");
    QStringList parts;
    for (auto t : plan) parts << QString::fromLatin1(
        SnapClone::Core::Media::tierName(t));
    return parts.join(QStringLiteral(" → "));
}

} // namespace SnapClone::Core::TieredDelivery
