//! bench — regenerate the corpus numbers in one command.
//!
//! Written because getting these numbers back costs an hour of manual
//! ffmpeg-and-grep, and that cost is what makes people quote stale
//! figures. Every measurement in `docs/SESSION_LOG.md` that compares
//! configurations was produced by hand; this reproduces the same shape
//! automatically so a change can be checked against the corpus in one
//! step instead of being argued about.
//!
//! Deliberately headless and CSV-emitting rather than a GUI: the
//! `egui-frontend` "benchmark this library" button should SHELL OUT to
//! this, so the numbers do not depend on a windowing stack that cannot
//! be verified in CI or in a sandbox. That split also means the bench
//! runs on a build machine with no display.
//!
//! Scoring is MS-SSIM plus grid-discontinuity, NOT PSNR, for the reason
//! the log records at length: PSNR does not charge for the artifact
//! this codec produces most, and every comparison scored on it in this
//! project turned out to flatter the codec.
//!
//! Usage:
//!   bench <clip.y4m> [more.y4m ...]              default config only
//!   bench --configs <clip.y4m> ...               sweep the named configs
//!   bench --csv out.csv <clip.y4m> ...
//!
//! Environment overrides (FAO_DEBLOCK, FAO_PLATE_DZ_DIV, ...) still
//! apply and are recorded in the CSV, so a sweep is reproducible from
//! its own output.

use fao_codec::residual::ResidualOpts;
use fao_codec::v11_plate as plate;

fn read_y4m(path: &str) -> (usize, usize, Vec<plate::Yuv420>) {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let hdr_end = data.iter().position(|&b| b == b'\n').expect("y4m header");
    let hdr = String::from_utf8_lossy(&data[..hdr_end]).to_string();
    let (mut w, mut h) = (0usize, 0usize);
    for tok in hdr.split_whitespace() {
        match tok.as_bytes().first() {
            Some(b'W') => w = tok[1..].parse().unwrap_or(0),
            Some(b'H') => h = tok[1..].parse().unwrap_or(0),
            _ => {}
        }
    }
    let fsz = w * h * 3 / 2;
    let mut off = hdr_end + 1;
    let mut frames = Vec::new();
    while off < data.len() {
        let Some(rel) = data[off..].iter().position(|&b| b == b'\n') else { break };
        off += rel + 1;
        if off + fsz > data.len() {
            break;
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
    (w, h, frames)
}

/// Grid-discontinuity ratio at one period; 1.0 = no grid structure.
/// Duplicated from bin/blockiness deliberately — a bench that depends
/// on another bin's internals breaks when that bin is edited for a
/// one-off investigation, which is exactly what happened to the manual
/// measurements this replaces.
fn grid_ratio(w: usize, h: usize, frames: &[plate::Yuv420], period: usize) -> f32 {
    let (mut on, mut on_n, mut off, mut off_n) = (0.0f64, 0u64, 0.0f64, 0u64);
    for f in frames {
        for y in 0..h {
            for x in 1..w {
                let d = (f.y[y * w + x] as i32 - f.y[y * w + x - 1] as i32).unsigned_abs() as f64;
                if x % period == 0 {
                    on += d;
                    on_n += 1;
                } else {
                    off += d;
                    off_n += 1;
                }
            }
        }
    }
    if on_n == 0 || off_n == 0 {
        return 1.0;
    }
    let b = off / off_n as f64;
    if b < 1e-9 { 1.0 } else { (on / on_n as f64 / b) as f32 }
}

struct Row {
    clip: String,
    config: String,
    frames: usize,
    bytes: usize,
    msssim: f64,
    /// Chroma PSNR, U and V. Reported because the luma-only figure
    /// hid a 12 dB chroma deficit against AV1 (SESSION_LOG 2026-09-01).
    psnr_u: f64,
    psnr_v: f64,
    p16_src: f32,
    p16_out: f32,
    secs: f32,
}

fn plane_psnr(recon: &[plate::Yuv420], src: &[plate::Yuv420], get: impl Fn(&plate::Yuv420) -> &Vec<u8>) -> f64 {
    let (mut se, mut n) = (0.0f64, 0u64);
    for (r, s) in recon.iter().zip(src) {
        for (&a, &b) in get(r).iter().zip(get(s)) {
            let d = a as f64 - b as f64;
            se += d * d;
            n += 1;
        }
    }
    if se == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 * n as f64 / se).log10() }
}

fn run(clip: &str, config: &str, dz: f32) -> Row {
    let (w, h, frames) = read_y4m(clip);
    let opts = ResidualOpts { dead_zone: dz, ..ResidualOpts::default() };
    let t0 = std::time::Instant::now();
    // FAO_NOPLATE=1: no long-term plate at all — frame 0 from gray
    // (intra + residual), later frames from whatever the core's model
    // is (with FAO_GMC, the warped previous reconstruction).
    // FAO_PRED_Y4M=<file>: an EXTERNAL model — e.g. an 8x-downscaled
    // AV1 base layer decoded and upscaled — used as the mode-0
    // predictor. Its bytes are charged by the caller (the base
    // stream), not here.
    let rep = if let Ok(path) = std::env::var("FAO_PRED_Y4M") {
        let (_, _, pred) = read_y4m(&path);
        let n = frames.len().min(pred.len());
        let (r, _) = plate::encode_gop_v3_core(&frames[..n], &mut |fi| pred[fi].clone(), 0, 0, &opts, None);
        r
    } else if std::env::var("FAO_NOPLATE").map(|v| v != "0").unwrap_or(false) {
        plate::encode_gop_no_plate(&frames, &opts)
    } else {
        let m = plate::estimate_motions(&frames);
        let pl = plate::build_plate(&frames, &m);
        plate::encode_gop_v3(&frames, &m, &pl, &opts)
    };
    let secs = t0.elapsed().as_secs_f32();
    let bytes = rep.plate_bytes
        + rep.motion_bytes
        + rep.frames.iter().map(|c| c.residual_bytes + c.mode_bytes).sum::<usize>();
    Row {
        clip: clip.rsplit('/').next().unwrap_or(clip).to_string(),
        config: config.to_string(),
        frames: frames.len(),
        bytes,
        msssim: fao_codec::msssim::recon_score(&rep.recon, &frames),
        psnr_u: plane_psnr(&rep.recon, &frames, |f| &f.u),
        psnr_v: plane_psnr(&rep.recon, &frames, |f| &f.v),
        // The SOURCE's own grid figure is reported alongside the
        // output's, because "less blocky" is only meaningful against
        // what the content already had — deblock strength 4 was
        // rejected for pushing the output BELOW the source.
        p16_src: grid_ratio(w, h, &frames, 16),
        p16_out: grid_ratio(w, h, &rep.recon, 16),
        secs,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut csv: Option<String> = None;
    let mut sweep = false;
    let mut clips = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--csv" => {
                csv = args.get(i + 1).cloned();
                i += 2;
            }
            "--configs" => {
                sweep = true;
                i += 1;
            }
            other => {
                clips.push(other.to_string());
                i += 1;
            }
        }
    }
    if clips.is_empty() {
        eprintln!("usage: bench [--configs] [--csv out.csv] <clip.y4m> ...");
        std::process::exit(2);
    }

    // The configs a change most often needs checking against. dz32 is
    // the operating point every number in the log uses.
    let configs: Vec<(&str, f32)> = if sweep {
        vec![("dz16", 16.0), ("dz32", 32.0), ("dz48", 48.0)]
    } else {
        vec![("dz32", 32.0)]
    };

    let mut rows = Vec::new();
    for clip in &clips {
        for (name, dz) in &configs {
            eprintln!("  {clip} @ {name} ...");
            rows.push(run(clip, name, *dz));
        }
    }

    println!(
        "{:<20} {:<6} {:>4} {:>10} {:>9} {:>6} {:>6} {:>8} {:>8} {:>7}",
        "clip", "cfg", "n", "bytes", "MS-SSIM", "U dB", "V dB", "P16 src", "P16 out", "sec"
    );
    for r in &rows {
        println!(
            "{:<20} {:<6} {:>4} {:>10} {:>9.5} {:>6.2} {:>6.2} {:>8.3} {:>8.3} {:>7.1}",
            r.clip, r.config, r.frames, r.bytes, r.msssim, r.psnr_u, r.psnr_v, r.p16_src, r.p16_out, r.secs
        );
    }
    // A single figure of merit, in BYTES, on the same axis every
    // constant in this codec was priced against: what the encode costs
    // plus what its quality shortfall would cost to buy back through
    // the quantiser (~25 MB per MS-SSIM point at dz32). Lower is
    // better, and unlike bytes-alone it cannot be gamed by degrading.
    const LADDER: f64 = 25_000_000.0;
    let total: f64 = rows
        .iter()
        .map(|r| r.bytes as f64 + (1.0 - r.msssim) * LADDER)
        .sum();
    println!("\nfigure of merit (bytes + quality shortfall priced at the ladder): {total:.0}");

    if let Some(path) = csv {
        let mut out =
            String::from("clip,config,frames,bytes,msssim,psnr_u,psnr_v,p16_src,p16_out,seconds\n");
        for r in &rows {
            out.push_str(&format!(
                "{},{},{},{},{:.5},{:.2},{:.2},{:.3},{:.3},{:.1}\n",
                r.clip, r.config, r.frames, r.bytes, r.msssim, r.psnr_u, r.psnr_v, r.p16_src, r.p16_out, r.secs
            ));
        }
        std::fs::write(&path, out).unwrap_or_else(|e| panic!("write {path}: {e}"));
        eprintln!("wrote {path}");
    }
}
