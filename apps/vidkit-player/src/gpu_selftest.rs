//! `vidkit-player --gpu-selftest`: the video shaders, checked on the GPU.
//!
//! Unit tests check the shaders' Rust mirrors; nothing checked the GLSL.
//! This renders synthetic NV12 frames through the real `Nv12Gpu` in a real
//! GL context (any: llvmpipe under Xvfb, the phone's own driver), reads the
//! pixels back inside the paint callback, and compares:
//!
//! 1. plain pass vs the CPU colour converter, on a 2-D pattern -- also pins
//!    orientation;
//! 2. RCAS at 1:1 vs `rcas_reference` computed on the CPU from the same
//!    converted frame -- the offscreen pass, its orientation and the shader
//!    arithmetic together;
//! 3. bicubic at 2x: exact on a linear ramp (Catmull-Rom reproduces linear
//!    functions), and overshooting a hard step, which bilinear never does.
//!
//! Prints one line per case and exits 0 only if all pass.

use std::sync::{Arc, Mutex};

use eframe::egui;
use vidkit_core::frame::{ColorInfo, CpuPlanes, Frame, FrameData, PlaneLayout};
use vidkit_egui::glow_present::{expected_rgb, rcas_reference, temporal_reference, Enhance, Nv12Gpu, Scaler};

fn nv12(w: u32, h: u32, y: impl Fn(u32, u32) -> u8) -> Frame {
    let mut luma = Vec::with_capacity((w * h) as usize);
    for yy in 0..h {
        for x in 0..w {
            luma.push(y(x, yy));
        }
    }
    let chroma = vec![128u8; (w as usize).div_ceil(2) * 2 * (h as usize).div_ceil(2)];
    Frame {
        data: FrameData::Cpu(CpuPlanes { data: vec![luma, chroma], strides: vec![w, w] }),
        width: w,
        height: h,
        layout: PlaneLayout::Nv12,
        color: ColorInfo::default(),
        pts: std::time::Duration::ZERO,
        duration: None,
        recycle: None,
        grain: None,
    }
}

struct Case {
    /// Run sharpening as the old two-pass (offscreen) path, for comparison.
    two_pass: bool,
    /// AV1 film grain to apply on the GPU before conversion.
    grain: Option<vidkit_core::av1_grain::GrainParams>,
    name: &'static str,
    /// The frame checked against. Multi-frame cases upload `history` first,
    /// one per update, then this.
    frame: Frame,
    history: Vec<Frame>,
    enhance: Enhance,
    /// Output size in pixels.
    out: (u32, u32),
    check: fn(&Frame, &[[u8; 3]], (u32, u32)) -> Result<String, String>,
}

fn luma_of(f: &Frame, x: u32, y: u32) -> u8 {
    match &f.data {
        FrameData::Cpu(p) => p.data[0][(y * f.width + x) as usize],
        _ => 0,
    }
}

fn conv(f: &Frame, x: u32, y: u32) -> [u8; 3] {
    expected_rgb(luma_of(f, x, y), 128, 128, f.color)
}

fn max_err(a: &[[u8; 3]], b: impl Fn(usize) -> [u8; 3], skip_border: Option<(u32, u32)>) -> u8 {
    let mut worst = 0u8;
    for (i, px) in a.iter().enumerate() {
        if let Some((w, h)) = skip_border {
            let (x, y) = (i as u32 % w, i as u32 / w);
            if x == 0 || y == 0 || x + 1 == w || y + 1 == h {
                continue;
            }
        }
        let e = b(i);
        for c in 0..3 {
            worst = worst.max(px[c].abs_diff(e[c]));
        }
    }
    worst
}

fn check_plain(f: &Frame, got: &[[u8; 3]], (w, _h): (u32, u32)) -> Result<String, String> {
    let worst = max_err(got, |i| conv(f, i as u32 % w, i as u32 / w), None);
    if worst <= 2 { Ok(format!("max error {worst}/255")) } else { Err(format!("max error {worst}/255 against the CPU converter")) }
}

fn check_rcas(f: &Frame, got: &[[u8; 3]], (w, h): (u32, u32)) -> Result<String, String> {
    let scale = (-0.2f32).exp2();
    let px = |x: u32, y: u32| conv(f, x, y).map(|v| v as f32 / 255.0);
    let expect = |i: usize| {
        let (x, y) = (i as u32 % w, i as u32 / w);
        let o = rcas_reference(px(x, y - 1), px(x - 1, y), px(x, y), px(x + 1, y), px(x, y + 1), scale);
        o.map(|v| (v * 255.0).round() as u8)
    };
    let worst = max_err(got, expect, Some((w, h)));
    // How much sharpening happened at all: a pass that silently did nothing
    // would match a flat reference too.
    let moved = got.iter().enumerate().filter(|(i, p)| {
        let (x, y) = (*i as u32 % w, *i as u32 / w);
        x > 0 && y > 0 && x + 1 < w && y + 1 < h && p[0].abs_diff(conv(f, x, y)[0]) > 3
    }).count();
    if worst > 3 {
        Err(format!("max error {worst}/255 against rcas_reference"))
    } else if moved == 0 {
        Err("no pixel changed: the RCAS pass did not run".into())
    } else {
        Ok(format!("max error {worst}/255, {moved} pixels sharpened"))
    }
}

fn check_ramp(f: &Frame, got: &[[u8; 3]], (w, h): (u32, u32)) -> Result<String, String> {
    // A horizontal ramp shown at 2x: interior output pixels sit on the line
    // through the source samples.
    let src_w = f.width as f32;
    let mut worst = 0.0f32;
    for y in 2..h - 2 {
        for x in 4..w - 4 {
            let sx = (x as f32 + 0.5) * src_w / w as f32 - 0.5;
            let yv = 40.0 + sx * 4.0;
            let e = expected_rgb(yv.round() as u8, 128, 128, f.color)[0] as f32;
            worst = worst.max((got[(y * w + x) as usize][0] as f32 - e).abs());
        }
    }
    if worst <= 3.0 { Ok(format!("linear ramp reproduced within {worst:.0}/255")) } else { Err(format!("ramp off by {worst:.0}/255")) }
}

fn check_step_overshoot(f: &Frame, got: &[[u8; 3]], (w, h): (u32, u32)) -> Result<String, String> {
    let hi = conv(f, f.width - 1, 0)[0];
    let lo = conv(f, 0, 0)[0];
    let row = &got[((h / 2) * w) as usize..((h / 2 + 1) * w) as usize];
    let (mx, mn) = (row.iter().map(|p| p[0]).max().unwrap_or(0), row.iter().map(|p| p[0]).min().unwrap_or(255));
    if mx > hi || mn < lo {
        Ok(format!("Catmull-Rom overshoot at the step: {mn}..{mx} beyond {lo}..{hi}"))
    } else {
        Err(format!("no overshoot ({mn}..{mx} within {lo}..{hi}): bicubic did not run"))
    }
}

fn at_pts(mut f: Frame, i: usize) -> Frame {
    f.pts = std::time::Duration::from_millis(40 * i as u64);
    f
}

/// Deterministic noise in -5..=5, different per frame and pixel.
fn noise(seed: u32, x: u32, y: u32) -> i32 {
    let mut v = seed.wrapping_mul(747796405).wrapping_add(x.wrapping_mul(2891336453)).wrapping_add(y.wrapping_mul(1181783497));
    v ^= v >> 15;
    v = v.wrapping_mul(277803737);
    (v >> 16) as i32 % 11 - 5
}

fn board(x: u32, y: u32) -> u8 {
    // Chalk-ish: dark board, a few pale strokes.
    if (y == 10 && (6..40).contains(&x)) || (x == 20 && (6..30).contains(&y)) || (y == 24 && (30..56).contains(&x)) { 200 } else { 50 }
}

fn noisy_board(frame: u32) -> Frame {
    nv12(64, 48, move |x, y| (board(x, y) as i32 + noise(frame, x, y)).clamp(16, 235) as u8)
}

fn check_denoise(f: &Frame, got: &[[u8; 3]], (w, h): (u32, u32)) -> Result<String, String> {
    // The CPU mirror over the same sequence, in the shader's own units
    // (luma 0..1 before colour conversion).
    let n = 10u32;
    let mut acc: Vec<f32> = Vec::new();
    for i in 0..n {
        let fr = noisy_board(i);
        for y in 0..h {
            for x in 0..w {
                let v = luma_of(&fr, x, y) as f32 / 255.0;
                let k = (y * w + x) as usize;
                if i == 0 { acc.push(v) } else { acc[k] = temporal_reference(v, acc[k]) }
            }
        }
    }
    let to_rgb = |l: f32| expected_rgb((l * 255.0).round().clamp(0.0, 255.0) as u8, 128, 128, f.color)[0];
    let worst = got.iter().enumerate().map(|(i, p)| p[0].abs_diff(to_rgb(acc[i]))).max().unwrap_or(0);
    // Noise left, against the clean board, vs one noisy frame's.
    let rms = |pick: &dyn Fn(u32, u32) -> u8| {
        let mut s = 0.0f64;
        for y in 0..h {
            for x in 0..w {
                let clean = expected_rgb(board(x, y), 128, 128, f.color)[0] as f64;
                s += (pick(x, y) as f64 - clean).powi(2);
            }
        }
        (s / (w * h) as f64).sqrt()
    };
    let last = noisy_board(n - 1);
    let before = rms(&|x, y| conv(&last, x, y)[0]);
    let after = rms(&|x, y| got[(y * w + x) as usize][0]);
    if worst > 2 {
        Err(format!("max error {worst}/255 against temporal_reference"))
    } else if before / after.max(0.01) < 2.0 {
        Err(format!("noise only {before:.2} -> {after:.2} rms"))
    } else {
        Ok(format!("noise {before:.2} -> {after:.2} rms ({:.1}x), max error {worst}/255 vs mirror", before / after))
    }
}

fn square_at(x0: u32) -> Frame {
    nv12(64, 48, move |x, y| if (x0..x0 + 8).contains(&x) && (16..32).contains(&y) { 200 } else { 100 })
}

fn check_no_ghost(f: &Frame, got: &[[u8; 3]], (w, _h): (u32, u32)) -> Result<String, String> {
    let (bg, sq) = (expected_rgb(100, 128, 128, f.color)[0], expected_rgb(200, 128, 128, f.color)[0]);
    let px = |x: u32, y: u32| got[(y * w + x) as usize][0];
    let old = px(12, 24);
    let new = px(44, 24);
    if old.abs_diff(bg) > 3 {
        Err(format!("ghost where the square was: {old} (background {bg})"))
    } else if new.abs_diff(sq) > 3 {
        Err(format!("new position lagging: {new} (square {sq})"))
    } else {
        Ok(format!("moved square shown at once ({new}), old place clean ({old})"))
    }
}

/// A 10-bit P010 frame, BT.2020 limited, with `transfer`: luma ramps across,
/// chroma varies in both axes so gamut and range constants are exercised.
fn p010(w: u32, h: u32, transfer: vidkit_core::codec::Transfer) -> Frame {
    let mut luma = Vec::with_capacity((w * h * 2) as usize);
    for y in 0..h {
        for x in 0..w {
            let code = (64 + (x * 876) / (w - 1)).saturating_sub(y % 3).max(64) as u16;
            luma.extend_from_slice(&(code << 6).to_le_bytes());
        }
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut chroma = Vec::with_capacity((cw * ch * 4) as usize);
    for y in 0..ch {
        for x in 0..cw {
            let cb = (512 + x as i32 * 300 / cw as i32 - 150) as u16;
            let cr = (512 + y as i32 * 300 / ch as i32 - 150) as u16;
            chroma.extend_from_slice(&(cb << 6).to_le_bytes());
            chroma.extend_from_slice(&(cr << 6).to_le_bytes());
        }
    }
    Frame {
        data: FrameData::Cpu(CpuPlanes { data: vec![luma, chroma], strides: vec![w * 2, w * 2] }),
        width: w,
        height: h,
        layout: PlaneLayout::P010,
        color: ColorInfo { matrix: vidkit_core::frame::Matrix::Bt2020Ncl, transfer, range: vidkit_core::frame::Range::Limited },
        pts: std::time::Duration::ZERO,
        duration: None,
        recycle: None,
        grain: None,
    }
}

/// An 8-bit I420 frame (three planes), luma and chroma varied in both axes:
/// the layout software VP9/AV1 decode now hands the GPU unconverted.
fn i420(w: u32, h: u32) -> Frame {
    let luma: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (16 + (x * 219) / (w - 1) - (y % 3)) as u8)).collect();
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let u: Vec<u8> = (0..ch).flat_map(|_| (0..cw).map(move |x| (128 + x as i32 * 150 / cw as i32 - 75) as u8)).collect();
    let v: Vec<u8> = (0..ch).flat_map(|y| (0..cw).map(move |_| (128 + y as i32 * 150 / ch as i32 - 75) as u8)).collect();
    Frame {
        data: FrameData::Cpu(CpuPlanes { data: vec![luma, u, v], strides: vec![w, cw, cw] }),
        width: w,
        height: h,
        layout: PlaneLayout::I420,
        color: ColorInfo::default(),
        pts: std::time::Duration::ZERO,
        duration: None,
        recycle: None,
        grain: None,
    }
}

/// The I420 pattern as a single-fd, padded-stride NV12 "dma-buf" -- a memfd,
/// since this machine has no dma-buf exporter (no /dev/dri, no udmabuf).
/// EGL refuses to import it, which exercises the path a real driver takes
/// when it cannot import: the mmap fallback, then an upload.
fn dmabuf_nv12(w: u32, h: u32) -> Frame {
    use std::os::fd::{FromRawFd, OwnedFd};
    let src = i420(w, h);
    let FrameData::Cpu(p) = &src.data else { unreachable!() };
    let (cw, ch) = (w.div_ceil(2) as usize, h.div_ceil(2) as usize);
    let stride = (w as usize + 32 + 63) & !63;
    let mut buf = vec![0u8; stride * h as usize + stride * ch];
    for y in 0..h as usize {
        buf[y * stride..y * stride + w as usize].copy_from_slice(&p.data[0][y * w as usize..(y + 1) * w as usize]);
    }
    for y in 0..ch {
        for x in 0..cw {
            let o = stride * h as usize + y * stride + 2 * x;
            buf[o] = p.data[1][y * cw + x];
            buf[o + 1] = p.data[2][y * cw + x];
        }
    }
    // SAFETY: memfd_create returns a new fd we own.
    let fd = unsafe { OwnedFd::from_raw_fd(libc::memfd_create(c"vk-selftest".as_ptr(), 0)) };
    std::io::Write::write_all(&mut std::fs::File::from(fd.try_clone().expect("dup")), &buf).expect("memfd write");
    Frame {
        data: FrameData::DmaBuf(vidkit_core::frame::DmaBufImage {
            planes: vec![vidkit_core::frame::DmaBufPlane { fd, offset: 0, stride: stride as u32 }],
            modifier: vidkit_core::dmabuf::DRM_FORMAT_MOD_LINEAR,
            fourcc: vidkit_core::dmabuf::DRM_FORMAT_NV12,
            owner: None,
        }),
        layout: PlaneLayout::Nv12,
        ..src
    }
}

/// The frame carrying its grain, as a decoder exporting grain delivers it.
fn with_grain(mut f: Frame) -> Frame {
    f.grain = Some(Arc::new(grain_params()));
    f
}

/// Film grain parameters shaped like SVT-AV1's (lag 3, overlap, all planes).
fn grain_params() -> vidkit_core::av1_grain::GrainParams {
    let mut y = [0i8; 24];
    for (i, c) in y.iter_mut().enumerate() {
        *c = ((i as i32 * 7) % 19 - 9) as i8;
    }
    let mut uv = [[0i8; 25]; 2];
    for i in 0..25 {
        uv[0][i] = ((i as i32 * 5) % 17 - 8) as i8;
        uv[1][i] = ((i as i32 * 3) % 13 - 6) as i8;
    }
    vidkit_core::av1_grain::GrainParams {
        seed: 20915,
        // Strong grain: at weak amplitudes a wrong overlap weight rarely
        // flips a rounded sample, and the test could not see it.
        y_points: vec![[0, 180], [54, 200], [134, 220], [255, 255]],
        cb_points: vec![[0, 200], [81, 230], [255, 255]],
        cr_points: vec![[0, 190], [134, 210], [255, 240]],
        chroma_scaling_from_luma: false,
        scaling_shift: 8,
        ar_coeff_lag: 3,
        ar_coeffs_y: y,
        ar_coeffs_uv: uv,
        ar_coeff_shift: 7,
        grain_scale_shift: 0,
        uv_mult: [0, 0],
        uv_luma_mult: [64, 64],
        uv_offset: [0, 0],
        overlap: true,
        clip_to_restricted_range: false,
    }
}

/// GPU-grained output vs the CPU reference: grain applied by vidkit-core
/// (byte-identical to dav1d), then the CPU converter.
fn check_grain(f: &Frame, got: &[[u8; 3]], (w, h): (u32, u32)) -> Result<String, String> {
    let FrameData::Cpu(pl) = &f.data else { return Err("not cpu".into()) };
    let (mut y, mut u, mut v) = (pl.data[0].clone(), pl.data[1].clone(), pl.data[2].clone());
    vidkit_core::av1_grain::apply_i420(&grain_params(), &mut y, &mut u, &mut v, w as usize, h as usize, false);
    let grained = CpuPlanes { data: vec![y, u, v], strides: pl.strides.clone() };
    let cpu = vidkit_core::planes_to_rgba8(&grained, f.layout, f.color, w as usize, h as usize).map_err(|e| e.to_string())?;
    let plain = vidkit_core::planes_to_rgba8(pl, f.layout, f.color, w as usize, h as usize).map_err(|e| e.to_string())?;
    let mut worst = 0u8;
    for (i, p) in got.iter().enumerate() {
        for c in 0..3 {
            worst = worst.max(p[c].abs_diff(cpu[i * 4 + c]));
        }
    }
    let changed = (0..got.len()).filter(|&i| (0..3).any(|c| cpu[i * 4 + c] != plain[i * 4 + c])).count();
    // A COARSE wiring check (the grained textures reach the conversion),
    // not the grain check: the grain itself is compared EXACTLY on the
    // planes ("grain planes exact"). Here the texture unit's fixed-point
    // bilinear chroma weights meet strong, high-frequency grained chroma --
    // measured 3/255 on correct planes -- and one chroma code becomes ~2 RGB
    // codes through the x2.02 blue coefficient.
    if worst > 4 {
        Err(format!("max error {worst}/255 against the CPU grain reference"))
    } else if changed * 2 < got.len() {
        Err(format!("grain changed only {changed} of {} pixels: the fixture is not exercising it", got.len()))
    } else {
        Ok(format!("max error {worst}/255, grain on {changed} of {} pixels", got.len()))
    }
}

/// GPU vs vidkit-core's CPU converter on the whole frame.
fn check_vs_cpu(f: &Frame, got: &[[u8; 3]], (w, h): (u32, u32)) -> Result<String, String> {
    // A dma-buf is checked against the pattern it was built from, NOT read
    // back through the same plane mapping the import used (a wrong chroma
    // offset would then agree with itself -- it did, until this).
    let source;
    let f = match &f.data {
        FrameData::DmaBuf(_) => {
            source = i420(f.width, f.height);
            &source
        }
        _ => f,
    };
    let FrameData::Cpu(pl) = &f.data else { return Err("not cpu".into()) };
    let layout = f.layout;
    let cpu = vidkit_core::planes_to_rgba8(pl, layout, f.color, w as usize, h as usize).map_err(|e| e.to_string())?;
    let mut worst = 0u8;
    let mut at = 0usize;
    for (i, p) in got.iter().enumerate() {
        for c in 0..3 {
            let d = p[c].abs_diff(cpu[i * 4 + c]);
            if d > worst {
                worst = d;
                at = i;
            }
        }
    }
    let spread = got.iter().map(|p| p[0]).max().unwrap_or(0) as i32 - got.iter().map(|p| p[0]).min().unwrap_or(0) as i32;
    if worst > 3 {
        Err(format!("max error {worst}/255 against the CPU converter at pixel {at} (gpu {:?})", got[at]))
    } else if spread < 100 {
        Err(format!("output range only {spread}: the fixture did not exercise the curve"))
    } else {
        Ok(format!("max error {worst}/255, output spans {spread} codes"))
    }
}

fn cases() -> Vec<Case> {
    // 2-D pattern: soft diagonal field plus a bright square near the TOP-LEFT,
    // so a vertical or horizontal flip anywhere shows as a large error.
    let pattern = |x: u32, y: u32| {
        if x < 12 && y < 8 && x > 3 && y > 2 { 220 } else { (40 + (x * 3 + y * 2) % 120) as u8 }
    };
    vec![
        Case {
            two_pass: false,
            grain: None,
            name: "temporal denoise, board",
            frame: at_pts(noisy_board(9), 9),
            history: (0..9).map(|i| at_pts(noisy_board(i), i as usize)).collect(),
            enhance: Enhance { temporal: true, ..Enhance::default() },
            out: (64, 48),
            check: check_denoise,
        },
        Case {
            two_pass: false,
            grain: None,
            name: "temporal, no ghosting",
            frame: at_pts(square_at(40), 8),
            history: (0..8).map(|i| at_pts(square_at(8), i)).collect(),
            enhance: Enhance { temporal: true, ..Enhance::default() },
            out: (64, 48),
            check: check_no_ghost,
        },
        Case { two_pass: false, grain: None, name: "HDR10 PQ, P010, BT.2020", history: vec![], frame: p010(64, 48, vidkit_core::codec::Transfer::Pq), enhance: Enhance::default(), out: (64, 48), check: check_vs_cpu },
        Case { two_pass: false, grain: None, name: "HLG, P010, BT.2020", history: vec![], frame: p010(64, 48, vidkit_core::codec::Transfer::Hlg), enhance: Enhance::default(), out: (64, 48), check: check_vs_cpu },
        Case { two_pass: false, grain: Some(grain_params()), name: "AV1 film grain", history: vec![], frame: with_grain(i420(256, 160)), enhance: Enhance::default(), out: (256, 160), check: check_grain },
        Case { two_pass: false, grain: None, name: "dma-buf NV12", history: vec![], frame: dmabuf_nv12(66, 48), enhance: Enhance::default(), out: (66, 48), check: check_vs_cpu },
        Case { two_pass: false, grain: None, name: "I420, three planes", history: vec![], frame: i420(64, 48), enhance: Enhance::default(), out: (64, 48), check: check_vs_cpu },
        Case { two_pass: false, grain: None, name: "plain conversion", history: vec![], frame: nv12(64, 48, pattern), enhance: Enhance::default(), out: (64, 48), check: check_plain },
        Case { two_pass: true, grain: None, name: "RCAS two-pass (offscreen)", history: vec![], frame: nv12(64, 48, pattern), enhance: Enhance { scaler: Scaler::Bilinear, sharpen: Some(0.2), temporal: false }, out: (64, 48), check: check_rcas },
        Case { two_pass: false, grain: None, name: "RCAS fused (one pass)", history: vec![], frame: nv12(64, 48, pattern), enhance: Enhance { scaler: Scaler::Bilinear, sharpen: Some(0.2), temporal: false }, out: (64, 48), check: check_rcas },
        Case { two_pass: false, grain: None, name: "bicubic 2x on a ramp", history: vec![], frame: nv12(32, 16, |x, _| (40 + x * 4) as u8), enhance: Enhance { scaler: Scaler::Bicubic, sharpen: None, temporal: false }, out: (64, 32), check: check_ramp },
        Case { two_pass: false, grain: None, name: "bicubic 2x on a step", history: vec![], frame: nv12(32, 16, |x, _| if x < 16 { 60 } else { 200 }), enhance: Enhance { scaler: Scaler::Bicubic, sharpen: None, temporal: false }, out: (64, 32), check: check_step_overshoot },
    ]
}

pub struct SelfTest {
    gpu: Option<Arc<Mutex<Nv12Gpu>>>,
    cases: Vec<Case>,
    at: usize,
    shown: u32,
    readback: Arc<Mutex<Option<Vec<[u8; 3]>>>>,
    failed: bool,
}

impl SelfTest {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let gpu = cc.gl.as_ref().and_then(|gl| unsafe { Nv12Gpu::new(gl) }.map_err(|e| eprintln!("GPU SELFTEST: shader build failed: {e}")).ok());
        Self { gpu: gpu.map(|g| Arc::new(Mutex::new(g))), cases: cases(), at: 0, shown: 0, readback: Arc::new(Mutex::new(None)), failed: false }
    }
}

impl eframe::App for SelfTest {
    // eframe 0.34 requires `ui`; the player drives everything from the
    // deprecated `update` the same way, so this does too.
    fn ui(&mut self, _ui: &mut egui::Ui, _f: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Some(gpu) = self.gpu.clone() else {
            println!("GPU SELFTEST FAIL: no GL context or shaders did not build");
            std::process::exit(1);
        };
        if self.at >= self.cases.len() {
            println!("GPU SELFTEST {}", if self.failed { "FAILED" } else { "PASSED" });
            std::process::exit(if self.failed { 1 } else { 0 });
        }
        if let Some(px) = self.readback.lock().ok().and_then(|mut r| r.take()) {
            let c = &self.cases[self.at];
            match (c.check)(&c.frame, &px, c.out) {
                Ok(m) => println!("  ok   {:<24} {m}", c.name),
                Err(m) => {
                    println!("  FAIL {:<24} {m}", c.name);
                    self.failed = true;
                }
            }
            self.at += 1;
            self.shown = 0;
            ctx.request_repaint();
            return;
        }
        let c = &self.cases[self.at];
        // One upload per update: history first, then the checked frame. The
        // temporal pass runs per upload, so this is one step per frame.
        let n = c.history.len() as u32;
        if self.shown <= n {
            let f = if self.shown < n { &c.history[self.shown as usize] } else { &c.frame };
            if let (Ok(mut g), Some(gl)) = (gpu.lock(), frame.gl()) {
                g.enhance = c.enhance;
                g.fused_rcas = !c.two_pass;
                // The grain arrives ON THE FRAME, as the decoder delivers it
                // (Frame::grain); the presenter's override stays off.
                g.grain_params = None;
                if let Err(e) = unsafe { g.upload(gl, f) } {
                    println!("GPU SELFTEST FAIL: upload: {e}");
                    std::process::exit(1);
                }
                if f.is_zero_copy() {
                    println!("       dma-buf: {} zero-copy frames; import: {}", g.zero_copy_frames, g.import_note.as_deref().unwrap_or("ok"));
                }
                // The grain stage itself, exactly: the GPU's grained planes
                // against vidkit-core's reference (byte-identical to dav1d).
                if c.grain.is_some() {
                    if let (Some((gy, gc)), FrameData::Cpu(pl)) = (unsafe { g.read_grain_planes(gl) }, &f.data) {
                        let (w, h) = (f.width as usize, f.height as usize);
                        let (mut y, mut u, mut v) = (pl.data[0].clone(), pl.data[1].clone(), pl.data[2].clone());
                        vidkit_core::av1_grain::apply_i420(c.grain.as_ref().unwrap(), &mut y, &mut u, &mut v, w, h, false);
                        let bad_y: Vec<(usize, usize, u8, u8)> = (0..w * h).filter(|&i| gy[i] != y[i]).map(|i| (i % w, i / w, gy[i], y[i])).collect();
                        let cw = w.div_ceil(2);
                        let bad_c: Vec<(usize, usize, usize, u8, u8)> = (0..u.len()).flat_map(|i| [(0, i), (1, i)]).filter(|&(k, i)| gc[i * 2 + k] != if k == 0 { u[i] } else { v[i] }).map(|(k, i)| (k, i % cw, i / cw, gc[i * 2 + k], if k == 0 { u[i] } else { v[i] })).collect();
                        if bad_y.is_empty() && bad_c.is_empty() {
                            println!("  ok   grain planes exact      {}x{} luma + chroma byte-identical to the CPU reference", w, h);
                        } else {
                            println!("  FAIL grain planes exact      luma {} / chroma {} samples differ; first {:?} {:?}", bad_y.len(), bad_c.len(), bad_y.first(), bad_c.first());
                            self.failed = true;
                        }
                    } else {
                        println!("  FAIL grain planes exact      the GPU grain pass did not run");
                        self.failed = true;
                    }
                }
            }
        }
        self.shown += 1;
        let ppp = ctx.pixels_per_point();
        let (w, h) = c.out;
        egui::CentralPanel::default().show(ctx, |ui| {
            let r = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(w as f32 / ppp, h as f32 / ppp));
            let (g2, rb, grab) = (gpu.clone(), self.readback.clone(), self.shown >= n + 2);
            ui.painter().add(egui::PaintCallback {
                rect: r,
                callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |info, painter| {
                    let vp = info.viewport_in_pixels();
                    let gl = painter.gl();
                    if let Ok(mut g) = g2.lock() {
                        unsafe { g.paint(gl, (vp.width_px, vp.height_px)) };
                    }
                    if grab {
                        use eframe::glow::HasContext;
                        let (vw, vh) = (vp.width_px, vp.height_px);
                        let mut buf = vec![0u8; (vw * vh * 4) as usize];
                        unsafe {
                            gl.read_pixels(vp.left_px, vp.from_bottom_px, vw, vh, eframe::glow::RGBA, eframe::glow::UNSIGNED_BYTE, eframe::glow::PixelPackData::Slice(Some(&mut buf)));
                        }
                        // GL rows are bottom-up; the checks are top-down.
                        let mut out = Vec::with_capacity((vw * vh) as usize);
                        for row in (0..vh).rev() {
                            for x in 0..vw {
                                let i = ((row * vw + x) * 4) as usize;
                                out.push([buf[i], buf[i + 1], buf[i + 2]]);
                            }
                        }
                        if let Ok(mut r) = rb.lock() {
                            *r = Some(out);
                        }
                    }
                })),
            });
        });
        ctx.request_repaint();
    }
}
