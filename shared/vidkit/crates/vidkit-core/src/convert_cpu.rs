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

/// Soft-knee roll-off. Mirrors `tonemap`; see that function for why plain
/// Reinhard is wrong here (it puts reference white at 0.5).
fn tonemap(x: f32, peak: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let v = x / peak.max(1.0);
    if v <= KNEE {
        v
    } else {
        let e = v - KNEE;
        KNEE + (1.0 - KNEE) * (e / (e + (1.0 - KNEE)))
    }
}

/// Luminance the tonemap treats as SDR white, in nits. ITU reference white.
pub const REFERENCE_WHITE_NITS: f32 = 203.0;

/// Sample a plane with bilinear filtering at normalised coordinates, matching
/// what `textureSampleLevel` does in the shader.
///
/// The chroma planes are half resolution, and the GPU sampler upscales them for
/// free. Doing nearest-neighbour here instead would be cheaper and would
/// disagree with the shader on every chroma edge, which is exactly the drift
/// the parity test is looking for.
fn sample_bilinear(data: &[u8], stride: usize, w: usize, h: usize, comps: usize, comp: usize, u: f32, v: f32) -> f32 {
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
        let ix = yi * stride + xi * comps + comp;
        data.get(ix).copied().unwrap_or(0) as f32 / 255.0
    };

    let top = at(x0i, y0i) * (1.0 - fx) + at(x1i, y0i) * fx;
    let bot = at(x0i, y1i) * (1.0 - fx) + at(x1i, y1i) * fx;
    top * (1.0 - fy) + bot * fy
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

/// The conversion proper, split out so tests can drive it without building a
/// whole `Frame`.
pub fn planes_to_rgba8(
    planes: &CpuPlanes,
    layout: PlaneLayout,
    color: ColorInfo,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, ConvertError> {
    if matches!(layout, PlaneLayout::P010 | PlaneLayout::I420_10le) {
        return Err(ConvertError::UnsupportedDepth(layout));
    }
    let want = layout.plane_count();
    if planes.data.len() < want {
        return Err(ConvertError::PlaneCount {
            want,
            got: planes.data.len(),
        });
    }

    let (cw, ch) = chroma_dims(layout, width, height);
    let coeffs = matrix_coeffs(color.matrix);
    let interleaved_chroma = matches!(layout, PlaneLayout::Nv12 | PlaneLayout::P010);

    let y_stride = planes.strides.first().copied().unwrap_or(width as u32) as usize;
    let c0_stride = planes
        .strides
        .get(1)
        .copied()
        .unwrap_or(if interleaved_chroma { cw as u32 * 2 } else { cw as u32 })
        as usize;
    let c1_stride = planes.strides.get(2).copied().unwrap_or(cw as u32) as usize;

    let mut out = vec![0u8; width * height * 4];

    for py in 0..height {
        for px in 0..width {
            // Same normalised sample point the shader computes, so the bilinear
            // chroma upscale lands identically.
            let u = (px as f32 + 0.5) / width as f32;
            let v = (py as f32 + 0.5) / height as f32;

            let y_raw = sample_bilinear(&planes.data[0], y_stride, width, height, 1, 0, u, v);
            let (cb_raw, cr_raw) = if interleaved_chroma {
                (
                    sample_bilinear(&planes.data[1], c0_stride, cw, ch, 2, 0, u, v),
                    sample_bilinear(&planes.data[1], c0_stride, cw, ch, 2, 1, u, v),
                )
            } else {
                (
                    sample_bilinear(&planes.data[1], c0_stride, cw, ch, 1, 0, u, v),
                    sample_bilinear(&planes.data[2], c1_stride, cw, ch, 1, 0, u, v),
                )
            };

            let (yv, cb, cr) = match color.range {
                Range::Limited => (
                    (y_raw - 16.0 / 255.0) * (255.0 / 219.0),
                    (cb_raw - 128.0 / 255.0) * (255.0 / 224.0),
                    (cr_raw - 128.0 / 255.0) * (255.0 / 224.0),
                ),
                Range::Full => (y_raw, cb_raw - 0.5, cr_raw - 0.5),
            };

            let mut rgb = [
                coeffs[0][0] * yv + coeffs[0][1] * cb + coeffs[0][2] * cr,
                coeffs[1][0] * yv + coeffs[1][1] * cb + coeffs[1][2] * cr,
                coeffs[2][0] * yv + coeffs[2][1] * cb + coeffs[2][2] * cr,
            ];

            match color.transfer {
                Transfer::Pq => {
                    for c in rgb.iter_mut() {
                        *c = tonemap(pq_eotf(*c) * 10000.0, REFERENCE_WHITE_NITS);
                    }
                }
                Transfer::Hlg => {
                    for c in rgb.iter_mut() {
                        *c = hlg_eotf(*c);
                    }
                }
                // SDR stays gamma-encoded, as in the shader: the swapchain is
                // non-linear sRGB and converting to linear and back would only
                // add rounding.
                Transfer::Sdr => {}
            }

            let o = (py * width + px) * 4;
            for c in 0..3 {
                out[o + c] = (rgb[c].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
            out[o + 3] = 255;
        }
    }
    Ok(out)
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

    #[test]
    fn output_is_tightly_packed_rgba_with_opaque_alpha() {
        let out = convert(&planes_nv12(126, 128, 128), PlaneLayout::Nv12, sdr709(Range::Limited));
        assert_eq!(out.len(), 2 * 2 * 4);
        for px in out.chunks(4) {
            assert_eq!(px[3], 255);
        }
    }

    #[test]
    fn ten_bit_layouts_are_refused_rather_than_misread() {
        let p = planes_nv12(126, 128, 128);
        assert!(matches!(
            planes_to_rgba8(&p, PlaneLayout::P010, sdr709(Range::Limited), 2, 2),
            Err(ConvertError::UnsupportedDepth(_))
        ));
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
}
