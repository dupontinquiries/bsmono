#pragma once
// ─── ui/video/Av1Policy.hpp ───────────────────────────────────────────────────
//
// AV1 encode-strategy policy: per-tier choice of constant-quality (CRF) vs
// target-bitrate, GOP length, hierarchical reference depth, and temporal
// filtering. This is the C++ MIRROR of the tested Rust crate
// `rust/snapclone-av1policy` — the Rust crate is the source of truth and carries
// the unit tests (CRF range, fps-independent GOP, user-tiers-use-CRF, etc.);
// this header reproduces the SAME decisions so the encoder backends don't need
// an FFI link for a few lines of arithmetic. Keep the two in lockstep: if you
// change a threshold here, change it in the Rust crate (and its tests) too.
//
// # Why this matters (the advanced-compression idea)
//
// AV1 encodes frames in a HIERARCHICAL reference pyramid: the I-frame and the
// lower-layer B-frames are REFERENCED BY many later frames; higher-layer "leaf"
// frames are decoded once. Bits spent on a reference frame propagate to
// everything predicting from it. SVT-AV1 concentrates quality on references
// AUTOMATICALLY — but only well in CONSTANT-QUALITY (CRF) mode. Under strict
// target-bitrate the rate controller flattens that distribution to hit the byte
// budget, discarding most of the hierarchical advantage. So for the user-facing
// tiers we switch to CRF + a deeper hierarchy + temporal filtering, which is
// the AV1 advanced-compression path. Tiny tiers keep bitrate (predictable size
// beats squeezing bytes there).
// ─────────────────────────────────────────────────────────────────────────────

#include "core/EncodeTier.hpp"
#include <cstdint>

namespace SnapClone::UI::Video {

using SnapClone::Core::Media::EncodeTier;

struct Av1Plan {
    bool     useCrf = true;          ///< true = CRF (constant quality), false = bitrate
    uint8_t  crf = 32;               ///< AV1 CRF 0..63 (lower = better); valid when useCrf
    uint32_t kbps = 0;               ///< target bitrate; valid when !useCrf
    uint32_t keyint = 30;            ///< GOP length in frames
    uint8_t  hierarchicalLevels = 0; ///< SVT hierarchical-levels; 0 = encoder default
    bool     temporalFiltering = false;
};

/// Mirror of snapclone_av1policy::plan_for. `seekable` true = scrub-friendly
/// short (~1s) GOP; false = byte-efficient long GOP. `fps` 0 defaults to 30.
inline Av1Plan av1PlanFor(EncodeTier tier, bool seekable, uint32_t fps)
{
    if (fps == 0) fps = 30;
    const uint32_t gopSecs = seekable ? 1u : 4u;
    uint32_t keyint = gopSecs * fps;
    if (keyint < 1) keyint = 1;

    Av1Plan p;
    p.keyint = keyint;
    switch (tier) {
        case EncodeTier::Thumbnail:
            p.useCrf = false; p.kbps = 300;
            p.keyint = keyint < fps ? fps : keyint;   // at least ~1s
            p.hierarchicalLevels = 0; p.temporalFiltering = false;
            break;
        case EncodeTier::Preview:
            p.useCrf = false; p.kbps = 700;
            p.keyint = keyint < fps ? fps : keyint;
            p.hierarchicalLevels = 0; p.temporalFiltering = false;
            break;
        case EncodeTier::Standard:
            p.useCrf = true; p.crf = 32;
            p.hierarchicalLevels = 4; p.temporalFiltering = true;
            break;
        case EncodeTier::HighQuality:
            p.useCrf = true; p.crf = 26;
            p.hierarchicalLevels = 5; p.temporalFiltering = true;
            break;
    }
    return p;
}

} // namespace SnapClone::UI::Video
