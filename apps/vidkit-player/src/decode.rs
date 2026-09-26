//! Decoding, by driving ffmpeg.
//!
//! Nothing in this tree decodes video. The demuxers, the player loop, the
//! sync policy, the presenter and the colour conversion are all finished and
//! all fed a test pattern, because a real decoder needs V4L2 hardware or a
//! codec library and neither could be stood up where this was written.
//!
//! ffmpeg can be. This spawns it and reads raw NV12 off its stdout, which
//! makes video actually appear — and is honest about what it costs.
//!
//! # This is a stopgap, and it duplicates work
//!
//! ffmpeg demuxes as well as decodes, so with this in the loop the container
//! parsing in `vidkit-mp4` and `vidkit-webm` is doing nothing for playback; it
//! still describes the file and still drives seeking and the section plan. Two
//! demuxers for one file is not the destination.
//!
//! The destination is [`vidkit_core::traits::Decoder`] over V4L2 — packets in,
//! DMA-BUFs out, hardware. That path exists and has never run. This one exists
//! so the other ninety percent of the player can be seen working, and so the
//! frame queue, the A/V clock and the presenter get exercised by real frame
//! timings rather than a synthetic loop.
//!
//! # Why a pipe and not libavcodec
//!
//! Linking libavcodec means a build that fails wherever the headers are the
//! wrong version, which is most places. A pipe costs a process and some memory
//! bandwidth and works against whatever ffmpeg the machine has. For a stopgap
//! that is the right trade; for the real path it would not be.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use vidkit_core::codec::Transfer;
use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, Matrix, PlaneLayout, Range};

/// What ffprobe says about a video stream.
#[derive(Clone, PartialEq, Debug)]
pub struct Probe {
    pub width: u32,
    pub height: u32,
    /// Frames per second, as a rational reduced to a float.
    pub fps: f64,
    /// ffmpeg's codec name (`h264`, `vp9`, `av1`...). Empty when ffprobe did
    /// not say; hardware selection then declines rather than guessing.
    pub codec: String,
    /// The stream's pixel format (`yuv420p`, `yuv420p10le`, `yuvj420p`...).
    pub pix_fmt: String,
    /// ffprobe's colour tags, verbatim; empty when untagged.
    pub color_space: String,
    pub color_range: String,
    pub color_transfer: String,
    /// The container's start time in seconds (ffmpeg's `-ss` is relative to
    /// it); 0 when unknown.
    pub start_time: f64,
}

impl Probe {
    /// More than 8 bits a sample: decoded to P010, so the precision reaches
    /// the GPU instead of being rounded (and banded) to 8 bits by ffmpeg.
    pub fn high_bit_depth(&self) -> bool {
        ["p10", "p12", "p16", "p010", "p016"].iter().any(|t| self.pix_fmt.contains(t))
    }

    /// What the pixels mean. Pure.
    ///
    /// Frames were tagged BT.709 limited whatever the file said, so BT.601
    /// SD video and full-range phone recordings were converted with the
    /// wrong matrix or range. Untagged: the usual convention -- under 720
    /// lines BT.601, else BT.709 -- and limited, unless the pixel format is
    /// a `yuvj` (JPEG, full-range) one.
    pub fn color(&self) -> ColorInfo {
        let matrix = match self.color_space.as_str() {
            "bt709" => Matrix::Bt709,
            "bt470bg" | "smpte170m" | "bt601" => Matrix::Bt601,
            "bt2020nc" | "bt2020c" | "bt2020_ncl" | "bt2020_cl" => Matrix::Bt2020Ncl,
            _ if self.height >= 720 => Matrix::Bt709,
            _ => Matrix::Bt601,
        };
        let range = match self.color_range.as_str() {
            "pc" | "jpeg" | "full" => Range::Full,
            "tv" | "mpeg" | "limited" => Range::Limited,
            _ if self.pix_fmt.starts_with("yuvj") => Range::Full,
            _ => Range::Limited,
        };
        let transfer = match self.color_transfer.as_str() {
            "smpte2084" => Transfer::Pq,
            "arib-std-b67" => Transfer::Hlg,
            _ => Transfer::Sdr,
        };
        ColorInfo { matrix, transfer, range }
    }

    pub fn layout(&self) -> PlaneLayout {
        if self.high_bit_depth() { PlaneLayout::P010 } else { PlaneLayout::Nv12 }
    }
}

/// The ffprobe arguments shared by the file and URL probes. `codec_name` is
/// asked for because hardware decode is chosen per codec.
const PROBE_ENTRIES: &str = "stream=codec_name,width,height,r_frame_rate,pix_fmt,color_space,color_range,color_transfer:format=start_time";

/// Parse `ffprobe -show_entries stream=...` output.
///
/// Separated from the process call so the parsing is tested without spawning
/// anything — and the rational is the part that goes wrong: `30000/1001` is
/// 29.97 and `30/1` is 30, and a parser that splits on `/` and takes the first
/// number gets 30000 fps.
pub fn parse_probe(text: &str) -> Option<Probe> {
    let mut w = None;
    let mut h = None;
    let mut fps = None;
    let mut codec = String::new();
    let mut start_time = 0.0;
    let (mut pix_fmt, mut color_space, mut color_range, mut color_transfer) = (String::new(), String::new(), String::new(), String::new());
    let tag = |v: &str| {
        let v = v.trim();
        if v == "unknown" || v == "N/A" { String::new() } else { v.to_string() }
    };
    for line in text.lines() {
        let (k, v) = line.split_once('=')?;
        match k.trim() {
            "width" => w = v.trim().parse().ok(),
            "height" => h = v.trim().parse().ok(),
            "codec_name" => codec = v.trim().to_string(),
            "start_time" => start_time = v.trim().parse::<f64>().ok().filter(|t| t.is_finite()).unwrap_or(0.0),
            "pix_fmt" => pix_fmt = tag(v),
            "color_space" => color_space = tag(v),
            "color_range" => color_range = tag(v),
            "color_transfer" => color_transfer = tag(v),
            "r_frame_rate" => {
                let v = v.trim();
                fps = match v.split_once('/') {
                    Some((n, d)) => {
                        let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
                        (d != 0.0).then_some(n / d)
                    }
                    None => v.parse().ok(),
                };
            }
            _ => {}
        }
    }
    Some(Probe {
        width: w?,
        height: h?,
        // A stream with no frame rate is not fatal: still images and some
        // captures report none, and 24 is a better guess than refusing.
        fps: fps.filter(|f| *f > 0.0).unwrap_or(24.0),
        codec,
        pix_fmt,
        color_space,
        color_range,
        color_transfer,
        start_time,
    })
}

/// Ask ffprobe about a file.
pub fn probe(path: &Path) -> Result<Probe, String> {
    probe_input(&path.to_string_lossy(), &[])
}

/// ffprobe a path or URL, with the headers a signed URL is bound to.
pub fn probe_input(input: &str, headers: &[(String, String)]) -> Result<Probe, String> {
    let mut cmd = Command::new("ffprobe");
    cmd.args(["-v", "error"]);
    if !headers.is_empty() {
        cmd.args(["-headers", &header_blob(headers)]);
    }
    cmd.args(["-select_streams", "v:0", "-show_entries", PROBE_ENTRIES, "-of", "default=noprint_wrappers=1"])
        .arg(input);
    let out = cmd.output().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "ffprobe is not installed".to_string(),
        _ => format!("ffprobe: {e}"),
    })?;
    if !out.status.success() {
        return Err(format!(
            "ffprobe {input}: {}",
            String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("failed")
        ));
    }
    parse_probe(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| format!("{input}: no video stream"))
}

/// The content peak in nits from ffprobe's first-frame side data. Pure.
///
/// MaxCLL (`max_content`) first: it is what the content actually reaches.
/// Else the mastering display's `max_luminance` (a rational, e.g.
/// `40000000/10000` = 4000 nits). Zero or absent: `None`, and the EETF uses
/// its 1000-nit default. A peak of 10000 (PQ's ceiling) from a lazy
/// encoder is accepted: over-assuming only compresses highlights more.
pub fn parse_hdr_peak(text: &str) -> Option<f32> {
    let mut cll = None;
    let mut master = None;
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "max_content" => cll = v.trim().parse::<f32>().ok().filter(|n| *n > 0.0),
            "max_luminance" => {
                let v = v.trim();
                master = match v.split_once('/') {
                    Some((n, d)) => match (n.parse::<f32>(), d.parse::<f32>()) {
                        (Ok(n), Ok(d)) if d > 0.0 => Some(n / d),
                        _ => None,
                    },
                    None => v.parse().ok(),
                }
                .filter(|n| *n > 0.0);
            }
            _ => {}
        }
    }
    cll.or(master).map(|n| n.clamp(100.0, 10000.0))
}

/// Read the content peak of a PQ stream: one ffprobe of its first frame.
pub fn probe_hdr_peak(input: &str) -> Option<f32> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-read_intervals", "%+#1", "-show_entries", "frame_side_data", "-of", "default=noprint_wrappers=1"])
        .arg(input)
        .output()
        .ok()?;
    parse_hdr_peak(&String::from_utf8_lossy(&out.stdout))
}

/// The container's duration, if ffprobe knows it.
pub fn probe_duration(input: &str) -> Option<Duration> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(input)
        .output()
        .ok()?;
    let secs: f64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    (secs.is_finite() && secs > 0.0).then(|| Duration::from_secs_f64(secs))
}

/// Kept for callers that probe a URL by name.
pub fn probe_url(url: &str, headers: &[(String, String)]) -> Result<Probe, String> {
    probe_input(url, headers)
}

fn header_blob(headers: &[(String, String)]) -> String {
    headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect()
}

/// The long edge to decode at: the display's, never past the GPU's largest
/// texture, never past the quality preset. Pure.
///
/// This replaced a fixed 1920 in the openers, which threw away everything a
/// 4K, 8K or 12K panel could show -- while `Quality::Auto`/`High` already
/// said "the device's own limit". Unknown display: 1920, the old behaviour,
/// until the first frame reports the monitor. A 12K panel (12288 wide) is
/// within the 16384 textures desktop GPUs offer; some mobile GPUs stop at
/// 8192, and the texture limit is what keeps an upload from failing there.
pub fn cap_for(display_long_px: Option<u32>, max_texture: Option<u32>, quality: Option<u32>) -> u32 {
    let mut c = display_long_px.unwrap_or(1920).max(640);
    if let Some(t) = max_texture.filter(|t| *t > 0) {
        c = c.min(t);
    }
    if let Some(q) = quality {
        c = c.min(q);
    }
    c
}

static DECODE_CAP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1920);

/// The cap the openers read at each open (each seek is one).
pub fn decode_cap() -> u32 {
    DECODE_CAP.load(Ordering::Relaxed)
}

pub fn set_decode_cap(c: u32) {
    DECODE_CAP.store(c, Ordering::Relaxed);
}

/// The output size for a long-edge cap, or `None` when no scaling is needed.
///
/// Computed HERE and passed to ffmpeg as explicit numbers. The streaming path
/// used to hand ffmpeg a `-2` expression and compute the size separately in
/// Rust; the two round differently, and a frame one row shorter than the
/// buffer tears every picture after the first.
/// Whether a fit is worth doing in ffmpeg, or better left to the GPU. Pure.
///
/// Scaling in ffmpeg is a full swscale pass over every source pixel on the
/// CPU; the GPU presenter scales for free as it draws. Measured on 4K60
/// VP9 (a 3840 source on a 3456-wide panel): the 0.9x shrink plus NV12
/// conversion took the player to 1.44x the CPU of decoding alone. A shrink
/// is only worth it when it removes a real share of the pixels (the target
/// under 60% of the source area -- 4K on a 1080p phone) or when the source
/// exceeds the GPU's largest texture, which is a hard limit.
pub fn worth_scaling(src: (u32, u32), fit: Option<(u32, u32)>, max_texture: u32) -> Option<(u32, u32)> {
    let (tw, th) = fit?;
    if src.0.max(src.1) > max_texture {
        return Some((tw, th));
    }
    let ratio = (tw as f64 * th as f64) / (src.0 as f64 * src.1 as f64).max(1.0);
    (ratio < 0.6).then_some((tw, th))
}

/// The size ffmpeg would scale `p` to at the current cap, or `None`.
pub fn scale_plan(p: &Probe) -> Option<(u32, u32)> {
    worth_scaling((p.width, p.height), fit_long_edge(p.width, p.height, Some(decode_cap())), texture_limit())
}

pub fn texture_limit() -> u32 {
    TEXTURE_LIMIT.load(Ordering::Relaxed)
}

static TEXTURE_LIMIT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
static PREFER_PLANAR: AtomicBool = AtomicBool::new(false);

/// GL_MAX_TEXTURE_SIZE, from the app once GL is up.
pub fn set_texture_limit(t: u32) {
    TEXTURE_LIMIT.store(t.max(1), Ordering::Relaxed);
}

/// The presenter takes I420 on the GPU: software decode of 8-bit 4:2:0
/// then asks ffmpeg for its native planar output (no conversion).
pub fn set_prefer_planar(on: bool) {
    PREFER_PLANAR.store(on, Ordering::Relaxed);
}

/// The layout to ask ffmpeg for. Pure. The decoder's native one where the
/// presenter takes it: software 8-bit 4:2:0 -> I420 for the GPU presenter;
/// hardware paths hand NV12 out natively, and the CPU presenter's fast
/// converter wants interleaved chroma.
pub fn output_layout(probe_layout: PlaneLayout, pix_fmt: &str, software: bool, gpu_takes_planar: bool) -> PlaneLayout {
    if software && gpu_takes_planar && probe_layout == PlaneLayout::Nv12 && matches!(pix_fmt, "yuv420p" | "yuvj420p") {
        PlaneLayout::I420
    } else {
        probe_layout
    }
}

pub fn fit_long_edge(w: u32, h: u32, cap: Option<u32>) -> Option<(u32, u32)> {
    let cap = cap?;
    let long = w.max(h);
    if long <= cap || w == 0 || h == 0 {
        return None;
    }
    let scale = cap as f64 / long as f64;
    // Even dimensions: NV12 chroma is subsampled by two, and an odd width
    // makes every row after the first shear.
    let sw = (((w as f64 * scale).round() as u32) & !1).max(2);
    let sh = (((h as f64 * scale).round() as u32) & !1).max(2);
    Some((sw, sh))
}

// ---------------------------------------------------------------------------
// hardware decode selection
// ---------------------------------------------------------------------------

/// How ffmpeg decodes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Accel {
    Software,
    /// VAAPI through a DRM render node — Intel and AMD desktops. Scaling runs
    /// on the GPU too, so a 4K source crosses the bus at the display size.
    /// `vpp`: the driver has video processing, so scale_vaapi can shrink
    /// frames before the download. Asahi's VA layer does not.
    Vaapi { device: String, codec: String, vpp: bool },
    /// Vulkan Video (VK_KHR_video_decode_*): Mesa RADV/ANV, NVK and NVIDIA's
    /// driver. The route NVIDIA has, which has no native VA-API.
    /// `device`: the Vulkan device index -- the integrated GPU when it can.
    Vulkan { codec: String, device: usize },
    /// A V4L2 memory-to-memory decoder (`vp9_v4l2m2m`...) — Venus on the
    /// Fairphone. An INPUT option: it replaces the software decoder.
    V4l2m2m { decoder: String },
}

impl Accel {
    pub fn label(&self) -> String {
        match self {
            Accel::Software => "software".into(),
            Accel::Vaapi { .. } => "VAAPI".into(),
            Accel::Vulkan { .. } => "Vulkan Video".into(),
            Accel::V4l2m2m { decoder } => decoder.clone(),
        }
    }
    pub fn is_hardware(&self) -> bool {
        !matches!(self, Accel::Software)
    }
}

/// What the machine offers, gathered once. Separated from gathering so the
/// selection table is tested with no hardware.
#[derive(Clone, Debug, Default)]
pub struct Machine {
    pub kind: MachineKind,
    /// The bound DRM driver (`amdgpu`, `i915`, `asahi`, `msm`...).
    pub gpu_driver: Option<String>,
    pub render_node: Option<String>,
    /// Every decoder `ffmpeg -decoders` lists.
    pub decoders: Vec<String>,
    /// `ffmpeg -hwaccels`.
    pub hwaccels: Vec<String>,
    /// What the VAAPI driver really decodes, from `vainfo`; `None` when
    /// vainfo is not installed (then the driver name is all there is).
    pub vaapi: Option<Vec<(vidkit_core::codec::Codec, u8)>>,
    /// The VA driver offers video processing (scale_vaapi).
    pub vaapi_vpp: bool,
    /// Codecs some device decodes through Vulkan Video, from `vulkaninfo`.
    pub vulkan: Option<Vec<vidkit_core::codec::Codec>>,
    /// Each Vulkan device's type and decode codecs, to pick the iGPU.
    pub vulkan_devices: Vec<vidkit_core::caps::VkVideoDevice>,
}

/// `vulkaninfo`'s Vulkan Video decode codecs for device 0, once. `None` when
/// vulkaninfo is missing or lists none.
pub fn vulkan_video_codecs() -> Option<Vec<vidkit_core::codec::Codec>> {
    let mut all: Vec<vidkit_core::codec::Codec> = Vec::new();
    for d in vulkan_devices() {
        for c in &d.codecs {
            if !all.contains(c) {
                all.push(*c);
            }
        }
    }
    (!all.is_empty()).then_some(all)
}

/// Every Vulkan device with its type and decode codecs, once.
pub fn vulkan_devices() -> &'static [vidkit_core::caps::VkVideoDevice] {
    static V: OnceLock<Vec<vidkit_core::caps::VkVideoDevice>> = OnceLock::new();
    V.get_or_init(|| {
        Command::new("vulkaninfo")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map(|o| vidkit_core::caps::parse_vulkaninfo_devices(&String::from_utf8_lossy(&o.stdout)))
            .unwrap_or_default()
    })
}

/// ffprobe's codec name as vidkit's codec.
pub fn codec_of(name: &str) -> Option<vidkit_core::codec::Codec> {
    use vidkit_core::codec::Codec;
    Some(match name {
        "h264" => Codec::H264,
        "hevc" => Codec::Hevc,
        "vp8" => Codec::Vp8,
        "vp9" => Codec::Vp9,
        "av1" => Codec::Av1,
        _ => return None,
    })
}

/// `vainfo`'s decode profiles, run once per process. `None` if vainfo is
/// missing or found no driver.
pub fn vaapi_profiles() -> Option<Vec<(vidkit_core::codec::Codec, u8)>> {
    vainfo().as_ref().map(|(p, _)| p.clone())
}

/// `vainfo`, once: (decode profiles, has video processing). The DRM display
/// first, as Asahi's AVD instructions use it: under Wayland, X or headless
/// the default display can be absent or the wrong one. The environment is
/// inherited, so a session's LIBVA_DRIVER_NAME (Asahi's v4l2_request layer)
/// applies.
fn vainfo() -> &'static Option<(Vec<(vidkit_core::codec::Codec, u8)>, bool)> {
    static P: OnceLock<Option<(Vec<(vidkit_core::codec::Codec, u8)>, bool)>> = OnceLock::new();
    P.get_or_init(|| {
        for args in [&["--display", "drm"][..], &[][..]] {
            let Ok(out) = Command::new("vainfo").args(args).stdin(Stdio::null()).stderr(Stdio::null()).output() else { return None };
            let text = String::from_utf8_lossy(&out.stdout);
            let p = vidkit_core::caps::parse_vainfo(&text);
            if !p.is_empty() {
                return Some((p, vidkit_core::caps::vainfo_has_video_proc(&text)));
            }
        }
        None
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum MachineKind {
    /// Apple silicon. The AVD is a STATELESS V4L2 decoder, which mainline
    /// ffmpeg cannot drive, and there is no VAAPI driver for AGX: software.
    Asahi,
    /// Qualcomm Venus: stateful V4L2, which ffmpeg's v4l2m2m decoders drive.
    QualcommVenus,
    #[default]
    Other,
}

/// DRM drivers that ship a VAAPI implementation. `nvidia` is absent on
/// purpose: its VAAPI needs a third-party shim that is rarely installed, and
/// trying it costs a failed spawn per session.
const VAAPI_DRIVERS: &[&str] = &["i915", "xe", "amdgpu", "radeon"];
const VAAPI_CODECS: &[&str] = &["h264", "hevc", "vp8", "vp9", "av1", "mpeg2video"];

/// Whether hardware decode is allowed, from `VIDKIT_HWDEC` (`0`/`off` to
/// refuse). Hardware is the default: every path falls back to software.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AccelPref {
    Auto,
    SoftwareOnly,
}

impl AccelPref {
    pub fn from_env() -> Self {
        match std::env::var("VIDKIT_HWDEC").ok().as_deref().map(str::trim) {
            Some("0") | Some("off") | Some("no") | Some("software") => AccelPref::SoftwareOnly,
            _ => AccelPref::Auto,
        }
    }
}

/// Choose a decoder. Pure.
///
/// V4L2 first: where a machine has a stateful decoder it is the fixed-function
/// block and the cheapest thing on the board. VAAPI only for a driver known to
/// have it. Everything else is software, which is also the answer when the
/// codec is unknown — a guess costs a failed spawn at every open.
pub fn choose_accel(codec: &str, m: &Machine, pref: AccelPref) -> Accel {
    if pref == AccelPref::SoftwareOnly || codec.is_empty() {
        return Accel::Software;
    }
    let m2m = format!("{codec}_v4l2m2m");
    if m.kind == MachineKind::QualcommVenus && m.decoders.iter().any(|d| *d == m2m) {
        return Accel::V4l2m2m { decoder: m2m };
    }
    // Hardware decode goes only through ffmpeg's NATIVE decoder. Fedora's
    // ffmpeg-free has no native h264/hevc (patents; H.264 only as
    // OpenH264, software): asking VAAPI for them fails, so don't.
    if !m.decoders.is_empty() {
        if let Some(c) = codec_of(codec) {
            if !m.decoders.iter().any(|d| d == vidkit_core::caps::native_decoder(c)) {
                return Accel::Software;
            }
        }
    }
    if m.kind == MachineKind::Asahi {
        // AVD is a V4L2 STATELESS decoder, which stock ffmpeg cannot drive.
        // It is reachable only through the VA-API layer (libva-v4l2_request,
        // sofus13's fork), and only for what that layer lists. The DRM node
        // is AGX's; the VA driver is the V4L2 one, so no GPU-driver check.
        let listed = matches!((&m.vaapi, codec_of(codec)), (Some(list), Some(c)) if list.iter().any(|(lc, _)| *lc == c));
        return match (&m.render_node, listed && m.hwaccels.iter().any(|h| h == "vaapi")) {
            (Some(node), true) => Accel::Vaapi { device: node.clone(), codec: codec.to_string(), vpp: m.vaapi_vpp },
            _ => Accel::Software,
        };
    }
    let vaapi_driver = m.gpu_driver.as_deref().is_some_and(|d| VAAPI_DRIVERS.contains(&d));
    if let (true, Some(node)) = (vaapi_driver, &m.render_node) {
        // Only a codec the driver lists. Asked for one it cannot do, ffmpeg
        // falls back to software INSIDE the process: frames arrive, the
        // status says VAAPI, and the CPU decodes -- 1080p AV1 on a laptop
        // without AV1 hardware, which is what heated one.
        let listed = match (&m.vaapi, codec_of(codec)) {
            (Some(list), Some(c)) => list.iter().any(|(lc, _)| *lc == c),
            (Some(_), None) => false,
            (None, _) => true,
        };
        if listed && m.hwaccels.iter().any(|h| h == "vaapi") && VAAPI_CODECS.contains(&codec) {
            return Accel::Vaapi { device: node.clone(), codec: codec.to_string(), vpp: m.vaapi_vpp || m.vaapi.is_none() };
        }
    }
    // Vulkan Video where VAAPI does not cover it: NVIDIA (no native VA-API),
    // and Intel/AMD for a codec their VA driver lacks. ffmpeg must have the
    // vulkan hwaccel and device 0 must list the codec.
    let vk_dev = codec_of(codec).and_then(|c| vidkit_core::caps::pick_vulkan_decoder(&m.vulkan_devices, Some(c)).map(|d| d.index));
    let vk_listed = matches!((&m.vulkan, codec_of(codec)), (Some(list), Some(c)) if list.contains(&c));
    if vk_listed && m.hwaccels.iter().any(|h| h == "vulkan") {
        return Accel::Vulkan { codec: codec.to_string(), device: vk_dev.unwrap_or(0) };
    }
    Accel::Software
}

/// Parse `ffmpeg -decoders`: lines like ` V....D h264_v4l2m2m  V4L2 mem2mem...`.
pub fn parse_decoders(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let flags = it.next()?;
            let name = it.next()?;
            // Flags are six characters of capitals and dots; the legend above
            // the table has "=" in that column and is skipped.
            (flags.len() == 6 && flags.chars().all(|c| c == '.' || c.is_ascii_uppercase())).then(|| name.to_string())
        })
        .collect()
}

/// Parse `ffmpeg -hwaccels`: a header line, then one name per line.
pub fn parse_hwaccels(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.contains(':') && !l.contains(' '))
        .map(str::to_string)
        .collect()
}

/// The machine, probed once per process. Two ffmpeg invocations and a few
/// sysfs reads: cheap, but not something to repeat at every seek.
pub fn machine() -> &'static Machine {
    static M: OnceLock<Machine> = OnceLock::new();
    M.get_or_init(|| {
        let run = |args: &[&str]| {
            Command::new("ffmpeg")
                .args(args)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default()
        };
        let p = vidkit_v4l2::sysprobe::probe(Path::new("/"));
        use vidkit_v4l2::sysprobe::Detected;
        let kind = match p.detected {
            Detected::AsahiM1M2 { .. } | Detected::AsahiM3Plus { .. } => MachineKind::Asahi,
            Detected::QualcommVenus { .. } => MachineKind::QualcommVenus,
            _ => MachineKind::Other,
        };
        // The INTEGRATED GPU's node on a hybrid laptop: decoding on the
        // discrete one keeps a second GPU powered for the whole video.
        let nodes: Vec<vidkit_core::caps::RenderNode> = (128..136)
            .map(|n| format!("renderD{n}"))
            .filter(|n| Path::new("/dev/dri").join(n).exists())
            .map(|n| {
                let dev = Path::new("/sys/class/drm").join(&n).join("device");
                vidkit_core::caps::RenderNode {
                    path: format!("/dev/dri/{n}"),
                    driver: std::fs::read_link(dev.join("driver")).ok().and_then(|p| p.file_name().map(|f| f.to_string_lossy().into_owned())).unwrap_or_default(),
                    vram: std::fs::read_to_string(dev.join("mem_info_vram_total")).ok().and_then(|v| v.trim().parse().ok()),
                }
            })
            .collect();
        let render_node = vidkit_core::caps::pick_render_node(&nodes).map(|n| n.path.clone());
        Machine {
            kind,
            gpu_driver: p.gpu_driver,
            render_node,
            decoders: parse_decoders(&run(&["-hide_banner", "-decoders"])),
            hwaccels: parse_hwaccels(&run(&["-hide_banner", "-hwaccels"])),
            vaapi: vaapi_profiles(),
            vaapi_vpp: vainfo().as_ref().is_some_and(|(_, v)| *v),
            vulkan: vulkan_video_codecs(),
            vulkan_devices: vulkan_devices().to_vec(),
        }
    })
}

/// Set once a hardware decoder has failed to produce a first frame, so every
/// later open (each seek is one) goes straight to software instead of paying
/// for the failed spawn again.
static HARDWARE_FAILED: AtomicBool = AtomicBool::new(false);

/// The decoder for this codec on this machine, honouring an earlier failure.
pub fn accel_for(codec: &str) -> Accel {
    if HARDWARE_FAILED.load(Ordering::Relaxed) {
        return Accel::Software;
    }
    choose_accel(codec, machine(), AccelPref::from_env())
}

// ---------------------------------------------------------------------------
// the video decoder
// ---------------------------------------------------------------------------

/// Where the video comes from. A path and a signed URL differ only in the
/// input options ffmpeg needs before `-i`.
#[derive(Clone, Debug)]
pub struct VideoInput {
    pub input: String,
    /// Sent with every request. Empty for a file.
    pub headers: Vec<(String, String)>,
    pub network: bool,
}

impl VideoInput {
    pub fn file(path: &Path) -> Self {
        Self { input: path.to_string_lossy().into_owned(), headers: Vec::new(), network: false }
    }
    pub fn url(url: &str, headers: &[(String, String)]) -> Self {
        // Reconnect and headers are HTTP protocol options: handed a path or a
        // file:// URL, ffmpeg rejects them and exits before the first frame.
        let network = url.starts_with("http://") || url.starts_with("https://");
        Self { input: url.to_string(), headers: headers.to_vec(), network }
    }
}

/// ffmpeg arguments to decode one video stream to raw NV12 on stdout. Pure.
///
/// The order is what breaks, so every flag's side of `-i` is pinned by a test:
/// `-ss`, `-headers`, `-reconnect`, `-hwaccel` and `-c:v <decoder>` are INPUT
/// options and must precede it. `-map 0:v:0 -an -sn -dn` makes the process a
/// video decoder only: audio has its own process, and letting this one pick
/// an audio stream either decodes it for nothing or — with the rawvideo muxer,
/// which cannot hold audio — refuses to start at all.
pub fn video_args(src: &VideoInput, from: Duration, accel: &Accel, scale_to: Option<(u32, u32)>) -> Vec<String> {
    video_args_fmt(src, from, accel, scale_to, PlaneLayout::Nv12)
}

/// [`video_args`] for an output layout: NV12, or P010 for a 10/12-bit source
/// so its precision reaches the GPU.
pub fn video_args_fmt(src: &VideoInput, from: Duration, accel: &Accel, scale_to: Option<(u32, u32)>, layout: PlaneLayout) -> Vec<String> {
    let (pix, vaapi_fmt) = match layout {
        PlaneLayout::P010 => ("p010le", "p010"),
        PlaneLayout::I420 => ("yuv420p", "nv12"),
        _ => ("nv12", "nv12"),
    };
    let mut a: Vec<String> = vec!["-v".into(), "error".into(), "-nostdin".into()];
    let push = |a: &mut Vec<String>, xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));

    match accel {
        Accel::Software => {}
        Accel::Vaapi { device, codec, vpp } => {
            push(&mut a, &["-hwaccel", "vaapi", "-hwaccel_device", device]);
            // ffmpeg's DEFAULT AV1 decoder is libdav1d (registered first),
            // a software decoder that ignores -hwaccel: without naming the
            // native decoder, "VAAPI" AV1 was dav1d on the CPU even on
            // hardware that decodes AV1. VP9/HEVC/H.264 default to the native
            // decoders already.
            if codec == "av1" {
                push(&mut a, &["-c:v", "av1"]);
            }
            if scale_to.is_some() && *vpp {
                // Keep frames on the GPU so scale_vaapi can shrink them before
                // the download; without this ffmpeg downloads full size.
                push(&mut a, &["-hwaccel_output_format", "vaapi"]);
            }
        }
        Accel::V4l2m2m { decoder } => push(&mut a, &["-c:v", decoder]),
        Accel::Vulkan { codec, device } => {
            // Frames are downloaded to system memory by ffmpeg (no
            // hwaccel_output_format) and scaled in software after. The
            // device is named: ffmpeg's default is device 0, which on a
            // hybrid laptop is often the discrete GPU.
            push(&mut a, &["-init_hw_device", &format!("vulkan=vk:{device}"), "-hwaccel", "vulkan", "-hwaccel_device", "vk"]);
            // Same trap as VAAPI: the default AV1 decoder is libdav1d, which
            // never touches the hardware.
            if codec == "av1" {
                push(&mut a, &["-c:v", "av1"]);
            }
        }
    }
    if src.network {
        if !src.headers.is_empty() {
            a.push("-headers".into());
            a.push(header_blob(&src.headers));
        }
        // Reconnect on a dropped connection. A CDN closing an idle socket
        // mid-film is routine, and without this the film simply stops.
        push(&mut a, &["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "5"]);
    }
    if !from.is_zero() {
        // Before -i: seeks the container to the nearest keyframe. After -i it
        // would decode from the start and discard -- over HTTP, download too.
        a.push("-ss".into());
        a.push(format!("{:.3}", from.as_secs_f64()));
    }
    a.push("-i".into());
    a.push(src.input.clone());
    push(&mut a, &["-map", "0:v:0", "-an", "-sn", "-dn"]);

    if let Some((w, h)) = scale_to {
        let vf = match accel {
            Accel::Vaapi { vpp: true, .. } => format!("scale_vaapi=w={w}:h={h}:format={vaapi_fmt},hwdownload,format={vaapi_fmt}"),
            // Bilinear, not swscale's bicubic default: for a downscale of two
            // or less the difference is invisible at arm's length and it is
            // roughly half the work per pixel, which on a phone core is the
            // difference between keeping up and not.
            _ => format!("scale={w}:{h}:flags=bilinear"),
        };
        a.push("-vf".into());
        a.push(vf);
    }
    push(&mut a, &["-f", "rawvideo", "-pix_fmt", pix, "-"]);
    a
}

/// Reusable plane buffers.
///
/// Every NV12 frame is two allocations of up to 3 MB. glibc serves those with
/// mmap and returns them with munmap, so each frame costs page faults on the
/// way in and a TLB shootdown on the way out; musl — postmarketOS — does the
/// same for anything this size (tideshell hit the identical cost). Recycling
/// makes steady-state playback allocate nothing.
#[derive(Default)]
pub struct PlanePool {
    free: Mutex<Vec<Vec<u8>>>,
    /// Buffers made fresh, and buffers handed out again: the ratio is what
    /// recycling is for. (Whether the pool is non-empty at some instant says
    /// nothing -- a decoder that keeps up takes spares straight back.)
    pub allocated: std::sync::atomic::AtomicU64,
    pub reused: std::sync::atomic::AtomicU64,
}

/// Bytes of spare planes the pool keeps; past this a returned buffer is
/// freed. A count (24) was 72 MB at 1080p and 1.9 GB at 12K; bytes are the
/// same bound at every size. Enough for every frame in flight at 4K.
pub const POOL_BYTES: usize = 384 << 20;

impl PlanePool {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A buffer of exactly `len` bytes. Contents are unspecified: the caller
    /// overwrites all of it with read_exact.
    pub fn take(&self, len: usize) -> Vec<u8> {
        let found = self.free.lock().ok().and_then(|mut f| {
            let i = f.iter().position(|v| v.capacity() >= len)?;
            Some(f.swap_remove(i))
        });
        match found {
            Some(mut v) => {
                self.reused.fetch_add(1, Ordering::Relaxed);
                v.resize(len, 0);
                v
            }
            None => {
                self.allocated.fetch_add(1, Ordering::Relaxed);
                vec![0u8; len]
            }
        }
    }

    pub fn put(&self, v: Vec<u8>) {
        if let Ok(mut f) = self.free.lock() {
            let held: usize = f.iter().map(|b| b.capacity()).sum();
            if held + v.capacity() <= POOL_BYTES {
                f.push(v);
            }
        }
    }

    /// Bytes held for reuse.
    pub fn bytes(&self) -> usize {
        self.free.lock().map(|f| f.iter().map(|b| b.capacity()).sum()).unwrap_or(0)
    }

    /// Return a frame's planes.
    pub fn recycle(&self, frame: Frame) {
        if let FrameData::Cpu(p) = frame.data {
            for v in p.data {
                self.put(v);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.free.lock().map(|f| f.len()).unwrap_or(0)
    }
}

/// What the engine's video thread pulls frames from: the ffmpeg child
/// process ([`FfmpegFrames`]) or an in-process decoder.
pub trait VideoSource: Send {
    fn next_frame(&mut self) -> Option<Frame>;
    fn frame_interval(&self) -> Duration;
    fn hdr_peak(&self) -> Option<f32>;
    /// Share the consumer's plane pool.
    fn set_pool(&mut self, pool: Arc<PlanePool>);
    /// "software 1920x1080", "VAAPI zero-copy 3840x2160"... for the status.
    fn label(&self) -> String;
    /// Bytes of one CPU frame (queue sizing); a dma-buf source still says
    /// what the frame would weigh, since the pool of surfaces is what bounds
    /// its queue.
    fn frame_bytes(&self) -> usize;
    /// A one-off note (a fallback happened), taken once.
    fn take_note(&mut self) -> Option<String>;
}

static FPS_CAP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The frame rate to show at most (the viewer's cap, or the battery
/// policy's); `None`: every frame. Read per frame by [`RateCapped`], and at
/// open by decoders that can skip work for it.
pub fn set_fps_cap(cap: Option<u32>) {
    FPS_CAP.store(cap.unwrap_or(0), Ordering::Relaxed);
}

pub fn fps_cap() -> Option<u32> {
    Some(FPS_CAP.load(Ordering::Relaxed)).filter(|&c| c > 0)
}

/// Whether a frame at `pts` is shown under `cap`, the last shown being at
/// `last`. A tenth of an interval of slack, so 60 -> 30 keeps every other
/// frame rather than drifting to every third on timestamp rounding. Pure.
pub fn keep_frame(pts: Duration, last: Option<Duration>, cap: Option<u32>) -> bool {
    match (cap, last) {
        (Some(c), Some(l)) if pts > l => (pts - l).as_secs_f64() >= 0.9 / c as f64,
        _ => true,
    }
}

/// Frames above the cap never reach the queue: no upload, no conversion,
/// no composite, no wakeup for them. (Decoding them is the decoder's
/// business: in process, AV1 skips non-reference frames outright.)
pub struct RateCapped {
    inner: Box<dyn VideoSource>,
    pool: Option<Arc<PlanePool>>,
    last: Option<Duration>,
    pub skipped: u64,
}

impl RateCapped {
    pub fn new(inner: Box<dyn VideoSource>) -> Self {
        Self { inner, pool: None, last: None, skipped: 0 }
    }
}

impl VideoSource for RateCapped {
    fn next_frame(&mut self) -> Option<Frame> {
        loop {
            let f = self.inner.next_frame()?;
            // A seek or a loop goes backwards: start over.
            if self.last.is_some_and(|l| f.pts < l) {
                self.last = None;
            }
            if keep_frame(f.pts, self.last, fps_cap()) {
                self.last = Some(f.pts);
                return Some(f);
            }
            self.skipped += 1;
            match &self.pool {
                Some(p) => p.recycle(f),
                None => drop(f),
            }
        }
    }
    fn frame_interval(&self) -> Duration {
        let i = self.inner.frame_interval();
        match fps_cap() {
            Some(c) => i.max(Duration::from_secs_f64(1.0 / c as f64)),
            None => i,
        }
    }
    fn hdr_peak(&self) -> Option<f32> {
        self.inner.hdr_peak()
    }
    fn set_pool(&mut self, pool: Arc<PlanePool>) {
        self.pool = Some(pool.clone());
        self.inner.set_pool(pool)
    }
    fn label(&self) -> String {
        match fps_cap() {
            Some(c) => format!("{} @≤{c} fps", self.inner.label()),
            None => self.inner.label(),
        }
    }
    fn frame_bytes(&self) -> usize {
        self.inner.frame_bytes()
    }
    fn take_note(&mut self) -> Option<String> {
        self.inner.take_note()
    }
}

/// A source that hands over to another if it ends before its first frame:
/// a hardware path that opened but cannot decode this stream (a pixel
/// format it will not give, a profile it refuses) falls back instead of
/// playing nothing. The fallback is opened only when needed.
pub struct FallbackSource {
    current: Box<dyn VideoSource>,
    fallback: Option<Box<dyn FnOnce() -> Option<Box<dyn VideoSource>> + Send>>,
    got_frame: bool,
    note: Option<String>,
}

impl FallbackSource {
    pub fn new(primary: Box<dyn VideoSource>, fallback: Box<dyn FnOnce() -> Option<Box<dyn VideoSource>> + Send>) -> Self {
        Self { current: primary, fallback: Some(fallback), got_frame: false, note: None }
    }
}

impl VideoSource for FallbackSource {
    fn next_frame(&mut self) -> Option<Frame> {
        loop {
            if let Some(f) = self.current.next_frame() {
                self.got_frame = true;
                return Some(f);
            }
            if self.got_frame {
                return None;
            }
            let fb = self.fallback.take()?;
            let why = self.current.take_note().unwrap_or_else(|| "no frames".into());
            let was = self.current.label();
            self.current = fb()?;
            self.note = Some(format!("{was} failed ({why}); now {}", self.current.label()));
        }
    }
    fn frame_interval(&self) -> Duration {
        self.current.frame_interval()
    }
    fn hdr_peak(&self) -> Option<f32> {
        self.current.hdr_peak()
    }
    fn set_pool(&mut self, pool: Arc<PlanePool>) {
        self.current.set_pool(pool)
    }
    fn label(&self) -> String {
        self.current.label()
    }
    fn frame_bytes(&self) -> usize {
        self.current.frame_bytes()
    }
    fn take_note(&mut self) -> Option<String> {
        self.note.take().or_else(|| self.current.take_note())
    }
}

impl VideoSource for FfmpegFrames {
    fn next_frame(&mut self) -> Option<Frame> {
        FfmpegFrames::next_frame(self)
    }
    fn frame_interval(&self) -> Duration {
        self.frame_interval
    }
    fn hdr_peak(&self) -> Option<f32> {
        self.hdr_peak
    }
    fn set_pool(&mut self, pool: Arc<PlanePool>) {
        FfmpegFrames::set_pool(self, pool)
    }
    fn label(&self) -> String {
        format!("{} {}x{}", self.accel.label(), self.width, self.height)
    }
    fn frame_bytes(&self) -> usize {
        FfmpegFrames::frame_bytes(self)
    }
    fn take_note(&mut self) -> Option<String> {
        self.note.take()
    }
}

/// A running ffmpeg, yielding NV12 frames.
pub struct FfmpegFrames {
    child: Child,
    pub width: u32,
    pub height: u32,
    pub frame_interval: Duration,
    /// NV12, or P010 for a high-bit-depth source.
    pub layout: PlaneLayout,
    /// What the pixels mean, from the stream's tags.
    pub color: ColorInfo,
    /// The content's peak in nits, for PQ streams that say.
    pub hdr_peak: Option<f32>,
    index: u64,
    pub frames_read: u64,
    /// What is decoding NOW (after any fallback), for the status line.
    pub accel: Accel,
    /// Software arguments to retry with if the hardware decoder produces
    /// nothing. Taken on first use; `None` once software is running.
    fallback: Option<Vec<String>>,
    /// Why a fallback happened, once, for the UI.
    pub note: Option<String>,
    pool: Arc<PlanePool>,
    /// AV1 film grain parameters for the frames, when the decoder was told
    /// to export grain instead of applying it (see [`grain_on_gpu`]).
    grain: Option<GrainFeed>,
}

/// Grow a pipe's kernel buffer. The default is 64 KiB: a 4K NV12 frame
/// (12 MB) crosses it in ~190 wakeups of each side, and a 12K frame (119 MB)
/// in ~1800. A bigger buffer means fewer context switches per frame; the
/// cap is /proc/sys/fs/pipe-max-size (1 MiB by default), and asking for more
/// than that fails, so this asks for at most that. Returns the size set.
pub fn grow_pipe(fd: std::os::fd::RawFd, want: usize) -> Option<usize> {
    let max = std::fs::read_to_string("/proc/sys/fs/pipe-max-size").ok().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1 << 20);
    let size = want.min(max) as libc::c_int;
    // SAFETY: fcntl on an fd we own, with an integer argument.
    let got = unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, size) };
    (got > 0).then_some(got as usize)
}

/// The pipe size for frames of `frame_bytes`: a frame's worth, capped.
pub const PIPE_WANT: usize = 1 << 20;

/// The efficiency cores of a big.LITTLE machine: the CPUs in the LOWEST
/// `cpu_capacity` tier, when capacities differ at all. Pure.
///
/// Measured on a 720p SVT-AV1 stream (rav1d, callgrind): 73% of AV1 decode
/// is entropy decoding, serial and CPU-only, so a GPU decoder could take at
/// most ~a quarter off the CPU. On an M2 (no AV1 block) the lever is the
/// other way round: run the software decoder on the cores that do a unit of
/// work for the least energy. M2 Max: 4 efficiency + 8 performance cores;
/// Fairphone 5: A55 / A78 / X1, three tiers -- the A55s are the lowest.
/// A symmetric machine returns nothing: nothing to pin to.
pub fn efficiency_cpus(capacities: &[(usize, u32)]) -> Vec<usize> {
    let (Some(min), Some(max)) = (capacities.iter().map(|c| c.1).min(), capacities.iter().map(|c| c.1).max()) else { return Vec::new() };
    if min == max {
        return Vec::new();
    }
    capacities.iter().filter(|c| c.1 == min).map(|c| c.0).collect()
}

/// `/sys/devices/system/cpu/cpu*/cpu_capacity`.
pub fn cpu_capacities() -> Vec<(usize, u32)> {
    let Ok(rd) = std::fs::read_dir("/sys/devices/system/cpu") else { return Vec::new() };
    let mut v: Vec<(usize, u32)> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().strip_prefix("cpu")?.parse::<usize>().ok()?;
            let c = std::fs::read_to_string(e.path().join("cpu_capacity")).ok()?.trim().parse().ok()?;
            Some((n, c))
        })
        .collect();
    v.sort();
    v
}

static DECODE_CPUS: Mutex<Option<Vec<usize>>> = Mutex::new(None);

/// Pin every ffmpeg the player starts from now on to these CPUs (`None`:
/// anywhere). Takes effect at the next (re)open.
pub fn set_decode_cpus(cpus: Option<Vec<usize>>) {
    if let Ok(mut c) = DECODE_CPUS.lock() {
        *c = cpus.filter(|v| !v.is_empty());
    }
}

pub fn decode_cpus() -> Option<Vec<usize>> {
    DECODE_CPUS.lock().ok().and_then(|c| c.clone())
}

/// Restrict a child to `cpus` before it execs, and size ffmpeg's threads to
/// match (more threads than cores only adds switching).
fn pin(cmd: &mut Command, cpus: &[usize]) {
    use std::os::unix::process::CommandExt;
    let cpus = cpus.to_vec();
    // SAFETY: sched_setaffinity on the calling (child) thread between fork
    // and exec touches no locks or allocations beyond the stack set.
    unsafe {
        cmd.pre_exec(move || {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            for &c in &cpus {
                libc::CPU_SET(c, &mut set);
            }
            if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

fn spawn_ffmpeg(args: &[String]) -> Result<Child, String> {
    let mut cmd = Command::new("ffmpeg");
    let pinned = decode_cpus();
    if let Some(cpus) = &pinned {
        pin(&mut cmd, cpus);
        cmd.args(["-threads", &cpus.len().to_string()]);
    }
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Inherited stderr would scribble over the UI; ffmpeg is chatty even
        // at -v error.
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "ffmpeg is not installed".to_string(),
            _ => format!("ffmpeg: {e}"),
        })
        .map(|c| {
            // Measured (pipe_bench, 5 alternating pairs, 240 4K frames each):
            // median 1.62 GB/s default vs 1.66 GB/s grown, ranges overlapping
            // -- NO measurable difference on the one-core sandbox, where
            // both processes share a CPU and switch regardless. A first
            // single pair showed +42%; it was noise. Kept because it cannot
            // hurt and the per-frame wakeups it removes are real on a
            // multi-core machine. Unmeasured there: run the bench.
            if let Some(out) = c.stdout.as_ref() {
                use std::os::fd::AsRawFd;
                let _ = grow_pipe(out.as_raw_fd(), PIPE_WANT);
            }
            c
        })
}

impl FfmpegFrames {
    /// Decode `path` to NV12, optionally scaled down. Probes the file itself.
    /// (Test convenience: the app opens with open_probed, probing once.)
    #[cfg(test)]
    pub fn open(path: &Path, max_long_edge: Option<u32>) -> Result<Self, String> {
        Self::open_at(path, max_long_edge, Duration::ZERO)
    }

    /// [`open`](Self::open) from `from`, probing again. Seeks in the app go
    /// through [`open_probed`](Self::open_probed) instead, which does not.
    pub fn open_at(path: &Path, max_long_edge: Option<u32>, from: Duration) -> Result<Self, String> {
        let p = probe(path)?;
        let accel = accel_for(&p.codec);
        Self::open_probed(&VideoInput::file(path), &p, max_long_edge, from, accel)
    }

    /// Start decoding with facts already known.
    ///
    /// The probe is the caller's so that a seek — a reopen — does not spawn
    /// ffprobe each time: that was a process and a container parse per scrub,
    /// and over HTTP a round trip.
    ///
    /// `max_long_edge` exists because decoding 1440p in software to throw most
    /// of it away is the expensive mistake the capability model is there to
    /// avoid; letting ffmpeg scale costs far less than scaling afterwards.
    pub fn open_probed(
        src: &VideoInput,
        p: &Probe,
        max_long_edge: Option<u32>,
        from: Duration,
        accel: Accel,
    ) -> Result<Self, String> {
        let scale_to = worth_scaling((p.width, p.height), fit_long_edge(p.width, p.height, max_long_edge), TEXTURE_LIMIT.load(Ordering::Relaxed));
        let (w, h) = scale_to.unwrap_or((p.width, p.height));
        let layout = output_layout(p.layout(), &p.pix_fmt, !accel.is_hardware(), PREFER_PLANAR.load(Ordering::Relaxed));
        let mut args = video_args_fmt(src, from, &accel, scale_to, layout);
        let fallback = accel.is_hardware().then(|| video_args_fmt(src, from, &Accel::Software, scale_to, layout));
        // Grain on the GPU: the feed starts first (it only demuxes), and the
        // decoder is told to export grain only once the feed is running --
        // otherwise a failed feed would show the film with no grain at all.
        let grain = grain_on_gpu(p, src, &accel, scale_to, layout)
            .then(|| GrainFeed::spawn(src, from, p.start_time))
            .and_then(|r| r.map_err(|e| eprintln!("vidkit: GPU film grain off: {e}")).ok());
        if grain.is_some() {
            args = with_grain_export(args);
        }
        let child = spawn_ffmpeg(&args)?;
        Ok(Self {
            child,
            width: w,
            height: h,
            frame_interval: Duration::from_secs_f64(1.0 / p.fps.max(1.0)),
            layout,
            color: p.color(),
            hdr_peak: (p.color().transfer == Transfer::Pq).then(|| probe_hdr_peak(&src.input)).flatten(),
            // Timestamps on the MEDIA timeline from the first frame: opened at
            // 2 s, the first frame is stamped 2 s. Starting at zero stamped a
            // seeked decoder's frames 0 s, 0.033 s... against a clock at 2 s,
            // so a clock-driven player dropped every one as late.
            index: (from.as_secs_f64() * p.fps.max(1.0)).round() as u64,
            frames_read: 0,
            accel,
            fallback,
            note: None,
            pool: PlanePool::new(),
            grain,
        })
    }

    pub fn bytes_per_sample(&self) -> usize {
        if self.layout == PlaneLayout::P010 { 2 } else { 1 }
    }

    /// Bytes in one frame of this decoder's output.
    pub fn frame_bytes(&self) -> usize {
        frame_bytes(self.width, self.height) * self.bytes_per_sample()
    }

    /// Share a pool with the consumer, which returns planes to it.
    pub fn set_pool(&mut self, pool: Arc<PlanePool>) {
        self.pool = pool;
    }

    /// Read one frame into pooled planes. `false` at end of stream.
    fn read_planes(&mut self, luma: &mut [u8], chroma: &mut [u8]) -> bool {
        let Some(stdout) = self.child.stdout.as_mut() else { return false };
        // read_exact, not read: a pipe hands over whatever is ready, so a
        // single read returns a partial frame and the picture tears diagonally
        // — the classic symptom of treating a pipe like a file. Straight into
        // the planes: the old path read into a scratch buffer and copied both
        // planes out, touching every byte twice.
        stdout.read_exact(luma).is_ok() && stdout.read_exact(chroma).is_ok()
    }

    /// The next frame, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Option<Frame> {
        let (w, h) = (self.width as usize, self.height as usize);
        let bps = self.bytes_per_sample();
        let (luma_len, chroma_len) = (w * h * bps, self.frame_bytes() - w * h * bps);
        loop {
            if self.layout == PlaneLayout::I420 {
                // Three planes, straight from the pipe into each -- no
                // interleave, no copy.
                let cw = (self.width as usize).div_ceil(2);
                let c_len = cw * (self.height as usize).div_ceil(2);
                let (mut y, mut u, mut v) = (self.pool.take(luma_len), self.pool.take(c_len), self.pool.take(c_len));
                let ok = match self.child.stdout.as_mut() {
                    Some(o) => o.read_exact(&mut y).is_ok() && o.read_exact(&mut u).is_ok() && o.read_exact(&mut v).is_ok(),
                    None => false,
                };
                if ok {
                    let pts = self.frame_interval.mul_f64(self.index as f64);
                    self.index += 1;
                    self.frames_read += 1;
                    let grain = self.grain_for(pts);
                    return Some(Frame {
                        data: FrameData::Cpu(CpuPlanes { data: vec![y, u, v], strides: vec![self.width, cw as u32, cw as u32] }),
                        width: self.width,
                        height: self.height,
                        layout: PlaneLayout::I420,
                        color: self.color,
                        pts,
                        duration: Some(self.frame_interval),
                        recycle: None,
                        grain,
                    });
                }
                self.pool.put(y);
                self.pool.put(u);
                self.pool.put(v);
                return None;
            }
            let mut luma = self.pool.take(luma_len);
            let mut chroma = self.pool.take(chroma_len);
            if self.read_planes(&mut luma, &mut chroma) {
                let pts = self.frame_interval.mul_f64(self.index as f64);
                self.index += 1;
                self.frames_read += 1;
                let grain = self.grain_for(pts);
                return Some(Frame {
                    // Strides in BYTES: two per sample for P010.
                    data: FrameData::Cpu(CpuPlanes { data: vec![luma, chroma], strides: vec![self.width * bps as u32; 2] }),
                    width: self.width,
                    height: self.height,
                    layout: self.layout,
                    color: self.color,
                    pts,
                    duration: Some(self.frame_interval),
                    recycle: None,
                    grain,
                });
            }
            self.pool.put(luma);
            self.pool.put(chroma);
            // A hardware decoder that produced NOTHING failed to start: no
            // device, no driver, an unsupported profile. Retry in software
            // once, and remember, so the next seek does not pay again. After
            // a first frame an end is a real end.
            if self.frames_read == 0 {
                if let Some(args) = self.fallback.take() {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    HARDWARE_FAILED.store(true, Ordering::Relaxed);
                    self.note = Some(format!("{} produced no frames; decoding in software", self.accel.label()));
                    self.accel = Accel::Software;
                    match spawn_ffmpeg(&args) {
                        Ok(c) => {
                            self.child = c;
                            continue;
                        }
                        Err(_) => return None,
                    }
                }
            }
            return None;
        }
    }
}

static GPU_GRAIN: AtomicBool = AtomicBool::new(false);
static GPU_PRESENTER: AtomicBool = AtomicBool::new(false);

/// Whether frames go to the GL presenter (which imports dma-bufs). Set by
/// the app at startup and cleared if the GPU path fails.
pub fn set_gpu_presenter(on: bool) {
    GPU_PRESENTER.store(on, Ordering::Relaxed);
}

pub fn gpu_presenter() -> bool {
    GPU_PRESENTER.load(Ordering::Relaxed)
}

/// Whether the presenter synthesises AV1 film grain itself (the GPU
/// presenter does; the CPU one does not). Takes effect at the next open.
pub fn set_gpu_grain(on: bool) {
    GPU_GRAIN.store(on, Ordering::Relaxed);
}

/// Whether to have the decoder EXPORT film grain instead of applying it,
/// so the GPU synthesises it at present time. Pure but for the global and
/// `VIDKIT_GRAIN_CPU=1` (keep dav1d's grain, for A/B).
///
/// Only where the GPU stage is exact: AV1, 8-bit NV12/I420, decoded in
/// software at source size (grain is defined on the source grid -- on a
/// frame ffmpeg scaled, 32x32 grain blocks would land on the wrong
/// pixels), from a local file (the feed demuxes the input a second time;
/// for a stream that would be a second download).
pub fn grain_on_gpu(p: &Probe, src: &VideoInput, accel: &Accel, scale_to: Option<(u32, u32)>, layout: PlaneLayout) -> bool {
    GPU_GRAIN.load(Ordering::Relaxed)
        && std::env::var("VIDKIT_GRAIN_CPU").as_deref() != Ok("1")
        && grain_eligible(p, src, accel, scale_to, layout)
}

/// [`grain_on_gpu`] without the switches. Pure.
pub fn grain_eligible(p: &Probe, src: &VideoInput, accel: &Accel, scale_to: Option<(u32, u32)>, layout: PlaneLayout) -> bool {
    p.codec == "av1" && !src.network && !accel.is_hardware() && scale_to.is_none() && matches!(layout, PlaneLayout::Nv12 | PlaneLayout::I420)
}

/// Decoder arguments with grain exported, not applied: an input (decoder)
/// option, so before `-i`. libdav1d and ffmpeg's native AV1 decoder both
/// honour it. Pure.
pub fn with_grain_export(mut a: Vec<String>) -> Vec<String> {
    let i = a.iter().position(|x| x == "-i").unwrap_or(a.len());
    a.splice(i..i, ["-export_side_data".to_string(), "film_grain".to_string()]);
    a
}

/// ffmpeg arguments for the grain feed: the video stream copied (demux
/// only, no decode) as IVF, from the same seek point as the decoder, with
/// the file's own timestamps (`-copyts`) so its units can be matched to
/// decoded frames by time. Pure.
pub fn grain_feed_args(src: &VideoInput, from: Duration) -> Vec<String> {
    let mut a: Vec<String> = ["-v", "error", "-nostdin"].iter().map(|s| s.to_string()).collect();
    if !from.is_zero() {
        a.push("-ss".into());
        a.push(format!("{:.3}", from.as_secs_f64()));
    }
    a.push("-i".into());
    a.push(src.input.clone());
    a.extend(["-map", "0:v:0", "-c", "copy", "-copyts", "-f", "ivf", "-"].iter().map(|s| s.to_string()));
    a
}

/// Film grain parameters per shown frame, read from the bitstream by a
/// second ffmpeg that only demuxes. The decoder's pixel pipe cannot carry
/// side data, so the parameters come from vacc's AV1 header parser
/// (byte-exact against dav1d: vidkit-vacc's grain oracle test).
struct GrainFeed {
    child: Child,
    units: GrainUnits,
}

type GrainUnit = (f64, Option<Arc<vidkit_core::av1_grain::GrainParams>>);

/// The feed's units, matched to decoded frames by time.
struct GrainUnits {
    /// (media time of the frame in seconds, its grain), in display order.
    rx: std::sync::mpsc::Receiver<GrainUnit>,
    /// A unit read ahead of the frame being matched.
    pending: Option<GrainUnit>,
    misses: u64,
    ended: bool,
}

impl GrainFeed {
    fn spawn(src: &VideoInput, from: Duration, start_time: f64) -> Result<Self, String> {
        let mut child = Command::new("ffmpeg")
            .args(grain_feed_args(src, from))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("grain feed: {e}"))?;
        let out = child.stdout.take().ok_or("grain feed: no stdout")?;
        // Bounded: the demuxer is far faster than the decoder, and the pipe
        // filling up is what holds it back rather than a film's worth of
        // parameters in memory.
        let (tx, rx) = std::sync::mpsc::sync_channel(64);
        std::thread::Builder::new()
            .name("vidkit-grain".into())
            .spawn(move || {
                let Ok(mut rd) = vidkit_vacc::grain::IvfReader::new(std::io::BufReader::with_capacity(1 << 16, out)) else { return };
                let mut st = vidkit_vacc::grain::GrainStream::default();
                let mut buf = Vec::new();
                while let Some(t) = rd.next_unit(&mut buf) {
                    match st.push_tu(&buf) {
                        Ok(Some(p)) => {
                            if tx.send((t - start_time, p.map(Arc::new))).is_err() {
                                return;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            eprintln!("vidkit: grain feed stopped: {e}");
                            return;
                        }
                    }
                    if st.present == Some(false) {
                        return; // no grain in this stream: nothing to feed
                    }
                }
            })
            .map_err(|e| format!("grain feed thread: {e}"))?;
        Ok(Self { child, units: GrainUnits::new(rx) })
    }

}

impl GrainUnits {
    fn new(rx: std::sync::mpsc::Receiver<GrainUnit>) -> Self {
        Self { rx, pending: None, misses: 0, ended: false }
    }

    /// The grain for the frame at media time `t` (`half`: half a frame
    /// interval). Units before the frame are skipped (the demuxer starts at
    /// the keyframe before the seek point; the decoder drops those frames);
    /// a unit after it is kept for a later frame. Waits a little for a unit
    /// that has not arrived: the demuxer normally runs far ahead.
    fn take(&mut self, t: f64, half: f64) -> Option<Arc<vidkit_core::av1_grain::GrainParams>> {
        loop {
            let item = match self.pending.take() {
                Some(i) => i,
                None if self.ended => return None,
                None => match self.rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(i) => i,
                    // Ended (no grain in the stream, the end of the file, a
                    // parse error): stop asking, or every later frame waits.
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        self.ended = true;
                        return None;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        self.misses += 1;
                        return None;
                    }
                },
            };
            match match_unit(item.0, t, half) {
                std::cmp::Ordering::Less => continue,
                std::cmp::Ordering::Equal => return item.1,
                std::cmp::Ordering::Greater => {
                    self.pending = Some(item);
                    self.misses += 1;
                    return None;
                }
            }
        }
    }
}

/// Where a feed unit at `unit` seconds sits against a frame at `frame`:
/// before it, on it (within half an interval), or after it. Pure.
pub fn match_unit(unit: f64, frame: f64, half: f64) -> std::cmp::Ordering {
    if unit < frame - half {
        std::cmp::Ordering::Less
    } else if unit <= frame + half {
        std::cmp::Ordering::Equal
    } else {
        std::cmp::Ordering::Greater
    }
}

impl Drop for GrainFeed {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl FfmpegFrames {
    /// The film grain for the frame at `pts`, if grain is being fed.
    fn grain_for(&mut self, pts: Duration) -> Option<Arc<vidkit_core::av1_grain::GrainParams>> {
        let half = self.frame_interval.as_secs_f64() / 2.0;
        self.grain.as_mut()?.units.take(pts.as_secs_f64(), half)
    }

    /// Whether the decoder is exporting grain for the presenter.
    pub fn grain_on_gpu(&self) -> bool {
        self.grain.is_some()
    }
}

impl Drop for FfmpegFrames {
    /// Kill it. An ffmpeg left running after the window closes keeps decoding
    /// a film nobody is watching, which on a phone is the whole battery.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Bytes in one NV12 frame.
pub fn frame_bytes(w: u32, h: u32) -> usize {
    let (w, h) = (w as usize, h as usize);
    // Chroma rounds UP: an odd dimension still needs a half-plane row, and
    // truncating leaves the last row short so the read never completes.
    w * h + w.div_ceil(2) * 2 * h.div_ceil(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn the_container_start_time_is_read() {
        let p = parse_probe("width=64\nheight=64\nr_frame_rate=30/1\nstart_time=0.080000\n").unwrap();
        assert!((p.start_time - 0.08).abs() < 1e-9);
        let q = parse_probe("width=64\nheight=64\nstart_time=N/A\n").unwrap();
        assert_eq!(q.start_time, 0.0);
    }

    /// Exporting grain is a decoder option (before -i); the feed copies the
    /// same seek window with the file's own timestamps.
    #[test]
    fn grain_arguments_sit_on_the_right_side_of_the_input() {
        let src = VideoInput::file(Path::new("/f.mkv"));
        let a = with_grain_export(video_args_fmt(&src, Duration::from_millis(1500), &Accel::Software, None, PlaneLayout::I420));
        let at = |a: &[String], x: &str| a.iter().position(|y| y == x).unwrap();
        assert!(at(&a, "-export_side_data") < at(&a, "-i") && a[at(&a, "-export_side_data") + 1] == "film_grain");
        let f = grain_feed_args(&src, Duration::from_millis(1500));
        assert!(at(&f, "-ss") < at(&f, "-i") && f[at(&f, "-ss") + 1] == "1.500");
        assert!(at(&f, "-copyts") > at(&f, "-i") && at(&f, "copy") > at(&f, "-i"));
        assert_eq!(f[at(&f, "-f") + 1], "ivf");
        assert!(!grain_feed_args(&src, Duration::ZERO).contains(&"-ss".to_string()));
    }

    #[test]
    fn grain_moves_to_the_gpu_only_where_it_is_exact() {
        let p = parse_probe("codec_name=av1\nwidth=64\nheight=64\npix_fmt=yuv420p\n").unwrap();
        let file = VideoInput::file(Path::new("/f.mkv"));
        let sw = Accel::Software;
        assert!(grain_eligible(&p, &file, &sw, None, PlaneLayout::I420));
        assert!(grain_eligible(&p, &file, &sw, None, PlaneLayout::Nv12));
        assert!(!grain_eligible(&p, &file, &sw, Some((32, 32)), PlaneLayout::I420), "scaled");
        assert!(!grain_eligible(&p, &file, &sw, None, PlaneLayout::P010), "10-bit");
        assert!(!grain_eligible(&p, &VideoInput::url("https://x/v", &[]), &sw, None, PlaneLayout::I420), "a stream");
        let hw = Accel::Vaapi { device: "/dev/dri/renderD128".into(), codec: "av1".into(), vpp: false };
        assert!(!grain_eligible(&p, &file, &hw, None, PlaneLayout::I420), "hardware");
        let vp9 = parse_probe("codec_name=vp9\nwidth=64\nheight=64\n").unwrap();
        assert!(!grain_eligible(&vp9, &file, &sw, None, PlaneLayout::I420));
    }

    #[test]
    fn feed_units_match_frames_within_half_an_interval() {
        use std::cmp::Ordering::*;
        let h = 1.0 / 60.0;
        assert_eq!(match_unit(1.0, 1.5, h), Less);
        assert_eq!(match_unit(1.5 + 0.001, 1.5, h), Equal);
        assert_eq!(match_unit(1.5 - 0.016, 1.5, h), Equal);
        assert_eq!(match_unit(1.5 + 0.034, 1.5, h), Greater);
    }

    /// Units before a frame are skipped, a unit past a missing frame is kept
    /// for its own frame, and a closed feed stops being waited on.
    #[test]
    fn grain_units_skip_behind_and_hold_ahead() {
        let g = |seed| Some(Arc::new(vidkit_core::av1_grain::GrainParams { seed, ..Default::default() }));
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        for (t, s) in [(0.9, 1), (1.0, 2), (1.1, 3), (1.3, 4)] {
            tx.send((t, g(s))).unwrap();
        }
        drop(tx);
        let mut u = GrainUnits::new(rx);
        let h = 0.05;
        assert_eq!(u.take(1.0, h).unwrap().seed, 2, "0.9 skipped");
        assert_eq!(u.take(1.1, h).unwrap().seed, 3);
        assert!(u.take(1.2, h).is_none(), "no unit for 1.2 (1.3 is ahead)");
        assert_eq!(u.take(1.3, h).unwrap().seed, 4, "1.3 was held, not lost");
        let t0 = std::time::Instant::now();
        assert!(u.take(1.4, h).is_none() && u.take(1.5, h).is_none());
        assert!(u.ended && t0.elapsed() < Duration::from_millis(150), "waited on a closed feed");
    }

    /// End to end against ffmpeg's own dav1d: a seeked decode with grain
    /// EXPORTED, plus our grain from the feed's parameters, gives exactly the
    /// picture ffmpeg shows with dav1d applying the grain -- every byte of
    /// every frame, from mid-GOP (the feed starts at the keyframe before the
    /// seek point and must skip to the decoder's first frame).
    #[test]
    fn exported_grain_plus_ours_is_dav1ds_picture() {
        if !have_ffmpeg() {
            return;
        }
        let enc = Command::new("ffmpeg").args(["-hide_banner", "-encoders"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("libsvtav1")).unwrap_or(false);
        if !enc {
            eprintln!("no libsvtav1; skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("vk-grainfeed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("g.mkv");
        let (w, h) = (352usize, 198usize);
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", &format!("testsrc2=size={w}x{h}:rate=30:duration=3,noise=alls=14:allf=t"),
            "-c:v", "libsvtav1", "-preset", "10", "-g", "30", "-svtav1-params", "film-grain=10", "-pix_fmt", "yuv420p"]).arg(&f).stderr(Stdio::null()).status().unwrap().success());
        let from = Duration::from_millis(1500);
        let reference = Command::new("ffmpeg").args(["-v", "error", "-ss", "1.500", "-i"]).arg(&f).args(["-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"]).output().unwrap().stdout;
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let fs = w * h + 2 * cw * ch;
        assert!(reference.len() >= fs * 30, "reference too short: {}", reference.len() / fs);

        set_gpu_grain(true);
        let p = probe(&f).unwrap();
        let mut d = FfmpegFrames::open_probed(&VideoInput::file(&f), &p, None, from, Accel::Software).unwrap();
        assert!(d.grain_on_gpu(), "grain not moved to the GPU");
        let (mut n, mut grained) = (0, 0);
        while let Some(fr) = d.next_frame() {
            let FrameData::Cpu(pl) = &fr.data else { panic!() };
            let (mut y, mut u, mut v) = (pl.data[0][..w * h].to_vec(), Vec::new(), Vec::new());
            if fr.layout == PlaneLayout::I420 {
                u = pl.data[1][..cw * ch].to_vec();
                v = pl.data[2][..cw * ch].to_vec();
            } else {
                for i in 0..cw * ch {
                    u.push(pl.data[1][2 * i]);
                    v.push(pl.data[1][2 * i + 1]);
                }
            }
            let g = fr.grain.as_ref().unwrap_or_else(|| panic!("frame {n}: no grain parameters"));
            vidkit_core::av1_grain::apply_i420(g, &mut y, &mut u, &mut v, w, h, false);
            grained += 1;
            let want = &reference[n * fs..(n + 1) * fs];
            let got: Vec<u8> = [y, u, v].concat();
            let bad = got.iter().zip(want).filter(|(a, b)| a != b).count();
            assert_eq!(bad, 0, "frame {n} (seed {}): {bad} bytes differ from ffmpeg+dav1d", g.seed);
            n += 1;
        }
        assert_eq!(n * fs, reference.len(), "frame count differs from ffmpeg's");
        eprintln!("DIAG {n} frames from 1.5 s, {grained} grained on our side, byte-identical to ffmpeg+dav1d");
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct Fake {
        n: u32,
        name: &'static str,
        note: Option<String>,
    }
    impl VideoSource for Fake {
        fn next_frame(&mut self) -> Option<Frame> {
            (self.n > 0).then(|| {
                self.n -= 1;
                crate::pattern::bars(0.0)
            })
        }
        fn frame_interval(&self) -> Duration { Duration::from_millis(40) }
        fn hdr_peak(&self) -> Option<f32> { None }
        fn set_pool(&mut self, _: Arc<PlanePool>) {}
        fn label(&self) -> String { self.name.into() }
        fn frame_bytes(&self) -> usize { 0 }
        fn take_note(&mut self) -> Option<String> { self.note.take() }
    }

    #[test]
    fn a_cap_keeps_every_other_frame_of_sixty() {
        let ms = |n: u64| Duration::from_micros(n * 16_667);
        let mut last = None;
        let mut kept = 0;
        for k in 0..60 {
            if keep_frame(ms(k), last, Some(30)) {
                last = Some(ms(k));
                kept += 1;
            }
        }
        assert_eq!(kept, 30);
        assert!(keep_frame(ms(1), Some(ms(0)), None), "no cap, every frame");
        assert!(keep_frame(ms(0), Some(ms(5)), Some(30)), "backwards (a seek) is shown");
        // 24 fps content under a 30 cap: nothing dropped.
        let f24 = |n: u64| Duration::from_micros(n * 41_667);
        assert!((1..48).all(|k| keep_frame(f24(k), Some(f24(k - 1)), Some(30))));
    }

    /// A hardware path that opens but gives nothing hands over; one that
    /// played and ended does not (that is the end of the film).
    #[test]
    fn a_source_that_gives_nothing_falls_back_once() {
        let mut s = FallbackSource::new(Box::new(Fake { n: 0, name: "V4L2", note: Some("pixel format QC08C".into()) }), Box::new(|| Some(Box::new(Fake { n: 3, name: "libav", note: None }) as Box<dyn VideoSource>)));
        let mut n = 0;
        while s.next_frame().is_some() {
            n += 1;
        }
        assert_eq!(n, 3);
        let note = s.take_note().unwrap();
        assert!(note.contains("V4L2 failed (pixel format QC08C); now libav"), "{note}");
        let mut t = FallbackSource::new(Box::new(Fake { n: 2, name: "V4L2", note: None }), Box::new(|| panic!("must not fall back after frames")));
        assert!(t.next_frame().is_some() && t.next_frame().is_some() && t.next_frame().is_none());
    }

    /// `30000/1001` is 29.97 and `30/1` is 30. A parser that splits on `/` and
    /// takes the first number gets 30000 fps and every frame is due
    /// immediately.
    #[test]
    fn a_rational_frame_rate_is_divided_not_truncated() {
        let p = parse_probe("width=1920\nheight=1080\nr_frame_rate=30000/1001\n").unwrap();
        assert_eq!((p.width, p.height), (1920, 1080));
        assert!((p.fps - 29.97).abs() < 0.01, "got {}", p.fps);

        let q = parse_probe("width=640\nheight=480\nr_frame_rate=30/1\n").unwrap();
        assert!((q.fps - 30.0).abs() < 0.001);
    }

    /// A stream with no frame rate is not fatal; still images report 0/0.
    #[test]
    fn a_missing_frame_rate_falls_back() {
        let p = parse_probe("width=64\nheight=64\nr_frame_rate=0/0\n").unwrap();
        assert!(p.fps > 0.0, "a zero rate would make every frame due at once");
    }

    #[test]
    fn a_probe_without_dimensions_is_not_a_video() {
        assert!(parse_probe("r_frame_rate=30/1\n").is_none());
        assert!(parse_probe("").is_none());
    }

    /// NV12 chroma rounds up. Truncating leaves the last row short and the
    /// read never completes, so playback stops one frame in with no error.
    #[test]
    fn odd_dimensions_round_the_chroma_plane_up() {
        assert_eq!(frame_bytes(2, 2), 4 + 2);
        assert_eq!(frame_bytes(1920, 1080), 1920 * 1080 * 3 / 2);
        let odd = frame_bytes(3, 3);
        assert_eq!(odd, 9 + 2 * 2 * 2, "an odd frame was rounded down");
    }

    /// Decode something ffmpeg made, so the whole path runs: spawn, pipe,
    /// frame sizing, NV12 layout.
    #[test]
    fn it_decodes_a_real_clip() {
        if !have_ffmpeg() {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        let path = std::env::temp_dir().join(format!("vidkit-dec-{}.mp4", std::process::id()));
        let made = Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=1",
                "-pix_fmt", "yuv420p",
            ])
            .arg(&path)
            .status()
            .expect("spawn ffmpeg");
        assert!(made.success(), "could not make a test clip");

        let mut d = FfmpegFrames::open(&path, None).expect("open");
        assert_eq!((d.width, d.height), (320, 240));
        assert!((d.frame_interval.as_secs_f64() - 0.1).abs() < 0.01);

        let first = d.next_frame().expect("a first frame");
        assert_eq!(first.layout, PlaneLayout::Nv12);
        assert_eq!(first.pts, Duration::ZERO);
        match &first.data {
            FrameData::Cpu(p) => {
                assert_eq!(p.data[0].len(), 320 * 240, "luma is the wrong size");
                assert_eq!(p.data[1].len(), 320 * 240 / 2, "chroma is the wrong size");
                // testsrc is a colour pattern, not a flat field: a decoder
                // handing back zeros would pass every size check above.
                assert!(
                    p.data[0].iter().any(|&v| v > 16),
                    "the luma plane is empty — nothing was decoded"
                );
            }
            _ => panic!("expected CPU planes"),
        }

        let mut n = 1;
        while d.next_frame().is_some() {
            n += 1;
        }
        assert!(n >= 9, "expected about ten frames, got {n}");
        // Timestamps advance, or the frame queue shows one frame forever.
        let _ = std::fs::remove_file(&path);
    }

    /// Scaling happens in ffmpeg. Decoding 1440p to throw most of it away is
    /// the expensive mistake the capability model exists to avoid.
    #[test]
    fn it_scales_down_on_request() {
        if !have_ffmpeg() {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        let path = std::env::temp_dir().join(format!("vidkit-scale-{}.mp4", std::process::id()));
        Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "testsrc=size=640x480:rate=5:duration=1",
                "-pix_fmt", "yuv420p",
            ])
            .arg(&path)
            .status()
            .expect("spawn");

        let mut d = FfmpegFrames::open(&path, Some(320)).expect("open");
        assert_eq!(d.width, 320, "not scaled");
        assert_eq!(d.height, 240, "aspect ratio lost");
        assert!(d.width % 2 == 0 && d.height % 2 == 0, "odd dimensions in NV12");
        let f = d.next_frame().expect("frame");
        match &f.data {
            FrameData::Cpu(p) => assert_eq!(p.data[0].len(), 320 * 240),
            _ => unreachable!(),
        }
        let _ = std::fs::remove_file(&path);
    }

    /// The exact clip the player showed a test pattern for. If this decodes,
    /// the decoder is fine and the wiring is not.
    #[test]
    fn it_decodes_the_clip_the_player_could_not() {
        if !have_ffmpeg() || !Path::new("/tmp/real.mp4").exists() {
            eprintln!("SKIP");
            return;
        }
        let mut d = FfmpegFrames::open(Path::new("/tmp/real.mp4"), Some(1920)).expect("open");
        eprintln!("DIAG size={}x{} interval={:?}", d.width, d.height, d.frame_interval);
        let f = d.next_frame();
        eprintln!("DIAG first frame: {}", f.is_some());
        assert!(f.is_some(), "the decoder returned nothing for a file it opened");
    }

    /// Seek must land somewhere else, and quickly.
    ///
    /// A clip that changes brightness over time, so "did it move" is a
    /// question the pixels answer. Asserting only the reported timestamp would
    /// pass against a seek that reset to zero and relabelled the frames.
    #[test]
    fn seeking_lands_at_a_different_picture() {
        if !have_ffmpeg() {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        let path = std::env::temp_dir().join(format!("vidkit-seek-{}.mp4", std::process::id()));
        // Luma as a function of TIME, so "did it move" is a question the
        // pixels answer. My first attempt faded a solid black source, which
        // stays black — both ends read 16.0 and the test failed on its own
        // fixture rather than on the seek.
        let ok = Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "color=c=black:size=160x120:rate=10:duration=10",
                "-vf", "geq=lum='min(250,T*25)':cb=128:cr=128",
                "-c:v", "libx264", "-g", "10", "-pix_fmt", "yuv420p",
            ])
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("SKIP: could not encode");
            return;
        }

        let mean = |f: &Frame| -> f64 {
            match &f.data {
                FrameData::Cpu(p) => {
                    p.data[0].iter().map(|&v| v as f64).sum::<f64>() / p.data[0].len() as f64
                }
                _ => 0.0,
            }
        };

        let mut d = FfmpegFrames::open(&path, None).expect("open");
        let start = mean(&d.next_frame().expect("first frame"));

        let began = std::time::Instant::now();
        // A seek is a reopen at the target (the engine's Opener does this).
        d = FfmpegFrames::open_at(&path, None, Duration::from_secs(8)).expect("open at 8 s");
        let after = d.next_frame().expect("a frame after seeking");
        let took = began.elapsed();

        assert!(
            (mean(&after) - start).abs() > 5.0,
            "the picture did not change: {start:.1} then {:.1}",
            mean(&after)
        );
        // Timestamps continue from the seek, not from zero, or the transport
        // jumps back to the start of the film on every scrub.
        assert!(
            after.pts >= Duration::from_secs(7),
            "timestamps restarted at {:?}",
            after.pts
        );
        // Restarting ffmpeg with -ss is near-instant; decoding to the target
        // would take about as long as the clip.
        assert!(took < Duration::from_secs(5), "seek took {took:?}");

        let _ = std::fs::remove_file(&path);
    }

    /// `-ss` before `-i` seeks the container; after it, ffmpeg decodes from
    /// the start and discards, so a scrub costs time proportional to how far
    /// you scrubbed — and over HTTP, downloads it too.
    #[test]
    fn the_seek_flag_precedes_the_input() {
        let input = StreamingInput {
            video_url: "http://cdn/v.mp4".into(),
            audio_url: None,
            headers: Vec::new(),
        };
        let mut args = streaming_args(&input, None);
        let at = args.iter().position(|a| a == "-i").unwrap();
        args.insert(at, "12.000".into());
        args.insert(at, "-ss".into());
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        let i = args.iter().position(|a| a == "-i").unwrap();
        assert!(ss < i, "-ss after -i decodes and discards: {args:?}");
    }

    #[test]
    fn a_missing_file_is_reported_rather_than_hanging() {
        if !have_ffmpeg() {
            return;
        }
        let e = probe(Path::new("/nonexistent/nope.mp4")).unwrap_err();
        assert!(!e.is_empty());
    }
}

// ---------------------------------------------------------------------------
// fetch while playing
// ---------------------------------------------------------------------------

/// Decode straight from URLs, without downloading first.
///
/// The download-then-play path waits for the whole file before the first
/// frame, which on a lecture is a long stare at a test pattern. This starts
/// decoding immediately.
///
/// # Why ffmpeg fetches rather than us
///
/// The obvious shape — download in the background, point the decoder at the
/// growing file — does not work. ffmpeg reaching the write head of a regular
/// file sees EOF and stops; there is no "wait for more" for a plain file. And
/// an MP4 written by a remux has its `moov` atom at the END, so the file is
/// not even openable until the last byte lands.
///
/// Giving ffmpeg the URLs sidesteps both: an HTTP input blocks for more data
/// instead of ending, and ffmpeg reads only as far ahead as it needs.
///
/// # Two inputs, because YouTube has two streams
///
/// Video and audio arrive as separate URLs. ffmpeg takes both as inputs and
/// maps one stream from each — the same muxing yt-dlp would have done, done
/// while playing instead of before it.
#[derive(Clone)]
pub struct StreamingInput {
    pub video_url: String,
    /// Separate audio, when the site serves it that way.
    pub audio_url: Option<String>,
    /// Headers the media URL is signed against. A CDN that checks the
    /// User-Agent answers 403 without them, which looks exactly like an
    /// expired link.
    pub headers: Vec<(String, String)>,
}

/// ffmpeg arguments for the VIDEO of a streamed source. Pure.
///
/// The video URL only. A separate audio URL is the audio process's to fetch:
/// this used to map `1:a:0` into the raw-video output as well, and the
/// rawvideo muxer cannot carry audio, so ffmpeg refused to start and a
/// YouTube stream with separate audio -- the ordinary case -- produced no
/// picture at all.
pub fn streaming_args(input: &StreamingInput, scale_to: Option<(u32, u32)>) -> Vec<String> {
    video_args(&VideoInput::url(&input.video_url, &input.headers), Duration::ZERO, &Accel::Software, scale_to)
}

impl FfmpegFrames {
    /// Start decoding from URLs, fetching as it goes. Probes the URL itself.
    /// (Test convenience: the app probes once and uses open_probed.)
    #[cfg(test)]
    pub fn open_streaming(input: &StreamingInput, max_long_edge: Option<u32>) -> Result<Self, String> {
        let p = probe_input(&input.video_url, &input.headers)?;
        Self::open_probed(&VideoInput::url(&input.video_url, &input.headers), &p, max_long_edge, Duration::ZERO, Accel::Software)
    }
}

#[cfg(test)]
mod color_tests {
    use super::*;

    fn probe(fields: &str) -> Probe {
        parse_probe(&format!("codec_name=hevc\nwidth=3840\nheight=2160\nr_frame_rate=24/1\n{fields}")).unwrap()
    }

    #[test]
    fn hdr10_is_recognised_and_decoded_to_p010() {
        let p = probe("pix_fmt=yuv420p10le\ncolor_space=bt2020nc\ncolor_range=tv\ncolor_transfer=smpte2084");
        assert_eq!(p.color(), ColorInfo { matrix: Matrix::Bt2020Ncl, transfer: Transfer::Pq, range: Range::Limited });
        assert_eq!(p.layout(), PlaneLayout::P010);
        let a = video_args_fmt(&VideoInput::file(Path::new("x")), Duration::ZERO, &Accel::Software, None, p.layout());
        assert!(a.windows(2).any(|w| w[0] == "-pix_fmt" && w[1] == "p010le"), "{a:?}");
        let v = video_args_fmt(&VideoInput::file(Path::new("x")), Duration::ZERO, &Accel::Vaapi { device: "/dev/dri/renderD128".into(), codec: "hevc".into(), vpp: true }, Some((1920, 1080)), PlaneLayout::P010);
        assert!(v.iter().any(|x| x.contains("format=p010,hwdownload,format=p010")), "{v:?}");
    }

    #[test]
    fn untagged_and_legacy_streams_get_the_usual_conventions() {
        let sd = parse_probe("width=720\nheight=576\nr_frame_rate=25/1\npix_fmt=yuv420p\ncolor_space=unknown\ncolor_range=unknown\ncolor_transfer=unknown").unwrap();
        assert_eq!(sd.color(), ColorInfo { matrix: Matrix::Bt601, transfer: Transfer::Sdr, range: Range::Limited }, "SD: BT.601");
        assert_eq!(sd.layout(), PlaneLayout::Nv12);
        let hd = probe("pix_fmt=yuv420p");
        assert_eq!(hd.color().matrix, Matrix::Bt709, "HD untagged: BT.709");
        let phone = probe("pix_fmt=yuvj420p");
        assert_eq!(phone.color().range, Range::Full, "yuvj is full range");
        let hlg = probe("pix_fmt=yuv420p10le\ncolor_space=bt2020nc\ncolor_transfer=arib-std-b67");
        assert_eq!(hlg.color().transfer, Transfer::Hlg);
    }

    /// A real 10-bit PQ-tagged clip through ffprobe and the decoder: P010
    /// frames of the right size, carrying the stream's colour.
    #[test]
    fn a_real_hdr10_clip_decodes_to_tagged_p010() {
        if Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).status().is_err() { return; }
        let f = std::env::temp_dir().join(format!("vk-hdr-{}.mkv", std::process::id()));
        let ok = Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=128x72:rate=10:duration=1",
            "-pix_fmt", "yuv420p10le", "-color_primaries", "bt2020", "-color_trc", "smpte2084", "-colorspace", "bt2020nc", "-color_range", "tv",
            "-c:v", "ffv1"]).arg(&f).status().map(|s| s.success()).unwrap_or(false);
        assert!(ok, "fixture");
        let p = super::probe(&f).unwrap();
        assert_eq!((p.layout(), p.color().transfer, p.color().matrix), (PlaneLayout::P010, Transfer::Pq, Matrix::Bt2020Ncl), "{p:?}");
        let mut d = FfmpegFrames::open_probed(&VideoInput::file(&f), &p, None, Duration::ZERO, Accel::Software).unwrap();
        let fr = d.next_frame().expect("a frame");
        assert_eq!(fr.layout, PlaneLayout::P010);
        assert_eq!(fr.color.transfer, Transfer::Pq);
        if let FrameData::Cpu(pl) = &fr.data {
            assert_eq!(pl.data[0].len(), 128 * 72 * 2, "two bytes a luma sample");
            assert_eq!(pl.strides[0], 256);
            // P010 keeps its 10 bits HIGH: the low 6 bits of every sample are 0.
            let low_bits: u32 = pl.data[0].chunks_exact(2).map(|b| (b[0] & 0x3f) as u32).sum();
            assert_eq!(low_bits, 0, "not P010's layout");
        }
        let _ = std::fs::remove_file(&f);
    }
}

#[cfg(test)]
mod peak_tests {
    use super::*;

    #[test]
    fn maxcll_wins_then_the_mastering_display() {
        assert_eq!(parse_hdr_peak("side_data_type=Mastering display metadata\nmax_luminance=40000000/10000\nside_data_type=Content light level metadata\nmax_content=1500"), Some(1500.0));
        assert_eq!(parse_hdr_peak("max_luminance=10000000/10000"), Some(1000.0));
        assert_eq!(parse_hdr_peak("max_content=0\nmax_luminance=6000000/10000"), Some(600.0), "a zero MaxCLL means unknown");
        assert_eq!(parse_hdr_peak("side_data_type=H.26[45] User Data Unregistered SEI message"), None);
    }

    /// A real x265 HDR10 stream with MaxCLL 4000, through ffprobe.
    #[test]
    fn a_real_hdr10_stream_reports_its_maxcll() {
        let f = std::env::temp_dir().join(format!("vk-cll-{}.mkv", std::process::id()));
        let ok = Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=128x72:rate=10:duration=1", "-pix_fmt", "yuv420p10le", "-c:v", "libx265",
            "-x265-params", "log-level=error:hdr10=1:max-cll=4000,400:master-display=G(13250,34500)B(7500,3000)R(34000,16000)WP(15635,16450)L(10000000,50):colorprim=bt2020:transfer=smpte2084:colormatrix=bt2020nc"])
            .arg(&f).status().map(|s| s.success()).unwrap_or(false);
        if !ok { eprintln!("no libx265; skipping"); return; }
        assert_eq!(probe_hdr_peak(&f.to_string_lossy()), Some(4000.0));
        let _ = std::fs::remove_file(&f);
    }
}

#[cfg(test)]
mod vaapi_tests {
    use super::*;
    use vidkit_core::codec::Codec;

    /// The heat bug, pinned: a driver that does not list AV1 gets software
    /// for AV1 (honestly labelled) and VAAPI for what it does list.
    #[test]
    fn vaapi_is_only_asked_for_codecs_the_driver_lists() {
        let m = Machine {
            kind: MachineKind::Other,
            gpu_driver: Some("i915".into()),
            render_node: Some("/dev/dri/renderD128".into()),
            decoders: vec![],
            hwaccels: vec!["vaapi".into()],
            vaapi: Some(vec![(Codec::H264, 8), (Codec::Vp9, 10)]),
            vaapi_vpp: true,
            vulkan: None,
            vulkan_devices: vec![],
        };
        assert_eq!(choose_accel("av1", &m, AccelPref::Auto), Accel::Software);
        assert!(matches!(choose_accel("vp9", &m, AccelPref::Auto), Accel::Vaapi { .. }));
        assert_eq!(choose_accel("hevc", &m, AccelPref::Auto), Accel::Software, "not listed");
        // Without vainfo, the old behaviour: trust the driver name.
        let unknown = Machine { vaapi: None, ..m };
        assert!(matches!(choose_accel("av1", &unknown, AccelPref::Auto), Accel::Vaapi { .. }));
    }
}

#[cfg(test)]
mod av1_tests {
    use super::*;

    /// VAAPI AV1 must name the native decoder, BEFORE -i: ffmpeg's default
    /// AV1 decoder is libdav1d, which never touches the hardware.
    #[test]
    fn vaapi_av1_names_the_native_decoder() {
        let src = VideoInput::file(Path::new("x.webm"));
        let a = video_args(&src, Duration::ZERO, &Accel::Vaapi { device: "/dev/dri/renderD128".into(), codec: "av1".into(), vpp: true }, None);
        let (cv, i) = (a.windows(2).position(|w| w[0] == "-c:v" && w[1] == "av1"), a.iter().position(|x| x == "-i"));
        assert!(cv.is_some() && cv < i, "{a:?}");
        // VP9 needs nothing: the native vp9 decoder is the default.
        let v = video_args(&src, Duration::ZERO, &Accel::Vaapi { device: "/dev/dri/renderD128".into(), codec: "vp9".into(), vpp: true }, None);
        assert!(!v.iter().any(|x| x == "-c:v"), "{v:?}");
        // Software AV1 is left to ffmpeg's default -- libdav1d, the fastest.
        let sw = video_args(&src, Duration::ZERO, &Accel::Software, None);
        assert!(!sw.iter().any(|x| x == "-c:v"));
    }
}

#[cfg(test)]
mod asahi_tests {
    use super::*;
    use vidkit_core::codec::Codec;

    fn asahi(vaapi: Option<Vec<(Codec, u8)>>) -> Machine {
        Machine {
            kind: MachineKind::Asahi,
            gpu_driver: Some("asahi".into()),
            render_node: Some("/dev/dri/renderD128".into()),
            decoders: vec![],
            hwaccels: vec!["vaapi".into()],
            vaapi,
            vaapi_vpp: false,
            vulkan: None,
            vulkan_devices: vec![],
        }
    }

    /// With the VA layer (the published M2 vainfo: H.264, HEVC, VP9), VP9
    /// goes to VAAPI; AV1 on an M2 is software; without the layer, all is.
    #[test]
    fn asahi_uses_the_va_layer_for_what_it_lists() {
        let with = asahi(Some(vec![(Codec::H264, 8), (Codec::Hevc, 10), (Codec::Vp9, 10)]));
        assert!(matches!(choose_accel("vp9", &with, AccelPref::Auto), Accel::Vaapi { vpp: false, .. }));
        assert_eq!(choose_accel("av1", &with, AccelPref::Auto), Accel::Software);
        assert_eq!(choose_accel("vp9", &asahi(None), AccelPref::Auto), Accel::Software);
    }

    /// No video processing: a downscale must not go through scale_vaapi
    /// (the filter fails on Asahi's VA layer); decode on the VA side, let
    /// ffmpeg download, scale in software.
    #[test]
    fn without_video_processing_scaling_is_done_in_software() {
        let src = VideoInput::file(Path::new("x.webm"));
        let a = video_args(&src, Duration::ZERO, &Accel::Vaapi { device: "/dev/dri/renderD128".into(), codec: "vp9".into(), vpp: false }, Some((1280, 720)));
        assert!(a.iter().any(|x| x == "-hwaccel"), "still hardware decode: {a:?}");
        assert!(!a.iter().any(|x| x.contains("scale_vaapi") || x == "-hwaccel_output_format"), "{a:?}");
        assert!(a.iter().any(|x| x.starts_with("scale=1280:720")), "{a:?}");
    }
}

#[cfg(test)]
mod vulkan_tests {
    use super::*;
    use vidkit_core::codec::Codec;

    fn nvidia() -> Machine {
        Machine {
            kind: MachineKind::Other,
            gpu_driver: Some("nvidia".into()),
            render_node: Some("/dev/dri/renderD128".into()),
            decoders: vec![],
            hwaccels: vec!["vaapi".into(), "vulkan".into(), "cuda".into()],
            vaapi: None,
            vaapi_vpp: false,
            vulkan: Some(vec![Codec::Av1, Codec::H264, Codec::Hevc, Codec::Vp9]),
            vulkan_devices: vec![vidkit_core::caps::VkVideoDevice { index: 0, integrated: false, codecs: vec![Codec::Av1, Codec::H264, Codec::Hevc, Codec::Vp9] }],
        }
    }

    /// The gaming PC: NVIDIA was software before (no VA driver in the
    /// list); with Vulkan Video it decodes in hardware.
    #[test]
    fn nvidia_decodes_through_vulkan_video() {
        assert_eq!(choose_accel("av1", &nvidia(), AccelPref::Auto), Accel::Vulkan { codec: "av1".into(), device: 0 });
        let no_vk = Machine { vulkan: None, ..nvidia() };
        assert_eq!(choose_accel("av1", &no_vk, AccelPref::Auto), Accel::Software);
        let no_hwaccel = Machine { hwaccels: vec!["vaapi".into()], ..nvidia() };
        assert_eq!(choose_accel("vp9", &no_hwaccel, AccelPref::Auto), Accel::Software, "an ffmpeg built without vulkan");
    }

    /// Intel with a VA driver lacking AV1 but Vulkan having it: VAAPI for
    /// VP9, Vulkan for AV1.
    #[test]
    fn vaapi_first_vulkan_fills_the_gap() {
        let m = Machine {
            gpu_driver: Some("i915".into()),
            vaapi: Some(vec![(Codec::H264, 8), (Codec::Vp9, 10)]),
            vaapi_vpp: true,
            vulkan: Some(vec![Codec::Av1, Codec::H264, Codec::Hevc]),
            vulkan_devices: vec![vidkit_core::caps::VkVideoDevice { index: 0, integrated: true, codecs: vec![Codec::Av1, Codec::H264, Codec::Hevc] }],
            ..nvidia()
        };
        assert!(matches!(choose_accel("vp9", &m, AccelPref::Auto), Accel::Vaapi { .. }));
        assert_eq!(choose_accel("av1", &m, AccelPref::Auto), Accel::Vulkan { codec: "av1".into(), device: 0 });
    }

    /// A hybrid laptop, dGPU as device 0: H.264 decodes on the iGPU
    /// (device 1), and that index reaches ffmpeg.
    #[test]
    fn hybrid_decode_goes_to_the_igpu_and_ffmpeg_is_told() {
        let m = Machine {
            vulkan: Some(vec![Codec::Av1, Codec::H264]),
            vulkan_devices: vec![
                vidkit_core::caps::VkVideoDevice { index: 0, integrated: false, codecs: vec![Codec::Av1, Codec::H264] },
                vidkit_core::caps::VkVideoDevice { index: 1, integrated: true, codecs: vec![Codec::H264] },
            ],
            ..nvidia()
        };
        let acc = choose_accel("h264", &m, AccelPref::Auto);
        assert_eq!(acc, Accel::Vulkan { codec: "h264".into(), device: 1 });
        let a = video_args(&VideoInput::file(Path::new("x.mp4")), Duration::ZERO, &acc, None);
        assert!(a.windows(2).any(|w| w[0] == "-init_hw_device" && w[1] == "vulkan=vk:1"), "{a:?}");
        assert_eq!(choose_accel("av1", &m, AccelPref::Auto), Accel::Vulkan { codec: "av1".into(), device: 0 }, "only the dGPU has AV1");
    }

    #[test]
    fn vulkan_args_name_the_native_av1_decoder_and_scale_in_software() {
        let src = VideoInput::file(Path::new("x.webm"));
        let a = video_args(&src, Duration::ZERO, &Accel::Vulkan { codec: "av1".into(), device: 0 }, Some((1280, 720)));
        let (hw, cv, i) = (a.iter().position(|x| x == "vulkan"), a.windows(2).position(|w| w[0] == "-c:v" && w[1] == "av1"), a.iter().position(|x| x == "-i"));
        assert!(hw < i && cv.is_some() && cv < i, "{a:?}");
        assert!(a.iter().any(|x| x.starts_with("scale=1280:720")), "{a:?}");
        assert!(!a.iter().any(|x| x == "-hwaccel_output_format"));
    }
}

#[cfg(test)]
mod fedora_tests {
    use super::*;
    use vidkit_core::codec::Codec;

    /// Asahi with the VA layer listing H.264, but Fedora's ffmpeg-free has
    /// no native h264 decoder: software (OpenH264), not a VAAPI that fails.
    #[test]
    fn no_native_decoder_no_hardware_path() {
        let m = Machine {
            kind: MachineKind::Asahi,
            gpu_driver: Some("asahi".into()),
            render_node: Some("/dev/dri/renderD128".into()),
            decoders: ["vp9", "av1", "libdav1d", "libopenh264"].iter().map(|s| s.to_string()).collect(),
            hwaccels: vec!["vaapi".into()],
            vaapi: Some(vec![(Codec::H264, 8), (Codec::Hevc, 10), (Codec::Vp9, 10)]),
            vaapi_vpp: false,
            vulkan: None,
            vulkan_devices: vec![],
        };
        assert_eq!(choose_accel("h264", &m, AccelPref::Auto), Accel::Software);
        assert!(matches!(choose_accel("vp9", &m, AccelPref::Auto), Accel::Vaapi { .. }));
    }
}

#[cfg(test)]
mod core_tests {
    use super::*;

    #[test]
    fn the_lowest_capacity_tier_is_the_efficiency_cores() {
        // M2 Max-shaped: 4 efficiency, 8 performance (capacities invented;
        // the SHAPE is the point).
        let m2max: Vec<(usize, u32)> = (0..4).map(|c| (c, 380)).chain((4..12).map(|c| (c, 1024))).collect();
        assert_eq!(efficiency_cpus(&m2max), vec![0, 1, 2, 3]);
        // Fairphone 5-shaped: 4 x A55, 3 x A78, 1 x X1.
        let fp5: Vec<(usize, u32)> = (0..4).map(|c| (c, 260)).chain((4..7).map(|c| (c, 870))).chain([(7, 1024)]).collect();
        assert_eq!(efficiency_cpus(&fp5), vec![0, 1, 2, 3]);
        let x86: Vec<(usize, u32)> = (0..8).map(|c| (c, 1024)).collect();
        assert!(efficiency_cpus(&x86).is_empty(), "symmetric: nothing to pin to");
        assert!(efficiency_cpus(&[]).is_empty());
    }

    /// The mechanics, for real: a child pinned to CPU 0 is allowed only CPU 0.
    #[test]
    fn a_pinned_child_really_is_pinned() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "grep Cpus_allowed_list /proc/self/status"]);
        pin(&mut cmd, &[0]);
        let out = cmd.output().unwrap();
        let line = String::from_utf8_lossy(&out.stdout);
        assert!(line.trim().ends_with("\t0") || line.trim().ends_with(" 0"), "{line}");
    }
}

#[cfg(test)]
mod planar_tests {
    use super::*;

    /// The measured case: a 3840x2160 VP9 on a 3456-wide panel is left to
    /// the GPU to fit; 4K on a 1080p phone is still shrunk in ffmpeg; past
    /// the texture limit, always.
    #[test]
    fn mild_fits_are_left_to_the_gpu() {
        let src = (3840, 2160);
        assert_eq!(worth_scaling(src, fit_long_edge(3840, 2160, Some(3456)), 16384), None, "0.81 of the area: GPU");
        assert_eq!(worth_scaling(src, fit_long_edge(3840, 2160, Some(1920)), 16384), Some((1920, 1080)), "0.25: ffmpeg");
        assert_eq!(worth_scaling(src, fit_long_edge(3840, 2160, Some(3456)), 2048), Some((3456, 1944)), "past the texture limit");
        assert_eq!(worth_scaling(src, None, 16384), None);
    }

    #[test]
    fn software_8bit_420_goes_planar_to_the_gpu_only() {
        use PlaneLayout::*;
        assert_eq!(output_layout(Nv12, "yuv420p", true, true), I420, "software VP9 to the GPU presenter");
        assert_eq!(output_layout(Nv12, "yuv420p", false, true), Nv12, "VAAPI hands NV12 out natively");
        assert_eq!(output_layout(Nv12, "yuv420p", true, false), Nv12, "the CPU presenter's fast path is NV12");
        assert_eq!(output_layout(P010, "yuv420p10le", true, true), P010, "10-bit stays P010");
        assert_eq!(output_layout(Nv12, "yuv444p", true, true), Nv12, "not 4:2:0: convert");
    }

    /// Real ffmpeg: I420 frames come through as three planes of the right
    /// sizes, byte-identical to ffmpeg's own yuv420p output.
    #[test]
    fn i420_frames_arrive_as_three_planes() {
        let f = std::env::temp_dir().join(format!("vk-i420-{}.mkv", std::process::id()));
        if !Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=160x90:rate=10:duration=1", "-c:v", "ffv1", "-pix_fmt", "yuv420p"]).arg(&f).status().map(|s| s.success()).unwrap_or(false) {
            return;
        }
        set_prefer_planar(true);
        let p = super::probe(&f).unwrap();
        let mut d = FfmpegFrames::open_probed(&VideoInput::file(&f), &p, None, Duration::ZERO, Accel::Software).unwrap();
        set_prefer_planar(false);
        let fr = d.next_frame().unwrap();
        assert_eq!(fr.layout, PlaneLayout::I420);
        let FrameData::Cpu(pl) = &fr.data else { panic!() };
        assert_eq!(pl.data.iter().map(Vec::len).collect::<Vec<_>>(), vec![160 * 90, 80 * 45, 80 * 45]);
        let reference = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&f).args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "yuv420p", "-"]).output().unwrap().stdout;
        assert!(pl.data.concat() == reference, "not ffmpeg's own bytes");
        let _ = std::fs::remove_file(&f);
    }
}

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn the_cap_follows_the_display_up_to_12k_and_the_gpu_limits_it() {
        assert_eq!(cap_for(None, None, None), 1920, "unknown display: the old default");
        assert_eq!(cap_for(Some(3840), Some(16384), None), 3840);
        assert_eq!(cap_for(Some(12288), Some(16384), None), 12288, "12K panel, desktop GPU");
        assert_eq!(cap_for(Some(12288), Some(8192), None), 8192, "a GPU that stops at 8K textures");
        assert_eq!(cap_for(Some(12288), Some(16384), Some(1920)), 1920, "the Medium preset wins");
        assert_eq!(cap_for(Some(320), None, None), 640, "a tiny window still decodes watchably");
        // And the decoder scales a 12K source to fit an 8K texture limit.
        assert_eq!(fit_long_edge(12288, 6480, Some(8192)), Some((8192, 4320)));
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;

    #[test]
    fn the_pool_is_bounded_by_bytes_not_count() {
        let p = PlanePool::new();
        let twelve_k_luma = 12288 * 6480;
        for _ in 0..10 {
            p.put(vec![0u8; twelve_k_luma]);
        }
        assert!(p.bytes() <= POOL_BYTES, "{} MB held", p.bytes() >> 20);
        assert_eq!(p.len(), POOL_BYTES / twelve_k_luma, "as many 12K planes as fit");
        let q = PlanePool::new();
        for _ in 0..40 {
            q.put(vec![0u8; 1920 * 1080]);
        }
        assert_eq!(q.len(), 40, "small planes: far more than the old count of 24");
    }
}

#[cfg(test)]
mod pipe_bench {
    use super::*;
    use std::os::fd::AsRawFd;

    /// Raw NV12 from /dev/zero through ffmpeg: no decode, only the pipe and
    /// the reads. Prints GB/s with the default pipe and a grown one.
    #[test]
    #[ignore]
    fn pipe_throughput() {
        let (w, h) = (3840usize, 2160usize);
        let frame = w * h * 3 / 2;
        for grow in [false, true, false, true, false, true, false, true, false, true] {
            let mut child = Command::new("ffmpeg")
                .args(["-v", "error", "-nostdin", "-f", "rawvideo", "-pix_fmt", "nv12", "-s", &format!("{w}x{h}"), "-r", "500", "-i", "/dev/zero", "-frames:v", "240", "-c:v", "rawvideo", "-f", "rawvideo", "-"])
                .stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
            let mut out = child.stdout.take().unwrap();
            let size = if grow { grow_pipe(out.as_raw_fd(), PIPE_WANT) } else { None };
            let mut buf = vec![0u8; frame];
            let t = std::time::Instant::now();
            let mut n = 0;
            while out.read_exact(&mut buf).is_ok() { n += 1; }
            let s = t.elapsed().as_secs_f64();
            let _ = child.wait();
            eprintln!("DIAG pipe {:>8}: {n} 4K frames in {s:.2} s = {:.2} GB/s, {:.0} fps", size.map(|s| format!("{} KiB", s >> 10)).unwrap_or("default".into()), (n * frame) as f64 / s / 1e9, n as f64 / s);
        }
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    fn input() -> StreamingInput {
        StreamingInput {
            video_url: "http://cdn/v.mp4".into(),
            audio_url: None,
            headers: vec![("User-Agent".into(), "Mozilla/5.0".into())],
        }
    }

    /// `-headers` is an INPUT option. After the `-i` it applies to, ffmpeg
    /// ignores it and the fetch 403s on any CDN that checks.
    #[test]
    fn headers_come_before_the_input_they_apply_to() {
        let a = streaming_args(&input(), None);
        let hi = a.iter().position(|x| x == "-headers").expect("no headers");
        let ii = a.iter().position(|x| x == "-i").expect("no input");
        assert!(hi < ii, "headers after -i are ignored: {a:?}");
    }

    /// The video process must not touch the audio URL: mapped into the
    /// rawvideo output, ffmpeg refuses to start and there is no picture.
    #[test]
    fn the_video_process_never_fetches_the_audio_url() {
        let mut i = input();
        i.audio_url = Some("http://cdn/a.m4a".into());
        let a = streaming_args(&i, None);
        assert_eq!(a.iter().filter(|x| *x == "-i").count(), 1, "{a:?}");
        assert!(!a.iter().any(|x| x.contains("a.m4a")), "{a:?}");
        assert!(a.windows(2).any(|w| w[0] == "-map" && w[1] == "0:v:0"));
        assert!(a.iter().any(|x| x == "-an"), "{a:?}");
    }

    /// The failure itself, against real ffmpeg: two local files stand in for
    /// the two URLs. The old argument shape produced zero bytes here.
    #[test]
    fn video_decodes_when_the_site_serves_audio_separately() {
        if Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null())
            .status().map(|s| !s.success()).unwrap_or(true) { return; }
        let dir = std::env::temp_dir().join(format!("vk_sep_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (v, au) = (dir.join("v.mp4"), dir.join("a.m4a"));
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=64x48:rate=10:duration=1", "-c:v", "libx264", "-pix_fmt", "yuv420p"]).arg(&v).status().unwrap().success());
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=duration=1", "-c:a", "aac"]).arg(&au).status().unwrap().success());
        let input = StreamingInput {
            video_url: v.to_string_lossy().into(),
            audio_url: Some(au.to_string_lossy().into()),
            headers: vec![],
        };
        let mut d = FfmpegFrames::open_streaming(&input, None).expect("open");
        let mut n = 0;
        while d.next_frame().is_some() { n += 1; }
        assert_eq!(n, 10, "one second at 10 fps");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A CDN closing an idle socket mid-film is routine; without reconnect the
    /// film just stops.
    #[test]
    fn it_reconnects_on_a_dropped_connection() {
        let a = streaming_args(&input(), None);
        assert!(a.iter().any(|x| x == "-reconnect"));
        assert!(a.iter().any(|x| x == "-reconnect_streamed"));
    }

    #[test]
    fn the_output_is_raw_nv12_on_stdout() {
        let a = streaming_args(&input(), None);
        assert_eq!(a.last().unwrap(), "-");
        assert!(a.windows(2).any(|w| w[0] == "-pix_fmt" && w[1] == "nv12"));
    }

    /// FETCH WHILE PLAYING, against a real socket: ffmpeg pulls over HTTP and
    /// frames come out. The server counts how much it served, and the check is
    /// that decoding STARTED before the whole file went out.
    #[test]
    fn it_decodes_over_http_without_downloading_first() {
        if Command::new("ffmpeg").arg("-version").stdout(Stdio::null())
            .stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true)
        {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        use std::io::{Read, Write};
        use std::net::TcpListener;

        // Long enough that serving it all takes visibly longer than the header.
        let clip = std::env::temp_dir().join(format!("vidkit-stream-{}.mp4", std::process::id()));
        let ok = Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=20",
                "-pix_fmt", "yuv420p", "-c:v", "libx264",
                // moov at the front, so a streaming reader can start.
                "-movflags", "+faststart",
            ])
            .arg(&clip)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("SKIP: could not encode");
            return;
        }
        let body = std::fs::read(&clip).unwrap();
        let total = body.len();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..32 {
                let Ok((mut sock, _)) = listener.accept() else { break };
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 { continue; }
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let (start, end) = req.lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split('=').nth(1).map(str::trim).map(str::to_string))
                    .and_then(|r| {
                        let (a, b) = r.split_once('-')?;
                        Some((a.parse::<usize>().ok()?, b.parse::<usize>().unwrap_or(body.len() - 1)))
                    })
                    .unwrap_or((0, body.len() - 1));
                if start >= body.len() { continue; }
                let slice = &body[start..=end.min(body.len() - 1)];
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nAccept-Ranges: bytes\r\nContent-Type: video/mp4\r\nContent-Range: bytes {start}-{}/{}\r\nContent-Length: {}\r\n\r\n",
                    start + slice.len() - 1, body.len(), slice.len());
                let _ = sock.write_all(head.as_bytes());
                // Dribble it, so a decoder that waited for the whole file
                // would visibly stall.
                for chunk in slice.chunks(16 * 1024) {
                    if sock.write_all(chunk).is_err() { break; }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        });

        let input = StreamingInput {
            video_url: format!("http://127.0.0.1:{port}/clip.mp4"),
            audio_url: None,
            headers: Vec::new(),
        };
        let started = std::time::Instant::now();
        let mut d = FfmpegFrames::open_streaming(&input, None).expect("open streaming");
        assert_eq!((d.width, d.height), (320, 240));

        let first = d.next_frame().expect("a first frame over http");
        let elapsed = started.elapsed();
        match &first.data {
            FrameData::Cpu(p) => {
                assert_eq!(p.data[0].len(), 320 * 240);
                assert!(p.data[0].iter().any(|&v| v > 16), "decoded an empty frame");
            }
            _ => panic!("expected CPU planes"),
        }
        // Serving 20 seconds of dribbled video takes far longer than this; a
        // first frame this early means it did not wait for the file.
        assert!(
            elapsed < Duration::from_secs(20),
            "waited {elapsed:?} for the first frame — that is download-then-play"
        );
        eprintln!("DIAG first frame after {elapsed:?} of a {total}-byte clip");
        let _ = std::fs::remove_file(&clip);
    }
}

// ---------------------------------------------------------------------------
// audio
// ---------------------------------------------------------------------------

/// PCM pulled out of the same source the video comes from.
///
/// A separate ffmpeg rather than a second output on the video one: two raw
/// streams down one pipe would need demultiplexing that ffmpeg will not do for
/// us, and a second process costs less than getting that wrong.
pub struct AudioStream {
    child: Child,
    pub rate: u32,
    pub channels: u16,
    buf: Vec<u8>,
    pub frames_read: u64,
}

/// Interleaved signed 16-bit samples, which is what every sink here wants.
pub const AUDIO_FORMAT: &str = "s16le";

/// ffmpeg arguments for audio-only output.
///
/// Pure, so the flags are checked without spawning: `-vn` matters most. Without
/// it ffmpeg picks the video stream too and the "PCM" is a video frame, which
/// plays as a burst of noise loud enough to hurt.
pub fn audio_args(input: &str, rate: u32, channels: u16, from: Duration) -> Vec<String> {
    let mut a: Vec<String> = vec!["-v".into(), "error".into(), "-nostdin".into()];
    if !from.is_zero() {
        a.push("-ss".into());
        a.push(format!("{:.3}", from.as_secs_f64()));
    }
    a.push("-i".into());
    a.push(input.into());
    a.push("-vn".into());
    a.push("-f".into());
    a.push(AUDIO_FORMAT.into());
    a.push("-acodec".into());
    a.push("pcm_s16le".into());
    a.push("-ar".into());
    a.push(rate.to_string());
    a.push("-ac".into());
    a.push(channels.to_string());
    a.push("-".into());
    a
}

/// `atempo` factors whose product is `speed`, each within 0.5..=2.0.
///
/// ffmpeg before 4.3 accepts only that range per instance, and a phone's
/// distribution may ship one that old; chaining two covers 0.25..4, the
/// range the speed control allows. `None` at 1.0: no filter, no cost.
pub fn atempo_chain(speed: f64) -> Option<String> {
    let s = speed.clamp(0.25, 4.0);
    if (s - 1.0).abs() < 1e-3 {
        return None;
    }
    let mut factors = Vec::new();
    let mut rest = s;
    while rest > 2.0 {
        factors.push(2.0);
        rest /= 2.0;
    }
    while rest < 0.5 {
        factors.push(0.5);
        rest /= 0.5;
    }
    factors.push(rest);
    Some(factors.iter().map(|f| format!("atempo={f:.4}")).collect::<Vec<_>>().join(","))
}

/// Insert the tempo filter before the output (`-`, always last).
pub fn with_tempo(mut args: Vec<String>, speed: f64) -> Vec<String> {
    if let Some(chain) = atempo_chain(speed) {
        let at = args.len().saturating_sub(1);
        args.insert(at, chain);
        args.insert(at, "-af".into());
    }
    args
}

/// Audio-only ffmpeg arguments for a STREAMED source: the audio URL when the
/// site serves audio separately (YouTube does), the video URL otherwise, with
/// the resolved headers -- the signed URL is bound to them, and without them
/// the fetch is a 403 -- and the same reconnect flags the video side uses.
pub fn audio_streaming_args(input: &StreamingInput, rate: u32, channels: u16, from: Duration) -> Vec<String> {
    let mut a: Vec<String> = vec!["-v".into(), "error".into(), "-nostdin".into()];
    if !from.is_zero() {
        a.push("-ss".into());
        a.push(format!("{:.3}", from.as_secs_f64()));
    }
    if !input.headers.is_empty() {
        a.push("-headers".into());
        a.push(input.headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect());
    }
    for f in ["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "5"] {
        a.push(f.into());
    }
    a.push("-i".into());
    a.push(input.audio_url.clone().unwrap_or_else(|| input.video_url.clone()));
    for f in ["-vn", "-f", AUDIO_FORMAT, "-acodec", "pcm_s16le", "-ar"] {
        a.push(f.into());
    }
    a.push(rate.to_string());
    a.push("-ac".into());
    a.push(channels.to_string());
    a.push("-".into());
    a
}

impl AudioStream {
    /// `speed` is applied by ffmpeg's `atempo` (pitch kept); 1.0 adds no
    /// filter at all.
    pub fn open(input: &str, rate: u32, channels: u16, from: Duration, speed: f64) -> Result<Self, String> {
        Self::open_args(with_tempo(audio_args(input, rate, channels, from), speed), rate, channels)
    }

    /// Audio from a streamed source -- see `audio_streaming_args`.
    pub fn open_streaming(input: &StreamingInput, rate: u32, channels: u16, from: Duration, speed: f64) -> Result<Self, String> {
        Self::open_args(with_tempo(audio_streaming_args(input, rate, channels, from), speed), rate, channels)
    }

    fn open_args(args: Vec<String>, rate: u32, channels: u16) -> Result<Self, String> {
        let child = Command::new("ffmpeg")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("ffmpeg (audio): {e}"))?;
        Ok(Self {
            child,
            rate,
            channels,
            // A tenth of a second per read: small enough that the clock stays
            // responsive, large enough that the pipe is not the bottleneck.
            buf: vec![0u8; (rate as usize / 10) * channels as usize * 2],
            frames_read: 0,
        })
    }

    /// The next block of PCM, or `None` at the end.
    /// Mutable so the engine can apply volume in place, without a copy.
    pub fn next_block(&mut self) -> Option<&mut [u8]> {
        let stdout = self.child.stdout.as_mut()?;
        // A partial final block is still audio. Reading exactly would discard
        // the tail of every file whose length is not a multiple of the block.
        let mut filled = 0;
        while filled < self.buf.len() {
            match stdout.read(&mut self.buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(_) => break,
            }
        }
        if filled == 0 {
            return None;
        }
        self.frames_read += (filled / (self.channels as usize * 2)) as u64;
        Some(&mut self.buf[..filled])
    }

    /// How much audio has been produced, which is what an audio clock needs.
    pub fn position(&self) -> Duration {
        Duration::from_secs_f64(self.frames_read as f64 / self.rate.max(1) as f64)
    }
}

impl Drop for AudioStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Somewhere for PCM to go.
///
/// A trait because there is no audio device in the sandbox this was written in,
/// and a sink that cannot be tested is a sink that does not work. Everything
/// above it — decoding, block sizing, the position an audio clock reads — is
/// exercised against a recording sink.
pub trait AudioSink: Send {
    fn play(&mut self, pcm: &[u8]) -> Result<(), String>;
    fn name(&self) -> &str;
    /// Audio written but not yet heard: what the audio clock must SUBTRACT
    /// (see vidkit_core::audio_clock). An estimate for a piped player.
    fn latency(&self) -> Duration {
        Duration::ZERO
    }
    /// Does `play` consume audio at the device's real rate? A sink that does
    /// not -- the silent fallback swallows everything instantly -- must never
    /// drive the playback clock, or the film would finish in a second.
    fn realtime(&self) -> bool {
        true
    }
    /// Stop or restart the device around a pause. A piped player cannot be
    /// paused (it plays out its buffer and underruns), so the default is to
    /// do nothing.
    fn set_paused(&mut self, _paused: bool) {}
    /// Frames of silence the device inserted because it ran dry, since the
    /// last call. The audio clock subtracts them, or every underrun would
    /// permanently shift sync. Only an in-process sink can know this.
    fn take_underrun_frames(&mut self) -> u64 {
        0
    }
}

/// Discards audio, and says so. Used when no player is installed.
pub struct NullSink {
    pub bytes: u64,
}

impl AudioSink for NullSink {
    fn play(&mut self, pcm: &[u8]) -> Result<(), String> {
        self.bytes += pcm.len() as u64;
        Ok(())
    }
    fn name(&self) -> &str {
        "none (silent)"
    }
    fn realtime(&self) -> bool {
        false
    }
}

/// The command-line players, in preference order.
///
/// Same shelling-out pattern as the clipboard: no crate dependency, and on a
/// PipeWire system the socket protocol is not something an application should
/// speak directly.
///
/// Each asks for an explicit output latency of PLAYER_LATENCY_MS, so the audio
/// clock subtracts a known figure rather than a default that differs per
/// player (pw-play's is 100 ms). The flags are from each tool's documentation;
/// they have not been run here -- this sandbox has no audio device.
pub const AUDIO_PLAYERS: &[(&str, &[&str])] = &[
    ("pw-play", &["--format", "s16", "--rate", "{rate}", "--channels", "{ch}", "--latency", "{lat}ms", "-"]),
    ("paplay", &["--format=s16le", "--rate={rate}", "--channels={ch}", "--latency-msec={lat}", "--raw"]),
    ("aplay", &["-q", "-f", "S16_LE", "-r", "{rate}", "-c", "{ch}", "--buffer-time={lat_us}", "-"]),
];

/// The output latency every player is asked for.
pub const PLAYER_LATENCY_MS: u32 = 40;

/// The pipe to the player is shrunk to this. Linux's default 64 KB holds
/// ~340 ms of 48 kHz stereo s16 -- written, not yet heard.
pub const PIPE_BYTES: i32 = 8192;

/// Fill in the rate and channel placeholders.
pub fn player_args(args: &[&str], rate: u32, channels: u16) -> Vec<String> {
    args.iter()
        .map(|a| {
            a.replace("{rate}", &rate.to_string())
                .replace("{ch}", &channels.to_string())
                .replace("{lat_us}", &(PLAYER_LATENCY_MS * 1000).to_string())
                .replace("{lat}", &PLAYER_LATENCY_MS.to_string())
        })
        .collect()
}

/// Which player is installed, if any.
pub fn find_audio_player() -> Option<&'static str> {
    AUDIO_PLAYERS.iter().map(|(c, _)| *c).find(|c| {
        Command::new("which")
            .arg(c)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Pipes PCM to a command-line player.
pub struct CommandSink {
    child: Child,
    player: &'static str,
    latency: Duration,
}

impl CommandSink {
    /// Open, shrink the pipe, and derive the latency from the pipe size the
    /// kernel ACTUALLY granted (read back, not assumed) plus the player's.
    pub fn open_with_format(player: &'static str, args: &[String], rate: u32, channels: u16) -> Result<Self, String> {
        let child = Command::new(player)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{player}: {e}"))?;
        let pipe = child.stdin.as_ref().map(|s| shrink_pipe(s)).unwrap_or(65_536);
        let per_sec = rate.max(1) as f64 * channels.max(1) as f64 * 2.0;
        let latency = Duration::from_secs_f64(pipe as f64 / per_sec)
            + Duration::from_millis(PLAYER_LATENCY_MS as u64);
        Ok(Self { child, player, latency })
    }
}

impl AudioSink for CommandSink {
    fn play(&mut self, pcm: &[u8]) -> Result<(), String> {
        use std::io::Write;
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| format!("{}: stdin closed", self.player))?;
        // A player that has exited leaves a broken pipe. Reported rather than
        // ignored: silently dropping audio for the rest of the film is the
        // failure that gets described as "it just went quiet".
        stdin
            .write_all(pcm)
            .map_err(|e| format!("{}: {e}", self.player))
    }
    fn name(&self) -> &str {
        self.player
    }
    fn latency(&self) -> Duration {
        self.latency
    }
}

/// Ask for a PIPE_BYTES pipe and return the size actually in force. Falls back
/// to reporting Linux's 64 KB default if the calls fail.
pub fn shrink_pipe(stdin: &std::process::ChildStdin) -> usize {
    use std::os::unix::io::AsRawFd;
    let fd = stdin.as_raw_fd();
    // SAFETY: fcntl on a valid, owned pipe fd with integer arguments only.
    unsafe {
        libc::fcntl(fd, libc::F_SETPIPE_SZ, PIPE_BYTES);
        let got = libc::fcntl(fd, libc::F_GETPIPE_SZ);
        if got > 0 { got as usize } else { 65_536 }
    }
}

impl Drop for CommandSink {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Open the best sink available, falling back to silence.
///
/// Returns the null sink rather than an error when nothing is installed: a
/// video player that refuses to play because the machine has no sound daemon
/// is worse than one that plays quietly and says so. The NAME is what the UI
/// shows, which is how the difference stays visible.
pub fn open_sink(rate: u32, channels: u16) -> Box<dyn AudioSink> {
    // In process first: no player process per seek, and a measured latency
    // instead of an estimate. VIDKIT_AUDIO=pipe skips it, for comparing the
    // two on a device.
    #[cfg(feature = "alsa")]
    if std::env::var("VIDKIT_AUDIO").as_deref() != Ok("pipe") {
        match crate::audio_out::open_cpal(rate, channels) {
            Ok(s) => return Box::new(s),
            Err(e) => eprintln!("vidkit audio: in-process output unavailable ({e}); trying a player"),
        }
    }
    if let Some(player) = find_audio_player() {
        let args = AUDIO_PLAYERS
            .iter()
            .find(|(c, _)| *c == player)
            .map(|(_, a)| player_args(a, rate, channels))
            .unwrap_or_default();
        if let Ok(s) = CommandSink::open_with_format(player, &args, rate, channels) {
            return Box::new(s);
        }
    }
    Box::new(NullSink { bytes: 0 })
}

#[cfg(test)]
mod audio_tests {

    /// Streamed audio: the separate audio URL is preferred, the headers apply
    /// to it (placed BEFORE its -i -- after, ffmpeg treats them as output
    /// options and the fetch is a 403), a seek lands before -i, and -vn is there.
    #[test]
    fn streamed_audio_takes_the_audio_url_with_its_headers() {
        let input = StreamingInput {
            video_url: "https://v.example/video".into(),
            audio_url: Some("https://a.example/audio".into()),
            headers: vec![("User-Agent".into(), "UA/1".into())],
        };
        let a = audio_streaming_args(&input, 48_000, 2, Duration::from_millis(2500));
        let at = |s: &str| a.iter().position(|x| x == s).unwrap_or_else(|| panic!("{s} missing: {a:?}"));
        assert_eq!(a[at("-i") + 1], "https://a.example/audio");
        assert!(at("-headers") < at("-i") && a[at("-headers") + 1].contains("User-Agent: UA/1\r\n"));
        assert!(at("-ss") < at("-i") && a[at("-ss") + 1] == "2.500");
        assert!(at("-vn") > at("-i"), "audio only");
        assert!(!a.iter().any(|x| x.contains("v.example")), "the video URL is not fetched for audio");
        let muxed = StreamingInput { audio_url: None, ..input };
        let m = audio_streaming_args(&muxed, 48_000, 2, Duration::ZERO);
        assert!(m.contains(&"https://v.example/video".to_string()), "no separate audio: the one URL");
        assert!(!m.contains(&"-ss".to_string()), "no seek from zero");
    }
    use super::*;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").stdout(Stdio::null())
            .stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }

    /// Without `-vn`, ffmpeg takes the video stream too and the "PCM" is a
    /// video frame — which plays as a burst of noise loud enough to hurt.
    #[test]
    fn audio_output_excludes_video() {
        let a = audio_args("in.mp4", 48_000, 2, Duration::ZERO);
        assert!(a.iter().any(|x| x == "-vn"), "no -vn: {a:?}");
        assert!(a.windows(2).any(|w| w[0] == "-f" && w[1] == "s16le"));
        // The codec as well as the container format. Reading 8-bit output as
        // pairs of bytes still yields plausible-looking samples, so the
        // decode test alone cannot tell the two apart.
        assert!(a.windows(2).any(|w| w[0] == "-acodec" && w[1] == "pcm_s16le"), "{a:?}");
        assert!(a.windows(2).any(|w| w[0] == "-ar" && w[1] == "48000"));
        assert!(a.windows(2).any(|w| w[0] == "-ac" && w[1] == "2"));
    }

    /// Every factor within old ffmpeg's 0.5..2 limit, the product the speed,
    /// and no filter at all at 1x.
    #[test]
    fn atempo_chains_cover_the_whole_speed_range() {
        assert_eq!(atempo_chain(1.0), None);
        for &speed in &[0.25, 0.3, 0.5, 0.75, 1.05, 1.5, 2.0, 2.5, 3.0, 4.0] {
            let c = atempo_chain(speed).unwrap_or_else(|| panic!("{speed}"));
            let f: Vec<f64> = c.split(',').map(|x| x.trim_start_matches("atempo=").parse().unwrap()).collect();
            assert!(!f.is_empty());
            assert!(f.iter().all(|x| (0.5..=2.0).contains(x)), "{speed}: {c}");
            let prod: f64 = f.iter().product();
            assert!((prod - speed).abs() < 1e-3, "{speed}: {c} = {prod}");
        }
        let a = with_tempo(audio_args("x", 48_000, 2, Duration::ZERO), 2.0);
        assert_eq!(a.last().unwrap(), "-", "output stays last");
        assert_eq!(a[a.len() - 3], "-af");
    }

    /// Audio must seek with the video, or a scrub desynchronises the two.
    #[test]
    fn the_seek_flag_precedes_the_input_here_too() {
        let a = audio_args("in.mp4", 48_000, 2, Duration::from_secs(9));
        let ss = a.iter().position(|x| x == "-ss").expect("no -ss");
        let i = a.iter().position(|x| x == "-i").expect("no -i");
        assert!(ss < i, "decoding and discarding to the seek point: {a:?}");
    }

    #[test]
    fn player_placeholders_are_filled_in() {
        let a = player_args(AUDIO_PLAYERS[2].1, 44_100, 1);
        assert!(a.contains(&"44100".to_string()), "{a:?}");
        assert!(a.contains(&"1".to_string()), "{a:?}");
        assert!(!a.iter().any(|x| x.contains('{')), "a placeholder survived: {a:?}");
    }

    /// Decode a tone and check it is not silence. Every size assertion passes
    /// against a stream of zeros, which is exactly what a wrong `-acodec` or a
    /// missing stream produces.
    #[test]
    fn it_decodes_audible_pcm() {
        if !have_ffmpeg() {
            eprintln!("SKIP: no ffmpeg");
            return;
        }
        let path = std::env::temp_dir().join(format!("vidkit-aud-{}.mp4", std::process::id()));
        let ok = Command::new("ffmpeg")
            .args([
                "-v", "error", "-y",
                "-f", "lavfi", "-i", "sine=frequency=440:duration=2",
                "-f", "lavfi", "-i", "color=c=black:size=64x64:rate=10:duration=2",
                "-shortest", "-c:v", "libx264", "-pix_fmt", "yuv420p",
            ])
            .arg(&path)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("SKIP: could not encode");
            return;
        }

        let mut a = AudioStream::open(&path.to_string_lossy(), 48_000, 2, Duration::ZERO, 1.0)
            .expect("open audio");
        let block = a.next_block().expect("a block of pcm").to_vec();
        assert!(!block.is_empty());

        let peak = block
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(peak > 1000, "the audio is silence — peak {peak}");

        // The position an audio clock would read advances with the samples.
        assert!(a.position() > Duration::from_millis(50), "{:?}", a.position());

        let mut total = block.len();
        while let Some(b) = a.next_block() {
            total += b.len();
        }
        // Two seconds, stereo, 48 kHz, 2 bytes a sample.
        let expected = 2 * 48_000 * 2 * 2;
        // Within ten percent, not merely "more than half": at half the
        // tolerance an 8-bit stream would pass, and 8-bit read as 16-bit is
        // noise at the wrong pitch rather than an error.
        let diff = (total as i64 - expected as i64).abs();
        assert!(
            diff * 10 < expected as i64,
            "got {total} bytes, expected about {expected} — wrong sample width?"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// atempo really compresses the stream: 2 s of media at 2x is 1 s of
    /// PCM, at 0.5x 4 s -- the only place tempo is observable, since an
    /// engine-level test ends with the video either way.
    #[test]
    fn speed_changes_how_much_audio_the_stream_yields() {
        if !have_ffmpeg() { return; }
        let path = std::env::temp_dir().join(format!("vidkit-tempo-{}.wav", std::process::id()));
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"]).arg(&path).status().unwrap().success());
        let secs = |speed: f64| {
            let mut a = AudioStream::open(&path.to_string_lossy(), 48_000, 2, Duration::ZERO, speed).unwrap();
            let mut n = 0usize;
            while let Some(b) = a.next_block() { n += b.len(); }
            n as f64 / (48_000.0 * 4.0)
        };
        let (one, two, half) = (secs(1.0), secs(2.0), secs(0.5));
        assert!((one - 2.0).abs() < 0.1, "1x: {one}");
        assert!((two - 1.0).abs() < 0.1, "2x: {two}");
        assert!((half - 4.0).abs() < 0.15, "0.5x: {half}");
        let _ = std::fs::remove_file(&path);
    }

    /// No sound daemon must not stop the video. A player that refuses to run
    /// because the machine is silent is worse than one that plays quietly and
    /// says so in the UI.
    #[test]
    fn a_machine_with_no_player_still_gets_a_sink() {
        let s = open_sink(48_000, 2);
        assert!(!s.name().is_empty());
        // Either a real player or the honest null one; never a failure.
    }

    /// A sink that discards must still account for what it discarded, or
    /// "audio is playing" and "audio is being thrown away" look identical.
    #[test]
    fn the_null_sink_counts_what_it_drops() {
        let mut s = NullSink { bytes: 0 };
        s.play(&[0u8; 100]).unwrap();
        s.play(&[0u8; 50]).unwrap();
        assert_eq!(s.bytes, 150);
        assert!(s.name().contains("silent"), "the name should admit it: {}", s.name());
    }
}
