//! flash_qblock — query-block flash attention: the perf lever for the quadratic.
//!
//! ## Why this and not the routing
//!
//! `attention_mh_full_t` is inaccurate above ~64 rows on Honeykrisp and its call
//! sites should move to the tiled kernel — but **that is an accuracy fix and
//! carries no speed expectation**. The c689 A/B tied chunked against naive,
//! **2698.9 vs 2630.7**, because `EXT_PAR` moves the same K/V volume; it just
//! moves it coalesced.
//!
//! The quadratic's actual lever is here. `ATTENTION_MH_EXT_PAR` dispatches **one
//! workgroup per (query row, head)**, so every row re-reads the whole K/V span:
//! bandwidth scales with `rows × span`. Staging each K/V tile once per *block*
//! of `R` rows divides the K/V traffic by `R` while the arithmetic stays the
//! same — the classic flash trade, applied on the query axis rather than the key
//! axis the decode path already uses.
//!
//! ## What is testable without a GPU, and what is not
//!
//! The kernel cannot run here. What *can* be checked is the part that is easy to
//! get wrong and expensive to debug on-device: **the online-softmax
//! recurrence**. Blocking the query axis does not change it, but the rescale
//! must still be exact when a later tile raises the running maximum — and a
//! reference that reproduces the same numbers as a single-pass softmax is what
//! makes an on-device mismatch mean "the WGSL is wrong" rather than "one of
//! these two is wrong".
//!
//! So this module carries the reference and its tests. The WGSL below is
//! **unverified against hardware** and says so.

/// Query rows per workgroup.
///
/// Eight: the K/V tile is staged once per block, so traffic falls by `R`, while
/// shared memory holds `R` running accumulators. Beyond about eight the
/// accumulators start competing with the K/V tile for the same 8 KB that keeps
/// occupancy high — the point of tiling in the first place.
pub const ROWS_PER_BLOCK: usize = 8;

/// One row's running softmax state.
#[derive(Clone, Debug)]
pub struct RowAcc {
    /// Running maximum.
    pub m: f32,
    /// Running denominator, in the current maximum's scale.
    pub den: f32,
    pub acc: Vec<f32>,
}

impl RowAcc {
    pub fn new(hd: usize) -> RowAcc {
        // -inf rather than 0: a first tile of all-negative scores would
        // otherwise never raise the maximum, and every weight would underflow.
        RowAcc { m: f32::NEG_INFINITY, den: 0.0, acc: vec![0.0; hd] }
    }

    /// Fold one tile of scores and values into this row.
    ///
    /// `scores` are pre-softmax logits for the tile; `values` is `tile × hd`
    /// row-major. This is the recurrence the WGSL must match exactly.
    pub fn fold(&mut self, scores: &[f32], values: &[f32], hd: usize) {
        if scores.is_empty() { return; }
        let tile_max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        if !tile_max.is_finite() && tile_max < 0.0 {
            // An entirely masked tile contributes nothing. Folding it anyway
            // would multiply the accumulator by exp(-inf - m) = 0 and erase
            // every earlier tile.
            return;
        }
        let new_m = if self.m > tile_max { self.m } else { tile_max };
        // Rescale what is already accumulated into the new maximum's scale.
        // When the maximum did not move this is exp(0) = 1 and costs nothing;
        // when it did, skipping it silently over-weights the earlier tiles.
        let rescale = if self.m.is_finite() { (self.m - new_m).exp() } else { 0.0 };
        self.den *= rescale;
        for a in self.acc.iter_mut() { *a *= rescale; }

        for (j, s) in scores.iter().enumerate() {
            let w = (s - new_m).exp();
            self.den += w;
            let base = j * hd;
            for c in 0..hd {
                self.acc[c] += w * values[base + c];
            }
        }
        self.m = new_m;
    }

    /// Normalise into the output row.
    ///
    /// A zero denominator yields zeros rather than NaN: a fully masked row is
    /// legitimate at the start of a windowed sequence, and one NaN propagates
    /// through every later layer until the whole output is NaN.
    pub fn finish(&self) -> Vec<f32> {
        if self.den <= 0.0 || !self.den.is_finite() { return vec![0.0; self.acc.len()]; }
        self.acc.iter().map(|a| a / self.den).collect()
    }
}

/// Reference attention over a block of query rows, tiled on the key axis.
///
/// `scores[r][t]` are the logits for row `r` at key `t`; `values` is
/// `span × hd`. Tiles the key axis exactly as the kernel does, so the two share
/// a numerical path.
pub fn block_attention(scores: &[Vec<f32>], values: &[f32], hd: usize, tile: usize)
    -> Vec<Vec<f32>> {
    let tile = tile.max(1);
    scores.iter().map(|row| {
        let mut acc = RowAcc::new(hd);
        let mut t0 = 0;
        while t0 < row.len() {
            let t1 = (t0 + tile).min(row.len());
            acc.fold(&row[t0..t1], &values[t0 * hd..t1 * hd], hd);
            t0 = t1;
        }
        acc.finish()
    }).collect()
}

/// Single-pass softmax, for checking the tiled path against.
pub fn reference_attention(scores: &[Vec<f32>], values: &[f32], hd: usize) -> Vec<Vec<f32>> {
    scores.iter().map(|row| {
        let m = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        if !m.is_finite() { return vec![0.0; hd]; }
        let w: Vec<f32> = row.iter().map(|s| (s - m).exp()).collect();
        let den: f32 = w.iter().sum();
        let mut out = vec![0.0f32; hd];
        for (j, wj) in w.iter().enumerate() {
            for c in 0..hd { out[c] += wj * values[j * hd + c]; }
        }
        if den > 0.0 { for o in out.iter_mut() { *o /= den; } }
        out
    }).collect()
}

/// K/V bytes read, per head, for a span of `rows` queries over `span` keys.
///
/// The whole argument for this kernel in one function: `EXT_PAR` reads the span
/// once per row; the query-block variant reads it once per block.
pub fn kv_bytes(rows: usize, span: usize, hd: usize, rows_per_block: usize) -> (u64, u64) {
    let per_span = 2 * span as u64 * hd as u64 * 4;      // K and V, f32
    let ext_par = rows as u64 * per_span;
    let blocks = rows.div_ceil(rows_per_block.max(1)) as u64;
    (ext_par, blocks * per_span)
}

/// The kernel. **Unverified against hardware** — see the module docs.
///
/// Kept beside the reference deliberately: the numbers it must reproduce are
/// forty lines above it, so a mismatch on-device has one place to look.
pub const ATTENTION_QBLOCK: &str = r#"
// Query-block flash attention. One workgroup per (row block, head): the K/V tile
// is staged into shared memory ONCE and folded into R rows, so K/V traffic falls
// by R relative to the per-row kernel. Online softmax per row, identical
// recurrence to ATTENTION_MH_EXT_PAR — see flash_qblock::RowAcc::fold, which is
// the executable spec for this arithmetic.
struct D { s:u32,hd:u32,nh:u32,nkv:u32,base:u32,nswa:u32,swaty:u32,_c:u32 };
@group(0)@binding(0) var<storage,read> q: array<f32>;
@group(0)@binding(1) var<storage,read> kc: array<f32>;
@group(0)@binding(2) var<storage,read> vc: array<f32>;
@group(0)@binding(3) var<storage,read_write> o: array<f32>;
@group(0)@binding(4) var<uniform> dd: D;
const R: u32 = 8u;          // rows per block; matches ROWS_PER_BLOCK
const KT: u32 = 64u;        // key tile
var<workgroup> ktile: array<f32,64*128>;   // staged K tile (hd<=128)
var<workgroup> vtile: array<f32,64*128>;   // staged V tile
@compute @workgroup_size(64,1,1)
fn main(@builtin(workgroup_id) wg:vec3<u32>, @builtin(local_invocation_id) lid:vec3<u32>){
  let blk=wg.x; let hh=wg.y; let tid=lid.x;
  let dm=dd.nh*dd.hd; let kvdm=dd.nkv*dd.hd;
  let kb=(hh/(dd.nh/dd.nkv))*dd.hd;
  let scale=1.0/sqrt(f32(dd.hd));
  // Per-row running state lives in registers; R is small enough that this fits.
  var m: array<f32,8>; var den: array<f32,8>;
  for(var r=0u;r<R;r++){ m[r]=-3.4e38; den[r]=0.0; }
  // acc is strided across threads: thread tid owns columns tid, tid+64, ...
  var acc: array<f32,16>;   // R * (hd/64), hd<=128 -> 8*2
  for(var i=0u;i<16u;i++){ acc[i]=0.0; }
  var t0=0u;
  loop {
    if(t0>=dd.s){break;}
    let t1=min(dd.s,t0+KT);
    // Stage the tile ONCE for all R rows — the entire point of the kernel.
    for(var j=t0+tid;j<t1;j+=64u){
      for(var c=0u;c<dd.hd;c++){
        ktile[(j-t0)*dd.hd+c]=kc[j*kvdm+kb+c];
        vtile[(j-t0)*dd.hd+c]=vc[j*kvdm+kb+c];
      }
    }
    workgroupBarrier();
    // Fold the staged tile into each row with the online-softmax recurrence.
    for(var r=0u;r<R;r++){
      let row=blk*R+r;
      if(row>=dd.s){continue;}
      let qpos=dd.base+row;
      var lmax=-3.4e38;
      for(var j=t0;j<t1;j++){
        if(j>qpos){break;}                     // causal
        var dot=0.0;
        for(var c=0u;c<dd.hd;c++){ dot+=q[row*dm+hh*dd.hd+c]*ktile[(j-t0)*dd.hd+c]; }
        lmax=max(lmax,dot*scale);
      }
      if(lmax<=-3.4e38){continue;}             // fully masked tile contributes nothing
      let nm=max(m[r],lmax);
      let rs=select(0.0,exp(m[r]-nm),m[r]>-3.4e38);
      den[r]*=rs;
      for(var c=tid;c<dd.hd;c+=64u){ acc[r*2u+c/64u]*=rs; }
      for(var j=t0;j<t1;j++){
        if(j>qpos){break;}
        var dot=0.0;
        for(var c=0u;c<dd.hd;c++){ dot+=q[row*dm+hh*dd.hd+c]*ktile[(j-t0)*dd.hd+c]; }
        let w=exp(dot*scale-nm);
        den[r]+=w;
        for(var c=tid;c<dd.hd;c+=64u){ acc[r*2u+c/64u]+=w*vtile[(j-t0)*dd.hd+c]; }
      }
      m[r]=nm;
    }
    workgroupBarrier();
    t0=t1;
  }
  for(var r=0u;r<R;r++){
    let row=blk*R+r;
    if(row>=dd.s){continue;}
    let d=select(1.0,den[r],den[r]>0.0);
    for(var c=tid;c<dd.hd;c+=64u){
      o[row*dm+hh*dd.hd+c]=select(0.0,acc[r*2u+c/64u]/d,den[r]>0.0);
    }
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn scores(rows: usize, span: usize, f: impl Fn(usize, usize) -> f32) -> Vec<Vec<f32>> {
        (0..rows).map(|r| (0..span).map(|t| f(r, t)).collect()).collect()
    }

    fn values(span: usize, hd: usize) -> Vec<f32> {
        (0..span * hd).map(|i| ((i % 17) as f32 - 8.0) / 4.0).collect()
    }

    fn close(a: &[Vec<f32>], b: &[Vec<f32>], tol: f32) {
        assert_eq!(a.len(), b.len());
        for (i, (ra, rb)) in a.iter().zip(b).enumerate() {
            for (c, (x, y)) in ra.iter().zip(rb).enumerate() {
                assert!((x - y).abs() <= tol, "row {i} col {c}: {x} vs {y}");
            }
        }
    }

    #[test]
    fn tiling_the_key_axis_matches_a_single_pass_softmax() {
        // The property the whole recurrence exists for. If this drifts, an
        // on-device mismatch means "the WGSL is wrong" rather than "one of
        // these two is wrong" — which is the difference between a morning and
        // a week.
        let (rows, span, hd) = (8, 200, 16);
        let s = scores(rows, span, |r, t| ((r * 7 + t * 3) % 23) as f32 / 5.0 - 2.0);
        let v = values(span, hd);
        for tile in [1, 7, 64, 199, 200, 512] {
            close(&block_attention(&s, &v, hd, tile), &reference_attention(&s, &v, hd), 1e-5);
        }
    }

    #[test]
    fn a_late_tile_raising_the_maximum_rescales_the_earlier_ones() {
        // The case the rescale exists for, and the one a naive implementation
        // gets right on random data and wrong on real attention, where the
        // largest logit often arrives late.
        let (span, hd) = (128, 8);
        let s = scores(1, span, |_, t| if t == span - 1 { 40.0 } else { 0.0 });
        let v = values(span, hd);
        close(&block_attention(&s, &v, hd, 16), &reference_attention(&s, &v, hd), 1e-5);
    }

    #[test]
    fn the_maximum_arriving_first_is_handled_too() {
        // The mirror case: rescale must be exp(0) = 1 and cost nothing, rather
        // than being skipped by a branch that assumes the max always moves.
        let (span, hd) = (128, 8);
        let s = scores(1, span, |_, t| if t == 0 { 40.0 } else { 0.0 });
        let v = values(span, hd);
        close(&block_attention(&s, &v, hd, 16), &reference_attention(&s, &v, hd), 1e-5);
    }

    #[test]
    fn a_fully_masked_tile_does_not_erase_the_accumulator() {
        // Folding it would multiply by exp(-inf - m) = 0 and wipe every earlier
        // tile — a silent zeroing that looks like a bad attention head.
        let hd = 4;
        let mut a = RowAcc::new(hd);
        a.fold(&[1.0, 2.0], &values(2, hd), hd);
        let before = a.finish();
        a.fold(&[f32::NEG_INFINITY; 2], &values(2, hd), hd);
        close(&[a.finish()], &[before], 1e-6);
    }

    #[test]
    fn a_fully_masked_row_yields_zeros_rather_than_nan() {
        // Legitimate at the start of a windowed sequence, and one NaN
        // propagates through every later layer until the whole output is NaN.
        let hd = 8;
        let a = RowAcc::new(hd);
        let out = a.finish();
        assert!(out.iter().all(|x| *x == 0.0 && x.is_finite()));
    }

    #[test]
    fn large_logits_do_not_overflow() {
        // Attention logits reach the hundreds before scaling on real models;
        // subtracting the running max is what keeps exp() in range, and a
        // reference that overflowed would validate nothing.
        let (span, hd) = (64, 8);
        let s = scores(2, span, |r, t| (r * 300 + t * 5) as f32);
        let v = values(span, hd);
        let out = block_attention(&s, &v, hd, 16);
        assert!(out.iter().flatten().all(|x| x.is_finite()), "{out:?}");
        close(&out, &reference_attention(&s, &v, hd), 1e-4);
    }

    #[test]
    fn a_tile_larger_than_the_span_is_one_pass() {
        let (span, hd) = (10, 4);
        let s = scores(3, span, |r, t| (r + t) as f32 / 3.0);
        let v = values(span, hd);
        close(&block_attention(&s, &v, hd, 1000), &reference_attention(&s, &v, hd), 1e-6);
    }

    #[test]
    fn a_zero_tile_does_not_loop_forever() {
        // Reachable from a config value; a tile of zero would advance t0 by
        // nothing and hang the reference.
        let s = scores(1, 4, |_, t| t as f32);
        let v = values(4, 2);
        assert_eq!(block_attention(&s, &v, 2, 0).len(), 1);
    }

    #[test]
    fn the_kv_traffic_falls_by_the_block_size() {
        // The entire argument for this kernel, as arithmetic. EXT_PAR reads the
        // span once per ROW; the query-block variant reads it once per BLOCK.
        let (ext, qblk) = kv_bytes(1024, 1024, 128, ROWS_PER_BLOCK);
        assert_eq!(ext / qblk, ROWS_PER_BLOCK as u64);
        assert!(ext > 1_000_000_000, "a 1k×1k head is a gigabyte of K/V re-reads");
    }

    #[test]
    fn a_partial_final_block_still_counts_as_a_block() {
        // 1000 rows at 8 per block is 125; 1001 is 126, not 125.
        let (_, a) = kv_bytes(1000, 512, 64, 8);
        let (_, b) = kv_bytes(1001, 512, 64, 8);
        assert!(b > a, "the leftover rows still read the span once");
    }

    #[test]
    fn the_kernel_and_the_reference_agree_on_the_block_size() {
        // The WGSL hardcodes R = 8u; a reference tuned to a different value
        // would validate arithmetic the kernel never runs.
        assert!(ATTENTION_QBLOCK.contains(&format!("const R: u32 = {ROWS_PER_BLOCK}u;")),
                "ROWS_PER_BLOCK and the kernel's R have drifted apart");
    }
}
