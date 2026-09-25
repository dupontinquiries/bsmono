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
    /// Hard ceiling on frames per second, whatever the pixel arithmetic says.
    ///
    /// The scaling below is pixels-per-second, which is the right SHAPE and
    /// too generous at the small end: a "4K30" software path extrapolates to
    /// 120 fps at 1080p, and no software decoder does that. Throughput stops
    /// being pixel-bound and becomes per-frame bound — parse, entropy decode,
    /// reference management and the memory traffic of handing frames on all
    /// cost per FRAME regardless of size.
    ///
    /// A flat fps ceiling cannot express the two data points reported from the
    /// target hardware — 480p at 90 is fine in performance mode, software
    /// decode at 1080p60 drops frames sometimes — because 90 is above 60 and a
    /// single number cannot allow one and refuse the other. PIXEL RATE can:
    /// 480p90 is ~37 Mpx/s and 1080p60 is ~124 Mpx/s, so rating a software
    /// path at 1080p30 (~62 Mpx/s) admits the first and refuses the second.
    ///
    /// That is what the software profiles now say, and this ceiling is only
    /// the backstop for the very small end, where the pixel arithmetic would
    /// otherwise promise several hundred frames a second and the per-frame
    /// costs — parse, entropy decode, reference management, handing the frame
    /// on — become the limit instead.
    pub max_fps_ceiling: u32,
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
        let scaled = (self.max_fps_at_max_size as u64) * max_px / px;
        let budget = scaled.min(self.max_fps_ceiling.max(1) as u64);
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
    /// On battery, apply the caps below before scoring.
    ///
    /// This used to be a field nobody read, with a doc comment claiming it
    /// weighted cost over resolution. It did not; selection ignored it
    /// entirely, so a phone unplugged picked exactly what it picked plugged in.
    pub on_battery: bool,
    /// Longest edge to fetch when on battery.
    ///
    /// Decode cost tracks pixels per second, so this is the biggest lever
    /// there is: 1080p60 down to 720p30 is a 4.5x cut in pixel throughput, and
    /// on a hand-held panel most of that resolution is past what the eye
    /// resolves anyway. 1280 is a defensible default rather than a measured
    /// one — set it to 854 for the 480p policy, or to u32::MAX to disable.
    pub battery_max_long_edge: u32,
    /// Frame rate ceiling when on battery.
    pub battery_max_hz: u32,
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
            battery_max_long_edge: 1280,
            battery_max_hz: 30,
            hdr_capable: false,
        }
    }
}

impl PlaybackBudget {
    /// The display constraints to score against, after the battery caps.
    ///
    /// Returns `(long_edge, short_edge, max_hz)`. Expressed as a clamp on the
    /// *display* rather than as a penalty in the scoring, so it composes with
    /// the existing resolution clamp instead of fighting it: on battery the
    /// player simply behaves as though it had a smaller, slower screen.
    pub fn effective_display(&self) -> (u32, u32, u32) {
        let long = self.display_width.max(self.display_height);
        let short = self.display_width.min(self.display_height);
        if !self.on_battery {
            return (long, short, self.display_max_hz);
        }
        let cap = self.battery_max_long_edge.max(1);
        if long <= cap {
            return (long, short, self.display_max_hz.min(self.battery_max_hz));
        }
        // Scale the short edge with the long one, so the aspect the clamp
        // implies stays the panel's. Scaling only the long edge would let a
        // portrait phone still ask for full-height video.
        let scaled_short = ((short as u64 * cap as u64) / long as u64).max(1) as u32;
        (cap, scaled_short, self.display_max_hz.min(self.battery_max_hz))
    }
}

/// What the GPU can be relied on for.
///
/// Separate from [`Delivery`], which says how the DECODER hands frames over.
/// This says whether there is a GPU worth converting and presenting on, and
/// the two are independent: Asahi M3 decodes AV1 in fixed-function hardware
/// straight into DMA-BUFs and has no usable GPU to import them with.
///
/// That combination is the reason this exists. Before M3 every profile with
/// hardware decode also had a working Vulkan driver, so "has a decoder" and
/// "has a GPU" were the same question and nothing had to ask the second one.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum GpuClass {
    /// No GPU driver at all; everything is CPU.
    None,
    /// A driver exists but is not something to lean on -- llvmpipe, or a
    /// bring-up driver that works and is neither fast nor power-efficient.
    /// Asahi's own words for M3 are "do not expect performant or
    /// power-efficient 3D acceleration right now".
    Unaccelerated,
    /// A real Vulkan driver: convert and present on it.
    Vulkan,
}

impl GpuClass {
    /// Should the colour conversion run on the GPU?
    ///
    /// Only for a real driver. On an unaccelerated one the shader path costs
    /// more than the CPU converter and burns battery doing it -- the upload is
    /// still 1.5 bytes a pixel, but the shader runs on the CPU anyway and the
    /// round trip is added for nothing.
    pub fn worth_converting_on(self) -> bool {
        matches!(self, GpuClass::Vulkan)
    }

    /// Can a decoder's DMA-BUF be imported rather than copied?
    pub fn can_import_dmabuf(self) -> bool {
        matches!(self, GpuClass::Vulkan)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct DeviceCaps {
    /// Free-form, for logs and the "why did it pick that" UI panel.
    pub name: String,
    pub caps: Vec<DecodeCap>,
    pub budget: PlaybackBudget,
    /// What the GPU is good for. See [`GpuClass`] -- hardware decode does not
    /// imply a usable GPU, and on Asahi M3 it explicitly does not.
    pub gpu: GpuClass,
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

    /// Will a decoded frame have to be copied through the CPU to be shown?
    ///
    /// True when the decoder hands back DMA-BUFs and there is no GPU to import
    /// them -- the M3 case. Worth knowing before choosing a stream: a 4K frame
    /// that must be mapped and converted on the CPU every 16ms is a very
    /// different budget from one imported as a texture.
    pub fn frames_need_cpu_copy(&self, p: &VideoProfile, w: u32, h: u32, fps: u32) -> bool {
        match self.best_path(p, w, h, fps) {
            Some(c) => c.delivery == Delivery::DmaBuf && !self.gpu.can_import_dmabuf(),
            None => false,
        }
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
/// The important fact for M1 AND M2: **there is no VP9 or AV1 hardware decode
/// here.** M3 is different and has its own profile — see [`asahi_m3`], where
/// AV1 IS fixed-function and the GPU is the part that is missing instead.
///
/// The original fact, for M1/M2: The AGX
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
        gpu: GpuClass::Vulkan,
        caps: vec![
            DecodeCap {
                codec: Codec::H264,
                max_bit_depth: 8,
                max_chroma: Chroma::Yuv420,
                max_width: 4096,
                max_height: 4096,
                max_fps_at_max_size: 30,
                max_fps_ceiling: 240,
        cost: Cost::Fixed,
                delivery: Delivery::DmaBuf,
                backend: Backend::V4l2Stateless,
            },
            DecodeCap {
                codec: Codec::Av1,
                max_bit_depth: 12,
                max_chroma: Chroma::Yuv444,
                max_width: 1920,
                max_height: 1080,
                max_fps_at_max_size: 30,
                max_fps_ceiling: 120,
        cost: Cost::SoftwareSimd,
                delivery: Delivery::CpuPlanes,
                backend: Backend::Dav1d,
            },
            DecodeCap {
                codec: Codec::Vp9,
                max_bit_depth: 12,
                max_chroma: Chroma::Yuv420,
                max_width: 1920,
                max_height: 1080,
                // ffvp9 is decent but not dav1d; deliberately rated below AV1
                // by giving it the slower Cost, not a smaller fps budget.
                max_fps_at_max_size: 30,
                max_fps_ceiling: 120,
        cost: Cost::SoftwareSlow,
                delivery: Delivery::CpuPlanes,
                backend: Backend::FfmpegSw,
            },
            DecodeCap {
                codec: Codec::Hevc,
                max_bit_depth: 10,
                max_chroma: Chroma::Yuv420,
                max_width: 1920,
                max_height: 1080,
                max_fps_at_max_size: 30,
                max_fps_ceiling: 120,
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
            battery_max_long_edge: 1280,
            battery_max_hz: 30,
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
/// Asahi on an M3-series Mac.
///
/// From the Asahi announcement that merged M3 support into the installer:
/// "hardware accelerated video decoding including support for AV1" works, and
/// separately "the only major exceptions remain full DCP support and the GPU
/// ... Do not expect performant or power-efficient 3D acceleration right now."
///
/// **That inverts the M1/M2 reasoning twice over and is why this is its own
/// profile rather than a tweak to the other one.**
///
/// On M1/M2 the AVD does H.264 and HEVC only, so VP9 and AV1 are dav1d on the
/// big cores, and the model prefers AV1 because dav1d beats ffvp9. Here AV1 is
/// FIXED-FUNCTION: it stops being the best software option and becomes the
/// cheapest option outright, by a margin nothing in software approaches.
///
/// And there is no GPU worth using. Every other hardware-decode profile pairs
/// a decoder with a Vulkan driver, so the frame lands in a DMA-BUF and is
/// imported as a texture. Here it lands in a DMA-BUF and has to be mapped and
/// converted on the CPU, which is a real cost at 4K and the reason
/// `frames_need_cpu_copy` exists.
///
/// *ASSUMED*, and flagged as such: the AV1 decode limits below are a guess.
/// The announcement says AV1 works and does not say to what resolution or
/// frame rate, and nobody has run this against an M3. Treat the numbers as
/// placeholders to be replaced by a runtime probe, not as measurements.
pub fn asahi_m3() -> DeviceCaps {
    let mk = |codec: Codec, depth: u8, w: u32, h: u32, fps: u32| DecodeCap {
        codec,
        max_bit_depth: depth,
        max_chroma: Chroma::Yuv420,
        max_width: w,
        max_height: h,
        max_fps_at_max_size: fps,
        max_fps_ceiling: 240,
        cost: Cost::Fixed,
        delivery: Delivery::DmaBuf,
        backend: Backend::V4l2Stateless,
    };
    DeviceCaps {
        name: "Asahi (M3)".into(),
        gpu: GpuClass::Unaccelerated,
        caps: vec![
            // The headline: AV1 in fixed-function silicon.
            mk(Codec::Av1, 10, 3840, 2160, 60),
            mk(Codec::H264, 8, 3840, 2160, 60),
            mk(Codec::Hevc, 10, 3840, 2160, 60),
            // VP9 is still software here -- the AVD does not do it, and that is
            // the opposite of the Fairphone, where VP9 is the hardware path and
            // AV1 is not.
            DecodeCap {
                codec: Codec::Vp9,
                max_bit_depth: 10,
                max_chroma: Chroma::Yuv420,
                max_width: 1920,
                max_height: 1080,
                max_fps_at_max_size: 30,
                max_fps_ceiling: 120,
        cost: Cost::SoftwareSimd,
                delivery: Delivery::CpuPlanes,
                backend: Backend::Dav1d,
            },
        ],
        budget: PlaybackBudget {
            display_width: 3024,
            display_height: 1964,
            display_max_hz: 120,
            hdr_capable: true,
            max_bitrate: None,
            on_battery: true,
            battery_max_long_edge: 1920,
            // AV1 in hardware makes 60 affordable on battery here, unlike
            // the software-decode profiles that drop to 30.
            battery_max_hz: 60,
        },
    }
}

/// `/dev/video*` M2M decoder, fall back to `fairphone_no_venus()`.
pub fn fairphone_venus() -> DeviceCaps {
    DeviceCaps {
        name: "Fairphone (Qualcomm Venus, postmarketOS)".into(),
        gpu: GpuClass::Vulkan,
        caps: vec![
            DecodeCap {
                codec: Codec::H264,
                max_bit_depth: 8,
                max_chroma: Chroma::Yuv420,
                max_width: 3840,
                max_height: 2160,
                max_fps_at_max_size: 30,
                max_fps_ceiling: 240,
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
                max_fps_ceiling: 240,
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
                max_fps_ceiling: 240,
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
                max_fps_ceiling: 120,
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
            battery_max_long_edge: 1280,
            battery_max_hz: 30,
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
/// Decode profiles from `vainfo`: (codec, max bit depth) for every profile
/// with a VLD (decode) entrypoint. Pure.
///
/// ```text
///       VAProfileH264High               :	VAEntrypointVLD
///       VAProfileHEVCMain10             :	VAEntrypointVLD
///       VAProfileVP9Profile2            :	VAEntrypointVLD
///       VAProfileAV1Profile0            :	VAEntrypointVLD
///       VAProfileH264High               :	VAEntrypointEncSlice   <- encode, ignored
/// ```
pub fn parse_vainfo(text: &str) -> Vec<(Codec, u8)> {
    let mut out: Vec<(Codec, u8)> = Vec::new();
    for line in text.lines() {
        let Some((prof, entry)) = line.split_once(':') else { continue };
        if entry.trim() != "VAEntrypointVLD" {
            continue;
        }
        let prof = prof.trim();
        let found = match prof {
            p if p.starts_with("VAProfileH264") => Some((Codec::H264, 8)),
            "VAProfileHEVCMain" => Some((Codec::Hevc, 8)),
            p if p.starts_with("VAProfileHEVCMain10") => Some((Codec::Hevc, 10)),
            "VAProfileVP9Profile0" => Some((Codec::Vp9, 8)),
            "VAProfileVP9Profile2" => Some((Codec::Vp9, 10)),
            "VAProfileVP8Version0_3" => Some((Codec::Vp8, 8)),
            // AV1 Main (Profile0) is 8- and 10-bit 4:2:0.
            "VAProfileAV1Profile0" => Some((Codec::Av1, 10)),
            _ => None,
        };
        if let Some((c, d)) = found {
            match out.iter_mut().find(|(oc, _)| *oc == c) {
                Some(e) => e.1 = e.1.max(d),
                None => out.push((c, d)),
            }
        }
    }
    out
}

/// Whether the VA driver offers video processing (`VAProfileNone :
/// VAEntrypointVideoProc`) -- what `scale_vaapi` needs. Pure.
///
/// Intel and AMD drivers list it; Asahi's VA-API-over-V4L2-stateless layer
/// (libva-v4l2_request, sofus13's AVD fork) does not: its published vainfo
/// has decode profiles only. Scaling on the VA side there fails outright.
pub fn vainfo_has_video_proc(text: &str) -> bool {
    text.lines().any(|l| {
        let mut p = l.split(':').map(str::trim);
        matches!((p.next(), p.next()), (Some("VAProfileNone"), Some("VAEntrypointVideoProc")))
    })
}

/// What an ffmpeg-based player can actually USE of a device's decoders.
/// Pure.
///
/// Stock ffmpeg cannot drive a V4L2 STATELESS decoder (Asahi's AVD; the
/// Request API) without out-of-tree patches -- Asahi's own August 2026
/// report says so. Such a decoder is usable here only through a VA-API
/// translation layer (libva-v4l2_request), i.e. when `vainfo` lists the
/// codec: then it becomes a VAAPI path. Otherwise it is software, costed as
/// software, so the selector's cost-first order stops choosing streams as
/// if they were hardware-decoded. Stateful V4L2 (Qualcomm Venus/Iris) is
/// untouched: ffmpeg's *_v4l2m2m decoders drive it.
pub fn usable_by_ffmpeg(mut caps: DeviceCaps, vaapi: Option<&[(Codec, u8)]>) -> DeviceCaps {
    for cap in caps.caps.iter_mut().filter(|c| c.backend == Backend::V4l2Stateless) {
        match vaapi.and_then(|v| v.iter().find(|(c, _)| *c == cap.codec)) {
            Some(&(_, depth)) => {
                cap.backend = Backend::Vaapi;
                cap.max_bit_depth = cap.max_bit_depth.min(depth);
            }
            None => {
                cap.cost = Cost::SoftwareSimd;
                cap.delivery = Delivery::CpuPlanes;
                cap.backend = if cap.codec == Codec::Av1 { Backend::Dav1d } else { Backend::FfmpegSw };
            }
        }
    }
    // vainfo is authoritative over the static table: a codec the VA layer
    // lists is hardware here even where the table only knows software.
    // (asahi_m_series predates AVD's HEVC and VP9 support, which Asahi's
    // August 2026 report calls "mostly reliable on all machines", and which
    // the published M2 vainfo lists.)
    for &(codec, depth) in vaapi.unwrap_or(&[]) {
        let has_hw = caps.caps.iter().any(|c| c.codec == codec && c.cost == Cost::Fixed);
        if let (false, Some(tpl)) = (has_hw, caps.caps.iter().find(|c| c.codec == codec).cloned()) {
            caps.caps.push(DecodeCap { cost: Cost::Fixed, backend: Backend::Vaapi, delivery: Delivery::CpuPlanes, max_bit_depth: depth, ..tpl });
        }
    }
    let how = match vaapi {
        Some(v) if !v.is_empty() => "stateless decode via VA-API",
        _ => "no VA-API layer: stateless decode unusable, software",
    };
    caps.name = format!("{} [{how}]", caps.name);
    caps
}

/// Codecs a Vulkan device decodes, from `vulkaninfo` device extensions
/// (`VK_KHR_video_decode_h264` ...). Pure.
///
/// Vulkan Video decode, as of Sept 2026: Mesa RADV and ANV (H.264, H.265,
/// AV1, VP9), NVIDIA's driver and NVK; FFmpeg decodes H.264/H.265/AV1 from
/// 6.1 and VP9 from 8.0. Not on the targets' mobile GPUs: Turnip (Adreno)
/// and Honeykrisp (Asahi) expose none -- their video blocks are separate
/// V4L2 devices. Only the first device's list is taken: vulkaninfo prints
/// one block per GPU and the player decodes on device 0.
pub fn parse_vulkaninfo_video(text: &str) -> Vec<Codec> {
    let mut out = Vec::new();
    let mut seen_device = 0;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("GPU id") || l.starts_with("VkPhysicalDeviceProperties") {
            seen_device += 1;
            if seen_device > 1 && !out.is_empty() {
                break;
            }
        }
        let c = if l.starts_with("VK_KHR_video_decode_h264") {
            Codec::H264
        } else if l.starts_with("VK_KHR_video_decode_h265") {
            Codec::Hevc
        } else if l.starts_with("VK_KHR_video_decode_av1") {
            Codec::Av1
        } else if l.starts_with("VK_KHR_video_decode_vp9") {
            Codec::Vp9
        } else {
            continue;
        };
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// Desktop caps from what the machine's decode APIs report: a codec listed
/// by VAAPI or by Vulkan Video is fixed-function (VAAPI named when both, the
/// more mature path on Intel/AMD); the rest is software. With neither
/// probe available, the old full-hardware assumption. Pure.
pub fn desktop_from_probes(vaapi: Option<&[(Codec, u8)]>, vulkan: Option<&[Codec]>) -> DeviceCaps {
    if vaapi.is_none() && vulkan.is_none() {
        return desktop_full_hw();
    }
    let mut caps = desktop_from_vaapi(vaapi.unwrap_or(&[]));
    for cap in caps.caps.iter_mut() {
        if cap.cost != Cost::Fixed && vulkan.is_some_and(|v| v.contains(&cap.codec)) {
            cap.cost = Cost::Fixed;
            cap.backend = Backend::VulkanVideo;
            cap.delivery = Delivery::CpuPlanes;
            cap.max_width = 7680;
            cap.max_height = 4320;
            cap.max_fps_at_max_size = 60;
            // Vulkan Video profiles carry 10-bit for HEVC/VP9/AV1 on every
            // driver listed above; H.264 is 8-bit everywhere.
            cap.max_bit_depth = if cap.codec == Codec::H264 { 8 } else { 10 };
        }
    }
    if let Some(v) = vulkan.filter(|v| !v.is_empty()) {
        caps.name = format!("{} + Vulkan Video: {}", caps.name, v.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(", "));
    }
    caps
}

/// A desktop/laptop's caps from what its VAAPI driver really decodes.
///
/// `desktop_full_hw` ASSUMES fixed-function AV1, VP9, HEVC and H.264. Most
/// laptops older than Intel's 11th generation (and AMD before RDNA2) have no
/// AV1 decoder, so the picker chose YouTube's AV1 stream, VAAPI init failed
/// inside ffmpeg, and ffmpeg fell back to software decode in the same
/// process -- frames still came, the status still said VAAPI, and the CPU
/// decoded 1080p AV1. Here: what `vainfo` lists is Fixed; every other codec
/// is software (dav1d for AV1, libavcodec otherwise) -- still playable, but
/// the selector's cost-first ordering now prefers the hardware ones.
pub fn desktop_from_vaapi(profiles: &[(Codec, u8)]) -> DeviceCaps {
    let mut caps = desktop_full_hw();
    caps.name = format!(
        "Desktop (VAAPI: {})",
        if profiles.is_empty() { "no decode profiles".to_string() } else { profiles.iter().map(|(c, d)| format!("{c:?} {d}-bit")).collect::<Vec<_>>().join(", ") }
    );
    for cap in caps.caps.iter_mut() {
        match profiles.iter().find(|(c, _)| *c == cap.codec) {
            Some(&(_, depth)) => cap.max_bit_depth = depth,
            None => {
                cap.cost = Cost::SoftwareSimd;
                cap.delivery = Delivery::CpuPlanes;
                cap.backend = if cap.codec == Codec::Av1 { Backend::Dav1d } else { Backend::FfmpegSw };
                // Software 1080p60 is realistic on a laptop; 4K is where the
                // fans start. The picker compares cost first anyway.
                cap.max_width = 3840;
                cap.max_height = 2160;
                cap.max_fps_at_max_size = 30;
            }
        }
    }
    caps
}

pub fn desktop_full_hw() -> DeviceCaps {
    let mk = |codec, depth| DecodeCap {
        codec,
        max_bit_depth: depth,
        max_chroma: Chroma::Yuv420,
        max_width: 7680,
        max_height: 4320,
        max_fps_at_max_size: 60,
        max_fps_ceiling: 240,
        cost: Cost::Fixed,
        delivery: Delivery::DmaBuf,
        backend: Backend::Vaapi,
    };
    DeviceCaps {
        name: "Desktop (VAAPI, full fixed-function)".into(),
        gpu: GpuClass::Vulkan,
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
            battery_max_long_edge: 1280,
            battery_max_hz: 30,
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
        // 1080p30: neither codec is offered at 1080p60 in software any more,
        // and the point of this test is the ORDERING, not the frame rate.
        let av1 = d.best_path(&prof("av01.0.08M.08"), 1920, 1080, 30).unwrap();
        let vp9 = d.best_path(&prof("vp09.00.50.08"), 1920, 1080, 30).unwrap();
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
    fn av1(depth: u8) -> VideoProfile {
        VideoProfile {
            codec: Codec::Av1,
            profile: Some(0),
            bit_depth: depth,
            chroma: Chroma::Yuv420,
            level: None,
        }
    }

    fn vp9(depth: u8) -> VideoProfile {
        VideoProfile {
            codec: Codec::Vp9,
            profile: Some(0),
            bit_depth: depth,
            chroma: Chroma::Yuv420,
            level: None,
        }
    }

    /// The two data points reported from the target hardware, pinned so a
    /// future "optimisation" to the ratings has to argue with them.
    ///
    /// A flat fps ceiling cannot express both -- 90 is above 60 -- so the
    /// budget is pixel rate, and rating software at 1080p30 admits 480p90
    /// (~37 Mpx/s) while refusing 1080p60 (~124 Mpx/s).
    #[test]
    fn software_decode_takes_480p90_and_refuses_1080p60() {
        let d = asahi_m_series();
        let p = av1(8);

        assert!(
            d.can_decode(&p, 854, 480, 90),
            "480p90 was refused; it is fine in performance mode"
        );
        assert!(
            !d.can_decode(&p, 1920, 1080, 60),
            "1080p60 was offered on a software path, where it drops frames"
        );
        // ...and the honest middle: 1080p30 is fine.
        assert!(d.can_decode(&p, 1920, 1080, 30), "1080p30 should be within budget");
    }

    /// The ceiling is the backstop for the very small end, where the pixel
    /// arithmetic alone would promise several hundred frames a second.
    #[test]
    fn a_tiny_frame_does_not_promise_unlimited_frames() {
        let d = asahi_m_series();
        assert!(
            !d.can_decode(&av1(8), 160, 120, 400),
            "400 fps was offered because the pixels were small"
        );
    }

    /// The headline change: on M3 the AVD decodes AV1 in fixed-function
    /// silicon. On M1/M2 the same codec is dav1d on the big cores.
    #[test]
    fn m3_decodes_av1_in_hardware_and_m1_does_not() {
        let m3 = asahi_m3();
        let older = asahi_m_series();
        let p = av1(10);

        let m3_path = m3.best_path(&p, 1920, 1080, 60).expect("m3 must decode av1 in hardware at 60");
        assert_eq!(m3_path.cost, Cost::Fixed, "m3 av1 is not fixed-function");

        // 1080p30, not 60: a software path is no longer rated for 1080p60,
        // because it drops frames there on the real hardware.
        let old_path = older
            .best_path(&p, 1920, 1080, 30)
            .expect("m1 must still decode av1");
        assert!(
            !old_path.cost.is_hardware(),
            "m1/m2 av1 became hardware, which would make this profile wrong"
        );
    }

    /// And the preference inverts against the Fairphone, which is the whole
    /// reason the capability model exists: VP9 is the hardware path on Venus
    /// and the software one here, and AV1 is the reverse.
    #[test]
    fn m3_and_venus_disagree_about_which_codec_is_cheap() {
        let m3 = asahi_m3();
        let fp = fairphone_venus();

        let m3_av1 = m3.best_path(&av1(8), 1920, 1080, 30).unwrap().cost;
        let m3_vp9 = m3.best_path(&vp9(8), 1920, 1080, 30).unwrap().cost;
        assert!(m3_av1 < m3_vp9, "m3 should prefer av1: {m3_av1:?} vs {m3_vp9:?}");

        let fp_vp9 = fp.best_path(&vp9(8), 1920, 1080, 30).unwrap().cost;
        let fp_av1 = fp.best_path(&av1(8), 1920, 1080, 30).map(|c| c.cost);
        assert!(
            fp_av1.map(|c| fp_vp9 < c).unwrap_or(true),
            "the fairphone should prefer vp9: {fp_vp9:?} vs {fp_av1:?}"
        );
    }

    /// The combination nothing in the model could express before: hardware
    /// decode into DMA-BUFs with no GPU to import them, so every frame is
    /// mapped and converted on the CPU.
    #[test]
    fn m3_decodes_in_hardware_and_still_copies_through_the_cpu() {
        let m3 = asahi_m3();
        assert!(!m3.gpu.can_import_dmabuf(), "m3 has no usable GPU");
        assert!(!m3.gpu.worth_converting_on(), "m3 should convert on the CPU");
        assert!(
            m3.frames_need_cpu_copy(&av1(10), 1920, 1080, 60),
            "a DMA-BUF with no importer must be reported as a CPU copy"
        );
    }

    /// ...and the profiles with a real driver must not be dragged along with
    /// it, or the GPU path is switched off everywhere.
    #[test]
    fn a_real_driver_imports_rather_than_copies() {
        for d in [asahi_m_series(), desktop_full_hw(), fairphone_venus()] {
            assert!(
                d.gpu.can_import_dmabuf(),
                "{} lost its GPU path",
                d.name
            );
            assert!(
                !d.frames_need_cpu_copy(&vp9(8), 1280, 720, 30) || d.gpu == GpuClass::Unaccelerated,
                "{} reported a needless CPU copy",
                d.name
            );
        }
    }

    #[test]
    fn among_two_paths_for_one_codec_the_cheaper_wins() {
        let d = DeviceCaps {
            name: "hw + sw fallback for the same codec".into(),
            gpu: GpuClass::Vulkan,
            caps: vec![
                DecodeCap {
                    codec: Codec::Vp9,
                    max_bit_depth: 10,
                    max_chroma: Chroma::Yuv420,
                    max_width: 1920,
                    max_height: 1080,
                    max_fps_at_max_size: 30,
                    max_fps_ceiling: 120,
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
                    max_fps_ceiling: 240,
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
    fn off_battery_the_panel_is_reported_as_it_is() {
        let b = PlaybackBudget {
            display_width: 1080,
            display_height: 2340,
            display_max_hz: 90,
            on_battery: false,
            ..Default::default()
        };
        assert_eq!(b.effective_display(), (2340, 1080, 90));
    }

    /// The lever: decode cost tracks pixels per second, so unplugging a phone
    /// should make it behave like a smaller, slower screen.
    #[test]
    fn on_battery_the_panel_shrinks_and_slows() {
        let b = PlaybackBudget {
            display_width: 1080,
            display_height: 2340,
            display_max_hz: 90,
            on_battery: true,
            battery_max_long_edge: 1280,
            battery_max_hz: 30,
            ..Default::default()
        };
        let (long, short, hz) = b.effective_display();
        assert_eq!(long, 1280);
        assert_eq!(hz, 30);
        // The short edge scales with the long one, so the clamp keeps the
        // panel's aspect. Leaving it alone would let a portrait phone still
        // ask for full-height video.
        assert_eq!(short, 1080 * 1280 / 2340);

        // 2340x1080 at 90 Hz against 1280x590 at 30: about a nine-fold cut in
        // pixels per second, which is the whole point.
        let before = 2340u64 * 1080 * 90;
        let after = long as u64 * short as u64 * hz as u64;
        assert!(before / after >= 8, "only a {}x cut", before / after);
    }

    #[test]
    fn a_panel_already_under_the_cap_is_left_alone_but_still_slowed() {
        let b = PlaybackBudget {
            display_width: 854,
            display_height: 480,
            display_max_hz: 60,
            on_battery: true,
            ..Default::default()
        };
        assert_eq!(b.effective_display(), (854, 480, 30));
    }

    /// The 480p policy the caps exist to express.
    #[test]
    fn the_cap_is_tunable_down_to_480p() {
        let b = PlaybackBudget {
            display_width: 1080,
            display_height: 2340,
            on_battery: true,
            battery_max_long_edge: 854,
            ..Default::default()
        };
        assert_eq!(b.effective_display().0, 854);
    }

    /// ...and disabling it must be possible, or a plugged-in laptop that
    /// happens to report battery gets crippled.
    #[test]
    fn the_cap_can_be_disabled() {
        let b = PlaybackBudget {
            display_width: 3840,
            display_height: 2160,
            display_max_hz: 144,
            on_battery: true,
            battery_max_long_edge: u32::MAX,
            battery_max_hz: u32::MAX,
            ..Default::default()
        };
        assert_eq!(b.effective_display(), (3840, 2160, 144));
    }

    #[test]
    fn a_zero_cap_does_not_divide_by_zero_or_collapse_to_nothing() {
        let b = PlaybackBudget {
            display_width: 1920,
            display_height: 1080,
            on_battery: true,
            battery_max_long_edge: 0,
            ..Default::default()
        };
        let (long, short, _) = b.effective_display();
        assert!(long >= 1 && short >= 1, "collapsed to {long}x{short}");
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

    const VAINFO_KABY_LAKE: &str = "vainfo: Driver version: Intel iHD driver
vainfo: Supported profile and entrypoints
      VAProfileNone                   :	VAEntrypointVideoProc
      VAProfileMPEG2Main              :	VAEntrypointVLD
      VAProfileH264Main               :	VAEntrypointVLD
      VAProfileH264High               :	VAEntrypointVLD
      VAProfileH264High               :	VAEntrypointEncSlice
      VAProfileHEVCMain               :	VAEntrypointVLD
      VAProfileHEVCMain10             :	VAEntrypointVLD
      VAProfileVP9Profile0            :	VAEntrypointVLD
      VAProfileVP9Profile2            :	VAEntrypointVLD
";

    #[test]
    fn vainfo_decode_profiles_are_read_and_encode_ignored() {
        let p = parse_vainfo(VAINFO_KABY_LAKE);
        assert_eq!(p, vec![(Codec::H264, 8), (Codec::Hevc, 10), (Codec::Vp9, 10)]);
        let tgl = format!("{VAINFO_KABY_LAKE}      VAProfileAV1Profile0            :	VAEntrypointVLD\n");
        assert!(parse_vainfo(&tgl).contains(&(Codec::Av1, 10)));
        assert!(parse_vainfo("libva error: vaGetDriverNameByIndex() failed").is_empty());
    }

    /// A laptop without AV1 decode: AV1 stays playable but costs software,
    /// so the selector's cost-first order picks VP9/H.264 over it.
    #[test]
    fn a_laptop_without_av1_hardware_prefers_what_it_can_decode() {
        let caps = desktop_from_vaapi(&parse_vainfo(VAINFO_KABY_LAKE));
        let cost = |c: Codec| caps.caps.iter().find(|x| x.codec == c).map(|x| x.cost);
        assert_eq!(cost(Codec::Av1), Some(Cost::SoftwareSimd));
        assert_eq!(cost(Codec::Vp9), Some(Cost::Fixed));
        assert_eq!(cost(Codec::H264), Some(Cost::Fixed));
        assert!(Cost::Fixed < Cost::SoftwareSimd, "the selector's order relies on this");
        assert!(caps.name.contains("Vp9 10-bit") && !caps.name.contains("Av1"), "{}", caps.name);
    }

    /// Verbatim from nixos-apple-silicon PR #544 (2026-09-09): an M2 MacBook
    /// Air (t8112-j413) with the sofus13 libva-v4l2_request layer. Note the
    /// missing space before the colon on the first profile line, and no
    /// VideoProc entrypoint at all.
    const VAINFO_ASAHI_M2: &str = "Trying display: wayland
libva info: VA-API version 1.24.0
libva info: User environment variable requested driver 'v4l2_request-sofus13'
libva info: Trying to open /run/opengl-driver/lib/dri/v4l2_request-sofus13_drv_video.so
libva info: Found init function __vaDriverInit_1_24
libva-v4l2request: detected 1 Request API decoder
libva info: va_openDriver() returns 0
vainfo: VA-API version: 1.24 (libva 2.24.0)
vainfo: Driver version: v4l2-request
vainfo: Supported profile and entrypoints
      VAProfileH264ConstrainedBaseline: VAEntrypointVLD
      VAProfileH264Main               : VAEntrypointVLD
      VAProfileH264High               : VAEntrypointVLD
      VAProfileHEVCMain               : VAEntrypointVLD
      VAProfileHEVCMain10             : VAEntrypointVLD
      VAProfileVP9Profile0            : VAEntrypointVLD
      VAProfileVP9Profile2            : VAEntrypointVLD
";

    #[test]
    fn the_real_asahi_vainfo_parses_and_has_no_video_processing() {
        let p = parse_vainfo(VAINFO_ASAHI_M2);
        assert_eq!(p, vec![(Codec::H264, 8), (Codec::Hevc, 10), (Codec::Vp9, 10)]);
        assert!(!vainfo_has_video_proc(VAINFO_ASAHI_M2), "no scale_vaapi on Asahi");
        let intel = format!("{VAINFO_KABY_LAKE}      VAProfileNone                   :\tVAEntrypointVideoProc\n");
        assert!(vainfo_has_video_proc(&intel));
    }

    /// Asahi without the VA-API layer: every stateless codec is software for
    /// an ffmpeg player. With it: exactly the listed ones are hardware.
    #[test]
    fn stateless_decoders_count_only_through_va_api() {
        // The cheapest path per codec -- what the selector uses; caps list a
        // hardware path and a software fallback side by side.
        let cost = |c: &DeviceCaps, k: Codec| c.caps.iter().filter(|x| x.codec == k).min_by_key(|x| x.cost).map(|x| (x.cost, x.backend));
        let bare = usable_by_ffmpeg(asahi_m_series(), None);
        assert_eq!(cost(&bare, Codec::H264), Some((Cost::SoftwareSimd, Backend::FfmpegSw)));
        assert!(bare.name.contains("no VA-API layer"));
        let with = usable_by_ffmpeg(asahi_m_series(), Some(&parse_vainfo(VAINFO_ASAHI_M2)));
        assert_eq!(cost(&with, Codec::H264), Some((Cost::Fixed, Backend::Vaapi)));
        assert_eq!(cost(&with, Codec::Vp9), Some((Cost::Fixed, Backend::Vaapi)));
        // Stateful (the FP5's Venus/Iris) is left alone: v4l2m2m drives it.
        let fp5 = usable_by_ffmpeg(fairphone_venus(), None);
        assert_eq!(cost(&fp5, Codec::Vp9), Some((Cost::Fixed, Backend::V4l2Stateful)));
    }

    /// CONSTRUCTED, not captured: the shape of `vulkaninfo`'s device
    /// extension list (names are the Khronos ones) for an NVIDIA GPU with
    /// no VA-API driver. Replace with a real capture when one is to hand.
    const VULKANINFO_SHAPE: &str = "GPU id : 0 (NVIDIA GeForce RTX 4070):
Device Extensions: count = 220
	VK_KHR_video_decode_av1                     : extension revision 1
	VK_KHR_video_decode_h264                    : extension revision 9
	VK_KHR_video_decode_h265                    : extension revision 8
	VK_KHR_video_decode_queue                   : extension revision 8
	VK_KHR_video_decode_vp9                     : extension revision 1
	VK_KHR_video_encode_h264                    : extension revision 14
GPU id : 1 (llvmpipe (LLVM 19.1.7, 256 bits)):
Device Extensions: count = 150
	VK_KHR_swapchain                            : extension revision 70
";

    #[test]
    fn vulkan_video_decoders_are_read_from_device_zero_only() {
        let v = parse_vulkaninfo_video(VULKANINFO_SHAPE);
        assert_eq!(v, vec![Codec::Av1, Codec::H264, Codec::Hevc, Codec::Vp9]);
        assert!(parse_vulkaninfo_video("GPU id : 0 (llvmpipe):\n\tVK_KHR_swapchain : 70\n").is_empty());
    }

    /// The gaming PC: NVIDIA, no VA-API. Vulkan Video makes its codecs
    /// fixed-function; without any probe, the old assumption stands.
    #[test]
    fn an_nvidia_machine_gets_hardware_through_vulkan_video() {
        let caps = desktop_from_probes(None, Some(&parse_vulkaninfo_video(VULKANINFO_SHAPE)));
        let best = |k: Codec| caps.caps.iter().filter(|x| x.codec == k).min_by_key(|x| x.cost).map(|x| (x.cost, x.backend));
        assert_eq!(best(Codec::Av1), Some((Cost::Fixed, Backend::VulkanVideo)));
        assert_eq!(best(Codec::Vp9), Some((Cost::Fixed, Backend::VulkanVideo)));
        // A laptop whose VA driver lacks AV1 but whose Vulkan driver has it:
        // VAAPI for what VA lists, Vulkan fills the gap.
        let mix = desktop_from_probes(Some(&parse_vainfo(VAINFO_KABY_LAKE)), Some(&[Codec::Av1]));
        let b = |k: Codec| mix.caps.iter().filter(|x| x.codec == k).min_by_key(|x| x.cost).map(|x| x.backend);
        assert_eq!(b(Codec::Vp9), Some(Backend::Vaapi));
        assert_eq!(b(Codec::Av1), Some(Backend::VulkanVideo));
        assert_eq!(desktop_from_probes(None, None).name, desktop_full_hw().name);
    }
}
