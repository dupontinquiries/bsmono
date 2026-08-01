# TODO: an LSP that flags non-vectorizing Rust in numeric code

Brief for another session. Self-contained; the findings below are measured, not assumed.

## Why

Two Rust-specific patterns cost 3x and 8x in this codebase's hot loops. Both are invisible on review,
survive every test, and are trivially fixable once seen. A language server could point at them the way
clippy points at a redundant clone.

## The two findings, measured

### 1. Float reductions serialise — 8x

```rust
let s: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();   // 5.00 us
```

Rust preserves IEEE order, so this is ONE dependency chain: every add waits for the previous. Eight
independent accumulators vectorize:

```rust
let mut acc = [0f32; 8];
for (ca, cb) in a.chunks_exact(8).zip(b.chunks_exact(8)) {
    for l in 0..8 { acc[l] += ca[l] * cb[l]; }             // 0.63 us  (+698%)
}
```

**Checked against Colibri**, whose C kernels do the same thing by hand — `float32x4_t ac0, ac1` combined
with `vaddvq_f32(vaddq_f32(ac0, ac1))` — and which does NOT use `-ffast-math`. So this is not "C is
faster"; it is that multiple accumulators are required in any language, and Rust's ordering guarantee
makes it mandatory rather than advisable.

Integer reductions do NOT need this: integer addition is associative, so LLVM reassociates freely.

### 2. Unprovable indices emit bounds checks — 3x

```rust
for i in 0..32 { sum += (raw[off + 2 + i] as i8 as i32) * (qa[i] as i32); }   // 2.45 us
```

`off + 2 + i` cannot be proven in range, so a check is emitted per element. Slicing to a fixed-size array
proves the length once:

```rust
let w: &[u8; 32] = blk[2..34].try_into().unwrap();
for i in 0..32 { sum += (w[i] as i8 as i32) * (qa[i] as i32); }               // 0.81 us  (+202%)
```

The `unwrap` is free — the range is constant. Measured smaller on aarch64 (+63%) than x86 (+200%), since
bounds checks are cheaper there, but still worth taking.

## What the LSP should do

**Scope: function bodies containing a loop over a slice, with float or integer arithmetic.** Outside
numeric code these patterns are fine and flagging them is noise.

Three diagnostics, in descending value:

1. **`float-reduction-serialised`** — an `iter().sum::<f32>()`, `fold(0.0, ..)`, or a `+=` into a single
   `f32` inside a loop whose trip count is not tiny. Quick-fix: rewrite with `chunks_exact(N)` and an
   `[f32; N]` accumulator. Do NOT fire on integer accumulators.
2. **`bounds-check-in-hot-loop`** — an index expression with more than one addend, or a variable base,
   inside a loop. Quick-fix: hoist a slice before the loop; suggest `try_into()` when the length is a
   constant.
3. **`reduction-order-changed`** (informational) — when applying fix 1, note that results will differ in
   the last bits. Correct for a dot product, wrong for anything relying on exact reproduction of a prior
   run's floats.

## Implementation notes

* **`rust-analyzer` already has the HIR** needed for both. This should be an extension or a fork adding
  diagnostics, not a new server — writing a Rust parser to do this would be the wrong shape of work.
* **Verify against the assembly, not intuition.** `cargo-show-asm` on the before and after is the check
  that a suggested fix actually vectorized. A quick-fix that does not change the emitted instructions is
  worse than none, because it teaches people to trust the diagnostic.
* **Benchmark harness first.** Both findings above came from a 30-line `examples/` binary comparing two
  spellings of the same function. Any new pattern should be measured that way before it becomes a
  diagnostic — this codebase has twice adopted a "faster" shape that measured slower (row blocking in
  cpumatvec, a scratch buffer in the matvec driver), and both were caught by measuring first.
* **Editor**: Helix consumes standard LSP diagnostics and code actions, so nothing Helix-specific is
  needed. Test in Helix and VS Code both; the code-action UX differs and a fix that is awkward to apply
  will not be applied.

## Reference material in this repo

* `apps/logscope/crates/gpu-gemm/examples/reduce_shape.rs` — the 8x float reduction comparison
* `apps/logscope/crates/gpu-gemm/examples/loop_shape.rs` — the 3x bounds-check comparison
* `apps/logscope/crates/gpu-gemm/src/cpuattn.rs` — `sum8`, with the Colibri comparison in the comment
* `apps/logscope/crates/gpu-gemm/src/cpumatvec.rs` — the sliced integer dot
