// plate_lab — the v11 measurement loop, one shot per run.
//
//   plate_lab <in.y4m> <out_recon.y4m> [--skip-thr N] [--boost N] [--report csv]
//
// Prints one accounting block: plate bytes (the web, amortizable),
// motion bytes, per-frame residual bytes (the marginal cost that has to
// beat AV1's post-keyframe marginal cost for the thesis to hold), and
// writes the decoder-side reconstruction as y4m so ffmpeg's psnr/ssim
// filters can score it EXACTLY the way the AV1/x264 baselines are scored
// — same tool, same numbers, no self-graded quality.

use fao_codec::residual::ResidualOpts;
use fao_codec::v11_plate as plate;
use std::fs;
use std::io::{BufWriter, Read, Write};

fn die(msg: &str) -> ! {
    eprintln!("plate_lab: {msg}");
    std::process::exit(2)
}

/// Minimal y4m reader: enough for ffmpeg-written 4:2:0 output.
fn read_y4m(path: &str) -> (Vec<plate::Yuv420>, String) {
    let mut data = Vec::new();
    fs::File::open(path)
        .unwrap_or_else(|e| die(&format!("open {path}: {e}")))
        .read_to_end(&mut data)
        .unwrap_or_else(|e| die(&format!("read {path}: {e}")));
    let hdr_end = data.iter().position(|&b| b == b'\n').unwrap_or_else(|| die("no y4m header"));
    let header = String::from_utf8_lossy(&data[..hdr_end]).to_string();
    if !header.starts_with("YUV4MPEG2") {
        die("not a y4m file");
    }
    let mut w = 0usize;
    let mut h = 0usize;
    for tok in header.split_whitespace().skip(1) {
        match tok.as_bytes()[0] {
            b'W' => w = tok[1..].parse().unwrap_or(0),
            b'H' => h = tok[1..].parse().unwrap_or(0),
            b'C' => {
                if !tok[1..].starts_with("420") {
                    die(&format!("only 4:2:0 supported, got {tok}"));
                }
            }
            _ => {}
        }
    }
    if w == 0 || h == 0 {
        die("no W/H in header");
    }
    let fsz = w * h + 2 * (w / 2) * (h / 2);
    let mut frames = Vec::new();
    let mut off = hdr_end + 1;
    while off < data.len() {
        let fe = data[off..]
            .iter()
            .position(|&b| b == b'\n')
            .unwrap_or_else(|| die("truncated FRAME header"))
            + off;
        if !data[off..].starts_with(b"FRAME") {
            die("expected FRAME marker");
        }
        off = fe + 1;
        if off + fsz > data.len() {
            die("truncated frame payload");
        }
        let mut f = plate::Yuv420::new(w, h);
        f.y.copy_from_slice(&data[off..off + w * h]);
        let co = off + w * h;
        let cs = (w / 2) * (h / 2);
        f.u.copy_from_slice(&data[co..co + cs]);
        f.v.copy_from_slice(&data[co + cs..co + 2 * cs]);
        frames.push(f);
        off += fsz;
    }
    (frames, header)
}

fn write_y4m(path: &str, header: &str, frames: &[plate::Yuv420]) {
    let f = fs::File::create(path).unwrap_or_else(|e| die(&format!("create {path}: {e}")));
    let mut w = BufWriter::new(f);
    writeln!(w, "{header}").unwrap();
    for fr in frames {
        w.write_all(b"FRAME\n").unwrap();
        w.write_all(&fr.y).unwrap();
        w.write_all(&fr.u).unwrap();
        w.write_all(&fr.v).unwrap();
    }
}

fn arg_val(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        die("usage: plate_lab <in.y4m> <out_recon.y4m> [--skip-thr N] [--boost N]");
    }
    let (frames, header) = read_y4m(&args[0]);
    if frames.is_empty() {
        die("no frames");
    }
    eprintln!("frames: {}  {}x{}", frames.len(), frames[0].w, frames[0].h);

    let t0 = std::time::Instant::now();
    let motions = if args.iter().any(|a| a == "--static") {
        vec![plate::PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0 }; frames.len()]
    } else {
        plate::estimate_motions(&frames)
    };
    let max_shift = motions.iter().map(|m| m.dx.abs().max(m.dy.abs())).fold(0.0f32, f32::max);
    let min_conf = motions.iter().skip(1).map(|m| m.confidence).fold(1.0f32, f32::min);
    eprintln!(
        "motion: max |shift| {max_shift:.2}px, min confidence {min_conf:.2}  ({:.1}s)",
        t0.elapsed().as_secs_f32()
    );

    let t1 = std::time::Instant::now();
    let pl = plate::build_plate(&frames, &motions);
    eprintln!("plate: {}x{} origin ({:.1},{:.1})  ({:.1}s)", pl.w, pl.h, pl.ox, pl.oy, t1.elapsed().as_secs_f32());

    let mut opts = ResidualOpts::default();
    if let Some(v) = arg_val(&args, "--skip-thr") {
        opts.skip_threshold = v.parse().unwrap_or(opts.skip_threshold);
    }
    if let Some(v) = arg_val(&args, "--boost") {
        opts.saliency_boost = v.parse().unwrap_or(opts.saliency_boost);
    }
    if let Some(v) = arg_val(&args, "--dz") {
        opts.dead_zone = v.parse().unwrap_or(opts.dead_zone);
    }
    eprintln!(
        "residual opts: skip_thr {} boost {} dz {} (rle {} dcpred {} deint {})",
        opts.skip_threshold, opts.saliency_boost, opts.dead_zone, opts.skip_block_rle, opts.dc_prediction, opts.channel_deinterleave
    );

    if let Some(degpath) = arg_val(&args, "--restore-exp") {
        // Learned-restoration experiment. args[0] = clean y4m, --restore-exp
        // <degraded y4m>. Train buckets on the FIRST half of the pairs,
        // evaluate on the held-out second half: luma PSNR before/after
        // restoration, plus FAST keypoints+coverage on degraded vs restored
        // (does restoration un-starve detection?).
        let (deg_frames, _) = read_y4m(&degpath);
        if deg_frames.len() != frames.len() {
            die("clean/degraded frame count mismatch");
        }
        let (w, h) = (frames[0].w, frames[0].h);
        let half = frames.len() / 2;
        let pairs: Vec<(&[u8], &[u8])> = deg_frames[..half]
            .iter()
            .zip(&frames[..half])
            .map(|(d, c)| (d.y.as_slice(), c.y.as_slice()))
            .collect();
        let t = std::time::Instant::now();
        let model = fao_codec::restore::train(&pairs, w, h, 3);
        eprintln!("train: {:.1}s on {} pairs (stride 3)", t.elapsed().as_secs_f32(), half);
        let psnr = |a: &[u8], b: &[u8]| -> f64 {
            let mut mse = 0.0f64;
            for i in 0..a.len() {
                let d = a[i] as f64 - b[i] as f64;
                mse += d * d;
            }
            mse /= a.len() as f64;
            10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
        };
        let (mut p_deg, mut p_res) = (0.0f64, 0.0f64);
        use fao_codec::v8_reconstruction_codec::FastDetector;
        let det = FastDetector { max_keypoints: 5000, ..FastDetector::default() };
        let coverage = |y: &[u8]| -> (usize, f32) {
            let kps = det.detect(y, w, h);
            let (cw, ch) = ((w + 63) / 64, (h + 63) / 64);
            let mut cells = vec![false; cw * ch];
            for k in &kps {
                cells[(k.y as usize / 64) * cw + (k.x as usize / 64)] = true;
            }
            (kps.len(), cells.iter().filter(|&&c| c).count() as f32 / cells.len() as f32)
        };
        let mut det_stats = (0usize, 0.0f32, 0usize, 0.0f32);
        for (i, (d, c)) in deg_frames[half..].iter().zip(&frames[half..]).enumerate() {
            let restored = fao_codec::restore::apply(&model, &d.y, w, h);
            p_deg += psnr(&d.y, &c.y);
            p_res += psnr(&restored, &c.y);
            if i == 0 {
                let (n0, c0) = coverage(&d.y);
                let (n1, c1) = coverage(&restored);
                det_stats = (n0, c0, n1, c1);
            }
        }
        let n = (frames.len() - half) as f64;
        println!("heldout_luma_psnr degraded {:.2} restored {:.2} (delta {:+.2} dB)", p_deg / n, p_res / n, (p_res - p_deg) / n);
        println!(
            "detection first-heldout: degraded n {} cov {:.2} | restored n {} cov {:.2}",
            det_stats.0, det_stats.1, det_stats.2, det_stats.3
        );
        return;
    }
    if args.iter().any(|a| a == "--triangulate") {
        // First whole-scene geometry attempt with the unblocked inputs:
        // track (spatially reseeded, pyramid, blur-tolerant) → classify
        // (HUD excluded) → observations for Static tracks → v8's actual
        // TriangulatePass over the translation-only pose track built the
        // way v8's encode chain builds it (cumulative negated phase-corr
        // motion, identity rotation, focal ≈ frame dims).
        use fao_codec::v8_reconstruction_codec::{
            FeatureTrackPass, MotionConsensusPass, MotionClass, CameraPose, FeatureObservation,
            TriangulatePass,
        };
        use fao_codec::geometry::Vec3;
        use fao_codec::phase_correlation::PhaseCorrelationPass;
        let (w, h) = (frames[0].w, frames[0].h);
        let mut pass = FeatureTrackPass::default();
        let consensus = MotionConsensusPass::default();
        let pc = PhaseCorrelationPass::new(512.min(w.min(h)));
        let mut prev: Option<&Vec<u8>> = None;
        let mut poses: Vec<CameraPose> = Vec::new();
        let mut cum = Vec3::zero();
        let mut obs: std::collections::HashMap<u32, Vec<FeatureObservation>> =
            std::collections::HashMap::new();
        let mut classes: std::collections::HashMap<u32, MotionClass> = std::collections::HashMap::new();
        for (i, f) in frames.iter().enumerate() {
            if let Some(pv) = prev {
                let cam = pc.estimate(&f.y, pv, w, h);
                if cam.confidence > 0.15 {
                    cum.x += -cam.dx;
                    cum.y += -cam.dy;
                }
                let feats = pass.process(&f.y, Some(pv.as_slice()), w, h, i as u32);
                consensus.classify(&mut pass.live_tracks, cam.dx, cam.dy, cam.confidence);
                for t in &pass.live_tracks {
                    classes.insert(t.feature_id, t.motion_class);
                }
                for sf in &feats {
                    obs.entry(sf.feature_id).or_default().push(FeatureObservation {
                        frame_idx: i as u32,
                        screen_x: sf.screen_x,
                        screen_y: sf.screen_y,
                    });
                }
            } else {
                let feats = pass.process(&f.y, None, w, h, i as u32);
                for sf in &feats {
                    obs.entry(sf.feature_id).or_default().push(FeatureObservation {
                        frame_idx: 0,
                        screen_x: sf.screen_x,
                        screen_y: sf.screen_y,
                    });
                }
            }
            poses.push(CameraPose {
                position: cum,
                rotation: [1.0, 0.0, 0.0, 0.0],
                focal: [w as f32, h as f32],
                principal: [w as f32 / 2.0, h as f32 / 2.0],
                distortion: 0.0,
            });
            prev = Some(&f.y);
        }
        // Static world tracks only, ≥4 observations.
        let mut world: Vec<(u32, Vec<FeatureObservation>)> = obs
            .into_iter()
            .filter(|(id, v)| v.len() >= 4 && matches!(classes.get(id), Some(MotionClass::Static)))
            .collect();
        // Global-affine analysis + optional rectification (--affine):
        // fit per-consecutive-pair affines from the static observations,
        // report how much motion translation-only leaves unexplained vs
        // the affine (the "does rotation/zoom matter here" metric), then
        // rectify every observation by the accumulated LINEAR part —
        // removing roll/zoom/shear (inexpressible in the pose model)
        // while preserving pan + parallax (what triangulation eats).
        {
            use fao_codec::global_motion as gm;
            // observations per frame: feature_id -> (x, y)
            let nfr = frames.len();
            let mut per_frame: Vec<std::collections::HashMap<u32, (f32, f32)>> =
                vec![std::collections::HashMap::new(); nfr];
            for (id, v) in &world {
                for o in v {
                    per_frame[o.frame_idx as usize].insert(*id, (o.screen_x, o.screen_y));
                }
            }
            let (mut rms_t_acc, mut rms_a_acc, mut np) = (0.0f64, 0.0f64, 0usize);
            let mut max_rot = 0.0f32;
            let mut cum: Vec<gm::Affine> = vec![gm::IDENTITY; nfr];
            for t in 1..nfr {
                let mut pairs: Vec<(f32, f32, f32, f32)> = Vec::new();
                for (id, &(x1, y1)) in &per_frame[t] {
                    if let Some(&(x0, y0)) = per_frame[t - 1].get(id) {
                        pairs.push((x0, y0, x1, y1));
                    }
                }
                let a = gm::fit(&pairs).unwrap_or(gm::IDENTITY);
                // translation-only comparator: median displacement.
                let tmodel = {
                    let mut dx: Vec<f32> = pairs.iter().map(|p| p.2 - p.0).collect();
                    let mut dy: Vec<f32> = pairs.iter().map(|p| p.3 - p.1).collect();
                    dx.sort_by(|u, v| u.partial_cmp(v).unwrap());
                    dy.sort_by(|u, v| u.partial_cmp(v).unwrap());
                    let (mx, my) = if dx.is_empty() { (0.0, 0.0) } else { (dx[dx.len() / 2], dy[dy.len() / 2]) };
                    [1.0, 0.0, mx, 0.0, 1.0, my]
                };
                if pairs.len() >= 6 {
                    rms_t_acc += gm::rms(&pairs, &tmodel) as f64;
                    rms_a_acc += gm::rms(&pairs, &a) as f64;
                    np += 1;
                    let (rot, _) = gm::describe(&a);
                    if rot.abs() > max_rot.abs() {
                        max_rot = rot;
                    }
                }
                cum[t] = gm::compose(&a, &cum[t - 1]);
            }
            if np > 0 {
                println!(
                    "pairwise residual RMS: translation {:.2}px affine {:.2}px | max per-pair rot {:.3}rad",
                    rms_t_acc / np as f64,
                    rms_a_acc / np as f64,
                    max_rot
                );
            }
            if args.iter().any(|a| a == "--rot") {
                // Full rotation rectification: per-pair homography from
                // static tracks → R = nearest rotation to K⁻¹HK →
                // accumulate → unproject/rotate/reproject every
                // observation into frame-0's orientation. Translation is
                // then RE-estimated from the rectified tracks (the raw
                // phase-corr translation contains rotation-induced shift
                // and would be inconsistent with rectified observations).
                use fao_codec::two_view as tv;
                let (fx, fy) = (w as f32, w as f32); // square pixels; v8's
                // focal=[w,h] convention is anisotropic and unphysical for
                // rotation math.
                let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
                let mut r_cum: Vec<tv::Mat3> = vec![tv::M3_IDENTITY; nfr];
                let mut total_angle = 0.0f32;
                for t in 1..nfr {
                    let mut prs: Vec<(f32, f32, f32, f32)> = Vec::new();
                    for (id, &(x1, y1)) in &per_frame[t] {
                        if let Some(&(x0, y0)) = per_frame[t - 1].get(id) {
                            prs.push((x0, y0, x1, y1));
                        }
                    }
                    // Per-pair MODEL SELECTION. A rotation-only homography
                    // fitted to translation-dominant (parallax) flow eats
                    // the parallax as phantom rotation — measured: 14.4°
                    // of fictitious rotation on a handheld GOP, degrading
                    // its triangulation. Apply the rotation only when the
                    // homography out-explains the pure-translation model
                    // by a clear margin; otherwise identity. Rotating
                    // content passes (H captures position-dependent shift
                    // translation cannot), parallax content fails the
                    // margin (its variance is depth structure both models
                    // leave behind).
                    let r = match tv::fit_homography(&prs) {
                        Some(hm) => {
                            let t_rms = {
                                let mut dx: Vec<f32> = prs.iter().map(|p| p.2 - p.0).collect();
                                let mut dy: Vec<f32> = prs.iter().map(|p| p.3 - p.1).collect();
                                dx.sort_by(|a, b| a.partial_cmp(b).unwrap());
                                dy.sort_by(|a, b| a.partial_cmp(b).unwrap());
                                let (mx, my) = (dx[dx.len() / 2], dy[dy.len() / 2]);
                                let ss: f32 = prs
                                    .iter()
                                    .map(|p| (p.2 - p.0 - mx).powi(2) + (p.3 - p.1 - my).powi(2))
                                    .sum();
                                (ss / prs.len() as f32).sqrt()
                            };
                            let h_rms = {
                                let ss: f32 = prs
                                    .iter()
                                    .map(|&(x, y, xp, yp)| {
                                        let pr = tv::m3_apply(&hm, [x, y, 1.0]);
                                        let wq = if pr[2].abs() > 1e-9 { pr[2] } else { 1e-9 };
                                        (pr[0] / wq - xp).powi(2) + (pr[1] / wq - yp).powi(2)
                                    })
                                    .sum();
                                (ss / prs.len() as f32).sqrt()
                            };
                            if h_rms * 1.5 < t_rms {
                                tv::rotation_from_homography(&hm, fx, fy, cx, cy)
                            } else {
                                tv::M3_IDENTITY
                            }
                        }
                        None => tv::M3_IDENTITY,
                    };
                    total_angle += tv::rotation_angle(&r);
                    r_cum[t] = tv::m3_mul(&r, &r_cum[t - 1]);
                }
                println!("accumulated rotation over GOP: {:.2} deg (sum of per-pair magnitudes)", total_angle.to_degrees());
                // Rectify all observations.
                for (_, v) in world.iter_mut() {
                    for o in v.iter_mut() {
                        let ray = [(o.screen_x - cx) / fx, (o.screen_y - cy) / fy, 1.0];
                        let r0 = tv::m3_apply(&tv::m3_transpose(&r_cum[o.frame_idx as usize]), ray);
                        if r0[2] > 1e-6 {
                            o.screen_x = fx * r0[0] / r0[2] + cx;
                            o.screen_y = fy * r0[1] / r0[2] + cy;
                        }
                    }
                }
                // Re-estimate translation from rectified static tracks.
                let mut rect_pf: Vec<std::collections::HashMap<u32, (f32, f32)>> =
                    vec![std::collections::HashMap::new(); nfr];
                for (id, v) in &world {
                    for o in v {
                        rect_pf[o.frame_idx as usize].insert(*id, (o.screen_x, o.screen_y));
                    }
                }
                // Translation-source selection, driven by what the
                // classifier learned about this content. On broadcast
                // footage phase correlation is HUD-locked (reads ≈0 —
                // near-zero baselines were silently poisoning every pose
                // downstream), so the median of the CLASSIFIED static
                // world tracks is the trustworthy camera estimate: gating
                // it in took the game GOP from 61 to 106 triangulated.
                // On parallax-dominant film the opposite holds — the
                // track median mixes depth-dependent flow and
                // UNDERestimates camera motion, while phase corr's
                // dominant peak locks to the background (measured: track-
                // median poses dropped the handheld GOP from 93 to 81).
                // The UiOverlay fraction is the regime detector: a big
                // screen-locked population is exactly the condition that
                // contaminates phase correlation.
                let ui_frac = {
                    let ui = classes.values().filter(|c| matches!(c, MotionClass::UiOverlay)).count();
                    ui as f32 / classes.len().max(1) as f32
                };
                if ui_frac > 0.15 {
                    let mut cx_cum = 0.0f32;
                    let mut cy_cum = 0.0f32;
                    for t in 1..nfr {
                        let mut dx: Vec<f32> = Vec::new();
                        let mut dy: Vec<f32> = Vec::new();
                        for (id, &(x1, y1)) in &rect_pf[t] {
                            if let Some(&(x0, y0)) = rect_pf[t - 1].get(id) {
                                dx.push(x1 - x0);
                                dy.push(y1 - y0);
                            }
                        }
                        if dx.len() >= 8 {
                            dx.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            dy.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            cx_cum += -dx[dx.len() / 2];
                            cy_cum += -dy[dy.len() / 2];
                        }
                        poses[t].position.x = cx_cum;
                        poses[t].position.y = cy_cum;
                    }
                    println!("translation source: static-track median (ui_frac {:.2} — phase-corr HUD-contaminated)", ui_frac);
                } else {
                    println!("translation source: phase correlation (ui_frac {:.2})", ui_frac);
                }
                println!("observations rectified by accumulated rotation");
            } else if args.iter().any(|a| a == "--affine") {
                // Rectify observations into frame-0's rotational frame using
                // the LINEAR part of the accumulated affine (translation
                // zeroed → pan/parallax preserved in the observations).
                let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
                for (_, v) in world.iter_mut() {
                    for o in v.iter_mut() {
                        let m = &cum[o.frame_idx as usize];
                        let lin_inv = gm::invert(&[m[0], m[1], 0.0, m[3], m[4], 0.0]).unwrap_or(gm::IDENTITY);
                        // rotate about the principal point, not the origin
                        let (rx, ry) = gm::apply(&lin_inv, o.screen_x - cx, o.screen_y - cy);
                        o.screen_x = rx + cx;
                        o.screen_y = ry + cy;
                    }
                }
                println!("observations rectified by accumulated linear affine");
            }
        }
        let cam_span = {
            let (mut minx, mut maxx, mut miny, mut maxy) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
            for p in &poses {
                minx = minx.min(p.position.x);
                maxx = maxx.max(p.position.x);
                miny = miny.min(p.position.y);
                maxy = maxy.max(p.position.y);
            }
            ((maxx - minx).powi(2) + (maxy - miny).powi(2)).sqrt()
        };
        println!("candidates: {} static world tracks (>=4 obs) | camera span {:.1}px", world.len(), cam_span);
        let tri = TriangulatePass::default();
        let pts = tri.triangulate_all(&world, &poses);
        if pts.is_empty() {
            println!("triangulated: 0");
            return;
        }
        let mut depths: Vec<f32> = pts.iter().map(|p| p.world_pos.z).collect();
        depths.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean_rep: f32 = pts.iter().map(|p| p.reprojection_error).sum::<f32>() / pts.len() as f32;
        let mean_base: f32 = pts.iter().map(|p| p.baseline).sum::<f32>() / pts.len() as f32;
        println!(
            "triangulated: {} of {} | depth z p10 {:.0} median {:.0} p90 {:.0} | mean reproj {:.2}px | mean baseline {:.1}px",
            pts.len(),
            world.len(),
            depths[depths.len() / 10],
            depths[depths.len() / 2],
            depths[depths.len() * 9 / 10],
            mean_rep,
            mean_base
        );
        return;
    }
    if args.iter().any(|a| a == "--track") {
        // Whole-scene readiness probe: run the real FeatureTrackPass over
        // the GOP and report the statistics pose estimation lives on —
        // per-frame survivors, survival rate across each step, track
        // lifetime distribution, and spatial coverage of LONG tracks
        // (>=8 frames; short tracks can't constrain a pose chain).
        use fao_codec::v8_reconstruction_codec::{FeatureTrackPass, MotionConsensusPass, MotionClass};
        use fao_codec::phase_correlation::PhaseCorrelationPass;
        let (w, h) = (frames[0].w, frames[0].h);
        let mut pass = FeatureTrackPass::default();
        let consensus = MotionConsensusPass::default();
        let pc = PhaseCorrelationPass::new(512.min(w.min(h)));
        let mut prev: Option<&Vec<u8>> = None;
        // Screen-locked accounting: per track, (frames where cam moved
        // >1px, frames among those where the track itself moved <0.5px).
        let mut locked: std::collections::HashMap<u32, (u32, u32)> = std::collections::HashMap::new();
        let mut classes: std::collections::HashMap<u32, MotionClass> = std::collections::HashMap::new();
        let mut lifetimes: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut long_track_cells: Vec<bool> = vec![false; ((w + 63) / 64) * ((h + 63) / 64)];
        let cw = (w + 63) / 64;
        let mut per_frame: Vec<usize> = Vec::new();
        for (i, f) in frames.iter().enumerate() {
            let feats = pass.process(&f.y, prev.map(|v| v.as_slice()), w, h, i as u32);
            per_frame.push(feats.len());
            if let Some(pv) = prev {
                let cam = pc.estimate(&f.y, pv, w, h);
                consensus.classify(&mut pass.live_tracks, cam.dx, cam.dy, cam.confidence);
                let cam_mag = (cam.dx * cam.dx + cam.dy * cam.dy).sqrt();
                for t in &pass.live_tracks {
                    classes.insert(t.feature_id, t.motion_class);
                    if cam.confidence > 0.2 && cam_mag > 1.0 {
                        let od = ((t.current_x - t.prev_x).powi(2) + (t.current_y - t.prev_y).powi(2)).sqrt();
                        let e = locked.entry(t.feature_id).or_insert((0, 0));
                        e.0 += 1;
                        if od < 0.5 {
                            e.1 += 1;
                        }
                    }
                }
            }
            for sf in &feats {
                let e = lifetimes.entry(sf.feature_id).or_insert(0);
                *e += 1;
                if *e >= 8 {
                    let cx = (sf.screen_x as usize).min(w - 1) / 64;
                    let cy = (sf.screen_y as usize).min(h - 1) / 64;
                    long_track_cells[cy * cw + cx] = true;
                }
            }
            prev = Some(&f.y);
        }
        let n = frames.len();
        let mut lt: Vec<u32> = lifetimes.values().copied().collect();
        lt.sort_unstable();
        let long_cov = long_track_cells.iter().filter(|&&c| c).count() as f32 / long_track_cells.len() as f32;
        println!(
            "tracks: total {} | lifetime median {} p90 {} max {} | frames>=8: {}",
            lt.len(),
            lt.get(lt.len() / 2).copied().unwrap_or(0),
            lt.get(lt.len() * 9 / 10).copied().unwrap_or(0),
            lt.last().copied().unwrap_or(0),
            lt.iter().filter(|&&v| v >= 8).count()
        );
        println!(
            "per-frame survivors: first {} mid {} last {} | long-track coverage {:.2}",
            per_frame[0], per_frame[n / 2], per_frame[n - 1], long_cov
        );
        // Class + screen-lock breakdown over >=8-frame tracks.
        let (mut st, mut dy, mut ui, mut un, mut scr) = (0usize, 0, 0, 0, 0usize);
        for (id, &life) in &lifetimes {
            if life < 8 {
                continue;
            }
            match classes.get(id) {
                Some(MotionClass::Static) => st += 1,
                Some(MotionClass::Dynamic) => dy += 1,
                Some(MotionClass::UiOverlay) => ui += 1,
                _ => un += 1,
            }
            if let Some(&(moving_frames, still_frames)) = locked.get(id) {
                if moving_frames >= 4 && still_frames * 10 >= moving_frames * 9 {
                    scr += 1;
                }
            }
        }
        println!(
            "long-track classes: static {st} dynamic {dy} ui {ui} unknown {un} | screen-locked {scr}"
        );
        return;
    }
    if args.iter().any(|a| a == "--features") {
        // Detection probe: FAST keypoints on first + mid frame, with
        // spatial coverage = fraction of 64px cells holding ≥1 keypoint.
        use fao_codec::v8_reconstruction_codec::FastDetector;
        let det = FastDetector { max_keypoints: 5000, ..FastDetector::default() };
        for (label, fi) in [("first", 0usize), ("mid", frames.len() / 2)] {
            let f = &frames[fi];
            let kps = det.detect(&f.y, f.w, f.h);
            let (cw, ch) = ((f.w + 63) / 64, (f.h + 63) / 64);
            let mut cells = vec![false; cw * ch];
            for k in &kps {
                cells[(k.y as usize / 64) * cw + (k.x as usize / 64)] = true;
            }
            let cov = cells.iter().filter(|&&c| c).count() as f32 / cells.len() as f32;
            println!("features {label}: n {} coverage {:.2}", kps.len(), cov);
        }
        return;
    }
    let t2 = std::time::Instant::now();
    if args.iter().any(|a| a == "--sr") {
        // Fence-slit experiment: prediction quality of the S=2
        // super-resolved plate vs the S=1 plate, then residual bytes
        // against the SR prediction. Luma PSNR computed directly.
        let sp = plate::build_plate_sr(&frames, &motions, 2);
        let psnr = |pred: &plate::Yuv420, f: &plate::Yuv420| -> f64 {
            let mut mse = 0.0f64;
            for i in 0..pred.y.len() {
                let d = pred.y[i] as f64 - f.y[i] as f64;
                mse += d * d;
            }
            mse /= pred.y.len() as f64;
            10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
        };
        let (mut p_base, mut p_sr) = (0.0f64, 0.0f64);
        for (f, m) in frames.iter().zip(&motions) {
            p_base += psnr(&plate::predict_frame(&sp.base, m, f.w, f.h), f);
            p_sr += psnr(&plate::predict_frame_sr(&sp, m, f.w, f.h), f);
        }
        let n = frames.len() as f64;
        println!("prediction_luma_psnr base {:.2} sr {:.2}  (delta {:+.2} dB)", p_base / n, p_sr / n, (p_sr - p_base) / n);
        let mut recs = Vec::with_capacity(frames.len());
        let mut res_total = 0usize;
        for (f, m) in frames.iter().zip(&motions) {
            let pred = plate::predict_frame_sr(&sp, m, f.w, f.h);
            let p444 = plate::interleave_444(&pred);
            let t444 = plate::interleave_444(f);
            match fao_codec::residual::encode(&p444, &t444, f.w, f.h, &opts) {
                Some(tr) => {
                    res_total += tr.len();
                    let r = fao_codec::residual::apply(p444, &tr, f.w, f.h);
                    recs.push(plate::deinterleave_420(&r, f.w, f.h));
                }
                None => recs.push(pred),
            }
        }
        println!("residual_total {}  marginal_per_frame {}", res_total, res_total / frames.len());
        write_y4m(&args[1], &header, &recs);
    } else if args.iter().any(|a| a == "--t3d") {
        let dz: f32 = arg_val(&args, "--dz").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let rep = plate::encode_gop_t3d(&frames, &motions, &pl, dz, opts.skip_threshold);
        let n = frames.len();
        let total = rep.plate_bytes + rep.side_bytes + rep.residual_bytes;
        println!("plate_bytes {}", rep.plate_bytes);
        println!(
            "side_bytes {} residual_bytes {}  prev_mode_share {:.3}  temporal_dc_share {:.3}",
            rep.side_bytes, rep.residual_bytes, rep.prev_mode_share, rep.temporal_dc_share
        );
        println!(
            "total_bytes {total}  amortized_per_frame {}  marginal_per_frame {}",
            total / n,
            (rep.side_bytes + rep.residual_bytes) / n
        );
        write_y4m(&args[1], &header, &rep.recon);
    } else if let Some(bandstr) = arg_val(&args, "--band") {
        let band: i32 = bandstr.parse().unwrap_or(0);
        let (rep, sigma, rho) = plate::encode_gop_v4(&frames, &motions, &pl, &opts, band);
        // Two-branch high-pass classification on a mid-GOP frame: how much
        // detail is world-derived (web's job) vs flat-mask (shader's job)?
        {
            let mid = frames.len() / 2;
            let m = &motions[mid];
            let pp = plate::predict_frame(&pl, m, frames[0].w, frames[0].h);
            let (wf, mf, hprms) = plate::classify_highpass(&frames[mid], &pp);
            println!("highpass_split world {:.2} mask {:.2} hp_rms {:.2} (vs plate pred, mid frame)", wf, mf, hprms);
        }
        let res_total: usize = rep.frames.iter().map(|c| c.residual_bytes).sum();
        let mode_total: usize = rep.frames.iter().map(|c| c.mode_bytes).sum();
        let n = frames.len();
        let total = rep.plate_bytes + rep.motion_bytes + res_total + mode_total;
        println!("plate_bytes {}", rep.plate_bytes);
        println!("mode_total {} residual_total {}  prev_mode_share {:.3}  grain_sigma {:.2} grain_rho {:.2}", mode_total, res_total, rep.prev_mode_share, sigma, rho);
        println!(
            "total_bytes {total}  amortized_per_frame {}  marginal_per_frame {}",
            total / n,
            (res_total + mode_total + rep.motion_bytes) / n
        );
        write_y4m(&args[1], &header, &rep.recon);
    } else if args.iter().any(|a| a == "--v1") {
        let rep = plate::encode_gop(&frames, &motions, &pl, &opts);
        let res_total: usize = rep.residual_bytes.iter().sum();
        let n = frames.len();
        let total = rep.plate_bytes + rep.motion_bytes + res_total;
        println!("plate_bytes {}", rep.plate_bytes);
        println!("residual_total {}", res_total);
        println!("total_bytes {total}  amortized_per_frame {}  marginal_per_frame {}", total / n, (res_total + rep.motion_bytes) / n);
        write_y4m(&args[1], &header, &rep.recon);
    } else {
        let rep = if args.iter().any(|a| a == "--v2") {
            plate::encode_gop_v2(&frames, &motions, &pl, &opts)
        } else {
            plate::encode_gop_v3(&frames, &motions, &pl, &opts)
        };
        let res_total: usize = rep.frames.iter().map(|c| c.residual_bytes).sum();
        let mode_total: usize = rep.frames.iter().map(|c| c.mode_bytes).sum();
        let n = frames.len();
        let total = rep.plate_bytes + rep.motion_bytes + res_total + mode_total;
        let mut per: Vec<usize> = rep.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).collect();
        println!("plate_bytes {}", rep.plate_bytes);
        println!("mode_total {} residual_total {}  prev_mode_share {:.3}", mode_total, res_total, rep.prev_mode_share);
        per.sort_unstable();
        println!("per_frame min {} median {} max {}", per[0], per[n / 2], per[n - 1]);
        println!(
            "total_bytes {total}  amortized_per_frame {}  marginal_per_frame {}",
            total / n,
            (res_total + mode_total + rep.motion_bytes) / n
        );
        write_y4m(&args[1], &header, &rep.recon);
    }
    eprintln!("encode+recon: {:.1}s", t2.elapsed().as_secs_f32());
}
