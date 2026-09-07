//! Executes `shaders/convert.wgsl` and checks the numbers.
//!
//! Run with lavapipe when there is no GPU:
//!
//! ```text
//! VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test -p vidkit-vk
//! ```
//!
//! Tests that need a device skip when there is none, so the suite still passes
//! on a machine without Vulkan. `shader_compiles_and_validates` does not skip:
//! naga needs no device, and a shader that does not compile should fail
//! everywhere.

use vidkit_vk::harness::*;

/// Expected values are computed by hand from the BT.709 matrix and compared
/// with a tolerance, because the shader runs in fp32 and stores fp16 — about
/// three decimal digits.
const EPS: f32 = 0.01;

fn approx(got: f32, want: f32, what: &str) {
    assert!(
        (got - want).abs() <= EPS,
        "{what}: got {got:.4}, want {want:.4}"
    );
}

fn approx_rgb(got: [f32; 4], want: [f32; 3], what: &str) {
    approx(got[0], want[0], &format!("{what} R"));
    approx(got[1], want[1], &format!("{what} G"));
    approx(got[2], want[2], &format!("{what} B"));
}

/// A 2x2 frame: 2x2 luma, 1x1 interleaved chroma (NV12 4:2:0).
fn nv12(y: u8, cb: u8, cr: u8) -> Vec<PlaneData> {
    vec![
        PlaneData::r8(2, 2, vec![y; 4]),
        PlaneData::rg8(1, 1, vec![cb, cr]),
    ]
}

/// The same content as three separate planes.
fn i420(y: u8, cb: u8, cr: u8) -> Vec<PlaneData> {
    vec![
        PlaneData::r8(2, 2, vec![y; 4]),
        PlaneData::r8(1, 1, vec![cb]),
        PlaneData::r8(1, 1, vec![cr]),
    ]
}

fn params(plane_count: u32) -> Params {
    Params {
        width: 2,
        height: 2,
        plane_count,
        ..Default::default()
    }
}

/// `None` means no Vulkan; the caller should skip.
fn harness() -> Option<Harness> {
    match Harness::new() {
        Ok(h) => {
            eprintln!("vulkan device: {}", h.device_name());
            Some(h)
        }
        Err(HarnessError::NoDevice(why)) => {
            eprintln!("skipping: {why}");
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

/// No device needed: naga parses and validates the WGSL. This is the check
/// that the shader was previously shipped without.
#[test]
fn shader_compiles_and_validates() {
    let spirv = compile_convert_shader().expect("convert.wgsl must compile");
    assert!(!spirv.is_empty());
    // SPIR-V magic number, so we know we got a module rather than an empty vec.
    assert_eq!(spirv[0], 0x0723_0203, "not a SPIR-V module");
}

// ---------------------------------------------------------------------------
// range expansion
// ---------------------------------------------------------------------------

#[test]
fn limited_range_black_and_white_map_to_zero_and_one() {
    let h = device!();

    // Y'=16 is black in limited range, not 16/255 grey.
    let out = h.run_convert(params(2), &nv12(16, 128, 128)).unwrap();
    approx_rgb(out[0], [0.0, 0.0, 0.0], "limited black");

    // Y'=235 is white, not 235/255.
    let out = h.run_convert(params(2), &nv12(235, 128, 128)).unwrap();
    approx_rgb(out[0], [1.0, 1.0, 1.0], "limited white");
}

/// Sub-black and super-white exist in real streams; they must clamp, not wrap.
#[test]
fn out_of_range_luma_clamps() {
    let h = device!();
    let out = h.run_convert(params(2), &nv12(0, 128, 128)).unwrap();
    approx_rgb(out[0], [0.0, 0.0, 0.0], "sub-black");
    let out = h.run_convert(params(2), &nv12(255, 128, 128)).unwrap();
    approx_rgb(out[0], [1.0, 1.0, 1.0], "super-white");
}

/// Full range uses the whole 0..255 span. Applying the limited-range expansion
/// to a full-range stream crushes blacks and blows highlights.
#[test]
fn full_range_uses_the_whole_span() {
    let h = device!();
    let mut p = params(2);
    p.range_id = 1;

    let out = h.run_convert(p, &nv12(0, 128, 128)).unwrap();
    approx_rgb(out[0], [0.0, 0.0, 0.0], "full black");

    let out = h.run_convert(p, &nv12(255, 128, 128)).unwrap();
    approx_rgb(out[0], [1.0, 1.0, 1.0], "full white");

    // Y'=16 is *not* black in full range; this is what distinguishes the two.
    let out = h.run_convert(p, &nv12(16, 128, 128)).unwrap();
    approx_rgb(out[0], [16.0 / 255.0; 3], "full-range 16 is dark grey");
}

// ---------------------------------------------------------------------------
// colour matrix
// ---------------------------------------------------------------------------

/// BT.709 limited-range red is Y'=63, Cb=102, Cr=240. Worked through the
/// matrix by hand: R = 0.2146 + 1.5748*0.5 = 1.002 (clamps to 1), G and B land
/// on ~0.
#[test]
fn bt709_primary_red_round_trips() {
    let h = device!();
    let out = h.run_convert(params(2), &nv12(63, 102, 240)).unwrap();
    approx_rgb(out[0], [1.0, 0.0, 0.0], "bt709 red");
}

#[test]
fn bt709_primary_blue_round_trips() {
    let h = device!();
    // Y'=32, Cb=240, Cr=118.
    let out = h.run_convert(params(2), &nv12(32, 240, 118)).unwrap();
    let got = out[0];
    assert!(got[2] > 0.9, "blue channel low: {got:?}");
    assert!(got[0] < 0.1 && got[1] < 0.1, "red/green leaked: {got:?}");
}

/// The matrix selector must actually be read. Feeding identical chroma through
/// BT.601 and BT.709 has to give different colours; if it does not, the
/// selector is being ignored and every BT.601 file is subtly wrong.
#[test]
fn bt601_and_bt709_differ_for_the_same_input() {
    let h = device!();
    let mut p601 = params(2);
    p601.matrix_id = 0;
    let mut p2020 = params(2);
    p2020.matrix_id = 2;

    let a = h.run_convert(params(2), &nv12(120, 90, 200)).unwrap()[0];
    let b = h.run_convert(p601, &nv12(120, 90, 200)).unwrap()[0];
    let c = h.run_convert(p2020, &nv12(120, 90, 200)).unwrap()[0];

    let diff = |x: [f32; 4], y: [f32; 4]| {
        (0..3).map(|i| (x[i] - y[i]).abs()).fold(0.0f32, f32::max)
    };
    assert!(diff(a, b) > 0.02, "709 and 601 agree too closely: {a:?} vs {b:?}");
    assert!(diff(a, c) > 0.02, "709 and 2020 agree too closely: {a:?} vs {c:?}");
}

/// Neutral chroma must stay neutral under every matrix, or grey ramps tint.
#[test]
fn neutral_chroma_is_grey_under_every_matrix() {
    let h = device!();
    for matrix_id in [0u32, 1, 2] {
        let mut p = params(2);
        p.matrix_id = matrix_id;
        let out = h.run_convert(p, &nv12(126, 128, 128)).unwrap()[0];
        let expected = (126.0 - 16.0) / 219.0;
        approx_rgb(
            out,
            [expected; 3],
            &format!("matrix {matrix_id} neutral"),
        );
    }
}

// ---------------------------------------------------------------------------
// plane layout
// ---------------------------------------------------------------------------

/// NV12 reads chroma from an interleaved RG texture, I420 from two separate R
/// textures. The same content through both paths must agree — otherwise one of
/// the two decoder families produces swapped or shifted colour.
#[test]
fn nv12_and_i420_agree_for_the_same_content() {
    let h = device!();
    let a = h.run_convert(params(2), &nv12(120, 90, 200)).unwrap()[0];
    let b = h.run_convert(params(3), &i420(120, 90, 200)).unwrap()[0];
    for i in 0..3 {
        approx(b[i], a[i], &format!("nv12 vs i420 channel {i}"));
    }
}

/// Cb and Cr are not interchangeable; swapping them turns skin tones green.
/// This is the check that catches an interleaved-plane channel swap.
#[test]
fn swapping_cb_and_cr_changes_the_result() {
    let h = device!();
    let a = h.run_convert(params(2), &nv12(120, 90, 200)).unwrap()[0];
    let b = h.run_convert(params(2), &nv12(120, 200, 90)).unwrap()[0];
    let diff = (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0f32, f32::max);
    assert!(diff > 0.2, "Cb/Cr swap had no effect: {a:?} vs {b:?}");
}

// ---------------------------------------------------------------------------
// geometry
// ---------------------------------------------------------------------------

/// Every pixel must be written. A dispatch that rounds the workgroup count
/// down leaves the right and bottom edges as uninitialised memory, which shows
/// up as a garbage strip only at certain resolutions.
#[test]
fn all_pixels_are_written_at_a_non_multiple_of_the_workgroup_size() {
    let h = device!();
    // 13x11 is not a multiple of the 8x8 workgroup.
    let w = 13u32;
    let hgt = 11u32;
    let p = Params {
        width: w,
        height: hgt,
        plane_count: 2,
        ..Default::default()
    };
    let planes = vec![
        PlaneData::r8(w, hgt, vec![235u8; (w * hgt) as usize]),
        PlaneData::rg8(
            (w + 1) / 2,
            (hgt + 1) / 2,
            vec![128u8; (((w + 1) / 2) * ((hgt + 1) / 2) * 2) as usize],
        ),
    ];
    let out = h.run_convert(p, &planes).unwrap();
    assert_eq!(out.len(), (w * hgt) as usize);
    for (i, px) in out.iter().enumerate() {
        approx(px[0], 1.0, &format!("pixel {i} R"));
        approx(px[3], 1.0, &format!("pixel {i} alpha"));
    }
}

/// Alpha must be opaque; a zero alpha composites the video away entirely.
#[test]
fn alpha_is_opaque() {
    let h = device!();
    let out = h.run_convert(params(2), &nv12(126, 128, 128)).unwrap();
    approx(out[0][3], 1.0, "alpha");
}

// ---------------------------------------------------------------------------
// transfer functions
// ---------------------------------------------------------------------------

/// The anchor that matters for HDR: ITU reference white is 203 nits, whose PQ
/// code in limited-range 8-bit is Y' = 142. That is *diffuse white* — a sheet
/// of paper, not a highlight — and it must land near 1.0 on an SDR display.
///
/// Getting this wrong is not subtle. A plain Reinhard curve maps the tonemap's
/// own target luminance to exactly 0.5, so every HDR file renders with white
/// paper as mid-grey and the whole image looks dim and washed out.
#[test]
fn pq_reference_white_lands_near_sdr_white() {
    let h = device!();
    let mut pq = params(2);
    pq.transfer_id = 1;

    let out = h.run_convert(pq, &nv12(142, 128, 128)).unwrap()[0];
    assert!(
        out[0] > 0.85,
        "PQ reference white came out at {:.3}; diffuse white must not render as grey",
        out[0]
    );
    assert!(out[0] <= 1.0, "PQ out of range: {out:?}");
}

/// Highlights above reference white must still be brighter than it, and must
/// not clip to a flat 1.0 the moment they exceed it.
#[test]
fn pq_highlights_roll_off_above_reference_white() {
    let h = device!();
    let mut pq = params(2);
    pq.transfer_id = 1;

    let white = h.run_convert(pq, &nv12(142, 128, 128)).unwrap()[0][0];
    let bright = h.run_convert(pq, &nv12(180, 128, 128)).unwrap()[0][0];
    let brightest = h.run_convert(pq, &nv12(235, 128, 128)).unwrap()[0][0];

    assert!(
        bright > white && brightest > bright,
        "PQ is not monotonic: {white:.3} -> {bright:.3} -> {brightest:.3}"
    );
    assert!(brightest <= 1.0, "PQ exceeds 1.0: {brightest}");
}

#[test]
fn hlg_stays_in_range() {
    let h = device!();
    let mut p = params(2);
    p.transfer_id = 2;
    for y in [16u8, 128, 235] {
        let out = h.run_convert(p, &nv12(y, 128, 128)).unwrap()[0];
        for c in 0..3 {
            assert!(
                out[c] >= 0.0 && out[c] <= 1.0,
                "HLG channel {c} out of range at Y={y}: {out:?}"
            );
        }
    }
}

/// Every transfer path must leave black at black; a tonemap that lifts the
/// floor produces washed-out shadows on every HDR file.
#[test]
fn black_stays_black_under_every_transfer() {
    let h = device!();
    for transfer_id in [0u32, 1, 2] {
        let mut p = params(2);
        p.transfer_id = transfer_id;
        let out = h.run_convert(p, &nv12(16, 128, 128)).unwrap()[0];
        approx_rgb(out, [0.0, 0.0, 0.0], &format!("transfer {transfer_id} black"));
    }
}
