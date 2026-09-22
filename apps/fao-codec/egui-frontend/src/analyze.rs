//! analyze — how much would a SHARED block dictionary save across these files?
//!
//! ## Which "shared library"
//!
//! This app has two. The Web Library dedups 3D meshes from the v8 reconstruction
//! codec (Worldloom), which is a scaffold -- for real footage today it would
//! honestly estimate zero, so it is not what this measures. The BLOCK DICTIONARY
//! (Runehoard, `fao_codec_lib::block_dict`) is the one built for game footage:
//! "in game footage, the SAME 16x16 luma blocks appear thousands of times --
//! HUD, textures, sprites". It stores each unique block once and frames refer to
//! it by id. Matches share HUDs, maps and menus, so one dictionary across many
//! files should beat one per file. This measures by how much.
//!
//! ## Faithful to the encoder, not a lookalike
//!
//! Blocks are cut EXACTLY as `encode_frame_from_dict` cuts them -- 16x16 luma,
//! whole blocks only (`w / 16` drops a partial edge), raw bytes row by row,
//! hashed with the dictionary's own `hash_block` -- from the SAME luma the real
//! pipeline makes (ffmpeg rgb24 -> `rgb_to_yuv420p` -> Y). A test checks the
//! unique count agrees with a real `BlockDictionary` fed the same frame.
//!
//! ## Fast, and what "fast" costs
//!
//! It samples K frames spread evenly through each file, each with an input seek
//! (`-ss` before `-i`) that jumps to a keyframe instead of decoding up to it --
//! a few frames per file, not the whole video. Two consequences, both reported:
//!
//!   * unique counts from a sample are a LOWER BOUND on the whole file's -- more
//!     frames only find more distinct blocks. So a cap warning is conservative:
//!     if the sample already overflows, the full selection certainly does.
//!   * the SAVINGS RATIO (shared unique / sum of per-file unique) is the robust
//!     number; absolute byte totals are the sample's, scaled, and approximate.

use fao_codec_lib::block_dict::BlockDictionary;
use std::collections::{HashMap, HashSet};

/// The dictionary's hard ceiling. At it, `encode_frame_from_dict` BAILS -- the
/// K-frame is not encoded at all -- so this is a cliff, not a soft limit.
pub const MAX_BLOCKS: usize = BlockDictionary::MAX_BLOCKS as usize;

/// Frames sampled per file by default: enough to see a HUD repeat, few enough
/// to stay fast.
pub const DEFAULT_SAMPLES: usize = 12;

/// The 16x16 luma blocks of one frame, exactly as `encode_frame_from_dict`
/// cuts them.
pub fn luma_blocks(y: &[u8], w: usize, h: usize) -> Vec<Vec<u8>> {
    const B: usize = 16;
    let (bw, bh) = (w / B, h / B);
    let mut out = Vec::with_capacity(bw * bh);
    for by in 0..bh {
        for bx in 0..bw {
            let mut block = Vec::with_capacity(B * B);
            for py in 0..B {
                let row = (by * B + py) * w + bx * B;
                block.extend_from_slice(&y[row..row + B]);
            }
            out.push(block);
        }
    }
    out
}

/// What one file's sample contributed.
#[derive(Debug, Clone, Default)]
pub struct FileBlocks {
    pub name: String,
    pub frames: usize,
    /// Samples asked for that produced no frame even after the retry. Counted,
    /// not silently dropped: a sample that quietly shrank is a weaker estimate
    /// that looks like a strong one.
    pub missed: usize,
    /// Every sampled block's hash, duplicates included.
    pub hashes: Vec<u64>,
}

impl FileBlocks {
    /// Add one frame's luma plane.
    pub fn add_frame(&mut self, y: &[u8], w: usize, h: usize, keep: &mut HashMap<u64, Vec<u8>>) {
        for b in luma_blocks(y, w, h) {
            let hsh = BlockDictionary::hash_block(&b);
            // Keep a bounded sample of distinct block BYTES, for measuring what
            // the dictionary really compresses them to.
            if keep.len() < COMPRESSION_SAMPLE { keep.entry(hsh).or_insert(b); }
            self.hashes.push(hsh);
        }
        self.frames += 1;
    }
}

/// Distinct blocks kept to measure compressed size.
const COMPRESSION_SAMPLE: usize = 4096;

/// Bytes the dictionary stores per unique block, MEASURED by serialising real
/// blocks through `BlockDictionary::to_bytes` (zstd level 12) rather than taking
/// the "~80 bytes after zstd" in block_dict.rs's header, which is a claim.
pub fn bytes_per_block(sample: &HashMap<u64, Vec<u8>>) -> f64 {
    if sample.is_empty() { return 256.0; }
    let mut d = BlockDictionary::new();
    for b in sample.values() { d.intern(b.clone()); }
    d.to_bytes().len() as f64 / sample.len() as f64
}

/// One file's line in the report.
#[derive(Debug, Clone, PartialEq)]
pub struct FileLine {
    pub name: String,
    pub unique: usize,
    /// Of its unique blocks, how many also appear in some OTHER selected file.
    pub shared_with_others: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub files: usize,
    pub frames: usize,
    pub blocks: usize,
    /// Sum of each file's own unique-block count: one dictionary per file.
    pub per_file_unique: usize,
    /// Unique blocks across the whole selection: one shared dictionary.
    pub shared_unique: usize,
    pub bytes_per_block: f64,
    pub lines: Vec<FileLine>,
    pub missed: usize,
}

impl Report {
    /// Blocks a shared dictionary stores once that per-file dictionaries store
    /// repeatedly.
    pub fn saved_blocks(&self) -> usize { self.per_file_unique - self.shared_unique }
    /// The robust number: fraction of per-file dictionary storage saved.
    pub fn savings_ratio(&self) -> f64 {
        if self.per_file_unique == 0 { 0.0 } else { self.saved_blocks() as f64 / self.per_file_unique as f64 }
    }
    pub fn per_file_bytes(&self) -> f64 { self.per_file_unique as f64 * self.bytes_per_block }
    pub fn shared_bytes(&self) -> f64 { self.shared_unique as f64 * self.bytes_per_block }
    /// How often a block repeats INSIDE a file -- what a per-file dictionary
    /// already buys before sharing adds anything.
    pub fn within_file_reuse(&self) -> f64 {
        if self.per_file_unique == 0 { 0.0 } else { self.blocks as f64 / self.per_file_unique as f64 }
    }
    /// True when even the SAMPLE overflows one dictionary. The sample's count is
    /// a lower bound, so this warning is conservative.
    pub fn shared_overflows(&self) -> bool { self.shared_unique > MAX_BLOCKS }
}

/// The analysis itself. Pure, so it is tested with hand-built hashes.
pub fn analyze(files: &[FileBlocks], bytes_per_block: f64) -> Report {
    let sets: Vec<HashSet<u64>> = files.iter().map(|f| f.hashes.iter().copied().collect()).collect();
    let mut seen_in: HashMap<u64, usize> = HashMap::new();
    for s in &sets { for h in s { *seen_in.entry(*h).or_insert(0) += 1; } }
    let lines = files.iter().zip(&sets).map(|(f, s)| FileLine {
        name: f.name.clone(),
        unique: s.len(),
        shared_with_others: s.iter().filter(|h| seen_in[h] > 1).count(),
    }).collect();
    Report {
        files: files.len(),
        frames: files.iter().map(|f| f.frames).sum(),
        blocks: files.iter().map(|f| f.hashes.len()).sum(),
        per_file_unique: sets.iter().map(|s| s.len()).sum(),
        shared_unique: seen_in.len(),
        bytes_per_block,
        lines,
        missed: files.iter().map(|f| f.missed).sum(),
    }
}

/// Sample `k` frames spread through one file and cut their blocks.
///
/// Frame times sit at the MIDDLE of k equal slices, so neither the opening
/// menu nor the final scoreboard is over-represented. Each is an input seek, so
/// a frame costs a keyframe decode, not a decode from the start.
pub fn sample_file(input: &str, name: &str, k: usize, keep: &mut HashMap<u64, Vec<u8>>)
    -> Result<FileBlocks, String> {
    let info = crate::transcode::probe(input)?;
    let (w, h) = (info.w & !1, info.h & !1);
    let mut fb = FileBlocks { name: name.to_string(), ..Default::default() };
    let dur = info.duration_s.max(0.0);
    let times: Vec<f64> = if dur > 0.0 {
        (0..k.max(1)).map(|i| dur * (i as f64 + 0.5) / k.max(1) as f64).collect()
    } else {
        vec![0.0]
    };
    let grab = |t: f64, keyframes_only: bool| -> Result<Vec<u8>, String> {
        let mut c = std::process::Command::new("ffmpeg");
        c.args(["-v", "error"]);
        // KEYFRAMES ONLY, the fast path. A normal input seek lands on the
        // keyframe BEFORE `t` and then decodes forward to `t` -- up to a whole
        // GOP of frames to deliver one. Skipping non-keyframes makes a sample one
        // keyframe decode. Measured on 1080p60 H.264 with a 2 s GOP (OBS's
        // default): 338 ms/frame plain, 114 ms/frame keyframe-only -- 3x.
        if keyframes_only { c.args(["-skip_frame", "nokey"]); }
        c.args(["-ss", &format!("{t:.3}"), "-i"]).arg(input)
         .args(["-map", &format!("0:{}", info.stream), "-frames:v", "1"]);
        if (w, h) != (info.w, info.h) { c.args(["-vf", &format!("crop={w}:{h}:0:0")]); }
        c.args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
        Ok(c.output().map_err(|e| format!("ffmpeg: {e}"))?.stdout)
    };
    for t in times {
        // The keyframe-only seek returns NOTHING for a time past the file's
        // last keyframe -- measured: the final slice of a 20 s file came back
        // empty, and `-noaccurate_seek` did not change that. So a miss is
        // retried with the ordinary seek, which always delivers; only the rare
        // miss pays full price.
        let mut rgb = grab(t, true)?;
        if rgb.len() != w * h * 3 { rgb = grab(t, false)?; }
        if rgb.len() != w * h * 3 { fb.missed += 1; continue; }
        let yuv = crate::decode::rgb_to_yuv420p(&rgb, w, h);
        fb.add_frame(&yuv[..w * h], w, h, keep);
    }
    if fb.frames == 0 { return Err(format!("{name}: no frames could be sampled")); }
    Ok(fb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb(name: &str, hashes: &[u64]) -> FileBlocks {
        FileBlocks { name: name.into(), frames: 1, hashes: hashes.to_vec(), ..Default::default() }
    }

    /// The core arithmetic, on hashes whose overlap is known exactly.
    #[test]
    fn savings_count_blocks_shared_across_files() {
        // A: {1,2,3} (1 repeats). B: {2,3,4}. Shared across files: 2 and 3.
        let r = analyze(&[fb("a", &[1, 1, 2, 3]), fb("b", &[2, 3, 4])], 100.0);
        assert_eq!(r.per_file_unique, 6, "3 + 3");
        assert_eq!(r.shared_unique, 4, "{{1,2,3,4}}");
        assert_eq!(r.saved_blocks(), 2);
        assert!((r.savings_ratio() - 2.0 / 6.0).abs() < 1e-12);
        assert_eq!(r.lines[0], FileLine { name: "a".into(), unique: 3, shared_with_others: 2 });
        assert_eq!(r.lines[1].shared_with_others, 2);
        assert!((r.per_file_bytes() - 600.0).abs() < 1e-9 && (r.shared_bytes() - 400.0).abs() < 1e-9);
    }

    #[test]
    fn disjoint_files_save_nothing_and_identical_files_save_half() {
        let none = analyze(&[fb("a", &[1, 2]), fb("b", &[3, 4])], 1.0);
        assert_eq!(none.savings_ratio(), 0.0);
        let same = analyze(&[fb("a", &[1, 2]), fb("b", &[1, 2])], 1.0);
        assert!((same.savings_ratio() - 0.5).abs() < 1e-12, "two identical files: store once, save half");
        assert_eq!(analyze(&[], 1.0).savings_ratio(), 0.0);
    }

    /// Within-file reuse is separate from cross-file sharing.
    #[test]
    fn within_file_reuse_is_reported_on_its_own() {
        let r = analyze(&[fb("a", &[7, 7, 7, 7])], 1.0);
        assert_eq!(r.within_file_reuse(), 4.0);
        assert_eq!(r.savings_ratio(), 0.0, "one file shares with nobody");
    }

    #[test]
    fn the_cap_warning_fires_past_max_blocks() {
        let big: Vec<u64> = (0..(MAX_BLOCKS as u64 + 5)).collect();
        assert!(analyze(&[fb("a", &big)], 1.0).shared_overflows());
        assert!(!analyze(&[fb("a", &[1, 2, 3])], 1.0).shared_overflows());
    }

    /// FAITHFUL TO THE ENCODER: cut the same frame through the real
    /// `encode_frame_from_dict` and through `luma_blocks`; the serialised
    /// dictionaries must be identical.
    ///
    /// The fixture has STRUCTURE INSIDE EVERY BLOCK, on purpose. My first
    /// version used a checkerboard of solid blocks -- and it passed with the
    /// blocks cut column-major, because a solid block reads the same either way.
    /// It could not catch the one bug a faithfulness test exists for. Now a
    /// block's bytes depend on the order they are read in, and the repeats are
    /// arranged so that only row-major cutting finds them.
    #[test]
    fn block_cutting_agrees_with_the_real_dictionary() {
        let (w, h) = (80usize, 48usize); // 5 x 3 whole blocks
        let mut yuv = vec![0u8; w * h * 3 / 2];
        // Inside a block: x varies faster than y, so transposing changes bytes.
        // Across blocks: every other block repeats, so there ARE duplicates.
        for y in 0..h { for x in 0..w {
            let (px, py) = (x % 16, y % 16);
            let kind = (x / 16 + y / 16) % 2;
            yuv[y * w + x] = (px * 11 + py * 3 + kind * 97) as u8;
        } }
        let mut real = BlockDictionary::new();
        fao_codec_lib::block_dict::encode_frame_from_dict(&yuv, w, h, &mut real).unwrap();
        let mine: HashSet<u64> = luma_blocks(&yuv[..w * h], w, h).iter()
            .map(|b| BlockDictionary::hash_block(b)).collect();
        assert_eq!(luma_blocks(&yuv[..w * h], w, h).len(), 15);
        assert_eq!(mine.len(), 2, "two block kinds, repeating");
        assert_eq!(real.to_bytes().len(), {
            let mut d = BlockDictionary::new();
            for b in luma_blocks(&yuv[..w * h], w, h) { d.intern(b); }
            d.to_bytes().len()
        }, "same blocks, same serialised dictionary");
    }

    /// A partial edge is dropped, as `w / 16` drops it in the encoder.
    #[test]
    fn a_partial_edge_block_is_not_counted() {
        assert_eq!(luma_blocks(&vec![0u8; 40 * 16], 40, 16).len(), 2, "40 wide = 2 whole blocks + 8 px dropped");
    }

    /// THE LAST SLICE IS RECOVERED. On a 20 s file with a 2 s GOP the
    /// keyframe-only seek for the final slice returns nothing; the retry must
    /// deliver it, so every requested sample is really sampled.
    #[test]
    fn every_requested_sample_is_delivered_even_near_the_end() {
        let ok = std::process::Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false);
        if !ok { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("analyze_end_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("gop.mp4");
        assert!(std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i",
            "testsrc2=size=128x96:rate=30:duration=20", "-c:v", "mpeg4", "-g", "60", "-pix_fmt", "yuv420p"])
            .arg(&f).status().unwrap().success());
        let mut keep = HashMap::new();
        let fb = sample_file(f.to_str().unwrap(), "gop", 12, &mut keep).unwrap();
        assert_eq!((fb.frames, fb.missed), (12, 0), "all 12 slices, including the last");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// END TO END with real ffmpeg: two clips sharing a static "HUD" band share
    /// blocks; a clip of unrelated content shares none of its moving area.
    #[test]
    fn real_clips_that_share_a_hud_show_savings() {
        let ok = std::process::Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false);
        if !ok { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("analyze_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Same solid HUD bar over different moving backgrounds.
        // NATIVE ENCODERS ONLY. These fixtures used libx264, which Fedora's
        // ffmpeg-free leaves out (patents): fleet_build.sh on Fedora failed with
        // "Unknown encoder 'libx264'". ffv1 is ffmpeg's own LOSSLESS codec --
        // needed here, since the analysis counts byte-identical blocks -- and it
        // is in every build. It does not go in MP4, hence .mkv.
        let make = |name: &str, src: &str| {
            let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", src,
                "-vf", "drawbox=x=0:y=0:w=iw:h=64:color=0x2040a0:t=fill", "-pix_fmt", "yuv420p", "-c:v", "ffv1"])
                .arg(dir.join(name)).status().unwrap().success();
            assert!(ok, "{name}");
        };
        make("m1.mkv", "testsrc=size=256x192:rate=10:duration=2");
        make("m2.mkv", "testsrc2=size=256x192:rate=10:duration=2");
        let mut keep = HashMap::new();
        let a = sample_file(dir.join("m1.mkv").to_str().unwrap(), "m1", 4, &mut keep).unwrap();
        let b = sample_file(dir.join("m2.mkv").to_str().unwrap(), "m2", 4, &mut keep).unwrap();
        assert_eq!((a.frames, b.frames), (4, 4));
        let r = analyze(&[a, b], bytes_per_block(&keep));
        assert!(r.savings_ratio() > 0.0, "the shared HUD must show up: {r:?}");
        assert!(r.lines.iter().all(|l| l.shared_with_others > 0));
        assert!(r.bytes_per_block > 0.0 && r.bytes_per_block < 256.0, "zstd must compress a flat bar: {}", r.bytes_per_block);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
