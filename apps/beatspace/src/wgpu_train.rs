//! wgpu_train — the training kernels, and the Asahi rules they are shaped by.
//!
//! `--features gpu` builds a wgpu path for generator INFERENCE. Training runs
//! on candle, whose backends are CUDA and Metal; on Asahi the Apple GPU is
//! reached through Vulkan, so there is no candle backend for it and the whole
//! menu is CPU. This module is the start of the other answer: train on wgpu,
//! the same way `logscope`'s `gpu-gemm` already runs transformer inference on
//! this exact hardware.
//!
//! ## Everything is a GEMM
//!
//! The generator is `Linear → ConvTranspose2d ×4 → tanh`, with BatchNorm and
//! ReLU between. Writing a separate GPU kernel for each, forward and backward,
//! is eight kernels to tune and eight to get numerically right.
//!
//! Instead every convolution is lowered to a matrix multiply — `im2col` for
//! the forward, `col2im` for the gradient that flows back to the input — so
//! ONE well-tuned GEMM carries the Linear layer, both convolutions and all
//! their backward passes. That matters more here than usual: the tuning below
//! is hardware-specific and hard-won, and it is worth doing once.
//!
//! ## What Asahi demands, from gpu-gemm's own findings
//!
//! Taken from `apps/logscope/crates/gpu-gemm/src/lib.rs`, which is running on
//! Honeykrisp today rather than theorising about it:
//!
//! * **The SIMD group is 32 threads.** Think in subgroups, not threads. A
//!   workgroup of 64 is two of them and is the safe default.
//! * **~32 KB of threadgroup memory per core, and asking for it pins occupancy
//!   to one workgroup.** On a latency-bound kernel that is the dominant cost,
//!   so the tiles below are sized to stay far under it: two 16×16 f32 tiles is
//!   2 KB, leaving room for many concurrent workgroups.
//! * **Subgroup reductions beat barrier trees here** even where the barrier
//!   tree won on the same silicon under Metal — but `subgroupAdd` is NOT used
//!   in these kernels, because gpu-gemm also records that Honeykrisp's
//!   subgroup support can hang large matmuls and ships with it disabled
//!   (`LOGSCOPE_MATVEC_SG=1` to re-enable). A tiled GEMM does not need it; it
//!   accumulates in registers.
//! * **Bound the work per dispatch.** gpu-gemm's preflight bounds decode
//!   window submits for watchdog safety on this driver. A GEMM tile loop is
//!   naturally bounded; the caller must still not launch a grid whose
//!   dimension exceeds 65535, which is a limit this project has already hit
//!   once and fixed by spilling into `y`.
//!
//! ## Verified on hardware, 2026-08-19
//!
//! ```text
//! adapter: Apple M2 Ultra (G14D B1) (Vulkan)
//! gemm 37x53x41 on a 3x3 tile grid
//! worst |gpu-cpu| = 1.669e-6 at (7, 24)
//! PASS — all 1517 elements within 1e-4
//! ```
//!
//! 1.669e-6 is the right answer rather than a near miss: f32 carries about
//! seven digits, the kernel accumulates 53 terms in tile order and the
//! reference in index order, and a deviation at that scale IS the reordering.
//! Exact equality would have been the suspicious result.
//!
//! `maintenance/ASAHI_GPU.md` records this and the rest of what is known about
//! this GPU, each item marked measured or assumed.
//!
//! ## What this module is, honestly
//!
//! Index arithmetic and shader source, with the backward math verified against
//! finite differences on the CPU and every kernel proven to compile with naga.
//! It has not been run on a GPU — this container has none — so nothing here
//! claims measured performance. The parts that can be checked without hardware
//! are checked; the rest is marked and left for a machine that has one.

/// A convolution's shape, in the form the lowering needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConvShape {
    pub batch: usize,
    pub in_ch: usize,
    pub out_ch: usize,
    pub in_h: usize,
    pub in_w: usize,
    pub kernel: usize,
    pub stride: usize,
    pub pad: usize,
}

impl ConvShape {
    /// Output height/width of a forward convolution.
    pub fn out_hw(&self) -> (usize, usize) {
        let f = |n: usize| (n + 2 * self.pad).saturating_sub(self.kernel) / self.stride + 1;
        (f(self.in_h), f(self.in_w))
    }

    /// Output height/width of a TRANSPOSED convolution.
    ///
    /// Not the forward formula inverted by hand — that is where sign errors
    /// live. A transposed conv is the gradient of a forward one, so its output
    /// is the forward's input: `(n-1)*stride - 2*pad + kernel`.
    pub fn out_hw_transposed(&self) -> (usize, usize) {
        let f = |n: usize| {
            (n.saturating_sub(1) * self.stride + self.kernel).saturating_sub(2 * self.pad)
        };
        (f(self.in_h), f(self.in_w))
    }

    /// The GEMM this convolution becomes: `(m, k, n)`.
    ///
    /// `m` = output channels, `k` = the patch length, `n` = one column per
    /// output position across the whole batch. Batching into `n` rather than
    /// looping over it is what keeps the GPU busy: 32 small GEMMs are 32
    /// dispatches, and dispatch setup is a measured hotspot on this driver.
    pub fn gemm_dims(&self) -> (usize, usize, usize) {
        let (oh, ow) = self.out_hw();
        (self.out_ch, self.in_ch * self.kernel * self.kernel, self.batch * oh * ow)
    }
}

/// Where a patch element reads from, for `im2col`.
///
/// Returns `None` when the read falls in the padding, which the caller writes
/// as zero. Returning an Option rather than clamping is deliberate: clamping
/// silently trains on duplicated edge pixels, and the difference does not show
/// up until the model has learned a border artefact.
pub fn im2col_src(
    s: &ConvShape,
    col_row: usize, // 0..k
    col_col: usize, // 0..n
) -> Option<(usize, usize, usize, usize)> {
    let (oh, ow) = s.out_hw();
    let per_image = oh * ow;
    let b = col_col / per_image;
    let rem = col_col % per_image;
    let (oy, ox) = (rem / ow, rem % ow);

    let kk = s.kernel * s.kernel;
    let c = col_row / kk;
    let krem = col_row % kk;
    let (ky, kx) = (krem / s.kernel, krem % s.kernel);

    let iy = (oy * s.stride + ky).checked_sub(s.pad)?;
    let ix = (ox * s.stride + kx).checked_sub(s.pad)?;
    if iy >= s.in_h || ix >= s.in_w || b >= s.batch || c >= s.in_ch {
        return None;
    }
    Some((b, c, iy, ix))
}

/// The GEMM tile edge. 16×16 threads = 256 per workgroup.
///
/// Two f32 tiles of this size is 2 KB of threadgroup memory. The AGX core has
/// about 32 KB, and gpu-gemm's finding is that reserving most of it pins
/// occupancy to a single workgroup per core — which on a latency-bound kernel
/// is the dominant cost. 2 KB leaves room for many.
pub const TILE: usize = 16;

/// Threads per workgroup, as a multiple of the 32-wide AGX SIMD group.
pub const WORKGROUP: usize = TILE * TILE;

/// A tiled GEMM: `C = A·B`, with optional transposes on either input.
///
/// One kernel serves the whole training step. The forward pass is `W·X`; the
/// gradient to the input is `Wᵀ·dY` and the gradient to the weights is
/// `dY·Xᵀ`, which are the same multiply with different transposes — so the
/// transposes are runtime flags rather than three separate shaders to keep in
/// agreement.
pub fn gemm_wgsl() -> String {
    format!(
        r#"
// Tiled GEMM for AGX/Honeykrisp. See the module docs for why these numbers.
//
// TILE={tile} => two {tile}x{tile} f32 tiles = {kb} KB of workgroup memory,
// well under the ~32 KB per core, so occupancy is not pinned to one group.
// No subgroup ops: gpu-gemm records that Honeykrisp subgroup support can hang
// large matmuls and ships with it off.

struct Dims {{
  m: u32,
  k: u32,
  n: u32,
  // 1 = read A transposed, i.e. A is stored k-major.
  a_t: u32,
  b_t: u32,
  // 0 = overwrite C, 1 = accumulate into it. Accumulation is what lets a
  // gradient sum over a batch without a second pass.
  accum: u32,
  _pad0: u32,
  _pad1: u32,
}};

@group(0) @binding(0) var<storage, read>       a : array<f32>;
@group(0) @binding(1) var<storage, read>       b : array<f32>;
@group(0) @binding(2) var<storage, read_write> c : array<f32>;
@group(0) @binding(3) var<uniform>             d : Dims;

var<workgroup> ta : array<f32, {tile_sq}u>;
var<workgroup> tb : array<f32, {tile_sq}u>;

fn a_at(row: u32, col: u32) -> f32 {{
  // row < m, col < k
  if (row >= d.m || col >= d.k) {{ return 0.0; }}
  if (d.a_t == 1u) {{ return a[col * d.m + row]; }}
  return a[row * d.k + col];
}}

fn b_at(row: u32, col: u32) -> f32 {{
  // row < k, col < n
  if (row >= d.k || col >= d.n) {{ return 0.0; }}
  if (d.b_t == 1u) {{ return b[col * d.k + row]; }}
  return b[row * d.n + col];
}}

@compute @workgroup_size({tile}u, {tile}u, 1u)
fn main(@builtin(global_invocation_id) gid: vec3<u32>,
        @builtin(local_invocation_id)  lid: vec3<u32>,
        @builtin(workgroup_id)         wid: vec3<u32>) {{
  let row = wid.y * {tile}u + lid.y;
  let col = wid.x * {tile}u + lid.x;

  var acc: f32 = 0.0;
  let tiles = (d.k + {tile}u - 1u) / {tile}u;
  for (var t: u32 = 0u; t < tiles; t = t + 1u) {{
    let kx = t * {tile}u + lid.x;
    let ky = t * {tile}u + lid.y;
    // Bounds are handled in the accessors, so a ragged edge reads zero rather
    // than needing a separate epilogue kernel.
    ta[lid.y * {tile}u + lid.x] = a_at(row, kx);
    tb[lid.y * {tile}u + lid.x] = b_at(ky, col);
    workgroupBarrier();

    for (var i: u32 = 0u; i < {tile}u; i = i + 1u) {{
      acc = acc + ta[lid.y * {tile}u + i] * tb[i * {tile}u + lid.x];
    }}
    // The SECOND barrier. Without it a fast thread overwrites the tile while a
    // slow one is still reading the previous iteration out of it — a race that
    // produces plausible-looking wrong numbers rather than a crash, which is
    // the worst failure mode a training kernel has.
    workgroupBarrier();
  }}

  if (row < d.m && col < d.n) {{
    let idx = row * d.n + col;
    if (d.accum == 1u) {{ c[idx] = c[idx] + acc; }} else {{ c[idx] = acc; }}
  }}
}}
"#,
        tile = TILE,
        tile_sq = TILE * TILE,
        kb = (TILE * TILE * 4 * 2) / 1024,
    )
}

/// `im2col`: gather input patches into the GEMM's B matrix.
pub fn im2col_wgsl() -> String {
    format!(
        r#"
// One thread per (k, n) element of the column matrix.
struct Cs {{
  batch: u32, in_ch: u32, in_h: u32, in_w: u32,
  kernel: u32, stride: u32, pad: u32, out_h: u32,
  out_w: u32, k_rows: u32, n_cols: u32, _pad: u32,
}};

@group(0) @binding(0) var<storage, read>       src : array<f32>;
@group(0) @binding(1) var<storage, read_write> col : array<f32>;
@group(0) @binding(2) var<uniform>             s   : Cs;

@compute @workgroup_size({wg}u, 1u, 1u)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
  let idx = gid.x;
  let total = s.k_rows * s.n_cols;
  if (idx >= total) {{ return; }}
  let row = idx / s.n_cols;
  let cl  = idx % s.n_cols;

  let per_image = s.out_h * s.out_w;
  let b   = cl / per_image;
  let rem = cl % per_image;
  let oy  = rem / s.out_w;
  let ox  = rem % s.out_w;

  let kk = s.kernel * s.kernel;
  let c    = row / kk;
  let krem = row % kk;
  let ky = krem / s.kernel;
  let kx = krem % s.kernel;

  // Signed, so the padding test is a comparison rather than a wrap. In u32 a
  // negative coordinate becomes an enormous positive one that passes a naive
  // `< in_h` check on some inputs and reads whatever is at that offset.
  let iy = i32(oy * s.stride + ky) - i32(s.pad);
  let ix = i32(ox * s.stride + kx) - i32(s.pad);

  var v: f32 = 0.0;
  if (iy >= 0 && ix >= 0 && iy < i32(s.in_h) && ix < i32(s.in_w)) {{
    let si = ((b * s.in_ch + c) * s.in_h + u32(iy)) * s.in_w + u32(ix);
    v = src[si];
  }}
  col[idx] = v;
}}
"#,
        wg = WORKGROUP,
    )
}

/// `col2im`: scatter-add the column gradient back to the input's shape.
pub fn col2im_wgsl() -> String {
    format!(
        r#"
// One thread per INPUT element, gathering every column that read it.
//
// The other direction — one thread per column element, adding into the input —
// is the obvious way to write this and is wrong: overlapping patches make many
// columns target one input pixel, so it needs an atomic add per write. WGSL
// has no f32 atomics, and a compare-exchange loop on the bit pattern is both
// slow and a source of non-determinism that makes a training run
// irreproducible. Gathering costs re-deriving the patch indices and is exact.
struct Cs {{
  batch: u32, in_ch: u32, in_h: u32, in_w: u32,
  kernel: u32, stride: u32, pad: u32, out_h: u32,
  out_w: u32, k_rows: u32, n_cols: u32, _pad: u32,
}};

@group(0) @binding(0) var<storage, read>       col : array<f32>;
@group(0) @binding(1) var<storage, read_write> dst : array<f32>;
@group(0) @binding(2) var<uniform>             s   : Cs;

@compute @workgroup_size({wg}u, 1u, 1u)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
  let idx = gid.x;
  let total = s.batch * s.in_ch * s.in_h * s.in_w;
  if (idx >= total) {{ return; }}

  let ix = idx % s.in_w;
  let t1 = idx / s.in_w;
  let iy = t1 % s.in_h;
  let t2 = t1 / s.in_h;
  let c  = t2 % s.in_ch;
  let b  = t2 / s.in_ch;

  var sum: f32 = 0.0;
  for (var ky: u32 = 0u; ky < s.kernel; ky = ky + 1u) {{
    for (var kx: u32 = 0u; kx < s.kernel; kx = kx + 1u) {{
      // Which output position would have read (iy, ix) through this kernel
      // offset? Only an exact stride multiple did.
      let ny = i32(iy) + i32(s.pad) - i32(ky);
      let nx = i32(ix) + i32(s.pad) - i32(kx);
      if (ny < 0 || nx < 0) {{ continue; }}
      if (u32(ny) % s.stride != 0u || u32(nx) % s.stride != 0u) {{ continue; }}
      let oy = u32(ny) / s.stride;
      let ox = u32(nx) / s.stride;
      if (oy >= s.out_h || ox >= s.out_w) {{ continue; }}

      let row = (c * s.kernel + ky) * s.kernel + kx;
      let cl  = (b * s.out_h + oy) * s.out_w + ox;
      sum = sum + col[row * s.n_cols + cl];
    }}
  }}
  dst[idx] = sum;
}}
"#,
        wg = WORKGROUP,
    )
}

/// Elementwise backward for the activations the generator uses.
///
/// One kernel with a mode switch rather than three: they share their whole
/// shape, and three shaders is three chances for one to drift.
pub fn activation_bwd_wgsl() -> String {
    format!(
        r#"
struct P {{ n: u32, mode: u32, _a: u32, _b: u32 }};

@group(0) @binding(0) var<storage, read>       y  : array<f32>;  // forward OUTPUT
@group(0) @binding(1) var<storage, read>       dy : array<f32>;
@group(0) @binding(2) var<storage, read_write> dx : array<f32>;
@group(0) @binding(3) var<uniform>             p  : P;

@compute @workgroup_size({wg}u, 1u, 1u)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
  let i = gid.x;
  if (i >= p.n) {{ return; }}
  let v = y[i];
  var g: f32 = 0.0;
  switch p.mode {{
    // ReLU. Reads the OUTPUT, not the input: y > 0 exactly when x > 0, and
    // keeping the output means the forward pass does not have to stash its
    // input as well — on a generator that is a whole activation's worth of
    // memory per layer.
    case 0u: {{ if (v > 0.0) {{ g = dy[i]; }} }}
    // tanh: 1 - y^2, which is why the output is the useful thing to keep.
    case 1u: {{ g = dy[i] * (1.0 - v * v); }}
    // LeakyReLU(0.2), the discriminator's activation.
    case 2u: {{ if (v > 0.0) {{ g = dy[i]; }} else {{ g = dy[i] * 0.2; }} }}
    default: {{ g = dy[i]; }}
  }}
  dx[i] = g;
}}
"#,
        wg = WORKGROUP,
    )
}

// ── CPU reference implementations ───────────────────────────────────────────
//
// These exist to make the MATH checkable without a GPU. The shaders above and
// the functions below implement the same operations, and the tests verify the
// references against finite differences — so when hardware is available, the
// kernels have something exact to be compared against rather than a plausible
// number nobody can dispute.

/// `C = A·B` with optional transposes, matching `gemm_wgsl`.
pub fn gemm_ref(
    a: &[f32], b: &[f32], m: usize, k: usize, n: usize, a_t: bool, b_t: bool,
) -> Vec<f32> {
    let mut c = vec![0f32; m * n];
    for i in 0..m {
        for j in 0..n {
            let mut acc = 0f32;
            for p in 0..k {
                let av = if a_t { a[p * m + i] } else { a[i * k + p] };
                let bv = if b_t { b[j * k + p] } else { b[p * n + j] };
                acc += av * bv;
            }
            c[i * n + j] = acc;
        }
    }
    c
}

/// `im2col`, matching `im2col_wgsl`.
pub fn im2col_ref(src: &[f32], s: &ConvShape) -> Vec<f32> {
    let (_, k, n) = s.gemm_dims();
    let mut col = vec![0f32; k * n];
    for row in 0..k {
        for cl in 0..n {
            if let Some((b, c, iy, ix)) = im2col_src(s, row, cl) {
                let si = ((b * s.in_ch + c) * s.in_h + iy) * s.in_w + ix;
                col[row * n + cl] = src[si];
            }
        }
    }
    col
}

/// `col2im`, matching `col2im_wgsl`.
pub fn col2im_ref(col: &[f32], s: &ConvShape) -> Vec<f32> {
    let (_, k, n) = s.gemm_dims();
    let mut dst = vec![0f32; s.batch * s.in_ch * s.in_h * s.in_w];
    for row in 0..k {
        for cl in 0..n {
            if let Some((b, c, iy, ix)) = im2col_src(s, row, cl) {
                let di = ((b * s.in_ch + c) * s.in_h + iy) * s.in_w + ix;
                dst[di] += col[row * n + cl];
            }
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> ConvShape {
        ConvShape {
            batch: 2, in_ch: 3, out_ch: 4,
            in_h: 5, in_w: 6, kernel: 3, stride: 1, pad: 1,
        }
    }

    #[test]
    fn every_kernel_compiles() {
        // The half that can be checked without hardware. naga is the same
        // validator gpu-gemm uses as its shader fitness gate.
        for (name, src) in [
            ("gemm", gemm_wgsl()),
            ("im2col", im2col_wgsl()),
            ("col2im", col2im_wgsl()),
            ("activation_bwd", activation_bwd_wgsl()),
        ] {
            let module = naga::front::wgsl::parse_str(&src)
                .unwrap_or_else(|e| panic!("{name} parse:\n{}", e.emit_to_string(&src)));
            let mut v = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            );
            v.validate(&module)
                .unwrap_or_else(|e| panic!("{name} validate:\n{}", e.emit_to_string(&src)));
        }
    }

    #[test]
    fn the_workgroup_is_a_whole_number_of_agx_simd_groups() {
        // The AGX SIMD group is 32. A workgroup that is not a multiple wastes
        // part of every group it launches.
        assert_eq!(WORKGROUP % 32, 0, "{WORKGROUP} threads is not whole subgroups");
        // And the tiles must stay far under the ~32 KB that pins occupancy to
        // one workgroup per core.
        let bytes = TILE * TILE * 4 * 2;
        assert!(bytes <= 4096, "{bytes} bytes of threadgroup memory is too much");
    }

    #[test]
    fn a_transposed_conv_outputs_its_forward_inputs_shape() {
        // The generator is four transposed convs and the shape formula is
        // where sign errors live. A transposed conv is the GRADIENT of a
        // forward one, so running the forward formula on its output must give
        // back the input.
        let s = ConvShape {
            batch: 1, in_ch: 8, out_ch: 4,
            in_h: 4, in_w: 4, kernel: 4, stride: 2, pad: 1,
        };
        let (th, tw) = s.out_hw_transposed();
        assert_eq!((th, tw), (8, 8), "stride-2 k4 p1 should double");
        let back = ConvShape { in_h: th, in_w: tw, ..s };
        assert_eq!(back.out_hw(), (4, 4), "the round trip lost the shape");
    }

    #[test]
    fn padding_reads_zero_rather_than_an_edge_pixel() {
        // Clamping instead trains on duplicated border pixels, and the model
        // learns a border artefact nobody traces back to the loader.
        let s = shape();
        // Top-left output, top-left kernel tap: that is (−1, −1) in the input.
        assert_eq!(im2col_src(&s, 0, 0), None);
        // The centre tap of the same output position is (0, 0).
        assert_eq!(im2col_src(&s, 4, 0), Some((0, 0, 0, 0)));
    }

    #[test]
    fn im2col_then_gemm_equals_a_direct_convolution() {
        // The lowering is only worth anything if it computes the same thing.
        // Checked against a direct nested-loop convolution, which is slow and
        // obviously correct.
        let s = shape();
        let (m, k, n) = s.gemm_dims();
        let src: Vec<f32> = (0..s.batch * s.in_ch * s.in_h * s.in_w)
            .map(|i| (i as f32 * 0.37).sin())
            .collect();
        let w: Vec<f32> = (0..m * k).map(|i| (i as f32 * 0.11).cos()).collect();

        let col = im2col_ref(&src, &s);
        let got = gemm_ref(&w, &col, m, k, n, false, false);

        let (oh, ow) = s.out_hw();
        for b in 0..s.batch {
            for oc in 0..s.out_ch {
                for oy in 0..oh {
                    for ox in 0..ow {
                        let mut want = 0f32;
                        for ic in 0..s.in_ch {
                            for ky in 0..s.kernel {
                                for kx in 0..s.kernel {
                                    let iy = oy * s.stride + ky;
                                    let ix = ox * s.stride + kx;
                                    if iy < s.pad || ix < s.pad { continue; }
                                    let (iy, ix) = (iy - s.pad, ix - s.pad);
                                    if iy >= s.in_h || ix >= s.in_w { continue; }
                                    let si = ((b * s.in_ch + ic) * s.in_h + iy) * s.in_w + ix;
                                    let wi = ((oc * s.in_ch + ic) * s.kernel + ky) * s.kernel + kx;
                                    want += src[si] * w[wi];
                                }
                            }
                        }
                        let ci = oc * n + (b * oh + oy) * ow + ox;
                        assert!((got[ci] - want).abs() < 1e-3,
                                "b{b} oc{oc} y{oy} x{ox}: {} vs {want}", got[ci]);
                    }
                }
            }
        }
    }

    #[test]
    fn col2im_is_the_exact_transpose_of_im2col() {
        // The property that makes the backward pass correct, and it is testable
        // without knowing what a gradient is: for any u and v,
        // <im2col(x), u> == <x, col2im(u)>. If that holds, col2im IS the
        // adjoint and the input gradient is right by construction.
        let s = shape();
        let (_, k, n) = s.gemm_dims();
        let nx = s.batch * s.in_ch * s.in_h * s.in_w;
        let x: Vec<f32> = (0..nx).map(|i| (i as f32 * 0.23).sin()).collect();
        let u: Vec<f32> = (0..k * n).map(|i| (i as f32 * 0.17).cos()).collect();

        let lhs: f32 = im2col_ref(&x, &s).iter().zip(&u).map(|(a, b)| a * b).sum();
        let rhs: f32 = col2im_ref(&u, &s).iter().zip(&x).map(|(a, b)| a * b).sum();
        assert!((lhs - rhs).abs() < 1e-2, "not adjoint: {lhs} vs {rhs}");
    }

    #[test]
    fn the_weight_gradient_matches_finite_differences() {
        // dW = dY · Xᵀ, which is the same GEMM with a transpose flag. Checked
        // against a numerical derivative of the actual loss, because a
        // backward pass that is self-consistently wrong is the failure that
        // trains for a week and produces noise.
        let s = ConvShape {
            batch: 1, in_ch: 2, out_ch: 2,
            in_h: 4, in_w: 4, kernel: 3, stride: 1, pad: 1,
        };
        let (m, k, n) = s.gemm_dims();
        let src: Vec<f32> = (0..s.batch * s.in_ch * s.in_h * s.in_w)
            .map(|i| (i as f32 * 0.31).sin())
            .collect();
        let mut w: Vec<f32> = (0..m * k).map(|i| (i as f32 * 0.13).cos()).collect();
        let col = im2col_ref(&src, &s);

        // Loss = sum of outputs, so dY is all ones and dW = 1 · Xᵀ.
        let dy = vec![1f32; m * n];
        let dw = gemm_ref(&dy, &col, m, n, k, false, true);

        let loss = |w: &[f32]| -> f32 {
            gemm_ref(w, &col, m, k, n, false, false).iter().sum()
        };
        let eps = 1e-3f32;
        for idx in [0usize, 5, 11, m * k - 1] {
            let orig = w[idx];
            w[idx] = orig + eps;
            let up = loss(&w);
            w[idx] = orig - eps;
            let dn = loss(&w);
            w[idx] = orig;
            let numeric = (up - dn) / (2.0 * eps);
            assert!((dw[idx] - numeric).abs() < 1e-1,
                    "dW[{idx}] analytic {} vs numeric {numeric}", dw[idx]);
        }
    }

    #[test]
    fn the_input_gradient_matches_finite_differences() {
        // dX = col2im(Wᵀ · dY). Both halves of the chain at once, which is
        // where a lowering usually breaks: each piece right, composed wrong.
        let s = ConvShape {
            batch: 1, in_ch: 2, out_ch: 3,
            in_h: 4, in_w: 4, kernel: 3, stride: 1, pad: 1,
        };
        let (m, k, n) = s.gemm_dims();
        let mut src: Vec<f32> = (0..s.batch * s.in_ch * s.in_h * s.in_w)
            .map(|i| (i as f32 * 0.29).cos())
            .collect();
        let w: Vec<f32> = (0..m * k).map(|i| (i as f32 * 0.19).sin()).collect();

        let dy = vec![1f32; m * n];
        let dcol = gemm_ref(&w, &dy, k, m, n, true, false);
        let dx = col2im_ref(&dcol, &s);

        let loss = |x: &[f32]| -> f32 {
            let col = im2col_ref(x, &s);
            gemm_ref(&w, &col, m, k, n, false, false).iter().sum()
        };
        let eps = 1e-3f32;
        for idx in [0usize, 3, 9, src.len() - 1] {
            let orig = src[idx];
            src[idx] = orig + eps;
            let up = loss(&src);
            src[idx] = orig - eps;
            let dn = loss(&src);
            src[idx] = orig;
            let numeric = (up - dn) / (2.0 * eps);
            assert!((dx[idx] - numeric).abs() < 1e-1,
                    "dX[{idx}] analytic {} vs numeric {numeric}", dx[idx]);
        }
    }

    #[test]
    fn a_strided_lowering_is_still_adjoint() {
        // Stride 2 is what the generator's deconvs use, and it is where the
        // col2im gather's "only exact stride multiples" test earns its place —
        // a version without it silently sums the wrong columns.
        let s = ConvShape {
            batch: 1, in_ch: 2, out_ch: 2,
            in_h: 8, in_w: 8, kernel: 4, stride: 2, pad: 1,
        };
        let (_, k, n) = s.gemm_dims();
        let nx = s.batch * s.in_ch * s.in_h * s.in_w;
        let x: Vec<f32> = (0..nx).map(|i| (i as f32 * 0.41).sin()).collect();
        let u: Vec<f32> = (0..k * n).map(|i| (i as f32 * 0.07).cos()).collect();
        let lhs: f32 = im2col_ref(&x, &s).iter().zip(&u).map(|(a, b)| a * b).sum();
        let rhs: f32 = col2im_ref(&u, &s).iter().zip(&x).map(|(a, b)| a * b).sum();
        assert!((lhs - rhs).abs() < 1e-2, "strided lowering not adjoint: {lhs} vs {rhs}");
    }
}

// ── batch normalisation ─────────────────────────────────────────────────────

/// BatchNorm2d's backward pass, per channel.
///
/// The layer between every deconv in the generator, and the one place where
/// the gradient is not a matrix multiply. It is also the easiest backward pass
/// to get subtly wrong: the mean and variance are FUNCTIONS OF THE BATCH, so
/// changing one sample changes every other sample's normalised value, and a
/// derivation that treats them as constants produces a gradient that is
/// directionally right and quantitatively wrong. Training with it converges —
/// slowly, to somewhere worse — which is the failure mode that costs a week.
///
/// For `y = γ·x̂ + β` with `x̂ = (x - μ)/√(σ² + ε)` over N elements per channel:
///
/// ```text
/// dγ = Σ dy·x̂
/// dβ = Σ dy
/// dx = γ/√(σ²+ε) · [ dy − (Σdy)/N − x̂·(Σ dy·x̂)/N ]
/// ```
///
/// The two subtracted terms are exactly the coupling through μ and σ². Dropping
/// them is the mistake above.
pub fn batchnorm_bwd_wgsl() -> String {
    format!(
        r#"
// One WORKGROUP per channel. The reduction is over the channel's whole plane
// across the batch, which is far more than one workgroup can hold, so each
// thread strides through it and a small shared buffer combines the partials.
//
// The buffer is {wg} f32 = {kb} KB. Kept small deliberately: an AGX core has
// ~32 KB of threadgroup memory and reserving most of it pins occupancy to one
// workgroup per core, which on a memory-bound pass like this is the dominant
// cost. No subgroupAdd — Honeykrisp can hang on it (see ASAHI_GPU.md).

struct Bn {{
  channels: u32,
  // Elements per channel across the whole batch: batch * height * width.
  per_ch: u32,
  eps: f32,
  _pad: u32,
}};

@group(0) @binding(0) var<storage, read>       xhat  : array<f32>;  // normalised forward value
@group(0) @binding(1) var<storage, read>       dy    : array<f32>;
@group(0) @binding(2) var<storage, read>       gamma : array<f32>;
@group(0) @binding(3) var<storage, read>       istd  : array<f32>;  // 1/sqrt(var+eps), per channel
@group(0) @binding(4) var<storage, read_write> dx    : array<f32>;
@group(0) @binding(5) var<storage, read_write> dgamma: array<f32>;
@group(0) @binding(6) var<storage, read_write> dbeta : array<f32>;
@group(0) @binding(7) var<uniform>             p     : Bn;

var<workgroup> red_a : array<f32, {wg}u>;
var<workgroup> red_b : array<f32, {wg}u>;

@compute @workgroup_size({wg}u, 1u, 1u)
fn main(@builtin(workgroup_id) wid: vec3<u32>,
        @builtin(local_invocation_id) lid: vec3<u32>) {{
  let ch = wid.x;
  if (ch >= p.channels) {{ return; }}
  let base = ch * p.per_ch;
  let tid = lid.x;

  // Pass one: the two sums this channel needs.
  var s_dy: f32 = 0.0;
  var s_dyx: f32 = 0.0;
  for (var i: u32 = tid; i < p.per_ch; i = i + {wg}u) {{
    let g = dy[base + i];
    s_dy = s_dy + g;
    s_dyx = s_dyx + g * xhat[base + i];
  }}
  red_a[tid] = s_dy;
  red_b[tid] = s_dyx;
  workgroupBarrier();

  // Tree reduction. Both barriers are required: the second stops a fast thread
  // from writing the next round while a slow one still reads this one.
  var stride: u32 = {wg}u / 2u;
  loop {{
    if (stride == 0u) {{ break; }}
    if (tid < stride) {{
      red_a[tid] = red_a[tid] + red_a[tid + stride];
      red_b[tid] = red_b[tid] + red_b[tid + stride];
    }}
    workgroupBarrier();
    stride = stride / 2u;
  }}
  let sum_dy  = red_a[0];
  let sum_dyx = red_b[0];

  // The parameter gradients are the sums themselves.
  if (tid == 0u) {{
    dgamma[ch] = sum_dyx;
    dbeta[ch]  = sum_dy;
  }}

  // Pass two: the input gradient, with the coupling through mean and variance.
  // Dropping the two subtracted terms gives a gradient that trains — slowly,
  // to somewhere worse.
  let n = f32(p.per_ch);
  let scale = gamma[ch] * istd[ch];
  for (var i: u32 = tid; i < p.per_ch; i = i + {wg}u) {{
    let xh = xhat[base + i];
    dx[base + i] = scale * (dy[base + i] - sum_dy / n - xh * sum_dyx / n);
  }}
}}
"#,
        wg = WORKGROUP,
        kb = (WORKGROUP * 4 * 2) / 1024,
    )
}

/// CPU reference for `batchnorm_bwd_wgsl`. Returns `(dx, dgamma, dbeta)`.
pub fn batchnorm_bwd_ref(
    xhat: &[f32], dy: &[f32], gamma: &[f32], istd: &[f32],
    channels: usize, per_ch: usize,
) -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let mut dx = vec![0f32; channels * per_ch];
    let mut dgamma = vec![0f32; channels];
    let mut dbeta = vec![0f32; channels];
    for c in 0..channels {
        let base = c * per_ch;
        let (mut s_dy, mut s_dyx) = (0f32, 0f32);
        for i in 0..per_ch {
            s_dy += dy[base + i];
            s_dyx += dy[base + i] * xhat[base + i];
        }
        dgamma[c] = s_dyx;
        dbeta[c] = s_dy;
        let n = per_ch as f32;
        let scale = gamma[c] * istd[c];
        for i in 0..per_ch {
            dx[base + i] =
                scale * (dy[base + i] - s_dy / n - xhat[base + i] * s_dyx / n);
        }
    }
    (dx, dgamma, dbeta)
}

#[cfg(test)]
mod bn_tests {
    use super::*;

    #[test]
    fn the_batchnorm_kernel_compiles() {
        let src = batchnorm_bwd_wgsl();
        let module = naga::front::wgsl::parse_str(&src)
            .unwrap_or_else(|e| panic!("parse:\n{}", e.emit_to_string(&src)));
        let mut v = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        v.validate(&module)
            .unwrap_or_else(|e| panic!("validate:\n{}", e.emit_to_string(&src)));
    }

    #[test]
    fn the_reduction_buffer_stays_off_the_occupancy_cliff() {
        // Two f32 buffers of WORKGROUP entries. An AGX core has ~32 KB and
        // reserving most of it pins occupancy to one workgroup per core, which
        // on a memory-bound pass is the dominant cost.
        let bytes = WORKGROUP * 4 * 2;
        assert!(bytes <= 4096, "{bytes} bytes of threadgroup memory");
        // The tree reduction halves from WORKGROUP, so it must be a power of
        // two or the last stride drops elements.
        assert!(WORKGROUP.is_power_of_two(), "{WORKGROUP} breaks the tree reduction");
    }

    #[test]
    fn the_batchnorm_gradient_matches_finite_differences() {
        // The check that matters. A derivation treating mean and variance as
        // constants is directionally right and quantitatively wrong, and it
        // TRAINS — slowly, to somewhere worse. Only a numerical derivative of
        // the real loss catches that.
        let (channels, per_ch) = (2usize, 12usize);
        let eps = 1e-5f32;
        let mut x: Vec<f32> = (0..channels * per_ch)
            .map(|i| (i as f32 * 0.37).sin() * 2.0)
            .collect();
        let gamma: Vec<f32> = vec![1.3, 0.7];
        let beta: Vec<f32> = vec![0.1, -0.2];

        // Forward, and the loss: a weighted sum, so dy is not all ones and a
        // sign error cannot cancel.
        let w: Vec<f32> = (0..channels * per_ch).map(|i| (i as f32 * 0.29).cos()).collect();
        let forward = |x: &[f32]| -> (Vec<f32>, Vec<f32>, Vec<f32>) {
            let mut xhat = vec![0f32; channels * per_ch];
            let mut istd = vec![0f32; channels];
            let mut y = vec![0f32; channels * per_ch];
            for c in 0..channels {
                let b = c * per_ch;
                let mean: f32 = x[b..b + per_ch].iter().sum::<f32>() / per_ch as f32;
                let var: f32 = x[b..b + per_ch].iter().map(|v| (v - mean).powi(2)).sum::<f32>()
                    / per_ch as f32;
                let is = 1.0 / (var + eps).sqrt();
                istd[c] = is;
                for i in 0..per_ch {
                    xhat[b + i] = (x[b + i] - mean) * is;
                    y[b + i] = gamma[c] * xhat[b + i] + beta[c];
                }
            }
            (y, xhat, istd)
        };
        let loss = |x: &[f32]| -> f32 {
            let (y, _, _) = forward(x);
            y.iter().zip(&w).map(|(a, b)| a * b).sum()
        };

        let (_, xhat, istd) = forward(&x);
        let dy = w.clone();
        let (dx, dgamma, dbeta) = batchnorm_bwd_ref(&xhat, &dy, &gamma, &istd, channels, per_ch);

        let h = 1e-3f32;
        for idx in [0usize, 3, 7, per_ch, per_ch + 5, channels * per_ch - 1] {
            let orig = x[idx];
            x[idx] = orig + h;
            let up = loss(&x);
            x[idx] = orig - h;
            let dn = loss(&x);
            x[idx] = orig;
            let numeric = (up - dn) / (2.0 * h);
            assert!((dx[idx] - numeric).abs() < 5e-2,
                    "dx[{idx}]: analytic {} vs numeric {numeric}", dx[idx]);
        }

        // And the parameter gradients, which are the plain sums.
        for c in 0..channels {
            let b = c * per_ch;
            let want_g: f32 = (0..per_ch).map(|i| dy[b + i] * xhat[b + i]).sum();
            let want_b: f32 = (0..per_ch).map(|i| dy[b + i]).sum();
            assert!((dgamma[c] - want_g).abs() < 1e-4);
            assert!((dbeta[c] - want_b).abs() < 1e-4);
        }
    }

    #[test]
    fn ignoring_the_batch_coupling_gives_a_visibly_different_gradient() {
        // Guarding the guard: if the naive gradient happened to match, the
        // test above would prove nothing. It does not match, by a lot.
        let (channels, per_ch) = (1usize, 8usize);
        let xhat: Vec<f32> = (0..per_ch).map(|i| (i as f32 - 3.5) / 2.0).collect();
        let dy: Vec<f32> = (0..per_ch).map(|i| (i as f32 * 0.7).sin()).collect();
        let gamma = vec![1.0f32];
        let istd = vec![1.0f32];
        let (dx, _, _) = batchnorm_bwd_ref(&xhat, &dy, &gamma, &istd, channels, per_ch);
        // The naive version: dx = gamma * istd * dy, no coupling terms.
        let naive: Vec<f32> = dy.clone();
        let diff: f32 = dx.iter().zip(&naive).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 0.5, "the coupling terms make no difference here: {diff}");
    }
}
