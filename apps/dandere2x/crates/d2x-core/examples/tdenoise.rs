//! tdenoise — motion-compensated temporal median over ±K frames.
//!
//! The "artifacts first" experiment: a low-bitrate frame's codec artefacts
//! (blocking, ringing, mosquito noise) are not the same in its neighbours,
//! so a per-pixel median over motion-aligned neighbours keeps what the
//! frames agree on and drops what only one of them says. This is the plate
//! idea with the window centred on each frame and the alignment done per
//! block (diamond search against each neighbour) instead of globally, so it
//! survives the parallax and actors that broke the global plate.
//!
//! Usage:
//!   tdenoise <in.rgb> <out.rgb> <width> <height> [--k 4] [--rmse 12] [--mean|--sharpest] [--seed]
//!   tdenoise ... --from F [--oracle hi.rgb]
//!
//! `--from F` inverts the hunt: one frame F is the source and every other
//! frame's blocks are REPLACED by their aligned block from F wherever the
//! alignment passes the gate (alignment is always computed on the input
//! frames, since that is all a real pipeline has). With `--oracle hi.rgb`
//! the copied pixels come from the high-bitrate version of F instead: the
//! upper bound of "restore the one best frame perfectly, then propagate it",
//! which is what targeting the network at unique frames would achieve.
//!
//! `in.rgb` is raw rgb24 frames (ffmpeg -f rawvideo -pix_fmt rgb24). A
//! neighbour's block is used only if its RMSE against the frame's own block
//! is ≤ `--rmse` (misregistered or occluded blocks stay out of the vote).
//! `--seed` hints each neighbour search with the accumulated global motion
//! from phase correlation. `--mean` averages instead of taking the median.
//! `--sharpest` takes, per block, the aligned candidate with the most
//! high-frequency energy instead of a median: a low-bitrate encoder's
//! artefact is temporally coherent smoothing, not independent noise, and
//! its quality varies inside a GOP (keyframes and reference frames get more
//! bits), so the "truth" a window holds is in its best-encoded frame, not
//! in the majority vote.
//!
//! Score with the high-bitrate encode of the same frames as ground truth;
//! that is the only honest reference for "did it remove artefacts or detail".
//!
//! Measured (Transformers, 32 frames at 24.55 s, 558 kbps vs 2.5 Mbps AV1,
//! same source, frame-aligned; frames 4..27 scored):
//!   as-is                       37.46 dB   SSIM 0.9740
//!   median ±4, rmse 12, seeded  37.45 dB   SSIM 0.9750   (97% of blocks had ≥1 aligned neighbour)
//!   median ±2, rmse 9,  seeded  37.44 dB   SSIM 0.9746
//!   sharpest ±4                 36.51 dB   (−1 dB on every frame)
//! The low-bitrate artefact is temporally COHERENT smoothing (deblock, CDEF,
//! loop restoration remove the same detail in every frame), not independent
//! per-frame noise: a median has nothing to vote against, and "take the
//! sharpest aligned block" copies misalignment as if it were detail. The GOP
//! quality spread is mild (one frame at 40.7 dB, the rest 36–38).
//!
//! `--from 6` (the 40.7 dB frame), same clip, PSNR / SSIM vs the 2.5 Mbps encode:
//!   distance   as-is          from 6 (low)   from 6 + oracle
//!   ±1         38.2 / 0.979   37.6 / 0.978   39.0 / 0.987   (+0.8 dB, +0.008)
//!   ±2         37.8 / 0.977   37.1 / 0.975   38.1 / 0.982
//!   ±3–4       37.3 / 0.975   36.7 / 0.973   37.4 / 0.978   (≈ 0)
//!   ±6–12      37.5 / 0.974   36.6 / 0.970   37.0 / 0.971   (loss)
//! Propagating the best LOW frame loses everywhere (its 40.7 dB is lower
//! low-frequency error, not more detail: HF 1.80 vs neighbours 1.8).
//! Propagating a perfectly RESTORED frame gains for ±2 frames and the copied
//! detail is real (HF 1.91–1.95 vs ground truth 1.90–1.98); past ±2 integer
//! alignment drift eats it. So one restored frame in ~5, propagated ±2, is
//! the shape that pays; sub-pel alignment is what would widen it.

use d2x_core::frame::{block_sse_bounded, Frame};
use d2x_core::matcher::{BlockMatcher, Diamond};
use d2x_core::phase_corr::PhaseCorrelation;
use d2x_core::predict::block_hf_energy;
use rayon::prelude::*;

fn read_frames(path: &str, w: u32, h: u32) -> anyhow::Result<Vec<Frame>> {
    let raw = std::fs::read(path)?;
    let fb = (w * h * 3) as usize;
    anyhow::ensure!(raw.len() % fb == 0, "{path}: not a whole number of {w}x{h} rgb24 frames");
    Ok((0..raw.len() / fb).map(|t| Frame::from_rgb(w, h, raw[t * fb..(t + 1) * fb].to_vec()).unwrap()).collect())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    anyhow::ensure!(args.len() >= 5, "usage: tdenoise in.rgb out.rgb width height [--k N] [--rmse T] [--mean] [--seed]");
    let (w, h): (u32, u32) = (args[3].parse()?, args[4].parse()?);
    let mut k = 4usize;
    let mut rmse = 12.0f64;
    let mut mean = false;
    let mut sharpest = false;
    let mut seed = false;
    let mut from: Option<usize> = None;
    let mut oracle: Option<Vec<Frame>> = None;
    let mut i = 5;
    while i < args.len() {
        match args[i].as_str() {
            "--k" => { k = args[i + 1].parse()?; i += 1; }
            "--rmse" => { rmse = args[i + 1].parse()?; i += 1; }
            "--mean" => mean = true,
            "--sharpest" => sharpest = true,
            "--from" => { from = Some(args[i + 1].parse()?); i += 1; }
            "--oracle" => { oracle = Some(read_frames(&args[i + 1], w, h)?); i += 1; }
            "--seed" => seed = true,
            a => anyhow::bail!("unknown arg {a}"),
        }
        i += 1;
    }
    let bs = 24u32; // 1920 and 1080 are both multiples of 24
    anyhow::ensure!(w % bs == 0 && h % bs == 0, "frame must be a multiple of {bs}");
    let frames = read_frames(&args[1], w, h)?;
    let n = frames.len();
    let raw_len = n * (w * h * 3) as usize;
    eprintln!("{n} frames {w}x{h}, k={k}, rmse<={rmse}, {}, seed={seed}", if sharpest { "sharpest" } else if mean { "mean" } else { "median" });

    // Pairwise global motion, accumulated, for seeding.
    let pc = PhaseCorrelation::for_frame(w, h);
    let mut disp = vec![(0.0f32, 0.0f32); n];
    if seed {
        for t in 1..n {
            let m = pc.estimate(&frames[t], &frames[t - 1]);
            disp[t] = (disp[t - 1].0 + m.dx, disp[t - 1].1 + m.dy);
        }
    }
    let limit = (rmse * rmse * (bs * bs * 3) as f64) as u64;
    let matcher = Diamond { max_steps: 16, radius: 24 };
    let cols = (w / bs) as usize;
    let rows = (h / bs) as usize;

    let mut out = Vec::with_capacity(raw_len);
    let mut used_total = 0u64;
    let mut blocks_total = 0u64;
    for t in 0..n {
        let cur = &frames[t];
        let (lo, hi) = match from {
            Some(f) => (f, f),
            None => (t.saturating_sub(k), (t + k).min(n - 1)),
        };
        if from == Some(t) {
            // The source frame itself: pass through (or its oracle, which is
            // "the network restored this one frame perfectly").
            let src = oracle.as_ref().map_or(cur, |o| &o[t]);
            out.extend_from_slice(&src.data);
            blocks_total += (cols * rows) as u64;
            eprintln!("frame {t:3}: source frame");
            continue;
        }
        // Per block: the set of aligned neighbour blocks that pass the RMSE test.
        let per_block: Vec<(Vec<(usize, u32, u32)>, u32)> = (0..cols * rows)
            .into_par_iter()
            .map(|bi| {
                let (x, y) = (((bi % cols) as u32) * bs, ((bi / cols) as u32) * bs);
                let mut srcs = Vec::new();
                for s in lo..=hi {
                    if s == t {
                        continue;
                    }
                    // cur[x] ≈ frames[s][x − (disp[t] − disp[s])] → source offset.
                    let hint = if seed {
                        (-((disp[t].0 - disp[s].0).round() as i64), -((disp[t].1 - disp[s].1).round() as i64))
                    } else {
                        (0, 0)
                    };
                    let c = matcher.search_from(&frames[s], cur, x, y, bs, limit, hint);
                    if let Some(c) = c {
                        if block_sse_bounded(cur, x, y, &frames[s], c.src_x, c.src_y, bs, limit).is_some() {
                            srcs.push((s, c.src_x, c.src_y));
                        }
                    }
                }
                (srcs, 0)
            })
            .collect();
        let mut f = cur.clone();
        let mut used = 0u64;
        for (bi, (srcs, _)) in per_block.iter().enumerate() {
            if srcs.is_empty() {
                continue;
            }
            used += 1;
            let (x, y) = (((bi % cols) as u32) * bs, ((bi / cols) as u32) * bs);
            if from.is_some() {
                let (s, sx, sy) = srcs[0];
                let src = oracle.as_ref().map_or(&frames[s], |o| &o[s]);
                f.copy_block(src, bs, sx, sy, x, y);
                continue;
            }
            if sharpest {
                let own = block_hf_energy(cur, x, y, bs);
                let best = srcs.iter().map(|&(s, sx, sy)| (block_hf_energy(&frames[s], sx, sy, bs), s, sx, sy)).fold(None, |acc: Option<(f64, usize, u32, u32)>, c| if acc.map_or(true, |a| c.0 > a.0) { Some(c) } else { acc });
                if let Some((e, s, sx, sy)) = best {
                    if e > own {
                        f.copy_block(&frames[s], bs, sx, sy, x, y);
                    }
                }
                continue;
            }
            let m = srcs.len() + 1;
            let mut samples = vec![0u8; m];
            for j in 0..bs {
                for i in 0..bs {
                    for c in 0..3 {
                        samples[0] = cur.data[cur.idx(x + i, y + j) + c];
                        for (q, &(s, sx, sy)) in srcs.iter().enumerate() {
                            samples[q + 1] = frames[s].data[frames[s].idx(sx + i, sy + j) + c];
                        }
                        let v = if mean {
                            ((samples.iter().map(|&v| v as u32).sum::<u32>() + (m as u32) / 2) / m as u32) as u8
                        } else {
                            let s = &mut samples[..m];
                            // Lower-median for even counts keeps the value a real sample.
                            let (_, med, _) = s.select_nth_unstable((m - 1) / 2);
                            *med
                        };
                        let di = f.idx(x + i, y + j) + c;
                        f.data[di] = v;
                    }
                }
            }
        }
        used_total += used;
        blocks_total += (cols * rows) as u64;
        eprintln!("frame {t:3}: {used}/{} blocks had ≥1 aligned neighbour", cols * rows);
        out.extend_from_slice(&f.data);
    }
    std::fs::write(&args[2], &out)?;
    eprintln!("done: {:.1}% of blocks denoised", 100.0 * used_total as f64 / blocks_total.max(1) as f64);
    Ok(())
}
