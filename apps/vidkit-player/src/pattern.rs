//! The built-in test pattern.
//!
//! BT.709 limited-range colour bars in NV12, with a sweep column so successive
//! frames differ — a stuck presenter then looks stuck rather than plausible.
//! The bar values exercise the CPU converter's range expansion and matrix: if
//! either is wrong the bars come out washed or hue-shifted, which is obvious on
//! sight and invisible to a compile.

use std::time::Duration;
use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, PlaneLayout};

pub const W: usize = 640;
pub const H: usize = 360;

/// (Y, Cb, Cr) for white, yellow, cyan, green, magenta, red, blue, black.
const BARS: [(u8, u8, u8); 8] = [
    (235, 128, 128),
    (219, 16, 138),
    (188, 154, 16),
    (173, 42, 26),
    (78, 214, 230),
    (63, 102, 240),
    (32, 240, 118),
    (16, 128, 128),
];

pub fn bars(phase: f32) -> Frame {
    let mut y = vec![0u8; W * H];
    let (cw, ch) = (W / 2, H / 2);
    let mut uv = vec![128u8; cw * ch * 2];

    let sweep = ((phase.fract().abs() * W as f32) as usize).min(W - 1);

    for row in 0..H {
        for col in 0..W {
            let bar = BARS[(col * BARS.len()) / W];
            let lift = if col.abs_diff(sweep) < 3 { 40u8 } else { 0 };
            y[row * W + col] = bar.0.saturating_add(lift);
        }
    }
    for row in 0..ch {
        for col in 0..cw {
            let bar = BARS[((col * 2) * BARS.len()) / W];
            uv[(row * cw + col) * 2] = bar.1;
            uv[(row * cw + col) * 2 + 1] = bar.2;
        }
    }

    Frame {
        data: FrameData::Cpu(CpuPlanes {
            data: vec![y, uv],
            strides: vec![W as u32, (cw * 2) as u32],
        }),
        width: W as u32,
        height: H as u32,
        layout: PlaneLayout::Nv12,
        color: ColorInfo::default(),
        pts: Duration::ZERO,
        duration: Some(Duration::from_millis(33)),
        recycle: None,
        grain: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_has_the_planes_the_layout_promises() {
        let f = bars(0.0);
        match &f.data {
            FrameData::Cpu(p) => {
                assert_eq!(p.data.len(), 2, "NV12 is two planes");
                assert_eq!(p.data[0].len(), W * H);
                assert_eq!(p.data[1].len(), W * H / 2);
                assert_eq!(p.strides, vec![W as u32, W as u32]);
            }
            _ => panic!("expected CPU planes"),
        }
    }

    /// The sweep is what makes a stuck presenter visible, so it has to move.
    #[test]
    fn successive_frames_differ() {
        let a = bars(0.0);
        let b = bars(0.5);
        let (x, y) = match (&a.data, &b.data) {
            (FrameData::Cpu(x), FrameData::Cpu(y)) => (x, y),
            _ => unreachable!(),
        };
        assert_ne!(x.data[0], y.data[0], "the pattern is static");
    }

    /// A negative or huge phase must not panic or index out of bounds --
    /// elapsed time is an f32 and this is called every frame forever.
    #[test]
    fn extreme_phases_do_not_panic() {
        for p in [-5.0f32, 0.0, 1.0, 1e6] {
            let _ = bars(p);
        }
    }
}
