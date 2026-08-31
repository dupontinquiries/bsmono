//! ground_lab — the procedural-surface case.
//!
//! A game draws its ground as a surface plus a closed-form pattern
//! (value noise, fBm, a tiling function). When the camera moves, the
//! screen fills with high-frequency detail that changes every pixel
//! every frame, while the TRUE description length of the whole shot is
//! a noise seed, a few parameters, and a camera path — tens of bytes.
//! A pixel-statistics coder cannot reach that bound because it does not
//! know the pattern is a function; it sees only aliasing-grade noise,
//! which is the most expensive thing it can be asked to code.
//!
//! This is the strongest form of the FAO argument, and it is worth
//! measuring rather than asserting, because there are two ways it could
//! fail to matter: the conventional coder might handle it more cheaply
//! than expected, and the codec's own 2D plate might not capture it
//! either (it does not — perspective ground motion is not a translation,
//! so near ground sweeps past while far ground barely moves).
//!
//! The canonical here is NOT a mesh. Tessellating a ground plane finely
//! enough to carry pixel-rate noise would need roughly a vertex per
//! pixel, which throws away the entire point. The canonical is the
//! FUNCTION — surface plus pattern plus parameters — evaluated at the
//! decoder's pose. That is a real gap in `engine_scene`, which today
//! only carries geometry, and this bin is the evidence for widening it.

use fao_codec::residual::ResidualOpts;
use fao_codec::v11_plate as plate;

const W: usize = 640;
const H: usize = 384;

/// The entire world model. In a real integration these fields come from
/// the game's own terrain parameters; on the wire they are ~24 bytes.
#[derive(Clone, Copy)]
struct Ground {
    seed: u32,
    scale: f32,
    octaves: u32,
    horizon_tint: [f32; 3],
}

fn hash2(x: i32, z: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6b343) ^ (z as u32).wrapping_mul(0xd8163841) ^ seed;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a2d39);
    h ^= h >> 15;
    (h & 0xffffff) as f32 / 16777215.0
}

fn value_noise(x: f32, z: f32, seed: u32) -> f32 {
    let (xi, zi) = (x.floor(), z.floor());
    let (fx, fz) = (x - xi, z - zi);
    // Smoothstep so the field is continuous under camera motion — a
    // nearest-neighbour lattice would alias in a way that flatters the
    // model-based coder unfairly.
    let (ux, uz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let (x0, z0) = (xi as i32, zi as i32);
    let a = hash2(x0, z0, seed);
    let b = hash2(x0 + 1, z0, seed);
    let c = hash2(x0, z0 + 1, seed);
    let d = hash2(x0 + 1, z0 + 1, seed);
    let top = a + (b - a) * ux;
    let bot = c + (d - c) * ux;
    top + (bot - top) * uz
}

fn fbm(x: f32, z: f32, g: &Ground) -> f32 {
    let (mut f, mut amp, mut sum, mut norm) = (1.0f32, 1.0f32, 0.0f32, 0.0f32);
    for _ in 0..g.octaves {
        sum += amp * value_noise(x * f, z * f, g.seed);
        norm += amp;
        f *= 2.03;
        amp *= 0.5;
    }
    sum / norm.max(1e-6)
}

/// Camera: position plus yaw, looking slightly down. A deliberately
/// plain pinhole so the lab renderer is inspectable rather than a black
/// box — the point of the experiment is the CONTENT, not the camera.
#[derive(Clone, Copy)]
struct Cam {
    x: f32,
    y: f32,
    z: f32,
    yaw: f32,
    pitch: f32,
    focal: f32,
}

fn render(cam: &Cam, g: &Ground) -> Vec<u8> {
    let mut rgb = vec![0u8; W * H * 3];
    let (sy, cy) = (cam.yaw.sin(), cam.yaw.cos());
    let (sp, cp) = (cam.pitch.sin(), cam.pitch.cos());
    for py in 0..H {
        for px in 0..W {
            // Ray in camera space, then yaw/pitch into world.
            let dx = (px as f32 - W as f32 / 2.0) / cam.focal;
            let dy = (py as f32 - H as f32 / 2.0) / cam.focal;
            let (cxr, cyr, czr) = (dx, dy * cp + sp, cp - dy * sp);
            let (wx, wy, wz) = (cxr * cy + czr * sy, cyr, czr * cy - cxr * sy);
            let i = (py * W + px) * 3;
            if wy <= 1e-4 {
                // Sky: a smooth gradient, cheap for any coder, present
                // so the frame is not 100% noise.
                let t = (-wy).min(1.0);
                for c in 0..3 {
                    rgb[i + c] = (g.horizon_tint[c] * (140.0 + 80.0 * t)).clamp(0.0, 255.0) as u8;
                }
                continue;
            }
            // Intersect the plane y = 0 from height cam.y.
            let t = cam.y / wy;
            if t > 4000.0 {
                for c in 0..3 {
                    rgb[i + c] = (g.horizon_tint[c] * 150.0) as u8;
                }
                continue;
            }
            let (gx, gz) = (cam.x + wx * t, cam.z + wz * t);
            let n = fbm(gx * g.scale, gz * g.scale, g);
            let shade = 60.0 + 170.0 * n;
            rgb[i] = (shade * 0.95) as u8;
            rgb[i + 1] = (shade * 0.88) as u8;
            rgb[i + 2] = (shade * 0.70) as u8;
        }
    }
    rgb
}

fn to_yuv(rgb: &[u8]) -> plate::Yuv420 {
    let mut f = plate::Yuv420::new(W, H);
    let cw = W / 2;
    let (mut us, mut vs) = (vec![0u32; cw * (H / 2)], vec![0u32; cw * (H / 2)]);
    for y in 0..H {
        for x in 0..W {
            let i = (y * W + x) * 3;
            let (r, gg, b) = (rgb[i] as f32, rgb[i + 1] as f32, rgb[i + 2] as f32);
            f.y[y * W + x] = (0.299 * r + 0.587 * gg + 0.114 * b).round().clamp(0.0, 255.0) as u8;
            let ci = (y / 2) * cw + x / 2;
            us[ci] += (-0.168736 * r - 0.331264 * gg + 0.5 * b + 128.0).round().clamp(0.0, 255.0) as u32;
            vs[ci] += (0.5 * r - 0.418688 * gg - 0.081312 * b + 128.0).round().clamp(0.0, 255.0) as u32;
        }
    }
    for (o, s) in f.u.iter_mut().zip(&us) {
        *o = (s / 4) as u8;
    }
    for (o, s) in f.v.iter_mut().zip(&vs) {
        *o = (s / 4) as u8;
    }
    f
}

fn psnr(a: &[plate::Yuv420], b: &[plate::Yuv420]) -> f64 {
    let (mut se, mut n) = (0.0f64, 0u64);
    for (x, y) in a.iter().zip(b) {
        for (p, q) in x.y.iter().zip(&y.y) {
            let d = *p as f64 - *q as f64;
            se += d * d;
            n += 1;
        }
    }
    if se == 0.0 {
        return 99.0;
    }
    10.0 * (255.0f64 * 255.0 * n as f64 / se).log10()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let n: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(48);
    let g = Ground { seed: 0x51ed270b, scale: 0.05, octaves: 4, horizon_tint: [1.0, 0.72, 0.55] };

    // Camera translates forward and yaws — the motion that turns a
    // static pattern into full-frame high-frequency change.
    let cams: Vec<Cam> = (0..n)
        .map(|i| {
            let t = i as f32;
            Cam {
                x: 3.0 * t,
                y: 28.0,
                z: 11.0 * t,
                yaw: 0.004 * t,
                pitch: 0.30,
                focal: W as f32 * 0.9,
            }
        })
        .collect();
    // The footage is NOT the canonical. Rendering the footage from the
    // same function at the same camera gives 99 dB at 2,904 B, which
    // measures only that the renderer is deterministic — the same
    // circularity engine_lab hit. Real footage of a procedural ground
    // still differs from the bare surface: props and characters sit on
    // it, and the delivered picture has been through lighting,
    // post-processing and usually a lossy encode. Both are injected
    // into the FOOTAGE ONLY; the canonical stays the clean function.
    let frames: Vec<plate::Yuv420> = cams
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut rgb = render(c, &g);
            // Props: a handful of solid markers at fixed world positions,
            // projected per frame. They are what the canonical cannot
            // know, and they move through the frame with the camera.
            for k in 0..6 {
                let (wx, wz) = (140.0 + k as f32 * 90.0, 260.0 + (k as f32 * 137.0) % 300.0);
                let (dx, dz) = (wx - c.x, wz - c.z);
                let (sy, cy) = (c.yaw.sin(), c.yaw.cos());
                let (rx, rz) = (dx * cy - dz * sy, dz * cy + dx * sy);
                if rz < 20.0 {
                    continue;
                }
                let px = W as f32 / 2.0 + c.focal * rx / rz;
                let py = H as f32 / 2.0 + c.focal * (c.y / rz - c.pitch);
                let rad = (c.focal * 6.0 / rz).clamp(2.0, 90.0);
                let (x0, x1) = ((px - rad).max(0.0) as usize, (px + rad).min(W as f32 - 1.0) as usize);
                let (y0, y1) = ((py - rad).max(0.0) as usize, (py + rad).min(H as f32 - 1.0) as usize);
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        if ((x as f32 - px).powi(2) + (y as f32 - py).powi(2)).sqrt() <= rad {
                            let i = (y * W + x) * 3;
                            rgb[i] = 40;
                            rgb[i + 1] = 190;
                            rgb[i + 2] = 220;
                        }
                    }
                }
            }
            // Photometric drift: exposure the canonical has no way to know.
            let gain = 1.0 + 0.05 * ((i as f32) * 0.19).sin();
            for v in rgb.iter_mut() {
                *v = ((*v as f32) * gain).round().clamp(0.0, 255.0) as u8;
            }
            to_yuv(&rgb)
        })
        .collect();

    // Write the footage for an external AV1 anchor.
    if let Some(path) = args.get(1) {
        use std::io::Write;
        let mut f = std::fs::File::create(path).expect("create y4m");
        writeln!(f, "YUV4MPEG2 W{W} H{H} F60:1 Ip A1:1 C420mpeg2").unwrap();
        for fr in &frames {
            f.write_all(b"FRAME\n").unwrap();
            f.write_all(&fr.y).unwrap();
            f.write_all(&fr.u).unwrap();
            f.write_all(&fr.v).unwrap();
        }
        eprintln!("wrote {path}");
    }

    let opts = ResidualOpts { dead_zone: 32.0, ..ResidualOpts::default() };
    let bytes = |r: &plate::PlateEncodeReportV2| -> usize {
        r.plate_bytes
            + r.motion_bytes
            + r.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).sum::<usize>()
    };

    // Branch 1: the codec as it stands — a 2D plate. Expected to fail,
    // and worth showing rather than assuming: perspective ground motion
    // is not a translation, so near ground sweeps past while far ground
    // barely moves and no single warp fits both.
    let m = plate::estimate_motions(&frames);
    let pl = plate::build_plate(&frames, &m);
    let r2d = plate::encode_gop_v3(&frames, &m, &pl, &opts);
    println!(
        "v11 2D plate:              {:>9} B @ {:.3} dB   (plate {} B)",
        bytes(&r2d),
        psnr(&frames, &r2d.recon),
        r2d.plate_bytes
    );

    // Branch 2: no world model at all, the control.
    let rnp = plate::encode_gop_no_plate(&frames, &opts);
    println!(
        "v11 no model (prev-MC):    {:>9} B @ {:.3} dB",
        bytes(&rnp),
        psnr(&frames, &rnp.recon)
    );

    // Branch 3: the PROCEDURAL canonical. The decoder holds the same
    // function and the same camera path, so the prediction is the
    // surface itself. Wire cost: the Ground parameters (seed u32, scale
    // f32, octaves u32, tint 3xf32 = 24 B) plus 24 B/frame of camera.
    let mut pred = |fi: usize| to_yuv(&render(&cams[fi], &g));
    let (rproc, _) = plate::encode_gop_v3_core(&frames, &mut pred, 24, n * 24, &opts, None);
    println!(
        "v11 procedural canonical:  {:>9} B @ {:.3} dB   (model 24 B + 24 B/frame camera)",
        bytes(&rproc),
        psnr(&frames, &rproc.recon)
    );

    // Branch 4: control — the right function, the WRONG camera path.
    // Catches a provider that helps for the wrong reason.
    let mut wrong = |fi: usize| {
        let mut c = cams[fi];
        c.x += 400.0;
        c.z += 400.0;
        to_yuv(&render(&c, &g))
    };
    let (rw, _) = plate::encode_gop_v3_core(&frames, &mut wrong, 24, n * 24, &opts, None);
    println!(
        "  control, WRONG camera:   {:>9} B @ {:.3} dB",
        bytes(&rw),
        psnr(&frames, &rw.recon)
    );
}
