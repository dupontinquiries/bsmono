//! Building `DeviceCaps` from what the kernel actually reports.
//!
//! The reference profiles in `vidkit_core::caps` are fallbacks. This is what
//! should run on startup: enumerate the decoder's OUTPUT formats, ask for each
//! one's frame size limits, and turn that into capability entries. It is the
//! difference between "the Fairphone probably has Venus" and "this device has
//! Venus and it does VP9 up to 4096x2160".

use crate::api::pixfmt;
use vidkit_core::caps::{Backend, Cost, DecodeCap, Delivery};
use vidkit_core::codec::Chroma;
use vidkit_core::codec::Codec;

/// One entry from `VIDIOC_ENUM_FMT` on the OUTPUT queue, plus the frame size
/// limits from `VIDIOC_ENUM_FRAMESIZES`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct EnumeratedFormat {
    pub fourcc: u32,
    pub max_width: u32,
    pub max_height: u32,
}

pub fn fourcc_to_codec(fourcc: u32) -> Option<Codec> {
    Some(match fourcc {
        pixfmt::H264 => Codec::H264,
        pixfmt::HEVC => Codec::Hevc,
        pixfmt::VP8 => Codec::Vp8,
        pixfmt::VP9 => Codec::Vp9,
        pixfmt::AV1_FRAME => Codec::Av1,
        _ => return None,
    })
}

/// Convert enumeration results into capability entries.
///
/// Two things this deliberately does not try to guess:
///
/// - **Bit depth.** `ENUM_FMT` on the OUTPUT queue says nothing about it; the
///   honest answer requires querying the codec's profile control
///   (`V4L2_CID_MPEG_VIDEO_*_PROFILE`) and the menu of values it accepts. Until
///   that is wired up this assumes 8-bit for everything except HEVC and VP9,
///   where Main10/Profile2 is near-universal on hardware that supports the
///   codec at all. Getting this wrong in the optimistic direction means
///   selecting a 10-bit stream the decoder rejects at runtime, so the fallback
///   to software must actually work.
/// - **Frame rate.** `ENUM_FRAMEINTERVALS` reports what the driver will accept,
///   not what it sustains. 30 fps at max size is the conservative floor;
///   measure and override.
pub fn caps_from_enumeration(formats: &[EnumeratedFormat]) -> Vec<DecodeCap> {
    formats
        .iter()
        .filter_map(|f| {
            let codec = fourcc_to_codec(f.fourcc)?;
            let max_bit_depth = match codec {
                Codec::Hevc | Codec::Vp9 => 10,
                _ => 8,
            };
            Some(DecodeCap {
                codec,
                max_bit_depth,
                max_chroma: Chroma::Yuv420,
                max_width: f.max_width,
                max_height: f.max_height,
                max_fps_at_max_size: 30,
                // Fixed-function silicon is not per-frame bound the way
                // software decode is, so the ceiling is only a sanity bound.
                // The driver reports sizes, not rates; a real rate needs
                // VIDIOC_ENUM_FRAMEINTERVALS, which is a separate job.
                max_fps_ceiling: 240,
                cost: Cost::Fixed,
                delivery: Delivery::DmaBuf,
                backend: Backend::V4l2Stateful,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a Venus-class decoder reports.
    fn venus_enumeration() -> Vec<EnumeratedFormat> {
        vec![
            EnumeratedFormat {
                fourcc: pixfmt::H264,
                max_width: 4096,
                max_height: 2160,
            },
            EnumeratedFormat {
                fourcc: pixfmt::HEVC,
                max_width: 4096,
                max_height: 2160,
            },
            EnumeratedFormat {
                fourcc: pixfmt::VP9,
                max_width: 4096,
                max_height: 2160,
            },
            // Something we do not handle, to prove unknown formats are skipped
            // rather than mapped to a wrong codec.
            EnumeratedFormat {
                fourcc: pixfmt::code(b'M', b'P', b'G', b'4'),
                max_width: 1920,
                max_height: 1088,
            },
        ]
    }

    #[test]
    fn venus_enumeration_produces_three_hardware_entries() {
        let caps = caps_from_enumeration(&venus_enumeration());
        assert_eq!(caps.len(), 3);
        assert!(caps.iter().all(|c| c.cost == Cost::Fixed));
        assert!(caps.iter().all(|c| c.delivery == Delivery::DmaBuf));
    }

    /// The probe must not invent AV1 support. This is the check that keeps the
    /// Fairphone from picking an AV1 stream it cannot decode in hardware.
    #[test]
    fn no_av1_entry_appears_when_the_driver_does_not_report_it() {
        let caps = caps_from_enumeration(&venus_enumeration());
        assert!(!caps.iter().any(|c| c.codec == Codec::Av1));
    }

    #[test]
    fn unknown_fourccs_are_skipped() {
        let caps = caps_from_enumeration(&[EnumeratedFormat {
            fourcc: 0xDEAD_BEEF,
            max_width: 1920,
            max_height: 1080,
        }]);
        assert!(caps.is_empty());
    }

    #[test]
    fn vp9_and_hevc_get_ten_bit_h264_does_not() {
        let caps = caps_from_enumeration(&venus_enumeration());
        let by = |c: Codec| caps.iter().find(|x| x.codec == c).unwrap().max_bit_depth;
        assert_eq!(by(Codec::Vp9), 10);
        assert_eq!(by(Codec::Hevc), 10);
        assert_eq!(by(Codec::H264), 8);
    }

    /// The probed size limits must reach the selection policy intact — a
    /// decoder that reports 4096 wide should not be clamped to a guess.
    #[test]
    fn enumerated_limits_reach_the_capability_entry() {
        let caps = caps_from_enumeration(&venus_enumeration());
        let vp9 = caps.iter().find(|c| c.codec == Codec::Vp9).unwrap();
        assert_eq!((vp9.max_width, vp9.max_height), (4096, 2160));
    }
}
