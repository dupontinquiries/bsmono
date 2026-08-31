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

    // --fuse-with <other.y4m>: build the canonical from THIS GOP plus
    // another take, pooled trimmed-median (canonical fusion / lineage
    // refinement). Combine with --save-plate to store the fused set
    // canonical.
    if let Some(other) = arg_val(&args, "--fuse-with") {
        let (frames2, _) = read_y4m(&other);
        let motions2 = plate::estimate_motions(&frames2);
        let fused = plate::build_plate_fused(&[(&frames[..], &motions[..]), (&frames2[..], &motions2[..])]);
        eprintln!("fused canonical: {}x{} from {} + {} frames", fused.w, fused.h, frames.len(), frames2.len());
        if let Some(sp) = arg_val(&args, "--save-plate") {
            plate::save_plate(&fused, &sp).unwrap_or_else(|e| die(&format!("save_plate: {e}")));
            println!("fused canonical saved to {sp}");
            return;
        }
    }
    if args.iter().any(|a| a == "--dictprobe") {
        let m = plate::estimate_motions(&frames);
        let pl = plate::build_plate(&frames, &m);
        println!("{}", plate::probe_dictionary(&frames, &pl, &m));
        return;
    }
    let t1 = std::time::Instant::now();
    // --use-plate <path>: CITE a stored canonical plate instead of
    // building+transmitting one. Motions get re-based by a phase-corr
    // alignment of frame 0 against the plate (8 wire bytes). This is
    // the library-citation bridge in its 2D-native form.
    // --route: encoder-side per-take routing. Encodes BOTH branches
    // (self-plate and cited canonical) and picks by RD cost
    // J = SSE + lambda*bytes with lambda ~ dz^2/3 (the standard
    // quantizer-tied multiplier shape). This turns the measured
    // routing law — cite only when the canonical beats your
    // self-model — into a code path instead of arithmetic in a log.
    // Lab-grade: runs both encodes in full; a production encoder
    // would estimate one branch cheaply first.
    let do_route = args.iter().any(|a| a == "--route") && arg_val(&args, "--use-plate").is_some();
    let (pl, motions, cited) = if let Some(pp) = arg_val(&args, "--use-plate") {
        let mut ext = plate::load_plate(&pp).unwrap_or_else(|e| die(&format!("load_plate: {e}")));
        if args.iter().any(|a| a == "--align-affine") {
            let (aligned, a) = plate::align_plate_affine(&ext, &frames[0]);
            eprintln!("cited plate affine-aligned: [{:.4} {:.4} {:.2}; {:.4} {:.4} {:.2}]", a[0], a[1], a[2], a[3], a[4], a[5]);
            ext = aligned;
        }
        let m2 = plate::motions_against_plate(&frames, &ext);
        // --patch-movers <sad/px thr>: the mover-layer MVP. Regions that
        // PERSISTENTLY deviate from the canonical are this take's staged
        // content (fusion voted the actors out of the canonical — the
        // donor-take asymmetry: it was repurchasing its own actors as
        // residual). Detect them, build the take's own plate, splice
        // those regions in, and transmit only the patch. Splice is at
        // frame-0 motion (quasi-static assumption — valid for the
        // locked-set citation regime this targets; general motion needs
        // per-frame layer compositing, the full mover-layer design).
        let ext = if let Some(thr_s) = arg_val(&args, "--patch-movers") {
            let thr: f32 = thr_s.parse().unwrap_or(10.0);
            const REGION: usize = 16;
            let (fw, fh) = (frames[0].w, frames[0].h);
            let (gw, gh) = (fw / REGION, fh / REGION);
            let mut dev = vec![0f32; gw * gh];
            let mut nsamp = 0u32;
            let mut fi = 0;
            while fi < frames.len() {
                let pred = plate::predict_frame(&ext, &m2[fi], fw, fh);
                for gy in 0..gh {
                    for gx in 0..gw {
                        let (rx, ry) = (gx * REGION, gy * REGION);
                        let mut sad = 0u32;
                        for dy in 0..REGION {
                            for dx in 0..REGION {
                                let a = pred.y[(ry + dy) * fw + rx + dx] as i32;
                                let b = frames[fi].y[(ry + dy) * fw + rx + dx] as i32;
                                sad += (a - b).unsigned_abs();
                            }
                        }
                        dev[gy * gw + gx] += sad as f32 / (REGION * REGION) as f32;
                    }
                }
                nsamp += 1;
                fi += 4;
            }
            let mover: Vec<bool> = dev.iter().map(|d| d / nsamp as f32 > thr).collect();
            let n_mover = mover.iter().filter(|&&m| m).count();
            let self_plate = plate::build_plate(&frames, &m2);
            let mut hybrid = ext.clone();
            let m0 = &m2[0];
            let mut patch: Vec<u8> = Vec::new();
            for gy in 0..gh {
                for gx in 0..gw {
                    if !mover[gy * gw + gx] {
                        continue;
                    }
                    for dy in 0..REGION {
                        for dx in 0..REGION {
                            let (fx, fy) = ((gx * REGION + dx) as f32, (gy * REGION + dy) as f32);
                            let sx = (fx - m0.dx - self_plate.ox).round() as isize;
                            let sy = (fy - m0.dy - self_plate.oy).round() as isize;
                            let hx = (fx - m0.dx - hybrid.ox).round() as isize;
                            let hy = (fy - m0.dy - hybrid.oy).round() as isize;
                            if sx >= 0 && sy >= 0 && (sx as usize) < self_plate.w && (sy as usize) < self_plate.h
                                && hx >= 0 && hy >= 0 && (hx as usize) < hybrid.w && (hy as usize) < hybrid.h
                            {
                                let v = self_plate.y[sy as usize * self_plate.w + sx as usize];
                                hybrid.y[hy as usize * hybrid.w + hx as usize] = v;
                                patch.push(v);
                                // chroma at half res
                                if dx % 2 == 0 && dy % 2 == 0 {
                                    let (scx, scy) = (sx as usize / 2, sy as usize / 2);
                                    let (hcx, hcy) = (hx as usize / 2, hy as usize / 2);
                                    let cw_s = self_plate.w / 2;
                                    let cw_h = hybrid.w / 2;
                                    let cu = self_plate.u[scy * cw_s + scx];
                                    let cv = self_plate.v[scy * cw_s + scx];
                                    hybrid.u[hcy * cw_h + hcx] = cu;
                                    hybrid.v[hcy * cw_h + hcx] = cv;
                                    patch.push(cu);
                                    patch.push(cv);
                                }
                            }
                        }
                    }
                }
            }
            let patch_bytes = zstd::encode_all(patch.as_slice(), 19).map(|c| c.len()).unwrap_or(patch.len())
                + (gw * gh + 7) / 8;
            println!(
                "mover_patch: {} of {} regions ({:.1}%), patch_bytes {}",
                n_mover, gw * gh,
                100.0 * n_mover as f32 / (gw * gh) as f32,
                patch_bytes
            );
            hybrid
        } else {
            ext
        };
        (ext, m2, true)
    } else {
        (plate::build_plate(&frames, &motions), motions, false)
    };
    eprintln!("plate: {}x{} origin ({:.1},{:.1}) cited={cited}  ({:.1}s)", pl.w, pl.h, pl.ox, pl.oy, t1.elapsed().as_secs_f32());
    if let Some(sp) = arg_val(&args, "--save-plate") {
        plate::save_plate(&pl, &sp).unwrap_or_else(|e| die(&format!("save_plate: {e}")));
        eprintln!("plate saved to {sp}");
    }
    let cited_plate = cited;

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
    if args.iter().any(|a| a == "--ledger") {
        println!("{}", fao_codec::v11_plate::residual_ledger(&frames));
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
        let mut pass = FeatureTrackPass {
            // Density for geometry export: default 500 is sized for the
            // codec's per-frame budget, not for scene capture.
            max_track_count: 2500,
            min_track_count: 500,
            ..FeatureTrackPass::default()
        };
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
        // Color-sampling positions from the RAW observations (rectification
        // below mutates geometry coords; pixels are indexed by raw coords).
        let color_at: std::collections::HashMap<u32, (u32, f32, f32)> = world
            .iter()
            .map(|(id, v)| {
                let o = v.last().unwrap();
                (*id, (o.frame_idx, o.screen_x, o.screen_y))
            })
            .collect();
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
        // Densification: --dense builds a DENSE mesh over the sparse
        // anchors (DensifyPass depth interpolation at two anchor frames
        // → MeshIntegrator fusion with real source colors) and swaps it
        // in for signature/library purposes. This is the step the
        // cross-take matching experiment demanded: ~100-point sparse PCA
        // signatures are take-variance-dominated; the signature was
        // designed for meshes where shape statistics stabilize.
        let dense_mesh: Option<fao_codec::v8_reconstruction_codec::SceneMesh> =
            if args.iter().any(|a| a == "--dense") {
                use fao_codec::v8_reconstruction_codec::{DensifyPass, MeshIntegrator, SceneMesh};
                let dp = DensifyPass::default();
                let mi = MeshIntegrator::default();
                let mut mesh = SceneMesh::default();
                let mut cov_report = Vec::new();
                for &af in &[0usize, frames.len() / 2] {
                    let pose = &poses[af];
                    // Project triangulated world points into the anchor.
                    let mut sp: Vec<(f32, f32, f32, f32)> = Vec::new();
                    for p in &pts {
                        let cz = p.world_pos.z - pose.position.z;
                        if cz <= 1.0 {
                            continue;
                        }
                        let sx = pose.principal[0] + pose.focal[0] * (p.world_pos.x - pose.position.x) / cz;
                        let sy = pose.principal[1] + pose.focal[1] * (p.world_pos.y - pose.position.y) / cz;
                        if sx < 0.0 || sy < 0.0 || sx >= w as f32 || sy >= h as f32 {
                            continue;
                        }
                        sp.push((sx, sy, cz, p.confidence));
                    }
                    let dm = dp.densify(&sp, w as u32, h as u32, af as u32);
                    let cov = dm.depths.iter().filter(|&&d| d > 0.0).count() as f32
                        / dm.depths.len() as f32;
                    cov_report.push(format!("anchor {af}: {} sparse → depth coverage {:.2}", sp.len(), cov));
                    let f = &frames[af];
                    let mut yuv = Vec::with_capacity(f.y.len() + f.u.len() + f.v.len());
                    yuv.extend_from_slice(&f.y);
                    yuv.extend_from_slice(&f.u);
                    yuv.extend_from_slice(&f.v);
                    let tris = mi.integrate(&dm, pose, dp.downsample, &mut mesh, &yuv, w, h);
                    cov_report.push(format!("anchor {af}: +{tris} triangles"));
                }
                println!(
                    "dense mesh: {} verts {} tris | {}",
                    mesh.vertices.len(),
                    mesh.indices.len() / 3,
                    cov_report.join(" | ")
                );
                Some(mesh)
            } else {
                None
            };
        // Cross-shot library experiment: --lib-add <dir> --name <label>
        // adds this GOP's sparse reconstruction as a canonical web;
        // --lib-match <dir> prints the similarity of this GOP's signature
        // against every stored web. Together they run the codec's core
        // asymptotic claim — same scene from different takes/angles
        // should match, different sets should not — live, for the first
        // time with a manifest that actually persists its first entry.
        let build_mesh = |pts: &[fao_codec::v8_reconstruction_codec::TriangulatedFeature]| {
            use fao_codec::v8_reconstruction_codec::SceneMesh;
            let mut mesh = SceneMesh::default();
            for p in pts {
                let Some(&(fi, sx, sy)) = color_at.get(&p.feature_id) else { continue };
                let f = &frames[fi as usize];
                let xi = (sx as usize).min(w - 1);
                let yi = (sy as usize).min(h - 1);
                let yv = f.y[yi * w + xi] as f32;
                let uv = f.u[(yi / 2) * (w / 2) + xi / 2] as f32 - 128.0;
                let vv = f.v[(yi / 2) * (w / 2) + xi / 2] as f32 - 128.0;
                let r = (yv + 1.402 * vv) / 255.0;
                let g = (yv - 0.344 * uv - 0.714 * vv) / 255.0;
                let b = (yv + 1.772 * uv) / 255.0;
                mesh.vertices.push(p.world_pos);
                mesh.vertex_colors.push([r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]);
            }
            mesh
        };
        if let Some(libdir) = arg_val(&args, "--lib-add") {
            use fao_codec::library::Library;
            use fao_codec::v8_reconstruction_codec::WebSignature;
            let name = arg_val(&args, "--name").unwrap_or_else(|| args[0].clone());
            let mesh = dense_mesh.clone().unwrap_or_else(|| build_mesh(&pts));
            let sig = WebSignature::from_mesh(&mesh);
            let dir = std::path::PathBuf::from(&libdir);
            if !dir.join("manifest.json").exists() {
                Library::init(&dir).unwrap_or_else(|e| die(&format!("lib init: {e}")));
            }
            let mut lib = Library::open_writable(&dir).unwrap_or_else(|e| die(&format!("lib open: {e}")));
            match lib.add_web(sig, &mesh, &poses, &name, 0, frames.len() as u32) {
                Ok(uuid) => println!("lib-add: {} as {} ({} verts)", name, uuid, mesh.vertices.len()),
                Err(e) => println!("lib-add REJECTED: {} ({} verts): {}", name, mesh.vertices.len(), e),
            }
        }
        if let Some(libdir) = arg_val(&args, "--lib-match") {
            use fao_codec::library::{signature_similarity, Library};
            use fao_codec::v8_reconstruction_codec::WebSignature;
            let mesh = dense_mesh.clone().unwrap_or_else(|| build_mesh(&pts));
            let sig = WebSignature::from_mesh(&mesh);
            let lib = Library::open(std::path::Path::new(&libdir)).unwrap_or_else(|e| die(&format!("lib open: {e}")));
            for stub in &lib.manifest.webs {
                println!(
                    "lib-match: vs {} ({}) similarity {:.3}",
                    stub.source_video, &stub.uuid[..8.min(stub.uuid.len())], 
                    signature_similarity(&stub.signature, &sig)
                );
            }
        }
        if let Some(ply_path) = arg_val(&args, "--export-ply") {
            // Colored sparse cloud, ASCII PLY: position from triangulation,
            // color sampled from the source frame at the point's last raw
            // observation (BT.601 full-range approximation).
            let mut body = String::new();
            let mut n_out = 0usize;
            for p in &pts {
                let Some(&(fi, sx, sy)) = color_at.get(&p.feature_id) else { continue };
                let f = &frames[fi as usize];
                let xi = (sx as usize).min(w - 1);
                let yi = (sy as usize).min(h - 1);
                let yv = f.y[yi * w + xi] as f32;
                let uv = f.u[(yi / 2) * (w / 2) + xi / 2] as f32 - 128.0;
                let vv = f.v[(yi / 2) * (w / 2) + xi / 2] as f32 - 128.0;
                let r = (yv + 1.402 * vv).round().clamp(0.0, 255.0) as u8;
                let g = (yv - 0.344 * uv - 0.714 * vv).round().clamp(0.0, 255.0) as u8;
                let b = (yv + 1.772 * uv).round().clamp(0.0, 255.0) as u8;
                // Flip Y so the cloud reads upright in standard viewers.
                body.push_str(&format!(
                    "{:.1} {:.1} {:.1} {} {} {}
",
                    p.world_pos.x, -p.world_pos.y, p.world_pos.z, r, g, b
                ));
                n_out += 1;
            }
            let header = format!(
                "ply
format ascii 1.0
comment fao-codec sparse triangulated cloud
element vertex {n_out}
property float x
property float y
property float z
property uchar red
property uchar green
property uchar blue
end_header
"
            );
            std::fs::write(&ply_path, header + &body).unwrap_or_else(|e| die(&format!("write {ply_path}: {e}")));
            println!("exported {} colored points -> {}", n_out, ply_path);
        }
        // --render3d <out.y4m>: THE BRIDGE — residual-coding against the
        // canonical's RENDER, so the 3D path produces BYTES for the first
        // time instead of match scores. The dense mesh is the canonical
        // (requires --dense); each frame's prediction is render_mesh at
        // that frame's pose; the prediction feeds encode_gop_v3_core —
        // the SAME mode decision / gain / side-info / DCT-residual
        // machinery as the 2D plate, so "3D render vs 2D plate as the
        // world model" is a controlled comparison. Accounting mirrors
        // the plate: model_bytes = serialized mesh (amortized; a
        // library-matched canonical ships as ~24 B UUID+alignment —
        // printed as the cited marginal), motion = 12 B/frame pose.
        // Unrasterized pixels render as neutral gray: the mode decision
        // routes those regions to prev/intra exactly as it does for
        // plate misses. Lab honesty: exact-black mesh output is
        // indistinguishable from a hole under this rule (none observed
        // on this material — film black lifts off 0 after the h264
        // chain).
        if let Some(out3d) = arg_val(&args, "--render3d") {
            use fao_codec::v8_rasterizer::render_mesh;
            let Some(mesh) = dense_mesh.as_ref() else {
                die("--render3d requires --dense (the sparse cloud does not rasterize)");
            };
            // Serialized canonical: verts f32x3 + colors u8x3 + indices
            // u32x3, zstd-19 — the wire form a library canonical would
            // actually ship (normals are recomputable; the spatial hash
            // is encoder state).
            let mesh_bytes = {
                let mut raw: Vec<u8> =
                    Vec::with_capacity(mesh.vertices.len() * 15 + mesh.indices.len() * 4);
                for v in &mesh.vertices {
                    raw.extend_from_slice(&v.x.to_le_bytes());
                    raw.extend_from_slice(&v.y.to_le_bytes());
                    raw.extend_from_slice(&v.z.to_le_bytes());
                }
                for c in &mesh.vertex_colors {
                    for ch in c {
                        raw.push((ch * 255.0).round().clamp(0.0, 255.0) as u8);
                    }
                }
                for i in &mesh.indices {
                    raw.extend_from_slice(&i.to_le_bytes());
                }
                zstd::encode_all(raw.as_slice(), 19).map(|c| c.len()).unwrap_or(raw.len())
            };
            let t3 = std::time::Instant::now();
            // Full-range BT.601, the exact inverse of the color sampling
            // that built the mesh (r = (Y + 1.402 V')/255 etc.).
            let rgb_to_yuv = |rgb: &[u8], fw: usize, fh: usize| -> plate::Yuv420 {
                let mut f = plate::Yuv420::new(fw, fh);
                let cw = fw / 2;
                let mut usum = vec![0u32; cw * (fh / 2)];
                let mut vsum = vec![0u32; cw * (fh / 2)];
                for y in 0..fh {
                    for x in 0..fw {
                        let i = (y * fw + x) * 3;
                        let (r, g, b) = (rgb[i] as f32, rgb[i + 1] as f32, rgb[i + 2] as f32);
                        let (r, g, b) = if rgb[i] == 0 && rgb[i + 1] == 0 && rgb[i + 2] == 0 {
                            (128.0, 128.0, 128.0) // hole -> neutral gray
                        } else {
                            (r, g, b)
                        };
                        let yv = 0.299 * r + 0.587 * g + 0.114 * b;
                        let uv = -0.168736 * r - 0.331264 * g + 0.5 * b + 128.0;
                        let vv = 0.5 * r - 0.418688 * g - 0.081312 * b + 128.0;
                        f.y[y * fw + x] = yv.round().clamp(0.0, 255.0) as u8;
                        let ci = (y / 2) * cw + x / 2;
                        usum[ci] += uv.round().clamp(0.0, 255.0) as u32;
                        vsum[ci] += vv.round().clamp(0.0, 255.0) as u32;
                    }
                }
                for (o, s) in f.u.iter_mut().zip(&usum) {
                    *o = (s / 4) as u8;
                }
                for (o, s) in f.v.iter_mut().zip(&vsum) {
                    *o = (s / 4) as u8;
                }
                f
            };
            let gray = plate::Yuv420::new(w, h); // render-failure fallback
            let mut n_rendered = 0usize;
            let mut pred_for = |fi: usize| -> plate::Yuv420 {
                match render_mesh(mesh, None, &poses[fi], w, h) {
                    Some(rgb) => {
                        n_rendered += 1;
                        rgb_to_yuv(&rgb, w, h)
                    }
                    None => {
                        let mut g = gray.clone();
                        for p in g.y.iter_mut() {
                            *p = 128;
                        }
                        for p in g.u.iter_mut().chain(g.v.iter_mut()) {
                            *p = 128;
                        }
                        g
                    }
                }
            };
            let pose_bytes = frames.len() * 12; // f32 xyz per frame
            let (rep, _) = plate::encode_gop_v3_core(
                &frames, &mut pred_for, mesh_bytes, pose_bytes, &opts, None,
            );
            let res_total: usize = rep.frames.iter().map(|c| c.residual_bytes).sum();
            let mode_total: usize = rep.frames.iter().map(|c| c.mode_bytes).sum();
            let n = frames.len();
            let total = mesh_bytes + pose_bytes + res_total + mode_total;
            let cited = 24 + pose_bytes + res_total + mode_total;
            eprintln!(
                "render3d: {} / {} frames rasterized  encode {:.1}s",
                n_rendered,
                n,
                t3.elapsed().as_secs_f32()
            );
            println!(
                "render3d byte split: mesh {} pose {} mode+mv {} residual {}",
                mesh_bytes, pose_bytes, mode_total, res_total
            );
            println!(
                "render3d prev_mode_share {:.3} (1.0 = render never used past frame 0)",
                rep.prev_mode_share
            );
            println!(
                "render3d total_bytes {total}  amortized_per_frame {}  cited_total {cited}  cited_per_frame {}",
                total / n,
                cited / n
            );
            write_y4m(&out3d, &header, &rep.recon);
        }
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
        eprintln!(
            "byte split: plate {} motion {} mode+mv {} residual {}",
            rep.plate_bytes, rep.motion_bytes, mode_total, res_total
        );
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
            if let Some(ls) = arg_val(&args, "--lookahead") {
                let thr: u32 = ls.parse().unwrap_or(1024);
                plate::encode_gop_v3_lookahead(&frames, &motions, &pl, &opts, thr)
            } else if do_route {
                // Branch A: self (own motions + own plate).
                let m_self = if args.iter().any(|a| a == "--static") {
                    vec![plate::PlateMotion { dx: 0.0, dy: 0.0, confidence: 1.0 }; frames.len()]
                } else {
                    plate::estimate_motions(&frames)
                };
                let pl_self = plate::build_plate(&frames, &m_self);
                let rep_a = plate::encode_gop_v3(&frames, &m_self, &pl_self, &opts);
                // Branch B: cited (pl/motions from --use-plate path).
                let rep_b = plate::encode_gop_v3(&frames, &motions, &pl, &opts);
                let sse = |rep: &plate::PlateEncodeReportV2| -> f64 {
                    let mut acc = 0.0f64;
                    for (r, f) in rep.recon.iter().zip(&frames) {
                        for (a, b) in r.y.iter().zip(&f.y) {
                            let d = *a as f64 - *b as f64;
                            acc += d * d;
                        }
                    }
                    acc
                };
                let bytes_a = rep_a.plate_bytes
                    + rep_a.motion_bytes
                    + rep_a.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).sum::<usize>();
                let bytes_b = 24
                    + rep_b.motion_bytes
                    + rep_b.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).sum::<usize>();
                let lambda = (opts.dead_zone as f64).powi(2) / 3.0;
                let (ja, jb) = (sse(&rep_a) + lambda * bytes_a as f64, sse(&rep_b) + lambda * bytes_b as f64);
                let pick_cited = jb < ja;
                println!(
                    "route: self J {:.3e} ({} B) | cited J {:.3e} ({} B) -> {}",
                    ja, bytes_a, jb, bytes_b,
                    if pick_cited { "CITED" } else { "SELF" }
                );
                if pick_cited { rep_b } else { rep_a }
            } else {
                plate::encode_gop_v3(&frames, &motions, &pl, &opts)
            }
        };
        let res_total: usize = rep.frames.iter().map(|c| c.residual_bytes).sum();
        let mode_total: usize = rep.frames.iter().map(|c| c.mode_bytes).sum();
        eprintln!(
            "byte split: plate {} motion {} mode+mv {} residual {}",
            rep.plate_bytes, rep.motion_bytes, mode_total, res_total
        );
        let n = frames.len();
        // A cited plate travels as a reference (UUID+alignment ≈ 24
        // bytes), not as payload.
        let plate_cost = if cited_plate { 24 } else { rep.plate_bytes };
        let total = plate_cost + rep.motion_bytes + res_total + mode_total;
        let mut per: Vec<usize> = rep.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).collect();
        println!("plate_bytes {} (cited: {})", plate_cost, cited_plate);
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
