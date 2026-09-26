//! CPU YUV -> RGBA, for presenters that cannot sample a DMA-BUF.
//!
//! **This mirrors `vidkit-vk/shaders/convert.wgsl` and must stay in step with
//! it.** Two copies of colour maths is a drift hazard, so
//! `vidkit-vk/tests/parity.rs` runs both over the same inputs and fails on any
//! disagreement — the same arrangement `maintenance/tools/voiceprint_parity_check.py`
//! exists for elsewhere in this tree. If you change one, the test tells you
//! about the other.
//!
//! Why a second implementation exists at all: egui's glow backend samples
//! ordinary textures, so a frame reaches the screen as an RGBA upload. The
//! zero-copy path — import the decoder's DMA-BUF and convert on the GPU — needs
//! the wgpu backend and `wgpu_hal` access, which is a separate presenter. This
//! is the portable one, and on a phone playing 1080p it is also the one that
//! costs measurable battery, so prefer the GPU path where it exists.

use crate::frame::{ColorInfo, CpuPlanes, Frame, FrameData, Matrix, PlaneLayout, Range};
use crate::codec::Transfer;

/// Column-major YUV->RGB coefficients, matching `yuv_to_rgb_matrix` in the
/// shader. `[y, cb, cr]` contribution per output channel.
fn matrix_coeffs(m: Matrix) -> [[f32; 3]; 3] {
    match m {
        // r = y + 1.59603*cr, g = y - 0.39176*cb - 0.81297*cr, b = y + 2.01723*cb
        Matrix::Bt601 => [
            [1.0, 0.0, 1.59603],
            [1.0, -0.39176, -0.81297],
            [1.0, 2.01723, 0.0],
        ],
        Matrix::Bt709 => [
            [1.0, 0.0, 1.57480],
            [1.0, -0.18732, -0.46812],
            [1.0, 1.85560, 0.0],
        ],
        Matrix::Bt2020Ncl => [
            [1.0, 0.0, 1.47460],
            [1.0, -0.16455, -0.57135],
            [1.0, 1.88140, 0.0],
        ],
    }
}

/// SMPTE ST 2084 EOTF, normalised so 1.0 == 10000 nits. Mirrors `pq_eotf`.
fn pq_eotf(e: f32) -> f32 {
    const M1: f32 = 0.1593017578125;
    const M2: f32 = 78.84375;
    const C1: f32 = 0.8359375;
    const C2: f32 = 18.8515625;
    const C3: f32 = 18.6875;
    let ep = e.max(0.0).powf(1.0 / M2);
    let num = (ep - C1).max(0.0);
    let den = C2 - C3 * ep;
    (num / den.max(1e-6)).powf(1.0 / M1)
}

/// ARIB STD-B67 inverse OETF. Mirrors `hlg_eotf`.
fn hlg_eotf(e: f32) -> f32 {
    const A: f32 = 0.17883277;
    const B: f32 = 0.28466892;
    const C: f32 = 0.55991073;
    if e <= 0.5 {
        (e * e) / 3.0
    } else {
        (((e - C) / A).exp() + B) / 12.0
    }
}

/// Luminance the tonemap treats as SDR white, in nits. ITU reference white.
pub const REFERENCE_WHITE_NITS: f32 = 203.0;

/// HLG's nominal display peak for its OOTF (BT.2100: 1000 nits, gamma 1.2).
const HLG_PEAK_NITS: f32 = 1000.0;

/// Linear BT.2020 -> linear BT.709 (BT.2087). Applied only when the stream's
/// matrix is BT.2020, used as the proxy for BT.2020 primaries: ColorInfo
/// carries no separate primaries, and HDR10 and HLG broadcast are BT.2020 in
/// both. Out-of-gamut results are clipped at 0, the simple choice.
const BT2020_TO_BT709: [[f32; 3]; 3] = [
    [1.6605, -0.5876, -0.0728],
    [-0.1246, 1.1329, -0.0083],
    [-0.0182, -0.1006, 1.1187],
];

/// SMPTE ST 2084 inverse EOTF: absolute nits -> PQ signal 0..1.
pub fn pq_oetf(nits: f32) -> f32 {
    const M1: f32 = 0.1593017578125;
    const M2: f32 = 78.84375;
    const C1: f32 = 0.8359375;
    const C2: f32 = 18.8515625;
    const C3: f32 = 18.6875;
    let y = (nits / 10000.0).clamp(0.0, 1.0).powf(M1);
    ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
}

/// Content peak assumed when the stream carries no MaxCLL / mastering peak:
/// 1000 nits, the most common HDR10 grade and HLG's nominal display.
pub const DEFAULT_HDR_PEAK_NITS: f32 = 1000.0;

/// The display peak an SDR output is tone mapped to: reference white.
/// BT.2408 puts SDR's 100% at 203 nits, so an SDR display's white IS the
/// HDR reference white, and everything above it is highlight to compress.
pub const SDR_PEAK_NITS: f32 = REFERENCE_WHITE_NITS;

/// BT.2390 EETF (ITU-R BT.2390-10, 5.4): map `nits` from content with peak
/// `src_peak` onto a display with peak `dst_peak`, in the PQ domain. Pure.
///
/// Below the knee `KS = 1.5 maxLum - 0.5` the signal passes unchanged, so
/// midtones and shadows are exactly as graded; above it a Hermite spline
/// rolls the rest of the content's range onto what the display has left.
/// Working in PQ (perceptual) rather than linear light is why highlights
/// keep their gradation: the old knee, in linear light, gave 203..4000 nits
/// about 11 output codes.
pub fn bt2390(nits: f32, src_peak: f32, dst_peak: f32) -> f32 {
    if src_peak <= dst_peak {
        return nits.min(dst_peak);
    }
    let src_max = pq_oetf(src_peak);
    let e1 = pq_oetf(nits) / src_max;
    let max_lum = pq_oetf(dst_peak) / src_max;
    let ks = 1.5 * max_lum - 0.5;
    let e2 = if e1 < ks {
        e1
    } else {
        let t = ((e1 - ks) / (1.0 - ks)).min(1.0);
        let (t2, t3) = (t * t, t * t * t);
        (2.0 * t3 - 3.0 * t2 + 1.0) * ks + (t3 - 2.0 * t2 + t) * (1.0 - ks) + (-2.0 * t3 + 3.0 * t2) * max_lum
    };
    pq_eotf(e2 * src_max) * 10000.0
}

/// sRGB encoding of linear light 0..1.
pub fn srgb_oetf(l: f32) -> f32 {
    let l = l.clamp(0.0, 1.0);
    if l <= 0.0031308 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// HDR R'G'B' (non-linear, as decoded) to SDR sRGB-encoded RGB. The one
/// definition every path mirrors (GLSL in vidkit-egui, WGSL in vidkit-vk).
///
/// 1. EOTF to linear light with ITU reference white (203 nits) at 1.0: PQ
///    from absolute nits; HLG through its OOTF (system gamma 1.2 at a
///    1000-nit nominal peak), which puts its reference white (75% signal)
///    at 1.0 too.
/// 2. BT.2020 -> BT.709 primaries in linear light, when `bt2020`.
/// 3. The soft-knee tonemap on the MAX channel, scaling all three by the
///    same factor: per-channel roll-off shifts the hue of bright saturated
///    colours (a bright orange drifts to yellow).
/// 4. sRGB OETF. This step was missing: linear light went straight into a
///    gamma-encoded buffer, and 18% grey rendered at 43/255 instead of ~118.
pub fn hdr_to_sdr(rgb: [f32; 3], transfer: Transfer, bt2020: bool) -> [f32; 3] {
    hdr_to_sdr_peak(rgb, transfer, bt2020, DEFAULT_HDR_PEAK_NITS)
}

/// [`hdr_to_sdr`] with the content's peak, from MaxCLL or the mastering
/// display when the stream has them.
pub fn hdr_to_sdr_peak(rgb: [f32; 3], transfer: Transfer, bt2020: bool, src_peak: f32) -> [f32; 3] {
    // Absolute nits first; the EETF works on nits.
    let (nits, src_peak) = match transfer {
        Transfer::Pq => (rgb.map(|c| pq_eotf(c) * 10000.0), src_peak),
        Transfer::Hlg => {
            let scene = rgb.map(hlg_eotf);
            // OOTF on luminance: Yd = Ys^1.2, applied as a common scale so
            // hue is kept. BT.2020 luma weights. HLG's peak IS its nominal
            // display, whatever MaxCLL says.
            let ys = 0.2627 * scene[0] + 0.6780 * scene[1] + 0.0593 * scene[2];
            let k = if ys > 0.0 { ys.powf(0.2) } else { 0.0 };
            (scene.map(|c| c * k * HLG_PEAK_NITS), HLG_PEAK_NITS)
        }
        Transfer::Sdr => return rgb,
    };
    let lin = if bt2020 {
        let m = BT2020_TO_BT709;
        [0, 1, 2].map(|r| (m[r][0] * nits[0] + m[r][1] * nits[1] + m[r][2] * nits[2]).max(0.0))
    } else {
        nits.map(|c| c.max(0.0))
    };
    // The EETF on the MAX channel, one scale for all three (hue kept), out
    // to display-relative linear light (1.0 = the SDR display's white).
    let peak = lin[0].max(lin[1]).max(lin[2]);
    let scale = if peak > 0.0 { bt2390(peak, src_peak, SDR_PEAK_NITS) / peak } else { 0.0 };
    lin.map(|c| srgb_oetf(c * scale / SDR_PEAK_NITS))
}

/// Sample a plane with bilinear filtering at normalised coordinates, matching
/// what `textureSampleLevel` does in the shader.
///
/// The chroma planes are half resolution, and the GPU sampler upscales them for
/// free. Doing nearest-neighbour here instead would be cheaper and would
/// disagree with the shader on every chroma edge, which is exactly the drift
/// the parity test is looking for.
/// How a plane's samples are stored.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SampleFormat {
    /// One byte per sample.
    U8,
    /// Two bytes little-endian, 10 significant bits in the HIGH bits — P010's
    /// layout. The value is already left-shifted, so dividing by 65535 gives
    /// the right normalised result with no correction.
    U16HighBits,
    /// Two bytes little-endian, 10 significant bits in the LOW bits —
    /// I420_10LE. Dividing by 65535 here would render the picture at about a
    /// sixteenth brightness, which reads as "the decoder is broken" rather
    /// than as a scaling bug. Divide by 1023.
    U16LowBits10,
}

impl SampleFormat {
    /// Limited-range constants in THIS format's normalisation: (black, luma
    /// scale, chroma centre, chroma scale). 8-bit is 16/219/128/224; 10-bit
    /// is 64/876/512/896. Using the 8-bit set on 10-bit data put neutral
    /// chroma at 0.50196 instead of 0.5 -- invisible in SDR, but PQ's steep
    /// curve made it a ~5% green cast on every 10-bit HDR frame (a neutral
    /// 203-nit grey came out R 192.7, G 201.9, B 191.0 nits).
    pub fn limited(self) -> (f32, f32, f32, f32) {
        // The normalised value of a 10-bit code under each layout's read().
        let (unit, max) = match self {
            SampleFormat::U8 => return (16.0 / 255.0, 255.0 / 219.0, 128.0 / 255.0, 255.0 / 224.0),
            SampleFormat::U16HighBits => (64.0 / 65535.0, 65535.0 / 64.0),
            SampleFormat::U16LowBits10 => (1.0 / 1023.0, 1023.0),
        };
        (64.0 * unit, max / 876.0, 512.0 * unit, max / 896.0)
    }

    /// Full-range chroma centre in this format's normalisation.
    pub fn centre(self) -> f32 {
        match self {
            SampleFormat::U8 => 0.5,
            SampleFormat::U16HighBits => 512.0 * 64.0 / 65535.0,
            SampleFormat::U16LowBits10 => 512.0 / 1023.0,
        }
    }

    pub fn bytes(self) -> usize {
        match self {
            SampleFormat::U8 => 1,
            _ => 2,
        }
    }

    fn read(self, data: &[u8], byte_index: usize) -> f32 {
        match self {
            SampleFormat::U8 => data.get(byte_index).copied().unwrap_or(0) as f32 / 255.0,
            SampleFormat::U16HighBits => {
                let lo = data.get(byte_index).copied().unwrap_or(0) as u16;
                let hi = data.get(byte_index + 1).copied().unwrap_or(0) as u16;
                (lo | (hi << 8)) as f32 / 65535.0
            }
            SampleFormat::U16LowBits10 => {
                let lo = data.get(byte_index).copied().unwrap_or(0) as u16;
                let hi = data.get(byte_index + 1).copied().unwrap_or(0) as u16;
                // Mask: the top six bits are undefined padding in this layout,
                // and a decoder that leaves them set would otherwise blow the
                // value past 1.0 and clip every highlight.
                ((lo | (hi << 8)) & 0x03FF) as f32 / 1023.0
            }
        }
    }
}

fn sample_bilinear(
    data: &[u8],
    stride: usize,
    w: usize,
    h: usize,
    comps: usize,
    comp: usize,
    fmt: SampleFormat,
    u: f32,
    v: f32,
) -> f32 {
    if w == 0 || h == 0 {
        return 0.0;
    }
    // Texel-space coordinate of the sample point, offset to texel centres.
    let x = u * w as f32 - 0.5;
    let y = v * h as f32 - 0.5;
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;

    let clampi = |i: f32, n: usize| -> usize { (i.max(0.0) as usize).min(n.saturating_sub(1)) };
    let x0i = clampi(x0, w);
    let y0i = clampi(y0, h);
    let x1i = clampi(x0 + 1.0, w);
    let y1i = clampi(y0 + 1.0, h);

    let at = |xi: usize, yi: usize| -> f32 {
        let ix = yi * stride + (xi * comps + comp) * fmt.bytes();
        fmt.read(data, ix)
    };

    let top = at(x0i, y0i) * (1.0 - fx) + at(x1i, y0i) * fx;
    let bot = at(x0i, y1i) * (1.0 - fx) + at(x1i, y1i) * fx;
    top * (1.0 - fy) + bot * fy
}

thread_local! {
    /// Tests set this to run the per-pixel reference loop as the oracle.
    static REFERENCE_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with the per-pixel reference converter forced. For parity tests.
pub fn with_reference_converter<T>(f: impl FnOnce() -> T) -> T {
    REFERENCE_ONLY.with(|r| r.set(true));
    let out = f();
    REFERENCE_ONLY.with(|r| r.set(false));
    out
}

/// Bilinear sample positions along one axis, for every output coordinate:
/// `(i0, i1, f)` computed with EXACTLY `sample_bilinear`'s expressions, so a
/// sample built from them is bit-identical to the reference.
fn axis(n_out: usize, n_tex: usize) -> Vec<(usize, usize, f32)> {
    (0..n_out)
        .map(|p| {
            let u = (p as f32 + 0.5) / n_out as f32;
            let x = u * n_tex as f32 - 0.5;
            let x0 = x.floor();
            let f = x - x0;
            let clampi = |i: f32, n: usize| -> usize { (i.max(0.0) as usize).min(n.saturating_sub(1)) };
            (clampi(x0, n_tex), clampi(x0 + 1.0, n_tex), f)
        })
        .collect()
}

/// NV12 / P010 conversion, row-streaming. The same arithmetic as the
/// per-pixel reference, in the same order, so the bytes are identical; what
/// goes is the per-pixel coordinate maths (three floor/clamp bilinear
/// setups a pixel), done once per column and once per row instead.
///
/// Measured before (release, x86_64): ~52 ns a pixel -- 18.9 ms a 640x360
/// frame, 679 ms a 4K one -- against 0.3 ns a pixel for the damage tree's
/// hashing. The converter, not the temporal tracking, was the cost.
#[allow(clippy::too_many_arguments)]
fn fast_interleaved(
    planes: &CpuPlanes,
    fmt: SampleFormat,
    color: ColorInfo,
    coeffs: [[f32; 3]; 3],
    width: usize,
    height: usize,
    cw: usize,
    ch: usize,
    y_stride: usize,
    c_stride: usize,
    out: &mut [u8],
) {
    let bps = fmt.bytes();
    // 8-bit samples through a 256-entry table holding exactly v / 255.0:
    // bit-identical to the division, 1 KB, resident in L1 for the whole
    // frame, and no divide on the hot path (twelve reads a pixel).
    let lut: [f32; 256] = std::array::from_fn(|v| v as f32 / 255.0);
    let read = |data: &[u8], ix: usize| -> f32 {
        match fmt {
            SampleFormat::U8 => lut[data.get(ix).copied().unwrap_or(0) as usize],
            _ => fmt.read(data, ix),
        }
    };
    let (lx, ly) = (axis(width, width), axis(height, height));
    let (cx, cy) = (axis(width, cw), axis(height, ch));
    let (luma, chroma) = (&planes.data[0][..], &planes.data[1][..]);
    let (black, ys, mid, cs) = fmt.limited();
    let centre = fmt.centre();
    let hdr = color.transfer != Transfer::Sdr;
    let bt2020 = color.matrix == Matrix::Bt2020Ncl;
    // The HORIZONTAL half of each chroma sample -- `c(x0)*(1-fx) + c(x1)*fx`
    // on one chroma row -- depends only on that row and the output column,
    // and consecutive output rows share chroma rows. So each chroma row is
    // blended horizontally ONCE, in one sequential pass, into a cached row
    // of (cb, cr) pairs, and the output rows that use it read it back
    // sequentially. Same floats, same order as the reference: bit-identical.
    let hrow = |r: usize, buf: &mut Vec<f32>| {
        buf.clear();
        let base = r * c_stride;
        for &(cx0, cx1, cfx) in cx.iter() {
            for comp in 0..2 {
                let a = read(chroma, base + (cx0 * 2 + comp) * bps);
                let b = read(chroma, base + (cx1 * 2 + comp) * bps);
                buf.push(a * (1.0 - cfx) + b * cfx);
            }
        }
    };
    // Two slots: the chroma rows the current output row blends between.
    let mut cache: [(usize, Vec<f32>); 2] = [(usize::MAX, Vec::with_capacity(width * 2)), (usize::MAX, Vec::with_capacity(width * 2))];
    let _ = ch;
    for py in 0..height {
        let (ly0, ly1, lfy) = ly[py];
        let (cy0, cy1, cfy) = cy[py];
        let (lr0, lr1) = (ly0 * y_stride, ly1 * y_stride);
        // Fill the slots for rows cy0 and cy1, reusing whichever is cached.
        for want in [cy0, cy1] {
            if cache[0].0 != want && cache[1].0 != want {
                // Evict the slot NOT holding the other row we need.
                let keep = if want == cy0 { cy1 } else { cy0 };
                let slot = if cache[0].0 == keep { 1 } else { 0 };
                hrow(want, &mut cache[slot].1);
                cache[slot].0 = want;
            }
        }
        let h0 = if cache[0].0 == cy0 { &cache[0].1 } else { &cache[1].1 };
        let h1 = if cache[0].0 == cy1 { &cache[0].1 } else { &cache[1].1 };
        let row = &mut out[py * width * 4..(py + 1) * width * 4];
        for (px, o) in row.chunks_exact_mut(4).enumerate() {
            let (lx0, lx1, lfx) = lx[px];
            let at = |base: usize, xi: usize| read(luma, base + xi * bps);
            // Where both weights are EXACTLY zero -- 1:1 output, which is
            // what the player asks for -- the bilinear blend is a*1 + b*0 = a
            // in IEEE arithmetic, so reading one texel is bit-identical.
            let y_raw = if lfx == 0.0 && lfy == 0.0 {
                at(lr0, lx0)
            } else {
                let top = at(lr0, lx0) * (1.0 - lfx) + at(lr0, lx1) * lfx;
                let bot = at(lr1, lx0) * (1.0 - lfx) + at(lr1, lx1) * lfx;
                top * (1.0 - lfy) + bot * lfy
            };

            let samp = |comp: usize| h0[px * 2 + comp] * (1.0 - cfy) + h1[px * 2 + comp] * cfy;
            let (cb_raw, cr_raw) = (samp(0), samp(1));

            let (yv, cb, cr) = match color.range {
                Range::Limited => ((y_raw - black) * ys, (cb_raw - mid) * cs, (cr_raw - mid) * cs),
                Range::Full => (y_raw, cb_raw - centre, cr_raw - centre),
            };
            let mut rgb = [
                coeffs[0][0] * yv + coeffs[0][1] * cb + coeffs[0][2] * cr,
                coeffs[1][0] * yv + coeffs[1][1] * cb + coeffs[1][2] * cr,
                coeffs[2][0] * yv + coeffs[2][1] * cb + coeffs[2][2] * cr,
            ];
            if hdr {
                rgb = hdr_to_sdr(rgb, color.transfer, bt2020);
            }
            for k in 0..3 {
                o[k] = (rgb[k].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            o[3] = 255;
        }
    }
}

/// Chroma plane dimensions for a layout at a given luma size.
fn chroma_dims(layout: PlaneLayout, w: usize, h: usize) -> (usize, usize) {
    match layout {
        // 4:2:0, rounded up: odd dimensions are legal and truncating loses the
        // last row and column of chroma.
        PlaneLayout::Nv12 | PlaneLayout::P010 | PlaneLayout::I420 | PlaneLayout::I420_10le => {
            (w.div_ceil(2), h.div_ceil(2))
        }
    }
}

#[derive(Debug)]
pub enum ConvertError {
    /// The frame is a DMA-BUF; this converter only handles CPU planes.
    NotCpuPlanes,
    /// 10-bit CPU planes are not handled yet, because nothing produces them on
    /// the software-decode path we have. Refusing beats silently reading the
    /// low byte of every sample and rendering a noise field.
    UnsupportedDepth(PlaneLayout),
    /// Plane count did not match the layout.
    PlaneCount { want: usize, got: usize },
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvertError::NotCpuPlanes => write!(f, "frame is a DMA-BUF, not CPU planes"),
            ConvertError::UnsupportedDepth(l) => write!(f, "layout {l:?} is not 8-bit"),
            ConvertError::PlaneCount { want, got } => {
                write!(f, "expected {want} planes, got {got}")
            }
        }
    }
}

impl std::error::Error for ConvertError {}

/// Convert one frame to tightly packed RGBA8, row-major, top-left origin —
/// the layout `egui::ColorImage` wants.
pub fn frame_to_rgba8(frame: &Frame) -> Result<Vec<u8>, ConvertError> {
    let planes = match &frame.data {
        FrameData::Cpu(p) => p,
        FrameData::DmaBuf(_) => return Err(ConvertError::NotCpuPlanes),
    };
    planes_to_rgba8(
        planes,
        frame.layout,
        frame.color,
        frame.width as usize,
        frame.height as usize,
    )
}

/// [`frame_to_rgba8`] into a caller-owned buffer.
pub fn frame_to_rgba8_into(frame: &Frame, out: &mut Vec<u8>) -> Result<(), ConvertError> {
    let planes = match &frame.data {
        FrameData::Cpu(p) => p,
        FrameData::DmaBuf(_) => return Err(ConvertError::NotCpuPlanes),
    };
    planes_to_rgba8_into(
        planes,
        frame.layout,
        frame.color,
        frame.width as usize,
        frame.height as usize,
        out,
    )
}

/// The conversion proper, split out so tests can drive it without building a
/// whole `Frame`.
pub fn planes_to_rgba8(
    planes: &CpuPlanes,
    layout: PlaneLayout,
    color: ColorInfo,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, ConvertError> {
    let mut out = Vec::new();
    planes_to_rgba8_into(planes, layout, color, width, height, &mut out)?;
    Ok(out)
}

/// The same conversion, writing into a caller-owned buffer.
///
/// A player converts a frame every 16 ms forever. Returning a fresh `Vec` each
/// time means allocating and freeing 8 MB per frame at 1080p — 500 MB/s of
/// allocator traffic that does no work, and on a phone that is heat. The buffer
/// is resized once and reused after that; `out.capacity()` stops growing after
/// the first frame, which is what `the_scratch_buffer_is_reused` asserts.
pub fn planes_to_rgba8_into(
    planes: &CpuPlanes,
    layout: PlaneLayout,
    color: ColorInfo,
    width: usize,
    height: usize,
    out: &mut Vec<u8>,
) -> Result<(), ConvertError> {
    let want = layout.plane_count();
    if planes.data.len() < want {
        return Err(ConvertError::PlaneCount {
            want,
            got: planes.data.len(),
        });
    }

    // The bit-depth gotcha, in one place: P010 puts its 10 bits in the HIGH
    // bits of each 16-bit sample and I420_10LE in the LOW bits. Reading either
    // with the other's scale is a picture that is uniformly too dark or
    // blindingly clipped, and it looks like a decoder fault rather than a
    // scaling one.
    let fmt = match layout {
        PlaneLayout::Nv12 | PlaneLayout::I420 => SampleFormat::U8,
        PlaneLayout::P010 => SampleFormat::U16HighBits,
        PlaneLayout::I420_10le => SampleFormat::U16LowBits10,
    };

    let (cw, ch) = chroma_dims(layout, width, height);
    let coeffs = matrix_coeffs(color.matrix);
    let interleaved_chroma = matches!(layout, PlaneLayout::Nv12 | PlaneLayout::P010);

    // Strides are in BYTES, so the fallbacks scale with the sample size. A
    // 10-bit plane whose stride is guessed as its pixel width is read at half
    // scale and skews every row after the first.
    let bps = fmt.bytes();
    let y_stride = planes
        .strides
        .first()
        .copied()
        .unwrap_or((width * bps) as u32) as usize;
    let c0_stride = planes.strides.get(1).copied().unwrap_or(if interleaved_chroma {
        (cw * 2 * bps) as u32
    } else {
        (cw * bps) as u32
    }) as usize;
    let c1_stride = planes.strides.get(2).copied().unwrap_or((cw * bps) as u32) as usize;

    out.clear();
    out.resize(width * height * 4, 0);

    if interleaved_chroma && !REFERENCE_ONLY.with(|r| r.get()) {
        fast_interleaved(planes, fmt, color, coeffs, width, height, cw, ch, y_stride, c0_stride, out);
        return Ok(());
    }

    for py in 0..height {
        for px in 0..width {
            // Same normalised sample point the shader computes, so the bilinear
            // chroma upscale lands identically.
            let u = (px as f32 + 0.5) / width as f32;
            let v = (py as f32 + 0.5) / height as f32;

            let y_raw = sample_bilinear(&planes.data[0], y_stride, width, height, 1, 0, fmt, u, v);
            let (cb_raw, cr_raw) = if interleaved_chroma {
                (
                    sample_bilinear(&planes.data[1], c0_stride, cw, ch, 2, 0, fmt, u, v),
                    sample_bilinear(&planes.data[1], c0_stride, cw, ch, 2, 1, fmt, u, v),
                )
            } else {
                (
                    sample_bilinear(&planes.data[1], c0_stride, cw, ch, 1, 0, fmt, u, v),
                    sample_bilinear(&planes.data[2], c1_stride, cw, ch, 1, 0, fmt, u, v),
                )
            };

            let (yv, cb, cr) = match color.range {
                Range::Limited => {
                    let (black, ys, mid, cs) = fmt.limited();
                    ((y_raw - black) * ys, (cb_raw - mid) * cs, (cr_raw - mid) * cs)
                }
                Range::Full => (y_raw, cb_raw - fmt.centre(), cr_raw - fmt.centre()),
            };

            let mut rgb = [
                coeffs[0][0] * yv + coeffs[0][1] * cb + coeffs[0][2] * cr,
                coeffs[1][0] * yv + coeffs[1][1] * cb + coeffs[1][2] * cr,
                coeffs[2][0] * yv + coeffs[2][1] * cb + coeffs[2][2] * cr,
            ];

            // SDR stays gamma-encoded: the swapchain is non-linear sRGB and
            // converting to linear and back would only add rounding.
            if color.transfer != Transfer::Sdr {
                rgb = hdr_to_sdr(rgb, color.transfer, color.matrix == Matrix::Bt2020Ncl);
            }

            let o = (py * width + px) * 4;
            for c in 0..3 {
                out[o + c] = (rgb[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            out[o + 3] = 255;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planes_nv12(y: u8, cb: u8, cr: u8) -> CpuPlanes {
        CpuPlanes {
            data: vec![vec![y; 4], vec![cb, cr]],
            strides: vec![2, 2],
        }
    }

    fn convert(p: &CpuPlanes, layout: PlaneLayout, color: ColorInfo) -> Vec<u8> {
        planes_to_rgba8(p, layout, color, 2, 2).unwrap()
    }

    fn sdr709(range: Range) -> ColorInfo {
        ColorInfo {
            matrix: Matrix::Bt709,
            transfer: Transfer::Sdr,
            range,
        }
    }

    #[test]
    fn limited_range_black_and_white() {
        let c = sdr709(Range::Limited);
        let black = convert(&planes_nv12(16, 128, 128), PlaneLayout::Nv12, c);
        assert_eq!(&black[0..4], &[0, 0, 0, 255]);
        let white = convert(&planes_nv12(235, 128, 128), PlaneLayout::Nv12, c);
        assert_eq!(&white[0..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn full_range_differs_from_limited() {
        let lim = convert(&planes_nv12(16, 128, 128), PlaneLayout::Nv12, sdr709(Range::Limited));
        let full = convert(&planes_nv12(16, 128, 128), PlaneLayout::Nv12, sdr709(Range::Full));
        assert_eq!(lim[0], 0);
        // A range, not a point: 16/255 scaled back by 255 lands on 16.5 in
        // f32, so the rounding can go either way. Both directions are
        // failures -- 0 would mean the range flag was ignored, and anything
        // near 128 would mean the wrong expansion ran.
        assert!(
            (15..=18).contains(&full[0]),
            "full-range 16 should stay dark grey, got {}",
            full[0]
        );
    }

    #[test]
    fn bt709_red() {
        let out = convert(&planes_nv12(63, 102, 240), PlaneLayout::Nv12, sdr709(Range::Limited));
        assert!(out[0] > 250, "red channel {}", out[0]);
        assert!(out[1] < 8 && out[2] < 8, "leak: {:?}", &out[0..3]);
    }

    #[test]
    fn nv12_and_i420_agree() {
        let c = sdr709(Range::Limited);
        let a = convert(&planes_nv12(120, 90, 200), PlaneLayout::Nv12, c);
        let i = CpuPlanes {
            data: vec![vec![120u8; 4], vec![90], vec![200]],
            strides: vec![2, 1, 1],
        };
        let b = convert(&i, PlaneLayout::I420, c);
        assert_eq!(a, b);
    }

    /// Cb and Cr are not interchangeable; swapping them turns skin tones green.
    #[test]
    fn cb_cr_are_not_interchangeable() {
        let c = sdr709(Range::Limited);
        let a = convert(&planes_nv12(120, 90, 200), PlaneLayout::Nv12, c);
        let b = convert(&planes_nv12(120, 200, 90), PlaneLayout::Nv12, c);
        assert_ne!(a, b);
    }

    #[test]
    fn every_matrix_keeps_neutral_chroma_grey() {
        for matrix in [Matrix::Bt601, Matrix::Bt709, Matrix::Bt2020Ncl] {
            let c = ColorInfo {
                matrix,
                transfer: Transfer::Sdr,
                range: Range::Limited,
            };
            let out = convert(&planes_nv12(126, 128, 128), PlaneLayout::Nv12, c);
            assert_eq!(out[0], out[1], "{matrix:?} tinted");
            assert_eq!(out[1], out[2], "{matrix:?} tinted");
        }
    }

    /// A player converts a frame every 16 ms forever, so the buffer must be
    /// reused rather than reallocated. Capacity is the observable: it settles
    /// after the first frame and never grows again.
    #[test]
    fn the_scratch_buffer_is_reused() {
        let c = sdr709(Range::Limited);
        let p = planes_nv12(126, 128, 128);
        // Deliberately over-allocated. A 2x2 frame needs 16 bytes, and a
        // reallocating implementation lands on capacity 16 too -- identical to
        // a reusing one, so the check cannot bind. Starting large means reuse
        // KEEPS the capacity and reallocation drops it.
        let mut buf = Vec::with_capacity(4096);

        planes_to_rgba8_into(&p, PlaneLayout::Nv12, c, 2, 2, &mut buf).unwrap();
        let settled = buf.capacity();
        assert!(
            settled >= 4096,
            "capacity fell to {settled}: the buffer was reallocated, not reused"
        );

        for _ in 0..50 {
            planes_to_rgba8_into(&p, PlaneLayout::Nv12, c, 2, 2, &mut buf).unwrap();
            assert_eq!(
                buf.capacity(),
                settled,
                "the buffer reallocated: 50 frames means 50 allocations"
            );
        }
        assert_eq!(buf.len(), 2 * 2 * 4, "output length is still correct");
    }

    /// The in-place form must produce exactly what the allocating one does,
    /// or the fast path quietly renders something else.
    #[test]
    fn in_place_and_allocating_conversion_agree() {
        let c = sdr709(Range::Limited);
        let p = planes_nv12(120, 90, 200);
        let owned = planes_to_rgba8(&p, PlaneLayout::Nv12, c, 2, 2).unwrap();
        let mut buf = vec![0xAAu8; 999]; // deliberately dirty and wrong-sized
        planes_to_rgba8_into(&p, PlaneLayout::Nv12, c, 2, 2, &mut buf).unwrap();
        assert_eq!(owned, buf, "in-place output differs, or stale bytes survived");
    }

    #[test]
    fn output_is_tightly_packed_rgba_with_opaque_alpha() {
        let out = convert(&planes_nv12(126, 128, 128), PlaneLayout::Nv12, sdr709(Range::Limited));
        assert_eq!(out.len(), 2 * 2 * 4);
        for px in out.chunks(4) {
            assert_eq!(px[3], 255);
        }
    }

    /// A 10-bit sample written into a P010 plane: value in the HIGH bits.
    fn p010_planes(y: u16, cb: u16, cr: u16) -> CpuPlanes {
        let hi = |v: u16| -> [u8; 2] {
            let shifted = v << 6; // 10 bits into the top of 16
            [(shifted & 0xFF) as u8, (shifted >> 8) as u8]
        };
        let mut luma = Vec::new();
        for _ in 0..4 {
            luma.extend_from_slice(&hi(y));
        }
        let mut chroma = Vec::new();
        chroma.extend_from_slice(&hi(cb));
        chroma.extend_from_slice(&hi(cr));
        CpuPlanes {
            data: vec![luma, chroma],
            strides: vec![4, 4],
        }
    }

    /// The same values in I420_10LE: value in the LOW bits, three planes.
    fn i420_10_planes(y: u16, cb: u16, cr: u16) -> CpuPlanes {
        let lo = |v: u16| -> [u8; 2] { [(v & 0xFF) as u8, (v >> 8) as u8] };
        let mut luma = Vec::new();
        for _ in 0..4 {
            luma.extend_from_slice(&lo(y));
        }
        CpuPlanes {
            data: vec![luma, lo(cb).to_vec(), lo(cr).to_vec()],
            strides: vec![4, 2, 2],
        }
    }

    /// 10-bit limited range: black is 64, white is 940 (16 and 235 scaled by 4).
    #[test]
    fn p010_black_and_white_land_on_zero_and_one() {
        let c = sdr709(Range::Limited);
        let black = planes_to_rgba8(&p010_planes(64, 512, 512), PlaneLayout::P010, c, 2, 2).unwrap();
        assert_eq!(&black[0..3], &[0, 0, 0], "10-bit black is not black");
        let white =
            planes_to_rgba8(&p010_planes(940, 512, 512), PlaneLayout::P010, c, 2, 2).unwrap();
        assert!(
            white[0] > 252 && white[1] > 252 && white[2] > 252,
            "10-bit white came out {:?}",
            &white[0..3]
        );
    }

    /// The gotcha this whole SampleFormat split exists for: the same 10-bit
    /// values stored in the two layouts must produce the SAME picture.
    ///
    /// Reading I420_10LE with P010's scale renders it at about a sixteenth
    /// brightness -- a near-black frame that looks like a broken decoder rather
    /// than a scaling bug.
    #[test]
    fn p010_and_i420_10le_agree_on_the_same_values() {
        let c = sdr709(Range::Limited);
        for (y, cb, cr) in [(64u16, 512u16, 512u16), (940, 512, 512), (500, 400, 700)] {
            let a =
                planes_to_rgba8(&p010_planes(y, cb, cr), PlaneLayout::P010, c, 2, 2).unwrap();
            let b = planes_to_rgba8(
                &i420_10_planes(y, cb, cr),
                PlaneLayout::I420_10le,
                c,
                2,
                2,
            )
            .unwrap();
            for i in 0..3 {
                assert!(
                    (a[i] as i32 - b[i] as i32).abs() <= 1,
                    "layouts disagree at ({y},{cb},{cr}) channel {i}: P010 {} vs I420_10LE {}",
                    a[i],
                    b[i]
                );
            }
        }
    }

    /// 10-bit and 8-bit must agree on the same colour, or a stream that
    /// switches representation shifts hue mid-playback.
    #[test]
    fn ten_bit_matches_eight_bit_for_the_same_colour() {
        let c = sdr709(Range::Limited);
        let eight = convert(&planes_nv12(126, 128, 128), PlaneLayout::Nv12, c);
        let ten =
            planes_to_rgba8(&p010_planes(126 * 4, 128 * 4, 128 * 4), PlaneLayout::P010, c, 2, 2)
                .unwrap();
        for i in 0..3 {
            assert!(
                (eight[i] as i32 - ten[i] as i32).abs() <= 2,
                "8-bit {} vs 10-bit {} on channel {i}",
                eight[i],
                ten[i]
            );
        }
    }

    /// I420_10LE's top six bits are undefined padding. A decoder that leaves
    /// them set must not blow the value past 1.0 and clip every highlight.
    #[test]
    fn undefined_high_bits_in_i420_10le_are_masked_off() {
        let c = sdr709(Range::Limited);
        let clean = planes_to_rgba8(&i420_10_planes(500, 512, 512), PlaneLayout::I420_10le, c, 2, 2)
            .unwrap();
        let dirty = planes_to_rgba8(
            &i420_10_planes(500 | 0xFC00, 512 | 0xFC00, 512 | 0xFC00),
            PlaneLayout::I420_10le,
            c,
            2,
            2,
        )
        .unwrap();
        assert_eq!(&clean[0..3], &dirty[0..3], "padding bits reached the picture");
    }

    #[test]
    fn a_short_plane_list_is_an_error() {
        let p = CpuPlanes {
            data: vec![vec![126u8; 4]],
            strides: vec![2],
        };
        assert!(matches!(
            planes_to_rgba8(&p, PlaneLayout::I420, sdr709(Range::Limited), 2, 2),
            Err(ConvertError::PlaneCount { .. })
        ));
    }

    /// Strides are not widths. A padded plane read as if tightly packed skews
    /// every row after the first.
    #[test]
    fn a_padded_luma_stride_is_honoured() {
        let c = sdr709(Range::Limited);
        // 2x2 luma in a plane with stride 4: row 0 is [235,235,pad,pad],
        // row 1 is [16,16,pad,pad].
        let padded = CpuPlanes {
            data: vec![vec![235, 235, 7, 7, 16, 16, 7, 7], vec![128, 128]],
            strides: vec![4, 2],
        };
        let out = planes_to_rgba8(&padded, PlaneLayout::Nv12, c, 2, 2).unwrap();
        // Top row white, bottom row black. Bilinear sampling at texel centres
        // of a 2x2 image hits the texels exactly.
        assert!(out[0] > 250, "top-left should be white, got {}", out[0]);
        let bottom_left = (1 * 2 + 0) * 4;
        assert!(
            out[bottom_left] < 5,
            "bottom-left should be black, got {}",
            out[bottom_left]
        );
    }

    /// PQ code for `nits` absolute. Inverse of pq_eotf, for fixtures.
    fn pq_encode(nits: f32) -> f32 {
        const M1: f32 = 0.1593017578125;
        const M2: f32 = 78.84375;
        const C1: f32 = 0.8359375;
        const C2: f32 = 18.8515625;
        const C3: f32 = 18.6875;
        let y = (nits / 10000.0).max(0.0).powf(M1);
        ((C1 + C2 * y) / (1.0 + C3 * y)).powf(M2)
    }

    /// Render a flat PQ grey of `nits` through the CPU path (10-bit P010,
    /// BT.2020, limited range) and return the 8-bit output.
    fn pq_grey_out(nits: f32) -> u8 {
        let e = pq_encode(nits);
        let code = (64.0 + e * 876.0).round() as u16; // limited-range 10-bit luma
        let c = ColorInfo { matrix: Matrix::Bt2020Ncl, transfer: Transfer::Pq, range: Range::Limited };
        let out = planes_to_rgba8(&p010_planes(code, 512, 512), PlaneLayout::P010, c, 2, 2).unwrap();
        out[0]
    }

    /// BT.2408: HDR reference white is 203 nits and should render as SDR
    /// white; 18% grey at that exposure (36.5 nits) is linear 0.18, which
    /// the display's sRGB curve encodes at ~118/255. The path wrote linear
    /// light into a gamma-encoded buffer, so midtones came out far too dark;
    /// only reference white was ever pinned.
    #[test]
    fn pq_midtones_land_where_bt2408_puts_them() {
        let grey = pq_grey_out(0.18 * 203.0);
        assert!((115..=121).contains(&grey), "18% grey rendered at {grey}/255, want ~118");
        // Reference white: 229 under BT.2390 onto a 203-nit SDR white (was
        // 244 under the old knee). The EETF's knee for 1000-nit content sits
        // near 94 nits, so diffuse white shares the top of the range with
        // the highlights -- ~6% dimmer, in exchange for their gradation.
        let white = pq_grey_out(203.0);
        assert!(white >= 225, "reference white at {white}/255");
        // Highlights stay ordered and distinct (without the roll-off 600
        // and 4000 nits both clip to 255). The knee curve leaves only ~11
        // codes for 203..4000 nits -- the "placeholder" the WGSL notes admit;
        // a display-referred curve (BT.2390) is the upgrade.
        let (a, b) = (pq_grey_out(600.0), pq_grey_out(4000.0));
        assert!(white < a && a < b, "{white} {a} {b}");
        assert!(pq_grey_out(0.0) <= 1, "black stays black");
    }

    /// A 10-bit neutral grey is neutral, in SDR and -- where the old 8-bit
    /// constants made it visibly green -- in PQ.
    #[test]
    fn ten_bit_neutral_grey_stays_neutral() {
        for transfer in [Transfer::Sdr, Transfer::Pq, Transfer::Hlg] {
            for code in [64u16, 300, 573, 800, 940] {
                let c = ColorInfo { matrix: Matrix::Bt2020Ncl, transfer, range: Range::Limited };
                let o = planes_to_rgba8(&p010_planes(code, 512, 512), PlaneLayout::P010, c, 2, 2).unwrap();
                let spread = o[0].max(o[1]).max(o[2]) - o[0].min(o[1]).min(o[2]);
                assert!(spread <= 1, "{transfer:?} code {code}: {:?}", &o[..3]);
            }
        }
        // And 10-bit limited black and white hit the ends exactly in SDR.
        let c = ColorInfo { matrix: Matrix::Bt709, transfer: Transfer::Sdr, range: Range::Limited };
        assert_eq!(planes_to_rgba8(&p010_planes(64, 512, 512), PlaneLayout::P010, c, 2, 2).unwrap()[0], 0);
        assert_eq!(planes_to_rgba8(&p010_planes(940, 512, 512), PlaneLayout::P010, c, 2, 2).unwrap()[0], 255);
    }

    /// The point of BT.2390 over the old linear-light knee: highlights keep
    /// gradation. Codes spent between reference white and the content peak.
    #[test]
    fn bt2390_gives_highlights_their_gradation() {
        let codes: Vec<u8> = [203.0, 300.0, 400.0, 600.0, 800.0, 1000.0].iter().map(|n| pq_grey_out(*n)).collect();
        eprintln!("DIAG 203..1000 nits -> {codes:?}");
        // Distinct up to 800 nits; the spline's slope is zero at the content
        // peak, so 800..1000 meet display white together.
        assert!(codes[..5].windows(2).all(|w| w[0] < w[1]), "each step distinct: {codes:?}");
        let span = codes[codes.len() - 1] - codes[0];
        assert!(span >= 20, "only {span} codes from 203 to 1000 nits");
        assert!(codes[5] >= 253, "content peak reaches display white: {}", codes[5]);
        // Past the content peak there is nothing left to map: it clips.
        assert!(pq_grey_out(4000.0) >= codes[5]);
    }

    #[test]
    fn the_eetf_passes_shadows_and_midtones_untouched_and_is_monotonic() {
        for n in [0.0, 1.0, 10.0, 36.5, 80.0] {
            assert!((bt2390(n, 1000.0, 203.0) - n).abs() < 0.05 * n.max(1.0), "{n} moved to {}", bt2390(n, 1000.0, 203.0));
        }
        let mut last = 0.0;
        for i in 0..=100 {
            let n = i as f32 * 10.0;
            let o = bt2390(n, 1000.0, 203.0);
            assert!(o >= last - 1e-3 && o <= 203.0 + 0.05, "{n} -> {o}");
            last = o;
        }
        assert!((bt2390(1000.0, 1000.0, 203.0) - 203.0).abs() < 1.0, "peak onto peak");
        assert_eq!(bt2390(150.0, 180.0, 203.0), 150.0, "content under the display peak is left alone");
    }

    /// The row-streaming fast path against the per-pixel reference: the SAME
    /// bytes, for every layout it takes, matrix, range and transfer, at odd
    /// and even sizes. Anything less and the GPU parity tests (which use this
    /// converter as ground truth) would be checking against a moved target.
    #[test]
    fn the_fast_path_is_byte_identical_to_the_reference() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut compared = 0usize;
        for &(w, h) in &[(2usize, 2usize), (7, 5), (16, 9), (33, 17), (64, 36)] {
            for layout in [PlaneLayout::Nv12, PlaneLayout::P010] {
                let bps = if layout == PlaneLayout::P010 { 2 } else { 1 };
                let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
                let mut luma = vec![0u8; w * h * bps];
                let mut chroma = vec![0u8; cw * ch * 2 * bps];
                for b in luma.iter_mut().chain(chroma.iter_mut()) {
                    *b = rnd() as u8;
                }
                if bps == 2 {
                    // P010: low 6 bits zero.
                    for pair in luma.chunks_exact_mut(2).chain(chroma.chunks_exact_mut(2)) {
                        pair[0] &= 0xC0;
                    }
                }
                let planes = CpuPlanes { data: vec![luma, chroma], strides: vec![(w * bps) as u32, (cw * 2 * bps) as u32] };
                for matrix in [Matrix::Bt601, Matrix::Bt709, Matrix::Bt2020Ncl] {
                    for range in [Range::Limited, Range::Full] {
                        for transfer in [Transfer::Sdr, Transfer::Pq, Transfer::Hlg] {
                            let c = ColorInfo { matrix, transfer, range };
                            let fast = planes_to_rgba8(&planes, layout, c, w, h).unwrap();
                            let reference = with_reference_converter(|| planes_to_rgba8(&planes, layout, c, w, h).unwrap());
                            assert_eq!(fast, reference, "{layout:?} {w}x{h} {c:?}");
                            compared += fast.len();
                        }
                    }
                }
            }
        }
        assert!(compared > 100_000, "the sweep did not run: {compared}");
    }

    /// `cargo test --release -p vidkit-core --lib converter_bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn converter_bench() {
        for (w, h) in [(640usize, 360usize), (1920, 1080), (3840, 2160)] {
            let luma: Vec<u8> = (0..w * h).map(|i| (i * 7 % 251) as u8).collect();
            let chroma: Vec<u8> = (0..w.div_ceil(2) * h.div_ceil(2) * 2).map(|i| (i * 13 % 251) as u8).collect();
            let planes = CpuPlanes { data: vec![luma, chroma], strides: vec![w as u32, w as u32] };
            let c = ColorInfo::default();
            let reps = (20_000_000 / (w * h)).max(2);
            let time = |reference: bool| {
                let run = || { std::hint::black_box(planes_to_rgba8(&planes, PlaneLayout::Nv12, c, w, h).unwrap()); };
                let t = std::time::Instant::now();
                for _ in 0..reps {
                    if reference { with_reference_converter(run) } else { run() }
                }
                t.elapsed().as_secs_f64() / reps as f64 * 1e3
            };
            let (r, f) = (time(true), time(false));
            eprintln!("DIAG {w}x{h}: reference {r:.2} ms ({:.1} ns/px), fast {f:.2} ms ({:.1} ns/px), {:.1}x", r * 1e6 / (w * h) as f64, f * 1e6 / (w * h) as f64, r / f);
        }
    }
}
