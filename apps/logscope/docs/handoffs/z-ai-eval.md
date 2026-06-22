# z-ai "more model support" bundle — evaluation & decision (2026-06-19)

**Decision: REJECTED for merge. Quarantined, not integrated.** A focused, rebased, *validated*
re-cut is the only acceptable path for the model-support content (see "Salvage path").

## What it was
A full `logscope-suite/` tree snapshot (15 MB), not a patch set. Titled "more model support."

## Why it was rejected
Diffed against the pristine original (`logscope-suite.zip`), the bundle is a **divergent-base
snapshot** whose changes go far beyond model support and would regress current work:

1. **It rewrites the mind-palace render files** — `scene3d.rs` (+587 lines of diff), `mesh3d.rs`
   (+261), `gpuscene.rs` (+76), `tabs/chat.rs` (+573). Wholesale-merging clobbers the region/wings
   work landed this session and the in-flight perf edits.
2. **It deletes existing files**: `pearl_input.rs`, `bin/pearl-input-view.rs`, and the entire
   `docs/prototypes/anamorph/` prototype + render set. These exist in the current tree; the z-ai
   snapshot predates them. A tree merge would silently remove the physical pearl-input UI and the
   node-reveal R&D.
3. **The model-support core is unvalidated and collides with merged work.** The real arch work lives
   in a **903-line `gpu-gemm/src/lib.rs` diff** (the Gemma2/Phi3 kernels: GEGlu_PACKED, GEMMA_RMSNORM,
   SOFTCAP, partial-rotary RoPE). `archcfg.rs` flips Gemma2/Phi3 to `supported: true`, but the
   bundle's *own* note admits they are **"M2-gated — kernels compile clean; bit-exact validation needs
   a reference run on real hardware."** Shipping arch support that *claims* support without a
   bit-exact gate violates the project's correctness bar (BATCH 0; every perf gate is `max|Δ|=0.0`).
   Worse, that 903-line lib.rs diff overlaps the exact decode/loader region where the **fused-QKV perf
   bundle was just merged** — direct conflict.
4. **The command-side bits are entangled, not standalone.** `vimmode.rs`/`cmdpalette.rs` add `CmdSpec`
   table entries and `Action::Parent`/`FirstChild`, but the handlers live in the divergent 573-line
   `chat.rs` rewrite; the new `Action` variants would break the exhaustive match in `chat.rs` without
   it. Nothing lifts cleanly in isolation.

## Salvage path (do later, only if validated)
The *intent* aligns with BATCH 7 ("Qwen2 Tier-1 arch") and is worth having. To capture it safely:
- Re-cut **only** the arch content (`archcfg.rs` registry fields `fused_qkv_tensor`/`partial_rotary`,
  the Gemma2/Phi3 kernel additions in `lib.rs`) as a unified diff **against the CURRENT tree** (post
  fused-QKV merge), resolving the decode/loader overlap by hand.
- Keep `supported: false` until each arch passes a **bit-exact gate vs llama.cpp/burn on the M2** —
  add `--gate-gemma2` / `--gate-phi3` mirroring the existing arch gates. Only flip to `true` per arch
  once its gate is green.
- Take the `[`/`]` Parent/FirstChild tree-nav (it's clean and unit-tested) as a **separate** small
  patch with its `chat.rs` handler, independent of the model work.
- Ignore all render-file (`scene3d`/`mesh3d`/`gpuscene`) and `chat.rs`-composer diffs from this bundle
  — they are divergent-base noise, superseded by current work.

Quarantined bundle: `~/incoming/z-ai-more_model_support--logscope-suite/` (not copied into the suite).
