//! subtile_gain — measure what subtiling buys, on real frames.
//!
//! The claim under test: a block that straddles the edge of something that moved is a
//! whole-cell miss even though most of it is unchanged, and its quadrants may already be
//! in the cache. That is an assertion about FOOTAGE, not about code, so it is measured
//! rather than argued.
//!
//! ```text
//! cargo run -p d2x-core --example subtile_gain -- <dir-of-pngs> [block] [ring]
//! ```
//!
//! For each frame, every cell that the whole-cell cache misses is looked up again at
//! quadrant granularity, against quadrants indexed from every cell upscaled so far. The
//! output is the share of misses that four cached quadrants could have covered — work the
//! network would not have had to do.
//!
//! No network runs: a hit is decided by content hashes, which is exactly what the real
//! cache decides on. So the numbers are the cache's, not an approximation of them.

use d2x_core::blockcache::{hash_cell, BlockCache};
use d2x_core::frame::{Frame, Pad};
use d2x_core::subtile::{subtile_keys, SubtileCfg};
use std::collections::HashSet;
use std::sync::Arc;

fn load(path: &std::path::Path) -> Option<Frame> {
    let img = image::open(path).ok()?.to_rgb8();
    Frame::from_rgb(img.width(), img.height(), img.into_raw()).ok()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| "/tmp/fr".to_string());
    let block: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(32);
    let ring: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(block / 2);
    // The SUBTILE's own ring, which need not be the cell's. This is the knob the first
    // measurement said everything about: with sub_ring = ring = block/2, a 16px quadrant is
    // hashed together with a 16px ring on every side — 48×48 pixels, of which the quadrant
    // is 11%. The key is then mostly context, so it almost never repeats, which is exactly
    // what the 0.5% coverage was saying.
    let sub_ring: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(ring);

    let cfg = SubtileCfg { levels: 1, ring, sub_ring, min_block: 8 };
    match cfg.refusal(block) {
        Some(why) => {
            println!("subtiling does not apply at block {block}, ring {ring}: {why}");
            return;
        }
        None => println!(
            "block {block}, cell ring {ring}, SUBTILE ring {sub_ring} — quadrants of {}px hashed over {}px",
            block / 2,
            block / 2 + 2 * sub_ring
        ),
    }

    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("frame dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        println!("no PNGs in {dir}");
        return;
    }

    // The whole-cell cache, as the pipeline runs it, and the quadrant store under it.
    let mut whole: HashSet<u64> = HashSet::new();
    let mut sub = BlockCache::new(u64::MAX / 2);
    let (mut tot_cells, mut tot_whole_hits, mut tot_sub_cover, mut tot_partial) = (0u64, 0u64, 0u64, 0u64);

    println!();
    println!("{:>5}  {:>7}  {:>8}  {:>8}  {:>8}", "frame", "cells", "whole", "subtile", "partial");
    for (n, p) in paths.iter().enumerate() {
        let Some(f) = load(p) else { continue };
        let padded = f.padded(ring, Pad::Black);
        let cols = f.width / block;
        let rows = f.height / block;
        let (mut cells, mut whole_hits, mut sub_cover, mut partial) = (0u64, 0u64, 0u64, 0u64);
        for by in 0..rows {
            for bx in 0..cols {
                let (x, y) = (bx * block, by * block);
                cells += 1;
                let key = hash_cell(&padded, x, y, block + 2 * ring);
                if whole.contains(&key) {
                    whole_hits += 1;
                    continue;
                }
                // A whole-cell miss. Could its quadrants have covered it?
                let qk = subtile_keys(&padded, x, y, block, sub_ring);
                let hits = qk.iter().filter(|k| sub.get(**k).is_some()).count();
                if hits == 4 {
                    sub_cover += 1;
                } else if hits > 0 {
                    partial += 1;
                }
                // Whatever the outcome, the network would have produced this cell, so it
                // and its quadrants enter the caches — the same order the pipeline uses.
                whole.insert(key);
                let qk_ins = subtile_keys(&padded, x, y, block, sub_ring);
                for k in qk_ins {
                    sub.insert(k, Arc::new(Vec::new()));
                }
            }
        }
        if n < 12 || n % 10 == 0 {
            println!("{n:>5}  {cells:>7}  {whole_hits:>8}  {sub_cover:>8}  {partial:>8}");
        }
        tot_cells += cells;
        tot_whole_hits += whole_hits;
        tot_sub_cover += sub_cover;
        tot_partial += partial;
    }

    let pct = |a: u64, b: u64| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
    let misses = tot_cells - tot_whole_hits;
    println!();
    println!("frames {}  cells {tot_cells}", paths.len());
    println!("whole-cell hits   {tot_whole_hits:>9}  ({:.1}% of cells)", pct(tot_whole_hits, tot_cells));
    println!("whole-cell misses {misses:>9}");
    println!(
        "  of those, coverable by four cached quadrants: {tot_sub_cover} ({:.1}% of misses, {:.1}% of all cells)",
        pct(tot_sub_cover, misses),
        pct(tot_sub_cover, tot_cells)
    );
    println!(
        "  partial (1-3 quadrants cached, a finer level might reach): {tot_partial} ({:.1}% of misses)",
        pct(tot_partial, misses)
    );
    println!();
    println!(
        "network calls: {} without subtiling, {} with — {:.1}% fewer",
        misses,
        misses - tot_sub_cover,
        pct(tot_sub_cover, misses)
    );
}
