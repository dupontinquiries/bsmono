# logscope-suite

One Cargo workspace for the egui port of **logscope**. Everything builds from
this directory — no external checkout, no symlink, no patch step.

## Layout
- `crates/logscope/` — the headless **logscope library** (vendored snapshot:
  GTK behind the optional `gui` feature, default features `convo`+`bloc`, the old
  GTK binary removed — the egui app is the frontend now). Lib-only.
- `crates/logscope-egui/` — the **egui/eframe app** (glow backend). Depends on
  `logscope` and, optionally, `gpu-gemm`.
- `crates/gpu-gemm/` — the pure-Rust **wgpu→Vulkan inference engine** + kernel
  validator. *Excluded* from the default build (built on demand).
- `burn-probe/` — Burn/Vulkan **correctness oracle** (dev-only, excluded).
- `logscope-patch/` — the patch + `llm_backend.rs` used to make the vendored
  logscope headless. Re-apply these to your canonical checkout if you swap it in.
- `INTEGRATION.md` — full integration notes.

## Quickstart (run from this directory)
- App (glow):                       `cargo run -p logscope-egui --release`
- App + pure-Rust GPU engine:       `cargo run -p logscope-egui --release --features wgpu-engine`
- GPU kernel/transformer validator: `cd crates/gpu-gemm && cargo run --release`
- Correctness oracle:               `cd burn-probe && cargo run`

The default build is light (no GPU stack); `gpu-gemm` and `burn-probe` compile
only when you ask for them.

## Using your own logscope instead of the vendored snapshot
Replace `crates/logscope/` with a symlink to your checkout, after applying the
patch there:
```
cd /path/to/your/logscope
patch -p1 < /path/to/logscope-suite/logscope-patch/logscope-headless.patch
cp /path/to/logscope-suite/logscope-patch/llm_backend.rs src/llm_backend.rs
```
then `ln -s /path/to/your/logscope crates/logscope` and keep it as the workspace
member. (Local llama.cpp inference on the M2: build with logscope's `llamacpp`
feature — see INTEGRATION.md.)
