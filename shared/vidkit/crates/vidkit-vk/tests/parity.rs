//! CPU converter vs GPU shader, over the same inputs.
//!
//! `vidkit-core::convert_cpu` and `shaders/convert.wgsl` implement the same
//! colour maths twice, because egui's glow backend samples ordinary textures
//! and cannot touch a DMA-BUF. Nothing else notices if one side changes and the
//! other does not — the situation
//! `maintenance/tools/voiceprint_parity_check.py` exists for elsewhere in this
//! tree, where a shared kit is a copy rather than a dependency.
//!
//! This is the stronger version of that check: rather than comparing source
//! text, it runs both and compares pixels. Skips when there is no Vulkan
//! device, so it is worth confirming it actually ran:
//!
//! ```text
//! VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
//!   cargo test -p vidkit-vk --test parity -- --nocapture
//! ```

use vidkit_core::codec::Transfer;
use vidkit_core::frame::{ColorInfo, CpuPlanes, Matrix, PlaneLayout, Range};
use vidkit_vk::harness::*;

/// Both sides are f32 internally; the GPU stores fp16 and the CPU side rounds
/// to u8. One 8-bit step of disagreement is the quantisation, not a drift.
const TOL: i32 = 2;

fn harness() -> Option<Harness> {
    match Harness::new() {
        Ok(h) => {
            eprintln!("parity against: {}", h.device_name());
            Some(h)
        }
        Err(HarnessError::NoDevice(why)) => {
            eprintln!("SKIPPED: {why}");
            None
        }
        Err(e) => panic!("harness setup failed: {e}"),
    }
}

macro_rules! device {
    () => {
        match harness() {
            Some(h) => h,
            None => return,
        }
    };
}

fn matrix_id(m: Matrix) -> u32 {
    match m {
        Matrix::Bt601 => 0,
        Matrix::Bt709 => 1,
        Matrix::Bt2020Ncl => 2,
    }
}

fn transfer_id(t: Transfer) -> u32 {
    match t {
        Transfer::Sdr => 0,
        Transfer::Pq => 1,
        Transfer::Hlg => 2,
    }
}

/// Run both paths over a flat 2x2 NV12 patch and compare.
fn compare_nv12(h: &Harness, y: u8, cb: u8, cr: u8, color: ColorInfo) {
    let gpu = h
        .run_convert(
            Params {
                width: 2,
                height: 2,
                plane_count: 2,
                matrix_id: matrix_id(color.matrix),
                transfer_id: transfer_id(color.transfer),
                range_id: match color.range {
                    Range::Limited => 0,
                    Range::Full => 1,
                },
                target_nits: vidkit_core::convert_cpu::REFERENCE_WHITE_NITS,
                _pad: 0,
            },
            &[
                PlaneData::r8(2, 2, vec![y; 4]),
                PlaneData::rg8(1, 1, vec![cb, cr]),
            ],
        )
        .expect("gpu convert");

    let planes = CpuPlanes {
        data: vec![vec![y; 4], vec![cb, cr]],
        strides: vec![2, 2],
    };
    let cpu = vidkit_core::planes_to_rgba8(&planes, PlaneLayout::Nv12, color, 2, 2).expect("cpu convert");

    for px in 0..4 {
        for c in 0..3 {
            let g = (gpu[px][c].clamp(0.0, 1.0) * 255.0 + 0.5) as i32;
            let p = cpu[px * 4 + c] as i32;
            assert!(
                (g - p).abs() <= TOL,
                "Y={y} Cb={cb} Cr={cr} {:?}/{:?}/{:?} pixel {px} channel {c}: \
                 gpu {g}, cpu {p} -- the two implementations have drifted",
                color.matrix,
                color.transfer,
                color.range
            );
        }
    }
}

fn sdr(matrix: Matrix, range: Range) -> ColorInfo {
    ColorInfo {
        matrix,
        transfer: Transfer::Sdr,
        range,
    }
}

#[test]
fn sdr_agrees_across_the_matrices_and_both_ranges() {
    let h = device!();
    let mut checked = 0;
    for matrix in [Matrix::Bt601, Matrix::Bt709, Matrix::Bt2020Ncl] {
        for range in [Range::Limited, Range::Full] {
            for (y, cb, cr) in [
                (16u8, 128u8, 128u8),  // black
                (235, 128, 128),       // white
                (126, 128, 128),       // mid grey
                (63, 102, 240),        // red
                (32, 240, 118),        // blue
                (120, 90, 200),        // arbitrary
                (0, 0, 0),             // sub-black, extreme chroma
                (255, 255, 255),       // super-white, extreme chroma
            ] {
                compare_nv12(&h, y, cb, cr, sdr(matrix, range));
                checked += 1;
            }
        }
    }
    // A comparison loop that ran zero times passes every assertion inside it.
    assert_eq!(checked, 48, "the sweep did not run the cases it claims to");
}

#[test]
fn pq_and_hlg_agree() {
    let h = device!();
    let mut checked = 0;
    for transfer in [Transfer::Pq, Transfer::Hlg] {
        for y in [16u8, 96, 142, 180, 235] {
            compare_nv12(
                &h,
                y,
                128,
                128,
                ColorInfo {
                    matrix: Matrix::Bt2020Ncl,
                    transfer,
                    range: Range::Limited,
                },
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 10, "the sweep did not run");
}

/// The tonemap is the piece most likely to be edited on one side only, and its
/// reference-white anchor is the value that was wrong before. Pin it in both.
#[test]
fn the_tonemap_anchor_matches_on_both_sides() {
    let h = device!();
    let color = ColorInfo {
        matrix: Matrix::Bt2020Ncl,
        transfer: Transfer::Pq,
        range: Range::Limited,
    };
    compare_nv12(&h, 142, 128, 128, color);

    // ...and is still the value the shader test asserts, so an edit that moved
    // both sides together would still be caught here.
    let planes = CpuPlanes {
        data: vec![vec![142u8; 4], vec![128, 128]],
        strides: vec![2, 2],
    };
    let cpu = vidkit_core::planes_to_rgba8(&planes, PlaneLayout::Nv12, color, 2, 2).unwrap();
    assert!(
        cpu[0] > 216,
        "PQ reference white came out at {}/255; diffuse white must not render as grey",
        cpu[0]
    );
}

/// Chroma is half resolution, so the upscale filter has to match. Nearest
/// neighbour on the CPU side against the GPU's bilinear sampler would agree on
/// every flat patch and disagree on every real frame — so the fixture has to
/// have a chroma edge in it, or this test proves nothing.
#[test]
fn chroma_upsampling_agrees_across_an_edge() {
    let h = device!();

    // 4x4 luma, 2x2 chroma with a diagonal split: two distinct chroma texels
    // per axis, so the bilinear weights actually differ per output pixel.
    let w = 4usize;
    let hgt = 4usize;
    let luma = vec![160u8; w * hgt];
    let chroma: Vec<u8> = vec![
        90, 200, 200, 90, // row 0: (Cb,Cr) pairs
        200, 90, 90, 200, // row 1
    ];

    let gpu = h
        .run_convert(
            Params {
                width: w as u32,
                height: hgt as u32,
                plane_count: 2,
                ..Default::default()
            },
            &[
                PlaneData::r8(w as u32, hgt as u32, luma.clone()),
                PlaneData::rg8(2, 2, chroma.clone()),
            ],
        )
        .expect("gpu convert");

    let planes = CpuPlanes {
        data: vec![luma, chroma],
        strides: vec![w as u32, 4],
    };
    let cpu = vidkit_core::planes_to_rgba8(
        &planes,
        PlaneLayout::Nv12,
        sdr(Matrix::Bt709, Range::Limited),
        w,
        hgt,
    )
    .unwrap();

    // Confirm the fixture actually has an edge, or the comparison below is
    // satisfied by a flat image.
    let reds: Vec<u8> = (0..w * hgt).map(|i| cpu[i * 4]).collect();
    let spread = reds.iter().max().unwrap() - reds.iter().min().unwrap();
    assert!(
        spread > 30,
        "fixture is too flat to test upsampling (spread {spread}); \
         nearest and bilinear would agree on it"
    );

    for px in 0..w * hgt {
        for c in 0..3 {
            let g = (gpu[px][c].clamp(0.0, 1.0) * 255.0 + 0.5) as i32;
            let p = cpu[px * 4 + c] as i32;
            assert!(
                (g - p).abs() <= TOL + 2,
                "pixel {px} channel {c}: gpu {g}, cpu {p} -- chroma upscale differs"
            );
        }
    }
}
