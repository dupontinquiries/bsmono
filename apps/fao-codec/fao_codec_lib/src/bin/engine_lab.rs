//! engine_lab — the citation thesis with inverse rendering REMOVED.
//!
//! Every citation experiment so far has failed, and each failure had a
//! different proximate cause: film grain decorrelation, no parallax in
//! the material, a pose model missing its forward axis, phase
//! correlation finding no peak on gradients, and finally the structural
//! one — a 2D plate is viewpoint-locked, so it cannot serve a camera
//! that has moved. Underneath all of them sits one confound: the
//! canonical and the poses were RECOVERED FROM PIXELS, so a negative
//! never distinguishes "the thesis is wrong" from "reconstruction is
//! hard".
//!
//! This bin removes the confound. A synthetic scene is rendered through
//! the codec's own rasterizer to produce the "footage"; the SAME mesh
//! and the SAME poses are then handed to the encoder as a canonical.
//! That is the engine-derived configuration — what blackstar-rs could
//! supply from `replay::Frame` plus its own assets — with no
//! triangulation, no phase correlation, and no pose estimation in the
//! path.
//!
//! It answers exactly one question, and it is the question the whole
//! v11 line rests on: WHEN THE WORLD MODEL IS PERFECT AND FREE TO
//! REUSE, does citing it beat coding each shot on its own? If the
//! answer is no here, no amount of reconstruction work can save the
//! thesis. If yes, then every measured failure so far is an
//! inverse-rendering problem and the roadmap follows from that.
//!
//! Usage: engine_lab [frames_per_gop]

use fao_codec::residual::ResidualOpts;
use fao_codec::v11_plate as plate;
use fao_codec::v8_rasterizer::render_mesh;
use fao_codec::v8_reconstruction_codec::{CameraPose, SceneMesh};
use fao_codec::geometry::Vec3;

/// A blocky "arena": a floor grid plus walls, coloured by a cheap
/// procedural texture so the content is not trivially flat (flat
/// content would let prev-MC win everything and prove nothing).
fn build_arena() -> SceneMesh {
    let mut m = SceneMesh::default();
    let tex = |x: i32, z: i32, face: u32| -> [f32; 3] {
        let n = ((x * 73 + z * 151 + face as i32 * 31) % 101).unsigned_abs() as f32 / 101.0;
        let n2 = ((x * 37 + z * 91 + face as i32 * 17) % 89).unsigned_abs() as f32 / 89.0;
        [0.25 + 0.7 * n, 0.25 + 0.7 * n2, 0.3 + 0.6 * (1.0 - n)]
    };
    let mut quad = |a: Vec3, b: Vec3, c: Vec3, d: Vec3, col: [f32; 3]| {
        let base = m.vertices.len() as u32;
        for v in [a, b, c, d] {
            m.vertices.push(v);
            m.vertex_colors.push(col);
        }
        m.indices.extend_from_slice(&[base, base + 1, base + 2]);
        m.indices.extend_from_slice(&[base, base + 2, base + 3]);
    };
    let s = 120.0f32;
    // Floor tiles.
    for gz in -8..8 {
        for gx in -8..8 {
            let (x0, z0) = (gx as f32 * s, gz as f32 * s);
            quad(
                Vec3::new(x0, 0.0, z0),
                Vec3::new(x0 + s, 0.0, z0),
                Vec3::new(x0 + s, 0.0, z0 + s),
                Vec3::new(x0, 0.0, z0 + s),
                tex(gx, gz, 0),
            );
        }
    }
    // Four walls, tiled vertically so there is structure at depth.
    for gy in 0..4 {
        let (y0, y1) = (gy as f32 * s, (gy + 1) as f32 * s);
        for gx in -8..8 {
            let x0 = gx as f32 * s;
            quad(
                Vec3::new(x0, y0, -8.0 * s),
                Vec3::new(x0 + s, y0, -8.0 * s),
                Vec3::new(x0 + s, y1, -8.0 * s),
                Vec3::new(x0, y1, -8.0 * s),
                tex(gx, gy, 1),
            );
            quad(
                Vec3::new(x0, y0, 8.0 * s),
                Vec3::new(x0 + s, y0, 8.0 * s),
                Vec3::new(x0 + s, y1, 8.0 * s),
                Vec3::new(x0, y1, 8.0 * s),
                tex(gx, gy, 2),
            );
        }
        for gz in -8..8 {
            let z0 = gz as f32 * s;
            quad(
                Vec3::new(-8.0 * s, y0, z0),
                Vec3::new(-8.0 * s, y0, z0 + s),
                Vec3::new(-8.0 * s, y1, z0 + s),
                Vec3::new(-8.0 * s, y1, z0),
                tex(gz, gy, 3),
            );
            quad(
                Vec3::new(8.0 * s, y0, z0),
                Vec3::new(8.0 * s, y0, z0 + s),
                Vec3::new(8.0 * s, y1, z0 + s),
                Vec3::new(8.0 * s, y1, z0),
                tex(gz, gy, 4),
            );
        }
    }
    m
}

fn pose_at(t: f32, w: usize, h: usize) -> CameraPose {
    // A camera orbiting inside the arena: real translation, so the two
    // GOPs see the world from genuinely different places — the
    // condition under which a 2D plate was measured to fail.
    let r = 520.0;
    CameraPose {
        position: Vec3::new(r * t.cos(), 190.0 + 40.0 * (t * 0.7).sin(), r * t.sin()),
        rotation: [1.0, 0.0, 0.0, 0.0],
        focal: [w as f32, w as f32],
        principal: [w as f32 / 2.0, h as f32 / 2.0],
        distortion: 0.0,
    }
}

fn rgb_to_yuv(rgb: &[u8], w: usize, h: usize) -> plate::Yuv420 {
    let mut f = plate::Yuv420::new(w, h);
    let cw = w / 2;
    let mut us = vec![0u32; cw * (h / 2)];
    let mut vs = vec![0u32; cw * (h / 2)];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 3;
            let (r, g, b) = (rgb[i] as f32, rgb[i + 1] as f32, rgb[i + 2] as f32);
            f.y[y * w + x] = (0.299 * r + 0.587 * g + 0.114 * b).round().clamp(0.0, 255.0) as u8;
            let ci = (y / 2) * cw + x / 2;
            us[ci] += (-0.168736 * r - 0.331264 * g + 0.5 * b + 128.0).round().clamp(0.0, 255.0) as u32;
            vs[ci] += (0.5 * r - 0.418688 * g - 0.081312 * b + 128.0).round().clamp(0.0, 255.0) as u32;
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
    let mut se = 0.0f64;
    let mut n = 0u64;
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
    let n: usize = args.first().and_then(|s| s.parse().ok()).unwrap_or(24);
    let (w, h) = (640, 384);
    let mesh = build_arena();
    eprintln!(
        "arena: {} verts {} tris | {}x{}, 2 GOPs of {n}",
        mesh.vertices.len(),
        mesh.indices.len() / 3,
        w,
        h
    );

    // Render the "footage". GOP A and GOP B are far apart on the orbit,
    // so B's camera is somewhere A never stood.
    let mut poses_a = Vec::new();
    let mut poses_b = Vec::new();
    let (mut ga, mut gb) = (Vec::new(), Vec::new());
    // THE FOOTAGE IS NOT THE CANONICAL, and this is the whole
    // difference between a ceiling and a result. A first version
    // rendered the footage from the same mesh at the same poses and
    // reported 99 dB at 1,176 B — bit-exact, because the prediction WAS
    // the source. That measures nothing except that rendering is
    // deterministic.
    //
    // Real footage differs from any static canonical in two ways that
    // matter, so both are injected here and only into the FOOTAGE:
    //   * a dynamic object the canonical cannot contain (the ball) —
    //     stands for every actor, projectile and particle;
    //   * a per-frame photometric drift (exposure/tone) — stands for
    //     dynamic lighting, post-processing and encoder drift, and is
    //     the thing that historically defeated cross-take citation on
    //     film.
    // The canonical stays the clean static arena, which is exactly what
    // an engine could hand over.
    let ball_at = |t: f32| -> (f32, f32, f32) {
        (300.0 * (t * 3.0).cos(), 120.0 + 90.0 * (t * 5.0).sin().abs(), 300.0 * (t * 3.0).sin())
    };
    let dress = |rgb: &mut Vec<u8>, pose: &CameraPose, t: f32, fi: usize| {
        // Project a bright sphere-ish blob at the ball's world position.
        let (bx, by, bz) = ball_at(t);
        let (dx, dy, dz) = (bx - pose.position.x, by - pose.position.y, bz - pose.position.z);
        if dz > 1.0 {
            let sx = pose.principal[0] + pose.focal[0] * dx / dz;
            let sy = pose.principal[1] + pose.focal[1] * dy / dz;
            let rad = (pose.focal[0] * 34.0 / dz).max(3.0);
            let (x0, x1) = ((sx - rad).max(0.0) as usize, (sx + rad).min(w as f32 - 1.0) as usize);
            let (y0, y1) = ((sy - rad).max(0.0) as usize, (sy + rad).min(h as f32 - 1.0) as usize);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let d = ((x as f32 - sx).powi(2) + (y as f32 - sy).powi(2)).sqrt();
                    if d <= rad {
                        let i = (y * w + x) * 3;
                        let shade = 1.0 - 0.5 * d / rad;
                        rgb[i] = (255.0 * shade) as u8;
                        rgb[i + 1] = (200.0 * shade) as u8;
                        rgb[i + 2] = (40.0 * shade) as u8;
                    }
                }
            }
        }
        // Photometric drift: a slow exposure wobble the canonical has no
        // way to know about.
        let gain = 1.0 + 0.06 * ((fi as f32) * 0.21).sin();
        for v in rgb.iter_mut() {
            *v = ((*v as f32) * gain).round().clamp(0.0, 255.0) as u8;
        }
    };
    for i in 0..n {
        let ta = i as f32 * 0.004;
        let tb = 1.9 + i as f32 * 0.004;
        let (pa, pb) = (pose_at(ta, w, h), pose_at(tb, w, h));
        let mut ra = render_mesh(&mesh, None, &pa, w, h).expect("render A");
        let mut rb = render_mesh(&mesh, None, &pb, w, h).expect("render B");
        dress(&mut ra, &pa, ta, i);
        dress(&mut rb, &pb, tb, i);
        ga.push(rgb_to_yuv(&ra, w, h));
        gb.push(rgb_to_yuv(&rb, w, h));
        poses_a.push(pa);
        poses_b.push(pb);
    }

    let opts = ResidualOpts { dead_zone: 32.0, ..ResidualOpts::default() };
    let bytes = |r: &plate::PlateEncodeReportV2| -> usize {
        r.plate_bytes
            + r.motion_bytes
            + r.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).sum::<usize>()
    };

    // Branch 1: GOP B self-coded with its own 2D plate — what the codec
    // does today.
    let mb = plate::estimate_motions(&gb);
    let pb = plate::build_plate(&gb, &mb);
    let self_rep = plate::encode_gop_v3(&gb, &mb, &pb, &opts);
    println!(
        "B self-coded (2D plate):        {:>9} B @ {:.3} dB   (plate {} B)",
        bytes(&self_rep),
        psnr(&gb, &self_rep.recon),
        self_rep.plate_bytes
    );

    // Branch 2: GOP B citing the ENGINE CANONICAL — the exact mesh, at
    // GOP B's exact poses. The canonical is charged as a 24-byte
    // reference because a library already holds it (that is the whole
    // amortisation claim); the pose stream is charged at 12 B/frame.
    let mut pred_b = |fi: usize| -> plate::Yuv420 {
        match render_mesh(&mesh, None, &poses_b[fi], w, h) {
            Some(rgb) => rgb_to_yuv(&rgb, w, h),
            None => plate::Yuv420::new(w, h),
        }
    };
    let (cite_rep, _) =
        plate::encode_gop_v3_core(&gb, &mut pred_b, 24, gb.len() * 12, &opts, None);
    println!(
        "B citing engine canonical:      {:>9} B @ {:.3} dB   (canonical = 24 B ref)",
        bytes(&cite_rep),
        psnr(&gb, &cite_rep.recon)
    );

    // Branch 3: control — the canonical's FULL cost paid by this GOP
    // alone, i.e. no amortisation. The gap between 2 and 3 is exactly
    // what a library buys.
    let mp = fao_codec::mesh_pack::MeshPackOpts { pos_bits: 10, keep_colors: true };
    let (wire, _dec) = fao_codec::mesh_pack::encode(&mesh, &mp);
    println!(
        "  same, canonical NOT amortised:{:>9} B            (mesh_pack = {} B)",
        bytes(&cite_rep) - 24 + wire.len(),
        wire.len()
    );

    // Emit the scene file so the same canonical can drive a real encode
    // through plate_lab --use-canonical3d, i.e. so the seam is exercised
    // end-to-end rather than only in-process.
    if let Some(out) = args.get(1) {
        let scene = fao_codec::engine_scene::EngineScene {
            width: w,
            height: h,
            mesh: mesh.clone(),
            poses: poses_b.clone(),
        };
        let wire = fao_codec::engine_scene::write(&scene, 10);
        std::fs::write(out, &wire).expect("write scene");
        println!("  wrote {} ({} B) — plate_lab --use-canonical3d", out, wire.len());
    }

    // Branch 4: how much of branch 2's win is just "a better predictor"
    // rather than reuse? Cite A's canonical rendered at A's poses while
    // coding B — a deliberately WRONG canonical usage, the control that
    // catches a provider that helps for the wrong reason.
    let mut pred_wrong = |fi: usize| -> plate::Yuv420 {
        match render_mesh(&mesh, None, &poses_a[fi], w, h) {
            Some(rgb) => rgb_to_yuv(&rgb, w, h),
            None => plate::Yuv420::new(w, h),
        }
    };
    let (wrong_rep, _) =
        plate::encode_gop_v3_core(&gb, &mut pred_wrong, 24, gb.len() * 12, &opts, None);
    println!(
        "  control, canonical at WRONG poses:{:>6} B @ {:.3} dB",
        bytes(&wrong_rep),
        psnr(&gb, &wrong_rep.recon)
    );
}
