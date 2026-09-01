//! blockiness — measure the artifact PSNR is worst at seeing.
//!
//! Every AV1 comparison in this codebase has been anchored on PSNR (and
//! sometimes SSIM), and both are close to blind to the specific failure
//! a block-transform coder has at low rate: the picture stays close in
//! mean-square terms while acquiring hard discontinuities on the
//! transform grid. A viewer calls that "blocky and unbearable" well
//! before PSNR calls it bad, which means a matched-PSNR comparison can
//! flatter whichever coder degrades that way.
//!
//! The measurement is deliberately simple and assumption-free about
//! which coder produced the file: for each candidate grid period P,
//! compare the mean absolute first difference ACROSS columns that fall
//! on a multiple of P against the mean at all other columns, and the
//! same for rows. A coder with no grid gives a ratio near 1.0. A coder
//! quantising P x P transforms leaves steps at the boundaries and the
//! ratio rises above 1. Reporting per-period rather than a single
//! number matters because AV1 chooses among several block sizes, so the
//! signature appears at whichever period dominates.
//!
//! What this is NOT: a perceptual quality score. It measures one
//! artifact. A codec could be terrible in other ways and score 1.0
//! here, so it belongs beside PSNR and SSIM, not instead of them.
//!
//! Usage: blockiness <file.y4m> [more.y4m ...]

fn read_y4m(path: &str) -> (usize, usize, Vec<Vec<u8>>) {
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
    assert!(w > 0 && h > 0, "{path}: no geometry in header");
    let fsz = w * h * 3 / 2;
    let mut off = hdr_end + 1;
    let mut frames = Vec::new();
    while off < data.len() {
        let Some(rel) = data[off..].iter().position(|&b| b == b'\n') else { break };
        off += rel + 1;
        if off + fsz > data.len() {
            break;
        }
        frames.push(data[off..off + w * h].to_vec());
        off += fsz;
    }
    (w, h, frames)
}

/// Ratio of mean |gradient| on the grid to mean |gradient| off it.
/// Averaged over frames; luma only, which is where blocking is seen.
fn grid_ratio(w: usize, h: usize, frames: &[Vec<u8>], period: usize) -> f32 {
    let (mut on, mut on_n, mut off, mut off_n) = (0.0f64, 0u64, 0.0f64, 0u64);
    for f in frames {
        // Vertical edges: difference across column x.
        for y in 0..h {
            for x in 1..w {
                let d = (f[y * w + x] as i32 - f[y * w + x - 1] as i32).unsigned_abs() as f64;
                if x % period == 0 {
                    on += d;
                    on_n += 1;
                } else {
                    off += d;
                    off_n += 1;
                }
            }
        }
        // Horizontal edges: difference across row y.
        for y in 1..h {
            for x in 0..w {
                let d = (f[y * w + x] as i32 - f[(y - 1) * w + x] as i32).unsigned_abs() as f64;
                if y % period == 0 {
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
    let (a, b) = (on / on_n as f64, off / off_n as f64);
    if b < 1e-9 {
        return 1.0;
    }
    (a / b) as f32
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: blockiness <file.y4m> [more.y4m ...]");
        std::process::exit(2);
    }
    println!(
        "{:<28} {:>7} {:>7} {:>7} {:>7} {:>7}",
        "file", "P=4", "P=8", "P=16", "P=32", "P=64"
    );
    for path in &args {
        let (w, h, frames) = read_y4m(path);
        if frames.is_empty() {
            eprintln!("{path}: no frames");
            continue;
        }
        // Every period is a multiple of 4 in AV1/this codec, so P=4
        // catches everything and the larger periods isolate WHICH size
        // dominates. A file whose worst ratio sits at P=4 is quantising
        // small transforms; at P=64 it is using huge ones and the steps
        // between them are what the eye reads as blocking.
        let r: Vec<f32> = [4usize, 8, 16, 32, 64]
            .iter()
            .map(|&p| grid_ratio(w, h, &frames, p))
            .collect();
        let name = path.rsplit('/').next().unwrap_or(path);
        println!(
            "{:<28} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3}",
            name, r[0], r[1], r[2], r[3], r[4]
        );
    }
    println!("\n1.000 = no grid structure. Higher = harder steps on that grid.");
}
