//! kernels — every WGSL compute source the engine ships, one file per kernel
//! (src/kernels/*.wgsl, real shader files: greppable, diffable, editor-highlighted).
//! Extracted verbatim from lib.rs; include_str! preserves bytes exactly, so pipeline
//! hashes and the wgsl_validate gates see identical sources. Doc comments moved with
//! their kernels. lib.rs `pub use`s everything, so crate::NAME paths are unchanged.

pub const GEMM: &str = include_str!("kernels/gemm.wgsl");
pub const LINEAR: &str = include_str!("kernels/linear.wgsl");
// Decode mat-vec (m=1), coalesced: ONE workgroup per output, the 64 threads split the k
// dimension so consecutive lanes read consecutive weights/activations (coalesced — bandwidth-
// optimal, vs the GEMM's per-thread strided rows), then tree-reduce. 2D grid (col = wg.y*gx +
// wg.x) so n>65535 (the vocab projection) fits the per-dimension workgroup limit. The reduction
// reorders the dot sum, so this is numerically equivalent but not bit-identical (~1e-6).
// Fused RMSNorm + int8 mat-vec (decode m=1): folds the input row's RMSNorm into the matvec's
// prologue so a norm→matmul pair becomes ONE dispatch. On Metal, where per-dispatch encoder setup
// (not GPU sync) is the decode wall, removing a dispatch is removing real time — and this pair fires
// TWICE per layer (attention-in, FFN-in). Each workgroup independently computes the row's
// inv = 1/sqrt(mean(x²)+eps) via a k-length reduction, then reads a[kk]·inv·nw[kk] in place of
// a[kk]. TRADEOFF: the reduction is recomputed once per output column (workgroup) — k extra input
// reads each — but the matmul's weight traffic is n·k and dominates bandwidth, so the redundant
// 2·k input reads are noise next to saving a whole dispatch's encoder setup. Net win exactly when
// setup cost > the extra reduction, which is the M4/Metal decode regime; LOGSCOPE_FUSE_NORM=0 backs
// out to the two-dispatch path for the A/B. Output is bit-identical to RMSNORM then MATVEC_Q8_0.
pub const RMSNORM_MATVEC_Q8: &str = include_str!("kernels/rmsnorm_matvec_q8.wgsl");
pub const MATVEC_Q8_0: &str = include_str!("kernels/matvec_q8_0.wgsl");
// Decode small-batch mat-vec (2<=m<=8): ONE workgroup per output, 64 threads split k. Each
// thread reads a weight value ONCE and accumulates it into all m activation rows — so the
// (memory-bound) weight read is amortized across the batch, like the m=1 mat-vec but for a
// small batch (the speculative-verify block, and batched decode at small N). Reduces per row.
pub const MATVEC_SMALL_Q8_0: &str = include_str!("kernels/matvec_small_q8_0.wgsl");
pub const LINEAR_Q8_0: &str = include_str!("kernels/linear_q8_0.wgsl");
// PREFILL GEMM, shared-memory tiled, with the workgroup tile as a wgpu OVERRIDE CONSTANT (`override
// TILE`) so the autotuner can specialize pipelines per shape without re-authoring WGSL. Each thread owns
// one output and accumulates k IN ORDER — so every TILE value is bit-exact to every other, and all match
// the naive LINEAR_Q8_0 within float tol (the a·(s·q) vs (a·s)·q association). Workgroup arrays are sized
// for the max tile (32 → 1024); a smaller TILE just uses a corner. Dequant happens on the W-tile load.
pub const LINEAR_Q8_0_TILED: &str = include_str!("kernels/linear_q8_0_tiled.wgsl");
// Register-blocked q8 prefill GEMM: each thread computes a 4×4 micro-tile of C (16 outputs), so each value
// loaded from the shared A/W tiles feeds 4 MACs instead of 1 — the arithmetic-intensity lever that takes a
// GEMM from single-digit % of peak toward tens of % ON NVIDIA/AMD. MEASURED RESULT: on the M2 Ultra (AGX)
// it LOST badly — 170 GFLOP/s vs the many-thread tiled@32's 999 — because AGX hides latency with high
// thread counts (tiled@32 = 1024 threads/wg), not per-thread ILP (RB = 256 threads, 24 live registers). So
// tiled stays the prefill default; RB is kept as a bench/portability option (it may still win on discrete
// GPUs that reward register blocking, which is why it's an autotune-eligible candidate elsewhere).
// Fixed params (no `override`) so it compiles on GL too and stays CI-validatable.
pub const LINEAR_Q8_0_RB: &str = include_str!("kernels/linear_q8_0_rb.wgsl");
pub const MATVEC_Q4: &str = include_str!("kernels/matvec_q4.wgsl");
// Residual-fold variants: identical to MATVEC_Q8_0 / MATVEC_Q4 but write c[col]=dot+resid[col],
// folding the post-projection residual add (wo/wdown) into the matmul epilogue — saves one ADD
// dispatch/layer each on the launch-bound decode path. resid is binding(3); c/dims shift to 4/5.
pub const MATVEC_Q8_0_ADD: &str = include_str!("kernels/matvec_q8_0_add.wgsl");
pub const MATVEC_Q4_ADD: &str = include_str!("kernels/matvec_q4_add.wgsl");
// Small-batch (2≤m≤8) Q4 mat-vec: one workgroup per output column, 64 threads split k, each weight
// nibble read ONCE and accumulated across all m rows (the bandwidth-bound cost stays near m=1 instead
// of riding the strided LINEAR_Q4, which re-reads the weights per tile). Mirrors MATVEC_SMALL_Q8_0; the
// per-term arithmetic (a*scale*qv) matches MATVEC_Q4 exactly, so each row is BIT-EXACT to the m=1 kernel.
// This is what lets Q4-resident (half the weight bytes of int8) compound with speculative/batched decode.
pub const MATVEC_SMALL_Q4: &str = include_str!("kernels/matvec_small_q4.wgsl");
pub const LINEAR_Q4: &str = include_str!("kernels/linear_q4.wgsl");
// Decode mat-vec, TILED: one workgroup computes `tile` (≤8) output columns, 64 threads split k.
// Each thread reads each activation a[kk] ONCE and reuses it across all `tile` columns — so the
// activation-reread bandwidth (which dominates the FFN mat-vecs, where the vector is far larger than
// one weight row) drops by `tile`×, while every weight is still read exactly once. Per-column
// accumulation is identical to MATVEC_Q8_0, so results are bit-identical to the untiled kernel.
pub const MATVEC_Q8_0_TILED: &str = include_str!("kernels/matvec_q8_0_tiled.wgsl");
// V3 decode mat-vec: 128-bit (vec4<u32>) wide loads — each thread pulls 4 words (16 q8 values) per memory
// transaction instead of one. The profile says decode is latency/occupancy-bound (≈6% BW, ≈1% ALU — the
// GPU idles on memory latency), and wider loads put more bytes in flight per request, the textbook fix.
// Requires k % 16 == 0 (always true for q8: k is a multiple of the 32-wide block). Same math as the others;
// results differ only by f32 reassociation.
pub const MATVEC_Q8_0_TILED_V3: &str = include_str!("kernels/matvec_q8_0_tiled_v3.wgsl");
// V2-T16: v2 with acc/red sized for tile up to 16 (more columns share each weight-word read). Costs 4 KiB
// workgroup memory (red[16*64]) — may lower occupancy, so it's a bench candidate vs v2@8, not a given win.
pub const MATVEC_Q8_0_TILED_V2_T16: &str = include_str!("kernels/matvec_q8_0_tiled_v2_t16.wgsl");
// V2-WG128: v2 with 128-thread workgroups — more threads per column split the k-reduction, exposing more
// memory parallelism per workgroup to hide latency (at the cost of a 7-step reduction and fewer workgroups).
pub const MATVEC_Q8_0_TILED_V2_WG128: &str = include_str!("kernels/matvec_q8_0_tiled_v2_wg128.wgsl");
// V2-WG128-SG: v2-wg128 with a SUBGROUP reduction. subgroupAdd collapses each 32-lane subgroup in one op
// (no barriers), and only the per-subgroup partials (nsg × tile) touch workgroup memory — vs the full
// red[tile*128] array + 7-step barrier tree. That frees shared memory and removes barriers, lifting
// occupancy — the diagnosed decode bottleneck. Requires the SUBGROUP device feature (Gpu.subgroups).
pub const MATVEC_Q8_0_TILED_V2_WG128_SG: &str = include_str!("kernels/matvec_q8_0_tiled_v2_wg128_sg.wgsl");
// V2 decode mat-vec: each thread owns WHOLE 32-bit weight words (4 quantized values), reading each word
// exactly once and using all four bytes — vs the original, where 4 threads read the SAME word (widx=kk/4)
// and extract one byte each. On drivers that don't coalesce same-address subgroup loads (a real risk on
// Asahi/AGX), that was up to 4× redundant weight traffic — the leading suspect for the decode running at
// ~6% of memory bandwidth. Fully coalesced (64 threads → 64 consecutive words). Bit-results differ from the
// original only by f32 reassociation (4-per-word accumulation order), far below q8 quant error.
pub const MATVEC_Q8_0_TILED_V2: &str = include_str!("kernels/matvec_q8_0_tiled_v2.wgsl");
pub const DEQUANT_Q4_0: &str = include_str!("kernels/dequant_q4_0.wgsl");
pub const SOFTMAX: &str = include_str!("kernels/softmax.wgsl");
pub const RMSNORM: &str = include_str!("kernels/rmsnorm.wgsl");
pub const ATTENTION: &str = include_str!("kernels/attention.wgsl");
pub const ROPE: &str = include_str!("kernels/rope.wgsl");
// NEOX split-half RoPE (DeepSeek/Qwen/Gemma convention): rotates dim i with i+half, unlike ROPE's
// adjacent (2i, 2i+1) pairing. Used by the MLA forward; matches mla.rs's rope_neox reference exactly.
pub const ROPE_NEOX: &str = include_str!("kernels/rope_neox.wgsl");
// ===== MoE v2 decode kernels: GPU-side routing (zero readbacks) =====
// MOE_TOPK: serial top-k over the router logits (n<=512, k<=16 — trivial work, the win is staying on-GPU).
// Selection order = descending logit; weights = softmax over the SELECTED logits (bit-faithful to
// moe::route's Softmax path: global softmax renormalized to the top-k == softmax over the selected).
// Output selw: [0..k) = expert index (u32), [k..2k) = combine weight (f32 bitcast to u32).
pub const MOE_TOPK: &str = include_str!("kernels/moe_topk.wgsl");
// MOE_MATVEC_Q8 / MOE_MATVEC_Q4: the base MATVEC_Q8_0/MATVEC_Q4 kernels with the weight row indexed
// through a GPU-side expert selection — row = sel[slot]*rstride + col — so the CPU never learns which
// experts won (no readback). `xper=1` reads a per-slot activation row (the down projection consumes
// hmid[slot]); `xper=0` broadcasts one activation row (gate_up consumes n2). Output c[slot*n + col].
pub const MOE_MATVEC_Q8: &str = include_str!("kernels/moe_matvec_q8.wgsl");
pub const MOE_MATVEC_Q4: &str = include_str!("kernels/moe_matvec_q4.wgsl");
// Fused gate‖up + SwiGLU (Q8). The unfused path was: MOE_MATVEC (write [slots,2ff]) → SWIGLU_PACKED
// (reduce to [slots,ff]) — two dispatches + a 2ff scratch buffer. This does both in ONE dispatch: each
// workgroup owns output column j in [0,ff), reduces BOTH the gate row (col=j) and the up row (col=ff+j)
// against the same activation, and writes silu(gate)·up straight to o[slot·ff + j]. Bit-identical to the
// two-step path (same reductions, same silu(v)=v/(1+exp(-v))). Halves the launch grid (ff not 2ff) and
// drops the intermediate buffer. `d.n` here is ff (the OUTPUT width); gate/up rows live at col and col+ff
// of the stacked weight (row stride rstride = 2ff).
pub const MOE_MATVEC_GLU_Q8: &str = include_str!("kernels/moe_matvec_glu_q8.wgsl");
// Fused gate‖up + SwiGLU (Q4) — identical structure, 4-bit weights (nibble − 8).
pub const MOE_MATVEC_GLU_Q4: &str = include_str!("kernels/moe_matvec_glu_q4.wgsl");
// Fused down-projection + weighted-combine + residual (Q8). The unfused path was: MOE_MATVEC (down,
// write [slots,d]) → MOE_WSUM (out[col] = resid[col] + Σ_s w[s]·down[s·d+col]) — two dispatches + a
// [slots,d] scratch. This does both in ONE: one workgroup per output column `col` loops over the selected
// experts, reduces each expert's down row against that slot's hmid activation, accumulates weight[s]·result
// (weights are the f32-bitcast tail of selw), adds the residual, and writes o[col]. Bit-identical to the
// two-step path (same reductions, same combine order s=0..slots). Removes 1 dispatch/MoE-layer + scratch.
pub const MOE_DOWN_COMBINE_Q8: &str = include_str!("kernels/moe_down_combine_q8.wgsl");
// Fused down + combine + residual (Q4) — identical structure, 4-bit weights (nibble − 8).
pub const MOE_DOWN_COMBINE_Q4: &str = include_str!("kernels/moe_down_combine_q4.wgsl");
pub const MOE_WSUM: &str = include_str!("kernels/moe_wsum.wgsl");
// Fused QK-Norm + RoPE for the decode step (t=1): ONE dispatch replaces the 4-chain
// (qk_norm(q) → qk_norm(k) → rope(q) → rope(k)) on the Qwen path. One workgroup per HEAD (q heads then
// k heads): 64 threads reduce the head's RMS (hd=128 → 2 elems/thread), normalize with the head's own
// weight (q_norm for q heads, k_norm for k heads), then each thread rotates exactly one pair (hd/2=64
// pairs) with the branch-free NORM/NEOX arithmetic. Output layout identical to the old chain:
// [0,ad)=roped q, [ad,ad+kv)=roped k. Float-tolerance note: the RMS reduction order differs from the
// standalone RMSNORM kernel → last-ulp differences, same tolerance class as the MoE v2 kernels.
pub const QKNORM_ROPE: &str = include_str!("kernels/qknorm_rope.wgsl");
pub const GATHER_ROWS: &str = include_str!("kernels/gather_rows.wgsl");
pub const SCATTER_ADD_W: &str = include_str!("kernels/scatter_add_w.wgsl");
// Row-broadcast scale: o[r,c] = x[r,c]·w[r]. The Llama4 weight-before-FFN step — the router gate
// multiplies the expert's INPUT rows (not equivalent to output-weighting through SwiGLU).
pub const ROW_SCALE: &str = include_str!("kernels/row_scale.wgsl");
pub const ADD: &str = include_str!("kernels/add.wgsl");
pub const ADD_BIAS: &str = include_str!("kernels/add_bias.wgsl");
pub const MUL: &str = include_str!("kernels/mul.wgsl");
/// AdaLN-Zero modulation: `o[r,c] = x[r,c] * (1 + scale[c]) + shift[c]`. `scale`/`shift` are a single
/// [1,cols] row derived from the timestep+conditioning embedding and broadcast over every token row —
/// this is what replaces the usual learned-affine LayerNorm/RMSNorm gain in a DiT block.
pub const DIT_MODULATE: &str = include_str!("kernels/dit_modulate.wgsl");
/// AdaLN-Zero gated residual: `o[r,c] = x[r,c] + gate[c] * y[r,c]`. `gate` starts at ~0 at
/// initialization (the "-Zero" in AdaLN-Zero) so a freshly-added DiT block starts as a no-op.
pub const DIT_GATED_ADD: &str = include_str!("kernels/dit_gated_add.wgsl");
/// Patchify: an [H,W,C] image (row-major, channel-last) → [num_patches, patch*patch*C] tokens, one
/// row per non-overlapping `patch x patch` block, patches scanned left-to-right/top-to-bottom and each
/// patch's contents flattened row-major-within-patch then channel-last (matches the ViT/DiT convention
/// so a plain linear projection turns a patch row directly into a token embedding).
pub const PATCHIFY: &str = include_str!("kernels/patchify.wgsl");
/// Unpatchify: the exact inverse of `PATCHIFY` — [num_patches, patch*patch*C] tokens → [H,W,C] image.
pub const UNPATCHIFY: &str = include_str!("kernels/unpatchify.wgsl");
pub const SILU: &str = include_str!("kernels/silu.wgsl");
/// 2D convolution, channel-last, batched: `x[N,H,W,Cin] * w[Cout,Cin,K,K] + bias[Cout] -> o[N,Hout,Wout,Cout]`.
/// Direct (untiled) accumulation — correctness first, matching how `block_decode`/`block_decode_at`
/// (the non-fused paths) came before their fused/tiled speed-ups elsewhere in this file. `Hout =
/// (H+2*pad-K)/stride+1` (same for W); the caller computes and passes these so the shader stays a flat
/// index walk. One invocation per output element (n,ho,wo,co).
pub const CONV2D: &str = include_str!("kernels/conv2d.wgsl");
/// GroupNorm, channel-last, batched: split `C` channels into `groups` equal chunks, normalize each
/// (n, group) over {its channels} × {every spatial position}, then apply the per-channel affine
/// `gamma`/`beta`. One invocation per (n, group) — each walks the whole group's spatial×channel extent
/// twice (stats, then write), same two-pass shape as `RMSNORM`'s per-row walk.
pub const GROUP_NORM: &str = include_str!("kernels/group_norm.wgsl");
/// Nearest-neighbor 2x spatial upsample, channel-last, batched: `x[N,H,W,C] -> o[N,2H,2W,C]`. The VAE
/// decoder's upsample stages use this (not a learned transposed-conv) — a plain repeat, followed by an
/// ordinary `conv2d` (stride 1, pad 1) to smooth/mix, matching the SD/SDXL-family VAE decoder design.
pub const UPSAMPLE2X: &str = include_str!("kernels/upsample2x.wgsl");
// True LayerNorm (mean-subtract + population variance + eps), then per-channel scale `gt` and bias
// `bt`. One row per invocation, mirroring RMSNORM's launch. Vision towers (CLIP/SigLIP/Qwen2-VL).
pub const LAYERNORM: &str = include_str!("kernels/layernorm.wgsl");
// tanh-approx GELU: 0.5x(1+tanh(√(2/π)(x+0.044715x³))).
pub const GELU_TANH: &str = include_str!("kernels/gelu_tanh.wgsl");
// QuickGELU: x·sigmoid(1.702x) (OpenAI CLIP).
pub const GELU_QUICK: &str = include_str!("kernels/gelu_quick.wgsl");
// Bidirectional multi-head attention — the ViT twin of ATTENTION_MH: identical layout & GQA mapping,
// but each query attends over ALL tokens (loop j in 0..t, no causal `j<=i` bound).
pub const ATTENTION_MH_FULL: &str = include_str!("kernels/attention_mh_full.wgsl");
// Fused SwiGLU: silu(gate)*up in one dispatch (replaces a separate SILU + MUL).
pub const SWIGLU: &str = include_str!("kernels/swiglu.wgsl");
// SwiGLU over a packed gate||up buffer: input row = [gate(h) | up(h)] (the fused
// wgate_up matmul output), so this reads both halves of one [t,2h] tensor and writes
// [t,h] = silu(gate)*up — pairing with the fused gate+up projection.
pub const SWIGLU_PACKED: &str = include_str!("kernels/swiglu_packed.wgsl");
// GeGLU-packed: gemma-family MLP gate. Same [gate|up] packed layout as SWIGLU_PACKED but the gate uses
// tanh-approx GELU (gelu_pytorch_tanh) instead of SiLU: gelu(v)·up. The building block for Gemma2 FFN.
pub const GEGLU_PACKED: &str = include_str!("kernels/geglu_packed.wgsl");
pub const GATHER: &str = include_str!("kernels/gather.wgsl");
// GATHER from an int8-resident (Q8_0-style) table: dequantize ONE row selected by ids[id_off].
// Dequant expression verbatim from MATVEC_Q8_0 (extractBits + select for AGX-safe signed bytes).
pub const GATHER_Q8_AT: &str = include_str!("kernels/gather_q8_at.wgsl");
// GPU argmax over a [1, n] row → index of the max into a 1-element u32 buffer. Single workgroup:
// grid-stride scan + shared-memory reduction. Tie-break MATCHES the CPU `argmax` (lowest index wins):
// strict `>` keeps the earlier index within a thread, and the reduction prefers the smaller index on
// equal values. Lets greedy decode pick the next token on-device — read back 4 bytes, not [vocab].
pub const ARGMAX: &str = include_str!("kernels/argmax.wgsl");
// Full CPU `pick()` on the GPU, depositing like ARGMAX: repeat-penalty → top-k → temperature
// softmax → min-p → top-p → uniform CDF walk, writing the chosen id into o[out_slot] AND into a
// rolling recent-context ring (o[kids..kids+rcap]) that the NEXT step's penalty reads — so a whole
// window of sampled steps runs with no CPU round-trip (greedy already had this via ARGMAX; sampled
// decode — what chat actually runs — did a full-vocab readback + a 151k-element CPU sort per token).
// The uniform draw comes from a CPU-precomputed xorshift01 stream (uni[out_slot]) so behavior is
// deterministic given the seed and directly comparable against the CPU sampler.
//
// Selection strategy (the part that must scale to a 151k vocab in ONE workgroup): penalties only
// LOWER logits (x/p keeps sign and shrinks; negative×p sinks further), so every member of the true
// post-penalty top-k lies within the RAW top-(top_k + n_penalized). We iterate raw maxima in exact
// descending (value, then ascending index) order — which makes exclusion of already-picked elements
// a single compare against the LAST pick (v < last_v || (v == last_v && i > last_i)) instead of any
// membership test — collect top_k+n_over candidates, then thread 0 applies the (≤64-entry) penalty
// override table, re-sorts the candidates by (post value desc, index asc) — exactly the CPU's
// stable descending sort semantics — truncates to top_k, and replays the CPU's serial float-op
// order for softmax/min-p/top-p/CDF. Host guarantees temperature>0 and 2 ≤ top_k ≤ 64.
pub const SAMPLE_AT: &str = include_str!("kernels/sample_at.wgsl");
pub const ROPE_MH: &str = include_str!("kernels/rope_mh.wgsl");
// Per-row-position RoPE: like ROPE_MH but each row reads its own absolute position
// from `pos[row]` instead of pos0+row. Needed for batched decode, where the N rows
// are N different sessions at N different positions.
pub const ROPE_MH_ROWS: &str = include_str!("kernels/rope_mh_rows.wgsl");
// MLA attention with SEPARATE score dim (qk) and value dim (vh) — MLA's qk_head (nope+rope) and v_head
// generally differ (DeepSeek-V3: 192 vs 128), so the standard ATTENTION_MH (single hd) can't be reused.
// One query row (decode step) at position `p` = t-1, causal over t cached rows. K is [t, nh·qk] with each
// head's [k_nope | k_rot] already materialized; V is [t, nh·vh]; Q is [1, nh·qk]. One thread per head.
// Pack the kv_b reconstruction [t, nh·(qk_nope+v_head)] + shared rot cache [t, qk_rope] into full per-head
// K [t, nh·qk_head] (each head = [k_nope | k_rot]) and V [t, nh·v_head]. One thread per (row, head).
// NEOX-rope the rope-part [qk_nope, qk_head) of each head of an MLA query [1, nh*qk_head] at position pos.
pub const MLA_QROPE: &str = include_str!("kernels/mla_qrope.wgsl");
// Pack the kv_b reconstruction [t, nh·(qk_nope+v_head)] into full per-head K [t, nh·qk_head] by
// concatenating the shared rot cache [t, qk_rope] onto each head's k_nope. One thread per (row, head).
pub const MLA_PACK_K: &str = include_str!("kernels/mla_pack_k.wgsl");
pub const MLA_PACK_V: &str = include_str!("kernels/mla_pack_v.wgsl");
pub const MLA_ATTN: &str = include_str!("kernels/mla_attn.wgsl");
// DSA lightning-indexer scoring (see dsa.rs::indexer_scores). One thread per cached key t computes
//   score[t] = (1/√nh) · Σ_h w[h] · ReLU( (q_idx[h]·k_idx[t]) / √hd )
// q_idx is [nh·hd] (rope'd), k_idx is [nk, hd] (one hd-wide key per position, shared across heads), w is
// [nh]. Bit-exact to the CPU reference: same per-head ReLU-then-weight, same √hd / √nh scaling, same order
// (heads inner, so the fp accumulation matches). This is the score that ranks keys for top-k selection.
pub const DSA_INDEXER: &str = include_str!("kernels/dsa_indexer.wgsl");
// DSA masked attention (see dsa.rs::masked_attention). One thread per value-dim column; a single query
// attends over only the keys whose mask[t] != 0, softmax renormalized over the selected set. q is [dk], k
// is [nk·dk], v is [nk·dv], mask is [nk]. With an all-ones mask this is exactly dense attention (the
// dense-equivalence anchor). Each thread owns one output column c and streams the selected keys twice
// (max, then weighted sum) so no shared memory is needed and it's bit-comparable to the CPU reference.
pub const DSA_MASKED_ATTN: &str = include_str!("kernels/dsa_masked_attn.wgsl");
// DSA top-k selection as a MASK (see dsa.rs::select_topk). One thread per key t sets mask[t]=1 iff key t is
// among the top `keep` by score, breaking ties by position (earlier wins) — exactly the reference's
// "> thr then == thr in ascending position order" rule, computed as a stable rank:
//   rank(t) = #{j: score[j] > score[t]} + #{j: score[j] == score[t] AND j < t};  mask[t] = rank(t) < keep.
// When keep >= nk every rank < nk <= keep, so all keys select (the no-op-below-threshold case) with no
// special branch. O(nk²) but nk is bounded by index_topk; matches the CPU set bit-for-bit incl. ties.
pub const DSA_TOPK_MASK: &str = include_str!("kernels/dsa_topk_mask.wgsl");
pub const ATTENTION_MH: &str = include_str!("kernels/attention_mh.wgsl");
// Prefill-extend attention: a segment of `s` queries at global positions [base, base+s)
// attends over a cache already holding [0, base+s) rows (parent prefix + this segment),
// causal by global position. Lets a shared segment be prefilled on top of a parent cache
// in O(s·(base+s)) instead of recomputing the parent — the batched `extend_prefix`.
// PARALLEL prefill attention: one workgroup per (query row, head), 64 threads splitting the causal KV
// loop [0, qpos] with a shared-memory softmax reduction — instead of ATTENTION_MH_EXT's single serial
// thread crawling the whole cache per row. Same math, same causal mask, but the per-row O(qpos·hd) work
// is spread across the GPU. This is what keeps each prefill dispatch SHORT (no watchdog trip) and fast.
//
// Shared score buffer sized at 2048 f32 = 8 KB, NOT the 32 KB max. This is deliberate, grounded in the
// Apple-GPU occupancy rule: every core has ~32 KB of threadgroup memory, and a workgroup's allocation
// divides how many run concurrently — 8 KB ⇒ 4 groups/core, 16 KB ⇒ 2, 32 KB ⇒ 1 (a matmul benchmark
// measured 400 vs 250 GFLOPS for 8 KB vs 32 KB for exactly this reason). 8 KB keeps occupancy high while
// still covering causal rows up to 2048 in one workgroup; longer rows fall back to the serial kernel
// (correct, slower) until a split-K causal prefill lands. `SCTILE` must match the array size below.
pub const ATTENTION_MH_EXT_PAR: &str = include_str!("kernels/attention_mh_ext_par.wgsl");
pub const ATTENTION_MH_EXT: &str = include_str!("kernels/attention_mh_ext.wgsl");
// Flash-style SPLIT-K decode attention, pass 1 of 2 — the long-context kernel. One workgroup per
// (query head, KV chunk): each computes its chunk's LOCAL softmax pieces — running max, exp-sum,
// and the unnormalized V-weighted accumulation — into a partials buffer [nh][nch][hd+2]. Pass 2
// combines chunks with the standard online-softmax rescale. Parallelism grows with context
// (heads × chunks workgroups) instead of saturating at n_heads; nothing needs scores[l] in shared
// memory, so there is no 4096-row cap. Numerics: per-chunk max + rescale reorders the summation
// vs the monolithic kernels — same tolerance class as the other fused paths (~1e-6 rel).
pub const DECODE_ATTN_FLASH_P1: &str = include_str!("kernels/decode_attn_flash_p1.wgsl");
// Two-span attention: the WARM (int8) span emitted as flash pass-1 partials. Same partial layout as
// DECODE_ATTN_FLASH_P1 ([hd accumulators | local_max | local_sum] per (head, chunk)) so the SAME pass-2
// combine folds warm chunks together with the hot f32 chunks — one online-softmax over the union. Reads
// int8 K/V (packed u32 + per-row f32 scale) with inline dequant, matching DECODE_ATTN_MH_Q8's layout.
// Writing into a shared `part` buffer at a chunk offset means warm and hot partials coexist and pass-2
// needs no changes. `coff` = the chunk index where this span's chunks start (hot span occupies [0,coff)).
pub const DECODE_ATTN_FLASH_P1_Q8: &str = include_str!("kernels/decode_attn_flash_p1_q8.wgsl");
// Split-K decode attention, pass 2: one workgroup per head folds its chunks — global max M,
// rescaled sum S = Σ sᵢ·exp(mᵢ−M), and o = Σ oᵢ·exp(mᵢ−M) / S.
pub const DECODE_ATTN_FLASH_P2: &str = include_str!("kernels/decode_attn_flash_p2.wgsl");
pub const DECODE_ATTN_MH_PAR: &str = include_str!("kernels/decode_attn_mh_par.wgsl");
pub const DECODE_ATTN_MH: &str = include_str!("kernels/decode_attn_mh.wgsl");
// ── int8 KV cache: per-row-quantized K/V (4x smaller than f32) ───────────────
// Per-row absmax scale: scale = max|x|/127 over each row of [n,c].
pub const ROW_ABSMAX: &str = include_str!("kernels/row_absmax.wgsl");
// Quantize+pack [n,c] f32 into [n,c/4] u32 (4 int8 per word) using per-row scales.
pub const QUANT_PACK: &str = include_str!("kernels/quant_pack.wgsl");
// Inverse of QUANT_PACK: unpack [n,c/4] u32 + per-row scales back to [n,c] f32. Promotes
// a demoted (int8) prefix cache to f32. AGX-safe signed extract (extractBits + select).
pub const DEQUANT_Q8_ROWS: &str = include_str!("kernels/dequant_q8_rows.wgsl");
// GQA decode attention reading int8-packed K/V + per-row scales (dequant on the fly).
pub const DECODE_ATTN_MH_Q8: &str = include_str!("kernels/decode_attn_mh_q8.wgsl");
/// Q4_K super-block dequantization. One workgroup per 144-byte super-block (256 weights).
///
/// Ported line-for-line from the CPU `gguf::dequantize` type-12 arm, which is validated by every Q4_K
/// model that loads today — so this shader has a reference to be checked against rather than a spec I
/// reconstructed. `--gate-q4k` asserts they agree exactly.
///
/// Bytes are read through a u32 view: super-block b starts at byte b*144, i.e. word b*36. The 6-bit
/// scale/min packing (`get_scale_min_k4`) is reproduced exactly, including the j>=4 branch that stitches
/// the high 2 bits from an earlier byte.
pub const Q4K_DEQUANT: &str = include_str!("kernels/q4k_dequant.wgsl");
/// Q4_K → our resident q4 layout, entirely on the GPU.
///
/// This is what removes the CPU load pass. Today the loader does Q4_K → f32 (CPU) → our q4 (CPU) → upload,
/// which for Scout is ~50 GiB of host copies and many minutes. With this kernel the raw file blocks go up
/// as-is and the conversion happens on the GPU, so nothing is materialized host-side and nothing is
/// dequantized to f32 at any point.
///
/// Output is a single buffer laid out qw‖qs, matching `quantize_q4_host_bytes` — so `q4_lin_from_slots`
/// wraps it unchanged. One workgroup per 256-weight super-block; its 8 lanes each own one 32-element
/// block, computing that block's absmax scale and packing 8 nibbles per word exactly as the CPU
/// `quantize_q4_resident_cpu` does (scale = amax/7, nibble = round(w/scale) + 8).
pub const Q4K_TO_Q4: &str = include_str!("kernels/q4k_to_q4.wgsl");

// ── Qwen3.5 Gated DeltaNet + gated attention (see GLM47-style notes in QWEN35_SUPPORT.md) ──
pub const DELTA_CONV_STEP: &str = include_str!("kernels/delta_conv_step.wgsl");
pub const DELTA_STEP: &str = include_str!("kernels/delta_step.wgsl");
pub const L2NORM_ROWS: &str = include_str!("kernels/l2norm_rows.wgsl");
pub const MUL_GATE: &str = include_str!("kernels/mul_gate.wgsl");
pub const DEINTERLEAVE2: &str = include_str!("kernels/deinterleave2.wgsl");
pub const DELTA_GB: &str = include_str!("kernels/delta_gb.wgsl");
