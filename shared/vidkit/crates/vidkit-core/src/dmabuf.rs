//! How a DMA-BUF frame splits into single-plane imports. Pure.
//!
//! The GL presenter imports each plane as its own EGLImage (Y as R8, CbCr
//! as GR88...) and samples them with the same shader as uploaded planes,
//! instead of one NV12 image as `samplerExternalOES` (a second shader, and
//! a conversion whose matrix and range the driver picks, not us).
//!
//! Producers describe planes differently: V4L2's single-planar NV12 is ONE
//! fd with the chroma "immediately after" luma (at stride x height), its
//! NM12 is two fds; VAAPI's DRM-PRIME export is one object with a layer per
//! plane. [`plane_imports`] normalises all of them.

use crate::frame::{DmaBufImage, PlaneLayout};

/// A DRM fourcc from its four characters.
pub const fn fourcc(s: &[u8; 4]) -> u32 {
    (s[0] as u32) | (s[1] as u32) << 8 | (s[2] as u32) << 16 | (s[3] as u32) << 24
}

pub const DRM_FORMAT_NV12: u32 = fourcc(b"NV12");
/// V4L2's non-contiguous NV12: one buffer per plane.
pub const V4L2_PIX_FMT_NV12M: u32 = fourcc(b"NM12");
pub const DRM_FORMAT_P010: u32 = fourcc(b"P010");
pub const DRM_FORMAT_YUV420: u32 = fourcc(b"YU12");
pub const DRM_FORMAT_R8: u32 = fourcc(b"R8  ");
pub const DRM_FORMAT_GR88: u32 = fourcc(b"GR88");
pub const DRM_FORMAT_R16: u32 = fourcc(b"R16 ");
pub const DRM_FORMAT_GR1616: u32 = fourcc(b"GR32");
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;
/// "No explicit modifier": the driver's implicit layout.
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// One plane, as its own image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlaneImport {
    /// Index into `DmaBufImage::planes` of the fd holding this plane.
    pub fd_index: usize,
    /// Single-plane DRM format to import it as.
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub offset: u32,
    pub stride: u32,
}

/// The layout a DMA-BUF fourcc decodes to.
pub fn layout_of(fourcc: u32) -> Option<PlaneLayout> {
    match fourcc {
        DRM_FORMAT_NV12 | V4L2_PIX_FMT_NV12M => Some(PlaneLayout::Nv12),
        DRM_FORMAT_P010 => Some(PlaneLayout::P010),
        DRM_FORMAT_YUV420 => Some(PlaneLayout::I420),
        _ => None,
    }
}

/// Split `img` (a `w`x`h` picture) into single-plane imports.
///
/// A plane list shorter than the layout's plane count means the remaining
/// planes follow in the same buffer, each `stride x plane height` after the
/// previous (V4L2 single-planar formats). A producer whose planes are
/// padded (Venus aligns luma rows to 32 lines) must list them explicitly
/// -- this cannot guess the padding.
pub fn plane_imports(img: &DmaBufImage, w: u32, h: u32) -> Result<(PlaneLayout, Vec<PlaneImport>), String> {
    let layout = layout_of(img.fourcc).ok_or_else(|| format!("dma-buf fourcc {:08x} is not imported", img.fourcc))?;
    let first = img.planes.first().ok_or("dma-buf with no planes")?;
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    // (format, width, height, bytes per texel) per plane.
    let shapes: &[(u32, u32, u32, u32)] = match layout {
        PlaneLayout::Nv12 => &[(DRM_FORMAT_R8, w, h, 1), (DRM_FORMAT_GR88, cw, ch, 2)],
        PlaneLayout::P010 => &[(DRM_FORMAT_R16, w, h, 2), (DRM_FORMAT_GR1616, cw, ch, 4)],
        PlaneLayout::I420 => &[(DRM_FORMAT_R8, w, h, 1), (DRM_FORMAT_R8, cw, ch, 1), (DRM_FORMAT_R8, cw, ch, 1)],
        other => return Err(format!("{other:?} dma-bufs are not imported")),
    };
    let mut out = Vec::with_capacity(shapes.len());
    for (i, &(format, pw, ph, bpt)) in shapes.iter().enumerate() {
        let imp = match img.planes.get(i) {
            Some(p) => PlaneImport { fd_index: i, format, width: pw, height: ph, offset: p.offset, stride: p.stride },
            None => {
                // Contiguous in the first buffer. Chroma rows of I420 are
                // half the luma stride; NV12's interleaved rows are full.
                let prev: &PlaneImport = out.last().ok_or("no previous plane")?;
                let stride = match layout {
                    PlaneLayout::I420 => first.stride.div_ceil(2),
                    _ => first.stride,
                };
                PlaneImport { fd_index: 0, format, width: pw, height: ph, offset: prev.offset + prev.stride * prev.height, stride }
            }
        };
        if imp.stride < pw * bpt {
            return Err(format!("plane {i}: stride {} is less than a row ({} bytes)", imp.stride, pw * bpt));
        }
        out.push(imp);
    }
    Ok((layout, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::DmaBufPlane;
    use std::os::fd::OwnedFd;

    fn fd() -> OwnedFd {
        std::fs::File::open("/dev/null").unwrap().into()
    }

    fn img(fourcc: u32, planes: &[(u32, u32)]) -> DmaBufImage {
        DmaBufImage { planes: planes.iter().map(|&(offset, stride)| DmaBufPlane { fd: fd(), offset, stride }).collect(), modifier: 0, fourcc }
    }

    #[test]
    fn fourccs_are_little_endian_characters() {
        assert_eq!(DRM_FORMAT_NV12, 0x3231_564e);
        assert_eq!(DRM_FORMAT_GR88, 0x3838_5247);
        assert_eq!(DRM_FORMAT_R8, 0x2020_3852);
    }

    /// V4L2 single-planar NV12: chroma at stride x height in the same fd.
    #[test]
    fn single_fd_nv12_puts_chroma_after_luma() {
        let (l, p) = plane_imports(&img(DRM_FORMAT_NV12, &[(0, 1920)]), 1920, 1080).unwrap();
        assert_eq!(l, PlaneLayout::Nv12);
        assert_eq!(p[0], PlaneImport { fd_index: 0, format: DRM_FORMAT_R8, width: 1920, height: 1080, offset: 0, stride: 1920 });
        assert_eq!(p[1], PlaneImport { fd_index: 0, format: DRM_FORMAT_GR88, width: 960, height: 540, offset: 1920 * 1080, stride: 1920 });
    }

    /// VAAPI exports planes explicitly (padded: 1088 rows of luma).
    #[test]
    fn explicit_planes_are_kept() {
        let (_, p) = plane_imports(&img(DRM_FORMAT_NV12, &[(0, 2048), (2048 * 1088, 2048)]), 1920, 1080).unwrap();
        assert_eq!((p[1].offset, p[1].stride, p[1].fd_index), (2048 * 1088, 2048, 1));
    }

    #[test]
    fn odd_sizes_round_chroma_up_and_i420_halves_the_stride() {
        let (l, p) = plane_imports(&img(DRM_FORMAT_YUV420, &[(0, 642)]), 641, 361).unwrap();
        assert_eq!(l, PlaneLayout::I420);
        assert_eq!((p[1].width, p[1].height, p[1].stride, p[1].offset), (321, 181, 321, 642 * 361));
        assert_eq!(p[2].offset, 642 * 361 + 321 * 181);
    }

    #[test]
    fn p010_imports_as_sixteen_bit_planes() {
        let (l, p) = plane_imports(&img(DRM_FORMAT_P010, &[(0, 7680), (7680 * 2160, 7680)]), 3840, 2160).unwrap();
        assert_eq!(l, PlaneLayout::P010);
        assert_eq!((p[0].format, p[1].format), (DRM_FORMAT_R16, DRM_FORMAT_GR1616));
    }

    #[test]
    fn nonsense_is_refused() {
        assert!(plane_imports(&img(fourcc(b"XR24"), &[(0, 256)]), 64, 64).is_err());
        assert!(plane_imports(&img(DRM_FORMAT_NV12, &[(0, 32)]), 64, 64).is_err(), "stride shorter than a row");
        assert!(plane_imports(&img(DRM_FORMAT_NV12, &[]), 64, 64).is_err());
    }
}
