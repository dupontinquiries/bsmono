//! AVFrame planes into vidkit planes. Pure (no libav calls), tested alone.

use vidkit_core::dmabuf::{DRM_FORMAT_GR1616, DRM_FORMAT_GR88, DRM_FORMAT_NV12, DRM_FORMAT_P010, DRM_FORMAT_R16, DRM_FORMAT_R8};
use vidkit_core::frame::{CpuPlanes, PlaneLayout};

/// The software formats handled in process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwFormat {
    Yuv420p,
    Nv12,
    P010,
    /// 10-bit planar, low bits: repacked to P010 (the GPU's 16-bit path).
    Yuv420p10,
}

impl SwFormat {
    pub fn from_av(f: i32) -> Option<Self> {
        use ffmpeg_sys_next::AVPixelFormat::*;
        match f {
            x if x == AV_PIX_FMT_YUV420P as i32 || x == AV_PIX_FMT_YUVJ420P as i32 => Some(Self::Yuv420p),
            x if x == AV_PIX_FMT_NV12 as i32 => Some(Self::Nv12),
            x if x == AV_PIX_FMT_P010LE as i32 => Some(Self::P010),
            x if x == AV_PIX_FMT_YUV420P10LE as i32 => Some(Self::Yuv420p10),
            _ => None,
        }
    }

    pub fn planes(self) -> usize {
        match self {
            Self::Yuv420p | Self::Yuv420p10 => 3,
            Self::Nv12 | Self::P010 => 2,
        }
    }

    pub fn plane_rows(self, i: usize, h: usize) -> usize {
        if i == 0 { h } else { h.div_ceil(2) }
    }
}

/// Copy (or for 10-bit planar, repack) into tightly strided planes.
/// `planes`: (bytes from the plane start, line size).
pub fn to_cpu(fmt: SwFormat, planes: &[(&[u8], usize)], w: usize, h: usize, alloc: &mut dyn FnMut(usize) -> Vec<u8>) -> (PlaneLayout, CpuPlanes) {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let copy = |alloc: &mut dyn FnMut(usize) -> Vec<u8>, (src, ls): (&[u8], usize), row: usize, rows: usize| {
        let mut v = alloc(row * rows);
        v.resize(row * rows, 0);
        for y in 0..rows {
            v[y * row..(y + 1) * row].copy_from_slice(&src[y * ls..y * ls + row]);
        }
        v
    };
    match fmt {
        SwFormat::Yuv420p => {
            let data = vec![copy(alloc, planes[0], w, h), copy(alloc, planes[1], cw, ch), copy(alloc, planes[2], cw, ch)];
            (PlaneLayout::I420, CpuPlanes { data, strides: vec![w as u32, cw as u32, cw as u32] })
        }
        SwFormat::Nv12 => {
            let data = vec![copy(alloc, planes[0], w, h), copy(alloc, planes[1], cw * 2, ch)];
            (PlaneLayout::Nv12, CpuPlanes { data, strides: vec![w as u32, (cw * 2) as u32] })
        }
        SwFormat::P010 => {
            let data = vec![copy(alloc, planes[0], w * 2, h), copy(alloc, planes[1], cw * 4, ch)];
            (PlaneLayout::P010, CpuPlanes { data, strides: vec![(w * 2) as u32, (cw * 4) as u32] })
        }
        SwFormat::Yuv420p10 => {
            // Value v (0..1023) becomes v << 6 in a 16-bit little-endian
            // sample: P010's layout, what the 16-bit texture path expects.
            let sample = |src: &[u8], ls: usize, x: usize, y: usize| u16::from_le_bytes([src[y * ls + 2 * x], src[y * ls + 2 * x + 1]]) << 6;
            let mut luma = alloc(w * h * 2);
            luma.resize(w * h * 2, 0);
            let (ys, yls) = planes[0];
            for y in 0..h {
                for x in 0..w {
                    luma[(y * w + x) * 2..(y * w + x) * 2 + 2].copy_from_slice(&sample(ys, yls, x, y).to_le_bytes());
                }
            }
            let mut chroma = alloc(cw * ch * 4);
            chroma.resize(cw * ch * 4, 0);
            let ((us, uls), (vs, vls)) = (planes[1], planes[2]);
            for y in 0..ch {
                for x in 0..cw {
                    let o = (y * cw + x) * 4;
                    chroma[o..o + 2].copy_from_slice(&sample(us, uls, x, y).to_le_bytes());
                    chroma[o + 2..o + 4].copy_from_slice(&sample(vs, vls, x, y).to_le_bytes());
                }
            }
            (PlaneLayout::P010, CpuPlanes { data: vec![luma, chroma], strides: vec![(w * 2) as u32, (cw * 4) as u32] })
        }
    }
}

/// The picture format of a DRM-PRIME descriptor's layers: one composed
/// layer (NV12 / P010), or separate layers (R8 + GR88, R16 + GR1616 -- what
/// VAAPI's SEPARATE_LAYERS export gives).
pub fn fourcc_of_layers(layers: &[u32]) -> Option<u32> {
    match layers {
        [f] if *f == DRM_FORMAT_NV12 || *f == DRM_FORMAT_P010 => Some(*f),
        [a, b] if *a == DRM_FORMAT_R8 && *b == DRM_FORMAT_GR88 => Some(DRM_FORMAT_NV12),
        [a, b] if *a == DRM_FORMAT_R16 && *b == DRM_FORMAT_GR1616 => Some(DRM_FORMAT_P010),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_name_the_picture() {
        assert_eq!(fourcc_of_layers(&[DRM_FORMAT_NV12]), Some(DRM_FORMAT_NV12));
        assert_eq!(fourcc_of_layers(&[DRM_FORMAT_R8, DRM_FORMAT_GR88]), Some(DRM_FORMAT_NV12));
        assert_eq!(fourcc_of_layers(&[DRM_FORMAT_R16, DRM_FORMAT_GR1616]), Some(DRM_FORMAT_P010));
        assert_eq!(fourcc_of_layers(&[DRM_FORMAT_R8]), None);
    }

    #[test]
    fn ten_bit_planar_becomes_p010() {
        // 2x2 luma of 1023, 0, 512, 1; chroma 1x1 of 100 / 900; padded rows.
        let y: Vec<u8> = [1023u16, 0, 0xdead, 512, 1, 0xdead].iter().flat_map(|v| v.to_le_bytes()).collect();
        let u: Vec<u8> = [100u16, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let v: Vec<u8> = [900u16, 0].iter().flat_map(|v| v.to_le_bytes()).collect();
        let (l, p) = to_cpu(SwFormat::Yuv420p10, &[(&y, 6), (&u, 4), (&v, 4)], 2, 2, &mut |n| Vec::with_capacity(n));
        assert_eq!(l, PlaneLayout::P010);
        let s = |b: &[u8], i: usize| u16::from_le_bytes([b[2 * i], b[2 * i + 1]]);
        assert_eq!((s(&p.data[0], 0), s(&p.data[0], 1), s(&p.data[0], 2), s(&p.data[0], 3)), (1023 << 6, 0, 512 << 6, 1 << 6));
        assert_eq!((s(&p.data[1], 0), s(&p.data[1], 1)), (100 << 6, 900 << 6));
    }

    #[test]
    fn padding_is_dropped() {
        let y = [1u8, 2, 9, 9, 3, 4, 9, 9];
        let (u, v) = ([5u8, 9], [6u8, 9]);
        let (l, p) = to_cpu(SwFormat::Yuv420p, &[(&y, 4), (&u, 2), (&v, 2)], 2, 2, &mut |n| Vec::with_capacity(n));
        assert_eq!(l, PlaneLayout::I420);
        assert_eq!(p.data, vec![vec![1, 2, 3, 4], vec![5], vec![6]]);
        assert_eq!(p.strides, vec![2, 1, 1]);
    }
}
