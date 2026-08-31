//! fao-engine-capture — footage and world model from a REAL in-repo game.
//!
//! Every engine-derived measurement so far used a scene I authored for
//! the experiment (`engine_lab`'s arena, `ground_lab`'s noise plane).
//! Both showed the thesis holds given a correct canonical, and both
//! carry the same objection: I built the world, so I built it to be
//! modellable. This removes that objection by taking the world from a
//! game nobody wrote for this codec.
//!
//! `games/soldier` (early-fps) is the one game in the fleet that runs
//! HEADLESS — software-rendered, painter's algorithm, no native deps,
//! `render()` returns an `egui::Mesh` of 2D triangles. So the pipeline
//! is: generate a level with the game's own procgen, walk a camera
//! through it, render with the game's own renderer, rasterise the
//! resulting triangles here, and hand the codec (a) those frames as
//! footage and (b) the game's own `Mesh3` list plus exact camera as the
//! canonical.
//!
//! This crate exists so that neither side depends on the other. A game
//! should not link a video codec to be recordable; the codec should not
//! track a game's types.
//!
//! Honest scope: the rasteriser below is a flat-shaded barycentric fill
//! over the game's depth-sorted output. It does not sample the texture
//! atlas, so the captured picture is the game's geometry and lighting
//! but not its texture detail. That makes this a WEAKER test than real
//! capture in one specific way — less high-frequency content — and the
//! direction of that bias is worth stating: less texture detail makes
//! prev-MC look better and the canonical look less necessary, so any
//! win measured here is a lower bound.

use early_fps::camera::FpsCam;
use early_fps::level::{Level, MapConfig};
use early_fps::math::{Vec2, Vec3};
use early_fps::mesh::Light;
use early_fps::render::Fog;
use fao_codec::residual::ResidualOpts;
use fao_codec::v11_plate as plate;

const W: usize = 640;
const H: usize = 384;

/// Rasterise the game's depth-sorted 2D output. Painter's order is
/// already correct, so no depth buffer — drawing in list order
/// reproduces what the game would show.
fn rasterise(mesh: &egui::Mesh) -> Vec<u8> {
    let mut rgb = vec![24u8; W * H * 3];
    for tri in mesh.indices.chunks_exact(3) {
        let v: Vec<&egui::epaint::Vertex> = tri.iter().map(|&i| &mesh.vertices[i as usize]).collect();
        let (p0, p1, p2) = (v[0].pos, v[1].pos, v[2].pos);
        let minx = p0.x.min(p1.x).min(p2.x).floor().max(0.0) as usize;
        let maxx = (p0.x.max(p1.x).max(p2.x).ceil().min(W as f32 - 1.0)).max(0.0) as usize;
        let miny = p0.y.min(p1.y).min(p2.y).floor().max(0.0) as usize;
        let maxy = (p0.y.max(p1.y).max(p2.y).ceil().min(H as f32 - 1.0)).max(0.0) as usize;
        let area = (p1.x - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (p1.y - p0.y);
        if area.abs() < 1e-6 {
            continue;
        }
        for y in miny..=maxy {
            for x in minx..=maxx {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let w0 = ((p1.x - p0.x) * (fy - p0.y) - (fx - p0.x) * (p1.y - p0.y)) / area;
                let w1 = ((fx - p0.x) * (p2.y - p0.y) - (p2.x - p0.x) * (fy - p0.y)) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let c = [
                    w2 * v[0].color.r() as f32 + w1 * v[1].color.r() as f32 + w0 * v[2].color.r() as f32,
                    w2 * v[0].color.g() as f32 + w1 * v[1].color.g() as f32 + w0 * v[2].color.g() as f32,
                    w2 * v[0].color.b() as f32 + w1 * v[1].color.b() as f32 + w0 * v[2].color.b() as f32,
                ];
                let i = (y * W + x) * 3;
                for k in 0..3 {
                    rgb[i + k] = c[k].clamp(0.0, 255.0) as u8;
                }
            }
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
            let (r, g, b) = (rgb[i] as f32, rgb[i + 1] as f32, rgb[i + 2] as f32);
            f.y[y * W + x] = (0.299 * r + 0.587 * g + 0.114 * b).round().clamp(0.0, 255.0) as u8;
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

    let cfg = MapConfig { seed: 0xC0FFEE, ..Default::default() };
    let level = Level::generate(cfg);
    eprintln!(
        "level: {}x{} cells, {} meshes, {} rooms",
        level.w,
        level.h,
        level.meshes.len(),
        level.rooms.len()
    );

    // A camera walk: forward through the level with a slow pan. Taken
    // from the game's own FpsCam so the projection the codec cites is
    // the projection the game rendered.
    let start = level
        .rooms
        .first()
        // Room::center is already world-space, so no tile-size scaling —
        // guessing otherwise put the camera outside the level entirely.
        .map(|r| Vec3::new(r.center.x, r.floor_y + 1.6, r.center.z))
        .unwrap_or(Vec3::new(0.0, 1.6, 0.0));
    let cams: Vec<FpsCam> = (0..n)
        .map(|i| {
            let t = i as f32;
            FpsCam {
                pos: Vec3::new(start.x + t * 0.22, start.y, start.z + t * 0.30),
                yaw: t * 0.010,
                pitch: 0.02,
            }
        })
        .collect();

    let light = Light::sun(Vec3::new(-0.4, -1.0, -0.3), egui::Color32::from_rgb(255, 244, 224));
    // Fog range taken from the game's own map_preview defaults rather
    // than invented, so the captured picture is one the game would
    // actually show.
    let fog = Fog::new(egui::Color32::from_rgb(18, 21, 27), 55.0, 280.0);
    let atlas = egui::TextureId::default();
    let scale = W as f32 * 0.9;

    // ACTORS. A canonical is the MAP; the things moving through it are
    // per-take and no library can hold them. That is not a limitation
    // invented for this test — it is how the integration would really
    // work, and it is the same split the mover layer makes in 2D.
    // Without it the canonical render IS the footage and the experiment
    // reports 99 dB, which measures only that the game is deterministic
    // (this bin did exactly that on its first run, as engine_lab and
    // ground_lab did before it).
    let actors_at = |fi: usize| -> Vec<early_fps::mesh::Mesh3> {
        let t = fi as f32;
        (0..5)
            .map(|k| {
                let mut m = early_fps::mesh::Mesh3::new(egui::Color32::from_rgb(210, 90, 60));
                let phase = k as f32 * 1.7;
                let c = Vec3::new(
                    start.x + 6.0 * (t * 0.06 + phase).cos() + k as f32 * 2.0,
                    start.y - 0.6,
                    start.z + 9.0 + 6.0 * (t * 0.05 + phase).sin(),
                );
                let r = 0.6;
                // A box, built with the game's own mesh primitive so the
                // actor goes through the same renderer as the map.
                let corners = [
                    Vec3::new(c.x - r, c.y - r, c.z - r),
                    Vec3::new(c.x + r, c.y - r, c.z - r),
                    Vec3::new(c.x + r, c.y + r, c.z - r),
                    Vec3::new(c.x - r, c.y + r, c.z - r),
                    Vec3::new(c.x - r, c.y - r, c.z + r),
                    Vec3::new(c.x + r, c.y - r, c.z + r),
                    Vec3::new(c.x + r, c.y + r, c.z + r),
                    Vec3::new(c.x - r, c.y + r, c.z + r),
                ];
                let faces = [
                    ([0, 1, 2, 3], Vec3::new(0.0, 0.0, -1.0)),
                    ([5, 4, 7, 6], Vec3::new(0.0, 0.0, 1.0)),
                    ([4, 0, 3, 7], Vec3::new(-1.0, 0.0, 0.0)),
                    ([1, 5, 6, 2], Vec3::new(1.0, 0.0, 0.0)),
                    ([3, 2, 6, 7], Vec3::new(0.0, 1.0, 0.0)),
                    ([4, 5, 1, 0], Vec3::new(0.0, -1.0, 0.0)),
                ];
                for (idx, nrm) in faces {
                    m.push_quad(
                        corners[idx[0]], corners[idx[1]], corners[idx[2]], corners[idx[3]],
                        nrm,
                        egui::Color32::from_rgb(210, 90, 60),
                    );
                }
                m.compute_bounds();
                m
            })
            .collect()
    };

    let render_at = |cam: &FpsCam| -> Vec<u8> {
        let m = early_fps::render::render(
            &level.meshes,
            cam,
            W as f32 / 2.0,
            H as f32 / 2.0,
            scale,
            &light,
            &fog,
            W as f32,
            H as f32,
            0.0,
            Vec2::new(0.0, 0.0),
            atlas,
            1,
            &[],
        );
        rasterise(&m)
    };

    let render_dressed = |fi: usize| -> Vec<u8> {
        let mut all: Vec<early_fps::mesh::Mesh3> = level.meshes.clone();
        all.extend(actors_at(fi));
        let m = early_fps::render::render(
            &all,
            &cams[fi],
            W as f32 / 2.0,
            H as f32 / 2.0,
            scale,
            &light,
            &fog,
            W as f32,
            H as f32,
            0.0,
            Vec2::new(0.0, 0.0),
            atlas,
            1,
            &[],
        );
        let mut rgb = rasterise(&m);
        // Exposure drift: the post-processing and encoder wobble a
        // static canonical cannot know about.
        let gain = 1.0 + 0.05 * ((fi as f32) * 0.23).sin();
        for v in rgb.iter_mut() {
            *v = ((*v as f32) * gain).round().clamp(0.0, 255.0) as u8;
        }
        rgb
    };
    let frames: Vec<plate::Yuv420> = (0..n).map(|i| to_yuv(&render_dressed(i))).collect();
    let nonflat = frames[0].y.iter().filter(|&&v| v != frames[0].y[0]).count();
    eprintln!("captured {n} frames, {nonflat} non-uniform px in frame 0");

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

    let m = plate::estimate_motions(&frames);
    let pl = plate::build_plate(&frames, &m);
    let r2d = plate::encode_gop_v3(&frames, &m, &pl, &opts);
    println!(
        "v11 2D plate:              {:>9} B @ {:.3} dB  (plate {} B)",
        bytes(&r2d),
        psnr(&frames, &r2d.recon),
        r2d.plate_bytes
    );

    let rnp = plate::encode_gop_no_plate(&frames, &opts);
    println!(
        "v11 no model:              {:>9} B @ {:.3} dB",
        bytes(&rnp),
        psnr(&frames, &rnp.recon)
    );

    // The engine-derived canonical: the game's OWN meshes rendered by
    // the game's OWN renderer at the exact camera. Charged as a 24-byte
    // library reference plus 24 B/frame of camera.
    let mut pred = |fi: usize| to_yuv(&render_at(&cams[fi]));
    let (rcanon, _) = plate::encode_gop_v3_core(&frames, &mut pred, 24, n * 24, &opts, None);
    println!(
        "v11 ENGINE canonical:      {:>9} B @ {:.3} dB  (24 B ref + 24 B/frame cam)",
        bytes(&rcanon),
        psnr(&frames, &rcanon.recon)
    );

    let mut wrong = |fi: usize| {
        let mut c = cams[fi];
        c.pos.x += 40.0;
        c.yaw += 0.8;
        to_yuv(&render_at(&c))
    };
    let (rw, _) = plate::encode_gop_v3_core(&frames, &mut wrong, 24, n * 24, &opts, None);
    println!(
        "  control, WRONG camera:   {:>9} B @ {:.3} dB",
        bytes(&rw),
        psnr(&frames, &rw.recon)
    );
}
