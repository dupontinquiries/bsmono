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

/// One actor's pose for one frame — the per-take state a decoder
/// would re-render from. Position only: the boxes are axis-aligned and
/// never rotate, so a rotation channel would be padding here. A real
/// engine emits (mesh id, position, quaternion); this harness charges
/// what it actually sends.
#[derive(Clone, Copy)]
struct ActorPose {
    c: Vec3,
}

/// Wire cost of carrying the actors as ENTITIES rather than pixels:
/// quantised positions, delta-coded frame to frame, zstd'd. Computed
/// from the bytes that would go on the wire, not assumed. Quantisation
/// step 1/256 world units — the render is at 640 px over a ~10 unit
/// view, so that is well below a pixel.
fn entity_stream_bytes(poses: &[Vec<ActorPose>]) -> usize {
    let q = |v: f32| (v * 256.0).round() as i32;
    let mut raw: Vec<u8> = Vec::new();
    let mut prev: Vec<[i32; 3]> = Vec::new();
    for (fi, frame) in poses.iter().enumerate() {
        for (k, a) in frame.iter().enumerate() {
            let cur = [q(a.c.x), q(a.c.y), q(a.c.z)];
            let base = if fi == 0 { [0, 0, 0] } else { prev[k] };
            for d in 0..3 {
                let delta = cur[d] - base[d];
                // zigzag varint, the same as any protobuf pose stream
                let mut z = ((delta << 1) ^ (delta >> 31)) as u32;
                loop {
                    let b = (z & 0x7f) as u8;
                    z >>= 7;
                    if z == 0 {
                        raw.push(b);
                        break;
                    }
                    raw.push(b | 0x80);
                }
            }
            if fi == 0 {
                prev.push(cur);
            } else {
                prev[k] = cur;
            }
        }
    }
    zstd::encode_all(raw.as_slice(), 19).map(|c| c.len()).unwrap_or(raw.len())
}

/// Camera stream charged the same way — the previous session assumed
/// 24 B/frame flat; a real pose stream is delta-coded.
fn camera_stream_bytes(cams: &[FpsCam]) -> usize {
    let q = |v: f32| (v * 256.0).round() as i32;
    let qa = |v: f32| (v * 8192.0).round() as i32; // ~0.1 mrad
    let mut raw: Vec<u8> = Vec::new();
    let mut prev = [0i32; 5];
    for (fi, c) in cams.iter().enumerate() {
        let cur = [q(c.pos.x), q(c.pos.y), q(c.pos.z), qa(c.yaw), qa(c.pitch)];
        for d in 0..5 {
            let delta = if fi == 0 { cur[d] } else { cur[d] - prev[d] };
            let mut z = ((delta << 1) ^ (delta >> 31)) as u32;
            loop {
                let b = (z & 0x7f) as u8;
                z >>= 7;
                if z == 0 {
                    raw.push(b);
                    break;
                }
                raw.push(b | 0x80);
            }
        }
        prev = cur;
    }
    zstd::encode_all(raw.as_slice(), 19).map(|c| c.len()).unwrap_or(raw.len())
}

fn has(args: &[String], flag: &str) -> bool {
    args.iter().any(|a| a == flag)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let n: usize = positional.first().and_then(|s| s.parse().ok()).unwrap_or(48);
    // Footage composition switches, so the byte budget can be
    // decomposed: what do the actors cost, what does the exposure
    // drift cost, what does the map cost when the model is exact.
    let with_actors = !has(&args, "--no-actors");
    let with_exposure = !has(&args, "--no-exposure");
    let dz: f32 = args
        .iter()
        .position(|a| a == "--dz")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(32.0);

    let cfg = MapConfig { seed: 0xC0FFEE, ..Default::default() };
    let level = Level::generate(cfg);
    eprintln!(
        "level: {}x{} cells, {} meshes, {} rooms | actors {} exposure {} dz {}",
        level.w,
        level.h,
        level.meshes.len(),
        level.rooms.len(),
        with_actors,
        with_exposure,
        dz
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
    // per-take. What a library CAN hold is the actor's MESH — a box
    // here, a character model in a real game — and what stays per-take
    // is its trajectory. So the split this file measures is two-level:
    //   * pixels:   actors are footage the map canonical cannot predict
    //   * entities: actors are (mesh, pose-per-frame) the decoder
    //               re-renders, charged at the cost of the pose stream
    // Without any of this the canonical render IS the footage and the
    // experiment reports 99 dB, which measures only that the game is
    // deterministic (this bin did exactly that on its first run).
    let actor_poses: Vec<Vec<ActorPose>> = (0..n)
        .map(|fi| {
            let t = fi as f32;
            (0..5)
                .map(|k| {
                    let phase = k as f32 * 1.7;
                    ActorPose {
                        c: Vec3::new(
                            start.x + 6.0 * (t * 0.06 + phase).cos() + k as f32 * 2.0,
                            start.y - 0.6,
                            start.z + 9.0 + 6.0 * (t * 0.05 + phase).sin(),
                        ),
                    }
                })
                .collect()
        })
        .collect();
    let actor_mesh = |a: &ActorPose| -> early_fps::mesh::Mesh3 {
        let mut m = early_fps::mesh::Mesh3::new(egui::Color32::from_rgb(210, 90, 60));
        let c = a.c;
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
    };

    // One renderer for everything: map meshes plus whatever actors the
    // caller passes, at the given camera, through the game's own
    // render(). The footage and every canonical go through this same
    // function, so a difference between them is a difference in WHAT
    // was known, never in how it was drawn.
    let render_scene = |cam: &FpsCam, actors: &[ActorPose]| -> Vec<u8> {
        let mut all: Vec<early_fps::mesh::Mesh3> = level.meshes.clone();
        all.extend(actors.iter().map(actor_mesh));
        let m = early_fps::render::render(
            &all,
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

    let render_footage = |fi: usize| -> Vec<u8> {
        let actors: &[ActorPose] = if with_actors { &actor_poses[fi] } else { &[] };
        let mut rgb = render_scene(&cams[fi], actors);
        if with_exposure {
            // Exposure drift: the post-processing and encoder wobble a
            // static canonical cannot know about.
            let gain = 1.0 + 0.05 * ((fi as f32) * 0.23).sin();
            for v in rgb.iter_mut() {
                *v = ((*v as f32) * gain).round().clamp(0.0, 255.0) as u8;
            }
        }
        rgb
    };
    // ── --vary <stage>: footage where EXACTLY ONE image-formation
    // stage changes over the GOP, camera held still, no actors, so the
    // (base render, frame) difference is that stage and nothing else.
    // Ground truth for the photometric marker bank: run the bank on
    // every frame, and it must name the stage that was varied.
    //   exposure  global gain 1 +- 0.08 sin
    //   wb        per-channel gain (R up, B down)
    //   vignette  radial falloff 1 - k r^2, k 0..0.35
    //   fog       fog end 280 -> 120 (denser), same colour
    //   light     sun direction swings 40 degrees
    //   none      identical render (bank must report nothing)
    if let Some(stage) = args.iter().position(|a| a == "--vary").and_then(|i| args.get(i + 1)) {
        let cam0 = cams[0];
        let base = to_yuv(&render_scene(&cam0, &[]));
        println!("vary={stage}  (camera static, no actors; bank vs base render)");
        for fi in 0..n {
            let t = fi as f32 / (n.max(2) - 1) as f32;
            let s = (t * std::f32::consts::PI).sin();
            let mut rgb: Vec<u8> = match stage.as_str() {
                "fog" => {
                    let fg = Fog::new(fog.color, 55.0 - 50.0 * s, 280.0 - 240.0 * s);
                    let mut all: Vec<early_fps::mesh::Mesh3> = level.meshes.clone();
                    all.truncate(all.len());
                    let m = early_fps::render::render(
                        &all, &cam0, W as f32 / 2.0, H as f32 / 2.0, scale, &light, &fg,
                        W as f32, H as f32, 0.0, Vec2::new(0.0, 0.0), atlas, 1, &[],
                    );
                    rasterise(&m)
                }
                "light" => {
                    let ang = 0.7 * s;
                    let dir = Vec3::new(-0.4 * ang.cos() - 0.3 * ang.sin(), -1.0, -0.3 * ang.cos() + 0.4 * ang.sin());
                    let lt = Light::sun(dir, egui::Color32::from_rgb(255, 244, 224));
                    let m = early_fps::render::render(
                        &level.meshes, &cam0, W as f32 / 2.0, H as f32 / 2.0, scale, &lt, &fog,
                        W as f32, H as f32, 0.0, Vec2::new(0.0, 0.0), atlas, 1, &[],
                    );
                    rasterise(&m)
                }
                _ => render_scene(&cam0, &[]),
            };
            match stage.as_str() {
                "exposure" => {
                    let g = 1.0 + 0.08 * s;
                    for v in rgb.iter_mut() { *v = ((*v as f32) * g).round().clamp(0.0, 255.0) as u8; }
                }
                "wb" => {
                    let (gr, gb) = (1.0 + 0.10 * s, 1.0 - 0.10 * s);
                    for px in rgb.chunks_exact_mut(3) {
                        px[0] = ((px[0] as f32) * gr).round().clamp(0.0, 255.0) as u8;
                        px[2] = ((px[2] as f32) * gb).round().clamp(0.0, 255.0) as u8;
                    }
                }
                "vignette" => {
                    let k = 0.35 * s;
                    let (cx, cy) = (W as f32 / 2.0, H as f32 / 2.0);
                    let rmax2 = cx * cx + cy * cy;
                    for y in 0..H {
                        for x in 0..W {
                            let r2 = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)) / rmax2;
                            let g = 1.0 - k * r2;
                            let i = (y * W + x) * 3;
                            for c in 0..3 { rgb[i + c] = ((rgb[i + c] as f32) * g).round().clamp(0.0, 255.0) as u8; }
                        }
                    }
                }
                _ => {}
            }
            let f = to_yuv(&rgb);
            let sig = fao_codec::photometric::signature(&base, &f);
            if fi % 4 == 0 || fi == n - 1 {
                println!("  f{fi:>2} s={s:.2} {}", sig.row());
            }
        }
        return;
    }
    let frames: Vec<plate::Yuv420> = (0..n).map(|i| to_yuv(&render_footage(i))).collect();
    let nonflat = frames[0].y.iter().filter(|&&v| v != frames[0].y[0]).count();
    eprintln!("captured {n} frames, {nonflat} non-uniform px in frame 0");

    if let Some(path) = positional.get(1) {
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

    let opts = ResidualOpts { dead_zone: dz, ..ResidualOpts::default() };
    let cam_bytes = camera_stream_bytes(&cams);
    let ent_bytes = if with_actors { entity_stream_bytes(&actor_poses) } else { 0 };
    // Library reference to the map canonical: a 24-byte handle, as
    // before. The actor mesh is one more library asset — its handle is
    // charged too, though the mesh itself lives in the library.
    const LIB_REF: usize = 24;

    let side = |r: &plate::PlateEncodeReportV2| -> usize { r.frames.iter().map(|c| c.mode_bytes).sum() };
    let resid = |r: &plate::PlateEncodeReportV2| -> usize { r.frames.iter().map(|c| c.residual_bytes).sum() };
    let report = |name: &str, r: &plate::PlateEncodeReportV2, extra: usize, note: &str| {
        let total = r.plate_bytes + r.motion_bytes + side(r) + resid(r) + extra;
        println!(
            "{name:<28} {total:>9} B  {:>7.3} dB  MS-SSIM {:.5}  [model {} motion {} side {} resid {}]  {note}",
            psnr(&frames, &r.recon),
            fao_codec::msssim::recon_score(&r.recon, &frames),
            r.plate_bytes + extra,
            r.motion_bytes,
            side(r),
            resid(r),
        );
    };
    let per_frame = std::env::var("FAO_CAP_FRAMES").is_ok();

    if !has(&args, "--skip-2d") {
        let m = plate::estimate_motions(&frames);
        let pl = plate::build_plate(&frames, &m);
        let mut r2d = plate::encode_gop_v3(&frames, &m, &pl, &opts);
        report("v11 2D plate", &r2d, 0, "");
        r2d.recon.clear();

        let rnp = plate::encode_gop_no_plate(&frames, &opts);
        report("v11 no model", &rnp, 0, "");
    }

    // The engine-derived canonical: the game's OWN meshes rendered by
    // the game's OWN renderer at the exact camera. Map only — actors
    // are footage the canonical does not know about.
    let mut pred_map = |fi: usize| to_yuv(&render_scene(&cams[fi], &[]));
    let (rmap, _) = plate::encode_gop_v3_core(&frames, &mut pred_map, LIB_REF, cam_bytes, &opts, None);
    report("ENGINE map canonical", &rmap, 0, &format!("(cam stream {cam_bytes} B)"));
    if per_frame {
        for (i, c) in rmap.frames.iter().enumerate().take(8) {
            eprintln!("  map  f{i:>2}: side {:>5} resid {:>6}", c.mode_bytes, c.residual_bytes);
        }
    }

    // ENTITIES: the decoder also holds the actor mesh and re-renders it
    // from a per-frame pose stream. The only thing left for the
    // residual is what neither knows — here, the exposure drift.
    if with_actors {
        let mut pred_ent = |fi: usize| to_yuv(&render_scene(&cams[fi], &actor_poses[fi]));
        let (rent, _) = plate::encode_gop_v3_core(
            &frames, &mut pred_ent, LIB_REF * 2, cam_bytes + ent_bytes, &opts, None,
        );
        report(
            "ENGINE map+entity canonical",
            &rent,
            0,
            &format!("(cam {cam_bytes} B + entity stream {ent_bytes} B)"),
        );
        if per_frame {
            for (i, c) in rent.frames.iter().enumerate().filter(|(_, c)| c.residual_bytes > 5 || c.mode_bytes > 100) {
                eprintln!("  ent  f{i:>2}: side {:>5} resid {:>6}", c.mode_bytes, c.residual_bytes);
            }
        }
    }

    if !has(&args, "--skip-control") {
        let mut wrong = |fi: usize| {
            let mut c = cams[fi];
            c.pos.x += 40.0;
            c.yaw += 0.8;
            to_yuv(&render_scene(&c, &[]))
        };
        let (rw, _) = plate::encode_gop_v3_core(&frames, &mut wrong, LIB_REF, cam_bytes, &opts, None);
        report("  control, WRONG camera", &rw, 0, "");
    }
}
