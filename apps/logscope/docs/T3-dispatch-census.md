# T3 — where the 16 dispatches per layer-token actually go

Measured with `kernel-validate --write-fixture` + `LOGSCOPE_DISPATCH_HIST=1 count-dispatch` on the
2-layer qwen3 Q8_0 fixture (GQA 4/2, QK-norm present). 32.0 dispatches/token, 16.00/layer-token,
15 distinct kernels. Reproduced twice, identical.

| /layer-token | share | kernel | what it is |
|---|---|---|---|
| 4.00 | 25.8% | `RMSNORM_MATVEC_Q8` | **already-fused** RMSNorm+int8 matvec (the projections) |
| 2.00 | 12.9% | `QKNORM_ROPE` | **already-fused** per-head QK-norm + RoPE, on q and k |
| 2.00 | 12.9% | `ATTENTION_MH` | attention, one workgroup per query head |
| 2.00 | 12.9% | SwiGLU + down-proj + residual | **already-fused** FFN epilogue |
| 2.00 | 12.9% | `MATVEC_Q8_0` | plain int8 matvec (no norm to fuse — e.g. attn output proj) |
| 1.00 | 6.5% | tiled matvec | |
| 1.00 | 6.5% | `RMSNORM` | standalone norm |
| 0.50 | 3.2% | sampler penalty phase | per token, not per layer |
| 0.50 | 3.2% | gather/index | per token, not per layer |

## The correction

Commit b268fa06's message read the top row as "the RMS-norm reduction … FOUR per layer-token —
attn_norm, ffn_norm, and QK-norm on q and k, each its own dispatch", and concluded the QK-norm pair
was the fusion to go after. **That is wrong on both counts**, and the cause is worth naming: the
histogram labels a kernel by its first leading comment, and `RMSNORM_MATVEC_Q8`'s first comment is
`--- RMS reduction over the input row ---` (its *prologue*). I improved the labeller in that same
commit and then misread its output the same way the old labels would have misled me — a label is
not an identity, and the check that settles it is one grep for the dispatch site.

What is actually true:
- The top entry is the **fused** norm+matvec, i.e. the projection matmuls. The norm rides along free
  in the prologue. There is no separate norm dispatch to remove there.
- `QKNORM_ROPE` **already exists and already fires** — the fusion I proposed as the next win has
  been shipped for some time. Proposing it again would have been reimplementing a solved problem,
  which is the specific waste the handover's "how this session wasted time" section warns about.

## Where that leaves T2/T3

Every norm→matmul and norm→rope pair on this path is fused. The remaining 16 are dominated by work
that has to happen: 4 projection matvecs, 2 attention, 2 FFN, 2 plain matvecs, 1 tiled. Dispatch
COUNT is close to its floor for this architecture, so the encode-bound problem (encode = 84% of
decode) is unlikely to yield much more to fusion of this kind.

That reframes the remaining T2 candidates, in the order I would try them:
1. **Cheaper recording per dispatch**, not fewer dispatches — the keep-list allocation removal
   (2dea4fce) was one instance; bind-group and dims-uniform caching are already in place, so the
   next measurement is a profile of `record_dispatch` itself on the operator's M2, not another
   guess from here.
2. **Pass fusion** remains RETRACTED (it broke AR decode 6/6 in c4-736) and is the only change that
   would collapse the per-dispatch pass overhead wholesale. Revisiting it needs `--gate-passfuse`
   green on the operator's driver, not a sandbox result.
3. The redundant RMS reduction inside `RMSNORM_MATVEC_Q8` (every workgroup recomputes the row's
   sum of squares) is a COMPUTE redundancy, not a dispatch one. On a memory-bound matvec it is
   probably free, as its own comment claims — but that claim is unmeasured, and a two-pass variant
   is cheap to gate if someone wants the number.

## Reproducing

```
kernel-validate --write-fixture /tmp/tiny-qwen3-q8.gguf
LOGSCOPE_DISPATCH_HIST=1 count-dispatch
```
Both are deterministic (fixed seeds), so counts are comparable across machines.
