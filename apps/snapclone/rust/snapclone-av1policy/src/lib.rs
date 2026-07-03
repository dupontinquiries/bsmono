//! snapclone-av1policy — AV1 encode-strategy policy.
//!
//! # The advanced-compression idea
//!
//! AV1 (and SVT-AV1) encode frames in a HIERARCHICAL reference structure: an
//! I-frame (temporal layer 0) anchors a GOP, then B-frames form a pyramid —
//! lower-layer frames are REFERENCED BY many later frames; higher-layer "leaf"
//! frames are decoded once and never predicted from. Bits invested in a
//! reference frame PROPAGATE to everything that predicts from it; bits spent on
//! a leaf frame benefit only that one frame.
//!
//! The big compression win is to let the encoder CONCENTRATE quality on
//! I-frames and reference frames and spend less on leaves. SVT-AV1 does this
//! automatically — but only well in CONSTANT-QUALITY (CRF / `--rc 0`) mode,
//! where it's free to spend few bits on a leaf and many on a reference. Under
//! the strict TARGET-BITRATE mode the current encoder uses, the rate controller
//! flattens that distribution to hit the byte budget per window, throwing away
//! most of the hierarchical advantage.
//!
//! So the policy here, per tier:
//!   * choose CRF (constant-quality) for the user-facing tiers so the encoder
//!     can do the reference-frame bit concentration,
//!   * keep bitrate mode for the tiny thumbnail/preview tiers (predictable size
//!     matters more than squeezing the last bytes there),
//!   * pick GOP length + hierarchical depth, balancing compression vs the
//!     seekability the player needs.
//!
//! # The seekability tension (why this is a policy, not a constant)
//!
//! More I-frames = better scrubbing but WORSE compression (I-frames are
//! expensive — no temporal prediction). The current code forces a 1s GOP purely
//! for seekability. A clip that's watched once end-to-end (the common snap)
//! doesn't need 1s seek granularity; a longer GOP + deeper hierarchy saves real
//! bytes. We expose `seekable` so the caller can ask for the scrub-friendly
//! short GOP when the user actually scrubs, and the byte-efficient long GOP for
//! plain delivery.
//!
//! All of this is pure decision logic — no I/O, no deps — so it's unit-tested
//! here; the C++ backends translate a `Plan` into SVT-AV1 `parameters-string`
//! / ffmpeg flags.

/// Quality/size tier (mirrors core::EncodeTier; kept as a plain enum so this
/// crate has no dependency on the C++ side).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Thumbnail,
    Preview,
    Standard,
    HighQuality,
}

/// Rate-control mode the backend should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateControl {
    /// Constant-quality: encoder spends bits where they help most (lets it
    /// concentrate quality on I-frames / reference frames). `crf` is the
    /// quality target (lower = better/bigger; AV1 CRF range ~0..63).
    ConstantQuality { crf: u8 },
    /// Strict target bitrate in kbps (predictable size; used for tiny tiers).
    TargetBitrate { kbps: u32 },
}

/// The full encode strategy for one transcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub rate_control: RateControl,
    /// Keyframe interval in frames (GOP length). Larger = better compression,
    /// coarser seeking.
    pub keyint: u32,
    /// Hierarchical prediction depth (SVT-AV1 `hierarchical-levels`): how many
    /// temporal layers the B-frame pyramid has. Deeper = more reference reuse =
    /// better compression, slightly higher latency/complexity. 0 = encoder
    /// default (let SVT pick).
    pub hierarchical_levels: u8,
    /// Enable SVT-AV1 temporal filtering (`enable-tf`): denoises the frames
    /// that become references so predictions off them are cleaner — a direct
    /// reference-frame-quality boost. Most effective on the user-facing tiers.
    pub temporal_filtering: bool,
    /// Whether this plan favors scrubbing (short GOP) over byte efficiency.
    pub seekable: bool,
}

/// Decide the encode strategy.
///
/// `seekable`: caller wants scrub-friendly short GOPs (true) or byte-efficient
/// long GOPs (false). `fps`: source frame rate, used to size the GOP in frames
/// from a target GOP DURATION (so the cadence is fps-independent). Defaults to
/// 30 when unknown (0).
pub fn plan_for(tier: Tier, seekable: bool, fps: u32) -> Plan {
    let fps = if fps == 0 { 30 } else { fps };

    // GOP duration: ~1s when seekable (matches the current behavior), longer
    // when optimizing for size. AV1 keeps quality across long GOPs far better
    // than older codecs, so a 4s GOP on delivery is safe and saves the repeated
    // I-frame cost.
    let gop_secs = if seekable { 1 } else { 4 };
    let keyint = (gop_secs * fps).max(1);

    match tier {
        // Tiny tiers: predictable size beats squeezing bytes. Keep target
        // bitrate, shallow hierarchy, frequent-enough keys, no temporal filter
        // (speed dominates at preset 13).
        Tier::Thumbnail => Plan {
            rate_control: RateControl::TargetBitrate { kbps: thumbnail_kbps() },
            keyint: keyint.max(fps), // at least ~1s even when "not seekable"
            hierarchical_levels: 0,
            temporal_filtering: false,
            seekable,
        },
        Tier::Preview => Plan {
            rate_control: RateControl::TargetBitrate { kbps: preview_kbps() },
            keyint: keyint.max(fps),
            hierarchical_levels: 0,
            temporal_filtering: false,
            seekable,
        },
        // User-facing tiers: CONSTANT-QUALITY so AV1 can concentrate bits on
        // references, plus a deeper hierarchy and temporal filtering. This is
        // the advanced-compression path.
        Tier::Standard => Plan {
            rate_control: RateControl::ConstantQuality { crf: 32 },
            keyint,
            hierarchical_levels: 4,
            temporal_filtering: true,
            seekable,
        },
        Tier::HighQuality => Plan {
            rate_control: RateControl::ConstantQuality { crf: 26 },
            keyint,
            hierarchical_levels: 5,
            temporal_filtering: true,
            seekable,
        },
    }
}

fn thumbnail_kbps() -> u32 {
    300
}
fn preview_kbps() -> u32 {
    700
}

impl Plan {
    /// True when the encoder will be in a mode that can concentrate quality on
    /// reference frames (the advanced-compression path).
    pub fn concentrates_reference_quality(&self) -> bool {
        matches!(self.rate_control, RateControl::ConstantQuality { .. })
    }
}

// ─── C FFI ───────────────────────────────────────────────────────────────────
//
// The C++ backends call sc_av1_plan() to get the strategy, then translate it
// into SVT-AV1 parameters-string / ffmpeg flags. Flat repr(C) struct so no
// allocation crosses the boundary.

/// C-ABI mirror of Plan. `use_crf` selects the rate-control union: when true,
/// `crf` is valid and `kbps` is 0; when false, `kbps` is valid and `crf` is 0.
#[repr(C)]
pub struct CAv1Plan {
    pub use_crf: u8, // 1 = constant-quality (CRF), 0 = target bitrate
    pub crf: u8,
    pub kbps: u32,
    pub keyint: u32,
    pub hierarchical_levels: u8,
    pub temporal_filtering: u8, // 1/0
}

/// Tier codes matching core::EncodeTier's integer values (Thumbnail=0 ..
/// HighQuality=3). Unknown codes default to Standard.
fn tier_from_code(code: u8) -> Tier {
    match code {
        0 => Tier::Thumbnail,
        1 => Tier::Preview,
        3 => Tier::HighQuality,
        _ => Tier::Standard,
    }
}

/// Compute the AV1 encode plan for a tier. `seekable` != 0 asks for the
/// scrub-friendly short GOP; `fps` 0 defaults to 30.
#[no_mangle]
pub extern "C" fn sc_av1_plan(tier_code: u8, seekable: u8, fps: u32) -> CAv1Plan {
    let plan = plan_for(tier_from_code(tier_code), seekable != 0, fps);
    let (use_crf, crf, kbps) = match plan.rate_control {
        RateControl::ConstantQuality { crf } => (1u8, crf, 0u32),
        RateControl::TargetBitrate { kbps } => (0u8, 0u8, kbps),
    };
    CAv1Plan {
        use_crf,
        crf,
        kbps,
        keyint: plan.keyint,
        hierarchical_levels: plan.hierarchical_levels,
        temporal_filtering: plan.temporal_filtering as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_tiers_use_constant_quality() {
        // The core of the idea: Standard + HighQuality must use CRF so the
        // encoder can spend bits on references, not flatten under a bitrate cap.
        assert!(plan_for(Tier::Standard, false, 30).concentrates_reference_quality());
        assert!(plan_for(Tier::HighQuality, false, 30).concentrates_reference_quality());
    }

    #[test]
    fn tiny_tiers_stay_bitrate_for_predictable_size() {
        assert!(!plan_for(Tier::Thumbnail, false, 30).concentrates_reference_quality());
        assert!(!plan_for(Tier::Preview, false, 30).concentrates_reference_quality());
    }

    #[test]
    fn highquality_is_higher_quality_than_standard() {
        // Lower CRF = better quality. HighQuality must be <= Standard's CRF.
        let s = plan_for(Tier::Standard, false, 30);
        let h = plan_for(Tier::HighQuality, false, 30);
        match (s.rate_control, h.rate_control) {
            (RateControl::ConstantQuality { crf: cs }, RateControl::ConstantQuality { crf: ch }) => {
                assert!(ch < cs, "HighQuality CRF must be lower (better) than Standard");
            }
            _ => panic!("both user tiers should be CRF"),
        }
        // And a deeper hierarchy.
        assert!(h.hierarchical_levels >= s.hierarchical_levels);
    }

    #[test]
    fn seekable_gives_short_gop_size_gives_long() {
        // The seekability tension: scrub-friendly = ~1s GOP; delivery = longer.
        let seek = plan_for(Tier::Standard, true, 30);
        let size = plan_for(Tier::Standard, false, 30);
        assert_eq!(seek.keyint, 30, "seekable ~1s GOP at 30fps");
        assert!(size.keyint > seek.keyint, "size-optimized GOP must be longer");
    }

    #[test]
    fn gop_is_fps_independent() {
        // 1s GOP scales with fps so the cadence is consistent across sources.
        assert_eq!(plan_for(Tier::Standard, true, 30).keyint, 30);
        assert_eq!(plan_for(Tier::Standard, true, 60).keyint, 60);
        assert_eq!(plan_for(Tier::Standard, true, 24).keyint, 24);
    }

    #[test]
    fn fps_zero_defaults_to_30() {
        assert_eq!(plan_for(Tier::Standard, true, 0).keyint, 30);
    }

    #[test]
    fn user_tiers_enable_temporal_filtering() {
        // Temporal filtering cleans reference frames → better predictions.
        assert!(plan_for(Tier::Standard, false, 30).temporal_filtering);
        assert!(plan_for(Tier::HighQuality, false, 30).temporal_filtering);
        assert!(!plan_for(Tier::Thumbnail, false, 30).temporal_filtering);
    }

    #[test]
    fn user_tiers_have_a_real_hierarchy() {
        // Deeper hierarchy = more reference reuse = better compression.
        assert!(plan_for(Tier::Standard, false, 30).hierarchical_levels >= 4);
        assert!(plan_for(Tier::HighQuality, false, 30).hierarchical_levels >= 4);
    }

    #[test]
    fn crf_in_valid_av1_range() {
        for t in [Tier::Standard, Tier::HighQuality] {
            if let RateControl::ConstantQuality { crf } = plan_for(t, false, 30).rate_control {
                assert!(crf <= 63, "AV1 CRF must be 0..=63");
            }
        }
    }
}
