# Build / Compile / Validate WORKAROUNDS — Corpus **v2**

> **This is a CORPUS, append-only. DO NOT OVERWRITE.** Each conversation adds a version;
> together they are the running record of "the assistant couldn't compile/build/validate X →
> here's the workaround we used." A fresh chat **MUST read this and apply these** rather than
> rediscovering them.
>
> **v1 (prior, DO NOT overwrite — preserved in the tree):**
> - `handoff/cargo-workaround/` — the offline Rust toolchain bundle (`pack-rust.sh` + `rustup.sh`).
> - `docs/OVERLAY_COMPILE_BLOCKER.md` — the glfw/ui-overlay build blocker + deferred fix.
> - `handoff/SESSION_HANDOFF.md` — the "you cannot compile/run, use the static checkers" era.
> - `handoff/check_delimiters.py`, `check_shaders.py`, `check_octree_winding.py`, `force_recompile_shader.sh`.
>
> **v2 (this entry):** consolidates the above + everything learned through **r311**. Next session → add a `_v3` section or file; keep v1/v2 intact.

---

## 0. THE CARDINAL CONSTRAINT (the source of most workarounds)
**There is no GPU in the sandbox**, and historically there was no compiler either. So the assistant
**writes Rust/GLSL essentially blind** and the **operator builds/runs/validates on their Apple M2** (Asahi,
Vulkan ~1.2, FP32; macOS/MoltenVK de-prioritized) and pastes back logs / profiles / screenshots.
- **Gates that exist in-sandbox:** `cargo check` (compile) + `glslangValidator` (shader-compile). **Nothing
  else.** They prove the code *compiles*; they prove **nothing** about behavior, visuals, or perf.
- **Therefore:** mark any behavior/perf change `-UNVERIFIED`; mark compile-verified / inert / docs `-CHECKED`;
  never credit an operator-observed visual to un-validated new code (attribution discipline).

---

## 1. "Couldn't compile at all — no toolchain / rustup is network-blocked"
**Workaround:** the offline toolchain bundle in `handoff/cargo-workaround/`.
- Run `pack-rust.sh` on a *networked* machine (the operator's Mac is fine — bytes are arch-independent; the
  *target triple in the filename* picks the runtime arch). It downloads standalone rustc/cargo/rust-std
  tarballs, verifies each `.sha256`, and zips them with a self-contained `INSTALL.sh` (the same install
  machinery rustup uses — avoids the "can't find crate for std" trap of a raw rustc). Do **not** ship
  rustup-init (it still needs `static.rust-lang.org`, the blocked host).
- This is what brought `cargo check` online in-sandbox (cargo 1.96).

## 2. "`cargo test` fails to LINK"
Root cause: glfw symbol refs pulled in via `keybindings.rs`, **plus** `Drives` lacks `PartialEq`.
**Workaround: `cargo check` is the gate — never `cargo test`.** Do not "fix the tests to run"; that's a
rabbit hole. The exact verify command (use verbatim):
```
env CARGO_INCREMENTAL=0 cargo check --bin unendlich-fullgame --features render,window,gpu,ui-overlay,audio
```
`CARGO_INCREMENTAL=0` keeps checks clean/repeatable.

## 3. "lib-without-features won't compile"
The bare lib fails on glfw/render imports. **Workaround:** always check against the **`--bin
unendlich-fullgame` target with the full feature set** (the command in §2), not a bare `cargo check`.

## 4. "The menu / overlay can't BUILD in-sandbox" (glfw native build)
`--features ui-overlay` → `window` → `glfw-sys`, which builds native GLFW and links X11/Wayland + GL dev
headers and needs `cmake` — none allow-listed in some sandboxes. Documented in `docs/OVERLAY_COMPILE_BLOCKER.md`.
- **Workaround / split:** menu *logic* lives in `engine::app` (compiles + is testable); menu *drawing* (egui
  through glfw) is only compilable on the dev machine.
- **Deferred real fix:** a `render-overlay-offscreen` feature that swaps the glfw surface for an offscreen
  `VkImage` via the existing Vulkan harness (no glfw window) → would compile in-sandbox + render headlessly
  under lavapipe.
- **IMPORTANT current status (as of this sandbox):** the full §2 command (with `window,ui-overlay`)
  **currently SUCCEEDS** here — the glfw build deps are present/cached. So **try the full command first.** If
  a *future* sandbox lacks the glfw deps and the build dies on `glfw-sys`/cmake, **fall back** to
  `--features render,gpu` (drop `window,ui-overlay,audio`) to check the non-UI code, and rely on
  `glslangValidator` for shaders.

## 5. Shader compile workarounds
- **Verify a shader directly:** `glslangValidator -V --target-env vulkan1.2 <file>.comp -o /tmp/x.spv`.
- **`#include` needs:** `#extension GL_GOOGLE_include_directive : require`.
- **Octree include ORDER matters** (GLSL can't pass SSBOs as function params): include `octree_types.glsl`
  → declare the `OctreeNodes { OctreeNode nodes[]; }` SSBO → **then** include `octree_sample.glsl` (its
  functions reference the global `nodes[]`).
- **`build.rs` regenerates SPV:** shaders in the `SHADERS` list compile to `assets/shaders/*.spv`, which
  pipelines `include_bytes!`. `cargo check` runs `build.rs` (so edits take effect), but **also `glslang` the
  file manually first** to catch errors fast. `handoff/force_recompile_shader.sh` + `handoff/shaders-staging/`
  exist for forcing recompiles.
- **Push-constant alignment:** vec3 in push constants is 16-aligned → padding surprises. **Use
  `vec4`/`ivec4`/`uvec4`** in GLSL and a `#[repr(C)]` Rust struct with matching field order + a
  `const _: [(); N] = [(); size_of::<Push>()];` size assertion (this pattern is used in
  `neural_feat_pipeline.rs` / `neural_gemm_pipeline.rs`).

## 6. Editing-tool workarounds (blind editing safety)
- **`create_file` fails if the path exists.** To overwrite, use bash `cat > path << 'EOF' … EOF`; to edit in
  place, use `str_replace`.
- **`str_replace` can silently DROP code** if `old_str`/`new_str` is too broad (this happened in r309 — the
  `neural_trainer` construction got dropped, leaving a dangling `} else { None };`). **Workaround:** keep
  `old_str` minimal + anchored on unique surrounding context; **after any non-trivial str_replace, re-grep
  for the symbols that must still exist and re-view** before moving on.
- **Static checkers (still useful):** `handoff/check_delimiters.py` (brace/paren balance on `.rs`/`.comp`),
  `handoff/check_shaders.py`, `handoff/check_octree_winding.py`. Run them on edited files as a net.

## 7. Python / pip
Ubuntu 24 = PEP-668 ("externally-managed"). **Workaround:** `pip install <pkg> --break-system-packages`.

## 8. "Can I trust lavapipe perf?" — NO
lavapipe (`lvp_icd.json`, software Vulkan) runs shaders for *validation* but its **perf ratios do NOT
transfer to M2** (the neural path measured 0.25× / GEMM-bound there). **Workaround:** never quote lavapipe
timings as predictive. The operator's M2 bench (e.g. `bench_lean.py`) is the only perf truth.

## 9. Pre-existing warnings — DO NOT chase
The `octree_pool_buffer` "field never read" warning and assorted unused-import/var warnings are
**pre-existing**. A clean `cargo check` ends in `Finished` with these still present — that's expected.

---

## 10. CHECKLIST — what a fresh chat MUST do
1. Verify with the **§2 command** (full features, `--bin unendlich-fullgame`). If glfw build dies in a bare
   sandbox, drop `window,ui-overlay,audio` (§4 fallback). **Never** rely on `cargo test` (§2).
2. **`glslang` every edited shader** (§5) before the full check; mind include order + push-constant alignment.
3. Use `cat >`/`str_replace` (not `create_file`) to overwrite; **re-grep + re-view after big str_replaces** (§6).
4. **Mark releases honestly:** `-CHECKED` (compiles/inert/docs) vs `-UNVERIFIED` (behavior/perf) — you cannot
   see the screen; the operator validates on M2 (§0).
5. Don't trust lavapipe perf (§8); don't chase pre-existing warnings (§9); don't try to make tests link (§2).
6. **Append the next session's findings as a `_v3` section/file — keep v1 and v2 intact (this is a corpus).**
