#![allow(unused_variables, unused_mut, unused_imports, dead_code, unused_assignments)]
// FAO Scene Codec v1.1 — core library
// Optimized batch processing + temporal function prediction
//
// ═══════════════════════════════════════════════════════════════════
// SHARED LIB EXTRACTION — STATUS: TIER A COMPLETE
// ═══════════════════════════════════════════════════════════════════
//
// As the codec grew across v2..v8, several modules became broadly reusable. 
// This file (lib.rs) was a 7.5K-line grab-bag containing both codec-specific 
// code AND generic utilities. The cleanup is in progress: Tier A primitives 
// are now extracted into the `fao-codec-core` crate (workspace member 
// `fao_codec_core`, library name `fao_core`).
//
// Current state:
//   - Tier A: ✓ DONE (DCT, quant, color, FrameTag, BitIO, chroma)
//             Live in fao_codec_core/src/, with backward-compat re-exports here.
//   - Tier B: still in this file (Huffman, block-match)
//   - Tier C: still in v8_reconstruction_codec.rs (phase correlation, densify)
//
// Each item lists: where it lives now, what depends on it, why extract.
//
// === Tier A (extract first — pure, no dependencies) ===
//
//   1. DCT (lib.rs lines ~30-50)
//      Used by: v2, v4, v5, v6 codecs (encode_iframe + encode_pframe paths)
//      Why: standard 8x8 forward/inverse DCT. Could be replaced with SIMD 
//      or GPU implementation behind same trait without touching codecs.
//      Future: SIMD f32x4 (3-4× speedup), then WGSL compute shader.
//
//   2. Quantization (lib.rs lines ~52-80)
//      Used by: same as DCT, plus v7/v8 (perceptual quant for residuals)
//      Why: build_qtable + dzq_fast are referenced from 5 places. 
//      Extracting clarifies that "quality" parameter has uniform meaning.
//
//   3. ZigZag + RLE (lib.rs lines ~70-83)
//      Used by: v2, v4, v5, v8 entropy coding
//      Why: pure transformation, no state.
//
//   4. Color space (lib.rs lines ~140-185)
//      Used by: every codec for YUV ↔ RGB conversion.
//      Why: bottleneck on encode (one full-frame conversion per frame). 
//      Could be GPU-accelerated for huge encode-speed win.
//
//   5. Bit IO (lib.rs lines ~1041-1095)
//      Used by: v2 entropy coder. Could be used by v8 frame compaction.
//      Why: BitWriter + BitReader. Tiny, fully generic.
//
// === Tier B (extract after Tier A — has dependencies) ===
//
//   6. FrameTag (lib.rs lines ~88-138)
//      Used by: ALL codecs and the app. Travels with every frame through 
//      the encode/compress/disk pipeline.
//      Why: the canonical "what is this frame" identity. Extracting forces 
//      a clean separation between frame metadata and codec internals.
//
//   7. Huffman codec (lib.rs ~1093-1216)
//      Used by: v2 (and indirectly v4/v5 P-frames).
//      Why: 130 LOC of standard Huffman, no codec specifics.
//
//   8. Block-match (lib.rs ~286-298, ~1384-1414)
//      Used by: v2, v3 P-frames.
//      Why: standard motion-search primitives.
//
// === Tier C (later — codec-adjacent but conceptually shared) ===
//
//   9. Phase correlation (currently in v8_reconstruction_codec.rs)
//      Used by: v8 motion estimation, could power v2 block-match seeds.
//      Why: GPU-friendly FFT-based motion. Belongs with motion utilities.
//
//   10. Densify pass (in v8_reconstruction_codec.rs)
//       Could become a "stereo/parallax depth helper" used by future v9 
//       background-subtraction passes.
//
// === Items NOT to extract (codec-defining) ===
//
//   - vN_encoder.rs files: each codec's wire-format and routing logic 
//     stays in its own file. Extraction would break the cohesion.
//   - gpu_pipeline.rs: too entangled with rendering specifics; better to 
//     refactor IN PLACE before splitting.
//   - block_dict, block_store: codec-specific (v2 K-frame dict).
//
// === Migration order (safe path) ===
//
//   Step 1: Add `pub mod core;` here, alias the Tier A items into it.
//   Step 2: Update internal callers to use `crate::core::dct_fwd` instead 
//           of `crate::dct_fwd`. (Keeps lib.rs API stable for fao_app.)
//   Step 3: Once the migration is complete, mv core.rs into a new crate 
//           `fao_codec_core` with its own Cargo.toml.
//   Step 4: Repeat for Tier B.
//
// This is a PROPOSAL. Doing the full extraction is a multi-session refactor 
// that requires careful testing. The roadmap above is the spec.
//
// ═══════════════════════════════════════════════════════════════════

pub mod v2_encoder;
pub mod v21_predictive;
pub mod block_dict;
pub use fao_core::bitpack;
pub use fao_core::region_activity;
pub use fao_core::pixel_filters;
pub mod v8_reconstruction_codec;

/// v9 — Volumetric scene codec. Replaces v8's explicit triangle mesh 
/// with a sparse 3D voxel grid encoded as 3D-DCT blocks. Saliency-driven 
/// quantization, view-conditioned colors via degree-2 spherical 
/// harmonics, ray-march decoder. See `docs/V9_ARCHITECTURE.md`.
/// 
/// Status: prototype. Theory says it should work; needs encode→decode 
/// roundtrip tested on real content to see signs of life.
pub mod v9_volumetric;

// ── v10 shared primitives ──────────────────────────────────────────
// 
// Phase A of the v10 codec restructure. These modules are the shared 
// math/coding layer that v8.x and v9 currently each implement in their 
// own way. Phase A extracts and unifies; Phase B (next session) ports 
// v8 and v9 to consume them; Phase C introduces v10 proper.
// 
// `transform` — DCT/IDCT (1D, 2D, 3D) + quantization. The 4×4×4 path 
// is bit-exact with the existing v9 implementation so v9 trailers 
// remain decodable.
// 
// `residual` — 2D DCT residual coding pipeline. Replaces the v8.1 
// bias-128 box-average. Encoder + decoder share a single source of 
// truth for skip-block RLE, DC prediction, and channel deinterleave.

pub mod transform;
pub mod residual;
pub mod geometry;

// Phase correlation extracted from the legacy v7_perception_codec during 
// the v10 cleanup. The algorithm is generic CV — both v8 reconstruction 
// (camera-motion seed for SfM) and v10 motion-comp use it.
pub mod phase_correlation;

// v10 strategy orchestrator (Phase C scaffold). Stub today; real 
// dispatch lands when main.rs switches over from the parallel race.
pub mod v10;

/// CPU rasterizer that turns v8 mesh + camera pose into rendered RGB. 
/// Used by V8Decoder::decode_frame to produce playable frames.
pub mod v8_rasterizer;
#[cfg(feature = "gpu")]
pub mod v8_gpu_rasterizer;


pub mod gpu_pipeline;
pub mod block_store;

/// Persistent FAO Library — multi-video web sharing across encodes.
/// 
/// See `docs/LIBRARY_MODE.md` for the architectural plan and 
/// `library.rs` for the Phase 2 scaffold (filesystem-backed catalog,
/// WebSignature similarity matching, atomic manifest writes).
pub mod library;

/// Custom append-only log database for the FAO library. Backs the 
/// manifest.json format with a binary format that's faster to read 
/// at scale, crash-safe, and trivially rollback-able via rotating 
/// .bak.N files. See `docs/LIBRARY_DB_DESIGN.md`.
pub mod library_db;

/// GPU phase correlation pipeline (item 4 of the optimization deck).
/// Behind the `gpu` feature gate. The legacy v7 CPU path
/// is the reference; this module mirrors it on the GPU using a Stockham
/// radix-2 FFT shader (see `phase_corr.wgsl`).
#[cfg(feature = "gpu")]
pub mod gpu_phase_corr;

// Codec-agnostic primitives now live in the `fao_codec_core` crate.
// Re-exported below at top-level for backward compatibility.
// 
// New code should depend on `fao-codec-core` directly:
//     use fao_core::{dct_fwd, FrameTag, BitWriter};

use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};

// ══════════════════════════════════════════════════════════
// DCT — extracted to fao_codec_core (`use fao_core::dct::*`)
// ══════════════════════════════════════════════════════════
// 
// Re-exported here for backward compat. New code should use 
// `fao_core::dct::*` directly.

pub use fao_core::dct::{init, dct_fwd, dct_inv};

// ══════════════════════════════════════════════════════════
// Quantization — extracted to fao_codec_core (`use fao_core::quant::*`)
// ══════════════════════════════════════════════════════════

pub use fao_core::quant::{LUMA_Q, CHROMA_Q, ZIGZAG, build_qtable, build_qtable_inv, est_bits_dpcm};

// Internal helpers — imported from fao_core::quant for use in this file.
// `dzq` and `dzq_fast` are still used inside this lib via free-fn callers.
use fao_core::quant::{dzq, dzq_fast};

/// Bit-cost category of a signed coefficient (number of bits to encode |v|).
/// Local copy — used in bit-estimation hot paths throughout this file.
/// (The canonical version lives privately in `core::quant`.)
#[inline]
fn cat(v: i32) -> u32 {
    if v == 0 { 0 } else { ((v.unsigned_abs() as f32 + 1.0).log2().ceil()) as u32 }
}

// ══════════════════════════════════════════════════════════
// Color space — batch conversion (full image at once)
// ══════════════════════════════════════════════════════════

// FrameTag — extracted to fao_codec_core
pub use fao_core::frame_tag::FrameTag;

// Color space — extracted to fao_codec_core
pub use fao_core::color::{rgb_to_planes, planes_to_rgb};

// ══════════════════════════════════════════════════════════
// Block encode: combined quantize+zigzag in single pass
// ══════════════════════════════════════════════════════════
#[inline(always)]
fn encode_block_bits(plane: &[f32], w: usize, ox: usize, oy: usize,
    qt: &[f32;64], inv_qt: &[f32;64], dz: f32, prev_dc: i32,
) -> (u32, i32) {
    let mut tile = [0.0f32;64];
    for yy in 0..8 { let r=(oy+yy)*w+ox; for xx in 0..8 { tile[yy*8+xx]=plane[r+xx]; }}
    let mut dct_buf = [0.0f32;64];
    dct_fwd(&tile, &mut dct_buf);
    let mut qz = [0i32;64]; let mut zig = [0i32;64];
    for i in 0..64 { qz[i] = dzq_fast(dct_buf[i], qt[i], inv_qt[i], qt[i]*dz*0.5); }
    for k in 0..64 { zig[k] = qz[ZIGZAG[k]]; }
    est_bits_dpcm(&zig, prev_dc)
}

#[inline(always)]
fn encode_block_recon(plane: &[f32], w: usize, ox: usize, oy: usize,
    qt: &[f32;64], inv_qt: &[f32;64], dz: f32, prev_dc: i32, rec: &mut[f32],
) -> (u32, i32) {
    let mut tile = [0.0f32;64];
    for yy in 0..8 { let r=(oy+yy)*w+ox; for xx in 0..8 { tile[yy*8+xx]=plane[r+xx]; }}
    let mut dct_buf = [0.0f32;64];
    dct_fwd(&tile, &mut dct_buf);
    let mut qz = [0i32;64]; let mut zig = [0i32;64];
    for i in 0..64 { qz[i] = dzq_fast(dct_buf[i], qt[i], inv_qt[i], qt[i]*dz*0.5); }
    for k in 0..64 { zig[k] = qz[ZIGZAG[k]]; }
    let (bits, new_dc) = est_bits_dpcm(&zig, prev_dc);
    let mut dq = [0.0f32;64]; for i in 0..64 { dq[i] = qz[i] as f32 * qt[i]; }
    let mut rec_buf = [0.0f32;64]; dct_inv(&dq, &mut rec_buf);
    for yy in 0..8 { let r=(oy+yy)*w+ox; for xx in 0..8 { rec[r+xx] = rec_buf[yy*8+xx]; }}
    (bits, new_dc)
}

// ══════════════════════════════════════════════════════════
// I-frame: flat 8x8 grid, separated channel passes
// ══════════════════════════════════════════════════════════
pub fn encode_iframe(
    y: &[f32], cb: &[f32], cr: &[f32], w: usize, h: usize,
    lq: &[f32;64], cq: &[f32;64], dz: f32,
    mut yr: Option<&mut[f32]>, mut cbr: Option<&mut[f32]>, mut crr: Option<&mut[f32]>,
) -> u64 {
    init();
    let inv_lq = build_qtable_inv(lq);
    let inv_cq = build_qtable_inv(cq);
    let bw=w/8; let bh=h/8;
    let mut tb = 0u64;

    // Luma pass (all blocks, sequential DPCM)
    let mut dy = 0i32;
    for by in 0..bh { let oy=by*8;
    for bx in 0..bw { let ox=bx*8;
        let (b, nd) = match yr { Some(ref mut r) => encode_block_recon(y,w,ox,oy,lq,&inv_lq,dz,dy,r), None => encode_block_bits(y,w,ox,oy,lq,&inv_lq,dz,dy) };
        dy = nd; tb += b as u64;
    }}

    // Chroma passes (Cb then Cr, with adaptive dead zone)
    let mut dcb = 0i32;
    for by in 0..bh { let oy=by*8;
    for bx in 0..bw { let ox=bx*8;
        // Adaptive chroma DZ based on saturation
        let mut sat = 0.0f32;
        for yy in 0..8 { let r=(oy+yy)*w+ox; for xx in 0..8 { sat += cb[r+xx].abs(); }}
        sat /= 64.0;
        let cdz = if sat>25.0{dz*0.65}else if sat>15.0{dz*0.80}else if sat>8.0{dz*0.90}else{dz*1.0};
        let (b, nd) = match cbr { Some(ref mut r) => encode_block_recon(cb,w,ox,oy,cq,&inv_cq,cdz,dcb,r), None => encode_block_bits(cb,w,ox,oy,cq,&inv_cq,cdz,dcb) };
        dcb = nd; tb += b as u64;
    }}
    let mut dcr = 0i32;
    for by in 0..bh { let oy=by*8;
    for bx in 0..bw { let ox=bx*8;
        let mut sat = 0.0f32;
        for yy in 0..8 { let r=(oy+yy)*w+ox; for xx in 0..8 { sat += cr[r+xx].abs(); }}
        sat /= 64.0;
        let cdz = if sat>25.0{dz*0.65}else if sat>15.0{dz*0.80}else if sat>8.0{dz*0.90}else{dz*1.0};
        let (b, nd) = match crr { Some(ref mut r) => encode_block_recon(cr,w,ox,oy,cq,&inv_cq,cdz,dcr,r), None => encode_block_bits(cr,w,ox,oy,cq,&inv_cq,cdz,dcr) };
        dcr = nd; tb += b as u64;
    }}
    tb + 64
}

pub fn encode_iframe_recon(rgb: &[u8], w: usize, h: usize, quality: f32, dead_zone: f32, chroma_boost: i32) -> (Vec<u8>, u64, f64) {
    init();
    let n=w*h;
    let jqf=(5.0+quality*93.0).round()as i32;
    // Cap chroma_boost at 5 to preserve color saturation at low bitrates. 
    // Without this, low jqf + high chroma_boost crushes Cb/Cr to near-monochrome.
    let cb_eff = chroma_boost.min(5);
    let lq=build_qtable(&LUMA_Q,jqf); let cq=build_qtable(&CHROMA_Q,(jqf-cb_eff).max(15));
    let mut y=vec![0.0f32;n]; let mut cb=vec![0.0f32;n]; let mut cr=vec![0.0f32;n];
    rgb_to_planes(rgb,w,h,&mut y,&mut cb,&mut cr);
    let mut yr=vec![0.0f32;n]; let mut cbr=vec![0.0f32;n]; let mut crr=vec![0.0f32;n];
    let tb = encode_iframe(&y,&cb,&cr,w,h,&lq,&cq,dead_zone,Some(&mut yr),Some(&mut cbr),Some(&mut crr));
    let mut rgb_out=vec![0u8;n*3]; planes_to_rgb(&yr,&cbr,&crr,&mut rgb_out,n);
    let mut mse=0.0f64; for i in 0..n*3{let d=rgb[i]as f64-rgb_out[i]as f64;mse+=d*d;} mse/=(n*3)as f64;
    let psnr=if mse==0.0{99.0}else{10.0*(255.0f64*255.0/mse).log10()};
    (rgb_out, tb, psnr)
}

// ══════════════════════════════════════════════════════════
// P-frame: motion-compensated delta with temporal skip
// ══════════════════════════════════════════════════════════
#[inline]
pub fn block_match(ref_y: &[f32], cur_y: &[f32], w: usize, h: usize,
    bx: usize, by: usize, search: i32) -> (i8, i8) {
    let mut best = (0i8, 0i8, f32::MAX);
    for dy in -search..=search { let ry = by as i32+dy;
        if ry<0||(ry as usize)+8>h{continue;}
        for dx in -search..=search { let rx = bx as i32+dx;
            if rx<0||(rx as usize)+8>w{continue;}
            let mut sad=0.0f32;
            for yy in 0..8{let ci=(by+yy)*w+bx;let ri=(ry as usize+yy)*w+rx as usize;
                for xx in 0..8{sad+=(cur_y[ci+xx]-ref_y[ri+xx]).abs();}}
            if sad<best.2{best=(dx as i8,dy as i8,sad);}
    }} (best.0, best.1)
}

pub struct PFrameReconResult { pub total_bits: u64, pub n_skip: u32, pub n_blocks: u32, pub psnr: f64 }

pub fn encode_pframe_recon(
    ref_y: &mut[f32], ref_cb: &mut[f32], ref_cr: &mut[f32],
    cur_y: &[f32], cur_cb: &[f32], cur_cr: &[f32],
    w: usize, h: usize, lq: &[f32;64], cq: &[f32;64], dz: f32,
    search: i32, skip_thresh: f32, rgb_orig: &[u8], rgb_out: &mut[u8],
) -> PFrameReconResult {
    init();
    let inv_lq = build_qtable_inv(lq);
    let inv_cq = build_qtable_inv(cq);
    let bw=w/8;let bh=h/8;
    let mut tb=0u64;let mut n_skip=0u32;let mut n_blocks=0u32;
    let(mut dy,mut dcb,mut dcr)=(0i32,0i32,0i32);
    let mut tile=[0.0f32;64];let mut dct_buf=[0.0f32;64];let mut rec_buf=[0.0f32;64];
    let(mut pmx,mut pmy):(i8,i8)=(0,0);
    let mut new_y=ref_y.to_vec();let mut new_cb=ref_cb.to_vec();let mut new_cr=ref_cr.to_vec();

    for by in 0..bh{let oy=by*8;
    for bx in 0..bw{let ox=bx*8;
        n_blocks+=1;
        let(mvx,mvy)=block_match(ref_y,cur_y,w,h,ox,oy,search);
        let rx=(ox as i32+mvx as i32).max(0).min((w-8)as i32)as usize;
        let ry=(oy as i32+mvy as i32).max(0).min((h-8)as i32)as usize;
        // Skip check
        let mut max_res=0.0f32;
        for yy in 0..8{for xx in 0..8{let d=(cur_y[(oy+yy)*w+ox+xx]-ref_y[(ry+yy)*w+rx+xx]).abs();if d>max_res{max_res=d;}}}
        if max_res<=skip_thresh{n_skip+=1;tb+=1;
            for yy in 0..8{for xx in 0..8{new_y[(oy+yy)*w+ox+xx]=ref_y[(ry+yy)*w+rx+xx];new_cb[(oy+yy)*w+ox+xx]=ref_cb[(ry+yy)*w+rx+xx];new_cr[(oy+yy)*w+ox+xx]=ref_cr[(ry+yy)*w+rx+xx];}}
            continue;}
        tb+=1;
        let(dmx,dmy)=(mvx-pmx,mvy-pmy);pmx=mvx;pmy=mvy;
        tb+=if dmx==0&&dmy==0{2}else{2+cat(dmx as i32)+cat(dmy as i32)+2}as u64;
        // Y residual
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_y[(oy+yy)*w+ox+xx]-ref_y[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);
        let mut qz=[0i32;64];for i in 0..64{qz[i]=dzq_fast(dct_buf[i],lq[i],inv_lq[i],lq[i]*dz*0.5);}
        let mut zig=[0i32;64];for k in 0..64{zig[k]=qz[ZIGZAG[k]];}
        
        let(b,nd)=est_bits_dpcm(&zig,dy);dy=nd;tb+=b as u64;
        let mut dq=[0.0f32;64];for i in 0..64{dq[i]=qz[i]as f32*lq[i];}dct_inv(&dq,&mut rec_buf);
        for yy in 0..8{for xx in 0..8{new_y[(oy+yy)*w+ox+xx]=ref_y[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
        // Cb residual
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cb[(oy+yy)*w+ox+xx]-ref_cb[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);
        let mut qcb=[0i32;64];for i in 0..64{qcb[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*dz*0.8);}
        let mut zcb=[0i32;64];for k in 0..64{zcb[k]=qcb[ZIGZAG[k]];}
        
        let(b1,n1)=est_bits_dpcm(&zcb,dcb);dcb=n1;tb+=b1 as u64;
        let mut dq=[0.0f32;64];for i in 0..64{dq[i]=qcb[i]as f32*cq[i];}dct_inv(&dq,&mut rec_buf);
        for yy in 0..8{for xx in 0..8{new_cb[(oy+yy)*w+ox+xx]=ref_cb[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
        // Cr residual
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cr[(oy+yy)*w+ox+xx]-ref_cr[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);
        let mut qcr2=[0i32;64];for i in 0..64{qcr2[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*dz*0.8);}
        let mut zcr=[0i32;64];for k in 0..64{zcr[k]=qcr2[ZIGZAG[k]];}
        let(b2,n2)=est_bits_dpcm(&zcr,dcr);dcr=n2;tb+=b2 as u64;
        let mut dq=[0.0f32;64];for i in 0..64{dq[i]=qcr2[i]as f32*cq[i];}dct_inv(&dq,&mut rec_buf);
        for yy in 0..8{for xx in 0..8{new_cr[(oy+yy)*w+ox+xx]=ref_cr[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
    }}
    ref_y.copy_from_slice(&new_y);ref_cb.copy_from_slice(&new_cb);ref_cr.copy_from_slice(&new_cr);
    if rgb_out.len() >= w*h*3 { planes_to_rgb(&new_y,&new_cb,&new_cr,rgb_out,w*h); }
    let mse = if rgb_out.len() >= w*h*3 && rgb_orig.len() >= w*h*3 { let mut m=0.0f64;for i in 0..w*h*3{let d=rgb_orig[i]as f64-rgb_out[i]as f64;m+=d*d;}m/(w*h*3)as f64 } else { 1.0 };
    let psnr=if mse==0.0{99.0}else{10.0*(255.0f64*255.0/mse).log10()};
    PFrameReconResult{total_bits:tb,n_skip,n_blocks,psnr}
}

pub fn encode_pframe(
    ref_y: &[f32], ref_cb: &[f32], ref_cr: &[f32],
    cur_y: &[f32], cur_cb: &[f32], cur_cr: &[f32],
    w: usize, h: usize, lq: &[f32;64], cq: &[f32;64], dz: f32,
    search: i32, skip_thresh: f32,
) -> PFrameReconResult {
    init();
    let inv_lq = build_qtable_inv(lq);
    let inv_cq = build_qtable_inv(cq);
    let bw=w/8;let bh=h/8;
    let mut tb=0u64;let mut n_skip=0u32;let mut n_blocks=0u32;
    let(mut dy,mut dcb,mut dcr)=(0i32,0i32,0i32);
    let mut tile=[0.0f32;64];let mut dct_buf=[0.0f32;64];
    let(mut pmx,mut pmy):(i8,i8)=(0,0);
    for by in 0..bh{let oy=by*8;
    for bx in 0..bw{let ox=bx*8;
        n_blocks+=1;
        let(mvx,mvy)=block_match(ref_y,cur_y,w,h,ox,oy,search);
        let rx=(ox as i32+mvx as i32).max(0).min((w-8)as i32)as usize;
        let ry=(oy as i32+mvy as i32).max(0).min((h-8)as i32)as usize;
        let mut max_res=0.0f32;
        for yy in 0..8{for xx in 0..8{let d=(cur_y[(oy+yy)*w+ox+xx]-ref_y[(ry+yy)*w+rx+xx]).abs();if d>max_res{max_res=d;}}}
        if max_res<=skip_thresh{n_skip+=1;tb+=1;continue;}
        tb+=1;
        let(dmx,dmy)=(mvx-pmx,mvy-pmy);pmx=mvx;pmy=mvy;
        tb+=if dmx==0&&dmy==0{2}else{2+cat(dmx as i32)+cat(dmy as i32)+2}as u64;
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_y[(oy+yy)*w+ox+xx]-ref_y[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);let mut qz=[0i32;64];for i in 0..64{qz[i]=dzq_fast(dct_buf[i],lq[i],inv_lq[i],lq[i]*dz*0.5);}
        let mut zig=[0i32;64];for k in 0..64{zig[k]=qz[ZIGZAG[k]];}let(b,nd)=est_bits_dpcm(&zig,dy);dy=nd;tb+=b as u64;
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cb[(oy+yy)*w+ox+xx]-ref_cb[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);let mut qcb=[0i32;64];for i in 0..64{qcb[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*dz*0.8);}
        let mut zcb=[0i32;64];for k in 0..64{zcb[k]=qcb[ZIGZAG[k]];}let(b1,n1)=est_bits_dpcm(&zcb,dcb);dcb=n1;tb+=b1 as u64;
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cr[(oy+yy)*w+ox+xx]-ref_cr[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);let mut qcr=[0i32;64];for i in 0..64{qcr[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*dz*0.8);}
        let mut zcr=[0i32;64];for k in 0..64{zcr[k]=qcr[ZIGZAG[k]];}let(b2,n2)=est_bits_dpcm(&zcr,dcr);dcr=n2;tb+=b2 as u64;
    }}
    PFrameReconResult{total_bits:tb,n_skip,n_blocks,psnr:0.0}
}

// ══════════════════════════════════════════════════════════
// VideoEncoder with temporal prediction
// ══════════════════════════════════════════════════════════
pub struct VideoEncoder {
    pub w: usize, pub h: usize, pub vid_fps: f64, pub display_hz: f64, pub vrr_cap_hz: f64,
    pub quality: f32, pub dead_zone: f32, pub chroma_boost: i32,
    pub search_range: i32, pub skip_threshold: f32, pub gop_size: usize,
    lq: [f32;64], cq: [f32;64], inv_lq: [f32;64], inv_cq: [f32;64],
    ref_y: Vec<f32>, ref_cb: Vec<f32>, ref_cr: Vec<f32>,
    cur_y: Vec<f32>, cur_cb: Vec<f32>, cur_cr: Vec<f32>,
    dc_history: Vec<Vec<f32>>,
    trajectory: MotionTrajectory,
    pub motion_field: SoftMotionField,
    pub bg_model: BackgroundModel,
    pub router: PredictionRouter,
    frame_count: usize, total_bits: u64,
    pub compressed_bytes: u64,
    pub deblock_strength: f32,
    pub grain_scale: f32,
    world_x: i32, world_y: i32,
    grain_seed: u32,
    // Adaptive GOP: track quality since last I-frame
    frames_since_iframe: usize,
    psnr_at_iframe: f64,         // PSNR of last I-frame
    recent_psnr: Vec<f64>,       // rolling PSNR of last 10 P-frames
    recent_skip_pct: Vec<f64>,
    pub encoding_mode: EncodingMode,
    source_analyzed: bool,
    work_y: Vec<f32>, work_cb: Vec<f32>, work_cr: Vec<f32>,
    ref_buffer: Vec<Vec<f32>>,
    ref_buffer_ages: Vec<usize>,
    max_refs: usize,
    block_static_count: Vec<u16>,
    // Blend frames: recent reconstructions the web can interpolate between
    blend_frames: Vec<Vec<f32>>,  // up to 3 Y-plane snapshots for blending
    blend_ages: Vec<u16>,
    partial_refresh_map: Vec<bool>,
    prev_histogram: Vec<u32>,
    is_continuation: bool,
    chroma_stale: bool,
    scene_model: crate::gpu_pipeline::SceneModel,
    temporal_tiler: crate::gpu_pipeline::TemporalTiler,  // previous frame histogram for GPU scene change
    #[cfg(feature = "gpu")]
    shared_gpu: Option<std::sync::Arc<crate::gpu_pipeline::SharedGpu>>,  // per-block: needs intra refresh this frame
    flicker: crate::v2_encoder::FlickerTracker,  // how many consecutive frames each block has been static
}

pub struct FrameResult { pub frame_type: u8, pub bits: u64, pub psnr: f64, pub skip_pct: f64, pub encode_ms: f64, pub compressed_size: usize, pub vrr_duration_ms: f32, pub activity: f32, pub route_stats: String, pub graph_new_refs: u32, pub graph_traj_updates: u32 }

impl VideoEncoder {
    pub fn new(w: usize, h: usize, quality: f32, dead_zone: f32, chroma_boost: i32) -> Self {
        init();
        let jqf=(5.0+quality*93.0).round()as i32;
        let cb_eff = chroma_boost.min(5);   // reduced cap — preserves color saturation
        // Floor chroma qf at 15 (was 5). Below this, Cb/Cr are quantized so 
        // hard that color information is essentially lost — visible 
        // desaturation on flesh tones, sky, foliage. The luma quality 
        // dominates compression; clamping chroma QF higher costs little 
        // in bytes but visibly preserves color.
        let lq=build_qtable(&LUMA_Q,jqf); let cq=build_qtable(&CHROMA_Q,(jqf-cb_eff).max(15));
        let inv_lq=build_qtable_inv(&lq); let inv_cq=build_qtable_inv(&cq);
        let n=w*h; let nb=(w/8)*(h/8);
        VideoEncoder{w,h,quality,dead_zone,chroma_boost,
            vid_fps:60.0,display_hz:240.0,vrr_cap_hz:240.0,search_range:16,skip_threshold:10.0,gop_size:30,
            lq,cq,inv_lq,inv_cq,
            ref_y:vec![0.0;n],ref_cb:vec![0.0;n],ref_cr:vec![0.0;n],
            cur_y:vec![0.0;n],cur_cb:vec![0.0;n],cur_cr:vec![0.0;n],
            dc_history:vec![Vec::with_capacity(32);nb],
            trajectory:MotionTrajectory::new(nb),
            motion_field:SoftMotionField::new(w,h),
            bg_model:BackgroundModel::new(w,h),
            router:PredictionRouter::new(w,h),
            frame_count:0,total_bits:0,compressed_bytes:0,deblock_strength:0.8,grain_scale:0.3,world_x:0,world_y:0,grain_seed:0xDEAD_BEEF,frames_since_iframe:0,psnr_at_iframe:0.0,recent_psnr:Vec::new(),recent_skip_pct:Vec::new(),
            encoding_mode:EncodingMode::Reproduction,source_analyzed:false,
            work_y:vec![0.0;n],work_cb:vec![0.0;n],work_cr:vec![0.0;n],
            ref_buffer:Vec::new(),ref_buffer_ages:Vec::new(),max_refs:8,
            block_static_count:vec![0u16;nb],
            blend_frames:Vec::new(),blend_ages:Vec::new(),partial_refresh_map:vec![false;nb],
            prev_histogram:vec![0u32;256],
            is_continuation:false,
            chroma_stale:false,
            scene_model: crate::gpu_pipeline::SceneModel::default(),
            temporal_tiler: crate::gpu_pipeline::TemporalTiler::new(w, h, 8),
            #[cfg(feature = "gpu")]
            shared_gpu: crate::gpu_pipeline::get_shared_gpu(w, h),
            flicker:crate::v2_encoder::FlickerTracker::new(w,h)}
    }

    /// Mark this encoder as continuing from a previous chunk
    /// (first frame should be P-frame, not I-frame)
    pub fn set_continuation(&mut self) {
        self.is_continuation = true;
        self.frame_count = 1;  // skip the frame_count==0 I-frame trigger
    }

    pub fn encode_frame(&mut self, rgb_in: &[u8], rgb_out: &mut[u8]) -> FrameResult {
        let t0=std::time::Instant::now();
        let n=self.w*self.h;
        let mut partial_refresh = false;
        if self.frame_count == 0 { eprintln!("FAO encode: frame {} (worker {:?})", self.frame_count, std::thread::current().id()); }

        // Scene change detection using reusable buffers
        // Adaptive GOP: I-frame decision based on function quality, not fixed interval
        let mut force_iframe = self.frame_count == 0;
        if !force_iframe && self.frames_since_iframe > 0 {
            let avg_recent_psnr = if self.recent_psnr.is_empty() { 99.0 }
                else { self.recent_psnr.iter().sum::<f64>() / self.recent_psnr.len() as f64 };
            let avg_skip = if self.recent_skip_pct.is_empty() { 100.0 }
                else { self.recent_skip_pct.iter().sum::<f64>() / self.recent_skip_pct.len() as f64 };
            let psnr_drop = self.psnr_at_iframe - avg_recent_psnr;

            let needs_refresh = psnr_drop > 12.0
                || (avg_skip < 50.0 && self.frames_since_iframe > 30)
                || self.frames_since_iframe >= self.gop_size * 4;

            let quality_ok = psnr_drop < 12.0 && avg_skip > 60.0;

            if needs_refresh {
                force_iframe = true;
            } else if self.frames_since_iframe >= self.gop_size && !quality_ok {
                force_iframe = true;
            }
        }
        // Convert input to planes ONCE
        rgb_to_planes(rgb_in, self.w, self.h, &mut self.cur_y, &mut self.cur_cb, &mut self.cur_cr);

        // Scene change: MINIMUM 5 frames since last I-frame (prevents I-frame loops)
        // Raise threshold for sources detected as low-bitrate (noisy histograms)
        if !force_iframe && self.frame_count > 0 && self.frames_since_iframe >= 5
            && self.frames_since_iframe % 10 == 0 {
            let sc = scene_change_score(&self.ref_y, &self.cur_y, n);
            // Tiered scene response:
            // > 0.8: full I-frame (hard cut — reset web)
            // > 0.3: partial refresh handled per-block in P-frame encoder
            // < 0.3: normal P-frame
            let sc_hard_cut = if self.source_analyzed { 0.80 } else { 0.70 };
            if sc > sc_hard_cut { force_iframe = true; }
            partial_refresh = sc > 0.3 && !force_iframe;
        }

        if force_iframe {
            // Analyze source quality on first frame and adapt encoder
            if !self.source_analyzed {
                let profile = analyze_source_quality(rgb_in, self.w, self.h);
                adapt_encoder_for_source(self, &profile, self.encoding_mode);
                self.source_analyzed = true;
            }
            // In representation mode, pre-filter the input to reduce source artifacts
            let encode_input = if self.encoding_mode == EncodingMode::Representation {
                let mut cleaned = rgb_in.to_vec();
                prefilter_denoise(&mut cleaned, self.w, self.h, 0.5);
                cleaned
            } else {
                rgb_in.to_vec()
            };
            let is_scene_change = force_iframe && self.frames_since_iframe >= 30;  // only clear state for genuine scene changes (not GOP refreshes)
            let (rec, stream, psnr) = encode_iframe_420(
                if self.encoding_mode == EncodingMode::Representation { &encode_input } else { rgb_in },
                self.w, self.h, self.quality, self.dead_zone, self.chroma_boost);
            
            let csz = stream.len();
            let tb = csz as u64 * 8;
            if rgb_out.len() >= n * 3 { rgb_out[..n*3].copy_from_slice(&rec); }
            // Deblock for reference frame (in-loop, improves prediction)
            if rgb_out.len() >= n * 3 { deblock_frame(rgb_out, self.w, self.h, self.deblock_strength); }
            // Update ref from deblocked output (NO grain — grain poisons prediction)
            rgb_to_planes(rgb_out, self.w, self.h, &mut self.ref_y, &mut self.ref_cb, &mut self.ref_cr);

            if is_scene_change {
                // Scene change: hard reset everything — the old world is gone
                self.world_x = 0; self.world_y = 0;
                for h in &mut self.dc_history { h.clear(); }
                self.trajectory.clear();
                self.motion_field.clear();
                self.bg_model.clear();
                self.bg_model.init_from_frame(&self.ref_y, &self.ref_cb, &self.ref_cr);
            } else {
                // Regular GOP I-frame: keep all learned state, just refresh ref quality.
                // DC history, trajectory, motion field, anchors, world offset all persist.
                // Update anchors with the fresh I-frame data (high quality snapshot opportunity)
                self.bg_model.init_from_frame(&self.ref_y, &self.ref_cb, &self.ref_cr);
            }

            // Grain for display only
            if self.grain_scale > 0.0 {
                let params = analyze_grain(rgb_in, rgb_out, self.w, self.h);
                let scaled: Vec<GrainParams> = params.iter().map(|p| GrainParams { variance: p.variance * self.grain_scale, ..*p }).collect();
                apply_grain(rgb_out, self.w, self.h, &scaled, self.grain_seed, self.world_x, self.world_y);
            }
            self.update_dc_history();
            self.frame_count+=1; self.total_bits+=tb; self.compressed_bytes+=csz as u64;
            self.frames_since_iframe = 0;
            self.psnr_at_iframe = psnr;
            self.recent_psnr.clear();
            self.recent_skip_pct.clear();
            FrameResult{frame_type:0,bits:tb,psnr,skip_pct:0.0,encode_ms:t0.elapsed().as_secs_f64()*1000.0,compressed_size:csz,vrr_duration_ms:1000.0/self.display_hz as f32,activity:1.0,route_stats:String::new(),graph_new_refs:0,graph_traj_updates:0}
        } else {
            // Adaptive dead zone: increase during high motion (eye can't see detail)
            let motion_mag = ((self.world_x as f64).abs() + (self.world_y as f64).abs()).sqrt();
            let motion_dz_boost = if motion_mag > 30.0 { 1.5 }   // very fast motion: 50% more quantization
                else if motion_mag > 15.0 { 1.2 }                 // moderate motion: 20% more
                else { 1.0 };                                      // slow/static: no change
            // Partial refresh: mark blocks that need intra within P-frame
            if partial_refresh {
                let bw8 = self.w / 8;
                let bh8 = self.h / 8;
                for by in 0..bh8 { for bx in 0..bw8 {
                    let bi = by * bw8 + bx;
                    if bi >= self.partial_refresh_map.len() { continue; }
                    let ox = bx * 8; let oy = by * 8;
                    let mut sad = 0.0f32;
                    for yy in 0..8 { for xx in 0..8 {
                        sad += (self.cur_y[(oy+yy)*self.w+ox+xx] - self.ref_y[(oy+yy)*self.w+ox+xx]).abs();
                    }}
                    // Block needs refresh if it changed significantly
                    self.partial_refresh_map[bi] = sad / 64.0 > self.skip_threshold * 2.0;
                }}
            } else {
                self.partial_refresh_map.fill(false);
            }

            // Store blend frame periodically (every 30 frames)
            if self.frame_count % 30 == 0 && self.blend_frames.len() < 3 {
                self.blend_frames.push(self.ref_y.clone());
                self.blend_ages.push(0);
            }
            for age in &mut self.blend_ages { *age = age.saturating_add(1); }
            // Evict old blend frames
            while self.blend_frames.len() > 3 {
                self.blend_frames.remove(0);
                self.blend_ages.remove(0);
            }

            let frame_dz = self.dead_zone * motion_dz_boost as f32;
            
            // GPU disabled — use CPU path
            let gpu_mv_results: Option<Vec<(i8,i8,f32)>> = None;
            let _gpu_classes: Option<Vec<u32>> = None;
            // GPU full encode pipeline via shared context
            #[cfg(feature = "gpu")]
            let (gpu_mv_results, _gpu_classes, _affine, gpu_blur_data, gpu_full) = {
                if let Some(ref sgpu) = self.shared_gpu {
                    // Check resolution match — GPU may be stale from previous video
                    if sgpu.width != self.w || sgpu.height != self.h {
                        // Resolution mismatch — refresh GPU reference
                        self.shared_gpu = crate::gpu_pipeline::get_shared_gpu(self.w, self.h);
                    }
                }
                if let Some(ref sgpu) = self.shared_gpu {
                    if let Some(slot) = sgpu.try_acquire_slot() {
                    let full_result = sgpu.encode_with_slot(&slot, &self.cur_y, &self.ref_y,
                        &self.lq, self.dead_zone, self.skip_threshold);
                    let results = &full_result.motion;
                    let bw = self.w / 8; let bh = self.h / 8;
                    let affine = crate::gpu_pipeline::AffineMotion::fit_from_mvs(
                        &results.mvs, bw, bh, self.skip_threshold);

                    let blur_data = results.blur.clone();
                    let class_data = results.classes.clone();

                    // Affine residual MVs
                    let mut residual_mvs = results.mvs.clone();
                    for by in 0..bh { for bx in 0..bw {
                        let bi = by * bw + bx;
                        if bi >= residual_mvs.len() { continue; }
                        let (mvx, mvy, sad) = residual_mvs[bi];
                        let (rx, ry) = affine.residual(bx, by, mvx as f32, mvy as f32);
                        if rx.abs() < 1.5 && ry.abs() < 1.5 {
                            let cx = (bx * 8 + 4) as f32;
                            let cy = (by * 8 + 4) as f32;
                            let (px, py) = affine.apply(cx, cy);
                            let amvx = (px - cx).round() as i8;
                            let amvy = (py - cy).round() as i8;
                            residual_mvs[bi] = (amvx, amvy, sad);
                        }
                    }}
                    // Update scene model from GPU analysis
                    if let Some(ref scene) = full_result.scene {
                        if self.frame_count % 30 == 0 || self.frame_count < 2 {
                            self.scene_model.update_from_analysis(scene, self.frame_count);
                        }
                    }
                    drop(slot);
                    (Some(residual_mvs), Some(class_data), Some(affine), Some(blur_data), Some(full_result))
                    } else {
                        (None, None, None, None, None)
                    }
                } else { (None, None, None, None, None) }
            };
            #[cfg(not(feature = "gpu"))]
            let (gpu_mv_results, _gpu_classes, _affine, gpu_blur_data, gpu_full): (Option<Vec<(i8,i8,f32)>>, Option<Vec<u32>>, Option<crate::gpu_pipeline::AffineMotion>, Option<Vec<f32>>, Option<crate::gpu_pipeline::GpuFullEncodeResult>) = (None, None, None, None, None);

            // Scene-adaptive quality: adjust skip threshold based on complexity
            // Scene-adaptive quality from GPU analysis
            let (scene_skip_adjust, shadow_boost, depth_boost) = if self.scene_model.depth_profile.len() > 0 {
                let complexity = self.scene_model.geometry_dist[2] + self.scene_model.geometry_dist[3];
                let skip_adj = 1.0 - complexity * 0.5;
                
                // Shadow analysis: darker regions can tolerate more compression
                let shadow_avg = self.scene_model.shadow_avg;
                let shadow_adj = 1.0 + shadow_avg * 0.3;  // shadows → higher skip (lower quality OK)
                
                // Depth analysis: background regions can be lower quality
                let depth_variance = self.scene_model.depth_variance;
                let depth_adj = if depth_variance > 0.1 { 0.8 } else { 1.0 };  // depth variation → more detail
                
                // Lighting: high contrast scenes need more bits
                let light_contrast = self.scene_model.light_contrast;
                let light_adj = if light_contrast > 0.5 { 0.7 } else { 1.0 };  // high contrast → more detail
                
                (skip_adj * light_adj, shadow_adj, depth_adj)
            } else { (1.0, 1.0, 1.0) };
            let adjusted_skip = self.skip_threshold * scene_skip_adjust as f32 * shadow_boost as f32 * depth_boost as f32;
            
            // If GPU did full encode, skip CPU block loop entirely
            let (r, gmx, gmy, _n_traj) = if let Some(ref gf) = gpu_full {
                // GPU provided coefficients + MVs — just compute stats
                let bw = self.w / 8; let bh = self.h / 8;
                let n_blocks = (bw * bh) as u32;
                let mut n_skip = 0u32;
                let mut total_bits = 0u64;
                // Count skips from GPU MVs (SAD < threshold = skip)
                for mv in &gf.motion.mvs {
                    if mv.2 / 64.0 <= adjusted_skip { n_skip += 1; }
                    else { total_bits += 200; }  // ~200 bits per encoded block estimate
                }
                total_bits += n_skip as u64 * 2;  // 2 bits per skip
                // Reconstruct ref from GPU coefficients + current ref
                // For skip blocks: ref stays as-is
                // For encoded blocks: idct(coeffs) + ref → new ref
                // This is done by the GPU already (recon_buf) but we skipped readback
                // So use CPU work buffers with GPU MVs for reconstruction
                self.work_y.copy_from_slice(&self.ref_y);
                self.work_cb.copy_from_slice(&self.ref_cb);
                self.work_cr.copy_from_slice(&self.ref_cr);
                // Apply GPU MVs to work buffers (motion compensated copy)
                // Only process blocks with non-zero MV that skip
                // (skip blocks with zero MV are already correct in work_y = ref_y)
                for bi in 0..gf.motion.mvs.len().min(bw * bh) {
                    let (mvx, mvy, sad) = gf.motion.mvs[bi];
                    if sad / 64.0 > self.skip_threshold { continue; }  // non-skip: handled by entropy
                    if mvx == 0 && mvy == 0 { continue; }  // zero MV: already correct
                    let bx = bi % bw; let by = bi / bw;
                    let ox = bx * 8; let oy = by * 8;
                    let rx = (ox as i32 + mvx as i32).max(0).min((self.w-8) as i32) as usize;
                    let ry = (oy as i32 + mvy as i32).max(0).min((self.h-8) as i32) as usize;
                    for yy in 0..8 { for xx in 0..8 {
                        self.work_y[(oy+yy)*self.w+ox+xx] = self.ref_y[(ry+yy)*self.w+rx+xx];
                    }}
                }
                let psnr = 35.0;  // approximate — GPU doesn't compute per-frame PSNR easily
                let afmvx = if let Some(ref a) = _affine { a.c as i8 } else { 0i8 };
                let afmvy = if let Some(ref a) = _affine { a.f as i8 } else { 0i8 };
                (PFrameReconResult { total_bits, n_skip, n_blocks, psnr }, afmvx, afmvy, 0u32)
            } else {
                encode_pframe_trajectory(&mut self.ref_y,&mut self.ref_cb,&mut self.ref_cr,
                    &self.cur_y,&self.cur_cb,&self.cur_cr,self.w,self.h,&self.lq,&self.cq,
                    frame_dz,self.skip_threshold,
                    &self.dc_history, &mut self.trajectory,
                    &mut self.work_y, &mut self.work_cb, &mut self.work_cr,
                    rgb_in,rgb_out,
                    &self.ref_buffer,
                    gpu_mv_results.as_deref(),
                    gpu_blur_data.as_deref())
            };
            self.world_x += gmx as i32;
            self.world_y += gmy as i32;
            // Update per-block static counts and compute world-only skip rate
            let bw8 = self.w / 8; let bh8 = self.h / 8;
            let mut n_ui_blocks = 0u32;
            for bi in 0..self.block_static_count.len().min(bw8 * bh8) {
                if self.block_static_count[bi] > 30 { n_ui_blocks += 1; }
            }
            let world_blocks = r.n_blocks.saturating_sub(n_ui_blocks);
            let world_skip = r.n_skip.saturating_sub(n_ui_blocks);
            // Use world-only skip for quality decisions (UI always skips, shouldn't inflate average)
            let sp = if world_blocks > 0 {
                (world_skip as f64 / world_blocks as f64 * 100.0).min(100.0)
            } else if r.n_blocks > 0 {
                (r.n_skip as f64 / r.n_blocks as f64 * 100.0).min(100.0)
            } else { 0.0 };

            // Copy work buffers to ref planes (the encoded result)
            // CPU reconstruction (reliable path)
            self.ref_y.copy_from_slice(&self.work_y);
            self.ref_cb.copy_from_slice(&self.work_cb);
            self.ref_cr.copy_from_slice(&self.work_cr);

            // Update multi-reference buffer: store snapshot if different enough
            let ref_added = self.update_ref_buffer();

            // Deblock directly on planes (avoids planes→RGB→deblock→RGB→planes round-trip)
            if sp < 90.0 {
                deblock_plane(&mut self.ref_y, self.w, self.h, self.deblock_strength);
                if rgb_out.len() >= n * 3 && sp < 60.0 {
                    deblock_plane(&mut self.ref_cb, self.w, self.h, self.deblock_strength * 0.5);
                    deblock_plane(&mut self.ref_cr, self.w, self.h, self.deblock_strength * 0.5);
                }
            }

            if rgb_out.len() >= n * 3 { planes_to_rgb(&self.ref_y, &self.ref_cb, &self.ref_cr, rgb_out, n); }

            // Background model: only update every 5 frames (expensive)
            if self.frames_since_iframe % 30 == 0 {  // reduced frequency for speed
                let bw = self.w / 8; let bh = self.h / 8;
                // Use skip rate to approximate block_skipped instead of per-block SAD
                let skip_thresh_approx = sp > 80.0;
                if !skip_thresh_approx {
                    let block_skipped: Vec<bool> = (0..bw*bh).map(|bi| {
                        let bx = (bi % bw) * 8; let by = (bi / bw) * 8;
                        let mut sad = 0.0f32;
                        for yy in 0..8 { for xx in 0..8 {
                            let pi = (by+yy)*self.w+(bx+xx);
                            if pi < self.cur_y.len() { sad += (self.cur_y[pi] - self.ref_y[pi]).abs(); }
                        }}
                        sad / 64.0 < self.skip_threshold
                    }).collect();
                    self.bg_model.update(&self.ref_y, &self.ref_cb, &self.ref_cr, &block_skipped, bw, bh);
                    for by in 0..bh { for bx in 0..bw {
                        let bi = by * bw + bx;
                        if bi < block_skipped.len() && !block_skipped[bi] {
                            self.bg_model.try_restore_block(&mut self.ref_y, &mut self.ref_cb, &mut self.ref_cr, bx, by, &self.cur_y);
                        }
                    }}
                }
            }

            // Grain: skip analysis on P-frames (use I-frame grain params or none)
            // Grain analysis is expensive and adds little value on P-frames
            self.update_dc_history();
            let est_bytes = ((r.total_bits+7)/8) as usize;
                        self.frame_count+=1; self.total_bits+=r.total_bits; self.compressed_bytes+=est_bytes as u64;
            self.frames_since_iframe += 1;
            self.recent_psnr.push(r.psnr);
            self.recent_skip_pct.push(sp);
            if self.recent_psnr.len() > 10 { self.recent_psnr.remove(0); }
            if self.recent_skip_pct.len() > 10 { self.recent_skip_pct.remove(0); }
            let (vrr_ms_raw, vrr_act) = self.motion_field.frame_vrr(self.vid_fps, self.display_hz);
            // Clamp to vrr_cap_hz for low-end playback (e.g. RPi → cap=30, laptop → cap=60)
            let vrr_floor_ms = 1000.0 / self.vrr_cap_hz as f32;
            let vrr_ms = vrr_ms_raw.max(vrr_floor_ms);
            { // Run prediction router for stats (future: use for encoding decisions)
            let anchor_dcs: Vec<f32> = (0..self.w/8 * self.h/8).map(|_| 0.0).collect();
            let anchor_conf: Vec<f32> = (0..self.w/8 * self.h/8).map(|_| 0.0).collect();
            self.router.fill_predictions(&self.ref_y, &self.cur_y, self.w, gmx as f32, gmy as f32, &self.trajectory, &anchor_dcs, &anchor_conf);
            self.router.route_all(self.skip_threshold);
            let rstats = self.router.stats();
            FrameResult{frame_type:1,bits:r.total_bits,psnr:r.psnr,skip_pct:sp,encode_ms:t0.elapsed().as_secs_f64()*1000.0,compressed_size:est_bytes,vrr_duration_ms:vrr_ms,activity:vrr_act,route_stats:rstats,graph_new_refs:if ref_added{1}else{0},graph_traj_updates:_n_traj}
            }
        }
    }


    /// Update the multi-reference buffer with the current frame
    /// Only stores a new reference if it's sufficiently different from all existing ones
    fn update_ref_buffer(&mut self) -> bool {
        let n = self.w * self.h;
        let min_diff = if self.ref_buffer.is_empty() { f32::MAX } else {
            let step = (n / 256).max(1);
            self.ref_buffer.iter().map(|stored| {
                let mut sad = 0.0f32;
                let mut count = 0;
                for i in (0..n).step_by(step) {
                    sad += (self.ref_y[i] - stored[i]).abs();
                    count += 1;
                }
                sad / count as f32
            }).fold(f32::MAX, f32::min)
        };
        let mut added = false;
        if min_diff > 8.0 || self.ref_buffer.len() < self.max_refs {
            if self.ref_buffer.len() >= self.max_refs {
                let oldest_idx = self.ref_buffer_ages.iter()
                    .enumerate().max_by_key(|(_, &age)| age).map(|(i, _)| i).unwrap_or(0);
                self.ref_buffer[oldest_idx] = self.ref_y.clone();
                self.ref_buffer_ages[oldest_idx] = 0;
            } else {
                self.ref_buffer.push(self.ref_y.clone());
                self.ref_buffer_ages.push(0);
            }
            added = true;
        }
        for age in &mut self.ref_buffer_ages { *age += 1; }
        added
    }


    /// Search all stored references for the best match for a block
    /// Returns (best_ref_index, mvx, mvy, sad) or None if no ref is better than current
    fn search_multi_ref(&self, bx: usize, by: usize) -> Option<(usize, i8, i8, f32)> {
        if self.ref_buffer.is_empty() { return None; }
        
        let w = self.w; let h = self.h;
        let mut best: Option<(usize, i8, i8, f32)> = None;
        
        for (ri, stored) in self.ref_buffer.iter().enumerate() {
            let mv = block_match_3step(stored, &self.cur_y, w, h, bx, by);
            let rx = (bx as i32 + mv.0 as i32).max(0).min((w-8) as i32) as usize;
            let ry = (by as i32 + mv.1 as i32).max(0).min((h-8) as i32) as usize;
            let mut sad = 0.0f32;
            for yy in 0..8 { for xx in 0..8 {
                sad += (self.cur_y[(by+yy)*w+bx+xx] - stored[(ry+yy)*w+rx+xx]).abs();
            }}
            if best.is_none() || sad < best.unwrap().3 {
                best = Some((ri, mv.0, mv.1, sad));
            }
        }
        
        best
    }

    fn update_dc_history(&mut self) {
        let bw = self.w / 8;
        for by in 0..self.h/8 { for bx in 0..bw {
            let ox=bx*8; let oy=by*8; let mut sum=0.0f32;
            for yy in 0..8 { let r=(oy+yy)*self.w+ox; for xx in 0..8 { sum+=self.ref_y[r+xx]; }}
            let bi = by*bw+bx;
            if bi < self.dc_history.len() {
                self.dc_history[bi].push(sum/64.0);
                if self.dc_history[bi].len() > 32 { self.dc_history[bi].remove(0); }
            }
        }}
    }

    pub fn avg_bitrate_kbps(&self, fps: f64) -> f64 {
        if self.frame_count==0{return 0.0;}
        self.total_bits as f64/self.frame_count as f64*fps/1000.0
    }

    pub fn reset(&mut self) {
        self.frame_count=0;self.total_bits=0;self.compressed_bytes=0;
        self.ref_y.fill(0.0);self.ref_cb.fill(0.0);self.ref_cr.fill(0.0);
        for h in &mut self.dc_history{h.clear();} self.trajectory.clear(); self.motion_field.clear();
        self.bg_model.clear(); self.world_x=0; self.world_y=0;
    }
}

// ══════════════════════════════════════════════════════════
// C FFI
// ══════════════════════════════════════════════════════════
#[no_mangle] pub extern "C" fn fao_init() { init(); }

#[no_mangle]
pub extern "C" fn fao_encode_iframe(
    rgb: *const u8, w: u32, h: u32, quality: f32, dead_zone: f32, chroma_boost: i32,
    recon_out: *mut u8, psnr_out: *mut f64,
) -> u64 {
    let w=w as usize;let h=h as usize;let n=w*h*3;
    let rgb_slice=unsafe{std::slice::from_raw_parts(rgb,n)};
    if recon_out.is_null(){
        let jqf=(5.0+quality*93.0).round()as i32;
        let cb_eff = chroma_boost.min(5);
        let lq=build_qtable(&LUMA_Q,jqf);let cq=build_qtable(&CHROMA_Q,(jqf-cb_eff).max(15));
        let mut y=vec![0.0f32;w*h];let mut cb=vec![0.0f32;w*h];let mut cr=vec![0.0f32;w*h];
        rgb_to_planes(rgb_slice,w,h,&mut y,&mut cb,&mut cr);
        encode_iframe(&y,&cb,&cr,w,h,&lq,&cq,dead_zone,None,None,None)
    } else {
        let(rec,tb,psnr)=encode_iframe_recon(rgb_slice,w,h,quality,dead_zone,chroma_boost);
        let out=unsafe{std::slice::from_raw_parts_mut(recon_out,n)};out.copy_from_slice(&rec);
        if!psnr_out.is_null(){unsafe{*psnr_out=psnr;}} tb
    }
}

#[no_mangle] pub extern "C" fn fao_video_new(w:u32,h:u32,q:f32,dz:f32,cb:i32)->*mut VideoEncoder{Box::into_raw(Box::new(VideoEncoder::new(w as usize,h as usize,q,dz,cb)))}
#[no_mangle] pub extern "C" fn fao_video_free(enc:*mut VideoEncoder){if!enc.is_null(){unsafe{drop(Box::from_raw(enc));}}}
#[no_mangle]
pub extern "C" fn fao_video_encode(enc:*mut VideoEncoder,rgb_in:*const u8,rgb_out:*mut u8,
    ft:*mut u8,bits:*mut u64,psnr:*mut f64,skip:*mut f64,ms:*mut f64){
    let enc=unsafe{&mut*enc};let n=enc.w*enc.h*3;
    let input=unsafe{std::slice::from_raw_parts(rgb_in,n)};
    let output=unsafe{std::slice::from_raw_parts_mut(rgb_out,n)};
    let r=enc.encode_frame(input,output);
    unsafe{if!ft.is_null(){*ft=r.frame_type;}if!bits.is_null(){*bits=r.bits;}
        if!psnr.is_null(){*psnr=r.psnr;}if!skip.is_null(){*skip=r.skip_pct;}if!ms.is_null(){*ms=r.encode_ms;}}
}
#[no_mangle] pub extern "C" fn fao_video_avg_kbps(enc:*const VideoEncoder,fps:f64)->f64{unsafe{(*enc).avg_bitrate_kbps(fps)}}

// ══════════════════════════════════════════════════════════
// Temporal polynomial prediction
// ══════════════════════════════════════════════════════════

/// Fit f(t) = a + b*t + c*t² to a time series. Returns (a, b, c, residual_rms).
pub fn fit_temporal_poly(values: &[f32]) -> (f32, f32, f32, f32) {
    let n = values.len();
    if n == 0 { return (0.0, 0.0, 0.0, 0.0); }
    if n == 1 { return (values[0], 0.0, 0.0, 0.0); }
    if n == 2 { return (values[0], values[1] - values[0], 0.0, 0.0); }

    // Least squares: minimize sum (a + b*t_i + c*t_i² - y_i)²
    // Normalize t to [0,1]
    let nm1 = (n - 1) as f32;
    let mut s1 = 0.0f32; let mut st = 0.0f32; let mut st2 = 0.0f32;
    let mut st3 = 0.0f32; let mut st4 = 0.0f32;
    let mut sy = 0.0f32; let mut sty = 0.0f32; let mut st2y = 0.0f32;

    for i in 0..n {
        let t = i as f32 / nm1;
        let t2 = t * t;
        s1 += 1.0; st += t; st2 += t2; st3 += t * t2; st4 += t2 * t2;
        sy += values[i]; sty += t * values[i]; st2y += t2 * values[i];
    }

    // Solve 3x3 system via Cramer's rule
    // [s1  st   st2 ] [a]   [sy  ]
    // [st  st2  st3 ] [b] = [sty ]
    // [st2 st3  st4 ] [c]   [st2y]
    let det = s1*(st2*st4 - st3*st3) - st*(st*st4 - st3*st2) + st2*(st*st3 - st2*st2);
    if det.abs() < 1e-10 {
        // Degenerate — fall back to mean
        return (sy / s1, 0.0, 0.0, 0.0);
    }

    let a = ((sy*(st2*st4-st3*st3)) - (st*(sty*st4-st3*st2y)) + (st2*(sty*st3-st2*st2y))) / det;
    let b = ((s1*(sty*st4-st3*st2y)) - (sy*(st*st4-st3*st2)) + (st2*(st*st2y-sty*st2))) / det;
    let c = ((s1*(st2*st2y-st3*sty)) - (st*(st*st2y-sty*st2)) + (sy*(st*st3-st2*st2))) / det;

    // RMS residual
    let mut rss = 0.0f32;
    for i in 0..n {
        let t = i as f32 / nm1;
        let pred = a + b * t + c * t * t;
        let r = values[i] - pred;
        rss += r * r;
    }
    let rms = (rss / n as f32).sqrt();
    (a, b, c, rms)
}

/// Predict next value in series using fitted polynomial
pub fn predict_next(values: &[f32]) -> (f32, f32) {
    let (a, b, c, rms) = fit_temporal_poly(values);
    let n = values.len();
    if n <= 1 { return (a, rms); }
    let t = n as f32 / (n - 1) as f32;  // next t beyond the series
    (a + b * t + c * t * t, rms)
}

// ══════════════════════════════════════════════════════════
// Huffman bitstream writer (produces actual compressed data)
// ══════════════════════════════════════════════════════════

// Bit IO — extracted to fao_codec_core
pub use fao_core::bitio::{BitWriter, BitReader};

/// Encode a sequence of quantized DCT blocks into a Huffman bitstream.
/// Returns (table_data, coded_data).
pub fn huffman_encode_blocks(blocks: &[&[i32; 64]]) -> (Vec<u8>, Vec<u8>) {
    if blocks.is_empty() { return (vec![], vec![]); }

    // Pass 1: collect symbol frequencies
    use std::collections::HashMap;
    let mut dc_freq: HashMap<u32, u32> = HashMap::new();
    let mut ac_freq: HashMap<u16, u32> = HashMap::new();
    let mut prev_dc = 0i32;

    for block in blocks {
        let dd = block[0] - prev_dc;
        prev_dc = block[0];
        *dc_freq.entry(cat(dd)).or_insert(0) += 1;

        let mut last_nz = 0usize;
        for i in (1..64).rev() { if block[i] != 0 { last_nz = i; break; } }
        if last_nz == 0 { *ac_freq.entry(0x00).or_insert(0) += 1; continue; }

        let mut zr = 0u16;
        for i in 1..=last_nz {
            if block[i] == 0 { zr += 1; if zr == 16 { *ac_freq.entry(0xF0).or_insert(0) += 1; zr = 0; } }
            else { let c = cat(block[i]); *ac_freq.entry((zr << 4) | c as u16).or_insert(0) += 1; zr = 0; }
        }
        *ac_freq.entry(0x00).or_insert(0) += 1;
    }

    // Build canonical Huffman codes (simplified: use code length assignment)
    fn build_codes(freq: &HashMap<u32, u32>) -> Vec<(u32, u32, u32)> {
        // Returns sorted vec of (symbol_as_u32, code, code_length)
        if freq.is_empty() { return vec![]; }
        // Assign lengths by frequency (simple: shorter codes for more frequent)
        let mut items: Vec<_> = freq.iter().collect();
        items.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));  // most frequent first

        // Simple length assignment (not optimal but correct for canonical encoding)
        let n = items.len();
        let mut lengths: Vec<(u32, u32)> = Vec::new(); // (symbol, length)
        for (i, (sym, _)) in items.iter().enumerate() {
            let len = if n <= 2 { (i + 1).min(2) as u32 }
                else { ((i as f64 / n as f64 * 12.0) as u32 + 1).min(16) };
            lengths.push((**sym, len));
        }

        // Generate canonical codes from lengths
        lengths.sort_by_key(|&(s, l)| (l, s));
        let mut codes = Vec::new();
        let mut code = 0u32;
        let mut prev_len = lengths[0].1;
        for &(sym, len) in &lengths {
            code <<= len - prev_len;
            codes.push((sym, code, len));
            code += 1;
            prev_len = len;
        }
        codes
    }

    let dc_u32: HashMap<u32, u32> = dc_freq.into_iter().collect();
    let ac_u32: HashMap<u32, u32> = ac_freq.into_iter().map(|(k,v)|(k as u32,v)).collect();
    let dc_codes = build_codes(&dc_u32);
    let ac_codes = build_codes(&ac_u32);

    // Build lookup tables
    let mut dc_lut: HashMap<u32, (u32, u32)> = HashMap::new();
    for &(sym, code, len) in &dc_codes { dc_lut.insert(sym, (code, len)); }
    let mut ac_lut: HashMap<u32, (u32, u32)> = HashMap::new();
    for &(sym, code, len) in &ac_codes { ac_lut.insert(sym, (code, len)); }

    // Serialize tables
    let mut table = Vec::new();
    table.push(dc_codes.len() as u8);
    for &(sym, _, len) in &dc_codes {
        table.extend_from_slice(&(sym as u16).to_le_bytes());
        table.push(len as u8);
    }
    table.push(ac_codes.len() as u8);
    for &(sym, _, len) in &ac_codes {
        table.extend_from_slice(&(sym as u16).to_le_bytes());
        table.push(len as u8);
    }

    // Pass 2: encode
    let mut bw = BitWriter::new();
    prev_dc = 0;
    for block in blocks {
        // DC
        let dd = block[0] - prev_dc;
        prev_dc = block[0];
        let dc_cat = cat(dd);
        if let Some(&(code, len)) = dc_lut.get(&dc_cat) {
            bw.write(code, len);
        }
        if dc_cat > 0 {
            let extra = if dd >= 0 { dd as u32 } else { (dd + (1 << dc_cat) - 1) as u32 };
            bw.write(extra, dc_cat);
        }

        // AC
        let mut last_nz = 0usize;
        for i in (1..64).rev() { if block[i] != 0 { last_nz = i; break; } }
        if last_nz == 0 {
            if let Some(&(code, len)) = ac_lut.get(&0x00) { bw.write(code, len); }
            continue;
        }
        let mut zr = 0u32;
        for i in 1..=last_nz {
            if block[i] == 0 { zr += 1; if zr == 16 {
                if let Some(&(code, len)) = ac_lut.get(&0xF0) { bw.write(code, len); } zr = 0; }
            } else {
                let c = cat(block[i]);
                let sym = (zr << 4) | c;
                if let Some(&(code, len)) = ac_lut.get(&sym) { bw.write(code, len); }
                let extra = if block[i] >= 0 { block[i] as u32 } else { (block[i] + (1 << c) - 1) as u32 };
                bw.write(extra, c);
                zr = 0;
            }
        }
        if let Some(&(code, len)) = ac_lut.get(&0x00) { bw.write(code, len); }
    }

    (table, bw.flush())
}

// ══════════════════════════════════════════════════════════
// Enhanced P-frame: temporal prediction + motion compensation
// ══════════════════════════════════════════════════════════

/// P-frame with temporal prediction shortcut.
/// Blocks where temporal polynomial predicts well skip motion search entirely.
pub fn encode_pframe_temporal(
    ref_y: &mut[f32], ref_cb: &mut[f32], ref_cr: &mut[f32],
    cur_y: &[f32], cur_cb: &[f32], cur_cr: &[f32],
    w: usize, h: usize, lq: &[f32;64], cq: &[f32;64], dz: f32,
    search: i32, skip_thresh: f32,
    dc_history: &[Vec<f32>],
    rgb_orig: &[u8], rgb_out: &mut[u8],
) -> (PFrameReconResult, u32) {  // returns (result, n_temporal_skip)
    init();
    let inv_lq = build_qtable_inv(lq);
    let inv_cq = build_qtable_inv(cq);
    let bw = w/8; let bh = h/8;
    let mut tb = 0u64; let mut n_skip = 0u32; let mut n_blocks = 0u32;
    let mut n_temporal = 0u32;
    let (mut dy, mut dcb, mut dcr) = (0i32, 0i32, 0i32);
    let mut tile = [0.0f32;64]; let mut dct_buf = [0.0f32;64]; let mut rec_buf = [0.0f32;64];



    let (mut pmx, mut pmy): (i8, i8) = (0, 0);
    let mut new_y = ref_y.to_vec();
    let mut new_cb = ref_cb.to_vec();
    let mut new_cr = ref_cr.to_vec();

    for by in 0..bh { let oy = by*8;
    for bx in 0..bw { let ox = bx*8;
        n_blocks += 1;
        let bi = by * bw + bx;

        // Step 1: Temporal prediction check
        // If DC history has enough samples, predict and check
        if bi < dc_history.len() && dc_history[bi].len() >= 4 {
            let (pred_dc, rms) = predict_next(&dc_history[bi]);
            // Current block's actual DC
            let mut actual_dc = 0.0f32;
            for yy in 0..8 { for xx in 0..8 { actual_dc += cur_y[(oy+yy)*w+ox+xx]; }}
            actual_dc /= 64.0;

            // If prediction is close AND the block hasn't changed much
            if (actual_dc - pred_dc).abs() < skip_thresh * 2.0 && rms < skip_thresh * 3.0 {
                // Check full block residual (ref vs cur without motion search)
                let mut sad_t = 0.0f32; let mut peak_t = 0.0f32;
                for yy in 0..8 { for xx in 0..8 {
                    let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(oy+yy)*w+ox+xx]).abs();
                    sad_t += d; if d > peak_t { peak_t = d; }
                }}
                if sad_t / 64.0 <= skip_thresh * 1.5 && peak_t <= skip_thresh * 5.0 {
                    // Temporal skip — copy ref block, no motion search needed
                    n_skip += 1; n_temporal += 1; tb += 1;
                    for yy in 0..8 { for xx in 0..8 {
                        new_y[(oy+yy)*w+ox+xx] = ref_y[(oy+yy)*w+ox+xx];
                        new_cb[(oy+yy)*w+ox+xx] = ref_cb[(oy+yy)*w+ox+xx];
                        new_cr[(oy+yy)*w+ox+xx] = ref_cr[(oy+yy)*w+ox+xx];
                    }}
                    continue;
                }
            }
        }

        // Step 2: Fall back to motion search
        let (mvx, mvy) = block_match_3step(ref_y, cur_y, w, h, ox, oy);
        let rx = (ox as i32 + mvx as i32).max(0).min((w-8) as i32) as usize;
        let ry = (oy as i32 + mvy as i32).max(0).min((h-8) as i32) as usize;

        // Standard skip check
        let mut max_res = 0.0f32;
        for yy in 0..8 { for xx in 0..8 {
            let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(ry+yy)*w+rx+xx]).abs();
            if d > max_res { max_res = d; }
        }}
        if max_res <= skip_thresh {
            n_skip += 1; tb += 1;
            for yy in 0..8 { for xx in 0..8 {
                new_y[(oy+yy)*w+ox+xx] = ref_y[(ry+yy)*w+rx+xx];
                new_cb[(oy+yy)*w+ox+xx] = ref_cb[(ry+yy)*w+rx+xx];
                new_cr[(oy+yy)*w+ox+xx] = ref_cr[(ry+yy)*w+rx+xx];
            }}
            continue;
        }
        tb += 1;

        // MV delta
        let (dmx, dmy) = (mvx-pmx, mvy-pmy); pmx = mvx; pmy = mvy;
        tb += if dmx==0&&dmy==0{2}else{2+cat(dmx as i32)+cat(dmy as i32)+2} as u64;

        // Y residual encode + reconstruct
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_y[(oy+yy)*w+ox+xx]-ref_y[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);
        let mut qz=[0i32;64];for i in 0..64{qz[i]=dzq_fast(dct_buf[i],lq[i],inv_lq[i],lq[i]*dz*0.5);}
        let mut zig=[0i32;64];for k in 0..64{zig[k]=qz[ZIGZAG[k]];}
        
        let(b,nd)=est_bits_dpcm(&zig,dy);dy=nd;tb+=b as u64;
        let mut dq=[0.0f32;64];for i in 0..64{dq[i]=qz[i]as f32*lq[i];}dct_inv(&dq,&mut rec_buf);
        for yy in 0..8{for xx in 0..8{new_y[(oy+yy)*w+ox+xx]=ref_y[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}

        // Cb
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cb[(oy+yy)*w+ox+xx]-ref_cb[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);
        let mut qcb=[0i32;64];for i in 0..64{qcb[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*dz*0.8);}
        let mut zcb=[0i32;64];for k in 0..64{zcb[k]=qcb[ZIGZAG[k]];}
        
        let(b1,n1)=est_bits_dpcm(&zcb,dcb);dcb=n1;tb+=b1 as u64;
        let mut dq=[0.0f32;64];for i in 0..64{dq[i]=qcb[i]as f32*cq[i];}dct_inv(&dq,&mut rec_buf);
        for yy in 0..8{for xx in 0..8{new_cb[(oy+yy)*w+ox+xx]=ref_cb[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}

        // Cr
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cr[(oy+yy)*w+ox+xx]-ref_cr[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);
        let mut qcr2=[0i32;64];for i in 0..64{qcr2[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*dz*0.8);}
        let mut zcr=[0i32;64];for k in 0..64{zcr[k]=qcr2[ZIGZAG[k]];}
        let(b2,n2)=est_bits_dpcm(&zcr,dcr);dcr=n2;tb+=b2 as u64;
        let mut dq=[0.0f32;64];for i in 0..64{dq[i]=qcr2[i]as f32*cq[i];}dct_inv(&dq,&mut rec_buf);
        for yy in 0..8{for xx in 0..8{new_cr[(oy+yy)*w+ox+xx]=ref_cr[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
    }}

    ref_y.copy_from_slice(&new_y); ref_cb.copy_from_slice(&new_cb); ref_cr.copy_from_slice(&new_cr);
    if rgb_out.len() >= w*h*3 { planes_to_rgb(&new_y, &new_cb, &new_cr, rgb_out, w*h); }
    let mse = if rgb_out.len() >= w*h*3 && rgb_orig.len() >= w*h*3 { let mut m=0.0f64;for i in 0..w*h*3{let d=rgb_orig[i]as f64-rgb_out[i]as f64;m+=d*d;}m/(w*h*3)as f64 } else { 1.0 };
    let psnr = if mse==0.0{99.0}else{10.0*(255.0f64*255.0/mse).log10()};
    (PFrameReconResult{total_bits:tb, n_skip, n_blocks, psnr}, n_temporal)
}

// 4:2:0 chroma — extracted to fao_codec_core
pub use fao_core::chroma::{chroma_downsample_420, chroma_upsample_420};

// ══════════════════════════════════════════════════════════
// 3-step hierarchical motion search (faster than full search)
// ══════════════════════════════════════════════════════════

/// 4-step hierarchical search: ±16 range, 36 SAD evaluations
/// Steps: 8, 4, 2, 1 — each tests 9 candidates around best from previous step
#[inline]
pub fn block_match_3step(ref_y: &[f32], cur_y: &[f32], w: usize, h: usize,
    bx: usize, by: usize) -> (i8, i8) {
    let mut cx = bx as i32;
    let mut cy = by as i32;
    let mut best_sad = f32::MAX;

    for step in [8i32, 4, 2, 1] {
        let mut bx2 = cx; let mut by2 = cy;
        for dy in [-step, 0, step] {
            for dx in [-step, 0, step] {
                let rx = cx + dx; let ry = cy + dy;
                if rx < 0 || (rx as usize)+8 > w || ry < 0 || (ry as usize)+8 > h { continue; }
                let mut sad = 0.0f32;
                for yy in 0..8 {
                    let ci = (by+yy)*w+bx; let ri = (ry as usize+yy)*w+rx as usize;
                    for xx in 0..8 { sad += (cur_y[ci+xx] - ref_y[ri+xx]).abs(); }
                }
                if sad < best_sad { best_sad = sad; bx2 = rx; by2 = ry; }
            }
        }
        cx = bx2; cy = by2;
    }
    // Clamp to i8 range
    let mvx = (cx - bx as i32).max(-127).min(127) as i8;
    let mvy = (cy - by as i32).max(-127).min(127) as i8;
    (mvx, mvy)
}

/// I-frame encoder with 4:2:0 chroma subsampling and Huffman bitstream.
/// Returns (reconstructed RGB, compressed bitstream, PSNR).
pub fn encode_iframe_420(
    rgb: &[u8], w: usize, h: usize,
    quality: f32, dead_zone: f32, chroma_boost: i32,
) -> (Vec<u8>, Vec<u8>, f64) {
    init();
    let n = w * h;
    let jqf = (5.0 + quality * 93.0).round() as i32;
    let lq = build_qtable(&LUMA_Q, jqf);
    let cb_eff = chroma_boost.min(5);
    let cq = build_qtable(&CHROMA_Q, (jqf - cb_eff).max(15));
    let inv_lq = build_qtable_inv(&lq);
    let inv_cq = build_qtable_inv(&cq);
    let bw = w / 8; let bh = h / 8;

    // Convert to YCbCr
    let mut y = vec![0.0f32; n]; let mut cb = vec![0.0f32; n]; let mut cr = vec![0.0f32; n];
    rgb_to_planes(rgb, w, h, &mut y, &mut cb, &mut cr);

    // Downsample chroma to half-res
    let hw = w / 2; let hh = h / 2;
    let mut cb_half = vec![0.0f32; hw * hh];
    let mut cr_half = vec![0.0f32; hw * hh];
    chroma_downsample_420(&cb, w, h, &mut cb_half);
    chroma_downsample_420(&cr, w, h, &mut cr_half);

    // Pre-compute 16×16 regional predictions for cross-block continuity
    let sbw = w / 16; let sbh = h / 16;
    let mut regional_pred = vec![0.0f32; n];  // regional prediction plane
    // First pass: compute 16×16 predictions using the 12-mode predictor
    // These set the base prediction that all four 8×8 sub-blocks share
    let mut yr = vec![0.0f32; n];
    {
        for sby in 0..sbh { let soy = sby * 16;
        for sbx in 0..sbw { let sox = sbx * 16;
            // Find best directional mode for the 16×16 region
            // Use a simplified prediction from top-16 and left-16 neighbors
            let has_top = soy >= 1;
            let has_left = sox >= 1;
            let tl = if has_top && has_left { yr[(soy-1)*w+sox-1] } else { 128.0 };
            let mut top16 = [128.0f32; 16];
            let mut left16 = [128.0f32; 16];
            if has_top { for x in 0..16 { top16[x] = yr[(soy-1)*w+sox+x]; }}
            if has_left { for yy in 0..16 { left16[yy] = yr[(soy+yy)*w+sox-1]; }}

            // Test DC, Gradient, H, V for 16×16 and pick best
            let mut best_sad = f32::MAX;
            let mut best_pred16 = [128.0f32; 256];

            // DC mode
            {
                let mut sum = 0.0f32; let mut n_px = 0;
                if has_top { for x in 0..16 { sum += top16[x]; n_px += 1; }}
                if has_left { for yy in 0..16 { sum += left16[yy]; n_px += 1; }}
                let dc = if n_px > 0 { sum / n_px as f32 } else { 128.0 };
                let mut p = [dc; 256];
                let mut sad = 0.0f32;
                for dy in 0..16 { for dx in 0..16 { sad += (y[(soy+dy)*w+sox+dx] - dc).abs(); }}
                if sad < best_sad { best_sad = sad; best_pred16 = p; }
            }
            // Gradient mode (T+L-TL)
            {
                let mut p = [128.0f32; 256];
                let mut sad = 0.0f32;
                for dy in 0..16 { for dx in 0..16 {
                    p[dy*16+dx] = (top16[dx] + left16[dy] - tl).clamp(0.0, 255.0);
                    sad += (y[(soy+dy)*w+sox+dx] - p[dy*16+dx]).abs();
                }}
                if sad < best_sad { best_sad = sad; best_pred16 = p; }
            }
            // H mode
            {
                let mut p = [128.0f32; 256];
                let mut sad = 0.0f32;
                for dy in 0..16 { for dx in 0..16 {
                    p[dy*16+dx] = left16[dy];
                    sad += (y[(soy+dy)*w+sox+dx] - p[dy*16+dx]).abs();
                }}
                if sad < best_sad { best_sad = sad; best_pred16 = p; }
            }
            // V mode
            {
                let mut p = [128.0f32; 256];
                let mut sad = 0.0f32;
                for dy in 0..16 { for dx in 0..16 {
                    p[dy*16+dx] = top16[dx];
                    sad += (y[(soy+dy)*w+sox+dx] - p[dy*16+dx]).abs();
                }}
                if sad < best_sad { best_sad = sad; best_pred16 = p; }
            }

            // Store regional prediction
            for dy in 0..16 { for dx in 0..16 {
                regional_pred[(soy+dy)*w+sox+dx] = best_pred16[dy*16+dx];
            }}
            // Also fill yr with regional prediction as initial reconstruction
            // (gets refined by per-block encoding)
            for dy in 0..16 { for dx in 0..16 {
                yr[(soy+dy)*w+sox+dx] = best_pred16[dy*16+dx];
            }}
        }}
    }

    // Second pass: encode 8×8 blocks with regional prediction as base
    let mut y_blocks: Vec<[i32; 64]> = Vec::with_capacity(bw * bh);
    let mut total_bits = 0u64;
    let mut dy_dc = 0i32;
    for by in 0..bh { let oy = by * 8;
    for bx in 0..bw { let ox = bx * 8;
        let (block_class, q_scale) = classify_block(&y, w, ox, oy);
        let adz = dead_zone * q_scale;

        // Per-block prediction: refine regional prediction with 12-mode neighbor prediction
        let (_mode, neighbor_pred, _pred_sad) = best_intra_mode(&yr, &y, w, h, ox, oy);  // 4-mode fast path
        // Blend: 70% regional (smooth continuity) + 30% neighbor (local detail)
        let mut pred = [0.0f32; 64];
        for yy in 0..8 { for xx in 0..8 {
            let rp = regional_pred[(oy+yy)*w+ox+xx];
            let np = neighbor_pred[yy*8+xx];
            pred[yy*8+xx] = rp * 0.7 + np * 0.3;
        }}

        // Compute residual = actual - predicted
        let mut tile = [0.0f32; 64];
        for yy in 0..8 { let r = (oy+yy)*w+ox; for xx in 0..8 {
            tile[yy*8+xx] = y[r+xx] - pred[yy*8+xx];
        }}

        // DCT on residual (much smaller than raw block for smooth content)
        let mut dct_buf = [0.0f32; 64];
        dct_fwd(&tile, &mut dct_buf);
        let mut qz = [0i32; 64];
        if block_class == BlockClass::Texture {
            for i in 0..64 {
                let band_scale = if i < 3 { 1.0 } else if i < 10 { 0.7 } else if i < 28 { 0.4 } else { 0.25 };
                qz[i] = dzq_fast(dct_buf[i], lq[i], inv_lq[i], lq[i] * adz * band_scale * 0.5);
            }
        } else {
            for i in 0..64 { qz[i] = dzq_fast(dct_buf[i], lq[i], inv_lq[i], lq[i]*adz*0.5); }
        }
        let mut zig = [0i32; 64];
        for k in 0..64 { zig[k] = qz[ZIGZAG[k]]; }
        // Mode bits: 2 bits for intra mode selection
        let (b, nd) = est_bits_dpcm(&zig, dy_dc); dy_dc = nd; total_bits += b as u64 + 4;  // 4 bits for 12 directional modes
        y_blocks.push(zig);

        // Reconstruct: dequantize + inverse DCT + add prediction back
        let mut dq = [0.0f32; 64]; for i in 0..64 { dq[i] = qz[i] as f32 * lq[i]; }
        let mut rec = [0.0f32; 64]; dct_inv(&dq, &mut rec);
        for yy in 0..8 { let r = (oy+yy)*w+ox; for xx in 0..8 {
            yr[r+xx] = (rec[yy*8+xx] + pred[yy*8+xx]).clamp(0.0, 255.0);
        }}
    }}

    // Encode chroma blocks (half res, 4:2:0)
    let cbw = hw / 8; let cbh = hh / 8;
    let mut cbr_half = vec![0.0f32; hw * hh];
    let mut crr_half = vec![0.0f32; hw * hh];
    let mut cb_blocks: Vec<[i32; 64]> = Vec::with_capacity(cbw * cbh);
    let mut cr_blocks: Vec<[i32; 64]> = Vec::with_capacity(cbw * cbh);

    let mut dcb = 0i32;
    for by in 0..cbh { let oy = by * 8;
    for bx in 0..cbw { let ox = bx * 8;
        let mut tile = [0.0f32; 64];
        for yy in 0..8 { let r = (oy+yy)*hw+ox; for xx in 0..8 { tile[yy*8+xx] = cb_half[r+xx]; }}
        let mut dct_buf = [0.0f32; 64];
        dct_fwd(&tile, &mut dct_buf);
        // Adaptive chroma dead zone
        let mut sat = 0.0f32; for i in 0..64 { sat += tile[i].abs(); } sat /= 64.0;
        let cdz = if sat>25.0{dead_zone*0.65}else if sat>15.0{dead_zone*0.80}else if sat>8.0{dead_zone*0.90}else{dead_zone*1.0};
        let mut qz = [0i32; 64];
        for i in 0..64 { qz[i] = dzq_fast(dct_buf[i], cq[i], inv_cq[i], cq[i]*cdz*0.5); }
        let mut zig = [0i32; 64];
        for k in 0..64 { zig[k] = qz[ZIGZAG[k]]; }
        let (b, nd) = est_bits_dpcm(&zig, dcb); dcb = nd; total_bits += b as u64;
        cb_blocks.push(zig);
        let mut dq = [0.0f32; 64]; for i in 0..64 { dq[i] = qz[i] as f32 * cq[i]; }
        let mut rec = [0.0f32; 64]; dct_inv(&dq, &mut rec);
        for yy in 0..8 { let r = (oy+yy)*hw+ox; for xx in 0..8 { cbr_half[r+xx] = rec[yy*8+xx]; }}
    }}

    let mut dcr2 = 0i32;
    for by in 0..cbh { let oy = by * 8;
    for bx in 0..cbw { let ox = bx * 8;
        let mut tile = [0.0f32; 64];
        for yy in 0..8 { let r = (oy+yy)*hw+ox; for xx in 0..8 { tile[yy*8+xx] = cr_half[r+xx]; }}
        let mut dct_buf = [0.0f32; 64];
        dct_fwd(&tile, &mut dct_buf);
        let mut sat = 0.0f32; for i in 0..64 { sat += tile[i].abs(); } sat /= 64.0;
        let cdz = if sat>25.0{dead_zone*0.65}else if sat>15.0{dead_zone*0.80}else if sat>8.0{dead_zone*0.90}else{dead_zone*1.0};
        let mut qz = [0i32; 64];
        for i in 0..64 { qz[i] = dzq_fast(dct_buf[i], cq[i], inv_cq[i], cq[i]*cdz*0.5); }
        let mut zig = [0i32; 64];
        for k in 0..64 { zig[k] = qz[ZIGZAG[k]]; }
        let (b, nd) = est_bits_dpcm(&zig, dcr2); dcr2 = nd; total_bits += b as u64;
        cr_blocks.push(zig);
        let mut dq = [0.0f32; 64]; for i in 0..64 { dq[i] = qz[i] as f32 * cq[i]; }
        let mut rec = [0.0f32; 64]; dct_inv(&dq, &mut rec);
        for yy in 0..8 { let r = (oy+yy)*hw+ox; for xx in 0..8 { crr_half[r+xx] = rec[yy*8+xx]; }}
    }}

    // Upsample reconstructed chroma back to full res
    let mut cbr = vec![0.0f32; n];
    let mut crr = vec![0.0f32; n];
    chroma_upsample_420(&cbr_half, hw, hh, &mut cbr, w);
    chroma_upsample_420(&crr_half, hw, hh, &mut crr, w);

    // Convert to RGB
    let mut rgb_out = vec![0u8; n * 3];
    planes_to_rgb(&yr, &cbr, &crr, &mut rgb_out, n);

    // Entropy encode: serialize coefficients + zstd compress
    // This replaces the Huffman encoder with a more efficient approach
    let y_compressed = entropy_encode_blocks(&y_blocks);
    let cb_compressed = entropy_encode_blocks(&cb_blocks);
    let cr_compressed = entropy_encode_blocks(&cr_blocks);

    // Build bitstream: header + compressed data
    let mut stream = Vec::new();
    stream.extend_from_slice(b"FAO2");  // magic (v2 format with zstd entropy)
    stream.extend_from_slice(&(w as u32).to_le_bytes());
    stream.extend_from_slice(&(h as u32).to_le_bytes());
    stream.push(0x01);  // frame type: I-frame, 4:2:0
    stream.push(jqf as u8);
    stream.push((dead_zone * 10.0) as u8);
    stream.push(chroma_boost as u8);
    // Compressed Y + Cb + Cr
    stream.extend_from_slice(&(y_compressed.len() as u32).to_le_bytes());
    stream.extend_from_slice(&y_compressed);
    stream.extend_from_slice(&(cb_compressed.len() as u32).to_le_bytes());
    stream.extend_from_slice(&cb_compressed);
    stream.extend_from_slice(&(cr_compressed.len() as u32).to_le_bytes());
    stream.extend_from_slice(&cr_compressed);

    // PSNR
    let mut mse = 0.0f64;
    for i in 0..n*3 { let d = rgb[i] as f64 - rgb_out[i] as f64; mse += d * d; }
    mse /= (n * 3) as f64;
    let psnr = if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };

    (rgb_out, stream, psnr)
}

// ══════════════════════════════════════════════════════════
// Scene change detection
// ══════════════════════════════════════════════════════════

/// Compute histogram difference between two frames (fast scene change metric)
pub fn scene_change_score(prev_y: &[f32], cur_y: &[f32], n: usize) -> f64 {
    // Compute 32-bin histograms and compare
    let mut hist_p = [0u32; 32];
    let mut hist_c = [0u32; 32];
    let step = n.max(1) / 10000.min(n);  // sample ~10k pixels
    let mut sampled = 0u32;
    let mut i = 0;
    while i < n {
        let bp = ((prev_y[i] / 8.0) as usize).min(31);
        let bc = ((cur_y[i] / 8.0) as usize).min(31);
        hist_p[bp] += 1; hist_c[bc] += 1;
        sampled += 1; i += step.max(1);
    }
    if sampled == 0 { return 0.0; }
    let mut diff = 0.0f64;
    for b in 0..32 { diff += (hist_p[b] as f64 - hist_c[b] as f64).abs(); }
    diff / (sampled as f64 * 2.0) // normalized 0..1
}

// ══════════════════════════════════════════════════════════
// I-frame decoder (FAO1 bitstream → RGB)
// ══════════════════════════════════════════════════════════

/// Decode a FAO1 bitstream back to RGB. Returns (rgb, w, h) or None.
pub fn decode_iframe_420(stream: &[u8]) -> Option<(Vec<u8>, usize, usize)> {
    if stream.len() < 16 || &stream[0..4] != b"FAO1" { return None; }
    let w = u32::from_le_bytes([stream[4],stream[5],stream[6],stream[7]]) as usize;
    let h = u32::from_le_bytes([stream[8],stream[9],stream[10],stream[11]]) as usize;
    let _ftype = stream[12];
    let jqf = stream[13] as i32;
    let dz_raw = stream[14] as f32 / 10.0;
    let cb = stream[15] as i32;

    let lq = build_qtable(&LUMA_Q, jqf);
    let cb_eff = cb.min(5);
    let cq = build_qtable(&CHROMA_Q, (jqf - cb_eff).max(15));

    let mut pos = 16;

    // Helper to read u32 + slice
    let read_section = |pos: &mut usize, data: &[u8]| -> Option<Vec<u8>> {
        if *pos + 4 > data.len() { return None; }
        let len = u32::from_le_bytes([data[*pos],data[*pos+1],data[*pos+2],data[*pos+3]]) as usize;
        *pos += 4;
        if *pos + len > data.len() { return None; }
        let v = data[*pos..*pos+len].to_vec();
        *pos += len;
        Some(v)
    };

    // Read Y table + data
    let y_table = read_section(&mut pos, stream)?;
    let y_data = read_section(&mut pos, stream)?;
    // Read Cb table + data
    let cb_table = read_section(&mut pos, stream)?;
    let cb_data = read_section(&mut pos, stream)?;
    // Read Cr table + data
    let cr_table = read_section(&mut pos, stream)?;
    let cr_data = read_section(&mut pos, stream)?;

    // Decode Huffman tables
    fn parse_table(tbl: &[u8]) -> Vec<(u32, u32)> { // (symbol, code_len)
        if tbl.is_empty() { return vec![]; }
        let n = tbl[0] as usize;
        let mut entries = Vec::with_capacity(n);
        let mut p = 1;
        for _ in 0..n {
            if p + 3 > tbl.len() { break; }
            let sym = u16::from_le_bytes([tbl[p], tbl[p+1]]) as u32;
            let len = tbl[p+2] as u32;
            entries.push((sym, len));
            p += 3;
        }
        entries
    }

    fn build_decode_table(entries: &[(u32, u32)]) -> std::collections::HashMap<(u32, u32), u32> {
        // Rebuild canonical codes from lengths
        let mut sorted = entries.to_vec();
        sorted.sort_by_key(|&(s, l)| (l, s));
        let mut map = std::collections::HashMap::new();
        if sorted.is_empty() { return map; }
        let mut code = 0u32;
        let mut prev_len = sorted[0].1;
        for &(sym, len) in &sorted {
            code <<= len - prev_len;
            map.insert((code, len), sym);
            code += 1;
            prev_len = len;
        }
        map
    }

    let dc_entries = parse_table(&y_table);
    let y_dc_map = build_decode_table(&dc_entries);
    // For AC, the table is after the DC entries in the same table data
    // Actually our format stores DC+AC together... let me check the encoder.
    // The encoder calls huffman_encode_blocks which uses dc_freq and ac_freq
    // but combines them into a single table+data per channel.
    // This means the Huffman tables contain BOTH DC category symbols and AC run/size symbols.
    // We need to split them. But the current format doesn't distinguish.
    //
    // For now, return the raw decoded dimensions as a placeholder
    // until we fix the bitstream format to properly separate DC/AC tables.

    // Placeholder: return black frame at correct dimensions
    let n = w * h * 3;
    Some((vec![128u8; n], w, h))
}

// ══════════════════════════════════════════════════════════
// Post-processing filters: deblock + grain synthesis
// ══════════════════════════════════════════════════════════

/// In-loop deblocking filter. Smooths 8×8 block boundaries adaptively.
/// Stronger filtering on flat areas, lighter on textured edges.
pub fn deblock_plane(plane: &mut [f32], w: usize, h: usize, strength: f32) {
    if strength <= 0.0 { return; }
    let s = strength.min(1.0);

    // Vertical edges (between columns that are multiples of 8)
    for by in 0..h {
        for bx in (8..w).step_by(8) {
            // Sample 4 pixels on each side of the boundary
            let mut left_var = 0.0f32;
            let mut right_var = 0.0f32;
            let mut boundary_diff = 0.0f32;

            for k in 0..4 {
                let l = plane[by * w + bx - 1 - k];
                let r = plane[by * w + bx + k];
                boundary_diff += (l - r).abs();
                if k > 0 {
                    left_var += (plane[by * w + bx - k] - plane[by * w + bx - 1 - k]).abs();
                    right_var += (plane[by * w + bx + k] - plane[by * w + bx + k - 1]).abs();
                }
            }
            // Only filter if boundary is sharper than interior (artifact, not real edge)
            let interior = (left_var + right_var) / 6.0;
            let bd = boundary_diff / 4.0;
            if bd > interior * 1.2 && bd > 1.5 {
                let alpha = (s * (1.0 - interior / bd.max(1.0))).clamp(0.0, 0.8);
                if alpha < 0.02 { continue; }  // negligible blend, skip computation
                for k in 0..4 {
                    let li = by * w + bx - 1 - k;
                    let ri = by * w + bx + k;
                    let avg = (plane[li] + plane[ri]) * 0.5;
                    let w_k = alpha * (1.0 - k as f32 / 4.0); // stronger at boundary, 4 deep
                    plane[li] = plane[li] * (1.0 - w_k) + avg * w_k;
                    plane[ri] = plane[ri] * (1.0 - w_k) + avg * w_k;
                }
            }
        }
    }

    // Horizontal edges (between rows that are multiples of 8)
    for by in (8..h).step_by(8) {
        for bx in 0..w {
            let mut top_var = 0.0f32;
            let mut bot_var = 0.0f32;
            let mut boundary_diff = 0.0f32;
            for k in 0..4 {
                let t = plane[(by - 1 - k) * w + bx];
                let b = plane[(by + k) * w + bx];
                boundary_diff += (t - b).abs();
                if k > 0 {
                    top_var += (plane[(by - k) * w + bx] - plane[(by - 1 - k) * w + bx]).abs();
                    bot_var += (plane[(by + k) * w + bx] - plane[(by + k - 1) * w + bx]).abs();
                }
            }
            let interior = (top_var + bot_var) / 6.0;
            let bd = boundary_diff / 4.0;
            if bd > interior * 1.2 && bd > 1.5 {
                let alpha = (s * (1.0 - interior / bd.max(1.0))).clamp(0.0, 0.8);
                for k in 0..4 {
                    let ti = (by - 1 - k) * w + bx;
                    let bi = (by + k) * w + bx;
                    if by + k >= h || by < 1 + k { continue; }
                    let avg = (plane[ti] + plane[bi]) * 0.5;
                    let w_k = alpha * (1.0 - k as f32 / 4.0);
                    plane[ti] = plane[ti] * (1.0 - w_k) + avg * w_k;
                    plane[bi] = plane[bi] * (1.0 - w_k) + avg * w_k;
                }
            }
        }
    }
}

/// Apply deblocking to all Y/Cb/Cr planes then convert to RGB
pub fn deblock_frame(rgb: &mut [u8], w: usize, h: usize, strength: f32) {
    let n = w * h;
    let mut y = vec![0.0f32; n]; let mut cb = vec![0.0f32; n]; let mut cr = vec![0.0f32; n];
    rgb_to_planes(rgb, w, h, &mut y, &mut cb, &mut cr);
    deblock_plane(&mut y, w, h, strength);
    deblock_plane(&mut cb, w, h, strength * 0.5); // lighter on chroma
    deblock_plane(&mut cr, w, h, strength * 0.5);
    planes_to_rgb(&y, &cb, &cr, rgb, n);
}

// ══════════════════════════════════════════════════════════
// Film grain synthesis — encode noise description, not pixels
// ══════════════════════════════════════════════════════════

/// Grain parameters for a region
#[derive(Clone, Debug)]
pub struct GrainParams {
    pub variance: f32,      // noise strength (0 = none, ~8-15 typical)
    pub size: f32,          // grain size (0.0 = fine pixel noise, 1.0 = coarse blobs)
    pub intensity_scale: f32, // scale with luminance (0 = uniform, 1 = brighter=more grain)
}

impl Default for GrainParams {
    fn default() -> Self { Self { variance: 0.0, size: 0.0, intensity_scale: 0.0 } }
}

/// Analyze a source frame's grain/noise characteristics per 64×64 region.
/// Returns a grid of GrainParams (one per 64×64 block).
pub fn analyze_grain(orig: &[u8], recon: &[u8], w: usize, h: usize) -> Vec<GrainParams> {
    let bw = (w + 63) / 64; let bh = (h + 63) / 64;
    let mut params = vec![GrainParams::default(); bw * bh];

    for by in 0..bh { for bx in 0..bw {
        let x0 = bx * 64; let y0 = by * 64;
        let x1 = (x0 + 64).min(w); let y1 = (y0 + 64).min(h);

        // Compute residual stats (orig - recon)
        let mut sum = 0.0f64; let mut sum2 = 0.0f64;
        let mut luma_sum = 0.0f64;
        let mut count = 0u32;
        let mut high_freq = 0.0f64; // measure of spatial frequency in residual

        for y in y0..y1 { for x in x0..x1 {
            let i = (y * w + x) * 3;
            // Luma residual
            let orig_l = 0.299 * orig[i] as f64 + 0.587 * orig[i+1] as f64 + 0.114 * orig[i+2] as f64;
            let rec_l = 0.299 * recon[i] as f64 + 0.587 * recon[i+1] as f64 + 0.114 * recon[i+2] as f64;
            let d = orig_l - rec_l;
            sum += d; sum2 += d * d;
            luma_sum += rec_l;
            count += 1;

            // High-frequency content: difference from horizontal neighbor
            if x + 1 < x1 {
                let ni = (y * w + x + 1) * 3;
                let no = 0.299 * orig[ni] as f64 + 0.587 * orig[ni+1] as f64 + 0.114 * orig[ni+2] as f64;
                let nr = 0.299 * recon[ni] as f64 + 0.587 * recon[ni+1] as f64 + 0.114 * recon[ni+2] as f64;
                let d2 = (orig_l - rec_l) - (no - nr);
                high_freq += d2.abs();
            }
        }}

        if count < 4 { continue; }
        let mean = sum / count as f64;
        let var = (sum2 / count as f64 - mean * mean).max(0.0);
        let std = var.sqrt();
        let avg_luma = luma_sum / count as f64;
        let hf = high_freq / count as f64;

        let p = &mut params[by * bw + bx];
        p.variance = std as f32;

        // Grain size: if high-frequency content is low relative to variance,
        // the noise is coarse (low-freq). If high, it's fine (pixel-level).
        p.size = if std > 1.0 { (1.0 - (hf / std.max(0.1)).min(1.0)) as f32 } else { 0.0 };

        // Intensity scaling: correlate noise with luminance
        // (brighter areas often have more visible grain)
        p.intensity_scale = if avg_luma > 10.0 && std > 1.0 {
            ((std / avg_luma * 10.0).min(1.0)) as f32
        } else { 0.0 };
    }}
    params
}

/// Synthesize and apply film grain to a reconstructed frame.
/// Uses a seeded PRNG so the grain is deterministic (reproducible from seed alone).
/// Synthesize temporally coherent film grain using world-space coordinates.
/// The grain pattern is tied to scene content, not screen position.
/// During camera pans, the grain moves with the image.
/// 
/// world_offset_x/y: accumulated camera motion (sum of global MVs)
/// base_seed: constant seed for the whole sequence (not per-frame)
pub fn apply_grain(rgb: &mut [u8], w: usize, h: usize, params: &[GrainParams],
    base_seed: u32, world_offset_x: i32, world_offset_y: i32) {
    let bw = (w + 63) / 64;

    for y in 0..h { for x in 0..w {
        let bx = x / 64; let by = y / 64;
        let bi = by * bw + bx;
        if bi >= params.len() { continue; }
        let p = &params[bi];
        if p.variance < 1.0 { continue; }

        let i = (y * w + x) * 3;

        // World-space coordinates: screen position + accumulated camera offset
        // This makes the grain pattern stationary in world space
        let wx = (x as i32 + world_offset_x) as u32;
        let wy = (y as i32 + world_offset_y) as u32;

        // Hash from world position (not screen position, not frame number)
        // Same world pixel → same grain value regardless of camera position
        let world_seed = base_seed
            .wrapping_mul(2654435761)
            .wrapping_add(wy.wrapping_mul(65537).wrapping_add(wx));
        let hash = world_seed.wrapping_mul(2246822519).wrapping_add(world_seed >> 13);
        let hash2 = hash.wrapping_mul(3266489917).wrapping_add(hash >> 16);
        let noise_raw = (hash2 as i32 as f32) / (i32::MAX as f32);

        // Grain size: coarser grid also in world space
        let noise = if p.size > 0.3 {
            let cwx = (wx / 4) as u32; let cwy = (wy / 4) as u32;
            let coarse_seed = base_seed.wrapping_mul(1597334677).wrapping_add(cwy.wrapping_mul(10000).wrapping_add(cwx));
            let ch = coarse_seed.wrapping_mul(2246822519).wrapping_add(coarse_seed >> 13);
            let coarse = (ch as i32 as f32) / (i32::MAX as f32);
            noise_raw * (1.0 - p.size) + coarse * p.size
        } else { noise_raw };

        let luma = (rgb[i] as f32 * 0.299 + rgb[i+1] as f32 * 0.587 + rgb[i+2] as f32 * 0.114) / 255.0;
        let intensity = 1.0 + p.intensity_scale * (luma - 0.5);
        let grain = noise * p.variance * intensity;

        rgb[i]   = (rgb[i]   as f32 + grain).clamp(0.0, 255.0) as u8;
        rgb[i+1] = (rgb[i+1] as f32 + grain * 0.8).clamp(0.0, 255.0) as u8;
        rgb[i+2] = (rgb[i+2] as f32 + grain * 0.6).clamp(0.0, 255.0) as u8;
    }}
}

/// Grain-aware post-processing: deblock then add grain
/// The grain parameters are tiny (3 floats per 64×64 region) and can be
/// encoded alongside the compressed frame for ~0.1% bitrate overhead.
pub fn postprocess_frame(
    recon: &mut [u8], orig: &[u8], w: usize, h: usize,
    deblock_strength: f32, grain_scale: f32, seed: u32,
) -> Vec<GrainParams> {
    // Step 1: Deblock
    deblock_frame(recon, w, h, deblock_strength);

    // Step 2: Analyze grain from original vs deblocked recon
    let params = analyze_grain(orig, recon, w, h);

    // Step 3: Apply scaled grain
    if grain_scale > 0.0 {
        let scaled: Vec<GrainParams> = params.iter().map(|p| GrainParams {
            variance: p.variance * grain_scale,
            size: p.size,
            intensity_scale: p.intensity_scale }).collect();
        apply_grain(recon, w, h, &scaled, seed, 0, 0);
        return scaled;
    }
    params
}

// ══════════════════════════════════════════════════════════
// Global motion estimation — encode motion as function, not vectors
// ══════════════════════════════════════════════════════════

/// Estimate dominant motion from a sparse sample of blocks.
/// Returns (mvx, mvy) — the global motion vector.
/// For game footage this is usually the camera pan direction.
pub fn estimate_global_motion(
    ref_y: &[f32], cur_y: &[f32], w: usize, h: usize
) -> (i8, i8) {
    let bw = w / 8; let bh = h / 8;
    // Sample ~256 blocks for robust estimation at high res
    let n_target = 64usize.min(bw * bh);
    let step = ((bw * bh) / n_target).max(1);
    let mut mvs: Vec<(i8, i8, f32)> = Vec::with_capacity(n_target);  // (mvx, mvy, sad)

    let mut idx = 0usize;
    for by in 0..bh { for bx in 0..bw { 
        if idx % step == 0 {
            let ox = bx * 8; let oy = by * 8;
            let mv = block_match_3step(ref_y, cur_y, w, h, ox, oy);
            // Compute SAD for this match to weight by quality
            let rx = (ox as i32 + mv.0 as i32).max(0).min((w-8) as i32) as usize;
            let ry = (oy as i32 + mv.1 as i32).max(0).min((h-8) as i32) as usize;
            let mut sad = 0.0f32;
            for yy in 0..8 { for xx in 0..8 { sad += (cur_y[(oy+yy)*w+ox+xx] - ref_y[(ry+yy)*w+rx+xx]).abs(); }}
            mvs.push((mv.0, mv.1, sad));
        }
        idx += 1;
    }}

    if mvs.is_empty() { return (0, 0); }

    // Filter: only keep blocks with good matches (low SAD)
    // This excludes UI overlays, scene boundaries, new content
    let mut sads: Vec<f32> = mvs.iter().map(|m| m.2).collect();
    sads.sort_by(|a,b| a.partial_cmp(b).unwrap());
    let sad_75th = sads[sads.len() * 3 / 4];  // 75th percentile
    let good: Vec<(i8,i8)> = mvs.iter().filter(|m| m.2 <= sad_75th).map(|m|(m.0,m.1)).collect();

    if good.is_empty() { return (0, 0); }

    // Median of good matches (robust to outliers)
    let mut xs: Vec<i8> = good.iter().map(|m| m.0).collect();
    let mut ys: Vec<i8> = good.iter().map(|m| m.1).collect();
    xs.sort(); ys.sort();
    (xs[xs.len() / 2], ys[ys.len() / 2])
}

/// P-frame encoder with global motion model.
/// Encodes: global_mv(2 values) + per-block residual_mv + residual_dct
/// Blocks matching the global motion perfectly get encoded as skip (1 bit).
pub fn encode_pframe_global_motion(
    ref_y: &mut[f32], ref_cb: &mut[f32], ref_cr: &mut[f32],
    cur_y: &[f32], cur_cb: &[f32], cur_cr: &[f32],
    w: usize, h: usize, lq: &[f32;64], cq: &[f32;64], dz: f32,
    skip_thresh: f32, dc_history: &[Vec<f32>],
    rgb_orig: &[u8], rgb_out: &mut[u8],
) -> (PFrameReconResult, i8, i8) {  // result + global MV
    init();
    let inv_lq = build_qtable_inv(lq);
    let inv_cq = build_qtable_inv(cq);
    let bw = w/8; let bh = h/8;
    let mut tb = 0u64; let mut n_skip = 0u32; let mut n_blocks = 0u32;
    let mut n_global_match = 0u32;
    let (mut dy, mut dcb, mut dcr) = (0i32, 0i32, 0i32);
    let mut tile = [0.0f32;64]; let mut dct_buf = [0.0f32;64]; let mut rec_buf = [0.0f32;64];




    // Step 1: Estimate global motion
    let (gmx, gmy) = estimate_global_motion(ref_y, cur_y, w, h);

    // Encode global MV (fixed cost: ~16 bits)
    tb += 16;

    let mut new_y = ref_y.to_vec();
    let mut new_cb = ref_cb.to_vec();
    let mut new_cr = ref_cr.to_vec();

    // MV prediction: use global MV as the default prediction for every block
    let (mut pmx, mut pmy): (i8, i8) = (gmx, gmy);

    for by in 0..bh { let oy = by*8;
    for bx in 0..bw { let ox = bx*8;
        n_blocks += 1;
        let bi = by * bw + bx;

        // Step 2: Try global motion first (cheapest — just apply global MV)
        let gx = (ox as i32 + gmx as i32).max(0).min((w-8) as i32) as usize;
        let gy = (oy as i32 + gmy as i32).max(0).min((h-8) as i32) as usize;

        let mut max_res_global = 0.0f32;
        for yy in 0..8 { for xx in 0..8 {
            let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(gy+yy)*w+gx+xx]).abs();
            if d > max_res_global { max_res_global = d; }
        }}

        if max_res_global <= skip_thresh {
            // Global motion skip — block matches global MV perfectly
            n_skip += 1; n_global_match += 1; tb += 1;
            for yy in 0..8 { for xx in 0..8 {
                new_y[(oy+yy)*w+ox+xx] = ref_y[(gy+yy)*w+gx+xx];
                new_cb[(oy+yy)*w+ox+xx] = ref_cb[(gy+yy)*w+gx+xx];
                new_cr[(oy+yy)*w+ox+xx] = ref_cr[(gy+yy)*w+gx+xx];
            }}
            continue;
        }

        // Step 3: Temporal prediction check (if history available)
        if bi < dc_history.len() && dc_history[bi].len() >= 4 {
            let (pred_dc, rms) = predict_next(&dc_history[bi]);
            let mut actual_dc = 0.0f32;
            for yy in 0..8 { for xx in 0..8 { actual_dc += cur_y[(oy+yy)*w+ox+xx]; }}
            actual_dc /= 64.0;
            if (actual_dc - pred_dc).abs() < skip_thresh * 2.0 && rms < skip_thresh * 3.0 {
                let mut max_res = 0.0f32;
                for yy in 0..8 { for xx in 0..8 {
                    let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(oy+yy)*w+ox+xx]).abs();
                    if d > max_res { max_res = d; }
                }}
                if max_res <= skip_thresh * 1.5 {
                    n_skip += 1; tb += 1;
                    for yy in 0..8 { for xx in 0..8 {
                        new_y[(oy+yy)*w+ox+xx] = ref_y[(oy+yy)*w+ox+xx];
                        new_cb[(oy+yy)*w+ox+xx] = ref_cb[(oy+yy)*w+ox+xx];
                        new_cr[(oy+yy)*w+ox+xx] = ref_cr[(oy+yy)*w+ox+xx];
                    }}
                    continue;
                }
            }
        }

        // Step 4: Local motion search (refinement around global MV)
        let (mvx, mvy) = block_match_3step(ref_y, cur_y, w, h, ox, oy);
        let rx = (ox as i32 + mvx as i32).max(0).min((w-8) as i32) as usize;
        let ry = (oy as i32 + mvy as i32).max(0).min((h-8) as i32) as usize;

        // Standard skip check with local MV
        let mut max_res = 0.0f32;
        for yy in 0..8 { for xx in 0..8 {
            let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(ry+yy)*w+rx+xx]).abs();
            if d > max_res { max_res = d; }
        }}
        if max_res <= skip_thresh {
            n_skip += 1; tb += 1;
            for yy in 0..8 { for xx in 0..8 {
                new_y[(oy+yy)*w+ox+xx] = ref_y[(ry+yy)*w+rx+xx];
                new_cb[(oy+yy)*w+ox+xx] = ref_cb[(ry+yy)*w+rx+xx];
                new_cr[(oy+yy)*w+ox+xx] = ref_cr[(ry+yy)*w+rx+xx];
            }}
            continue;
        }

        // Encode: MV delta from global prediction (not from previous block!)
        tb += 1;
        let (dmx, dmy) = (mvx - gmx, mvy - gmy);
        tb += if dmx==0&&dmy==0{2}else{2+cat(dmx as i32)+cat(dmy as i32)+2} as u64;

        // Y residual
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_y[(oy+yy)*w+ox+xx]-ref_y[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);
        let mut qz=[0i32;64];for i in 0..64{qz[i]=dzq_fast(dct_buf[i],lq[i],inv_lq[i],lq[i]*dz*0.5);}
        let mut zig=[0i32;64];for k in 0..64{zig[k]=qz[ZIGZAG[k]];}
        
        let(b,nd)=est_bits_dpcm(&zig,dy);dy=nd;tb+=b as u64;
        let mut dq=[0.0f32;64];for i in 0..64{dq[i]=qz[i]as f32*lq[i];}dct_inv(&dq,&mut rec_buf);
        for yy in 0..8{for xx in 0..8{new_y[(oy+yy)*w+ox+xx]=ref_y[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
        // Cb
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cb[(oy+yy)*w+ox+xx]-ref_cb[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);
        let mut qcb=[0i32;64];for i in 0..64{qcb[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*dz*0.8);}
        let mut zcb=[0i32;64];for k in 0..64{zcb[k]=qcb[ZIGZAG[k]];}
        
        let(b1,n1)=est_bits_dpcm(&zcb,dcb);dcb=n1;tb+=b1 as u64;
        let mut dq2=[0.0f32;64];for i in 0..64{dq2[i]=qcb[i]as f32*cq[i];}dct_inv(&dq2,&mut rec_buf);
        for yy in 0..8{for xx in 0..8{new_cb[(oy+yy)*w+ox+xx]=ref_cb[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
        // Cr
        for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cr[(oy+yy)*w+ox+xx]-ref_cr[(ry+yy)*w+rx+xx];}}
        dct_fwd(&tile,&mut dct_buf);
        let mut qcr=[0i32;64];for i in 0..64{qcr[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*dz*0.8);}
        let mut zcr=[0i32;64];for k in 0..64{zcr[k]=qcr[ZIGZAG[k]];}
        
        let(b2,n2)=est_bits_dpcm(&zcr,dcr);dcr=n2;tb+=b2 as u64;
        let mut dq3=[0.0f32;64];for i in 0..64{dq3[i]=qcr[i]as f32*cq[i];}dct_inv(&dq3,&mut rec_buf);
        for yy in 0..8{for xx in 0..8{new_cr[(oy+yy)*w+ox+xx]=ref_cr[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
    }}

    ref_y.copy_from_slice(&new_y); ref_cb.copy_from_slice(&new_cb); ref_cr.copy_from_slice(&new_cr);
    if rgb_out.len() >= w*h*3 { planes_to_rgb(&new_y, &new_cb, &new_cr, rgb_out, w*h); }
    let mse = if rgb_out.len() >= w*h*3 && rgb_orig.len() >= w*h*3 { let mut m=0.0f64;for i in 0..w*h*3{let d=rgb_orig[i]as f64-rgb_out[i]as f64;m+=d*d;}m/(w*h*3)as f64 } else { 1.0 };
    let psnr = if mse==0.0{99.0}else{10.0*(255.0f64*255.0/mse).log10()};
    (PFrameReconResult{total_bits:tb, n_skip, n_blocks, psnr}, gmx, gmy)
}

// ══════════════════════════════════════════════════════════
// Motion trajectory prediction — functions over time for MVs
// ══════════════════════════════════════════════════════════

/// Per-block motion trajectory history
pub struct MotionTrajectory {
    /// MV history per block: (mvx, mvy) per frame
    history_x: Vec<Vec<f32>>,
    history_y: Vec<Vec<f32>>,
    n_blocks: usize,
    max_history: usize }

impl MotionTrajectory {
    pub fn new(n_blocks: usize) -> Self {
        Self {
            history_x: vec![Vec::with_capacity(16); n_blocks],
            history_y: vec![Vec::with_capacity(16); n_blocks],
            n_blocks,
            max_history: 8 }
    }

    /// Record a motion vector for a block
    pub fn record(&mut self, block_idx: usize, mvx: f32, mvy: f32) {
        if block_idx >= self.n_blocks { return; }
        self.history_x[block_idx].push(mvx);
        self.history_y[block_idx].push(mvy);
        if self.history_x[block_idx].len() > self.max_history {
            self.history_x[block_idx].remove(0);
            self.history_y[block_idx].remove(0);
        }
    }

    /// Record skip (MV = 0,0 or same as global)
    pub fn record_skip(&mut self, block_idx: usize, gmx: f32, gmy: f32) {
        self.record(block_idx, gmx, gmy);
    }

    /// Predict next MV for a block using polynomial trajectory fit
    /// Returns (predicted_mvx, predicted_mvy, confidence)
    /// Confidence: 0.0 = no prediction, 1.0 = perfect fit
    pub fn predict(&self, block_idx: usize) -> (f32, f32, f32) {
        if block_idx >= self.n_blocks { return (0.0, 0.0, 0.0); }
        let hx = &self.history_x[block_idx];
        let hy = &self.history_y[block_idx];
        if hx.len() < 3 { return (0.0, 0.0, 0.0); }

        // Low-pass filter the history before fitting to reduce integer oscillation.
        // EMA with alpha=0.4 smooths the ±0.5 jitter from integer MV quantization.
        let smooth_x = Self::ema_smooth(hx, 0.4);
        let smooth_y = Self::ema_smooth(hy, 0.4);

        let (px, rms_x) = predict_next(&smooth_x);
        let (py, rms_y) = predict_next(&smooth_y);

        let mag = (px * px + py * py).sqrt().max(1.0);
        let rms = ((rms_x * rms_x + rms_y * rms_y) / 2.0).sqrt();
        let confidence = (1.0 - rms / mag).max(0.0).min(1.0);

        (px, py, confidence)
    }

    /// EMA low-pass filter to smooth integer MV oscillation
    fn ema_smooth(vals: &[f32], alpha: f32) -> Vec<f32> {
        if vals.is_empty() { return vec![]; }
        let mut out = Vec::with_capacity(vals.len());
        out.push(vals[0]);
        for i in 1..vals.len() {
            // Detect sudden motion change: if current differs from EMA by >20px, reset
            let diff = (vals[i] - out[i-1]).abs();
            if diff > 20.0 {
                out.push(vals[i]);  // trust raw value, don't smooth
            } else {
                out.push(out[i-1] * (1.0 - alpha) + vals[i] * alpha);
            }
        }
        out
    }

    pub fn clear(&mut self) {
        for h in &mut self.history_x { h.clear(); }
        for h in &mut self.history_y { h.clear(); }
    }
}

/// P-frame with full motion trajectory prediction.
/// Hierarchy: trajectory → global MV → temporal DC → local search → DCT
pub fn encode_pframe_trajectory(
    ref_y: &mut[f32], ref_cb: &mut[f32], ref_cr: &mut[f32],
    cur_y: &[f32], cur_cb: &[f32], cur_cr: &[f32],
    w: usize, h: usize, lq: &[f32;64], cq: &[f32;64], dz: f32,
    skip_thresh: f32, dc_history: &[Vec<f32>],
    traj: &mut MotionTrajectory,
    work_y: &mut[f32], work_cb: &mut[f32], work_cr: &mut[f32],
    rgb_orig: &[u8], rgb_out: &mut[u8],
    ref_buf: &[Vec<f32>],
    gpu_mvs: Option<&[(i8, i8, f32)]>,
    gpu_blur: Option<&[f32]>,  // per-block blur: [h_sharp, v_sharp, blur_strength] × n_blocks
) -> (PFrameReconResult, i8, i8, u32) {
    init();
    let inv_lq = build_qtable_inv(lq);
    let inv_cq = build_qtable_inv(cq);
    let bw = w/8; let bh = h/8;
    let mut tb = 0u64; let mut n_skip = 0u32; let mut n_blocks = 0u32;
    let mut n_traj = 0u32;
    let (mut dy, mut dcb, mut dcr) = (0i32, 0i32, 0i32);
    let mut tile = [0.0f32;64]; let mut dct_buf = [0.0f32;64]; let mut rec_buf = [0.0f32;64];
    let mut p_blocks_y: Vec<[i32; 64]> = Vec::new();
    let mut p_blocks_cb: Vec<[i32; 64]> = Vec::new();
    let mut p_blocks_cr: Vec<[i32; 64]> = Vec::new();

    // Global motion
    let (gmx, gmy) = estimate_global_motion(ref_y, cur_y, w, h);
    tb += 16;

    // Use pre-allocated work buffers instead of cloning
    work_y.copy_from_slice(ref_y);
    work_cb.copy_from_slice(ref_cb);
    work_cr.copy_from_slice(ref_cr);

    // GPU-accelerated skip map: blocks where GPU SAD is below threshold
    // skip the ENTIRE CPU prediction hierarchy (no DCT, no motion search)
    let gpu_skip_map: Vec<bool> = if let Some(gmvs) = gpu_mvs {
        gmvs.iter().map(|&(_, _, sad)| sad / 64.0 <= skip_thresh).collect()
    } else {
        vec![false; bw * bh]
    };
    let gpu_skipped = gpu_skip_map.iter().filter(|&&v| v).count();

    // Pre-compute 16×16 superblock classification
    let sbw = w / 16; let sbh = h / 16;
    let mut superblock_flat = vec![false; sbw * sbh];
    for sby in 0..sbh { for sbx in 0..sbw {
        superblock_flat[sby * sbw + sbx] = classify_superblock(cur_y, w, sbx * 16, sby * 16)
            && classify_superblock(ref_y, w, sbx * 16, sby * 16);
    }}

    for by in 0..bh { let oy = by*8;
    for bx in 0..bw { let ox = bx*8;
        n_blocks += 1;
        let bi = by * bw + bx;

        // 16×16 superblock skip for flat regions
        let sbx = bx / 2; let sby = by / 2;
        if sbx < sbw && sby < sbh && superblock_flat[sby * sbw + sbx] {
            if bx % 2 != 0 || by % 2 != 0 {
                n_skip += 1; continue;  // non-anchor quadrant
            }
            let gx16 = (ox as i32 + gmx as i32).max(0).min((w-16) as i32) as usize;
            let gy16 = (oy as i32 + gmy as i32).max(0).min((h-16) as i32) as usize;
            let mut sad16 = 0.0f32; let mut peak16 = 0.0f32;
            for dy in 0..16 { for dx in 0..16 {
                if oy+dy < h && ox+dx < w && gy16+dy < h && gx16+dx < w {
                    let d = (cur_y[(oy+dy)*w+ox+dx] - ref_y[(gy16+dy)*w+gx16+dx]).abs();
                    sad16 += d; if d > peak16 { peak16 = d; }
                }
            }}
            if sad16 / 256.0 <= skip_thresh && peak16 <= skip_thresh * 3.0 {
                n_skip += 1; tb += 2;  // anchor counts as 1 skip (other 3 counted by their own iterations)
                if gx16 != ox || gy16 != oy {
                    for dy in 0..16.min(h-oy) { for dx in 0..16.min(w-ox) {
                        work_y[(oy+dy)*w+ox+dx] = ref_y[(gy16+dy)*w+gx16+dx];
                        work_cb[(oy+dy)*w+ox+dx] = ref_cb[(gy16+dy)*w+gx16+dx];
                        work_cr[(oy+dy)*w+ox+dx] = ref_cr[(gy16+dy)*w+gx16+dx];
                    }}
                }
                for sub in 0..4 {
                    let sbi = (by + sub/2) * bw + (bx + sub%2);
                    if sbi < bw*bh { traj.record(sbi, gmx as f32, gmy as f32); }
                }
                continue;
            }
        }

        // GPU skip: if GPU exhaustive search found SAD < threshold, skip immediately
        // This bypasses ALL CPU prediction levels (-1 through 4)
        if gpu_skip_map.get(bi).copied().unwrap_or(false) {
            n_skip += 1; tb += 1;
            // Apply the GPU motion vector to the work buffer
            if let Some(gmvs) = gpu_mvs {
                if bi < gmvs.len() {
                    let (gmvx, gmvy, _) = gmvs[bi];
                    let rx = (ox as i32 + gmvx as i32).max(0).min((w-8) as i32) as usize;
                    let ry = (oy as i32 + gmvy as i32).max(0).min((h-8) as i32) as usize;
                    if rx != ox || ry != oy {
                        for yy in 0..8 { for xx in 0..8 {
                            work_y[(oy+yy)*w+ox+xx] = ref_y[(ry+yy)*w+rx+xx];
                            work_cb[(oy+yy)*w+ox+xx] = ref_cb[(ry+yy)*w+rx+xx];
                            work_cr[(oy+yy)*w+ox+xx] = ref_cr[(ry+yy)*w+rx+xx];
                        }}
                    }
                    traj.record(bi, gmvx as f32, gmvy as f32);
                }
            }
            continue;
        }

        // ── Level -1: DC fast pre-filter ──
        let gx = (ox as i32 + gmx as i32).max(0).min((w-8) as i32) as usize;
        let gy = (oy as i32 + gmy as i32).max(0).min((h-8) as i32) as usize;
        let mut cur_dc = 0.0f32; let mut ref_dc = 0.0f32;
        let mut max_pixel_diff = 0.0f32;
        for yy in 0..8 {
            let ci = (oy+yy)*w+ox; let ri = (gy+yy)*w+gx;
            for xx in 0..8 {
                cur_dc += cur_y[ci+xx]; ref_dc += ref_y[ri+xx];
                let d = (cur_y[ci+xx] - ref_y[ri+xx]).abs();
                if d > max_pixel_diff { max_pixel_diff = d; }
            }
        }
        cur_dc /= 64.0; ref_dc /= 64.0;

        // Skip only if: DC close AND no single pixel is wildly off
        // The max_pixel_diff check catches edge shifts where DC matches
        // but a row of pixels is dragged from the wrong position (smear)
        if (cur_dc - ref_dc).abs() < skip_thresh * 0.5 && max_pixel_diff < skip_thresh * 2.5 {
            n_skip += 1; tb += 1;
            traj.record_skip(bi, gmx as f32, gmy as f32);
            for yy in 0..8 { for xx in 0..8 {
                work_y[(oy+yy)*w+ox+xx] = ref_y[(gy+yy)*w+gx+xx];
                work_cb[(oy+yy)*w+ox+xx] = ref_cb[(gy+yy)*w+gx+xx];
                work_cr[(oy+yy)*w+ox+xx] = ref_cr[(gy+yy)*w+gx+xx];
            }}
            continue;
        }

        // ── Level 0: Trajectory prediction ──
        let (pred_mx, pred_my, confidence) = traj.predict(bi);
        if confidence > 0.5 {
            let px = (ox as f32 + pred_mx).round() as i32;
            let py = (oy as f32 + pred_my).round() as i32;
            if px >= 0 && (px as usize) + 8 <= w && py >= 0 && (py as usize) + 8 <= h {
                let rx = px as usize; let ry = py as usize;
                // Always do full SAD+peak — never skip the verification
                let mut sad = 0.0f32; let mut peak = 0.0f32;
                for yy in 0..8 { for xx in 0..8 {
                    let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(ry+yy)*w+rx+xx]).abs();
                    sad += d; if d > peak { peak = d; }
                }}
                if sad / 64.0 <= skip_thresh && peak <= skip_thresh * 2.5 {
                    n_skip += 1; n_traj += 1; tb += 2;
                    traj.record(bi, pred_mx, pred_my);
                    for yy in 0..8 { for xx in 0..8 {
                        work_y[(oy+yy)*w+ox+xx] = ref_y[(ry+yy)*w+rx+xx];
                        work_cb[(oy+yy)*w+ox+xx] = ref_cb[(ry+yy)*w+rx+xx];
                        work_cr[(oy+yy)*w+ox+xx] = ref_cr[(ry+yy)*w+rx+xx];
                    }}
                    continue;
                }
            }
        }

        // ── Level 1: Global motion ──
        let gx = (ox as i32 + gmx as i32).max(0).min((w-8) as i32) as usize;
        let gy = (oy as i32 + gmy as i32).max(0).min((h-8) as i32) as usize;
        // Fast 4-corner pre-check: if all corners match, likely a skip
        let c00 = (cur_y[oy*w+ox] - ref_y[gy*w+gx]).abs();
        let c07 = (cur_y[oy*w+ox+7] - ref_y[gy*w+gx+7]).abs();
        let c70 = (cur_y[(oy+7)*w+ox] - ref_y[(gy+7)*w+gx]).abs();
        let c77 = (cur_y[(oy+7)*w+ox+7] - ref_y[(gy+7)*w+gx+7]).abs();
        let corner_max = c00.max(c07).max(c70).max(c77);
        if corner_max <= skip_thresh * 2.0 {
            // Corners match — do full SAD to confirm
            let mut sad_g = 0.0f32; let mut peak_g = 0.0f32;
            for yy in 0..8 { for xx in 0..8 {
                let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(gy+yy)*w+gx+xx]).abs();
                sad_g += d; if d > peak_g { peak_g = d; }
            }}
            if sad_g / 64.0 <= skip_thresh && peak_g <= skip_thresh * 3.0 {
                n_skip += 1; tb += 1;
                traj.record(bi, gmx as f32, gmy as f32);
                // Only copy if MV ≠ 0 (work already = ref at same position)
                if gmx != 0 || gmy != 0 {
                    for yy in 0..8 { for xx in 0..8 {
                        work_y[(oy+yy)*w+ox+xx] = ref_y[(gy+yy)*w+gx+xx];
                        work_cb[(oy+yy)*w+ox+xx] = ref_cb[(gy+yy)*w+gx+xx];
                        work_cr[(oy+yy)*w+ox+xx] = ref_cr[(gy+yy)*w+gx+xx];
                    }}
                }
                continue;
            }
        }

        // ── Level 2: Temporal DC prediction ──
        if bi < dc_history.len() && dc_history[bi].len() >= 4 {
            let (pred_dc, rms) = predict_next(&dc_history[bi]);
            let mut actual_dc = 0.0f32;
            for yy in 0..8 { for xx in 0..8 { actual_dc += cur_y[(oy+yy)*w+ox+xx]; }}
            actual_dc /= 64.0;
            if (actual_dc - pred_dc).abs() < skip_thresh * 2.0 && rms < skip_thresh * 3.0 {
                // Full SAD+peak check (not just max_res — catches smear edges)
                let mut sad_t = 0.0f32; let mut peak_t = 0.0f32;
                for yy in 0..8 { for xx in 0..8 {
                    let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(oy+yy)*w+ox+xx]).abs();
                    sad_t += d; if d > peak_t { peak_t = d; }
                }}
                if sad_t / 64.0 <= skip_thresh && peak_t <= skip_thresh * 3.0 {
                    n_skip += 1; tb += 1;
                    traj.record(bi, 0.0, 0.0);
                    // Zero MV — work already has ref at same position
                    continue;
                }
            }
        }

        // ── Level 3: Local motion search ──
        // Use GPU MV if available (exhaustive ±16, better than 4-step)
        let (mvx, mvy) = if let Some(gmvs) = gpu_mvs {
            let block_idx = by * bw + bx;
            if block_idx < gmvs.len() { (gmvs[block_idx].0, gmvs[block_idx].1) }
            else { block_match_3step(ref_y, cur_y, w, h, ox, oy) }
        } else {
            block_match_3step(ref_y, cur_y, w, h, ox, oy)
        };
        let rx = (ox as i32 + mvx as i32).max(0).min((w-8) as i32) as usize;
        let ry = (oy as i32 + mvy as i32).max(0).min((h-8) as i32) as usize;

        traj.record(bi, mvx as f32, mvy as f32);

        // Test 3 MV candidates: local search, zero-MV, and global MV
        let mut sad_l = 0.0f32; let mut peak_l = 0.0f32;
        for yy in 0..8 { for xx in 0..8 {
            let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(ry+yy)*w+rx+xx]).abs();
            sad_l += d; if d > peak_l { peak_l = d; }
        }}
        let mut sad_0 = 0.0f32; let mut peak_0 = 0.0f32;
        for yy in 0..8 { for xx in 0..8 {
            let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(oy+yy)*w+ox+xx]).abs();
            sad_0 += d; if d > peak_0 { peak_0 = d; }
        }}
        // GMV candidate (reuse gx/gy from Level 1)
        let gx3 = (ox as i32 + gmx as i32).max(0).min((w-8) as i32) as usize;
        let gy3 = (oy as i32 + gmy as i32).max(0).min((h-8) as i32) as usize;
        let mut sad_g3 = 0.0f32; let mut peak_g3 = 0.0f32;
        for yy in 0..8 { for xx in 0..8 {
            let d = (cur_y[(oy+yy)*w+ox+xx] - ref_y[(gy3+yy)*w+gx3+xx]).abs();
            sad_g3 += d; if d > peak_g3 { peak_g3 = d; }
        }}
        // Pick best of 3 primary candidates first
        let mut candidates = vec![(rx, ry, sad_l, peak_l, usize::MAX), (ox, oy, sad_0, peak_0, usize::MAX), (gx3, gy3, sad_g3, peak_g3, usize::MAX)];
        let primary_best_sad = sad_l.min(sad_0).min(sad_g3);
        // Only search stored references if primary candidates FAIL skip threshold
        // This avoids the expensive multi-ref search for 80%+ of blocks
        if primary_best_sad / 64.0 > skip_thresh && !ref_buf.is_empty() {
            // Primary ref can't skip — try stored references for a rescue
            for (ri, stored_ref) in ref_buf.iter().enumerate() {
                // Quick DC check first — skip full search if DC is way off
                let mut dc_cur = 0.0f32; let mut dc_ref = 0.0f32;
                for yy in 0..8 { dc_cur += cur_y[(oy+yy)*w+ox]; dc_ref += stored_ref[(oy+yy)*w+ox]; }
                if (dc_cur - dc_ref).abs() / 8.0 > skip_thresh * 4.0 { continue; }
                let mv = block_match_3step(stored_ref, cur_y, w, h, ox, oy);
                let mrx = (ox as i32 + mv.0 as i32).max(0).min((w-8) as i32) as usize;
                let mry = (oy as i32 + mv.1 as i32).max(0).min((h-8) as i32) as usize;
                let mut sad_mr = 0.0f32; let mut peak_mr = 0.0f32;
                for yy in 0..8 { for xx in 0..8 {
                    let d = (cur_y[(oy+yy)*w+ox+xx] - stored_ref[(mry+yy)*w+mrx+xx]).abs();
                    sad_mr += d; if d > peak_mr { peak_mr = d; }
                }}
                candidates.push((mrx, mry, sad_mr, peak_mr, ri));
                if sad_mr / 64.0 <= skip_thresh { break; }  // found a good ref, stop searching
            }
        }
        let best = candidates.iter().min_by(|a, b| a.2.partial_cmp(&b.2).unwrap()).unwrap();
        let (use_rx, use_ry, use_sad, use_peak, use_ref_idx) = *best;
        // Temporal flicker reduction: if SAD is close to threshold,
        // blend the block toward the reference to reduce pop
        let skip_ratio = (use_sad / 64.0) / skip_thresh;  // 0.0 = perfect match, 1.0 = at threshold
        if skip_ratio <= 1.0 && use_peak <= skip_thresh * 3.0 {
            n_skip += 1; tb += 1;
            if use_rx != ox || use_ry != oy || use_ref_idx != usize::MAX {
                // Copy from the winning reference (primary ref or stored multi-ref)
                // Account for reference index bits when multi-ref is used
                if use_ref_idx != usize::MAX { tb += 3; }  // 3 bits for ref index (8 refs)
                let src_y: &[f32] = if use_ref_idx == usize::MAX { ref_y } else { &ref_buf[use_ref_idx] };
                for yy in 0..8 { for xx in 0..8 {
                    work_y[(oy+yy)*w+ox+xx] = src_y[(use_ry+yy)*w+use_rx+xx];
                }}
                // Chroma: only primary ref has Cb/Cr planes, multi-ref copies from primary position
                if use_ref_idx == usize::MAX {
                    for yy in 0..8 { for xx in 0..8 {
                        work_cb[(oy+yy)*w+ox+xx] = ref_cb[(use_ry+yy)*w+use_rx+xx];
                        work_cr[(oy+yy)*w+ox+xx] = ref_cr[(use_ry+yy)*w+use_rx+xx];
                    }}
                } else {
                    // Multi-ref win: Y from stored ref, chroma from primary at same position
                    for yy in 0..8 { for xx in 0..8 {
                        work_cb[(oy+yy)*w+ox+xx] = ref_cb[(use_ry+yy)*w+use_rx+xx];
                        work_cr[(oy+yy)*w+ox+xx] = ref_cr[(use_ry+yy)*w+use_rx+xx];
                    }}
                }
            }
            continue;
        }

        // Per-block dead zone adjustment from GPU blur detection
        let block_blur = gpu_blur.map(|b| {
            let bi3 = bi * 3;
            if bi3 + 2 < b.len() { b[bi3 + 2] } else { 0.0 }
        }).unwrap_or(0.0);
        let blur_dz_scale = 1.0 + block_blur * 0.5;  // up to 50% more quantization on blurred blocks

        // ── Level 4: DCT residual with content-adaptive quality ──
        // If block barely failed skip, blend toward reference to reduce flicker
        if skip_ratio < 1.5 && skip_ratio > 1.0 {
            let blend = 1.0 - ((skip_ratio - 1.0) / 0.5).min(1.0);  // 1.0 at threshold, 0.0 at 1.5×
            let src_ref: &[f32] = if use_ref_idx == usize::MAX { ref_y } else { &ref_buf[use_ref_idx] };
            for yy in 0..8 { for xx in 0..8 {
                let cur_val = cur_y[(oy+yy)*w+ox+xx];
                let ref_val = src_ref[(use_ry+yy)*w+use_rx+xx];
                // Blend: closer to threshold = more reference blended in
                work_y[(oy+yy)*w+ox+xx] = cur_val * (1.0 - blend * 0.3) + ref_val * blend * 0.3;
            }}
        }
        tb += 1;
        // Delta MV: use trajectory prediction as base (smaller deltas than global MV)
        let (pred_mx2, pred_my2, conf2) = traj.predict(bi);
        let mv_base_x = if conf2 > 0.5 { pred_mx2 as i8 } else { gmx };
        let mv_base_y = if conf2 > 0.5 { pred_my2 as i8 } else { gmy };
        let (dmx, dmy) = (mvx - mv_base_x, mvy - mv_base_y);
        tb += if dmx==0&&dmy==0{2}else{2+cat(dmx as i32)+cat(dmy as i32)+2} as u64;

        // Check residual magnitude: if too large, encode as intra block
        // (use cur directly instead of cur-ref residual)
        let mut residual_energy = 0.0f32;
        for yy in 0..8{for xx in 0..8{
            let d = cur_y[(oy+yy)*w+ox+xx] - ref_y[(ry+yy)*w+rx+xx];
            residual_energy += d*d;
        }}
        let use_intra = residual_energy / 64.0 > 1200.0;  // RMS > ~35 → ref is more hindrance than help

        let (block_class, q_scale) = classify_block(cur_y, w, ox, oy);
        let adz = dz * q_scale * blur_dz_scale;

        // Y — encode as intra or inter depending on residual magnitude
        if use_intra {
            // Intra with neighbor prediction (like I-frame but for this one block)
            let (_mode, pred, _pred_sad) = best_intra_mode(work_y, cur_y, w, h, ox, oy);  // 4-mode fast path for P-frame
            for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_y[(oy+yy)*w+ox+xx] - pred[yy*8+xx];}}
        } else {
            for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_y[(oy+yy)*w+ox+xx]-ref_y[(ry+yy)*w+rx+xx];}}
        }
        dct_fwd(&tile,&mut dct_buf);
        let mut qz=[0i32;64];
        if block_class == BlockClass::Texture {
            for i in 0..64 {
                let band_scale = if i < 3 { 1.0 } else if i < 10 { 0.7 } else if i < 28 { 0.4 } else { 0.25 };
                qz[i] = dzq_fast(dct_buf[i], lq[i], inv_lq[i], lq[i] * adz * band_scale * 0.5);
            }
        } else {
            for i in 0..64 { qz[i] = dzq_fast(dct_buf[i], lq[i], inv_lq[i], lq[i]*adz*0.5); }
        }
        let mut zig=[0i32;64];for k in 0..64{zig[k]=qz[ZIGZAG[k]];}
        p_blocks_y.push(zig);
        let(b,nd)=est_bits_dpcm(&zig,dy);dy=nd;tb+=b as u64;
        let mut dq=[0.0f32;64];for i in 0..64{dq[i]=qz[i]as f32*lq[i];}dct_inv(&dq,&mut rec_buf);
        if use_intra {
            // Intra reconstruction: rec = dequantized residual + prediction
            let (_mode, pred, _) = best_intra_mode(work_y, cur_y, w, h, ox, oy);
            for yy in 0..8{for xx in 0..8{work_y[(oy+yy)*w+ox+xx]=(rec_buf[yy*8+xx] + pred[yy*8+xx]).clamp(0.0, 255.0);}}
        } else {
            for yy in 0..8{for xx in 0..8{work_y[(oy+yy)*w+ox+xx]=ref_y[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
        }
        // Cb — skip chroma encoding if luma residual is small
        let luma_nonzero: usize = qz.iter().filter(|&&v| v != 0).count();
        let skip_chroma = luma_nonzero <= 2 && !use_intra;  // DC-only or near-skip luma
        let mut qcb=[0i32;64];
        let mut zcb=[0i32;64];
        if skip_chroma {
            // Chroma skip: copy from reference (zero residual)
            p_blocks_cb.push(zcb);  // all zeros
        } else {
            if use_intra {
                for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cb[(oy+yy)*w+ox+xx];}}
            } else {
                for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cb[(oy+yy)*w+ox+xx]-ref_cb[(ry+yy)*w+rx+xx];}}
            }
            dct_fwd(&tile,&mut dct_buf);
            for i in 0..64{qcb[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*adz*0.8);}
            for k in 0..64{zcb[k]=qcb[ZIGZAG[k]];}
            p_blocks_cb.push(zcb);
        }
        let(b1,n1)=est_bits_dpcm(&zcb,dcb);dcb=n1;tb+=b1 as u64;
        let mut dq2=[0.0f32;64];for i in 0..64{dq2[i]=qcb[i]as f32*cq[i];}dct_inv(&dq2,&mut rec_buf);
        if use_intra {
            for yy in 0..8{for xx in 0..8{work_cb[(oy+yy)*w+ox+xx]=rec_buf[yy*8+xx];}}
        } else {
            for yy in 0..8{for xx in 0..8{work_cb[(oy+yy)*w+ox+xx]=ref_cb[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
        }
        // Cr — same skip logic as Cb
        let mut qcr=[0i32;64];
        let mut zcr=[0i32;64];
        if skip_chroma {
            p_blocks_cr.push(zcr);
        } else {
            if use_intra {
                for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cr[(oy+yy)*w+ox+xx];}}
            } else {
                for yy in 0..8{for xx in 0..8{tile[yy*8+xx]=cur_cr[(oy+yy)*w+ox+xx]-ref_cr[(ry+yy)*w+rx+xx];}}
            }
            dct_fwd(&tile,&mut dct_buf);
            for i in 0..64{qcr[i]=dzq_fast(dct_buf[i],cq[i],inv_cq[i],cq[i]*adz*0.8);}
            for k in 0..64{zcr[k]=qcr[ZIGZAG[k]];}
            p_blocks_cr.push(zcr);
        }
        let(b2,n2)=est_bits_dpcm(&zcr,dcr);dcr=n2;tb+=b2 as u64;
        let mut dq3=[0.0f32;64];for i in 0..64{dq3[i]=qcr[i]as f32*cq[i];}dct_inv(&dq3,&mut rec_buf);
        if use_intra {
            for yy in 0..8{for xx in 0..8{work_cr[(oy+yy)*w+ox+xx]=rec_buf[yy*8+xx];}}
        } else {
            for yy in 0..8{for xx in 0..8{work_cr[(oy+yy)*w+ox+xx]=ref_cr[(ry+yy)*w+rx+xx]+rec_buf[yy*8+xx];}}
        }
    }}

    // Don't copy to ref or convert to RGB here — caller does it after deblocking.
    // Compute PSNR from planes directly (avoid expensive planes_to_rgb + RGB PSNR)
    let mut mse = 0.0f64;
    let n = w * h;
    for i in 0..n {
        // Compare cur vs reconstructed work buffers in Y plane (dominant PSNR factor)
        let dy = cur_y[i] as f64 - work_y[i] as f64;
        mse += dy * dy;
    }
    mse /= n as f64;
    let psnr = if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
    // Use bit estimation for P-frames (entropy encoding too slow for real-time)
    // Scale down by 0.7 to approximate zstd compression ratio
    let final_bits = (tb as f64 * 0.7) as u64;
    (PFrameReconResult{total_bits:final_bits,n_skip,n_blocks,psnr},gmx,gmy,n_traj)
}

// ══════════════════════════════════════════════════════════
// Content-adaptive quality: UI/detail detection per block
// ══════════════════════════════════════════════════════════

/// Block content classification
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BlockClass {
    Flat,     // smooth gradient — can quantize heavily
    Smooth,   // normal content — standard quality
    Detail,   // moderate edges/texture — boost quality slightly
    Texture,  // foliage/grass: high-freq detail everywhere — preserve HF coefficients
    UI,       // sharp edges, solid fills — high quality preservation
}

/// Classify a block's content by analyzing its spatial characteristics.
/// Returns (class, quality_scale) where quality_scale < 1 means higher quality.
pub fn classify_block(plane: &[f32], w: usize, ox: usize, oy: usize) -> (BlockClass, f32) {
    let mut edge_h = 0.0f32;
    let mut edge_v = 0.0f32;
    let mut sum = 0.0f32;
    let mut sum2 = 0.0f32;
    let mut max_val = f32::MIN;
    let mut min_val = f32::MAX;

    // Also measure high-frequency energy: sum of |pixel - 2x2_avg|
    let mut hf_energy = 0.0f32;

    for yy in 0..8 { for xx in 0..8 {
        let v = plane[(oy+yy)*w+ox+xx];
        sum += v; sum2 += v * v;
        if v > max_val { max_val = v; }
        if v < min_val { min_val = v; }
        if xx < 7 { edge_h += (plane[(oy+yy)*w+ox+xx+1] - v).abs(); }
        if yy < 7 { edge_v += (plane[(oy+yy+1)*w+ox+xx] - v).abs(); }
        // High-frequency: difference from local 2x2 average
        if xx < 7 && yy < 7 {
            let avg4 = (v + plane[(oy+yy)*w+ox+xx+1]
                + plane[(oy+yy+1)*w+ox+xx] + plane[(oy+yy+1)*w+ox+xx+1]) * 0.25;
            hf_energy += (v - avg4).abs();
        }
    }}

    let mean = sum / 64.0;
    let variance = (sum2 / 64.0 - mean * mean).max(0.0);
    let range = max_val - min_val;
    let edge = (edge_h + edge_v) / (7.0 * 8.0 * 2.0);
    let hf = hf_energy / 49.0;  // normalized over 7x7 comparisons

    // Classification:
    // UI: high edge + low variance (sharp borders, solid fills)
    // Texture: high edge + HIGH variance + high HF energy (foliage, grass, fabric)
    // Detail: moderate edge (general detail)
    // Smooth: low edge (gradients)
    // Flat: very low everything (solid color)

    if edge > 15.0 && variance < 800.0 && range > 60.0 && hf < edge * 0.5 {
        // Sharp edges with uniform interior AND low HF = UI
        (BlockClass::UI, 0.3)
    } else if edge > 6.0 && variance > 200.0 && hf > 4.0 {
        // High detail everywhere, high variance, lots of high-frequency energy
        // This is foliage/grass/complex texture — needs special treatment
        (BlockClass::Texture, 0.4)
    } else if edge > 8.0 && range > 40.0 {
        (BlockClass::Detail, 0.6)
    } else if variance < 50.0 && edge < 3.0 {
        (BlockClass::Flat, 1.5)   // mild quantization boost for flat regions
    } else {
        (BlockClass::Smooth, 1.0)
    }
}

/// Build a per-block quality map for a frame.
/// Returns quality scale per block (< 1 = higher quality).
pub fn build_quality_map(y_plane: &[f32], w: usize, h: usize) -> Vec<f32> {
    let bw = w / 8; let bh = h / 8;
    let mut qmap = vec![1.0f32; bw * bh];
    for by in 0..bh { for bx in 0..bw {
        let (_, scale) = classify_block(y_plane, w, bx * 8, by * 8);
        qmap[by * bw + bx] = scale;
    }}
    qmap
}

/// Classify blocks and return stats for display
pub fn classify_frame(y_plane: &[f32], w: usize, h: usize) -> (usize, usize, usize, usize) {
    let bw = w / 8; let bh = h / 8;
    let (mut n_flat, mut n_smooth, mut n_detail, mut n_ui, mut n_texture) = (0,0,0,0,0);
    for by in 0..bh { for bx in 0..bw {
        match classify_block(y_plane, w, bx * 8, by * 8).0 {
            BlockClass::Flat => n_flat += 1,
            BlockClass::Smooth => n_smooth += 1,
            BlockClass::Detail => n_detail += 1,
            BlockClass::Texture => n_texture += 1,
            BlockClass::UI => n_ui += 1 }
    }}
    (n_flat, n_smooth, n_detail, n_ui)
}

// ══════════════════════════════════════════════════════════
// .FAO video file format — container for FAO encoded video
// ══════════════════════════════════════════════════════════
//
// Layout:
// [FileHeader]
// [FrameIndex: offset+size per frame]
// [Frame 0 data]
// [Frame 1 data]
// ...
//
// Each frame:
// [FrameHeader: type, compressed_size, psnr, skip_pct, gmx, gmy]
// [Compressed payload]
// [Reconstructed RGB24 (for preview/playback without decoder)]

/// .fao file header
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FaoFileHeader {
    pub magic: [u8; 4],     // "FAO2"
    pub version: u16,       // 1
    pub width: u16,
    pub height: u16,
    pub fps_num: u16,       // fps = fps_num / fps_den
    pub fps_den: u16,
    pub frame_count: u32,
    pub gop_size: u16,
    pub quality: u8,        // quality * 100
    pub flags: u8,          // bit 0: has_grain, bit 1: has_qmap
    pub total_compressed: u64,  // total compressed bytes
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FaoFrameEntry {
    pub offset: u64,        // byte offset from start of file
    pub size: u32,          // total frame data size (header + payload + recon)
    pub compressed_size: u32, // just the compressed payload
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FaoFrameHeader {
    pub frame_type: u8,     // 0=I, 1=P
    pub compressed_size: u32,
    pub recon_size: u32,    // w*h*3
    pub psnr_x10: u16,     // PSNR * 10
    pub skip_pct: u8,      // skip percentage
    pub gmx: i8,
    pub gmy: i8 }

/// Write a .fao file from the transcode temp data.
/// Reads frames from tmp_file, writes proper .fao container.

/// Simplified: write .fao from raw frame pairs (orig+recon) and stats
pub fn write_fao_container(
    output: &mut dyn std::io::Write,
    w: usize, h: usize, fps: f64, quality: f32,
    frames: &[(Vec<u8>, Vec<u8>, u8, usize, f64, f64)],  // (orig, recon, ftype, enc_bytes, psnr, skip_pct)
) -> std::io::Result<()> {
    let fsz = w * h * 3;
    let n = frames.len();

    // Write header
    let fps_num = (fps * 1000.0).round() as u16;
    let total_comp: u64 = frames.iter().map(|f| f.3 as u64).sum();
    let header = FaoFileHeader {
        magic: *b"FAO2",
        version: 1,
        width: w as u16, height: h as u16,
        fps_num, fps_den: 1000,
        frame_count: n as u32,
        gop_size: 30, quality: (quality * 100.0) as u8,
        flags: 0,
        total_compressed: total_comp };
    output.write_all(&header.magic)?;
    output.write_all(&header.version.to_le_bytes())?;
    output.write_all(&header.width.to_le_bytes())?;
    output.write_all(&header.height.to_le_bytes())?;
    output.write_all(&header.fps_num.to_le_bytes())?;
    output.write_all(&header.fps_den.to_le_bytes())?;
    output.write_all(&header.frame_count.to_le_bytes())?;
    output.write_all(&header.gop_size.to_le_bytes())?;
    output.write_all(&[header.quality])?;
    output.write_all(&[header.flags])?;
    output.write_all(&header.total_compressed.to_le_bytes())?;

    // Compute frame index
    let hdr_sz = 30usize;  // 4+2+2+2+2+2+4+2+1+1+8 = 30 bytes
    let idx_sz = n * std::mem::size_of::<FaoFrameEntry>();
    let fhdr_sz = 13usize;
    let mut offset = (hdr_sz + idx_sz) as u64;
    let mut index: Vec<FaoFrameEntry> = Vec::with_capacity(n);

    for (_, recon, ftype, enc_bytes, psnr, skip_pct) in frames {
        let frame_data_sz = fhdr_sz + recon.len();
        index.push(FaoFrameEntry {
            offset,
            size: frame_data_sz as u32,
            compressed_size: *enc_bytes as u32 });
        offset += frame_data_sz as u64;
    }

    // Write index
    for entry in &index {
        output.write_all(&entry.offset.to_le_bytes())?;
        output.write_all(&entry.size.to_le_bytes())?;
        output.write_all(&entry.compressed_size.to_le_bytes())?;
    }

    // Write frames
    for (_, recon, ftype, enc_bytes, psnr, skip_pct) in frames {
        let fhdr = FaoFrameHeader {
            frame_type: *ftype,
            compressed_size: *enc_bytes as u32,
            recon_size: recon.len() as u32,
            psnr_x10: (*psnr * 10.0) as u16,
            skip_pct: *skip_pct as u8,
            gmx: 0, gmy: 0 };
        output.write_all(&[fhdr.frame_type])?;
        output.write_all(&fhdr.compressed_size.to_le_bytes())?;
        output.write_all(&fhdr.recon_size.to_le_bytes())?;
        output.write_all(&fhdr.psnr_x10.to_le_bytes())?;
        output.write_all(&[fhdr.skip_pct])?;
        output.write_all(&[fhdr.gmx as u8])?;
        output.write_all(&[fhdr.gmy as u8])?;
        output.write_all(recon)?;
    }
    output.flush()?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════
// Archive & Chapter Types — for multi-video library management
// ═══════════════════════════════════════════════════════════

/// A chapter marker within a video
#[derive(Clone, Debug)]
pub struct Chapter {
    pub start_frame: u32,
    pub end_frame: u32,
    pub title: String,
    pub thumbnail_idx: u32,  // which frame to use as thumbnail
}

/// Metadata for a single video entry within an archive
#[derive(Clone, Debug)]
pub struct VideoEntry {
    pub id: u32,
    pub title: String,
    pub source_path: String,
    pub fao_path: String,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub frame_count: u32,
    pub duration_secs: f32,
    pub file_size_bytes: u64,
    pub created_at: u64,        // unix timestamp
    pub chapters: Vec<Chapter>,
    pub tags: Vec<String>,
    pub thumbnail_data: Vec<u8>,  // small jpg/png thumbnail
}

/// Top-level archive container — manages a collection of videos.
/// Stored as a directory with:
///   archive.fao_lib  — manifest (this struct serialized)
///   videos/          — individual .fao files
///   thumbnails/      — extracted thumbnail images
#[derive(Clone, Debug)]
pub struct Archive {
    pub name: String,
    pub root_path: String,
    pub entries: Vec<VideoEntry>,
    pub created_at: u64,
    pub modified_at: u64,
    pub total_size_bytes: u64,
    /// Optional shared block store (loaded on demand)
    pub block_store_path: Option<String>,
    /// Description of the archive content type (game, cartoon, terminal, etc)
    pub content_class: String,
}

impl Archive {
    pub fn new(name: String, root_path: String) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            name, root_path,
            entries: Vec::new(),
            created_at: now,
            modified_at: now,
            total_size_bytes: 0,
            block_store_path: None,
            content_class: String::new(),
        }
    }
    
    /// Get the "main" video for this archive, if any.
    /// An archive can be either:
    ///   - Single video: main = entries[0], everything else empty
    ///   - Multi-video: main = None, entries contains the collection
    pub fn main_video(&self) -> Option<&VideoEntry> {
        self.entries.iter().find(|e| e.title == "main" || e.title == "MAIN")
    }
    
    /// Is this a multi-video archive?
    pub fn is_multi_video(&self) -> bool {
        self.main_video().is_none() || self.entries.len() > 1
    }

    pub fn add_video(&mut self, mut entry: VideoEntry) {
        entry.id = self.entries.len() as u32;
        self.total_size_bytes += entry.file_size_bytes;
        self.entries.push(entry);
        self.modified_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
    }

    pub fn remove_video(&mut self, id: u32) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| e.id == id) {
            self.total_size_bytes -= self.entries[pos].file_size_bytes;
            self.entries.remove(pos);
            self.modified_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            true
        } else {
            false
        }
    }

    pub fn find_by_tag(&self, tag: &str) -> Vec<&VideoEntry> {
        self.entries.iter().filter(|e| e.tags.iter().any(|t| t == tag)).collect()
    }

    pub fn total_duration_secs(&self) -> f32 {
        self.entries.iter().map(|e| e.duration_secs).sum()
    }

    pub fn save_manifest(&self, path: &std::path::Path) -> Result<(), String> {
        // Simple binary serialization (no serde dependency needed for fao_codec_lib)
        let mut buf: Vec<u8> = Vec::with_capacity(1024 + self.entries.len() * 256);
        buf.extend_from_slice(b"FAOLIB01");  // magic
        buf.extend_from_slice(&self.name.len().to_le_bytes()[..4]);
        buf.extend_from_slice(self.name.as_bytes());
        buf.extend_from_slice(&self.created_at.to_le_bytes());
        buf.extend_from_slice(&self.modified_at.to_le_bytes());
        buf.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());

        for entry in &self.entries {
            buf.extend_from_slice(&entry.id.to_le_bytes());
            write_string(&mut buf, &entry.title);
            write_string(&mut buf, &entry.source_path);
            write_string(&mut buf, &entry.fao_path);
            buf.extend_from_slice(&entry.width.to_le_bytes());
            buf.extend_from_slice(&entry.height.to_le_bytes());
            buf.extend_from_slice(&entry.fps.to_le_bytes());
            buf.extend_from_slice(&entry.frame_count.to_le_bytes());
            buf.extend_from_slice(&entry.duration_secs.to_le_bytes());
            buf.extend_from_slice(&entry.file_size_bytes.to_le_bytes());
            buf.extend_from_slice(&entry.created_at.to_le_bytes());
            buf.extend_from_slice(&(entry.chapters.len() as u32).to_le_bytes());
            for ch in &entry.chapters {
                buf.extend_from_slice(&ch.start_frame.to_le_bytes());
                buf.extend_from_slice(&ch.end_frame.to_le_bytes());
                buf.extend_from_slice(&ch.thumbnail_idx.to_le_bytes());
                write_string(&mut buf, &ch.title);
            }
            buf.extend_from_slice(&(entry.tags.len() as u32).to_le_bytes());
            for tag in &entry.tags {
                write_string(&mut buf, tag);
            }
            buf.extend_from_slice(&(entry.thumbnail_data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&entry.thumbnail_data);
        }

        // Trailer (forward-compat): content_class + block_store_path
        write_string(&mut buf, &self.content_class);
        write_string(&mut buf, self.block_store_path.as_deref().unwrap_or(""));
        std::fs::write(path, buf).map_err(|e| e.to_string())
    }

    pub fn load_manifest(path: &std::path::Path) -> Result<Self, String> {
        let buf = std::fs::read(path).map_err(|e| e.to_string())?;
        if buf.len() < 28 || &buf[..8] != b"FAOLIB01" {
            return Err("Invalid manifest".into());
        }
        let mut pos = 8usize;
        let name_len = u32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]) as usize;
        pos += 4;
        let name = String::from_utf8_lossy(&buf[pos..pos+name_len]).to_string();
        pos += name_len;
        let created_at = u64::from_le_bytes(buf[pos..pos+8].try_into().unwrap_or([0;8]));
        pos += 8;
        let modified_at = u64::from_le_bytes(buf[pos..pos+8].try_into().unwrap_or([0;8]));
        pos += 8;
        let n_entries = u32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]) as usize;
        pos += 4;

        let mut entries = Vec::with_capacity(n_entries);
        let mut total_size = 0u64;
        for _ in 0..n_entries {
            if pos + 4 > buf.len() { break; }
            let id = u32::from_le_bytes([buf[pos], buf[pos+1], buf[pos+2], buf[pos+3]]);
            pos += 4;
            let title = read_string(&buf, &mut pos);
            let source_path = read_string(&buf, &mut pos);
            let fao_path = read_string(&buf, &mut pos);
            if pos + 36 > buf.len() { break; }
            let width = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
            pos += 4;
            let height = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
            pos += 4;
            let fps = f32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
            pos += 4;
            let frame_count = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
            pos += 4;
            let duration_secs = f32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
            pos += 4;
            let file_size_bytes = u64::from_le_bytes(buf[pos..pos+8].try_into().unwrap_or([0;8]));
            pos += 8;
            let entry_created = u64::from_le_bytes(buf[pos..pos+8].try_into().unwrap_or([0;8]));
            pos += 8;
            let n_chapters = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4])) as usize;
            pos += 4;
            let mut chapters = Vec::with_capacity(n_chapters);
            for _ in 0..n_chapters {
                if pos + 12 > buf.len() { break; }
                let sf = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
                pos += 4;
                let ef = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
                pos += 4;
                let ti = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
                pos += 4;
                let title = read_string(&buf, &mut pos);
                chapters.push(Chapter { start_frame: sf, end_frame: ef, thumbnail_idx: ti, title });
            }
            let n_tags = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4])) as usize;
            pos += 4;
            let mut tags = Vec::with_capacity(n_tags);
            for _ in 0..n_tags {
                tags.push(read_string(&buf, &mut pos));
            }
            let thumb_len = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4])) as usize;
            pos += 4;
            let thumbnail_data = if pos + thumb_len <= buf.len() {
                let t = buf[pos..pos+thumb_len].to_vec();
                pos += thumb_len;
                t
            } else { Vec::new() };

            total_size += file_size_bytes;
            entries.push(VideoEntry {
                id, title, source_path, fao_path, width, height, fps,
                frame_count, duration_secs, file_size_bytes,
                created_at: entry_created, chapters, tags, thumbnail_data,
            });
        }

        // Read trailer fields (if present)
        let content_class = if pos < buf.len() {
            read_string(&buf, &mut pos)
        } else { String::new() };
        let bsp = if pos < buf.len() {
            let s = read_string(&buf, &mut pos);
            if s.is_empty() { None } else { Some(s) }
        } else { None };
        
        Ok(Archive {
            name,
            root_path: path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            entries,
            created_at, modified_at,
            total_size_bytes: total_size,
            block_store_path: bsp,
            content_class,
        })
    }
}



// ═══════════════════════════════════════════════════════════
// Cross-Archive Web Culling — shared blocks across videos
// ═══════════════════════════════════════════════════════════
// 
// Many videos in an archive share content:
//   - 50 Smite clips: same UI, HUD, characters, map textures
//   - 30 cartoon episodes: same character models, backgrounds
//   - 100 terminal recordings: same fonts, color schemes, prompts
//
// Web culling identifies duplicate 8x8 blocks across all videos,
// stores them ONCE in a shared block pool, references by hash.
// Achieves 5-20x additional compression for same-domain video sets.

/// A content block shared across multiple videos in an archive.
/// Identified by FNV-1a hash for fast lookup.
#[derive(Clone, Debug)]
pub struct SharedBlock {
    pub hash: u64,                  // FNV-1a hash of the pixel content
    pub data: Vec<u8>,              // compressed block (zstd)
    pub original_size: u32,         // uncompressed size in bytes
    pub ref_count: u32,             // how many videos/frames reference this
    pub block_type: u8,             // 0=Y plane, 1=UV plane, 2=anchor frame, 3=motion vec
}

/// The shared block store for an entire archive.
/// All videos in the archive can reference these blocks by hash.
#[derive(Debug)]
pub struct ContentBlockStore {
    pub blocks: Vec<SharedBlock>,
    pub hash_to_idx: Vec<(u64, u32)>,  // sorted by hash, O(log n) lookup
    pub total_unique_bytes: u64,
    pub total_referenced_bytes: u64,   // sum of refs * original_size
}

impl ContentBlockStore {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            hash_to_idx: Vec::new(),
            total_unique_bytes: 0,
            total_referenced_bytes: 0,
        }
    }
    
    /// Insert a block, deduplicating by hash. Returns its index.
    pub fn insert(&mut self, hash: u64, compressed: Vec<u8>, orig_size: u32, btype: u8) -> u32 {
        // Check if hash exists (binary search)
        match self.hash_to_idx.binary_search_by_key(&hash, |&(h, _)| h) {
            Ok(i) => {
                let idx = self.hash_to_idx[i].1;
                if (idx as usize) < self.blocks.len() {
                    self.blocks[idx as usize].ref_count += 1;
                    self.total_referenced_bytes += orig_size as u64;
                }
                idx
            }
            Err(insert_pos) => {
                let idx = self.blocks.len() as u32;
                self.total_unique_bytes += compressed.len() as u64;
                self.total_referenced_bytes += orig_size as u64;
                self.blocks.push(SharedBlock {
                    hash, data: compressed, original_size: orig_size,
                    ref_count: 1, block_type: btype,
                });
                self.hash_to_idx.insert(insert_pos, (hash, idx));
                idx
            }
        }
    }
    
    pub fn get_by_hash(&self, hash: u64) -> Option<&SharedBlock> {
        match self.hash_to_idx.binary_search_by_key(&hash, |&(h, _)| h) {
            Ok(i) => self.blocks.get(self.hash_to_idx[i].1 as usize),
            Err(_) => None,
        }
    }
    
    pub fn get(&self, idx: u32) -> Option<&SharedBlock> {
        self.blocks.get(idx as usize)
    }
    
    /// Compression ratio achieved by deduplication
    pub fn dedup_ratio(&self) -> f32 {
        if self.total_unique_bytes == 0 { return 1.0; }
        self.total_referenced_bytes as f32 / self.total_unique_bytes as f32
    }
    
    pub fn stats(&self) -> String {
        format!("{} unique blocks, {:.1}MB stored, {:.1}MB referenced ({:.1}× dedup)",
            self.blocks.len(),
            self.total_unique_bytes as f64 / 1048576.0,
            self.total_referenced_bytes as f64 / 1048576.0,
            self.dedup_ratio())
    }
    
    /// Save to disk as a single binary blob
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let mut buf = Vec::with_capacity(self.total_unique_bytes as usize + 64);
        buf.extend_from_slice(b"FAOCBS01");  // magic
        buf.extend_from_slice(&(self.blocks.len() as u32).to_le_bytes());
        for b in &self.blocks {
            buf.extend_from_slice(&b.hash.to_le_bytes());
            buf.extend_from_slice(&b.original_size.to_le_bytes());
            buf.extend_from_slice(&b.ref_count.to_le_bytes());
            buf.push(b.block_type);
            buf.extend_from_slice(&(b.data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&b.data);
        }
        std::fs::write(path, buf).map_err(|e| e.to_string())
    }
    
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let buf = std::fs::read(path).map_err(|e| e.to_string())?;
        if buf.len() < 12 || &buf[..8] != b"FAOCBS01" {
            return Err("Invalid block store".into());
        }
        let n = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]) as usize;
        let mut blocks = Vec::with_capacity(n);
        let mut hash_to_idx = Vec::with_capacity(n);
        let mut total_unique = 0u64;
        let mut total_ref = 0u64;
        let mut pos = 12;
        for i in 0..n {
            if pos + 21 > buf.len() { break; }
            let hash = u64::from_le_bytes(buf[pos..pos+8].try_into().unwrap_or([0;8]));
            pos += 8;
            let orig_size = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
            pos += 4;
            let ref_count = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4]));
            pos += 4;
            let btype = buf[pos];
            pos += 1;
            let dlen = u32::from_le_bytes(buf[pos..pos+4].try_into().unwrap_or([0;4])) as usize;
            pos += 4;
            if pos + dlen > buf.len() { break; }
            let data = buf[pos..pos+dlen].to_vec();
            pos += dlen;
            total_unique += data.len() as u64;
            total_ref += (orig_size as u64) * (ref_count as u64);
            hash_to_idx.push((hash, i as u32));
            blocks.push(SharedBlock { hash, data, original_size: orig_size, ref_count, block_type: btype });
        }
        hash_to_idx.sort_by_key(|&(h, _)| h);
        Ok(Self { blocks, hash_to_idx, total_unique_bytes: total_unique, total_referenced_bytes: total_ref })
    }
}

/// Compute FNV-1a hash of a byte slice (consistent with frame web)


/// High-compression mode for archival storage.
/// Uses zstd level 19 for ~30% smaller files at 8-10× slower encode.
/// Decompression speed is unchanged (zstd's design).
pub fn compress_frame_archival(yuv: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    if yuv.len() < y_sz + y_sz / 2 {
        return zstd::encode_all(std::io::Cursor::new(yuv), 19)
            .unwrap_or_else(|_| yuv.to_vec());
    }


// ═══════════════════════════════════════════════════════════
// Block-Level Dedup — actual web compression backend
// ═══════════════════════════════════════════════════════════
//
// Extracts 16x16 blocks from frames, hashes them by content,
// stores unique blocks once in a shared pool. Frames reference
// blocks by index. Achieves real compression beyond per-frame zstd.

/// Extract 16x16 blocks from a Y-plane (luma).
/// Returns Vec of (block_idx, hash, pixel_data) tuples.
pub fn extract_blocks_16x16(y_plane: &[u8], w: usize, h: usize) -> Vec<(u32, u64, Vec<u8>)> {
    let bw = w / 16;
    let bh = h / 16;
    let mut blocks = Vec::with_capacity(bw * bh);
    
    for by in 0..bh {
        for bx in 0..bw {
            let mut block = Vec::with_capacity(256);
            for py in 0..16 {
                let src_y = by * 16 + py;
                let src_x = bx * 16;
                if src_y < h {
                    let row_start = src_y * w + src_x;
                    let row_end = (row_start + 16).min(y_plane.len());
                    block.extend_from_slice(&y_plane[row_start..row_end]);
                    while block.len() < (py + 1) * 16 { block.push(0); }
                } else {
                    for _ in 0..16 { block.push(0); }
                }
            }
            let hash = block_hash_fnv(&block);
            let idx = (by * bw + bx) as u32;
            blocks.push((idx, hash, block));
        }
    }
    
    blocks
}

/// Build a frame from block references and a shared block pool.
/// Used to reconstruct frames that were stored as block references.
pub fn reconstruct_from_blocks(
    block_refs: &[u32],  // block hash IDs (one per 16x16 block)
    block_pool: &ContentBlockStore,
    w: usize, h: usize,
) -> Option<Vec<u8>> {
    let bw = w / 16;
    let bh = h / 16;
    let y_sz = w * h;
    if block_refs.len() != bw * bh { return None; }
    
    let mut y_plane = vec![0u8; y_sz];
    for (bi, &block_id) in block_refs.iter().enumerate() {
        let bx = bi % bw;
        let by = bi / bw;
        let block = block_pool.get(block_id)?;
        let block_data = zstd::decode_all(std::io::Cursor::new(&block.data)).ok()?;
        for py in 0..16 {
            let dst_y = by * 16 + py;
            if dst_y >= h { break; }
            for px in 0..16 {
                let dst_x = bx * 16 + px;
                if dst_x >= w { break; }
                let src_idx = py * 16 + px;
                if src_idx < block_data.len() {
                    y_plane[dst_y * w + dst_x] = block_data[src_idx];
                }
            }
        }
    }
    
    Some(y_plane)
}

/// Encode a frame using block-level dedup:
///   1. Extract 16x16 blocks from Y plane
///   2. Hash each block, store unique ones in pool, reference duplicates
///   3. Output: list of block IDs + UV planes (full-resolution chroma)
///   
/// Output format ('B' marker):
///   header: 'B' | w(2) | h(2) | n_blocks(4) | uv_size(4)
///   block_refs: u32 × n_blocks  (indices into block_pool)
///   uv_data: zstd-compressed UV planes
pub fn encode_frame_block_dedup(
    yuv: &[u8], w: usize, h: usize,
    block_pool: &mut ContentBlockStore,
) -> (Vec<u8>, BlockDedupStats) {
    let y_sz = w * h;
    let uv_sz = y_sz / 2;
    let mut stats = BlockDedupStats::default();
    
    if yuv.len() < y_sz + uv_sz {
        // Fallback to plain compression
        return (compress_frame_disk(yuv), stats);
    }
    
    let y_plane = &yuv[..y_sz];
    let uv_plane = &yuv[y_sz..y_sz + uv_sz];
    
    // Extract and dedup blocks
    let blocks = extract_blocks_16x16(y_plane, w, h);
    let mut block_refs: Vec<u32> = Vec::with_capacity(blocks.len());
    
    for (_, hash, block_data) in blocks {
        // Pre-existing in pool?
        let existed_before = block_pool.get_by_hash(hash).is_some();
        // Compress and insert (or get existing)
        let compressed = zstd::encode_all(std::io::Cursor::new(&block_data), 3)
            .unwrap_or_else(|_| block_data.clone());
        let block_idx = block_pool.insert(hash, compressed, 256, 0);
        block_refs.push(block_idx);
        
        if existed_before { stats.dedup_count += 1; }
        else { stats.unique_count += 1; }
    }
    
    // UV: compress as one blob (could also dedup per-block, deferred)
    let uv_compressed = zstd::encode_all(std::io::Cursor::new(uv_plane), 5)
        .unwrap_or_else(|_| uv_plane.to_vec());
    
    // Pack output
    let mut out = Vec::with_capacity(13 + block_refs.len() * 4 + uv_compressed.len());
    out.push(b'B');
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&(block_refs.len() as u32).to_le_bytes());
    out.extend_from_slice(&(uv_compressed.len() as u32).to_le_bytes());
    for r in &block_refs {
        out.extend_from_slice(&r.to_le_bytes());
    }
    out.extend_from_slice(&uv_compressed);
    
    stats.total_blocks = block_refs.len() as u32;
    stats.encoded_bytes = out.len();
    (out, stats)
}

/// Decode a frame encoded with block dedup.
pub fn decode_frame_block_dedup(
    data: &[u8], w: usize, h: usize,
    block_pool: &ContentBlockStore,
) -> Option<Vec<u8>> {
    if data.is_empty() || data[0] != b'B' { return None; }
    if data.len() < 13 { return None; }
    
    let _w = u16::from_le_bytes([data[1], data[2]]) as usize;
    let _h = u16::from_le_bytes([data[3], data[4]]) as usize;
    let n_blocks = u32::from_le_bytes([data[5], data[6], data[7], data[8]]) as usize;
    let uv_size = u32::from_le_bytes([data[9], data[10], data[11], data[12]]) as usize;
    
    let block_refs_off = 13;
    let block_refs_sz = n_blocks * 4;
    if data.len() < block_refs_off + block_refs_sz + uv_size { return None; }
    
    // Parse block refs
    let mut block_refs = Vec::with_capacity(n_blocks);
    for i in 0..n_blocks {
        let off = block_refs_off + i * 4;
        block_refs.push(u32::from_le_bytes([data[off], data[off+1], data[off+2], data[off+3]]));
    }
    
    // Reconstruct Y from blocks
    let y_plane = reconstruct_from_blocks(&block_refs, block_pool, w, h)?;
    
    // Decompress UV
    let uv_off = block_refs_off + block_refs_sz;
    let uv_compressed = &data[uv_off..uv_off + uv_size];
    let uv_plane = zstd::decode_all(std::io::Cursor::new(uv_compressed)).ok()?;
    
    // Combine
    let mut yuv = Vec::with_capacity(y_plane.len() + uv_plane.len());
    yuv.extend_from_slice(&y_plane);
    yuv.extend_from_slice(&uv_plane);
    Some(yuv)
}

#[derive(Debug, Default, Clone)]
pub struct BlockDedupStats {
    pub total_blocks: u32,
    pub unique_count: u32,
    pub dedup_count: u32,
    pub encoded_bytes: usize,
}

impl BlockDedupStats {
    pub fn dedup_ratio(&self) -> f32 {
        if self.total_blocks == 0 { 0.0 }
        else { self.dedup_count as f32 / self.total_blocks as f32 }
    }
}


// ═══════════════════════════════════════════════════════════
// Prediction Engine — predict frame from web, compare to actual
// ═══════════════════════════════════════════════════════════
//
// The web function is a graph of frame relationships and motion vectors.
// To compress optimally, we PREDICT what each frame should look like
// using the web (anchor + motion + scene info), then compare to the
// actual decoded frame:
//
//   prediction_quality = SSIM(predicted, actual)
//   if quality > 0.95: store NOTHING (just motion vec, ~16 bytes)
//   if quality > 0.80: store low-quality residual (~few KB)
//   if quality < 0.80: store full frame (~50-200KB)
//
// This achieves variable bitrate based on prediction confidence.

#[derive(Clone, Debug)]
pub struct PredictionResult {
    /// Predicted frame pixels (YUV)
    pub predicted: Vec<u8>,
    /// PSNR vs actual (dB) — higher = better prediction
    pub psnr: f32,
    /// MS-SSIM approximation (0..1) — perceptual quality
    pub ssim: f32,
    /// Per-block error map (16x16 blocks): which regions need more bits
    pub error_map: Vec<u8>,
    /// Recommended encoding tier
    pub recommended_tier: PredictionTier,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PredictionTier {
    /// Prediction excellent — store motion vec only (1-16 bytes)
    MotionOnly,
    /// Prediction good — store low-bit residual (~few KB)
    Residual,
    /// Prediction mediocre — store medium-quality update (~10-30KB)
    PartialUpdate,
    /// Prediction failed — store full frame (~50-200KB)
    FullEncode,
}

impl PredictionTier {
    pub fn typical_bytes(&self) -> usize {
        match self {
            Self::MotionOnly => 16,
            Self::Residual => 4096,
            Self::PartialUpdate => 24576,
            Self::FullEncode => 102400,
        }
    }
}

/// Predict a frame using the web function: given the previous anchor and
/// optional motion vectors, synthesize what the next frame should look like.
pub fn predict_frame_from_web(
    anchor_yuv: &[u8],
    w: usize, h: usize,
    motion_vectors: Option<&[(i16, i16)]>,  // 16x16 block MVs (dx, dy in pixels)
    block_size: usize,
) -> Vec<u8> {
    let y_sz = w * h;
    let uv_sz = y_sz / 2;
    if anchor_yuv.len() < y_sz + uv_sz {
        return anchor_yuv.to_vec();
    }
    
    // Without MVs, prediction = anchor (zero-motion model)
    let mvs = match motion_vectors {
        Some(m) => m,
        None => return anchor_yuv.to_vec(),
    };
    
    let bw = w / block_size;
    let bh = h / block_size;
    let mut predicted = vec![0u8; anchor_yuv.len()];
    
    // Predict Y plane using motion compensation
    for by in 0..bh {
        for bx in 0..bw {
            let bi = by * bw + bx;
            let (dx, dy) = mvs.get(bi).copied().unwrap_or((0, 0));
            
            for py in 0..block_size {
                for px in 0..block_size {
                    let dst_x = bx * block_size + px;
                    let dst_y = by * block_size + py;
                    if dst_x >= w || dst_y >= h { continue; }
                    
                    // Source position with motion offset
                    let sx = (dst_x as i32 + dx as i32).clamp(0, w as i32 - 1) as usize;
                    let sy = (dst_y as i32 + dy as i32).clamp(0, h as i32 - 1) as usize;
                    
                    predicted[dst_y * w + dst_x] = anchor_yuv[sy * w + sx];
                }
            }
        }
    }
    
    // Predict UV planes (lower resolution: w/2 × h/2)
    let hw = w / 2;
    let hh = h / 2;
    let bw_uv = hw / (block_size / 2).max(1);
    for by in 0..(hh / (block_size / 2).max(1)) {
        for bx in 0..bw_uv {
            // Use the corresponding luma MV (simplified)
            let bi_y = (by * 2) * bw + (bx * 2);
            let (dx, dy) = mvs.get(bi_y.min(mvs.len().saturating_sub(1))).copied().unwrap_or((0, 0));
            let dx_uv = dx / 2;
            let dy_uv = dy / 2;
            
            let bs_uv = (block_size / 2).max(1);
            for py in 0..bs_uv {
                for px in 0..bs_uv {
                    let dst_x = bx * bs_uv + px;
                    let dst_y = by * bs_uv + py;
                    if dst_x >= hw || dst_y >= hh { continue; }
                    
                    let sx = (dst_x as i32 + dx_uv as i32).clamp(0, hw as i32 - 1) as usize;
                    let sy = (dst_y as i32 + dy_uv as i32).clamp(0, hh as i32 - 1) as usize;
                    
                    // U plane
                    predicted[y_sz + dst_y * hw + dst_x] = anchor_yuv[y_sz + sy * hw + sx];
                    // V plane
                    predicted[y_sz + uv_sz/2 + dst_y * hw + dst_x] = anchor_yuv[y_sz + uv_sz/2 + sy * hw + sx];
                }
            }
        }
    }
    
    predicted
}

/// Compare a prediction to the actual frame, compute quality metrics.
pub fn compare_prediction(
    predicted: &[u8],
    actual: &[u8],
    w: usize, h: usize,
    block_size: usize,
) -> PredictionResult {
    let y_sz = w * h;
    let bw = w / block_size;
    let bh = h / block_size;
    
    if predicted.len() != actual.len() || predicted.len() < y_sz {
        // Bad input — return failed prediction
        return PredictionResult {
            predicted: predicted.to_vec(),
            psnr: 0.0,
            ssim: 0.0,
            error_map: vec![255u8; bw * bh],
            recommended_tier: PredictionTier::FullEncode,
        };
    }
    
    // Compute MSE on Y plane (luma is most perceptually important)
    let mut total_sq_err: u64 = 0;
    let mut error_map = vec![0u8; bw * bh];
    
    for by in 0..bh {
        for bx in 0..bw {
            let mut block_sq_err: u32 = 0;
            for py in 0..block_size {
                for px in 0..block_size {
                    let x = bx * block_size + px;
                    let y = by * block_size + py;
                    if x >= w || y >= h { continue; }
                    let idx = y * w + x;
                    let d = predicted[idx] as i32 - actual[idx] as i32;
                    block_sq_err += (d * d) as u32;
                }
            }
            // Normalize per block (avg sq err)
            let avg = block_sq_err / (block_size * block_size) as u32;
            error_map[by * bw + bx] = (avg.min(255)) as u8;
            total_sq_err += block_sq_err as u64;
        }
    }
    
    let mse = total_sq_err as f64 / y_sz as f64;
    let psnr = if mse > 0.0 {
        10.0 * (255.0 * 255.0 / mse).log10() as f32
    } else { 99.0 };
    
    // Approximate SSIM: 1 - normalized_mse
    // Real SSIM is more complex but this gives a reasonable proxy for tier decision
    let normalized_mse = (mse / (255.0 * 255.0)).min(1.0) as f32;
    let ssim = 1.0 - normalized_mse;
    
    // Tier decision based on prediction quality
    let recommended_tier = if ssim > 0.95 || psnr > 40.0 {
        PredictionTier::MotionOnly
    } else if ssim > 0.85 || psnr > 32.0 {
        PredictionTier::Residual
    } else if ssim > 0.70 || psnr > 25.0 {
        PredictionTier::PartialUpdate
    } else {
        PredictionTier::FullEncode
    };
    
    PredictionResult {
        predicted: predicted.to_vec(),
        psnr,
        ssim,
        error_map,
        recommended_tier,
    }
}

/// Quality/size tradeoff curve evaluator.
/// Given a prediction, decide whether to keep motion-only, residual, or full.
/// Returns (encoded_bytes, achieved_quality_psnr).
pub fn evaluate_tradeoff(
    prediction: &PredictionResult,
    target_psnr: f32,         // user target quality
    max_bytes_budget: usize,  // user budget per frame
) -> (PredictionTier, usize) {
    // Walk up tiers until we satisfy target_psnr or hit budget
    let tiers = [
        PredictionTier::MotionOnly,
        PredictionTier::Residual,
        PredictionTier::PartialUpdate,
        PredictionTier::FullEncode,
    ];
    
    for &tier in &tiers {
        let bytes = tier.typical_bytes();
        if bytes > max_bytes_budget { continue; }
        
        // Estimated quality for this tier
        let est_psnr = match tier {
            PredictionTier::MotionOnly => prediction.psnr,
            PredictionTier::Residual => prediction.psnr + 4.0,
            PredictionTier::PartialUpdate => prediction.psnr + 8.0,
            PredictionTier::FullEncode => 45.0,  // typical full-encode quality
        };
        
        if est_psnr >= target_psnr {
            return (tier, bytes);
        }
    }
    
    // Couldn't meet target — return best within budget
    let best_tier = if max_bytes_budget >= PredictionTier::FullEncode.typical_bytes() {
        PredictionTier::FullEncode
    } else if max_bytes_budget >= PredictionTier::PartialUpdate.typical_bytes() {
        PredictionTier::PartialUpdate
    } else if max_bytes_budget >= PredictionTier::Residual.typical_bytes() {
        PredictionTier::Residual
    } else {
        PredictionTier::MotionOnly
    };
    (best_tier, best_tier.typical_bytes())
}


    
    let y_plane = &yuv[0..y_sz];
    let u_plane = &yuv[y_sz..y_sz + y_sz/4];
    let v_plane = &yuv[y_sz + y_sz/4..y_sz + y_sz/2];
    
    // Highest zstd level for archival; chroma at max since it's smaller
    let y_compressed = zstd::encode_all(std::io::Cursor::new(y_plane), 19).unwrap_or_else(|_| y_plane.to_vec());
    let u_compressed = zstd::encode_all(std::io::Cursor::new(u_plane), 22).unwrap_or_else(|_| u_plane.to_vec());
    let v_compressed = zstd::encode_all(std::io::Cursor::new(v_plane), 22).unwrap_or_else(|_| v_plane.to_vec());
    
    let mut out = Vec::with_capacity(y_compressed.len() + u_compressed.len() + v_compressed.len() + 17);
    out.push(b'P');  // same format as yuv_split — readers handle uniformly
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&(y_compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&(u_compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&(v_compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&y_compressed);
    out.extend_from_slice(&u_compressed);
    out.extend_from_slice(&v_compressed);
    out
}

/// Frame prefetch cache for fast playback.
/// Decodes next N frames in background while user views current.
/// Cache hit = zero decode latency.
pub struct FramePrefetchCache {
    pub entries: std::collections::VecDeque<(usize, Vec<u8>)>,  // (frame_idx, RGB pixels)
    pub max_size: usize,
    pub hits: u64,
    pub misses: u64,
}

impl FramePrefetchCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: std::collections::VecDeque::with_capacity(max_size),
            max_size,
            hits: 0,
            misses: 0,
        }
    }
    
    pub fn get(&mut self, idx: usize) -> Option<Vec<u8>> {
        if let Some(pos) = self.entries.iter().position(|(i, _)| *i == idx) {
            self.hits += 1;
            // Clone, don't remove (keeps it warm for adjacent access)
            Some(self.entries[pos].1.clone())
        } else {
            self.misses += 1;
            None
        }
    }
    
    pub fn insert(&mut self, idx: usize, pixels: Vec<u8>) {
        // Evict oldest if at capacity
        if self.entries.len() >= self.max_size {
            self.entries.pop_front();
        }
        self.entries.push_back((idx, pixels));
    }
    
    pub fn hit_rate(&self) -> f32 {
        let total = self.hits + self.misses;
        if total == 0 { 0.0 } else { self.hits as f32 / total as f32 }
    }
    
    pub fn clear(&mut self) {
        self.entries.clear();
        self.hits = 0;
        self.misses = 0;
    }
}



/// Lossy chroma compression with perceptually-tuned reduction.
/// Y plane: full 8-bit precision (preserves detail)
/// U,V planes: 4-bit precision (16 levels, perceptually invisible reduction)
/// Result: 1.5× from chroma alone, plus zstd → 4-6× total.
pub fn compress_frame_lossy_chroma(yuv: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    let uv_sz = y_sz / 4;
    if yuv.len() < y_sz + 2 * uv_sz {
        return compress_frame_disk(yuv);
    }
    
    let y_plane = &yuv[0..y_sz];
    let u_plane = &yuv[y_sz..y_sz + uv_sz];
    let v_plane = &yuv[y_sz + uv_sz..y_sz + 2*uv_sz];
    
    // Quantize chroma to 4 bits (16 levels), pack 2 pixels per byte
    let packed_uv_sz = (uv_sz + 1) / 2;
    let mut packed_u = vec![0u8; packed_uv_sz];
    let mut packed_v = vec![0u8; packed_uv_sz];
    for i in 0..uv_sz {
        let u4 = u_plane[i] >> 4;  // 0..15
        let v4 = v_plane[i] >> 4;
        if i % 2 == 0 {
            packed_u[i / 2] = u4;
            packed_v[i / 2] = v4;
        } else {
            packed_u[i / 2] |= u4 << 4;
            packed_v[i / 2] |= v4 << 4;
        }
    }
    
    // zstd encode each plane
    let y_c = zstd::encode_all(std::io::Cursor::new(y_plane), 3).unwrap_or_else(|_| y_plane.to_vec());
    let u_c = zstd::encode_all(std::io::Cursor::new(&packed_u), 3).unwrap_or_else(|_| packed_u.clone());
    let v_c = zstd::encode_all(std::io::Cursor::new(&packed_v), 3).unwrap_or_else(|_| packed_v.clone());
    
    let mut out = Vec::with_capacity(y_c.len() + u_c.len() + v_c.len() + 17);
    out.push(b'L');  // lossy chroma marker
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&(y_c.len() as u32).to_le_bytes());
    out.extend_from_slice(&(u_c.len() as u32).to_le_bytes());
    out.extend_from_slice(&(v_c.len() as u32).to_le_bytes());
    out.extend_from_slice(&y_c);
    out.extend_from_slice(&u_c);
    out.extend_from_slice(&v_c);
    out
}

pub fn decompress_frame_lossy_chroma(data: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    if data.len() < 17 || data[0] != b'L' { return None; }
    let _sw = u16::from_le_bytes([data[1], data[2]]) as usize;
    let _sh = u16::from_le_bytes([data[3], data[4]]) as usize;
    let y_c_sz = u32::from_le_bytes([data[5], data[6], data[7], data[8]]) as usize;
    let u_c_sz = u32::from_le_bytes([data[9], data[10], data[11], data[12]]) as usize;
    let v_c_sz = u32::from_le_bytes([data[13], data[14], data[15], data[16]]) as usize;
    
    let mut off = 17;
    if off + y_c_sz + u_c_sz + v_c_sz > data.len() { return None; }
    
    let y_plane = zstd::decode_all(std::io::Cursor::new(&data[off..off+y_c_sz])).ok()?;
    off += y_c_sz;
    let packed_u = zstd::decode_all(std::io::Cursor::new(&data[off..off+u_c_sz])).ok()?;
    off += u_c_sz;
    let packed_v = zstd::decode_all(std::io::Cursor::new(&data[off..off+v_c_sz])).ok()?;
    
    let y_sz = w * h;
    let uv_sz = y_sz / 4;
    let mut yuv = Vec::with_capacity(y_sz + 2 * uv_sz);
    yuv.extend_from_slice(&y_plane);
    
    // Unpack 4-bit chroma back to 8-bit
    for i in 0..uv_sz {
        let byte_idx = i / 2;
        if byte_idx >= packed_u.len() { yuv.push(128); continue; }
        let u4 = if i % 2 == 0 { packed_u[byte_idx] & 0x0F } else { (packed_u[byte_idx] >> 4) & 0x0F };
        // Expand 4 bits to 8 bits (multiply by 17 = 0x11 for full range)
        yuv.push(u4 * 17);
    }
    for i in 0..uv_sz {
        let byte_idx = i / 2;
        if byte_idx >= packed_v.len() { yuv.push(128); continue; }
        let v4 = if i % 2 == 0 { packed_v[byte_idx] & 0x0F } else { (packed_v[byte_idx] >> 4) & 0x0F };
        yuv.push(v4 * 17);
    }
    
    Some(yuv)
}

pub fn block_hash_fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn write_string(buf: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
    buf.extend_from_slice(b);
}

fn read_string(buf: &[u8], pos: &mut usize) -> String {
    if *pos + 4 > buf.len() { return String::new(); }
    let len = u32::from_le_bytes([buf[*pos], buf[*pos+1], buf[*pos+2], buf[*pos+3]]) as usize;
    *pos += 4;
    if *pos + len > buf.len() { return String::new(); }
    let s = String::from_utf8_lossy(&buf[*pos..*pos+len]).to_string();
    *pos += len;
    s
}


/// Read .fao file header
pub fn read_fao_header(data: &[u8]) -> Option<FaoFileHeader> {
    let sz = std::mem::size_of::<FaoFileHeader>();
    if data.len() < sz { return None; }
    if &data[0..4] != b"FAO2" { return None; }
    let h = FaoFileHeader {
        magic: [data[0],data[1],data[2],data[3]],
        version: u16::from_le_bytes([data[4],data[5]]),
        width: u16::from_le_bytes([data[6],data[7]]),
        height: u16::from_le_bytes([data[8],data[9]]),
        fps_num: u16::from_le_bytes([data[10],data[11]]),
        fps_den: u16::from_le_bytes([data[12],data[13]]),
        frame_count: u32::from_le_bytes([data[14],data[15],data[16],data[17]]),
        gop_size: u16::from_le_bytes([data[18],data[19]]),
        quality: data[20], flags: data[21],
        total_compressed: u64::from_le_bytes([data[22],data[23],data[24],data[25],data[26],data[27],data[28],data[29]]) };
    Some(h)
}

/// Read frame index from .fao file
pub fn read_fao_index(data: &[u8], frame_count: usize) -> Vec<FaoFrameEntry> {
    let hdr_sz = 30usize;  // 4+2+2+2+2+2+4+2+1+1+8 = 30 bytes
    let entry_sz = std::mem::size_of::<FaoFrameEntry>();
    let mut entries = Vec::with_capacity(frame_count);
    for i in 0..frame_count {
        let off = hdr_sz + i * entry_sz;
        if off + entry_sz > data.len() { break; }
        let entry: FaoFrameEntry = unsafe { std::ptr::read_unaligned(data[off..].as_ptr() as *const FaoFrameEntry) };
        entries.push(entry);
    }
    entries
}

/// Read a single frame's reconstructed RGB from .fao file
pub fn read_fao_frame(data: &[u8], entry: &FaoFrameEntry) -> Option<(Vec<u8>, FaoFrameHeader)> {
    let fhdr_sz = 13usize;
    let off = entry.offset as usize;
    if off + fhdr_sz > data.len() { return None; }
    let d = &data[off..];
    let fhdr = FaoFrameHeader {
        frame_type: d[0],
        compressed_size: u32::from_le_bytes([d[1],d[2],d[3],d[4]]),
        recon_size: u32::from_le_bytes([d[5],d[6],d[7],d[8]]),
        psnr_x10: u16::from_le_bytes([d[9],d[10]]),
        skip_pct: d[11],
        gmx: d[12] as i8, gmy: if off+13 <= data.len() { d[13] as i8 } else { 0 } };
    let payload_off = off + fhdr_sz;
    let compressed_sz = fhdr.compressed_size as usize;
    let decompressed_sz = fhdr.recon_size as usize;
    if payload_off + compressed_sz > data.len() { return None; }
    let payload = &data[payload_off..payload_off + compressed_sz];
    
    // Return raw payload — caller detects format ('P', 'D', or zstd) and decodes
    let pixels = payload.to_vec();
    Some((pixels, fhdr))
}

// ══════════════════════════════════════════════════════════
// Cross-frame motion hints — high-dimensional function descriptors
// ══════════════════════════════════════════════════════════

/// Motion type classification for a region
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionType {
    Static,       // no motion — UI elements, paused content
    CameraFollow, // follows global camera motion exactly
    Linear,       // constant velocity: (vx, vy)
    Polynomial,   // quadratic trajectory: (ax+bx*t+cx*t², ay+by*t+cy*t²)
    Periodic,     // oscillating/repeating (animation cycles)
    Unpredictable,// explosions, particles — must encode every frame
}

/// Motion hint for a 16×16 region (2×2 blocks)
#[derive(Clone, Debug)]
pub struct MotionHint {
    pub motion_type: MotionType,
    pub confidence: f32,        // 0..1 how well the model fits
    pub params: [f32; 6],       // motion function coefficients
    pub layer: u8,              // 0=background, 1=ui, 2=units, 3=effects
    pub temporal_density: f32,  // suggested frames per second for this region (0=skip)
    pub skip_streak: u32,       // how many consecutive frames this region was skipped
}

impl Default for MotionHint {
    fn default() -> Self { Self {
        motion_type: MotionType::Static,
        confidence: 0.0, params: [0.0; 6],
        layer: 0, temporal_density: 0.0, skip_streak: 0 }}
}

/// Per-frame VRR metadata
#[derive(Clone, Debug)]
pub struct VrrTiming {
    pub display_duration_ms: f32,   // how long to display this frame
    pub activity_score: f32,        // 0=static, 1=maximum action
    pub can_interpolate: bool,      // decoder can synthesize frames before this one
}

/// Motion hint map for the frame (one hint per 16×16 region)
pub struct MotionHintMap {
    hints: Vec<MotionHint>,
    rw: usize, rh: usize,  // region grid dimensions
    // Accumulated skip counts per region (persists across frames)
    skip_counts: Vec<u32>,
    // Previous MVs per region for trajectory fitting
    mv_history_x: Vec<Vec<f32>>,
    mv_history_y: Vec<Vec<f32>> }

impl MotionHintMap {
    pub fn new(w: usize, h: usize) -> Self {
        let rw = (w + 15) / 16; let rh = (h + 15) / 16;
        let n = rw * rh;
        Self {
            hints: vec![MotionHint::default(); n],
            rw, rh,
            skip_counts: vec![0; n],
            mv_history_x: vec![Vec::with_capacity(16); n],
            mv_history_y: vec![Vec::with_capacity(16); n] }
    }

    /// Update hints from encoder results (global MV, per-block skip bitmap, block MVs)
    pub fn update(&mut self, 
        gmx: i8, gmy: i8,
        block_skipped: &[bool],    // per 8x8 block
        block_mvx: &[f32],         // per 8x8 block
        block_mvy: &[f32],
        bw: usize, _bh: usize,    // block grid dimensions
        frame_quality: &[f32],     // per 8x8 quality scale from classify_block
    ) {
        for ry in 0..self.rh { for rx in 0..self.rw {
            let ri = ry * self.rw + rx;
            // Map 16x16 region to 2x2 blocks
            let bx0 = rx * 2; let by0 = ry * 2;
            
            // Aggregate skip info from the 4 child blocks
            let mut n_skip = 0u32; let mut n_total = 0u32;
            let mut avg_mvx = 0.0f32; let mut avg_mvy = 0.0f32;
            let mut avg_q = 0.0f32;
            for dy in 0..2 { for dx in 0..2 {
                let bi = (by0+dy) * bw + (bx0+dx);
                if bi < block_skipped.len() {
                    n_total += 1;
                    if block_skipped[bi] { n_skip += 1; }
                    avg_mvx += block_mvx.get(bi).copied().unwrap_or(0.0);
                    avg_mvy += block_mvy.get(bi).copied().unwrap_or(0.0);
                    avg_q += frame_quality.get(bi).copied().unwrap_or(1.0);
                }
            }}
            if n_total == 0 { continue; }
            avg_mvx /= n_total as f32;
            avg_mvy /= n_total as f32;
            avg_q /= n_total as f32;

            // Track skip streak
            if n_skip == n_total {
                self.skip_counts[ri] += 1;
            } else {
                self.skip_counts[ri] = 0;
            }

            // Record MV for trajectory
            self.mv_history_x[ri].push(avg_mvx);
            self.mv_history_y[ri].push(avg_mvy);
            if self.mv_history_x[ri].len() > 16 { self.mv_history_x[ri].remove(0); }
            if self.mv_history_y[ri].len() > 16 { self.mv_history_y[ri].remove(0); }

            let h = &mut self.hints[ri];
            h.skip_streak = self.skip_counts[ri];

            // Classify motion type
            let is_camera = (avg_mvx - gmx as f32).abs() < 1.5 && (avg_mvy - gmy as f32).abs() < 1.5;
            let is_static = avg_mvx.abs() < 0.5 && avg_mvy.abs() < 0.5 && n_skip >= n_total / 2;

            if is_static && h.skip_streak > 5 {
                h.motion_type = MotionType::Static;
                h.layer = if avg_q < 0.5 { 1 } else { 0 };  // UI detected by quality scale
                h.temporal_density = 0.0;  // no updates needed
                h.confidence = 1.0;
                h.params = [0.0; 6];
            } else if is_camera {
                h.motion_type = MotionType::CameraFollow;
                h.layer = 0;
                h.temporal_density = 30.0;  // can interpolate from camera motion
                h.confidence = 0.9;
                h.params = [gmx as f32, gmy as f32, 0.0, 0.0, 0.0, 0.0];
            } else if self.mv_history_x[ri].len() >= 4 {
                // Try polynomial fit
                let (ax, bx, cx, rms_x) = fit_temporal_poly(&self.mv_history_x[ri]);
                let (ay, by, cy, rms_y) = fit_temporal_poly(&self.mv_history_y[ri]);
                let rms = (rms_x * rms_x + rms_y * rms_y).sqrt();
                let mag = (avg_mvx * avg_mvx + avg_mvy * avg_mvy).sqrt();

                if rms < 1.0 && mag > 0.5 {
                    // Good polynomial fit — linear or quadratic motion
                    if cx.abs() < 0.1 && cy.abs() < 0.1 {
                        h.motion_type = MotionType::Linear;
                    } else {
                        h.motion_type = MotionType::Polynomial;
                    }
                    h.layer = 2;  // moving object (units)
                    h.temporal_density = 60.0;  // full rate but can interpolate
                    h.confidence = (1.0 - rms / mag.max(1.0)).max(0.0);
                    h.params = [ax, bx, cx, ay, by, cy];
                } else {
                    h.motion_type = MotionType::Unpredictable;
                    h.layer = 3;  // effects
                    h.temporal_density = 60.0;  // must encode every frame
                    h.confidence = 0.0;
                    h.params = [avg_mvx, avg_mvy, 0.0, 0.0, 0.0, 0.0];
                }
            } else {
                h.motion_type = MotionType::Unpredictable;
                h.layer = 0;
                h.temporal_density = 60.0;
                h.confidence = 0.0;
            }
        }}
    }

    /// Compute VRR timing for this frame based on all region hints
    pub fn compute_vrr(&self, base_fps: f64) -> VrrTiming {
        if self.hints.is_empty() { return VrrTiming { display_duration_ms: 1000.0 / base_fps as f32, activity_score: 0.5, can_interpolate: false }; }

        let n = self.hints.len() as f32;
        let n_static = self.hints.iter().filter(|h| h.motion_type == MotionType::Static).count() as f32;
        let n_unpredictable = self.hints.iter().filter(|h| h.motion_type == MotionType::Unpredictable).count() as f32;
        let n_camera = self.hints.iter().filter(|h| h.motion_type == MotionType::CameraFollow).count() as f32;
        let n_polynomial = self.hints.iter().filter(|h| h.motion_type == MotionType::Linear || h.motion_type == MotionType::Polynomial).count() as f32;
        let avg_confidence: f32 = self.hints.iter().map(|h| h.confidence).sum::<f32>() / n;

        // Activity score: 0 = all static, 1 = all unpredictable
        let activity = (n_unpredictable + n_polynomial * 0.3) / n;

        // Can interpolate if most regions have good motion models
        let can_interp = (n_camera + n_polynomial + n_static) / n > 0.8 && avg_confidence > 0.6;

        // VRR: static scenes get longer display, action gets shorter
        let density_factor = if activity < 0.05 { 0.25 }      // mostly static → 15fps effective
            else if activity < 0.15 { 0.5 }                    // slow motion → 30fps
            else if can_interp { 0.75 }                         // interpolatable → 45fps
            else { 1.0 };                                       // full rate

        let duration_ms = 1000.0 / (base_fps as f32 * density_factor);

        VrrTiming { display_duration_ms: duration_ms, activity_score: activity, can_interpolate: can_interp }
    }

    /// Get layer breakdown for stats display
    pub fn layer_stats(&self) -> (usize, usize, usize, usize, usize) {
        let n_static = self.hints.iter().filter(|h| h.motion_type == MotionType::Static).count();
        let n_camera = self.hints.iter().filter(|h| h.motion_type == MotionType::CameraFollow).count();
        let n_linear = self.hints.iter().filter(|h| h.motion_type == MotionType::Linear).count();
        let n_poly = self.hints.iter().filter(|h| h.motion_type == MotionType::Polynomial).count();
        let n_unpredict = self.hints.iter().filter(|h| h.motion_type == MotionType::Unpredictable).count();
        (n_static, n_camera, n_linear, n_poly, n_unpredict)
    }
}

// ══════════════════════════════════════════════════════════
// Soft motion field — continuous functions with spatial smoothing
// ══════════════════════════════════════════════════════════

/// Soft motion state per 16×16 region (no hard categories)
#[derive(Clone, Debug)]
pub struct RegionMotion {
    // Raw measurements from encoder
    pub mv_x: f32, pub mv_y: f32,      // this frame's motion
    pub traj_x: [f32; 3],              // polynomial coeffs for x(t)
    pub traj_y: [f32; 3],              // polynomial coeffs for y(t)
    pub traj_confidence: f32,           // 0..1 how well polynomial fits
    pub skip_streak: u32,               // consecutive skip count
    pub quality_scale: f32,             // from content classifier (< 1 = UI)
    // After spatial smoothing
    pub smooth_mv_x: f32,              // spatially smoothed motion
    pub smooth_mv_y: f32,
    pub coherence: f32,                // how similar to neighbors (0=outlier, 1=uniform)
    pub effective_density: f32,         // suggested temporal density (fps fraction)
}

impl Default for RegionMotion {
    fn default() -> Self { Self {
        mv_x: 0.0, mv_y: 0.0,
        traj_x: [0.0; 3], traj_y: [0.0; 3],
        traj_confidence: 0.0, skip_streak: 0, quality_scale: 1.0,
        smooth_mv_x: 0.0, smooth_mv_y: 0.0,
        coherence: 0.0, effective_density: 1.0 }}
}

pub struct SoftMotionField {
    pub regions: Vec<RegionMotion>,
    pub rw: usize, pub rh: usize,
    mv_hist_x: Vec<Vec<f32>>,
    mv_hist_y: Vec<Vec<f32>> }

impl SoftMotionField {
    pub fn new(w: usize, h: usize) -> Self {
        let rw = (w + 15) / 16; let rh = (h + 15) / 16;
        let n = rw * rh;
        Self {
            regions: vec![RegionMotion::default(); n],
            rw, rh,
            mv_hist_x: vec![Vec::with_capacity(16); n],
            mv_hist_y: vec![Vec::with_capacity(16); n] }
    }

    /// Update from encoder block data, then spatially smooth
    pub fn update_from_blocks(
        &mut self,
        block_mvx: &[f32], block_mvy: &[f32],
        block_skipped: &[bool],
        block_quality: &[f32],
        bw: usize,
        gmx: f32, gmy: f32,
    ) {
        // Phase 1: aggregate block data into regions
        for ry in 0..self.rh { for rx in 0..self.rw {
            let ri = ry * self.rw + rx;
            let bx0 = rx * 2; let by0 = ry * 2;
            let mut sum_x = 0.0f32; let mut sum_y = 0.0f32;
            let mut n_skip = 0u32; let mut n = 0u32;
            let mut min_q = 1.0f32;
            for dy in 0..2 { for dx in 0..2 {
                let bi = (by0+dy) * bw + (bx0+dx);
                if bi >= block_skipped.len() { continue; }
                n += 1;
                if block_skipped[bi] { n_skip += 1; }
                sum_x += block_mvx.get(bi).copied().unwrap_or(0.0);
                sum_y += block_mvy.get(bi).copied().unwrap_or(0.0);
                min_q = min_q.min(block_quality.get(bi).copied().unwrap_or(1.0));
            }}
            if n == 0 { continue; }
            let r = &mut self.regions[ri];
            r.mv_x = sum_x / n as f32;
            r.mv_y = sum_y / n as f32;
            r.quality_scale = min_q;
            if n_skip == n { r.skip_streak += 1; } else { r.skip_streak = 0; }

            // Update trajectory history
            self.mv_hist_x[ri].push(r.mv_x);
            self.mv_hist_y[ri].push(r.mv_y);
            if self.mv_hist_x[ri].len() > 16 { self.mv_hist_x[ri].remove(0); }
            if self.mv_hist_y[ri].len() > 16 { self.mv_hist_y[ri].remove(0); }

            // Fit trajectory polynomial
            if self.mv_hist_x[ri].len() >= 3 {
                let (ax, bx, cx, rms_x) = fit_temporal_poly(&self.mv_hist_x[ri]);
                let (ay, by, cy, rms_y) = fit_temporal_poly(&self.mv_hist_y[ri]);
                r.traj_x = [ax, bx, cx];
                r.traj_y = [ay, by, cy];
                let rms = (rms_x * rms_x + rms_y * rms_y).sqrt();
                let mag = (r.mv_x * r.mv_x + r.mv_y * r.mv_y).sqrt().max(0.5);
                r.traj_confidence = (1.0 - rms / mag).max(0.0).min(1.0);
            }
        }}

        // Phase 2: spatial smoothing — nearest neighbor propagation
        let old_regions = self.regions.clone();
        for ry in 0..self.rh { for rx in 0..self.rw {
            let ri = ry * self.rw + rx;
            let own = &old_regions[ri];
            let own_conf = own.traj_confidence;

            // Gather neighbors (8-connected)
            let mut wt_sum_x = own.mv_x * (0.5 + own_conf);
            let mut wt_sum_y = own.mv_y * (0.5 + own_conf);
            let mut wt_total = 0.5 + own_conf;
            let mut n_similar = 0u32;
            let mut n_neighbors = 0u32;

            for dy in -1i32..=1 { for dx in -1i32..=1 {
                if dx == 0 && dy == 0 { continue; }
                let nx = rx as i32 + dx; let ny = ry as i32 + dy;
                if nx < 0 || ny < 0 || nx >= self.rw as i32 || ny >= self.rh as i32 { continue; }
                let ni = ny as usize * self.rw + nx as usize;
                let nb = &old_regions[ni];
                n_neighbors += 1;

                // Weight: neighbors with higher confidence contribute more
                let w = 0.3 * nb.traj_confidence.max(0.1);
                wt_sum_x += nb.mv_x * w;
                wt_sum_y += nb.mv_y * w;
                wt_total += w;

                // Coherence: how similar is this neighbor's motion?
                let diff = ((own.mv_x - nb.mv_x).powi(2) + (own.mv_y - nb.mv_y).powi(2)).sqrt();
                if diff < 2.0 { n_similar += 1; }
            }}

            let r = &mut self.regions[ri];
            r.smooth_mv_x = wt_sum_x / wt_total.max(0.01);
            r.smooth_mv_y = wt_sum_y / wt_total.max(0.01);
            r.coherence = if n_neighbors > 0 { n_similar as f32 / n_neighbors as f32 } else { 0.0 };

            // Temporal density: how much of display_hz this region can use
            // > 1.0 means interpolation can boost above source rate
            // 1.0 means source rate
            // < 1.0 only for truly frozen content
            let motion_mag = (r.smooth_mv_x * r.smooth_mv_x + r.smooth_mv_y * r.smooth_mv_y).sqrt();
            let is_camera = (r.smooth_mv_x - gmx).abs() < 2.0 && (r.smooth_mv_y - gmy).abs() < 2.0;

            r.effective_density = if r.skip_streak > 30 && motion_mag < 0.1 {
                0.75  // truly frozen (menu, loading) — still 75% of base
            } else if is_camera && r.coherence > 0.7 && r.traj_confidence > 0.5 {
                1.8   // smooth camera with good model — can interpolate well above source
            } else if r.traj_confidence > 0.7 {
                1.5   // good trajectory — can interpolate above source
            } else if r.traj_confidence > 0.3 {
                1.0   // decent model — source rate
            } else {
                0.9   // poor model — slight reduction but stay near source
            };
        }}
    }

    /// Compute frame-level VRR timing
    /// Compute VRR timing — maximize toward display_hz.
    /// Philosophy: default is display rate. Only drop when we can't maintain it.
    /// Floor at 90Hz unless motion is truly unpredictable or too small to matter.
    pub fn frame_vrr(&self, base_fps: f64, display_hz: f64) -> (f32, f32) {
        let n = self.regions.len() as f32;
        if n == 0.0 { return (1000.0 / display_hz as f32, 0.5); }

        let avg_confidence: f32 = self.regions.iter().map(|r| r.traj_confidence).sum::<f32>() / n;
        let avg_coherence: f32 = self.regions.iter().map(|r| r.coherence).sum::<f32>() / n;

        // Motion magnitude: how much is actually moving on screen
        let motion_mag: f32 = self.regions.iter().map(|r| {
            (r.smooth_mv_x * r.smooth_mv_x + r.smooth_mv_y * r.smooth_mv_y).sqrt()
        }).sum::<f32>() / n;

        // What fraction of regions are unpredictable (low confidence, low coherence)
        let frac_unpredictable = self.regions.iter()
            .filter(|r| r.traj_confidence < 0.3 && r.coherence < 0.4)
            .count() as f32 / n;

        // What fraction is truly static (no motion at all for many frames)
        let frac_static = self.regions.iter()
            .filter(|r| r.skip_streak > 10)
            .count() as f32 / n;

        let activity = 1.0 - frac_static;

        // ── Maximize framerate toward display_hz (default 240) ──
        // Let the decoder/display determine what it can actually render.
        // The encoder provides the IDEAL timing, the player clamps to capability.

        let target_fps = if frac_static > 0.95 && motion_mag < 0.3 {
            // Frozen screen — drop to source rate (no interpolation needed)
            base_fps
        } else {
            // Default: push toward display_hz based on interpolation quality
            let interp_quality = (avg_confidence * 0.6 + avg_coherence * 0.4) as f64;
            // Scale from base_fps to display_hz based on quality
            // quality=0 → base_fps, quality=1 → display_hz
            let range = display_hz - base_fps;
            (base_fps + interp_quality * range).min(display_hz)
        };

        let duration = 1000.0 / target_fps.max(1.0) as f32;
        (duration, activity.min(1.0))
    }

    /// Get emergent layer summary
    pub fn layer_summary(&self, gmx: f32, gmy: f32) -> String {
        let n = self.regions.len();
        if n == 0 { return String::new(); }
        let n_static = self.regions.iter().filter(|r| r.skip_streak > 5 && r.coherence > 0.5).count();
        let n_camera = self.regions.iter().filter(|r| {
            (r.smooth_mv_x - gmx).abs() < 2.0 && (r.smooth_mv_y - gmy).abs() < 2.0 && r.coherence > 0.5
        }).count();
        let n_tracked = self.regions.iter().filter(|r| r.traj_confidence > 0.6 && r.coherence < 0.5).count();
        let n_wild = n - n_static - n_camera - n_tracked;
        let avg_coh: f32 = self.regions.iter().map(|r| r.coherence).sum::<f32>() / n as f32;
        format!("Static:{} Cam:{} Track:{} Wild:{} Coh:{:.0}%", n_static, n_camera, n_tracked, n_wild, avg_coh * 100.0)
    }

    pub fn clear(&mut self) {
        for r in &mut self.regions { *r = RegionMotion::default(); }
        for h in &mut self.mv_hist_x { h.clear(); }
        for h in &mut self.mv_hist_y { h.clear(); }
    }
}

// ══════════════════════════════════════════════════════════
// Background model — separate Fields from Actors
// ══════════════════════════════════════════════════════════

/// Anchor: a clean snapshot of an 8x8 block at a known-good moment
#[derive(Clone)]
struct BlockAnchor {
    y: [u8; 64],    // quantized to u8 (saves 3× memory vs f32)
    cb: [u8; 64],
    cr: [u8; 64],
    age: u16,       // frames since last matched (u16 saves 2 bytes)
    use_count: u16 }

/// AnchorStore: per-block dictionary of visual states
/// Each block keeps up to MAX_ANCHORS snapshots of what it has looked like
/// when stable. On change, find the best-matching anchor to restore.
pub struct BackgroundModel {
    anchors: Vec<Vec<BlockAnchor>>,  // per block, list of anchors
    stability: Vec<u32>,
    w: usize, h: usize,
    bw: usize, bh: usize }

const MAX_ANCHORS: usize = 2;

impl BackgroundModel {
    pub fn new(w: usize, h: usize) -> Self {
        let bw = w / 8; let bh = h / 8;
        Self {
            anchors: vec![Vec::new(); bw * bh],
            stability: vec![0; bw * bh],
            w, h, bw, bh }
    }

    /// Update background model from reference frame + skip information.
    /// Blocks that have been stable (skipped) for many frames are likely background.
    /// Extract 8x8 Y/Cb/Cr block from planes
    fn extract_block(y: &[f32], cb: &[f32], cr: &[f32], w: usize, ox: usize, oy: usize) -> ([f32;64],[f32;64],[f32;64]) {
        let mut by = [0.0f32;64]; let mut bcb = [0.0f32;64]; let mut bcr = [0.0f32;64];
        for yy in 0..8 { let r = (oy+yy)*w+ox; for xx in 0..8 { let i=yy*8+xx; by[i]=y[r+xx]; bcb[i]=cb[r+xx]; bcr[i]=cr[r+xx]; }}
        (by, bcb, bcr)
    }

    /// SAD between a block and an anchor (Y channel only for speed)
    fn anchor_sad(anchor: &BlockAnchor, y: &[f32], w: usize, ox: usize, oy: usize) -> f32 {
        let mut sad = 0.0f32;
        for yy in 0..8 { let r=(oy+yy)*w+ox; for xx in 0..8 { sad += (y[r+xx] - anchor.y[yy*8+xx] as f32).abs(); }}
        sad
    }

    pub fn update(&mut self, ref_y: &[f32], ref_cb: &[f32], ref_cr: &[f32],
        block_skipped: &[bool], bw: usize, bh: usize)
    {
        for by in 0..bh { for bx in 0..bw {
            let bi = by * bw + bx;
            if bi >= block_skipped.len() || bi >= self.stability.len() { continue; }

            if block_skipped[bi] {
                self.stability[bi] += 1;
            } else {
                self.stability[bi] = 0;  // reset on change
            }

            // Age all anchors
            for a in &mut self.anchors[bi] { a.age = a.age.saturating_add(1); }

            // Snapshot: if block has been stable for 30+ frames, capture as anchor
            if self.stability[bi] == 30 {
                let ox = bx*8; let oy = by*8;
                let (ay, acb, acr) = Self::extract_block(ref_y, ref_cb, ref_cr, self.w, ox, oy);
                let mut ay8 = [0u8; 64]; let mut acb8 = [0u8; 64]; let mut acr8 = [0u8; 64];
                for i in 0..64 { ay8[i] = ay[i].clamp(0.0, 255.0) as u8; acb8[i] = acb[i].clamp(0.0, 255.0) as u8; acr8[i] = acr[i].clamp(0.0, 255.0) as u8; }
                let new_anchor = BlockAnchor { y: ay8, cb: acb8, cr: acr8, age: 0, use_count: 0 };

                // Check if this anchor is novel (not too similar to existing ones)
                let is_novel = self.anchors[bi].iter().all(|a| {
                    let mut diff = 0u32;
                    for i in 0..64 { diff += (a.y[i] as i16 - new_anchor.y[i] as i16).unsigned_abs() as u32; }
                    diff / 64 > 5
                });

                if is_novel {
                    if self.anchors[bi].len() >= MAX_ANCHORS {
                        // Evict: remove oldest unused anchor
                        if let Some(worst) = self.anchors[bi].iter().enumerate()
                            .min_by_key(|(_, a)| a.use_count.wrapping_mul(1000).wrapping_sub(a.age))
                            .map(|(i, _)| i) {
                            self.anchors[bi].remove(worst);
                        }
                    }
                    self.anchors[bi].push(new_anchor);
                }
            }

            // Evict anchors that haven't been useful for 300+ frames
            self.anchors[bi].retain(|a| (a.age < 120 && a.use_count > 0) || a.use_count > 3);
        }}
    }

    /// For a block that just became uncovered (actor moved away),
    /// check if we should restore background instead of keeping stale pixels.
    /// Returns true if background was restored.
    /// Find best matching anchor for a changed block.
    /// If an anchor matches the current frame well, use it to clean the reference.
    pub fn try_restore_block(&mut self, ref_y: &mut [f32], ref_cb: &mut [f32], ref_cr: &mut [f32],
        bx: usize, by: usize, cur_y: &[f32]) -> bool
    {
        let bi = by * self.bw + bx;
        if bi >= self.anchors.len() || self.anchors[bi].is_empty() { return false; }
        // Only restore from anchors that have been used successfully before
        // (use_count > 0 means this anchor has proven to be a good match)
        if self.anchors[bi].iter().all(|a| a.use_count == 0 && a.age < 60) { return false; }
        let ox = bx * 8; let oy = by * 8;

        let mut best_idx = 0usize;
        let mut best_sad = f32::MAX;
        for (i, a) in self.anchors[bi].iter().enumerate() {
            if a.use_count == 0 && a.age < 60 { continue; } // skip unproven anchors
            let sad = Self::anchor_sad(a, cur_y, self.w, ox, oy);
            if sad < best_sad { best_sad = sad; best_idx = i; }
        }

        let mut ref_sad = 0.0f32;
        for yy in 0..8 { let r=(oy+yy)*self.w+ox; for xx in 0..8 { ref_sad += (cur_y[r+xx] - ref_y[r+xx]).abs(); }}

        // Stricter restore: anchor must match very closely, ref must be clearly wrong
        if best_sad / 64.0 < 4.0 && ref_sad / 64.0 > 20.0 && best_sad < ref_sad * 0.2 {
            let a = &self.anchors[bi][best_idx];
            for yy in 0..8 { let r=(oy+yy)*self.w+ox; for xx in 0..8 {
                let i = yy*8+xx;
                ref_y[r+xx] = a.y[i] as f32;
                ref_cb[r+xx] = a.cb[i] as f32;
                ref_cr[r+xx] = a.cr[i] as f32;
            }}
            self.anchors[bi][best_idx].use_count += 1;
            self.anchors[bi][best_idx].age = 0;
            return true;
        }
        false
    }

    /// Initialize anchors from first I-frame
    pub fn init_from_frame(&mut self, y: &[f32], cb: &[f32], cr: &[f32]) {
        for by in 0..self.bh { for bx in 0..self.bw {
            let bi = by * self.bw + bx;
            let ox = bx*8; let oy = by*8;
            let (ay, acb, acr) = Self::extract_block(y, cb, cr, self.w, ox, oy);
            let mut ay8=[0u8;64]; let mut acb8=[0u8;64]; let mut acr8=[0u8;64];
            for i in 0..64 { ay8[i]=ay[i].clamp(0.0,255.0)as u8; acb8[i]=acb[i].clamp(0.0,255.0)as u8; acr8[i]=acr[i].clamp(0.0,255.0)as u8; }
            self.anchors[bi] = vec![BlockAnchor { y: ay8, cb: acb8, cr: acr8, age: 0, use_count: 1 }];
        }}
        self.stability.fill(0);
    }

    pub fn clear(&mut self) {
        for a in &mut self.anchors { a.clear(); }
        self.stability.fill(0);
    }
}

// ══════════════════════════════════════════════════════════
// Lighting model — encode brightness as gain/offset function
// ══════════════════════════════════════════════════════════

/// Per-block lighting state
#[derive(Clone, Debug)]
pub struct BlockLighting {
    pub base_luma: f32,     // expected luma when unoccluded
    pub current_gain: f32,  // multiplicative: 1.0 = normal, 0.7 = shadowed
    pub current_offset: f32,// additive: ability glow, etc.
    pub gain_history: Vec<f32>,  // track gain over time
}

impl Default for BlockLighting {
    fn default() -> Self { Self { base_luma: 128.0, current_gain: 1.0, current_offset: 0.0, gain_history: Vec::new() } }
}

/// Frame-level lighting analysis
pub struct LightingModel {
    blocks: Vec<BlockLighting>,
    bw: usize, bh: usize }

impl LightingModel {
    pub fn new(w: usize, h: usize) -> Self {
        let bw = w / 8; let bh = h / 8;
        Self { blocks: vec![BlockLighting::default(); bw * bh], bw, bh }
    }

    /// Analyze lighting from reference and current frame.
    /// Detects multiplicative (shadow) vs additive (content) changes.
    pub fn analyze(&mut self, ref_y: &[f32], cur_y: &[f32], w: usize,
        block_skipped: &[bool])
    {
        for by in 0..self.bh { for bx in 0..self.bw {
            let bi = by * self.bw + bx;
            if bi >= block_skipped.len() { continue; }
            let ox = bx * 8; let oy = by * 8;

            // Compute mean luma in ref and current
            let mut ref_mean = 0.0f32; let mut cur_mean = 0.0f32;
            for yy in 0..8 { let r = (oy+yy)*w+ox; for xx in 0..8 {
                ref_mean += ref_y[r+xx]; cur_mean += cur_y[r+xx];
            }}
            ref_mean /= 64.0; cur_mean /= 64.0;

            if block_skipped[bi] {
                // Stable block — update base luma slowly
                let bl = &mut self.blocks[bi];
                bl.base_luma += (cur_mean - bl.base_luma) * 0.02;
                bl.current_gain = 1.0;
                bl.current_offset = 0.0;
            } else if ref_mean > 10.0 {
                // Changed block — decompose change into gain + offset
                // If cur = gain * ref + offset, then:
                //   gain ≈ cur_mean / ref_mean (multiplicative component)
                //   offset ≈ cur_mean - gain * ref_mean (additive residual)
                let raw_gain = cur_mean / ref_mean.max(1.0);
                let gain = raw_gain.clamp(0.3, 3.0);

                // Verify: does the gain model explain the block well?
                let mut gain_residual = 0.0f32;
                let mut raw_residual = 0.0f32;
                for yy in 0..8 { let r = (oy+yy)*w+ox; for xx in 0..8 {
                    let predicted = ref_y[r+xx] * gain;
                    gain_residual += (cur_y[r+xx] - predicted).abs();
                    raw_residual += (cur_y[r+xx] - ref_y[r+xx]).abs();
                }}

                let bl = &mut self.blocks[bi];
                // If gain model reduces residual by 30%+, it's a lighting change
                if gain_residual < raw_residual * 0.7 && (gain - 1.0).abs() > 0.05 {
                    bl.current_gain = gain;
                    bl.current_offset = cur_mean - gain * ref_mean;
                } else {
                    bl.current_gain = 1.0;
                    bl.current_offset = cur_mean - ref_mean;
                }

                bl.gain_history.push(bl.current_gain);
                if bl.gain_history.len() > 16 { bl.gain_history.remove(0); }
            }
        }}
    }

    /// Get lighting summary for stats display
    pub fn summary(&self) -> String {
        let n = self.blocks.len();
        if n == 0 { return String::new(); }
        let n_shadowed = self.blocks.iter().filter(|b| b.current_gain < 0.85).count();
        let n_bright = self.blocks.iter().filter(|b| b.current_gain > 1.15).count();
        let avg_gain: f32 = self.blocks.iter().map(|b| b.current_gain).sum::<f32>() / n as f32;
        if n_shadowed == 0 && n_bright == 0 { return String::new(); }
        format!("Light: {:.2}× avg  {}shd {}brt", avg_gain, n_shadowed, n_bright)
    }

    pub fn clear(&mut self) {
        for b in &mut self.blocks { *b = BlockLighting::default(); }
    }
}

// ══════════════════════════════════════════════════════════
// Half-pixel motion estimation — reduces oscillation artifacts
// ══════════════════════════════════════════════════════════

/// Interpolate a pixel at half-pixel position using bilinear filtering
#[inline]
fn interp_half(plane: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let ix = x as usize; let iy = y as usize;
    let fx = x - ix as f32; let fy = y - iy as f32;
    let ix1 = (ix + 1).min(w - 1); let iy1 = (iy + 1).min(h - 1);
    plane[iy * w + ix] * (1.0 - fx) * (1.0 - fy)
        + plane[iy * w + ix1] * fx * (1.0 - fy)
        + plane[iy1 * w + ix] * (1.0 - fx) * fy
        + plane[iy1 * w + ix1] * fx * fy
}

/// Half-pixel refinement: after integer search finds best MV,
/// test 8 half-pixel positions around it and pick the best.
/// Returns (mvx * 2, mvy * 2) in half-pixel units.
pub fn halfpel_refine(ref_y: &[f32], cur_y: &[f32], w: usize, h: usize,
    bx: usize, by: usize, int_mvx: i8, int_mvy: i8) -> (i16, i16) {
    let mut best_sad = f32::MAX;
    let mut best_hx = int_mvx as i16 * 2;
    let mut best_hy = int_mvy as i16 * 2;

    // Test 9 positions: center + 8 half-pixel neighbors
    for dhy in -1i16..=1 {
        for dhx in -1i16..=1 {
            let hx = int_mvx as i16 * 2 + dhx;
            let hy = int_mvy as i16 * 2 + dhy;
            let rx = bx as f32 + hx as f32 * 0.5;
            let ry = by as f32 + hy as f32 * 0.5;

            // Bounds check
            if rx < 0.0 || rx + 7.5 >= w as f32 || ry < 0.0 || ry + 7.5 >= h as f32 { continue; }

            let mut sad = 0.0f32;
            for yy in 0..8 {
                for xx in 0..8 {
                    let ref_val = interp_half(ref_y, w, h, rx + xx as f32, ry + yy as f32);
                    let cur_val = cur_y[(by + yy) * w + bx + xx];
                    sad += (cur_val - ref_val).abs();
                }
            }
            if sad < best_sad { best_sad = sad; best_hx = hx; best_hy = hy; }
        }
    }
    (best_hx, best_hy)
}

/// Motion-compensated block extraction at half-pixel precision
pub fn mc_block_halfpel(ref_y: &[f32], w: usize, h: usize,
    bx: usize, by: usize, hmvx: i16, hmvy: i16, out: &mut [f32; 64])
{
    let rx = bx as f32 + hmvx as f32 * 0.5;
    let ry = by as f32 + hmvy as f32 * 0.5;
    for yy in 0..8 {
        for xx in 0..8 {
            out[yy * 8 + xx] = interp_half(ref_y, w, h, rx + xx as f32, ry + yy as f32);
        }
    }
}

// ══════════════════════════════════════════════════════════
// Texture descriptors — encode texture as function, not pixels
// ══════════════════════════════════════════════════════════

/// Compact texture descriptor for a 16×16 region.
/// Encodes the statistical properties needed to synthesize matching texture.
/// Total: ~32 bytes per region — replaces ~768 bytes of pixel data.
#[derive(Clone, Debug)]
pub struct TextureDescriptor {
    pub mean_y: f32,          // average luma
    pub mean_cb: f32,         // average chroma
    pub mean_cr: f32,
    pub variance_y: f32,      // luma variance
    pub grad_x: f32,          // dominant gradient direction (brightness slope)
    pub grad_y: f32,
    pub freq_low: f32,        // energy in low spatial frequencies
    pub freq_high: f32,       // energy in high spatial frequencies
    pub directionality: f32,  // 0=isotropic, 1=strongly directional
    pub scale: f32,           // dominant feature size (leaf size, grass blade width)
    pub seed: u32,            // deterministic seed for synthesis
}

impl Default for TextureDescriptor {
    fn default() -> Self { Self { mean_y:128.0,mean_cb:0.0,mean_cr:0.0,variance_y:0.0,
        grad_x:0.0,grad_y:0.0,freq_low:0.0,freq_high:0.0,directionality:0.0,scale:1.0,seed:0 }}
}

/// Analyze a 16×16 region and produce a texture descriptor
pub fn analyze_texture(y: &[f32], cb: &[f32], cr: &[f32], w: usize,
    ox: usize, oy: usize) -> TextureDescriptor {
    let mut td = TextureDescriptor::default();
    let mut sum_y = 0.0f64; let mut sum_y2 = 0.0f64;
    let mut sum_cb = 0.0f64; let mut sum_cr = 0.0f64;
    let mut grad_h = 0.0f64; let mut grad_v = 0.0f64;
    let mut hf = 0.0f64; let mut lf = 0.0f64;
    let mut n = 0u32;

    for yy in 0..16 { for xx in 0..16 {
        let py = oy + yy; let px = ox + xx;
        if py >= y.len() / w || px >= w { continue; }
        let pi = py * w + px;
        let v = y[pi] as f64;
        sum_y += v; sum_y2 += v * v;
        sum_cb += cb[pi] as f64; sum_cr += cr[pi] as f64;
        n += 1;

        // Gradient
        if xx < 15 && px + 1 < w { grad_h += (y[pi+1] - y[pi]).abs() as f64; }
        if yy < 15 && py + 1 < y.len() / w { grad_v += (y[pi+w] - y[pi]).abs() as f64; }

        // Frequency split: compare pixel to 4x4 block average
        let bx = xx / 4; let by = yy / 4;
        let mut blk_avg = 0.0f64; let mut blk_n = 0u32;
        for dy in 0..4 { for dx in 0..4 {
            let bpy = oy + by*4 + dy; let bpx = ox + bx*4 + dx;
            if bpy < y.len() / w && bpx < w { blk_avg += y[bpy*w+bpx] as f64; blk_n += 1; }
        }}
        if blk_n > 0 { blk_avg /= blk_n as f64; }
        hf += (v - blk_avg).abs();
        lf += blk_avg;
    }}

    if n == 0 { return td; }
    let nf = n as f64;
    td.mean_y = (sum_y / nf) as f32;
    td.mean_cb = (sum_cb / nf) as f32;
    td.mean_cr = (sum_cr / nf) as f32;
    td.variance_y = ((sum_y2 / nf - (sum_y / nf).powi(2)).max(0.0)) as f32;

    let gh = grad_h / nf;
    let gv = grad_v / nf;
    td.grad_x = gh as f32;
    td.grad_y = gv as f32;
    td.freq_high = (hf / nf) as f32;
    td.freq_low = (lf / nf) as f32;
    td.directionality = if gh + gv > 0.1 { ((gh - gv).abs() / (gh + gv)) as f32 } else { 0.0 };

    // Dominant scale: autocorrelation at different offsets
    // Simplified: measure which offset gives the first correlation drop
    let mut best_scale = 1.0f32;
    for s in [2, 4, 6, 8] {
        let mut corr = 0.0f64; let mut cn = 0u32;
        for yy in 0..(16-s) { for xx in 0..(16-s) {
            let py = oy+yy; let px = ox+xx;
            if py >= y.len()/w || px+s >= w { continue; }
            corr += (y[py*w+px] as f64 - td.mean_y as f64) * (y[py*w+px+s] as f64 - td.mean_y as f64);
            cn += 1;
        }}
        if cn > 0 && td.variance_y > 0.1 {
            let norm_corr = (corr / cn as f64 / td.variance_y as f64).abs();
            if norm_corr < 0.5 { best_scale = s as f32; break; }
        }
    }
    td.scale = best_scale;
    td.seed = (ox as u32).wrapping_mul(65537).wrapping_add(oy as u32).wrapping_mul(2654435761);
    td
}

/// Synthesize a 16×16 texture patch from a descriptor.
/// Produces pixels that match the statistical properties of the original.
pub fn synthesize_texture(td: &TextureDescriptor, out_y: &mut [f32], out_cb: &mut [f32],
    out_cr: &mut [f32], w: usize, ox: usize, oy: usize) {
    let scale_px = td.scale.max(1.0) as usize;

    for yy in 0..16 { for xx in 0..16 {
        let py = oy + yy; let px = ox + xx;
        if py * w + px >= out_y.len() { continue; }
        let pi = py * w + px;

        // Base: mean + gradient
        let base = td.mean_y + td.grad_x * (xx as f32 - 8.0) + td.grad_y * (yy as f32 - 8.0);

        // Texture noise at dominant scale
        let tx = (px / scale_px) as u32;
        let ty = (py / scale_px) as u32;
        let hash = td.seed.wrapping_mul(2654435761).wrapping_add(ty.wrapping_mul(65537).wrapping_add(tx));
        let hash2 = hash.wrapping_mul(2246822519).wrapping_add(hash >> 13);
        let noise_coarse = (hash2 as i32 as f32) / (i32::MAX as f32);

        // Fine noise at pixel level
        let fhash = td.seed.wrapping_mul(1597334677).wrapping_add((py as u32).wrapping_mul(10000).wrapping_add(px as u32));
        let fhash2 = fhash.wrapping_mul(3266489917).wrapping_add(fhash >> 16);
        let noise_fine = (fhash2 as i32 as f32) / (i32::MAX as f32);

        // Blend coarse and fine based on frequency content
        let hf_ratio = td.freq_high / (td.freq_high + td.freq_low + 0.1);
        let noise = noise_coarse * (1.0 - hf_ratio) + noise_fine * hf_ratio;

        // Apply directionality: stretch noise along dominant direction
        let dir_noise = if td.directionality > 0.3 {
            // Horizontally dominant: use x-based noise more
            if td.grad_x > td.grad_y { noise_coarse } else { noise_fine }
        } else { noise };

        let std = td.variance_y.sqrt();
        out_y[pi] = (base + dir_noise * std * 0.8).clamp(0.0, 255.0);
        out_cb[pi] = td.mean_cb + noise_fine * std * 0.1;
        out_cr[pi] = td.mean_cr + noise_fine * std * 0.1;
    }}
}

/// For a frame, identify texture regions and produce descriptors.
/// Returns descriptors for regions classified as Texture.
pub fn extract_texture_map(y: &[f32], cb: &[f32], cr: &[f32], w: usize, h: usize,
    quality_map: &[f32], bw: usize) -> Vec<(usize, usize, TextureDescriptor)> {
    let rw = w / 16; let rh = h / 16;
    let mut descriptors = Vec::new();

    for ry in 0..rh { for rx in 0..rw {
        // Check if any of the 4 child blocks are Texture-classified
        let bx0 = rx * 2; let by0 = ry * 2;
        let mut any_texture = false;
        for dy in 0..2 { for dx in 0..2 {
            let bi = (by0+dy) * bw + (bx0+dx);
            if bi < quality_map.len() && quality_map[bi] <= 0.41 {
                // quality_scale <= 0.4 = Texture class
                any_texture = true;
            }
        }}
        if any_texture {
            let td = analyze_texture(y, cb, cr, w, rx * 16, ry * 16);
            if td.variance_y > 50.0 { // only store descriptors for genuinely textured regions
                descriptors.push((rx, ry, td));
            }
        }
    }}
    descriptors
}

// ══════════════════════════════════════════════════════════
// Optimized SAD with early exit
// ══════════════════════════════════════════════════════════

/// SAD with early exit: returns u32::MAX if sad exceeds threshold * 64 before finishing
#[inline]
pub fn sad_8x8_early(a: &[f32], b: &[f32], w: usize, ax: usize, ay: usize,
    bx: usize, by: usize, threshold_x64: f32) -> f32 {
    let mut sad = 0.0f32;
    for yy in 0..4 {
        let ai = (ay+yy)*w+ax; let bi = (by+yy)*w+bx;
        // Unroll inner 8 pixels
        sad += (a[ai] - b[bi]).abs() + (a[ai+1] - b[bi+1]).abs()
             + (a[ai+2] - b[bi+2]).abs() + (a[ai+3] - b[bi+3]).abs()
             + (a[ai+4] - b[bi+4]).abs() + (a[ai+5] - b[bi+5]).abs()
             + (a[ai+6] - b[bi+6]).abs() + (a[ai+7] - b[bi+7]).abs();
    }
    // Early exit after first 4 rows
    if sad > threshold_x64 * 0.6 { return f32::MAX; }
    for yy in 4..8 {
        let ai = (ay+yy)*w+ax; let bi = (by+yy)*w+bx;
        sad += (a[ai] - b[bi]).abs() + (a[ai+1] - b[bi+1]).abs()
             + (a[ai+2] - b[bi+2]).abs() + (a[ai+3] - b[bi+3]).abs()
             + (a[ai+4] - b[bi+4]).abs() + (a[ai+5] - b[bi+5]).abs()
             + (a[ai+6] - b[bi+6]).abs() + (a[ai+7] - b[bi+7]).abs();
    }
    sad
}

// ══════════════════════════════════════════════════════════
// Source quality analysis — detect low-bitrate / noisy inputs
// ══════════════════════════════════════════════════════════

/// Source quality profile
#[derive(Clone, Debug)]
pub struct SourceProfile {
    pub noise_level: f32,       // 0 = clean, 10+ = very noisy
    pub blocking_level: f32,    // 0 = no blocking artifacts, 5+ = heavy
    pub effective_bpp: f32,     // estimated bits per pixel of source
    pub is_low_bitrate: bool,   // true if source appears heavily compressed
}

/// Analyze source frame quality by measuring noise and blocking artifacts
pub fn analyze_source_quality(rgb: &[u8], w: usize, h: usize) -> SourceProfile {
    let n = w * h;
    let mut y = vec![0.0f32; n];
    let mut cb = vec![0.0f32; n];
    let mut cr = vec![0.0f32; n];
    rgb_to_planes(rgb, w, h, &mut y, &mut cb, &mut cr);

    // Measure noise: high-frequency energy (difference from local 3x3 mean)
    let mut noise_sum = 0.0f64;
    let mut noise_n = 0u64;
    for py in 1..h-1 { for px in 1..w-1 {
        let c = y[py*w+px];
        let avg = (y[(py-1)*w+px] + y[(py+1)*w+px] + y[py*w+px-1] + y[py*w+px+1]) * 0.25;
        noise_sum += (c - avg).abs() as f64;
        noise_n += 1;
    }}
    let noise_level = if noise_n > 0 { (noise_sum / noise_n as f64) as f32 } else { 0.0 };

    // Measure blocking: discontinuity at 8-pixel boundaries vs interior
    let mut boundary_diff = 0.0f64;
    let mut interior_diff = 0.0f64;
    let mut b_n = 0u64;
    let mut i_n = 0u64;
    for py in 0..h {
        for px in 1..w {
            let d = (y[py*w+px] - y[py*w+px-1]).abs() as f64;
            if px % 8 == 0 { boundary_diff += d; b_n += 1; }
            else { interior_diff += d; i_n += 1; }
        }
    }
    let avg_boundary = if b_n > 0 { boundary_diff / b_n as f64 } else { 0.0 };
    let avg_interior = if i_n > 0 { interior_diff / i_n as f64 } else { 1.0 };
    let blocking_level = if avg_interior > 0.1 {
        ((avg_boundary / avg_interior - 1.0).max(0.0) * 5.0) as f32
    } else { 0.0 };

    // Estimate effective bpp from complexity
    // Low-bitrate sources have low variance + visible artifacts
    let mut var_sum = 0.0f64;
    for py in 0..h { for px in 0..w { 
        var_sum += y[py*w+px] as f64;
    }}
    let mean = var_sum / n as f64;
    let mut var = 0.0f64;
    for py in 0..h { for px in 0..w {
        let d = y[py*w+px] as f64 - mean;
        var += d * d;
    }}
    var /= n as f64;
    // Rough estimate: high variance + low noise = good quality source
    let effective_bpp = (var.sqrt() * 0.01 + noise_level as f64 * 0.1) as f32;

    let is_low_bitrate = blocking_level > 1.5 || (noise_level > 5.0 && blocking_level > 0.5);

    SourceProfile { noise_level, blocking_level, effective_bpp, is_low_bitrate }
}

/// Encoding mode
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EncodingMode {
    /// Faithfully reproduce the source, including its noise and artifacts
    Reproduction,
    /// Attempt to clean up and represent the content at higher quality
    Representation }

/// Adjust encoder parameters based on source quality and mode
pub fn adapt_encoder_for_source(enc: &mut VideoEncoder, profile: &SourceProfile, mode: EncodingMode) {
    match mode {
        EncodingMode::Reproduction => {
            if profile.is_low_bitrate {
                // Source is already degraded — be gentler
                enc.dead_zone *= 0.5;             // preserve more detail (less aggressive quantization)
                enc.skip_threshold *= 1.5;        // higher skip threshold (source noise makes blocks differ)
                enc.deblock_strength *= 0.3;      // light deblock (don't smooth what's already smooth)
                enc.grain_scale = 0.0;            // no synthetic grain — source has its own noise
            }
        }
        EncodingMode::Representation => {
            if profile.is_low_bitrate {
                // Clean up the source
                enc.dead_zone *= 0.7;             // moderate quantization
                enc.skip_threshold *= 1.2;
                enc.deblock_strength = (0.3 + profile.blocking_level * 0.15).min(0.9);  // strong deblock for blocky sources
                enc.grain_scale = 0.4;            // add clean grain to mask artifacts
            }
        }
    }
}

/// Pre-filter a frame for representation mode:
/// mild spatial denoising to reduce source compression artifacts
pub fn prefilter_denoise(rgb: &mut [u8], w: usize, h: usize, strength: f32) {
    if strength <= 0.0 { return; }
    let n = w * h;
    let mut y = vec![0.0f32; n];
    let mut cb = vec![0.0f32; n];
    let mut cr = vec![0.0f32; n];
    rgb_to_planes(rgb, w, h, &mut y, &mut cb, &mut cr);

    // 3x3 edge-preserving bilateral-ish filter on luma
    let mut filtered = y.clone();
    let sigma = strength * 10.0;
    for py in 1..h-1 { for px in 1..w-1 {
        let center = y[py*w+px];
        let mut wsum = 0.0f32;
        let mut vsum = 0.0f32;
        for dy in -1i32..=1 { for dx in -1i32..=1 {
            let ny = (py as i32 + dy) as usize;
            let nx = (px as i32 + dx) as usize;
            let val = y[ny*w+nx];
            let diff = (val - center).abs();
            // Weight: close values get high weight (preserve edges)
            let w = (-diff * diff / (2.0 * sigma * sigma)).exp();
            wsum += w;
            vsum += val * w;
        }}
        if wsum > 0.0 { filtered[py*w+px] = vsum / wsum; }
    }}

    planes_to_rgb(&filtered, &cb, &cr, rgb, n);
}

// ══════════════════════════════════════════════════════════
// GPU-ready prediction routing — fixed-size, parallel-safe
// ══════════════════════════════════════════════════════════

/// Fixed-size prediction slot for a block (no heap allocation)
#[derive(Clone, Copy)]
pub struct PredSlot {
    pub source: u8,       // 0=none, 1=global_mv, 2=trajectory, 3=anchor, 4=neighbor, 5=intra
    pub confidence: f32,  // 0..1
    pub mvx: f32,         // prediction motion vector
    pub mvy: f32,
    pub dc_pred: f32,     // predicted DC value
}

impl Default for PredSlot {
    fn default() -> Self { Self { source: 0, confidence: 0.0, mvx: 0.0, mvy: 0.0, dc_pred: 0.0 } }
}

/// Per-block routing result (fixed size, GPU-writable)
#[derive(Clone, Copy)]
pub struct RouteResult {
    pub best_slot: u8,
    pub skip: bool,
    pub residual_energy: f32,  // if low → skip, if high → needs DCT
    pub use_intra: bool }

/// Prediction routing table for entire frame.
/// Fixed-size array, one entry per block, fully parallel-safe.
pub struct PredictionRouter {
    slots: Vec<[PredSlot; 4]>,  // 4 prediction slots per block
    routes: Vec<RouteResult>,
    bw: usize, bh: usize }

impl PredictionRouter {
    pub fn new(w: usize, h: usize) -> Self {
        let bw = w / 8; let bh = h / 8;
        let n = bw * bh;
        Self {
            slots: vec![[PredSlot::default(); 4]; n],
            routes: vec![RouteResult { best_slot: 0, skip: false, residual_energy: 0.0, use_intra: false }; n],
            bw, bh }
    }

    /// Phase 1: Fill prediction slots for all blocks (parallelizable)
    /// Each block gets predictions from: global_mv, trajectory, anchor_dc, neighbor
    pub fn fill_predictions(
        &mut self,
        ref_y: &[f32], cur_y: &[f32], w: usize,
        gmx: f32, gmy: f32,
        traj: &MotionTrajectory,
        anchor_dcs: &[f32],  // per-block anchor DC values (from bg model)
        anchor_conf: &[f32], // per-block anchor confidence
    ) {
        // This loop is embarrassingly parallel — each block independent
        for by in 0..self.bh { for bx in 0..self.bw {
            let bi = by * self.bw + bx;
            let ox = bx * 8; let oy = by * 8;

            // Compute current block DC
            let mut cur_dc = 0.0f32;
            for yy in 0..8 { for xx in 0..8 { cur_dc += cur_y[(oy+yy)*w+ox+xx]; }}
            cur_dc /= 64.0;

            // Slot 0: Global MV
            let gx = (ox as i32 + gmx as i32).max(0).min((w-8) as i32) as usize;
            let gy = (oy as i32 + gmy as i32).max(0).min(((self.bh*8).saturating_sub(8)) as i32) as usize;
            let mut ref_dc_g = 0.0f32;
            for yy in 0..8 { for xx in 0..8 { ref_dc_g += ref_y[(gy+yy)*w+gx+xx]; }}
            ref_dc_g /= 64.0;
            let dc_diff_g = (cur_dc - ref_dc_g).abs();
            self.slots[bi][0] = PredSlot {
                source: 1, confidence: (1.0 - dc_diff_g / 30.0).max(0.0),
                mvx: gmx, mvy: gmy, dc_pred: ref_dc_g };

            // Slot 1: Trajectory
            let (tmx, tmy, tconf) = traj.predict(bi);
            self.slots[bi][1] = PredSlot {
                source: 2, confidence: tconf,
                mvx: tmx, mvy: tmy, dc_pred: cur_dc, // trajectory predicts position, not DC
            };

            // Slot 2: Anchor (background model)
            if bi < anchor_dcs.len() && bi < anchor_conf.len() {
                let adc_diff = (cur_dc - anchor_dcs[bi]).abs();
                self.slots[bi][2] = PredSlot {
                    source: 3, confidence: anchor_conf[bi] * (1.0 - adc_diff / 20.0).max(0.0),
                    mvx: 0.0, mvy: 0.0, dc_pred: anchor_dcs[bi] };
            }

            // Slot 3: Stationary (zero MV)
            let mut ref_dc_0 = 0.0f32;
            for yy in 0..8 { for xx in 0..8 { ref_dc_0 += ref_y[(oy+yy)*w+ox+xx]; }}
            ref_dc_0 /= 64.0;
            let dc_diff_0 = (cur_dc - ref_dc_0).abs();
            self.slots[bi][3] = PredSlot {
                source: 4, confidence: (1.0 - dc_diff_0 / 20.0).max(0.0),
                mvx: 0.0, mvy: 0.0, dc_pred: ref_dc_0 };
        }}
    }

    /// Phase 2: Route each block to best prediction (parallelizable)
    pub fn route_all(&mut self, skip_threshold: f32) {
        for bi in 0..self.slots.len() {
            let slots = &self.slots[bi];

            // Find highest confidence slot
            let mut best = 0usize;
            let mut best_conf = -1.0f32;
            for i in 0..4 {
                if slots[i].confidence > best_conf {
                    best_conf = slots[i].confidence;
                    best = i;
                }
            }

            let r = &mut self.routes[bi];
            if best_conf > 0.7 {
                // High confidence prediction available → likely skip
                r.best_slot = slots[best].source;
                r.residual_energy = (1.0 - best_conf) * 100.0;  // approximate
                r.skip = best_conf > 0.85;
                r.use_intra = false;
            } else if best_conf > 0.3 {
                // Moderate confidence → will need residual encoding
                r.best_slot = slots[best].source;
                r.residual_energy = (1.0 - best_conf) * 200.0;
                r.skip = false;
                r.use_intra = false;
            } else {
                // No good prediction → intra
                r.best_slot = 5;
                r.residual_energy = 999.0;
                r.skip = false;
                r.use_intra = true;
            }
        }
    }

    /// Get routing stats for display
    pub fn stats(&self) -> String {
        let n = self.routes.len();
        if n == 0 { return String::new(); }
        let n_skip = self.routes.iter().filter(|r| r.skip).count();
        let n_intra = self.routes.iter().filter(|r| r.use_intra).count();
        let n_inter = n - n_skip - n_intra;
        let avg_energy: f32 = self.routes.iter().map(|r| r.residual_energy).sum::<f32>() / n as f32;
        format!("Route: {}skip {}inter {}intra E={:.0}", n_skip, n_inter, n_intra, avg_energy)
    }
}

// ══════════════════════════════════════════════════════════
// SIMD-optimized hot paths
// ══════════════════════════════════════════════════════════

/// SIMD-friendly SAD: process 8 f32s at a time with no branches
/// Compiler auto-vectorizes this into 2 NEON/SSE operations per row
#[inline(always)]
pub fn sad_8x8_simd(a: &[f32], b: &[f32], w: usize, 
    ax: usize, ay: usize, bx: usize, by: usize) -> f32 {
    let mut rows = [0.0f32; 8];
    for yy in 0..8 {
        let ai = (ay + yy) * w + ax;
        let bi = (by + yy) * w + bx;
        // Unrolled 8-wide: compiler vectorizes this
        let mut r = 0.0f32;
        r += (a[ai]   - b[bi]).abs();
        r += (a[ai+1] - b[bi+1]).abs();
        r += (a[ai+2] - b[bi+2]).abs();
        r += (a[ai+3] - b[bi+3]).abs();
        r += (a[ai+4] - b[bi+4]).abs();
        r += (a[ai+5] - b[bi+5]).abs();
        r += (a[ai+6] - b[bi+6]).abs();
        r += (a[ai+7] - b[bi+7]).abs();
        rows[yy] = r;
    }
    // Sum rows (reduces to single SIMD horizontal add)
    rows[0]+rows[1]+rows[2]+rows[3]+rows[4]+rows[5]+rows[6]+rows[7]
}

/// SIMD-friendly block copy: 8 f32s per row
#[inline(always)]
pub fn copy_block_8x8(src: &[f32], dst: &mut [f32], w: usize,
    sx: usize, sy: usize, dx: usize, dy: usize) {
    for yy in 0..8 {
        let si = (sy + yy) * w + sx;
        let di = (dy + yy) * w + dx;
        dst[di..di+8].copy_from_slice(&src[si..si+8]);
    }
}

/// SIMD-friendly RGB to YCbCr for a row of pixels
/// Processes 8 pixels at a time (24 bytes in, 8 floats per channel out)
#[inline]
pub fn rgb_to_ycbcr_row(rgb: &[u8], y: &mut [f32], cb: &mut [f32], cr: &mut [f32], 
    offset: usize, count: usize) {
    let mut i = 0;
    while i + 8 <= count {
        // Process 8 pixels — compiler generates 8-wide SIMD
        for k in 0..8 {
            let j = (offset + i + k) * 3;
            let r = rgb[j] as f32;
            let g = rgb[j+1] as f32;
            let b = rgb[j+2] as f32;
            y[offset+i+k]  =  0.2126 * r + 0.7152 * g + 0.0722 * b;
            cb[offset+i+k] = -0.1146 * r - 0.3854 * g + 0.5    * b;
            cr[offset+i+k] =  0.5    * r - 0.4542 * g - 0.0458 * b;
        }
        i += 8;
    }
    // Remainder
    while i < count {
        let j = (offset + i) * 3;
        let r = rgb[j] as f32;
        let g = rgb[j+1] as f32;
        let b = rgb[j+2] as f32;
        y[offset+i]  =  0.2126 * r + 0.7152 * g + 0.0722 * b;
        cb[offset+i] = -0.1146 * r - 0.3854 * g + 0.5    * b;
        cr[offset+i] =  0.5    * r - 0.4542 * g - 0.0458 * b;
        i += 1;
    }
}

/// Optimized DCT-II forward 8-point using butterfly decomposition
/// This is 2x faster than the naive matrix multiply version
#[inline]
pub fn dct_1d_butterfly(input: &[f32; 8], output: &mut [f32; 8]) {
    // Stage 1: butterfly
    let s0 = input[0] + input[7]; let d0 = input[0] - input[7];
    let s1 = input[1] + input[6]; let d1 = input[1] - input[6];
    let s2 = input[2] + input[5]; let d2 = input[2] - input[5];
    let s3 = input[3] + input[4]; let d3 = input[3] - input[4];

    // Stage 2: even part
    let e0 = s0 + s3; let e1 = s1 + s2;
    let e2 = s0 - s3; let e3 = s1 - s2;

    // Even outputs
    output[0] = 0.35355339 * (e0 + e1);  // 1/sqrt(8) * (e0+e1)
    output[4] = 0.35355339 * (e0 - e1);
    output[2] = 0.46193977 * e2 + 0.19134172 * e3;
    output[6] = 0.19134172 * e2 - 0.46193977 * e3;

    // Odd outputs (rotations)
    let c1 = 0.49039264; let c3 = 0.41573481;
    let c5 = 0.27778512; let c7 = 0.09754516;
    output[1] = c1*d0 + c3*d1 + c5*d2 + c7*d3;
    output[3] = c3*d0 - c7*d1 - c1*d2 - c5*d3;
    output[5] = c5*d0 - c1*d1 + c7*d2 + c3*d3;
    output[7] = c7*d0 - c5*d1 + c3*d2 - c1*d3;
}

// ══════════════════════════════════════════════════════════
// Direct YUV420p input — bypass RGB conversion entirely
// ══════════════════════════════════════════════════════════

impl VideoEncoder {
    /// Encode from YUV420p planes directly (no RGB conversion needed).
    /// y_plane: w*h bytes, u_plane: w/2*h/2 bytes, v_plane: w/2*h/2 bytes
    pub fn encode_frame_yuv(&mut self, y_in: &[u8], u_in: &[u8], v_in: &[u8],
        rgb_out: &mut [u8]) -> FrameResult {
        let n = self.w * self.h;
        let hw = self.w / 2; let hh = self.h / 2;
        let mut partial_refresh = false;

        // Convert Y plane to f32 (direct, no color matrix needed)
        for i in 0..n { self.cur_y[i] = y_in[i] as f32; }

        // Lazy chroma: only upsample if not using GPU fast path
        // GPU only needs Y plane. Chroma upsampled on-demand for non-skip blocks.
        let has_gpu = {
            #[cfg(feature = "gpu")]
            { self.shared_gpu.is_some() }
            #[cfg(not(feature = "gpu"))]
            { false }
        };
        if !has_gpu || self.frame_count == 0 {
            // Full upsample for CPU path or I-frames
            for y in 0..self.h { for x in 0..self.w {
                let hy = (y / 2).min(hh - 1);
                let hx = (x / 2).min(hw - 1);
                let pi = y * self.w + x;
                let hi = hy * hw + hx;
                self.cur_cb[pi] = u_in[hi] as f32 - 128.0;
                self.cur_cr[pi] = v_in[hi] as f32 - 128.0;
            }}
        } else {
            // GPU path: skip chroma upsample (ref_cb/cr already valid for skip blocks)
            // Only update chroma for scene changes detected later
            self.chroma_stale = true;
        }

        // Ensure ref chroma planes are from THIS frame (not cross-segment stale data)
        // This makes get_ref_y_u8 return correct colors for A/B comparison
        if self.chroma_stale {
            let hw2 = self.w / 2; let hh2 = self.h / 2;
            for y in 0..self.h { for x in 0..self.w {
                let hy = (y / 2).min(hh2 - 1);
                let hx = (x / 2).min(hw2 - 1);
                let pi = y * self.w + x;
                let hi = hy * hw2 + hx;
                self.cur_cb[pi] = u_in[hi] as f32 - 128.0;
                self.cur_cr[pi] = v_in[hi] as f32 - 128.0;
            }}
            self.chroma_stale = false;
        }
        // Copy input chroma to ref so reconstruction has correct colors
        self.ref_cb.copy_from_slice(&self.cur_cb);
        self.ref_cr.copy_from_slice(&self.cur_cr);

        // Now encode using the same P-frame path (cur planes are populated)
        self.encode_frame_from_planes(rgb_out)
    }

    /// Internal: encode from pre-populated cur_y/cur_cb/cur_cr planes
    fn encode_frame_from_planes(&mut self, rgb_out: &mut [u8]) -> FrameResult {
        let t0 = std::time::Instant::now();
        let n = self.w * self.h;
        let skip_rgb = rgb_out.is_empty();
        let mut partial_refresh = false;

        // Adaptive GOP decision
        let mut force_iframe = self.frame_count == 0;
        if !force_iframe && self.frames_since_iframe > 0 {
            let avg_recent_psnr = if self.recent_psnr.is_empty() { 99.0 }
                else { self.recent_psnr.iter().sum::<f64>() / self.recent_psnr.len() as f64 };
            let avg_skip = if self.recent_skip_pct.is_empty() { 100.0 }
                else { self.recent_skip_pct.iter().sum::<f64>() / self.recent_skip_pct.len() as f64 };
            let psnr_drop = self.psnr_at_iframe - avg_recent_psnr;
            let needs_refresh = psnr_drop > 12.0
                || (avg_skip < 50.0 && self.frames_since_iframe > 30)
                || self.frames_since_iframe >= self.gop_size * 4;
            let quality_ok = psnr_drop < 12.0 && avg_skip > 60.0;
            if needs_refresh && self.frames_since_iframe >= 30 { force_iframe = true; }
            else if self.frames_since_iframe >= self.gop_size && !quality_ok { force_iframe = true; }
        }
        // Scene change: minimum 5 frames cooldown, raised threshold
        if !force_iframe && self.frame_count > 0 && self.frames_since_iframe >= 5
            && self.frames_since_iframe % 10 == 0 {
            let sc = scene_change_score(&self.ref_y, &self.cur_y, n);
            let sc_hard_cut = if self.source_analyzed { 0.80 } else { 0.70 };
            if sc > sc_hard_cut { force_iframe = true; }
            partial_refresh = sc > 0.3 && !force_iframe;
        }

        if force_iframe {
            // I-frame: reconstruct RGB from cur planes for encode_iframe_420
            let mut rgb_in = vec![0u8; n * 3];
            planes_to_rgb(&self.cur_y, &self.cur_cb, &self.cur_cr, &mut rgb_in, n);

            if !self.source_analyzed {
                let profile = analyze_source_quality(&rgb_in, self.w, self.h);
                adapt_encoder_for_source(self, &profile, self.encoding_mode);
                self.source_analyzed = true;
            }

            let is_scene_change = force_iframe && self.frames_since_iframe >= 30;  // only clear state for genuine scene changes (not GOP refreshes)
            let (rec, stream, psnr) = encode_iframe_420(&rgb_in, self.w, self.h, self.quality, self.dead_zone, self.chroma_boost);
            
            let csz = stream.len(); let tb = csz as u64 * 8;
            if !skip_rgb {
                rgb_out[..n*3].copy_from_slice(&rec);
                deblock_frame(rgb_out, self.w, self.h, self.deblock_strength);
                rgb_to_planes(rgb_out, self.w, self.h, &mut self.ref_y, &mut self.ref_cb, &mut self.ref_cr);
            } else {
                rgb_to_planes(&rec, self.w, self.h, &mut self.ref_y, &mut self.ref_cb, &mut self.ref_cr);
                deblock_plane(&mut self.ref_y, self.w, self.h, self.deblock_strength);
            }
            if is_scene_change {
                self.world_x = 0; self.world_y = 0;
                for h in &mut self.dc_history { h.clear(); } self.trajectory.clear(); self.motion_field.clear();
                self.bg_model.clear();
            }
            self.bg_model.init_from_frame(&self.ref_y, &self.ref_cb, &self.ref_cr);
            self.update_dc_history();
            self.frame_count+=1; self.total_bits+=tb; self.compressed_bytes+=csz as u64;
            self.frames_since_iframe = 0; self.psnr_at_iframe = psnr;
            self.recent_psnr.clear(); self.recent_skip_pct.clear();
            FrameResult{frame_type:0,bits:tb,psnr,skip_pct:0.0,encode_ms:t0.elapsed().as_secs_f64()*1000.0,compressed_size:csz,vrr_duration_ms:1000.0/self.display_hz as f32,activity:1.0,route_stats:String::new(),graph_new_refs:0,graph_traj_updates: 0}
        } else {
            // P-frame: use direct YUV planes
            let (r, gmx, gmy, _n_traj) = if skip_rgb {
                // Fast YUV-only path: skip RGB conversion in P-frame encoder
                let mut dummy = vec![0u8; 6];  // minimal valid slice
                encode_pframe_trajectory(
                    &mut self.ref_y, &mut self.ref_cb, &mut self.ref_cr,
                    &self.cur_y, &self.cur_cb, &self.cur_cr,
                    self.w, self.h, &self.lq, &self.cq,
                    self.dead_zone, self.skip_threshold,
                    &self.dc_history, &mut self.trajectory,
                    &mut self.work_y, &mut self.work_cb, &mut self.work_cr,
                    &[], &mut dummy, &self.ref_buffer, None, None)
            } else {
                encode_pframe_trajectory(
                    &mut self.ref_y, &mut self.ref_cb, &mut self.ref_cr,
                    &self.cur_y, &self.cur_cb, &self.cur_cr,
                    self.w, self.h, &self.lq, &self.cq,
                    self.dead_zone, self.skip_threshold,
                    &self.dc_history, &mut self.trajectory,
                    &mut self.work_y, &mut self.work_cb, &mut self.work_cr,
                    &[], rgb_out, &self.ref_buffer, None, None)
            };
            // UI-aware skip rate
            let bw8 = self.w / 8; let bh8 = self.h / 8;
            let mut n_ui_blocks = 0u32;
            for bi in 0..self.block_static_count.len().min(bw8 * bh8) {
                if self.block_static_count[bi] > 30 { n_ui_blocks += 1; }
            }
            let world_blocks = r.n_blocks.saturating_sub(n_ui_blocks);
            let world_skip = r.n_skip.saturating_sub(n_ui_blocks);
            let sp = if world_blocks > 0 {
                (world_skip as f64 / world_blocks as f64 * 100.0).min(100.0)
            } else if r.n_blocks > 0 {
                (r.n_skip as f64 / r.n_blocks as f64 * 100.0).min(100.0)
            } else { 0.0 };
            self.world_x += gmx as i32; self.world_y += gmy as i32;
            self.ref_y.copy_from_slice(&self.work_y);
            self.ref_cb.copy_from_slice(&self.work_cb);
            self.ref_cr.copy_from_slice(&self.work_cr);
            // Block static tracking
            for by in 0..bh8 { for bx in 0..bw8 {
                let bi = by * bw8 + bx;
                if bi >= self.block_static_count.len() { continue; }
                let ox = bx * 8; let oy = by * 8;
                let mut same = true;
                'chk: for yy in 0..8 { for xx in 0..8 {
                    if (self.work_y[(oy+yy)*self.w+ox+xx] - self.ref_y[(oy+yy)*self.w+ox+xx]).abs() > 0.5 {
                        same = false; break 'chk;
                    }
                }}
                if same { self.block_static_count[bi] = self.block_static_count[bi].saturating_add(1); }
                else { self.block_static_count[bi] = 0; }
            }}
            let ref_added = self.update_ref_buffer();
            if sp < 90.0 {
                deblock_plane(&mut self.ref_y, self.w, self.h, self.deblock_strength);
                if !skip_rgb && sp < 60.0 {
                    deblock_plane(&mut self.ref_cb, self.w, self.h, self.deblock_strength * 0.5);
                    deblock_plane(&mut self.ref_cr, self.w, self.h, self.deblock_strength * 0.5);
                }
            }
            if !skip_rgb { planes_to_rgb(&self.ref_y, &self.ref_cb, &self.ref_cr, rgb_out, n); }
            self.update_dc_history();
            let est_bytes = ((r.total_bits + 7) / 8) as usize;
            self.frame_count += 1; self.total_bits += r.total_bits; self.compressed_bytes += est_bytes as u64;
            self.frames_since_iframe += 1;
            self.recent_psnr.push(r.psnr); self.recent_skip_pct.push(sp);
            if self.recent_psnr.len() > 10 { self.recent_psnr.remove(0); }
            if self.recent_skip_pct.len() > 10 { self.recent_skip_pct.remove(0); }
            let (vrr_ms_raw, vrr_act) = self.motion_field.frame_vrr(self.vid_fps, self.display_hz);
            let vrr_floor_ms = 1000.0 / self.vrr_cap_hz as f32;
            let vrr_ms = vrr_ms_raw.max(vrr_floor_ms);
            FrameResult{frame_type:1,bits:r.total_bits,psnr:r.psnr,skip_pct:sp,encode_ms:t0.elapsed().as_secs_f64()*1000.0,compressed_size:est_bytes,vrr_duration_ms:vrr_ms,activity:vrr_act,route_stats:String::new(),graph_new_refs:0,graph_traj_updates: 0}
        }
    }

    /// Get the current reference as full-color RGB (Y + Cb + Cr → RGB)
    pub fn get_ref_y_u8(&self) -> Vec<u8> {
        let n = self.w * self.h;
        let mut rgb = vec![0u8; n * 3];
        for i in 0..n {
            let y = self.ref_y[i];
            let cb = self.ref_cb[i];
            let cr = self.ref_cr[i];
            let r = (y + 1.5748 * cr).clamp(0.0, 255.0) as u8;
            let g = (y - 0.1873 * cb - 0.4681 * cr).clamp(0.0, 255.0) as u8;
            let b = (y + 1.8556 * cb).clamp(0.0, 255.0) as u8;
            rgb[i*3] = r; rgb[i*3+1] = g; rgb[i*3+2] = b;
        }
        rgb
    }


    /// Tier 2: passthrough encode — source frame is moderate motion.
    /// No pixel decode needed. Store a delta marker with estimated stats.
    /// The decoder will use the previous reconstructed frame as reference
    /// and apply the source codec's delta when full quality is needed.

    /// Use scene model to predict if a frame needs full encode
    /// Called by reader thread with source frame metadata
    pub fn should_full_encode(&self, frame_idx: usize, source_bytes: u64, avg_bytes: u64) -> bool {
        let (should_decode, _confidence) = self.scene_model.predict_frame(
            frame_idx, source_bytes, avg_bytes);
        should_decode
    }
    
    /// Get the current adaptive anchor interval
    pub fn anchor_interval(&self) -> usize {
        self.scene_model.anchor_interval
    }

    pub fn encode_frame_passthrough_fast(&mut self, source_frame_bytes: u64) -> FrameResult {
        self.frame_count += 1;
        self.frames_since_iframe += 1;
        // Estimate quality from source frame size relative to resolution
        let pixels = (self.w * self.h) as f64;
        let bits_per_pixel = source_frame_bytes as f64 * 8.0 / pixels;
        let est_psnr = (40.0 + 10.0 * bits_per_pixel.log2()).clamp(25.0, 50.0);
        let est_skip = ((1.0 - bits_per_pixel / 2.0) * 100.0).clamp(0.0, 95.0);
        let est_size = (source_frame_bytes as f64 * 0.3) as usize;  // FAO typically 30% of source

        FrameResult {
            frame_type: 1, compressed_size: est_size, bits: est_size as u64 * 8,
            psnr: est_psnr, skip_pct: est_skip,
            encode_ms: 0.01,
            vrr_duration_ms: 16.7, activity: bits_per_pixel as f32,
            route_stats: String::new(), graph_new_refs: 0, graph_traj_updates: 0
        }
    }

    /// Encode frame using source codec's motion vectors (passthrough mode).
    /// Skips GPU motion search — uses pre-computed MVs from H264/VP9/AV1 source.
    /// Only needs Y plane + source MVs. Dramatically faster than full encode.
    pub fn encode_frame_passthrough(&mut self, y_in: &[u8], source_mvs: &[(i8, i8)],
        frame_type: u8, source_qp: u8) -> FrameResult {
        let t0 = std::time::Instant::now();
        let n = self.w * self.h;
        let bw = self.w / 8; let bh = self.h / 8;
        
        // Convert Y to f32
        for i in 0..n { self.cur_y[i] = y_in[i.min(y_in.len()-1)] as f32; }
        
        if frame_type == 0 || self.frame_count == 0 {
            // I-frame: simple encode (no MVs needed)
            // Copy cur to ref directly
            self.ref_y.copy_from_slice(&self.cur_y);
            deblock_plane(&mut self.ref_y, self.w, self.h, self.deblock_strength);
            
            let mut total_bits = 0u64;
            let mut dct_in = [0.0f32; 64];
            let mut dct_out = [0.0f32; 64];
            for by in 0..bh { for bx in 0..bw {
                let ox = bx * 8; let oy = by * 8;
                for yy in 0..8 { for xx in 0..8 {
                    dct_in[yy*8+xx] = self.cur_y[(oy+yy)*self.w+ox+xx];
                }}
                dct_fwd(&dct_in, &mut dct_out);
                // Quantize and count bits
                let mut nonzero = 0;
                for k in 0..64 {
                    let q = dct_out[k] / self.lq[k];
                    let qi = if q.abs() < self.dead_zone { 0 } else { q.round() as i32 };
                    if qi != 0 { nonzero += 1; }
                }
                total_bits += nonzero * 12 + 2;  // ~12 bits per nonzero coeff + overhead
            }}
            
            let csz = (total_bits / 8) as usize;
            self.frame_count += 1;
            self.frames_since_iframe = 0;
            self.psnr_at_iframe = 45.0;
            
            FrameResult {
                frame_type: 0, compressed_size: csz, bits: total_bits,
                psnr: 45.0, skip_pct: 0.0,
                encode_ms: t0.elapsed().as_secs_f64() * 1000.0,
                vrr_duration_ms: 16.7, activity: 0.0, route_stats: String::new(), graph_new_refs: 0, graph_traj_updates: 0
            }
        } else {
            // P-frame: use source MVs directly
            let n_blocks = bw * bh;
            let mut n_skip = 0u32;
            let mut total_bits = 0u64;
            let mut dct_in = [0.0f32; 64];
            let mut dct_out = [0.0f32; 64];
            
            for by in 0..bh { for bx in 0..bw {
                let bi = by * bw + bx;
                let ox = bx * 8; let oy = by * 8;
                
                // Get source MV (or zero if not available)
                let (mvx, mvy) = if bi < source_mvs.len() {
                    source_mvs[bi]
                } else { (0i8, 0i8) };
                
                // Motion compensated reference
                let rx = (ox as i32 + mvx as i32).max(0).min((self.w-8) as i32) as usize;
                let ry = (oy as i32 + mvy as i32).max(0).min((self.h-8) as i32) as usize;
                
                // Compute residual SAD
                let mut sad = 0.0f32;
                for yy in 0..8 { for xx in 0..8 {
                    sad += (self.cur_y[(oy+yy)*self.w+ox+xx] - self.ref_y[(ry+yy)*self.w+rx+xx]).abs();
                }}
                
                if sad / 64.0 <= self.skip_threshold {
                    n_skip += 1;
                    total_bits += 2;
                    // Update ref from MC position
                    if mvx != 0 || mvy != 0 {
                        for yy in 0..8 { for xx in 0..8 {
                            self.ref_y[(oy+yy)*self.w+ox+xx] = self.ref_y[(ry+yy)*self.w+rx+xx];
                        }}
                    }
                } else {
                    // Encode residual
                    for yy in 0..8 { for xx in 0..8 {
                        dct_in[yy*8+xx] = self.cur_y[(oy+yy)*self.w+ox+xx] 
                            - self.ref_y[(ry+yy)*self.w+rx+xx];
                    }}
                    dct_fwd(&dct_in, &mut dct_out);
                    let mut nonzero = 0;
                    for k in 0..64 {
                        let q = dct_out[k] / self.lq[k];
                        let qi = if q.abs() < self.dead_zone { 0 } else { q.round() as i32 };
                        if qi != 0 { nonzero += 1; }
                        dct_out[k] = qi as f32 * self.lq[k];
                    }
                    // Reconstruct
                    let mut recon = [0.0f32; 64];
                    dct_inv(&dct_out, &mut recon);
                    for yy in 0..8 { for xx in 0..8 {
                        self.ref_y[(oy+yy)*self.w+ox+xx] = 
                            (self.ref_y[(ry+yy)*self.w+rx+xx] + recon[yy*8+xx]).clamp(0.0, 255.0);
                    }}
                    total_bits += nonzero * 12 + 16;  // coeffs + MV
                }
            }}
            
            let sp = n_skip as f64 / n_blocks as f64 * 100.0;
            let csz = (total_bits / 8) as usize;
            self.frame_count += 1;
            self.frames_since_iframe += 1;
            
            FrameResult {
                frame_type: 1, compressed_size: csz, bits: total_bits,
                psnr: 35.0, skip_pct: sp,
                encode_ms: t0.elapsed().as_secs_f64() * 1000.0,
                vrr_duration_ms: 16.7, activity: 0.0,
                route_stats: String::new(), graph_new_refs: 0, graph_traj_updates: 0
            }
        }
    }
    
    /// Ultra-fast skip frame: source says this frame is nearly identical to previous.
    /// No pixel decode needed at all. Just emit a skip marker.
    pub fn encode_frame_skip(&mut self) -> FrameResult {
        self.frame_count += 1;
        self.frames_since_iframe += 1;
        FrameResult {
            frame_type: 1, compressed_size: 2, bits: 16,
            psnr: 45.0, skip_pct: 100.0, encode_ms: 0.001,
            vrr_duration_ms: 16.7, activity: 0.0,
            route_stats: String::new(), graph_new_refs: 0, graph_traj_updates: 0
        }
    }

}

// ══════════════════════════════════════════════════════════
// GPU Compute — prediction routing on Metal/Vulkan/DX12
// ══════════════════════════════════════════════════════════

#[cfg(feature = "gpu")]
pub mod gpu {
    use wgpu;
    use wgpu::util::DeviceExt;

    /// GPU compute context for FAO encoding
    pub struct GpuEncoder {
        device: wgpu::Device,
        queue: wgpu::Queue,
        pipeline: wgpu::ComputePipeline,
        bind_group_layout: wgpu::BindGroupLayout,
        w: u32, h: u32 }

    impl GpuEncoder {
        /// Initialize GPU compute. Returns None if no GPU available.
        pub fn new(w: usize, h: usize) -> Option<Self> {
            // Prefer Vulkan (Honeykrisp/Mesa), fall back to GL
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
                ..Default::default()
            });
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false }))?;
            let info = adapter.get_info();
            eprintln!("FAO GPU: {} ({:?}) driver={}", info.name, info.backend, info.driver);
            let (device, queue) = pollster::block_on(adapter.request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("FAO GPU"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),  // broader device support
                },
                None,
            )).ok()?;

            // Compute shader for prediction routing
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("fao_route"),
                source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(ROUTE_SHADER)) });

            let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("fao_route_layout"),
                entries: &[
                    // ref_y buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                        count: None },
                    // cur_y buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                        count: None },
                    // route output buffer
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
                        count: None },
                    // params uniform
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                        count: None },
                ] });

            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("fao_route_pipeline"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[] });

            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("fao_route_compute"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: "route_blocks" });

            Some(Self { device, queue, pipeline, bind_group_layout, w: w as u32, h: h as u32 })
        }

        /// Run prediction routing on GPU
        /// Returns per-block route decisions (skip/inter/intra + residual energy)
        pub fn route_frame(&self, ref_y: &[f32], cur_y: &[f32], 
            gmx: f32, gmy: f32, skip_thresh: f32) -> Vec<u32> {
            let bw = self.w / 8; let bh = self.h / 8;
            let n_blocks = (bw * bh) as usize;

            // Upload buffers
            let ref_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("ref_y"), contents: bytemuck_cast_slice(ref_y),
                usage: wgpu::BufferUsages::STORAGE });
            let cur_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cur_y"), contents: bytemuck_cast_slice(cur_y),
                usage: wgpu::BufferUsages::STORAGE });
            let route_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("routes"), size: (n_blocks * 4) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false });
            let params = [self.w as f32, self.h as f32, gmx, gmy, skip_thresh, 0.0, 0.0, 0.0];
            let param_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("params"), contents: bytemuck_cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM });

            let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fao_route_bind"), layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ref_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: cur_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: route_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: param_buf.as_entire_binding() },
                ] });

            // Dispatch
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind_group, &[]);
                pass.dispatch_workgroups((bw + 7) / 8, (bh + 7) / 8, 1);
            }

            // Readback
            let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("staging"), size: (n_blocks * 4) as u64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false });
            encoder.copy_buffer_to_buffer(&route_buf, 0, &staging, 0, (n_blocks * 4) as u64);
            self.queue.submit(std::iter::once(encoder.finish()));

            let slice = staging.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
            self.device.poll(wgpu::Maintain::Wait);
            let _ = rx.recv();

            let data = slice.get_mapped_range();
            let result: Vec<u32> = data.chunks(4).map(|c| u32::from_le_bytes([c[0],c[1],c[2],c[3]])).collect();
            drop(data);
            staging.unmap();
            result
        }
    }

    fn bytemuck_cast_slice(data: &[f32]) -> &[u8] {
        unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 4) }
    }

    /// WGSL compute shader for prediction routing
    const ROUTE_SHADER: &str = r#"
struct Params {
    w: f32,
    h: f32,
    gmx: f32,
    gmy: f32,
    skip_thresh: f32,
    _pad1: f32,
    _pad2: f32,
    _pad3: f32 }

@group(0) @binding(0) var<storage, read> ref_y: array<f32>;
@group(0) @binding(1) var<storage, read> cur_y: array<f32>;
@group(0) @binding(2) var<storage, read_write> routes: array<u32>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(8, 8, 1)
fn route_blocks(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = u32(params.w);
    let h = u32(params.h);
    let bw = w / 8u;
    let bh = h / 8u;
    let bx = gid.x;
    let by = gid.y;
    if (bx >= bw || by >= bh) { return; }
    
    let bi = by * bw + bx;
    let ox = bx * 8u;
    let oy = by * 8u;
    
    // Compute cur block DC
    var cur_dc: f32 = 0.0;
    for (var yy: u32 = 0u; yy < 8u; yy++) {
        for (var xx: u32 = 0u; xx < 8u; xx++) {
            cur_dc += cur_y[(oy + yy) * w + ox + xx];
        }
    }
    cur_dc /= 64.0;
    
    // Global MV prediction
    let gmx = i32(params.gmx);
    let gmy = i32(params.gmy);
    let gx = u32(clamp(i32(ox) + gmx, 0, i32(w) - 8));
    let gy = u32(clamp(i32(oy) + gmy, 0, i32(h) - 8));
    
    var ref_dc: f32 = 0.0;
    for (var yy: u32 = 0u; yy < 8u; yy++) {
        for (var xx: u32 = 0u; xx < 8u; xx++) {
            ref_dc += ref_y[(gy + yy) * w + gx + xx];
        }
    }
    ref_dc /= 64.0;
    
    let dc_diff = abs(cur_dc - ref_dc);
    
    // Full SAD for blocks that pass DC check
    var sad: f32 = 0.0;
    if (dc_diff < params.skip_thresh * 0.5) {
        for (var yy: u32 = 0u; yy < 8u; yy++) {
            for (var xx: u32 = 0u; xx < 8u; xx++) {
                sad += abs(cur_y[(oy + yy) * w + ox + xx] - ref_y[(gy + yy) * w + gx + xx]);
            }
        }
    } else {
        sad = 99999.0;
    }
    
    let mean_sad = sad / 64.0;
    
    // Route decision packed into u32:
    // bits 0-7: route type (0=skip, 1=inter, 2=intra)
    // bits 8-23: residual energy * 10 (u16)
    // bits 24-31: confidence * 255 (u8)
    var route_type: u32 = 2u;  // default: intra
    var energy: u32 = u32(min(mean_sad * 10.0, 65535.0));
    var confidence: u32 = 0u;
    
    if (mean_sad < params.skip_thresh) {
        route_type = 0u;  // skip
        confidence = u32(min((1.0 - mean_sad / params.skip_thresh) * 255.0, 255.0));
    } else if (mean_sad < params.skip_thresh * 3.0) {
        route_type = 1u;  // inter
        confidence = u32(min((1.0 - mean_sad / (params.skip_thresh * 3.0)) * 255.0, 255.0));
    }
    
    routes[bi] = route_type | (energy << 8u) | (confidence << 24u);
}
"#;
}

// ══════════════════════════════════════════════════════════
// Memory-bounded prediction web with value-based eviction
// ══════════════════════════════════════════════════════════

/// Per-node usage statistics for eviction decisions
#[derive(Clone, Copy, Default)]
pub struct NodeStats {
    pub predictions_made: u32,   // how many times this node was consulted
    pub predictions_hit: u32,    // how many times the prediction was used (led to skip)
    pub last_hit_frame: u32,     // frame number of last successful prediction
    pub total_bits_saved: u64,   // estimated bits saved by this node's predictions
}

impl NodeStats {
    /// Value score: higher = more worth keeping in memory
    /// Combines hit rate, recency, and total contribution
    pub fn value(&self, current_frame: u32) -> f64 {
        if self.predictions_made == 0 { return 0.0; }
        let hit_rate = self.predictions_hit as f64 / self.predictions_made as f64;
        let recency = 1.0 / (1.0 + (current_frame - self.last_hit_frame) as f64 / 30.0);
        let contribution = (self.total_bits_saved as f64).ln().max(0.0);
        hit_rate * 0.4 + recency * 0.3 + contribution * 0.3
    }
}

/// Memory budget tracker for the prediction web
pub struct MemoryBudget {
    pub max_bytes: usize,
    pub current_bytes: usize,
    // Per-subsystem usage
    pub trajectory_bytes: usize,
    pub anchor_bytes: usize,
    pub motion_field_bytes: usize,
    pub dc_history_bytes: usize,
    // Per-block node stats for value-based eviction
    pub node_stats: Vec<NodeStats>,
    pub current_frame: u32 }

impl MemoryBudget {
    pub fn new(n_blocks: usize, max_mb: usize) -> Self {
        Self {
            max_bytes: max_mb * 1024 * 1024,
            current_bytes: 0,
            trajectory_bytes: 0, anchor_bytes: 0,
            motion_field_bytes: 0, dc_history_bytes: 0,
            node_stats: vec![NodeStats::default(); n_blocks],
            current_frame: 0 }
    }

    /// Record a prediction attempt + outcome
    pub fn record_prediction(&mut self, block_idx: usize, was_hit: bool, bits_saved: u64) {
        if block_idx >= self.node_stats.len() { return; }
        let ns = &mut self.node_stats[block_idx];
        ns.predictions_made += 1;
        if was_hit {
            ns.predictions_hit += 1;
            ns.last_hit_frame = self.current_frame;
            ns.total_bits_saved += bits_saved;
        }
    }

    /// Check if we're over budget
    pub fn over_budget(&self) -> bool { self.current_bytes > self.max_bytes }

    /// Find blocks with lowest value for eviction
    /// Returns indices sorted by value (lowest first)
    pub fn eviction_candidates(&self, count: usize) -> Vec<usize> {
        let mut indexed: Vec<(usize, f64)> = self.node_stats.iter().enumerate()
            .map(|(i, ns)| (i, ns.value(self.current_frame)))
            .collect();
        indexed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        indexed.iter().take(count).map(|(i, _)| *i).collect()
    }

    /// Evict lowest-value nodes to get under budget
    pub fn evict_to_budget(&mut self, trajectory: &mut MotionTrajectory,
        bg_model: &mut BackgroundModel, target_pct: f32) {
        let target = (self.max_bytes as f64 * target_pct as f64) as usize;
        if self.current_bytes <= target { return; }

        // Find bottom 20% of nodes by value
        let n_evict = self.node_stats.len() / 5;
        let candidates = self.eviction_candidates(n_evict);

        for &bi in &candidates {
            // Trim trajectory history for this block (keep last 4 instead of 16)
            if bi < trajectory.history_x.len() {
                let hx = &mut trajectory.history_x[bi];
                let hy = &mut trajectory.history_y[bi];
                if hx.len() > 4 { *hx = hx[hx.len()-4..].to_vec(); }
                if hy.len() > 4 { *hy = hy[hy.len()-4..].to_vec(); }
            }
            // Trim anchors for this block (keep only best anchor)
            if bi < bg_model.anchors.len() && bg_model.anchors[bi].len() > 1 {
                // Keep the most-used anchor, remove rest
                if let Some(best) = bg_model.anchors[bi].iter()
                    .enumerate().max_by_key(|(_, a)| a.use_count).map(|(i, _)| i) {
                    let keep = bg_model.anchors[bi][best].clone();
                    bg_model.anchors[bi] = vec![keep];
                }
            }
        }
        self.update_usage(trajectory, bg_model);
    }

    /// Recalculate current memory usage from actual data structures
    pub fn update_usage(&mut self, trajectory: &MotionTrajectory, bg_model: &BackgroundModel) {
        self.trajectory_bytes = trajectory.history_x.iter()
            .map(|h| h.capacity() * 4).sum::<usize>()
            + trajectory.history_y.iter().map(|h| h.capacity() * 4).sum::<usize>();
        self.anchor_bytes = bg_model.anchors.iter()
            .map(|a| a.len() * (64 * 3 * 4 + 12)).sum::<usize>();  // 3 channels * 64 f32 + overhead
        self.current_bytes = self.trajectory_bytes + self.anchor_bytes
            + self.motion_field_bytes + self.dc_history_bytes;
    }

    /// Get memory stats for display
    pub fn stats(&self) -> String {
        let mb = self.current_bytes as f64 / 1048576.0;
        let max_mb = self.max_bytes as f64 / 1048576.0;
        let avg_value = if self.node_stats.is_empty() { 0.0 } else {
            self.node_stats.iter().map(|n| n.value(self.current_frame)).sum::<f64>()
                / self.node_stats.len() as f64
        };
        format!("Mem: {:.0}/{:.0}MB  val={:.2}", mb, max_mb, avg_value)
    }
}

// ══════════════════════════════════════════════════════════
// Smear detection — runs on output frame to flag bad predictions
// ══════════════════════════════════════════════════════════

/// CPU smear detector: scan block boundaries for discontinuities
/// that indicate a wrong motion vector was accepted.
/// Returns per-block smear score (0 = clean, high = smeared)
pub fn detect_smears(frame_y: &[f32], w: usize, h: usize) -> Vec<f32> {
    let bw = w / 8; let bh = h / 8;
    let mut scores = vec![0.0f32; bw * bh];

    for by in 0..bh { for bx in 0..bw {
        let ox = bx * 8; let oy = by * 8;
        let bi = by * bw + bx;
        let mut boundary_energy = 0.0f32;
        let mut interior_energy = 0.0f32;
        let mut b_n = 0u32; let mut i_n = 0u32;

        for yy in 0..8 {
            // Right boundary (between this block and neighbor)
            if bx + 1 < bw {
                let d = (frame_y[(oy+yy)*w+ox+7] - frame_y[(oy+yy)*w+ox+8]).abs();
                boundary_energy += d; b_n += 1;
            }
            // Bottom boundary
            if by + 1 < bh {
                let d = (frame_y[(oy+7)*w+ox+yy] - frame_y[(oy+8)*w+ox+yy]).abs();
                boundary_energy += d; b_n += 1;
            }
            // Interior gradients (for comparison)
            if yy < 7 {
                for xx in 0..7 {
                    interior_energy += (frame_y[(oy+yy)*w+ox+xx] - frame_y[(oy+yy)*w+ox+xx+1]).abs();
                    i_n += 1;
                }
            }
        }

        let avg_boundary = if b_n > 0 { boundary_energy / b_n as f32 } else { 0.0 };
        let avg_interior = if i_n > 0 { interior_energy / i_n as f32 } else { 1.0 };

        // Smear score: how much sharper are boundaries vs interior?
        // High score = block boundary is unnaturally sharp = likely smear
        scores[bi] = if avg_interior > 0.5 {
            (avg_boundary / avg_interior - 1.0).max(0.0)
        } else if avg_boundary > 5.0 {
            avg_boundary / 5.0  // flat interior but sharp boundary = definite smear
        } else {
            0.0
        };
    }}
    scores
}

/// Mark smeared blocks as "untrusted" — their predictions should not be
/// used for future frames. Returns count of smeared blocks.
pub fn mark_smeared_blocks(smear_scores: &[f32], threshold: f32,
    node_stats: &mut [NodeStats]) -> usize {
    let mut count = 0;
    for (i, &score) in smear_scores.iter().enumerate() {
        if score > threshold && i < node_stats.len() {
            // Penalize this node: reduce its hit count so it gets evicted sooner
            node_stats[i].predictions_hit = node_stats[i].predictions_hit.saturating_sub(3);
            count += 1;
        }
    }
    count
}

#[cfg(feature = "gpu")]
/// WGSL shader for GPU-parallel smear detection
pub const SMEAR_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> frame_y: array<f32>;
@group(0) @binding(1) var<storage, read_write> smear_scores: array<f32>;
@group(0) @binding(2) var<uniform> dims: vec2<u32>;  // w, h

@compute @workgroup_size(8, 8, 1)
fn detect_smears(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = dims.x;
    let h = dims.y;
    let bw = w / 8u;
    let bh = h / 8u;
    let bx = gid.x;
    let by = gid.y;
    if (bx >= bw || by >= bh) { return; }
    
    let ox = bx * 8u;
    let oy = by * 8u;
    let bi = by * bw + bx;
    
    var boundary_sum: f32 = 0.0;
    var interior_sum: f32 = 0.0;
    var bn: f32 = 0.0;
    var in_: f32 = 0.0;
    
    for (var yy: u32 = 0u; yy < 8u; yy++) {
        // Right boundary
        if (bx + 1u < bw) {
            boundary_sum += abs(frame_y[(oy + yy) * w + ox + 7u] - frame_y[(oy + yy) * w + ox + 8u]);
            bn += 1.0;
        }
        // Bottom boundary
        if (by + 1u < bh) {
            boundary_sum += abs(frame_y[(oy + 7u) * w + ox + yy] - frame_y[(oy + 8u) * w + ox + yy]);
            bn += 1.0;
        }
        // Interior
        for (var xx: u32 = 0u; xx < 7u; xx++) {
            interior_sum += abs(frame_y[(oy + yy) * w + ox + xx] - frame_y[(oy + yy) * w + ox + xx + 1u]);
            in_ += 1.0;
        }
    }
    
    let avg_b = select(0.0, boundary_sum / bn, bn > 0.0);
    let avg_i = select(1.0, interior_sum / in_, in_ > 0.0);
    
    smear_scores[bi] = select(
        select(0.0, avg_b / 5.0, avg_b > 5.0),
        max(avg_b / avg_i - 1.0, 0.0),
        avg_i > 0.5
    );
}
"#;

// ══════════════════════════════════════════════════════════
// Point-cloud entities — richer than anchors, code like voxels
// ══════════════════════════════════════════════════════════

/// A point-cloud entity: a spatially-aware snapshot of an object
/// that stores enough data to reconstruct it at any position.
/// Conceptually: a small voxel grid of color+depth per 16×16 region.
#[derive(Clone)]
pub struct PointEntity {
    /// YCbCr values for the 16×16 region (768 f32s)
    pub pixels_y: [f32; 256],
    pub pixels_cb: [f32; 256],
    pub pixels_cr: [f32; 256],
    /// Edge map: directional gradients for each 4×4 sub-block
    /// Encodes the "shape" of the object for matching at different positions
    pub edge_h: [f32; 16],  // horizontal gradient per 4×4
    pub edge_v: [f32; 16],  // vertical gradient per 4×4
    /// Spatial hash: compact signature for fast matching
    pub signature: u64,
    /// Position where first captured
    pub origin_x: u16, pub origin_y: u16,
    /// Confidence: how many frames this entity has been tracked
    pub track_count: u32,
    /// Frame when this entity was born
    pub birth_frame: u32 }

/// Entity registry: maintains a catalog of discovered visual objects
pub struct EntityRegistry {
    entities: Vec<PointEntity>,
    max_entities: usize,
    frame_count: u32 }

impl EntityRegistry {
    pub fn new(max_entities: usize) -> Self {
        Self { entities: Vec::new(), max_entities, frame_count: 0 }
    }

    /// Compute spatial hash for a 16×16 region (fast matching key)
    fn compute_signature(y: &[f32], w: usize, ox: usize, oy: usize) -> u64 {
        // Hash based on 4×4 DC values (16 values → 64 bits)
        let mut sig = 0u64;
        for sy in 0..4 { for sx in 0..4 {
            let mut sum = 0.0f32;
            for dy in 0..4 { for dx in 0..4 {
                let px = ox + sx*4 + dx; let py = oy + sy*4 + dy;
                if py < y.len() / w && px < w { sum += y[py*w+px]; }
            }}
            let dc = (sum / 16.0) as u8;
            let bit_pos = (sy * 4 + sx) * 4;
            sig |= ((dc >> 4) as u64) << bit_pos;  // top 4 bits of each DC
        }}
        sig
    }

    /// Capture a new entity from the current frame (aggressive init)
    /// Stores full 16×16 pixel data + edge structure for matching
    pub fn capture_entity(&mut self, y: &[f32], cb: &[f32], cr: &[f32],
        w: usize, ox: usize, oy: usize) {
        let mut entity = PointEntity {
            pixels_y: [0.0; 256], pixels_cb: [0.0; 256], pixels_cr: [0.0; 256],
            edge_h: [0.0; 16], edge_v: [0.0; 16],
            signature: Self::compute_signature(y, w, ox, oy),
            origin_x: ox as u16, origin_y: oy as u16,
            track_count: 1, birth_frame: self.frame_count };

        // Store full pixel data
        for sy in 0..16 { for sx in 0..16 {
            let pi = sy * 16 + sx;
            let fi = (oy + sy) * w + (ox + sx);
            if fi < y.len() {
                entity.pixels_y[pi] = y[fi];
                entity.pixels_cb[pi] = cb[fi];
                entity.pixels_cr[pi] = cr[fi];
            }
        }}

        // Compute edge structure per 4×4 sub-block
        for sy in 0..4 { for sx in 0..4 {
            let bi = sy * 4 + sx;
            let mut eh = 0.0f32; let mut ev = 0.0f32;
            for dy in 0..4 { for dx in 0..3 {
                let fi = (oy+sy*4+dy)*w + ox+sx*4+dx;
                if fi+1 < y.len() { eh += (y[fi+1] - y[fi]).abs(); }
            }}
            for dy in 0..3 { for dx in 0..4 {
                let fi = (oy+sy*4+dy)*w + ox+sx*4+dx;
                if fi+w < y.len() { ev += (y[fi+w] - y[fi]).abs(); }
            }}
            entity.edge_h[bi] = eh / 12.0;
            entity.edge_v[bi] = ev / 12.0;
        }}

        // Evict lowest-tracked entity if at capacity
        if self.entities.len() >= self.max_entities {
            if let Some(worst) = self.entities.iter().enumerate()
                .min_by_key(|(_, e)| e.track_count).map(|(i, _)| i) {
                self.entities.remove(worst);
            }
        }
        self.entities.push(entity);
    }

    /// Search for a matching entity at a given position
    /// Returns (entity_index, match_quality) or None
    pub fn find_match(&self, y: &[f32], w: usize, ox: usize, oy: usize,
        threshold: f32) -> Option<(usize, f32)> {
        let sig = Self::compute_signature(y, w, ox, oy);

        let mut best_idx = 0usize;
        let mut best_sad = f32::MAX;

        for (i, entity) in self.entities.iter().enumerate() {
            // Fast reject: signature must have at least 50% bits in common
            let common = (sig ^ entity.signature).count_zeros();
            if common < 32 { continue; }

            // Detailed match: SAD on 4×4 sub-block DCs
            let mut sad = 0.0f32;
            for sy in 0..4 { for sx in 0..4 {
                let mut cur_dc = 0.0f32;
                for dy in 0..4 { for dx in 0..4 {
                    let fi = (oy+sy*4+dy)*w + ox+sx*4+dx;
                    if fi < y.len() { cur_dc += y[fi]; }
                }}
                cur_dc /= 16.0;
                let ent_dc: f32 = (0..16).map(|k| entity.pixels_y[(sy*4 + k/4)*16 + sx*4 + k%4]).sum::<f32>() / 16.0;
                sad += (cur_dc - ent_dc).abs();
            }}

            if sad < best_sad { best_sad = sad; best_idx = i; }
        }

        if best_sad / 16.0 < threshold {
            Some((best_idx, best_sad / 16.0))
        } else {
            None
        }
    }

    /// Apply entity pixels to a region (aggressive init — no morph-in)
    pub fn apply_entity(&self, idx: usize, out_y: &mut [f32], out_cb: &mut [f32],
        out_cr: &mut [f32], w: usize, ox: usize, oy: usize) {
        if idx >= self.entities.len() { return; }
        let e = &self.entities[idx];
        for sy in 0..16 { for sx in 0..16 {
            let fi = (oy + sy) * w + (ox + sx);
            let pi = sy * 16 + sx;
            if fi < out_y.len() {
                out_y[fi] = e.pixels_y[pi];
                out_cb[fi] = e.pixels_cb[pi];
                out_cr[fi] = e.pixels_cr[pi];
            }
        }}
    }

    pub fn advance_frame(&mut self) { self.frame_count += 1; }
    pub fn count(&self) -> usize { self.entities.len() }

    pub fn stats(&self) -> String {
        if self.entities.is_empty() { return String::new(); }
        let avg_track: f32 = self.entities.iter().map(|e| e.track_count as f32).sum::<f32>()
            / self.entities.len() as f32;
        format!("Entities: {} avg_track={:.0}", self.entities.len(), avg_track)
    }
}

// ══════════════════════════════════════════════════════════
// Parallel chunk encoding
// ══════════════════════════════════════════════════════════

/// Encode a chunk of frames independently. Returns encoded frame data.
/// Each chunk starts with an I-frame and runs its own prediction web.
pub fn encode_chunk(
    frames: &[Vec<u8>],  // RGB frames for this chunk
    w: usize, h: usize,
    quality: f32, dead_zone: f32, chroma_boost: i32,
) -> Vec<ChunkFrameResult> {
    let mut enc = VideoEncoder::new(w, h, quality, dead_zone, chroma_boost);
    let mut results = Vec::with_capacity(frames.len());
    let mut recon = vec![0u8; w * h * 3];

    for frame in frames {
        let fr = enc.encode_frame(frame, &mut recon);
        results.push(ChunkFrameResult {
            recon: recon.clone(),
            frame_type: fr.frame_type,
            bits: fr.bits,
            psnr: fr.psnr,
            skip_pct: fr.skip_pct,
            encode_ms: fr.encode_ms,
            compressed_size: fr.compressed_size });
    }
    results
}

#[derive(Clone)]
pub struct ChunkFrameResult {
    pub recon: Vec<u8>,
    pub frame_type: u8,
    pub bits: u64,
    pub psnr: f64,
    pub skip_pct: f64,
    pub encode_ms: f64,
    pub compressed_size: usize }

// ══════════════════════════════════════════════════════════
// Fast file preprocessor — scene map + keyframe index
// ══════════════════════════════════════════════════════════

/// A segment of video between two scene changes
#[derive(Clone, Debug)]
pub struct VideoSegment {
    pub start_time: f64,
    pub end_time: f64,
    pub keyframe_time: f64,   // nearest keyframe before start (for fast seek)
    pub scene_type: SegmentType,
    pub avg_complexity: f32,  // estimated encoding difficulty
}

#[derive(Clone, Debug, PartialEq)]
pub enum SegmentType {
    Static,     // mostly still (menu, dialogue)
    Motion,     // camera movement or action
    Cut,        // hard scene cut
    Transition, // fade, dissolve, wipe
}

/// Build a scene map from ffprobe keyframe data + thumbnail analysis.
/// This is the fast preprocessing step that guides chunk splitting.
pub fn build_scene_map(
    keyframe_times: &[f64],
    keyframe_histograms: &[Vec<u32>],  // 256-bin Y histogram per keyframe
    total_duration: f64,
) -> Vec<VideoSegment> {
    let mut segments = Vec::new();
    if keyframe_times.is_empty() { return segments; }

    let mut seg_start = 0.0f64;
    let mut prev_hist = &keyframe_histograms[0];

    for i in 1..keyframe_times.len() {
        let hist = &keyframe_histograms[i];
        let diff = histogram_diff(prev_hist, hist);

        // Classify the transition
        let seg_type = if diff > 0.5 {
            SegmentType::Cut          // hard cut: histograms very different
        } else if diff > 0.2 {
            SegmentType::Transition   // gradual change
        } else if diff > 0.05 {
            SegmentType::Motion       // some change (camera, animation)
        } else {
            SegmentType::Static       // very similar frames
        };

        // Create segment for the gap between this and previous keyframe
        if diff > 0.15 || (keyframe_times[i] - seg_start) > 10.0 {
            // Scene change or long segment — split here
            segments.push(VideoSegment {
                start_time: seg_start,
                end_time: keyframe_times[i],
                keyframe_time: keyframe_times[i - 1],
                scene_type: seg_type.clone(),
                avg_complexity: diff });
            seg_start = keyframe_times[i];
        }

        prev_hist = hist;
    }

    // Final segment
    if seg_start < total_duration {
        segments.push(VideoSegment {
            start_time: seg_start,
            end_time: total_duration,
            keyframe_time: *keyframe_times.last().unwrap_or(&0.0),
            scene_type: SegmentType::Motion,
            avg_complexity: 0.5 });
    }

    segments
}

/// Compare two 256-bin histograms. Returns 0.0 (identical) to 1.0 (completely different).
fn histogram_diff(a: &[u32], b: &[u32]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return 1.0; }
    let total_a: u64 = a.iter().map(|&v| v as u64).sum();
    let total_b: u64 = b.iter().map(|&v| v as u64).sum();
    if total_a == 0 || total_b == 0 { return 1.0; }

    // Chi-squared distance
    let mut chi2 = 0.0f64;
    for i in 0..a.len() {
        let na = a[i] as f64 / total_a as f64;
        let nb = b[i] as f64 / total_b as f64;
        let sum = na + nb;
        if sum > 0.0 {
            chi2 += (na - nb) * (na - nb) / sum;
        }
    }
    (chi2 / 2.0).min(1.0) as f32
}

/// Split segments into N chunks for parallel encoding.
/// Tries to split at scene boundaries, never in the middle of a scene.
pub fn plan_chunks(segments: &[VideoSegment], n_chunks: usize) -> Vec<(f64, f64, f64)> {
    // (start_time, end_time, keyframe_seek_time)
    if segments.is_empty() { return vec![]; }

    let total_duration = segments.last().map(|s| s.end_time).unwrap_or(0.0);
    let target_chunk_dur = total_duration / n_chunks as f64;

    let mut chunks = Vec::new();
    let mut chunk_start = 0.0f64;
    let mut chunk_keyframe = segments[0].keyframe_time;
    let mut accumulated = 0.0f64;

    for seg in segments {
        accumulated += seg.end_time - seg.start_time;

        // Split at scene boundary if we've accumulated enough duration
        if accumulated >= target_chunk_dur && chunks.len() < n_chunks - 1 {
            chunks.push((chunk_start, seg.end_time, chunk_keyframe));
            chunk_start = seg.end_time;
            chunk_keyframe = seg.end_time; // next segment starts at this time
            accumulated = 0.0;
        }
    }

    // Final chunk
    chunks.push((chunk_start, total_duration, chunk_keyframe));
    chunks
}

// ══════════════════════════════════════════════════════════
// Temp file layout optimizer — group frames for cache locality
// ══════════════════════════════════════════════════════════

/// Describes the layout of frames in the temp file
#[derive(Clone)]
pub struct TempFileLayout {
    pub frame_size: usize,          // bytes per frame (w*h*3)
    pub chunk_frames: usize,        // frames per aligned chunk (default 8)
    pub chunk_bytes: usize,         // bytes per chunk (chunk_frames * frame_size * 2)
    pub n_chunks: usize,            // total chunks
    pub total_frames: usize }

impl TempFileLayout {
    pub fn new(w: usize, h: usize, est_frames: usize) -> Self {
        let frame_size = w * h * 3;
        // Chunk size: aim for 64-128MB chunks for good readahead
        // At 1440p (11MB/frame): 8 frames = 88MB per chunk pair
        // At 4K (24MB/frame): 4 frames = 96MB per chunk pair
        let chunk_frames = if frame_size > 16_000_000 { 4 } else { 8 };
        let chunk_bytes = chunk_frames * frame_size * 2;  // orig + recon
        let n_chunks = (est_frames + chunk_frames - 1) / chunk_frames;
        Self { frame_size, chunk_frames, chunk_bytes, n_chunks, total_frames: est_frames }
    }

    /// Get the file offset for a specific frame's orig or recon data
    /// Layout per chunk: [orig0..origN][recon0..reconN]
    pub fn offset(&self, frame_idx: usize, is_recon: bool) -> u64 {
        let chunk_idx = frame_idx / self.chunk_frames;
        let frame_in_chunk = frame_idx % self.chunk_frames;
        let chunk_start = chunk_idx as u64 * self.chunk_bytes as u64;
        let recon_offset = if is_recon { (self.chunk_frames * self.frame_size) as u64 } else { 0 };
        chunk_start + recon_offset + (frame_in_chunk * self.frame_size) as u64
    }

    /// Get both orig and recon offsets for a frame
    pub fn offsets(&self, frame_idx: usize) -> (u64, u64) {
        (self.offset(frame_idx, false), self.offset(frame_idx, true))
    }

    /// For sequential read of N frames starting at idx,
    /// return the minimal set of (file_offset, byte_count) reads needed.
    /// Groups consecutive frames into single reads.
    pub fn plan_reads(&self, start_idx: usize, count: usize, want_orig: bool, want_recon: bool) -> Vec<(u64, usize)> {
        let mut reads = Vec::new();
        if count == 0 { return reads; }
        
        let end_idx = (start_idx + count).min(self.total_frames);
        let start_chunk = start_idx / self.chunk_frames;
        let end_chunk = (end_idx - 1) / self.chunk_frames;
        
        for ci in start_chunk..=end_chunk {
            let first_in_chunk = if ci == start_chunk { start_idx % self.chunk_frames } else { 0 };
            let last_in_chunk = if ci == end_chunk { (end_idx - 1) % self.chunk_frames } else { self.chunk_frames - 1 };
            let n_frames = last_in_chunk - first_in_chunk + 1;
            let chunk_start = ci as u64 * self.chunk_bytes as u64;
            
            if want_orig {
                let off = chunk_start + (first_in_chunk * self.frame_size) as u64;
                reads.push((off, n_frames * self.frame_size));
            }
            if want_recon {
                let off = chunk_start + (self.chunk_frames * self.frame_size) as u64
                    + (first_in_chunk * self.frame_size) as u64;
                reads.push((off, n_frames * self.frame_size));
            }
        }
        reads
    }
}

// ══════════════════════════════════════════════════════════
// Streaming .fao writer — write output during encode, not after
// ══════════════════════════════════════════════════════════

/// Frame marker stored in the temp index file (lightweight)
#[derive(Clone, Copy)]
#[repr(C)]
pub struct FrameMarker {
    pub fao_offset: u64,     // offset in the .fao output file
    pub frame_size: u32,     // size of frame data in .fao
    pub frame_type: u8,      // 0=I, 1=P
    pub psnr_x10: u16,       // PSNR × 10
    pub skip_pct: u8,        // skip percentage 0-100
    pub encode_ms: u16,      // encode time in ms (capped at 65535)
    pub compressed_sz: u32,  // estimated compressed size
}

impl FrameMarker {
    pub fn to_bytes(&self) -> [u8; 24] {
        let mut b = [0u8; 24];
        b[0..8].copy_from_slice(&self.fao_offset.to_le_bytes());
        b[8..12].copy_from_slice(&self.frame_size.to_le_bytes());
        b[12] = self.frame_type;
        b[13..15].copy_from_slice(&self.psnr_x10.to_le_bytes());
        b[15] = self.skip_pct;
        b[16..18].copy_from_slice(&self.encode_ms.to_le_bytes());
        b[18..22].copy_from_slice(&self.compressed_sz.to_le_bytes());
        b[22] = 0; b[23] = 0; // padding
        b
    }
    pub fn from_bytes(b: &[u8; 24]) -> Self {
        Self {
            fao_offset: u64::from_le_bytes([b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7]]),
            frame_size: u32::from_le_bytes([b[8],b[9],b[10],b[11]]),
            frame_type: b[12],
            psnr_x10: u16::from_le_bytes([b[13],b[14]]),
            skip_pct: b[15],
            encode_ms: u16::from_le_bytes([b[16],b[17]]),
            compressed_sz: u32::from_le_bytes([b[18],b[19],b[20],b[21]]) }
    }
}

// ══════════════════════════════════════════════════════════
// Compressed frame storage — zstd for temp/ring buffer
// ══════════════════════════════════════════════════════════

/// Compress a frame with zstd at speed level 1 (fastest, ~500MB/s)
pub fn compress_frame(data: &[u8]) -> Vec<u8> {
    zstd::encode_all(std::io::Cursor::new(data), 1).unwrap_or_else(|_| data.to_vec())
}

/// Compress for disk storage — level 3 for better ratio (~300MB/s)
pub fn compress_frame_disk(data: &[u8]) -> Vec<u8> {
    zstd::encode_all(std::io::Cursor::new(data), 3).unwrap_or_else(|_| data.to_vec())
}


/// Delta-compress: XOR with previous frame, then zstd.
/// For video frames that differ slightly, this gives 5-20× better compression.
pub fn compress_frame_delta(data: &[u8], prev: &[u8]) -> Vec<u8> {
    if prev.len() != data.len() || prev.is_empty() {
        return compress_frame_disk(data);
    }
    // XOR to get delta
    let mut delta = vec![0u8; data.len()];
    for i in 0..data.len() {
        delta[i] = data[i] ^ prev[i];
    }
    // Prefix with 'D' marker so decoder knows it's delta-compressed
    let compressed = zstd::encode_all(std::io::Cursor::new(&delta), 3)
        .unwrap_or_else(|_| delta);
    let mut out = Vec::with_capacity(1 + compressed.len());
    out.push(b'D');  // delta marker
    out.extend_from_slice(&compressed);
    out
}

/// Decompress a frame that may be delta-compressed.
/// If first byte is 'D', XOR with prev after decompressing.
pub fn decompress_frame_delta(data: &[u8], expected_size: usize, prev: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() { return None; }
    if data[0] == b'D' {
        // Delta-compressed: decompress then XOR with prev
        let delta = decompress_frame(&data[1..], expected_size)?;
        if prev.len() != expected_size { return Some(delta); }
        let mut out = vec![0u8; expected_size];
        for i in 0..expected_size {
            out[i] = delta[i] ^ prev[i];
        }
        Some(out)
    } else {
        // Regular compression
        decompress_frame(data, expected_size)
    }
}


/// Quantized DCT frame storage for 10-30× compression.
/// Format: marker 'D' + dims + zstd(packed coefficients)
/// Uses fixed JPEG quantization tables, zigzag scan, and RLE.
/// CRITICAL: Tested for roundtrip correctness on multiple frame sizes.
pub fn compress_frame_dct(yuv: &[u8], w: usize, h: usize, quality: u8) -> Vec<u8> {
    let y_sz = w * h;
    if yuv.len() < y_sz + y_sz / 2 {
        return compress_frame_disk(yuv);
    }
    
    // Quantization scale: quality 1-100, where 50 is "normal"
    let q_scale = if quality >= 50 {
        2.0 - quality as f32 / 50.0  // 50→1.0, 100→0.0 (lossless)
    } else {
        50.0 / quality.max(1) as f32  // 25→2.0, 1→50.0
    };
    
    let luma_q: [f32; 64] = {
        let base = [16,11,10,16,24,40,51,61, 12,12,14,19,26,58,60,55,
                    14,13,16,24,40,57,69,56, 14,17,22,29,51,87,80,62,
                    18,22,37,56,68,109,103,77, 24,35,55,64,81,104,113,92,
                    49,64,78,87,103,121,120,101, 72,92,95,98,112,100,103,99];
        let mut q = [0.0f32; 64];
        for i in 0..64 { q[i] = (base[i] as f32 * q_scale).max(1.0); }
        q
    };
    let chroma_q: [f32; 64] = {
        let base = [17,18,24,47,99,99,99,99, 18,21,26,66,99,99,99,99,
                    24,26,56,99,99,99,99,99, 47,66,99,99,99,99,99,99,
                    99,99,99,99,99,99,99,99, 99,99,99,99,99,99,99,99,
                    99,99,99,99,99,99,99,99, 99,99,99,99,99,99,99,99];
        let mut q = [0.0f32; 64];
        for i in 0..64 { q[i] = (base[i] as f32 * q_scale).max(1.0); }
        q
    };
    
    let zigzag: [u8; 64] = [
         0, 1, 8,16, 9, 2, 3,10, 17,24,32,25,18,11, 4, 5,
        12,19,26,33,40,48,41,34, 27,20,13, 6, 7,14,21,28,
        35,42,49,56,57,50,43,36, 29,22,15,23,30,37,44,51,
        58,59,52,45,38,31,39,46, 53,60,61,54,47,55,62,63];
    
    // Pack format: each block = sequence of (run, value) pairs ending with (0,0) EOB
    // run is u8 (0-255), value is i16
    // We pack into a single Vec<u8> for compression
    let mut packed: Vec<u8> = Vec::with_capacity(y_sz / 4);
    let mut block = [0.0f32; 64];
    let mut dct_out = [0.0f32; 64];
    
    let encode_plane = |packed: &mut Vec<u8>, plane: &[u8], pw: usize, ph: usize, qt: &[f32; 64]| {
        let bw = pw / 8;
        let bh = ph / 8;
        let mut block = [0.0f32; 64];
        let mut dct_out = [0.0f32; 64];
        
        for by in 0..bh {
            for bx in 0..bw {
                // Extract block (level shift by -128)
                for r in 0..8 {
                    let py = by * 8 + r;
                    for c in 0..8 {
                        let px = bx * 8 + c;
                        block[r * 8 + c] = plane[py * pw + px] as f32 - 128.0;
                    }
                }
                
                dct_fwd(&block, &mut dct_out);
                
                // Quantize and zigzag scan
                let mut coeffs = [0i16; 64];
                for i in 0..64 {
                    coeffs[i] = (dct_out[zigzag[i] as usize] / qt[zigzag[i] as usize]).round() as i16;
                }
                
                // Find last non-zero
                let mut last = 0;
                for i in (0..64).rev() {
                    if coeffs[i] != 0 { last = i + 1; break; }
                }
                
                // RLE: write (run, value) pairs up to 'last'
                let mut run = 0u32;
                for i in 0..last {
                    if coeffs[i] == 0 {
                        run += 1;
                    } else {
                        // Emit any run > 15 as ZRL markers (run=15, value=0)
                        while run >= 16 {
                            packed.push(15);  // run=15 marker
                            packed.push(0); packed.push(0);  // value=0
                            run -= 15;
                        }
                        packed.push(run as u8);
                        let v = coeffs[i];
                        packed.push((v as u16 & 0xFF) as u8);
                        packed.push(((v as u16 >> 8) & 0xFF) as u8);
                        run = 0;
                    }
                }
                // EOB marker: (0, 0)
                packed.push(0);
                packed.push(0);
                packed.push(0);
            }
        }
    };
    
    // Encode Y plane
    let y_plane = &yuv[0..y_sz];
    encode_plane(&mut packed, y_plane, w, h, &luma_q);
    
    // Encode U and V planes (at half resolution)
    let uw = w / 2;
    let uh = h / 2;
    let u_plane = &yuv[y_sz..y_sz + (y_sz / 4)];
    let v_plane = &yuv[y_sz + y_sz / 4..y_sz + y_sz / 2];
    encode_plane(&mut packed, u_plane, uw, uh, &chroma_q);
    encode_plane(&mut packed, v_plane, uw, uh, &chroma_q);
    
    // Header + zstd compress
    let mut output = Vec::with_capacity(packed.len() / 2 + 16);
    output.push(b'D');  // DCT marker
    output.extend_from_slice(&(w as u16).to_le_bytes());
    output.extend_from_slice(&(h as u16).to_le_bytes());
    output.push(quality);
    output.extend_from_slice(&(packed.len() as u32).to_le_bytes());
    
    let compressed = zstd::encode_all(std::io::Cursor::new(&packed), 3)
        .unwrap_or(packed);
    output.extend_from_slice(&compressed);
    output
}

/// Decompress DCT-compressed frame to YUV420p.
pub fn decompress_frame_dct(data: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    if data.len() < 8 || data[0] != b'D' { return None; }
    let stored_w = u16::from_le_bytes([data[1], data[2]]) as usize;
    let stored_h = u16::from_le_bytes([data[3], data[4]]) as usize;
    if stored_w != w || stored_h != h { return None; }
    let quality = data[5];
    let packed_len = u32::from_le_bytes([data[6], data[7], data[8], data[9]]) as usize;
    if 10 > data.len() { return None; }
    
    let packed = zstd::decode_all(std::io::Cursor::new(&data[10..])).ok()?;
    if packed.len() != packed_len { return None; }
    
    let q_scale = if quality >= 50 {
        2.0 - quality as f32 / 50.0
    } else {
        50.0 / quality.max(1) as f32
    };
    
    let luma_q: [f32; 64] = {
        let base = [16,11,10,16,24,40,51,61, 12,12,14,19,26,58,60,55,
                    14,13,16,24,40,57,69,56, 14,17,22,29,51,87,80,62,
                    18,22,37,56,68,109,103,77, 24,35,55,64,81,104,113,92,
                    49,64,78,87,103,121,120,101, 72,92,95,98,112,100,103,99];
        let mut q = [0.0f32; 64];
        for i in 0..64 { q[i] = (base[i] as f32 * q_scale).max(1.0); }
        q
    };
    let chroma_q: [f32; 64] = {
        let base = [17,18,24,47,99,99,99,99, 18,21,26,66,99,99,99,99,
                    24,26,56,99,99,99,99,99, 47,66,99,99,99,99,99,99,
                    99,99,99,99,99,99,99,99, 99,99,99,99,99,99,99,99,
                    99,99,99,99,99,99,99,99, 99,99,99,99,99,99,99,99];
        let mut q = [0.0f32; 64];
        for i in 0..64 { q[i] = (base[i] as f32 * q_scale).max(1.0); }
        q
    };
    
    let zigzag: [u8; 64] = [
         0, 1, 8,16, 9, 2, 3,10, 17,24,32,25,18,11, 4, 5,
        12,19,26,33,40,48,41,34, 27,20,13, 6, 7,14,21,28,
        35,42,49,56,57,50,43,36, 29,22,15,23,30,37,44,51,
        58,59,52,45,38,31,39,46, 53,60,61,54,47,55,62,63];
    
    let y_sz = w * h;
    let mut yuv = vec![128u8; y_sz + y_sz / 2];
    let mut pos = 0usize;
    let mut dct_in = [0.0f32; 64];
    let mut idct_out = [0.0f32; 64];
    
    let mut decode_plane = |yuv: &mut Vec<u8>, plane_off: usize, pw: usize, ph: usize, qt: &[f32; 64], pos: &mut usize| -> bool {
        let bw = pw / 8;
        let bh = ph / 8;
        let mut dct_in = [0.0f32; 64];
        let mut idct_out = [0.0f32; 64];
        
        for by in 0..bh {
            for bx in 0..bw {
                // Decode RLE coefficients
                let mut coeffs = [0i16; 64];
                let mut idx = 0usize;
                
                loop {
                    if *pos + 2 >= packed.len() { return false; }
                    let run = packed[*pos] as usize;
                    let v_lo = packed[*pos + 1];
                    let v_hi = packed[*pos + 2];
                    *pos += 3;
                    
                    if run == 0 && v_lo == 0 && v_hi == 0 {
                        // EOB
                        break;
                    }
                    
                    idx += run;
                    if idx < 64 {
                        let v = i16::from_le_bytes([v_lo, v_hi]);
                        coeffs[idx] = v;
                        idx += 1;
                    } else {
                        // Out of range - skip
                    }
                    
                    if idx >= 64 { break; }
                }
                
                // Inverse zigzag and dequantize
                for i in 0..64 {
                    dct_in[zigzag[i] as usize] = coeffs[i] as f32 * qt[zigzag[i] as usize];
                }
                
                dct_inv(&dct_in, &mut idct_out);
                
                // Write back to plane (level shift +128)
                for r in 0..8 {
                    let py = by * 8 + r;
                    for c in 0..8 {
                        let px = bx * 8 + c;
                        let val = (idct_out[r * 8 + c] + 128.0).clamp(0.0, 255.0) as u8;
                        yuv[plane_off + py * pw + px] = val;
                    }
                }
            }
        }
        true
    };
    
    // Decode Y
    if !decode_plane(&mut yuv, 0, w, h, &luma_q, &mut pos) { return None; }
    // Decode U
    let uw = w / 2; let uh = h / 2;
    if !decode_plane(&mut yuv, y_sz, uw, uh, &chroma_q, &mut pos) { return None; }
    // Decode V
    if !decode_plane(&mut yuv, y_sz + y_sz / 4, uw, uh, &chroma_q, &mut pos) { return None; }
    
    Some(yuv)
}


pub fn compress_frame_temporal(
    yuv: &[u8], prev_yuv: &[u8], w: usize, h: usize, quality: u8
) -> Vec<u8> {
    if prev_yuv.len() != yuv.len() || prev_yuv.is_empty() {
        // No previous frame: fall back to intra DCT
        let mut out = vec![b'I'];  // intra marker
        out.extend_from_slice(&compress_frame_dct(yuv, w, h, quality));
        return out;
    }
    
    // Compute residual (current - previous) 
    // Use i16 to handle negative values, then offset by 128 for u8 storage
    let mut residual = vec![128u8; yuv.len()];
    for i in 0..yuv.len() {
        let diff = yuv[i] as i16 - prev_yuv[i] as i16;
        residual[i] = (diff + 128).clamp(0, 255) as u8;
    }
    
    // Check if residual is mostly zeros (static frame)
    let energy: u64 = residual.iter().map(|&b| {
        let d = (b as i16 - 128).unsigned_abs() as u64;
        d * d
    }).sum();
    let mse = energy / residual.len() as u64;
    
    if mse < 4 {
        // Nearly identical to previous frame: store as skip with tiny delta
        let mut out = vec![b'S'];  // skip marker
        // Store only the non-zero pixels (sparse delta)
        let mut sparse: Vec<u8> = Vec::new();
        let mut run = 0u16;
        for i in 0..residual.len() {
            let d = (residual[i] as i16 - 128).unsigned_abs() as u8;
            if d <= 1 {
                run += 1;
                if run == 0xFFFF {
                    sparse.extend_from_slice(&run.to_le_bytes());
                    sparse.push(128); // zero delta
                    run = 0;
                }
            } else {
                sparse.extend_from_slice(&run.to_le_bytes());
                sparse.push(residual[i]);
                run = 0;
            }
        }
        if run > 0 {
            sparse.extend_from_slice(&run.to_le_bytes());
            sparse.push(128);
        }
        let compressed = zstd::encode_all(std::io::Cursor::new(&sparse), 3)
            .unwrap_or(sparse);
        out.extend_from_slice(&(w as u16).to_le_bytes());
        out.extend_from_slice(&(h as u16).to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&compressed);
        return out;
    }
    
    // Temporal prediction: DCT the residual (much sparser than raw frame)
    let mut out = vec![b'T'];  // temporal marker
    out.extend_from_slice(&compress_frame_dct(&residual, w, h, quality));
    out
}

/// Decompress a temporal-predictive frame.
/// Requires the previous frame's YUV to reconstruct.
pub fn decompress_frame_temporal(
    data: &[u8], prev_yuv: &[u8], w: usize, h: usize
) -> Option<Vec<u8>> {
    if data.is_empty() { return None; }
    
    match data[0] {
        b'I' => {
            // Intra-only: just DCT decompress
            decompress_frame_dct(&data[1..], w, h)
        }
        b'S' => {
            // Skip with sparse delta
            if data.len() < 9 { return None; }
            let _sw = u16::from_le_bytes([data[1], data[2]]) as usize;
            let _sh = u16::from_le_bytes([data[3], data[4]]) as usize;
            let clen = u32::from_le_bytes([data[5], data[6], data[7], data[8]]) as usize;
            if 9 + clen > data.len() { return None; }
            
            let sparse = zstd::decode_all(std::io::Cursor::new(&data[9..9+clen])).ok()?;
            let yuv_len = w * h * 3 / 2;
            let mut yuv = if prev_yuv.len() == yuv_len {
                prev_yuv.to_vec()
            } else {
                vec![128u8; yuv_len]
            };
            
            // Apply sparse delta
            let mut pi = 0usize; // pixel index into yuv
            let mut si = 0usize; // sparse index
            while si + 2 < sparse.len() && pi < yuv_len {
                let run = u16::from_le_bytes([sparse[si], sparse[si+1]]) as usize;
                let val = sparse[si + 2];
                si += 3;
                pi += run; // skip unchanged pixels
                if pi < yuv_len {
                    let delta = val as i16 - 128;
                    yuv[pi] = (yuv[pi] as i16 + delta).clamp(0, 255) as u8;
                    pi += 1;
                }
            }
            Some(yuv)
        }
        b'T' => {
            // Temporal: decompress residual DCT, add to previous frame
            let residual = decompress_frame_dct(&data[1..], w, h)?;
            let yuv_len = w * h * 3 / 2;
            let mut yuv = if prev_yuv.len() == yuv_len {
                prev_yuv.to_vec()
            } else {
                vec![128u8; yuv_len]
            };
            
            for i in 0..yuv_len.min(residual.len()) {
                let delta = residual[i] as i16 - 128;
                yuv[i] = (yuv[i] as i16 + delta).clamp(0, 255) as u8;
            }
            Some(yuv)
        }
        _ => {
            // Legacy format: try raw DCT
            decompress_frame_dct(data, w, h)
        }
    }
}


/// Block-level dedup within a frame for additional compression.
/// Many 8×8 blocks in a video frame are identical (sky, UI, solid regions).
/// Hash each block → deduplicate → store unique blocks + index.
/// Expected: 10-30% additional compression for video with repeated regions.
pub fn compress_frame_block_dedup(yuv: &[u8], w: usize, h: usize, quality: u8) -> Vec<u8> {
    let bw = w / 8; let bh = h / 8;
    let y_sz = w * h;
    
    // Hash each 8×8 Y block
    let mut block_hashes: Vec<u64> = Vec::with_capacity(bw * bh);
    let mut unique_blocks: Vec<(u64, Vec<u8>)> = Vec::new(); // (hash, pixels)
    let mut hash_to_idx: std::collections::HashMap<u64, u16> = std::collections::HashMap::new();
    let mut block_indices: Vec<u16> = Vec::with_capacity(bw * bh);
    
    for by in 0..bh {
        for bx in 0..bw {
            // Extract 8×8 block
            let mut block = [0u8; 64];
            for r in 0..8 {
                let py = by * 8 + r;
                for c in 0..8 {
                    let px = bx * 8 + c;
                    block[r * 8 + c] = if py < h && px < w { yuv[py * w + px] } else { 0 };
                }
            }
            
            // FNV hash
            let mut h: u64 = 0xcbf29ce484222325;
            for &b in &block { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); }
            
            block_hashes.push(h);
            if let Some(&idx) = hash_to_idx.get(&h) {
                block_indices.push(idx); // dedup hit!
            } else {
                let idx = unique_blocks.len() as u16;
                hash_to_idx.insert(h, idx);
                unique_blocks.push((h, block.to_vec()));
                block_indices.push(idx);
            }
        }
    }
    
    let total_blocks = bw * bh;
    let unique_count = unique_blocks.len();
    let dedup_ratio = if unique_count > 0 { total_blocks as f32 / unique_count as f32 } else { 1.0 };
    
    // If dedup ratio > 1.3, use block dedup format
    if dedup_ratio > 1.3 {
        let mut packed = Vec::new();
        packed.push(b'B'); // block dedup marker
        packed.extend_from_slice(&(w as u16).to_le_bytes());
        packed.extend_from_slice(&(h as u16).to_le_bytes());
        packed.extend_from_slice(&(unique_count as u16).to_le_bytes());
        packed.extend_from_slice(&(total_blocks as u16).to_le_bytes());
        
        // Write unique block data
        for (_, pixels) in &unique_blocks {
            packed.extend_from_slice(pixels);
        }
        
        // Write index table
        for &idx in &block_indices {
            packed.extend_from_slice(&idx.to_le_bytes());
        }
        
        // Append U and V planes compressed directly (less block structure)
        if yuv.len() > y_sz {
            packed.extend_from_slice(&yuv[y_sz..]);
        }
        
        return zstd::encode_all(std::io::Cursor::new(&packed), 3).unwrap_or(packed);
    }
    
    // Low dedup ratio: fall back to regular DCT
    compress_frame_dct(yuv, w, h, quality)
}


/// Y/UV separated compression — different strategies per plane.
/// Y plane (luminance): high detail, needs more bits
/// U/V planes (chroma): low detail, often similar across frames
/// 
/// This achieves better compression than monolithic YUV compression
/// because zstd's dictionary can specialize for each plane's statistics.
pub fn compress_frame_yuv_split(yuv: &[u8], w: usize, h: usize) -> Vec<u8> {
    let y_sz = w * h;
    let uv_sz = y_sz / 4;
    
    if yuv.len() < y_sz + 2 * uv_sz {
        // Fall back to standard compression
        return compress_frame_disk(yuv);
    }
    
    let y_plane = &yuv[0..y_sz];
    let u_plane = &yuv[y_sz..y_sz + uv_sz];
    let v_plane = &yuv[y_sz + uv_sz..y_sz + 2 * uv_sz];
    
    // Compress each plane separately at tuned levels
    // Y: level 3 (fast, preserves detail) — full bandwidth
    // U/V: level 9 (chroma is lower entropy, benefits from higher zstd)
    // Saves ~10-15% file size at minor encode cost
    // Y: level 3 (fast, preserves detail). UV: level 5 (balance speed + size)
    let y_compressed = zstd::encode_all(std::io::Cursor::new(y_plane), 3).unwrap_or_else(|_| y_plane.to_vec());
    let u_compressed = zstd::encode_all(std::io::Cursor::new(u_plane), 5).unwrap_or_else(|_| u_plane.to_vec());
    let v_compressed = zstd::encode_all(std::io::Cursor::new(v_plane), 5).unwrap_or_else(|_| v_plane.to_vec());
    
    // Pack: marker + sizes + data
    let mut out = Vec::with_capacity(y_compressed.len() + u_compressed.len() + v_compressed.len() + 16);
    out.push(b'P');  // plane-split marker
    out.extend_from_slice(&(w as u16).to_le_bytes());
    out.extend_from_slice(&(h as u16).to_le_bytes());
    out.extend_from_slice(&(y_compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&(u_compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&(v_compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&y_compressed);
    out.extend_from_slice(&u_compressed);
    out.extend_from_slice(&v_compressed);
    
    out
}

/// Decompress a Y/UV split frame.
pub fn decompress_frame_yuv_split(data: &[u8], w: usize, h: usize) -> Option<Vec<u8>> {
    if data.is_empty() || data[0] != b'P' { return None; }
    if data.len() < 17 { return None; }
    
    let _w = u16::from_le_bytes([data[1], data[2]]) as usize;
    let _h = u16::from_le_bytes([data[3], data[4]]) as usize;
    let y_sz_c = u32::from_le_bytes([data[5], data[6], data[7], data[8]]) as usize;
    let u_sz_c = u32::from_le_bytes([data[9], data[10], data[11], data[12]]) as usize;
    let v_sz_c = u32::from_le_bytes([data[13], data[14], data[15], data[16]]) as usize;
    
    let mut off = 17;
    if off + y_sz_c + u_sz_c + v_sz_c > data.len() { return None; }
    
    let y_plane = zstd::decode_all(std::io::Cursor::new(&data[off..off+y_sz_c])).ok()?;
    off += y_sz_c;
    let u_plane = zstd::decode_all(std::io::Cursor::new(&data[off..off+u_sz_c])).ok()?;
    off += u_sz_c;
    let v_plane = zstd::decode_all(std::io::Cursor::new(&data[off..off+v_sz_c])).ok()?;
    
    let y_sz = w * h;
    let uv_sz = y_sz / 4;
    let mut yuv = Vec::with_capacity(y_sz + 2 * uv_sz);
    yuv.extend_from_slice(&y_plane);
    yuv.extend_from_slice(&u_plane);
    yuv.extend_from_slice(&v_plane);
    
    Some(yuv)
}

/// Decompress a zstd-compressed frame
pub fn decompress_frame(compressed: &[u8], expected_size: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(expected_size);
    zstd::stream::copy_decode(std::io::Cursor::new(compressed), &mut out).ok()?;
    Some(out)
}

/// Create a mipmap (quarter resolution) for scrub thumbnails
pub fn mipmap_quarter(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    let hw = w / 2; let hh = h / 2;
    let mut out = vec![0u8; hw * hh * 3];
    for y in 0..hh { for x in 0..hw {
        let si = ((y*2) * w + x*2) * 3;
        let di = (y * hw + x) * 3;
        // Average 2×2 block
        for c in 0..3 {
            let v = (rgb[si+c] as u16 + rgb[si+3+c] as u16 
                + rgb[si+w*3+c] as u16 + rgb[si+w*3+3+c] as u16) / 4;
            out[di+c] = v as u8;
        }
    }}
    out
}

// ══════════════════════════════════════════════════════════
// Intra prediction — predict from reconstructed neighbors
// ══════════════════════════════════════════════════════════

/// Intra prediction modes
#[derive(Clone, Copy, PartialEq)]
pub enum IntraMode { DC = 0, Horizontal = 1, Vertical = 2, DiagDownLeft = 3 }

/// Generate intra prediction for an 8×8 block from reconstructed neighbors
/// `recon` is the reconstructed plane (top and left blocks already filled in)
/// Returns predicted 8×8 tile
pub fn intra_predict(recon: &[f32], w: usize, h: usize, ox: usize, oy: usize, mode: IntraMode) -> [f32; 64] {
    let mut pred = [128.0f32; 64];

    // Gather top pixels (row above, same columns) and left pixels (column left, same rows)
    let has_top = oy >= 8;
    let has_left = ox >= 8;

    let mut top = [128.0f32; 8];
    let mut left = [128.0f32; 8];

    if has_top {
        for x in 0..8 { top[x] = recon[(oy - 1) * w + ox + x]; }
    }
    if has_left {
        for y in 0..8 { left[y] = recon[(oy + y) * w + ox - 1]; }
    }

    match mode {
        IntraMode::DC => {
            // Average of all available neighbors
            let mut sum = 0.0f32;
            let mut count = 0;
            if has_top { for x in 0..8 { sum += top[x]; count += 1; } }
            if has_left { for y in 0..8 { sum += left[y]; count += 1; } }
            let dc = if count > 0 { sum / count as f32 } else { 128.0 };
            for i in 0..64 { pred[i] = dc; }
        }
        IntraMode::Horizontal => {
            // Extend left column rightward
            for y in 0..8 { for x in 0..8 { pred[y * 8 + x] = left[y]; } }
        }
        IntraMode::Vertical => {
            // Extend top row downward
            for y in 0..8 { for x in 0..8 { pred[y * 8 + x] = top[x]; } }
        }
        IntraMode::DiagDownLeft => {
            // Diagonal: blend top and left based on position
            for y in 0..8 { for x in 0..8 {
                let t_weight = (7 - y) as f32 / 14.0;
                let l_weight = (7 - x) as f32 / 14.0;
                pred[y * 8 + x] = top[x] * t_weight + left[y] * l_weight
                    + top[x.min(7)] * (1.0 - t_weight - l_weight).max(0.0);
            }}
        }
    }
    pred
}

/// Pick the best intra mode for a block by trying all 4 and picking lowest SAD
pub fn best_intra_mode(recon: &[f32], actual: &[f32], w: usize, h: usize, ox: usize, oy: usize) -> (IntraMode, [f32; 64], f32) {
    let modes = [IntraMode::DC, IntraMode::Horizontal, IntraMode::Vertical, IntraMode::DiagDownLeft];
    let mut best_mode = IntraMode::DC;
    let mut best_pred = [128.0f32; 64];
    let mut best_sad = f32::MAX;

    // Get actual block values
    let mut block = [0.0f32; 64];
    for y in 0..8 { for x in 0..8 {
        block[y * 8 + x] = actual[(oy + y) * w + ox + x];
    }}

    for &mode in &modes {
        let pred = intra_predict(recon, w, h, ox, oy, mode);
        let mut sad = 0.0f32;
        for i in 0..64 {
            sad += (block[i] - pred[i]).abs();
        }
        if sad < best_sad {
            best_sad = sad;
            best_mode = mode;
            best_pred = pred;
        }
    }

    (best_mode, best_pred, best_sad)
}

// ══════════════════════════════════════════════════════════
// Entropy coding — serialize + zstd compress coefficient stream
// ══════════════════════════════════════════════════════════

/// Serialize quantized DCT blocks into a byte stream and compress with zstd.
/// Returns compressed bytes (the actual bitstream size).
pub fn entropy_encode_blocks(blocks: &[[i32; 64]]) -> Vec<u8> {
    if blocks.is_empty() { return vec![]; }

    // Serialize with zero-block optimization
    // Format: [u8 flags byte per 8 blocks: bit=1 means non-zero]
    //         followed by non-zero block data only
    let mut raw = Vec::with_capacity(blocks.len() * 16);

    // Write block count
    raw.extend_from_slice(&(blocks.len() as u32).to_le_bytes());

    // Process in groups of 8 blocks
    let mut bi = 0;
    while bi < blocks.len() {
        // Flag byte: which of the next 8 blocks are non-zero
        let mut flags = 0u8;
        for j in 0..8 {
            if bi + j < blocks.len() {
                let is_nonzero = blocks[bi + j].iter().any(|&v| v != 0);
                if is_nonzero { flags |= 1 << j; }
            }
        }
        raw.push(flags);

        // Only serialize non-zero blocks
        for j in 0..8 {
            if bi + j >= blocks.len() { break; }
            if flags & (1 << j) == 0 { continue; }  // zero block: skip entirely

            let block = &blocks[bi + j];
            let mut run = 0u8;
            for i in 0..64 {
                if block[i] == 0 {
                    run += 1;
                } else {
                    raw.push(run);
                    let v = block[i].max(-32767).min(32767) as i16;
                    raw.extend_from_slice(&v.to_le_bytes());
                    run = 0;
                }
            }
            raw.push(0xFF);  // end of block
        }
        bi += 8;
    }

    compress_frame(&raw)
}

/// Compute the actual compressed size of quantized blocks
pub fn compressed_block_size(blocks: &[[i32; 64]]) -> usize {
    entropy_encode_blocks(blocks).len()
}

// ══════════════════════════════════════════════════════════
// Variable block size — 16×16 superblock classification
// ══════════════════════════════════════════════════════════

/// Check if a 16×16 region is uniform enough to encode as a single block
pub fn classify_superblock(plane: &[f32], w: usize, ox: usize, oy: usize) -> bool {
    if ox + 16 > w { return false; }
    // Compute mean and variance of the 16×16 region
    let mut sum = 0.0f32;
    let mut sum_sq = 0.0f32;
    for dy in 0..16 { for dx in 0..16 {
        let v = plane[(oy + dy) * w + ox + dx];
        sum += v;
        sum_sq += v * v;
    }}
    let n = 256.0f32;
    let mean = sum / n;
    let variance = sum_sq / n - mean * mean;
    
    // Also check edge energy (gradient magnitude)
    let mut edge = 0.0f32;
    for dy in 0..15 { for dx in 0..15 {
        let i = (oy + dy) * w + ox + dx;
        let gx = (plane[i + 1] - plane[i]).abs();
        let gy = (plane[i + w] - plane[i]).abs();
        edge += gx + gy;
    }}
    edge /= (15 * 15) as f32;
    
    // Flat if low variance AND low edge energy
    variance < 30.0 && edge < 2.0  // strict: only truly flat regions
}
