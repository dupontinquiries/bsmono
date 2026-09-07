//! What this machine can actually decode, and at what cost.
//!
//! This is the input to format selection. It is deliberately a *data* model,
//! not a set of `#[cfg]` branches: it is built at runtime by probing (V4L2
//! enum_fmt, VAAPI profile query, Vulkan video queue caps), and it is
//! constructible by hand in tests. Every "should we pick AV1 or VP9 here"
//! question resolves against this and nothing else.

use crate::codec::{Chroma, Codec, VideoProfile};

/// Roughly how much it costs to decode this way. Not milliwatts — an ordering.
/// Selection uses the ordering; the UI uses it to explain itself.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Cost {
    /// Fixed-function block. Near-free, and the frame lands in a DMA-BUF.
    Fixed,
    /// GPU compute shader decode. Rare; listed for completeness.
    GpuCompute,
    /// Well-vectorised software decode on big cores (dav1d w/ NEON, ffvp9 AVX2).
    SoftwareSimd,
    /// Software decode that will make the fans spin or the phone hot.
    SoftwareSlow,
}

impl Cost {
    pub fn is_hardware(self) -> bool {
        matches!(self, Cost::Fixed | Cost::GpuCompute)
    }
}

/// How the decoded frame reaches us. Drives the presenter, not the decoder.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Delivery {
    /// Exported DMA-BUF fds, importable into Vulkan with
    /// `VK_EXT_external_memory_dma_buf` + `VK_EXT_image_drm_format_modifier`.
    DmaBuf,
    /// Plain CPU planes. Needs a staging upload every frame.
    CpuPlanes,
}

/// One (codec, constraint) capability entry.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecodeCap {
    pub codec: Codec,
    pub max_bit_depth: u8,
    pub max_chroma: Chroma,
    pub max_width: u32,
    pub max_height: u32,
    /// Highest frame rate this path sustains at `max_width x max_height`.
    /// Used to reject "4K60 AV1 in software on a Fairphone" before we try it.
    pub max_fps_at_max_size: u32,
    pub cost: Cost,
    pub delivery: Delivery,
    pub backend: Backend,
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Backend {
    /// V4L2 stateless / Request API. Asahi AVD (`apple,avd`) is this.
    V4l2Stateless,
    /// V4L2 stateful M2M. Qualcomm Venus is this.
    V4l2Stateful,
    Vaapi,
    /// `VK_KHR_video_decode_*`. RADV, ANV, NVK.
    VulkanVideo,
    Dav1d,
    /// libavcodec software path.
    FfmpegSw,
}

impl DecodeCap {
    pub fn accepts(&self, p: &VideoProfile, w: u32, h: u32, fps: u32) -> bool {
        if p.codec != self.codec {
            return false;
        }
        if p.bit_depth > self.max_bit_depth {
            return false;
        }
        if chroma_rank(p.chroma) > chroma_rank(self.max_chroma) {
            return false;
        }
        if w > self.max_width || h > self.max_height {
            return false;
        }
        // Scale the fps budget by how much smaller than max this stream is.
        // Crude but it is the right shape: software decode throughput is
        // roughly pixels/second, so a 1080p stream on a "4K30" software path
        // is fine at 60.
        let max_px = (self.max_width as u64) * (self.max_height as u64);
        let px = (w as u64).max(1) * (h as u64).max(1);
        if px == 0 {
            return false;
        }
        let budget = (self.max_fps_at_max_size as u64) * max_px / px;
        (fps as u64) <= budget
    }
}

fn chroma_rank(c: Chroma) -> u8 {
    match c {
        Chroma::Yuv420 => 0,
        Chroma::Yuv422 => 1,
        Chroma::Yuv444 => 2,
    }
}

/// Non-decode constraints that still change which stream we should fetch.
#[derive(Clone, PartialEq, Debug)]
pub struct PlaybackBudget {
    /// Physical output size in pixels. No point pulling 4K for a 1080x2340 phone.
    pub display_width: u32,
    pub display_height: u32,
    pub display_max_hz: u32,
    /// Hard ceiling in bits/sec, or `None` for unmetered.
    pub max_bitrate: Option<u64>,
    /// On battery: weight `Cost` much more heavily than resolution.
    pub on_battery: bool,
    /// Presenter can display >8 bit / wide gamut end to end.
    pub hdr_capable: bool,
}

impl Default for PlaybackBudget {
    fn default() -> Self {
        Self {
            display_width: 1920,
            display_height: 1080,
            display_max_hz: 60,
            max_bitrate: None,
            on_battery: false,
            hdr_capable: false,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct DeviceCaps {
    /// Free-form, for logs and the "why did it pick that" UI panel.
    pub name: String,
    pub caps: Vec<DecodeCap>,
    pub budget: PlaybackBudget,
}

impl DeviceCaps {
    /// Best (lowest-cost) path that can take this stream, if any.
    pub fn best_path(&self, p: &VideoProfile, w: u32, h: u32, fps: u32) -> Option<&DecodeCap> {
        self.caps
            .iter()
            .filter(|c| c.accepts(p, w, h, fps))
            .min_by_key(|c| c.cost)
    }

    pub fn can_decode(&self, p: &VideoProfile, w: u32, h: u32, fps: u32) -> bool {
        self.best_path(p, w, h, fps).is_some()
    }
}

// ---------------------------------------------------------------------------
// Reference profiles.
//
// These are starting points, NOT a substitute for runtime probing. Ship the
// probe; use these as the fallback when probing fails and as test fixtures.
// The numbers below are my estimates and are the thing most likely to be wrong
// in this file — measure them on your own hardware before trusting them.
// ---------------------------------------------------------------------------

/// Apple M1/M2 under Asahi.
///
/// The important fact: **there is no VP9 or AV1 hardware decode here.** The AGX
/// video block (AVD) is driven by the `apple,avd` V4L2 stateless driver and, as
/// of my knowledge, exposes H.264 only. Honeykrisp does not expose
/// `VK_KHR_video_decode_*`. So VP9 and AV1 are CPU work — but on Firestorm/Avalanche
/// cores with dav1d's NEON paths that is genuinely fine for 1080p, and AV1 via
/// dav1d comfortably beats VP9 via libavcodec at equal quality.
///
/// That inversion is the whole point of modelling this: on Asahi we *prefer AV1*,
/// on the Fairphone we *avoid it*.
pub fn asahi_m_series() -> DeviceCaps {
    DeviceCaps {
        name: "Apple M-series (Asahi)".into(),
        caps: vec![
            DecodeCap {
                codec: Codec::H264,
                max_bit_depth: 8,
                max_chroma: Chroma::Yuv420,
                max_width: 4096,
                max_height: 4096,
                max_fps_at_max_size: 30,
                cost: Cost::Fixed,
                delivery: Delivery::DmaBuf,
                backend: Backend::V4l2Stateless,
            },
            DecodeCap {
                codec: Codec::Av1,
                max_bit_depth: 12,
                max_chroma: Chroma::Yuv444,
                max_width: 3840,
                max_height: 2160,
                max_fps_at_max_size: 30,
                cost: Cost::SoftwareSimd,
                delivery: Delivery::CpuPlanes,
                backend: Backend::Dav1d,
            },
            DecodeCap {
                codec: Codec::Vp9,
                max_bit_depth: 12,
                max_chroma: Chroma::Yuv420,
                max_width: 3840,
                max_height: 2160,
                // ffvp9 is decent but not dav1d; deliberately rated below AV1
                // by giving it the slower Cost, not a smaller fps budget.
                max_fps_at_max_size: 30,
                cost: Cost::SoftwareSlow,
                delivery: Delivery::CpuPlanes,
                backend: Backend::FfmpegSw,
            },
            DecodeCap {
                codec: Codec::Hevc,
                max_bit_depth: 10,
                max_chroma: Chroma::Yuv420,
                max_width: 3840,
                max_height: 2160,
                max_fps_at_max_size: 30,
                cost: Cost::SoftwareSlow,
                delivery: Delivery::CpuPlanes,
                backend: Backend::FfmpegSw,
            },
        ],
        budget: PlaybackBudget {
            display_width: 2560,
            display_height: 1600,
            display_max_hz: 60,
            max_bitrate: None,
            on_battery: true,
            hdr_capable: true,
        },
    }
}

/// Fairphone 4/5 class Qualcomm under postmarketOS with mainline Venus.
///
/// Venus is a **stateful** V4L2 M2M decoder: H.264, HEVC, VP9. No AV1 — AV1
/// fixed-function decode does not appear on Qualcomm until the 8-Gen-3 era.
/// So here the preference inverts: VP9 in hardware beats AV1 in software by an
/// enormous margin in both frame rate and battery, even though AV1 would be
/// the smaller download.
///
/// Caveat worth its own line: mainline Venus on these SoCs needs signed
/// firmware present and the right `venus` DT node. If probing finds no
/// `/dev/video*` M2M decoder, fall back to `fairphone_no_venus()`.
pub fn fairphone_venus() -> DeviceCaps {
    DeviceCaps {
        name: "Fairphone (Qualcomm Venus, postmarketOS)".into(),
        caps: vec![
            DecodeCap {
                codec: Codec::H264,
                max_bit_depth: 8,
                max_chroma: Chroma::Yuv420,
                max_width: 3840,
                max_height: 2160,
                max_fps_at_max_size: 30,
                cost: Cost::Fixed,
                delivery: Delivery::DmaBuf,
                backend: Backend::V4l2Stateful,
            },
            DecodeCap {
                codec: Codec::Hevc,
                max_bit_depth: 10,
                max_chroma: Chroma::Yuv420,
                max_width: 3840,
                max_height: 2160,
                max_fps_at_max_size: 30,
                cost: Cost::Fixed,
                delivery: Delivery::DmaBuf,
                backend: Backend::V4l2Stateful,
            },
            DecodeCap {
                codec: Codec::Vp9,
                max_bit_depth: 10,
                max_chroma: Chroma::Yuv420,
                max_width: 3840,
                max_height: 2160,
                max_fps_at_max_size: 30,
                cost: Cost::Fixed,
                delivery: Delivery::DmaBuf,
                backend: Backend::V4l2Stateful,
            },
            DecodeCap {
                codec: Codec::Av1,
                max_bit_depth: 10,
                max_chroma: Chroma::Yuv420,
                // dav1d on a 778G-class little/big mix: 1080p30 is realistic,
                // 1080p60 is marginal, 4K is not happening.
                max_width: 1920,
                max_height: 1080,
                max_fps_at_max_size: 30,
                cost: Cost::SoftwareSimd,
                delivery: Delivery::CpuPlanes,
                backend: Backend::Dav1d,
            },
        ],
        budget: PlaybackBudget {
            display_width: 1080,
            display_height: 2340,
            display_max_hz: 90,
            max_bitrate: Some(6_000_000),
            on_battery: true,
            hdr_capable: false,
        },
    }
}

/// Same phone, Venus unavailable. Everything is software; be conservative.
pub fn fairphone_no_venus() -> DeviceCaps {
    let mut d = fairphone_venus();
    d.name = "Fairphone (no Venus — software only)".into();
    for c in &mut d.caps {
        if c.cost == Cost::Fixed {
            c.cost = Cost::SoftwareSlow;
            c.delivery = Delivery::CpuPlanes;
            c.backend = Backend::FfmpegSw;
            c.max_width = 1920;
            c.max_height = 1080;
            c.max_fps_at_max_size = 30;
        }
    }
    d
}

/// Desktop AMD RDNA2+ / Intel Arc / recent iGPU: everything in fixed function.
pub fn desktop_full_hw() -> DeviceCaps {
    let mk = |codec, depth| DecodeCap {
        codec,
        max_bit_depth: depth,
        max_chroma: Chroma::Yuv420,
        max_width: 7680,
        max_height: 4320,
        max_fps_at_max_size: 60,
        cost: Cost::Fixed,
        delivery: Delivery::DmaBuf,
        backend: Backend::Vaapi,
    };
    DeviceCaps {
        name: "Desktop (VAAPI, full fixed-function)".into(),
        caps: vec![
            mk(Codec::H264, 8),
            mk(Codec::Hevc, 10),
            mk(Codec::Vp9, 10),
            mk(Codec::Av1, 10),
        ],
        budget: PlaybackBudget {
            display_width: 3840,
            display_height: 2160,
            display_max_hz: 144,
            max_bitrate: None,
            on_battery: false,
            hdr_capable: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::parse_codec_string;

    fn prof(s: &str) -> VideoProfile {
        parse_codec_string(s).unwrap()
    }

    #[test]
    fn asahi_has_no_hardware_av1_or_vp9() {
        let d = asahi_m_series();
        for codec in [Codec::Av1, Codec::Vp9] {
            let hw = d.caps.iter().any(|c| c.codec == codec && c.cost.is_hardware());
            assert!(!hw, "{codec} must not claim hardware decode on Asahi");
        }
        // ...but H.264 is fixed-function via AVD.
        let h264 = d
            .best_path(&prof("avc1.640028"), 1920, 1080, 30)
            .expect("h264 path");
        assert_eq!(h264.cost, Cost::Fixed);
        assert_eq!(h264.delivery, Delivery::DmaBuf);
    }

    #[test]
    fn asahi_prefers_av1_over_vp9_in_software() {
        let d = asahi_m_series();
        let av1 = d.best_path(&prof("av01.0.08M.08"), 1920, 1080, 60).unwrap();
        let vp9 = d.best_path(&prof("vp09.00.50.08"), 1920, 1080, 60).unwrap();
        assert!(av1.cost < vp9.cost, "dav1d should outrank ffvp9 on Asahi");
    }

    #[test]
    fn fairphone_prefers_vp9_over_av1() {
        let d = fairphone_venus();
        let av1 = d.best_path(&prof("av01.0.08M.08"), 1920, 1080, 30).unwrap();
        let vp9 = d.best_path(&prof("vp09.00.50.08"), 1920, 1080, 30).unwrap();
        assert!(vp9.cost < av1.cost, "Venus VP9 must beat dav1d AV1 here");
        assert_eq!(vp9.delivery, Delivery::DmaBuf);
    }

    #[test]
    fn fps_budget_scales_with_resolution() {
        let d = fairphone_venus();
        let p = prof("av01.0.08M.08");
        // 1080p60 AV1 in software: over budget (cap is 1080p30).
        assert!(d.best_path(&p, 1920, 1080, 60).is_none());
        // 720p60 is ~half the pixels, so it fits.
        assert!(d.best_path(&p, 1280, 720, 60).is_some());
    }

    /// A real probe produces *both* a hardware and a software entry for the
    /// same codec — Venus VP9 plus an ffmpeg VP9 fallback. The reference
    /// profiles above list one path per codec, which meant `best_path`'s
    /// preference for the cheapest was never actually exercised: with one
    /// candidate, picking the min and picking the max are the same thing.
    #[test]
    fn among_two_paths_for_one_codec_the_cheaper_wins() {
        let d = DeviceCaps {
            name: "hw + sw fallback for the same codec".into(),
            caps: vec![
                DecodeCap {
                    codec: Codec::Vp9,
                    max_bit_depth: 10,
                    max_chroma: Chroma::Yuv420,
                    max_width: 3840,
                    max_height: 2160,
                    max_fps_at_max_size: 30,
                    cost: Cost::SoftwareSlow,
                    delivery: Delivery::CpuPlanes,
                    backend: Backend::FfmpegSw,
                },
                DecodeCap {
                    codec: Codec::Vp9,
                    max_bit_depth: 10,
                    max_chroma: Chroma::Yuv420,
                    max_width: 3840,
                    max_height: 2160,
                    max_fps_at_max_size: 30,
                    cost: Cost::Fixed,
                    delivery: Delivery::DmaBuf,
                    backend: Backend::V4l2Stateful,
                },
            ],
            budget: PlaybackBudget::default(),
        };
        let p = prof("vp09.00.50.08");
        let best = d.best_path(&p, 1920, 1080, 30).unwrap();
        assert_eq!(best.cost, Cost::Fixed, "must not fall back to software");
        assert_eq!(best.backend, Backend::V4l2Stateful);
        assert_eq!(best.delivery, Delivery::DmaBuf);
    }

    /// The size limit must be enforced in its own right, not just as a
    /// side-effect of the frame-rate budget. A slow, huge stream slips through
    /// the fps check (its budget is generous at low fps) and would otherwise
    /// be handed to a decoder that cannot allocate for it.
    #[test]
    fn an_oversize_stream_is_rejected_even_at_a_low_frame_rate() {
        let d = fairphone_venus();
        let p = prof("av01.0.08M.08"); // dav1d path, capped at 1920x1080
        // 5 fps at 4K: the pixel-scaled fps budget alone would allow this.
        assert!(
            d.best_path(&p, 3840, 2160, 5).is_none(),
            "size limit must be checked independently of frame rate"
        );
    }

    #[test]
    fn oversize_streams_are_rejected_not_clamped() {
        let d = fairphone_venus();
        assert!(d
            .best_path(&prof("av01.0.12M.10"), 3840, 2160, 30)
            .is_none());
    }

    #[test]
    fn losing_venus_downgrades_everything() {
        let d = fairphone_no_venus();
        assert!(!d.caps.iter().any(|c| c.cost.is_hardware()));
        assert!(d.caps.iter().all(|c| c.delivery == Delivery::CpuPlanes));
    }

    #[test]
    fn desktop_takes_10bit_av1_4k() {
        let d = desktop_full_hw();
        let c = d.best_path(&prof("av01.0.12M.10"), 3840, 2160, 60).unwrap();
        assert_eq!(c.cost, Cost::Fixed);
    }

    #[test]
    fn twelve_bit_is_rejected_by_ten_bit_hardware() {
        let d = desktop_full_hw();
        let p = VideoProfile {
            codec: Codec::Av1,
            profile: Some(2),
            bit_depth: 12,
            chroma: Chroma::Yuv420,
            level: None,
        };
        assert!(d.best_path(&p, 1920, 1080, 30).is_none());
    }
}
