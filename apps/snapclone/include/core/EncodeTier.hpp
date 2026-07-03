#pragma once
// ─── core/EncodeTier.hpp ─────────────────────────────────────────────────────
//
// Shared media-quality tier enum used by both:
//   • core/TieredDeliveryPolicy (decides which tiers to send)
//   • ui/video/VideoEncoderBackend (maps tier → encoder args)
//
// Lives in core/ to keep the dependency direction clean: core code never
// includes from ui/. Both core and ui can include this.
//
// ─────────────────────────────────────────────────────────────────────────────

#include <QtGlobal>

namespace SnapClone::Core::Media {

/// Quality / encode-speed tier for a particular transcode invocation.
/// Encoder backends translate this to backend-specific args (svtav1enc
/// preset, ffmpeg -preset, custom-codec --tier). This is the "what does
/// the caller want?" abstraction; backends figure out how to deliver it.
enum class EncodeTier : quint8 {
    Thumbnail   = 0,  ///< Arrives-first symbol: tiny (~50 KB), 64-128px wide,
                       ///< 1-3 keyframes only. Roughly an "animated GIF" feel
                       ///< for video, a JPEG thumbnail for stills.
    Preview     = 1,  ///< Quick-look quality: 360-480p wide, ~150-300 KB for
                       ///< a 5s clip. Encoder runs at fastest preset.
    Standard    = 2,  ///< Default-quality: matches source resolution up to
                       ///< 720p, balanced preset. Most snaps land here.
    HighQuality = 3,  ///< Source resolution preserved, quality preset.
                       ///< Larger file, slower encode. For the "I really
                       ///< wanted high quality video messages" use case.
};

[[nodiscard]] constexpr const char *tierName(EncodeTier t) noexcept
{
    switch (t) {
        case EncodeTier::Thumbnail:   return "thumbnail";
        case EncodeTier::Preview:     return "preview";
        case EncodeTier::Standard:    return "standard";
        case EncodeTier::HighQuality: return "highquality";
    }
    return "?";
}

/// Wire-format marker for tier on a transmitted snap version.
/// Lives next to MediaType (Types.hpp) conceptually — both are byte values
/// transmitted on the wire and must remain stable across versions.
[[nodiscard]] constexpr quint8 tierToWire(EncodeTier t) noexcept {
    return static_cast<quint8>(t);
}

[[nodiscard]] constexpr EncodeTier tierFromWire(quint8 v) noexcept {
    // Defensive: unknown wire value clamps to Standard rather than
    // producing an out-of-range enum (which is UB to switch on).
    switch (v) {
        case 0: return EncodeTier::Thumbnail;
        case 1: return EncodeTier::Preview;
        case 2: return EncodeTier::Standard;
        case 3: return EncodeTier::HighQuality;
        default: return EncodeTier::Standard;
    }
}

} // namespace SnapClone::Core::Media
