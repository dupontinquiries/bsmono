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

    let t2 = std::time::Instant::now();
    if args.iter().any(|a| a == "--t3d") {
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
