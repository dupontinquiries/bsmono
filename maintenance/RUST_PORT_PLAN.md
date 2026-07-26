# RUST_PORT_PLAN — porting the monorepo's C, C++, and Python to Rust

*The directive is "port all C, C++, and Python to Rust." Measured honestly, that is ~940K lines
(games ≈700K with headers, apps+system ≈175K C++, vendor ≈42K, plus ~80K Python). This document
is the ledger that makes the directive executable without destroying working, tested code: every
component, its size, its porting strategy, and its blockers — sequenced by leverage. The doctrine
is the one this repo already practices, not a new invention.*

## The doctrine (from the ports already in flight)

1. **Twin, prove parity, then retire.** A port ships alongside its original with a mechanical
   parity gate (byte-identical output, bit-exact tensors, pixel/ctest equality), and the original
   retires only after the twin has run green everywhere the original ran. Precedents: gpu-gemm's
   bit-exact CPU-reference gates against llama.cpp semantics; the egui ports keeping Qt/GTK as
   "the parity reference"; blackstar's `rust_fsm` behind `-DUSE_RUST_FSM`.
2. **Port the seams, not the files.** Extract a pure core first (paperbound → `paperbound-core`
   is the live example), keep the host language as a thin shell, then replace the shell last.
3. **std-only for build gates.** Anything that gates a build (linters, validators, content
   checks) is written dependency-free so it compiles with bare rustc on any host — no lockfile,
   no network, no edition-mismatch circus (this sandbox's cargo 1.75 wall is the cautionary tale).
4. **Never port a negative.** Vendored third-party code and external backends are *replaced by
   Rust equivalents or kept*, not transliterated.

## Ledger

### Shipped (this pass)
* **blackstar `tools/lint_dead_fields.py` → `rust/contentlint` (91 → ~210 lines, std-only).**
  Byte-identical stdout and exit semantics proven on BOTH paths (clean tree, and a synthetic
  dead-field probe → identical DEAD/FAILED output, exit 1). Wired as ctest `lint_dead_fields_rs`
  via `cargo run` next to the Python test — the suite is 65 tests; both green here. The gap this
  closes: the six Python content gates silently drop out of the suite on hosts without Python3;
  the Rust twins run wherever cargo exists. Python retires per doctrine rule 1.

### Tier 1 — content/tooling Python → std-only Rust twins (weeks, mechanical, high leverage)
* blackstar's remaining 5 ctest gates: `content_build.py --quiet` (254 ln), `test_silhouette.py`
  (95), `map_score.py --check` (118), `test_openworld.py` (148), `test_gltf.py` (72) — same
  recipe as lint_dead_fields; contentlint grows one bin per gate. `map_score` and `openworld`
  need float care: match Python's formatting exactly or normalize both outputs in the parity
  diff.
* unendlich `harness/spv_reflect.py` + friends (~4.3K PY): the shader descriptor gate — port so
  shader CI needs no Python. SPIR-V header parsing is pure binary reading; std-only.
* logscope harness scripts (~1.5K PY), voiceterm build/test scripts (~3K PY), shared (~0.8K),
  tools (~5.4K — identify per-script; `rustcheck` is already Rust).
* blackstar's non-test tools (~15K PY total incl. the above): the content *pipeline*
  (arena_forge, content_build, gen_spine, ctex, extract_models…) — port after the gates, reusing
  their parsers.

### Tier 2 — Python applications → Rust (months, real projects)
* **paperbound (27.5K PY)**: continue the `paperbound-core` extraction — the pattern is already
  chosen; finish moving sim logic into the core crate, then replace the Python shell.
* **gyroball (13.4K PY + 35.8K C/C++)**: not even in the manifest yet; register it first, then
  same split as paperbound. The C/C++ side joins Tier 3.
* **kbm3 pipeline/vocoder/explorer (Python, torch)**: PARTIALLY BLOCKED and recorded as such in
  the manifest — the RpGAN R1/R2 penalties need second-order gradients, which Burn does not
  have. The Rust side (gpu/, burn 0.21) already mirrors what Burn *can* express. Port the
  explorer/server (no autograd) now; the trainers stay Python until Burn grows double
  backward or the loss is reformulated (gradient-penalty-free variant) — revisit, don't force.

### Tier 3 — C/C++ engines → Rust (quarters; seam-first)
* **blackstar (53K C + headers)**: the `rust_fsm` seam exists and is FFI-proven. Sequence:
  content structs/loaders (already lint-covered, data-driven) → sim/FSM modules behind the
  existing flag → renderer last (exengine SDL2/GL → wgpu is a rewrite, coordinate with the
  papercraft engines' shared primitives). ctest 64+1 is the safety net; keep it green at every
  step.
* **snapclone engine (apps 67.7K + system 66.8K C++ — establish whether these are one engine
  vendored twice; if so the port is half the size)**: camera/peer native engine. Seam: the
  network/protocol layer maps onto shared/net-kit + snapcore (already Rust); media pipeline
  last. The egui frontend already replaced the UI layer.
* **bot-arena C++ (6.8K)**: small; GTK4 original already has its egui twin — port the remaining
  native helpers whole.
* **photosync (1.6K C++)**: small daemon; port whole against its existing integration tests.
* **av1tuner / ambient-learner**: measure first (not in this census's >500-line cut for C/C++).

### Keep / replace-not-port (recorded reasons)
* **vendor/opendht (~25K C++ + bindings)**: third-party library. Porting it is rewriting
  OpenDHT; the Rust move is *replacement* — snapcore's DHT RPC already fronts it via net-kit, so
  the seam exists. Evaluate mature Rust DHT crates against the wire behavior; until one passes,
  opendht stays vendored. Not a porting target.
* **llama.cpp (external, not in-tree)**: gpu-gemm IS its Rust replacement, correctness-gated,
  already running real models. The llamacpp feature remains the fallback per its own docs; it
  retires model-family-by-model-family as the engine's arch coverage grows (GLM-4.7/deepseek2
  just landed — see GLM47_FLASH_SUPPORT.md).
* **Build glue (bash + inline python3 in build/build.sh, gitmind-sync.sh)**: keep. tomllib
  one-liners on dev hosts are not a portability risk; porting them buys nothing the gates don't.

## Sequencing rationale
Tier 1 first because it is mechanical, verifiable in-sandbox today (proven by the shipped
first port), and removes the Python3 host dependency from every test suite. Tier 2's paperbound
continues an in-flight migration. Tier 3 is real engineering measured in quarters, protected at
every step by the suites Tier 1 hardened — that ordering is the whole point.
