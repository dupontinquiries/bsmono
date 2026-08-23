# C9 — session backlog

Everything this session found and did not finish. Companion to root `TODO.md`
(the standing checklist) and `SCRATCHPAD.md` (the design log); this file is
scoped to one session's loose ends so they can be closed or promoted rather
than rediscovered.

**Every item says how it was established.** `verified` = a command was run and
its output is quoted. `read` = concluded from source or docs, not executed.
`assumed` = neither, and flagged as such. That distinction is the whole point
of the file — this session found three separate docs asserting engine state
that had stopped being true, and the fix is not to write more carefully but to
record what was actually checked.

**And a rule the first draft was missing: a tag records when a thing was
established, not that it stays true.** Section 4 below was written `read`, with
"not verifiable in this sandbox" as its premise, and both halves of that
premise were false within the same session — the crates build under cargo 1.96,
and `Gpu::new()` works despite there being no Vulkan ICD. A `verified` tag ages
exactly the same way: it means a command ran once, on one day, in one
environment. When an item blocks work, **retest its premise before believing
it** — this file's own worst entry was one nobody thought to recheck because it
looked rigorous.

Status legend, matching `SCRATCHPAD.md`: `idea` · `queued` · `in-progress` ·
`blocked` · `done` · `parked`

---

## 0. Size budget — resolved, and the method matters more than the answer

`REPO_SIZE.md` sets 500 MiB. Measured now, after merging c8-003 and extracting
step-counter: **370.31 MiB, 1 pack, 0 loose** (`verified`). It was 370.34 MiB
before both. The incoming work was effectively free.

- **C9-0a — Why REPO_SIZE.md said 551 MiB and this clone says 370.** `done` ·
  `verified`. Not a discrepancy, a sequence: the `filter-repo` pass already ran
  here. Our history carries **zero** objects for all three paths REPO_SIZE.md §2
  lists as rewrite candidates (`parts_web_full.bin.gz`, `parts_web.bin.gz`,
  `logscope-linux-x64`). That doc measured a pre-rewrite clone.

- **C9-0b — A bundle's file size is the sender's repo size, not the cost of
  receiving it.** `done` · `verified`. This is the finding worth keeping. A
  self-contained bundle carries its whole history, so `ls -la` says nothing
  about what we lack. Measured properly — scratch repo with
  `objects/info/alternates` pointing at ours, fetch, then `git repack -a -d -l`
  (`--local` excludes anything the alternate supplies) — the pack that remains
  IS the incremental cost:

  | bundle | on disk | actually new | verdict |
  |---|---:|---:|---|
  | `c8-003-monorepo` | 369 M | **888 K** | merged, 31 commits |
  | `step-counter--c4-803-gait` | 459 M | 256 M | **extracted**, 6 files |
  | `c8parallel--session-work` | 76 K | 339 K | merged, 23 commits |

  step-counter is the one that was genuinely heavy, and for a reason that makes
  merging it wrong rather than expensive: it forked at `b985c6d1`, **before**
  our rewrite, so ~247 MiB of its 256 is the blackstar/logscope assets the
  rewrite deliberately removed. Merging would have undone it. Its real payload
  was one commit, 7 files, 1178 lines, all under `shared/` — applied via
  format-patch + am in `ee2d00ae` so the original authorship survived.

- **C9-0c — blackstar checkpoint bundle.** `done` · `verified`. 487 MiB on
  disk, 279 MiB of objects we lack, and the same story as step-counter for the
  same reason: forked at `b985c6d1`, before the rewrite, carrying
  `parts_web*.bin.gz`, `logscope-linux-x64` and pre-rewrite `.iqm`/`scene.bin`
  revisions. We already hold every heavy `.pack` it ships. Source extracted in
  `952f4a5f` (10 commits); the assets stayed out. Pack after all three bundles:
  **370.38 MiB**, from 370.34 at session start.

  The extract-not-merge rule now has three data points and one clear statement:
  **a bundle that forked before the history rewrite cannot be merged, only
  harvested.** Its asset history is the purge, running backwards.

---

## 1. Incoming work — all integrated

**No unmerged refs remain.** Every branch and bundle handed over this session
is either merged or harvested, and the pack went from 370.34 to 370.41 MiB
absorbing all of it.

| line | how | result |
|---|---|---|
| `c8parallel` | merged | 23 commits, one disjoint overlap |
| `c8-003` | merged | 31 commits, 888 KiB, one same-fix conflict |
| `step-counter` | **extracted** | 6 files; forked pre-rewrite |
| `c4-858` blackstar | **extracted** | 10 commits; forked pre-rewrite |
| `road-warrior` | subtree-merged | 25 files, `core` + `shell` |
| `trap-autotune` | merged | 23 commits + `shared/analog-kit` |
| `launcher` | merged | 5 commits, merged clean after all |

- **C9-1a — road-warrior.** `done`. Imported at `b20897c5` (subtree merge, so
  blame reaches the original), registered at `bb957ffc`. 60 core tests pass
  headless. Its `BUILDING.md` says 56 — drifted by four, deliberately left
  alone inside the import.

- **C9-1b — trap line.** `done`, `07ee5a5e`. `incoming/trap` was a strict
  prefix and `trap-session` byte-identical, so three refs were one merge.
  Verified: analog-kit 3, studio-core 32, studio-live 59, studio-fx 133,
  studio-collab 70, studio-media 9 (`--no-default-features`; the `opus`
  feature builds libopus from C).

- **C9-1c — launcher.** `done`, `66cb0786`. Predicted to conflict on
  `manifest/projects.toml`; did not — both sides added adjacent entries.
  38 headless tests pass.

- **C9-1d — c8-003.** merged; **notes still parked** at
  `refs/notes/toolcalls-c8003`. Unchanged from the last round.

- **C9-1e — c4-858-delta.** `queued`. Prerequisite located inside the
  blackstar bundle. Applyable, almost certainly by extraction.

### New, from integrating

- **C9-1f — Two fullscreen app launchers, both `primary = true`.** `blocked`
  on a decision · `verified`. `apps.hatch-egui` and `apps.launcher-egui`
  overlap almost exactly, down to both doing their own freedesktop icon-theme
  resolution, and both target desktop+phone. REPO_SIZE.md §3 anticipated this
  ("the launcher port, the presentation work still to port into hatch"), so
  the intended end state is one launcher and this session landed the second.
  `launcher-egui/src/icontheme.rs` is the obvious first thing to share
  whichever wins: 9 tested functions, no UI dependency.

- **C9-1g — `kind = "kpackage"` is invisible to the orchestrator, not
  handled by it.** `idea` · `verified`. The launcher plasmoid's entry argues
  no orchestrator case is needed because build.sh has a safe unknown-kind
  fallback. True — but with `targets = []` the effect is that
  `build/build.sh --only launcher-plasmoid` prints the header and nothing
  else. Fine for a manual `kpackagetool6` install; worth knowing before
  anyone expects `--run` to deploy it.

- **C9-1h — A third accelerometer reader.** `idea` · `verified`.
  road-warrior's `shell/src/gyro.rs` reads `net.hadess.SensorProxy` over
  D-Bus. `shared/sensor-kit` parses the same daemon's `monitor-sensor` output.
  `apps/voiceterm/egui-frontend/src/iio_accel.rs` reads sysfs
  `in_accel_*_raw × scale`. One daemon, three parsers, and sensor-kit's own
  `gait.rs` header already names the voiceterm one as "to be promoted to a
  shared crate". Well past this repo's bar for lifting a kit. Not attempted:
  gyro.rs has never run against a live iio-sensor-proxy — only its failure
  path — and unifying three readers on the strength of one verified one is
  the wrong order.

---

## 2. Bugs found, not yet fixed

- **C9-2a — `snapclone-crypto`'s build.rs rewrote a committed header.** `done`
  · `verified` both directions, in `9932142f`. Default is now OUT_DIR — cbindgen
  still runs, so it still fails the build on an API that cannot be expressed in
  C, but nothing is written into the source tree. `SNAPCLONE_WRITE_FFI_HEADER=1`
  regenerates deliberately and warns that the hand-written `CryptoError_*` alias
  block will be lost. The tell that the old default was wrong: an escape hatch
  already existed and the trap was still the default path, and two sessions paid
  the tax before either changed it.

- **C9-2b — Ported crates may carry more doc examples written against the C++
  surface.** `idea` · `read`.
  `snapclone-nat`'s module doctest called `nat.start()`, which is the Qt
  class's method, not the Rust one (fixed in `cd85b79a`). The module doc says
  in its first line that it is a port of the Qt class, so the example was
  plausibly transcribed from C++ along with everything else. Worth a sweep of
  the other ported crates' `rust,no_run` blocks — they only compile when the
  crate builds standalone, which C9-2c shows is not a given.

- **C9-2c — Sweep for other feature-unification hiders.** `queued` · tool
  `verified`, sweep NOT RUN (c10). `maintenance/tools/feature_sweep.sh` now
  exists: 260 packages, one `cargo ailite check` each in its own directory,
  resumable because the run is hours long. `--report` names what was NOT
  REACHED so a partial run cannot be read as a clean one. Carries CLAUDE.md
  §3's native-dep gates, and that table is proven load-bearing — without the
  beatspace entry the crate goes ok(62s) → FAIL(150s). What remains is
  literally running it somewhere with the hours; this box cannot.
  ORIGINAL NOTE (c10, before the tool existed): `cargo check -p <each>` is the right test; the two known instances
  (`snapclone-nat` tokio sync, `scene-kit` — both found in earlier sessions)
  now check clean: fixes were committed. A full cross-repo sweep would take
  hours (50+ workspace members, 30–60 seconds per crate depending on features),
  but the methodology is proven and ready to run on a time-unlimited machine or
  in CI. Spot-checked sample (audio-kit, av1tuner_core, crypto-kit, logscope):
  all pass. Others timed out (bot-arena, fao-codec) which suggests build size
  rather than feature gaps. The issue class is real, the tool is the right one,
  but the time cost is high. Noted as parked rather than done: a CI gate that
  runs this sweep and stops a commit on new failures would be the highest
  confidence outcome.

- **C9-2e — Stale lockfiles: found by accident three times, now checked.**
  `done` · `verified`. `maintenance/tools/lock_check.py` scans every committed
  Cargo.lock for dependencies declared in its own Cargo.toml and absent from
  it. 110 lockfiles, 12 skipped as workspace members, **1 stale** —
  `apps/voiceterm`, missing `audio-kit`, left behind by c8-003's repoint
  commits. Fixed in `2557301d`.
  The failure is quiet by construction: a stale lock is fine online, because
  cargo resolves the name and rewrites the file. It bites offline, on a clean
  clone, or in CI.
  **Known limit, stated so the checker is not trusted past its range:** it
  verifies a package APPEARS in the lock, not that the dependency EDGE is
  recorded. Regenerating voiceterm's lock also added `net-kit` to web2qml's
  dependency list and the scan did not flag it, because net-kit was already
  present as a package. Edge drift needs the real resolver.

- **C9-2d — `snapclone-relay` cannot build here.** `parked` · `verified`.
  `openssl-sys` needs `libssl-dev`, absent in this sandbox. Environment, not
  code. Worth confirming it builds on hardware before assuming so.

---

## 3. Tooling that lives outside the repo and probably should not

All of these were written this session in `/home/claude/` and **will be lost
when the sandbox resets**. Each is small; the question is whether it earns a
place in `maintenance/tools/`.

- **C9-3a — `cap.sh`: pin `CAPTURE_LOG` across shells.** `done` · `verified`
  (c10, `bb55960f`). The default was fixed rather than the pin documented —
  keyed by (user, repo cksum), so fresh shells in one repo share one log and
  the hook computes the same fallback for commits from unsourced shells.
  Reproducing the two-line-note failure exposed a SECOND defect the pin never
  fixed: flush-on-next-command means a shell's final command is never
  written, so a one-command-per-fresh-shell harness recorded **nothing** —
  the pinned-log sessions survived only because their invocations ran many
  commands and lost one per invocation. An EXIT-trap flush was tried and
  abandoned on instrumented evidence (DEBUG firings inside the EXIT handler
  see a frozen BASH_COMMAND; one marker, three writes); the landed mechanism
  parks the pending command in a `$CAPTURE_LOG.pending.<pid>` sidecar and
  sweeps dead shells' sidecars in at the next source or commit as
  `exit:null`. `--selftest` simulates the harness end to end and fails on
  three deliberately broken copies. Pinning still works and still wins.

- **C9-3b — `export-bundle.sh`: the `c9-NNN-<summary>.bundle` convention.**
  `queued` · syntax-checked and slug-normaliser `verified`; **the export path
  itself is unrun**.
  Does three things worth keeping: `--all` with no implied base (per
  `INCOMING_BUNDLE_REBASE.md`); explicitly appends `refs/notes/toolcalls`,
  which `--all` does **not** include; refuses a dirty tree and fails hard over
  500 MiB.

- **C9-3c — Bundle survey + incremental-cost probe.** `done` · `verified`
  (c10). The c9 scripts were lost with the sandbox; rewritten as
  `maintenance/tools/survey-bundle.sh` and `probe-bundle.sh`.
  The survey answers "self-contained or incremental, and do I have the
  prerequisite" by parsing the bundle header up to the first empty line —
  nothing fetched — and calls out a missing `refs/notes/toolcalls`
  explicitly (failure mode #3). The probe answers "what would this actually
  cost" via the alternates-backed scratch + `git repack -a -d -l`, as in c9.
  Verified both directions: applyable/NOT-applyable on a crafted incremental
  pair; probe of the c9-005 bundle (369 MiB) against a clone made from it
  measures **0 bytes** in 0.3 s — fetch negotiation sees every tip already
  present and skips the transfer, so the speed is the mechanism, not a skip.
  One trap found by testing the failure path: a bundle recording only `HEAD`
  matches nothing under a `+refs/*:refs/*` refspec, and the fetch SUCCEEDS
  having fetched zero refs — the first probe draft printed a lying "0 bytes"
  for a bundle it never measured. Refspecs are now built from
  `git bundle list-heads`, and a success that lands no refs is refused.

- **C9-3d — `build/build.sh --run` cannot build anything on an x86_64 host.**
  `queued` · `verified`.
  `manifest/targets.toml` sets `desktop = aarch64-unknown-linux-gnu` because the
  build host is Asahi. The orchestrator only really builds when
  `triple == $HOST` (build.sh:55), so on x86_64 every project prints the cross
  note instead — including `--run --only peek`, which `LAYOUT.md` cites as the
  proof that cargo dispatch works end to end. Not a bug in the orchestrator;
  the host assumption is simply baked into the manifest. Options: a `host`
  pseudo-target, or `--host-triple` to override. Until then, real builds in a
  sandbox mean invoking cargo directly.

---

## 4. logscope AI backend

Context: `vllm-main` is staged in `wip/archives/` as a reference. Its **Rust
workspace** (15 crates, edition 2024, toolchain pinned 1.95) is the more
relevant half for this stack than the Python side.

> **CORRECTION (later the same session).** Everything below originally said
> this work was not verifiable here, on two premises that were retested and
> both failed. The first: these crates could not be built in this sandbox —
> true under cargo 1.75, false under 1.96 (`logscope` checks in 68 s under
> `cargo-ailite`; plain cargo gets OOM-killed, so the profile is what matters
> now, not the toolchain). The second: there is no GPU — there is no Vulkan ICD
> and no `/dev/dri`, but **`Gpu::new()` succeeds and real dispatches produce
> correct results**, via Mesa's software drivers in
> `/usr/lib/x86_64-linux-gnu/dri`. A test that had been passing in every run of
> this suite all session — `a_view_downloads_its_own_bytes_not_the_buffer_head`
> — was demonstrating a working device the whole time.
>
> This file exists to stop exactly that: a confident claim whose provenance
> nobody rechecks. It became the clearest example of the thing it was written
> to prevent, which is worth leaving on the record rather than editing away.

- **C9-4a — MLA layer loading.** `blocked` · `verified` (c10).

  Verified driving the glm-dsa fixture with `LOGSCOPE_TRY_UNSUPPORTED_ARCH=1`:
  MLA tensors ARE detected, dimensions ARE recovered from shapes, and loading
  FAILS at exactly the right place: "MLA LAYER LOADING IS NOT IMPLEMENTED", a
  diagnostic that already exists in the code (src/lib.rs:10153). This is a
  FUTURE feature, not a current gap — the skeleton is there but the forward
  path is unwritten. Secondary: glm-dsa also lists sparse attention (IndexShare)
  as unimplemented, but llama.cpp also falls back to dense there, and dense is
  EXACT at or below the index_topk bound, so it is acknowledged but not a
  correctness blocker.

  **The work is not one change but at minimum three:**
  1. The MLA forward path — the central piece, substantial.
  2. The allocator trim (C9-4b is measured; new_cache_for's own doc already
     gates the logic and documents the 10-100x waste). Gated on `all_layers_mla`
     per the design comment. Belongs in the same commit as the loader to avoid
     shipping broken output.
  3. A correctness gate on a tiny deepseek2 fixture (like qwen2/qwen3 have via
     bit-exact-vs-real). M2-hardware gates are reasonable; this sandbox can
     drive the fixture + gate construction but not the final validation.

  **Not verified here:** whether deepseek2's two expert-weighting gaps (both
  listed by archfeat) actually block loading, or whether they are masking a
  broader issue. The fixture would surface that; running it on M2 would settle
  it.

- **C9-4b — the dual KV allocation, measured.** `done` · `verified`.
  Not a bug: `new_cache_for` allocates the dense K/V for MLA models
  deliberately (a mixed checkpoint needs them), and `Model::kv_bytes_per_token`
  already sums both rather than reporting the compressed figure, so the context
  planner is not fooled. What was unknown was the size, and the justifying
  comment guessed it as "little":

  | model | dense B/token | latent B/token | ratio | both @8k |
  |---|---:|---:|---:|---:|
  | GLM-4.7-Flash | 1,638,400 | 92,160 | 17.8x | 13.20 GiB |
  | DeepSeek-V3 | 11,993,088 | 140,544 | 85.3x | 92.57 GiB |

  Against the 31 GiB clamped buffer ceiling, GLM-4.7-Flash at 8k spends 13.2
  GiB of cache to use 0.7, and DeepSeek-V3 is over the ceiling at 4k. Long
  context is the whole point of MLA, so the trim belongs in the SAME change as
  the loader — a loader that makes MLA work while the allocator hands the
  compression back is not worth shipping. Comment corrected, cost pinned by a
  test that fails if the allocator is ever fixed.

- **C9-4c — loader tests: substantially unblocked.** `in-progress` ·
  `verified`.
  One real end-to-end loader test now exists (above). The remaining piece is
  smaller than it looked: `gen2.py`/`gen3.py` emit weights-only GGUFs with **no
  `tokenizer.ggml.tokens`**, so the fixture-gated tests in `ctxwin` still
  return early even with the file present — they need a tokenizer, not just a
  model. Traced with a throwaway probe rather than guessed: the guard chain is
  three `Err(_) => return` in a row and only the third fires. Teaching the
  generators to emit vocab metadata would light those up.

  **And the pattern worth fixing beyond this crate:** a fixture-gated test that
  returns early passes silently. The new test prints `SKIPPED` and the exact
  command to generate its fixture; every other one in this tree does not, which
  is how the whole area stayed unexamined.

- **C9-4d — Prefix caching: THREE implementations, and the wiring is an
  ownership problem.** `blocked` on a decision · `verified` (c10). The original
  entry pointed at the wrong one. What is actually in the tree:

  1. **`gpu-gemm::run_fleet_trie`** (lib.rs:15632) — LIVE and validated
     bit-exact. Builds a radix trie over request tokens, `extend_prefix`
     per edge, deepest-shared-node grouping. But its `caches: HashMap<usize,
     KvCache>` is **local to one call** — two successive fleet runs over the
     same shared prefix re-prefill it from scratch. That is the real gap.
  2. **`gpu-gemm::PrefixStore`** (lib.rs:13536) — the persistence layer that
     would close it: GDSF-ranked, two-tier (hot f32 → warm int8 → drop).
     Built and tested, but its ONLY caller is a gate in `main.rs:7028`.
     FLEET.md names the wiring as its own next step and it has not happened.
  3. **`logscope::kvcache::Paged`** (677 lines) — PagedAttention block
     allocator, ref-counted with COW `fork`, content-addressed `admit_cached`,
     retention tuning. **Zero callers anywhere**, `#[allow(dead_code)]`. This
     is what the old entry proposed building a coordinator over; doing that
     would add a third consumer to the least-connected of the three while the
     live path still re-prefills every call.

  **Why the wiring hasn't happened, which is not laziness.** The two
  components' ownership models do not currently meet. `PrefixStore::get`
  returns `Option<&Tier>` — a borrow — while `run_fleet_trie` needs to hold
  parent caches AND insert freshly-built children in the same pass. `KvCache`
  wraps wgpu `Tensor` buffers and is not `Clone`, so a hot hit can be neither
  moved out nor copied; holding the borrow across the build loop conflicts
  with the `&mut self` that `insert` needs. Verified by reading the
  signatures, not inferred: `run_batched` takes `prefix: Option<(&KvCache,
  usize)>` (a borrow is fine at the RUN phase), `promote_cache(&QKvCache,
  len, kv_dim) -> KvCache` (warm→hot yields a fresh owned value), and
  `KvCache` at lib.rs:13260 derives nothing.

  **UPDATE (c10, `f045efd3`): the seam below is now IMPLEMENTED for (1)+(2).**
  `PrefixStore::take` transfers ownership out, `run_fleet_trie_cached` hands
  every node cache back at the end, and `run_fleet_trie` delegates to a shared
  inner with `store: Option<&mut PrefixStore>` so the validated path is the
  same code with `None`. 1219 tests pass on llvmpipe. Still open: the
  collision length guard is reasoned-correct but not test-enforced (this
  fixture's argmax does not move on garbage KV — needs M2), and (3) is
  untouched.

  Warm-tier hits are now covered (`05cbc785`): a budget set between the hot and
  warm footprints forces one demotion, and the next run must promote it back.
  That test deliberately does NOT assert token equality — promotion is lossy, so
  equality holds here only because this fixture's argmax is insensitive and
  would flake on a sharper model. It asserts what the design does guarantee:
  the entry demotes, the warm branch of `take` runs, and every request is
  answered. Both halves fail on broken copies.

  **A finding that changes how green this suite should be read:** 28 gpu-gemm
  test sites are fixture-gated and pass SILENTLY when the fixture is absent —
  the same class C9-4c flagged. Running `sh tools/tiny-variants/gen-all.sh`
  first takes the suite from 9 s to 23 s at the same pass count, which is the
  measure of how much was not executing. Any session reporting a gpu-gemm pass
  count without generating fixtures first is reporting a partly hollow number;
  this one did, in its first pass, before catching it.

  **The seam that fits, as originally designed:** give `PrefixStore` a
  `take(&mut self, gpu, key) -> Option<(usize, KvCache)>` that REMOVES the
  entry and transfers ownership (promoting warm→hot on the way out). Then
  `run_fleet_trie_cached(&self, …, store: &mut PrefixStore)` is a small delta
  over the existing function: take-on-entry, build-on-miss exactly as today,
  hand every node cache back with `insert` at the end and let `enforce` run
  once. Ownership transfers fully in both directions, so no borrow is held
  across a mutation. A NEW method rather than a change to `run_fleet_trie`,
  so the bit-exact-validated path stays untouched and the two can be
  differentially tested against each other.

  **Two things the tests must say.** Same fleet run twice must be
  byte-identical AND show prefill savings on the second run. And a warm
  (int8) hit is LOSSY by construction, so the bit-exactness test needs a
  budget large enough to keep entries hot; a separate test should pin that a
  demoted-then-promoted prefix is within int8 error rather than equal. Both
  are runnable here — the whole gpu-gemm suite (1216 tests) passes on
  llvmpipe in 9 s.

  **The decision that is not mine:** whether `logscope::kvcache` is the
  intended future (block-level paging, which `PrefixStore` does not do) or
  dead weight to delete. Same shape as C9-7a and C9-1f.

- **C9-4e — `logscope-serve`'s three named gaps have a Rust reference now.**
  `idea` · `read`. `SERVING.md` lists token streaming, an OpenAI-compatible
  `/v1/chat/completions` shim, and per-key budgets as next steps. vLLM's
  `server` and `chat` crates are those three, in Rust. Note the toolchain
  mismatch: vLLM pins 1.95, we have 1.96 — fine for reading, and its
  `rust-toolchain.toml` would demand a toolchain rustup cannot fetch here.

- **C9-4f — Throughput: be honest about what ports.** `read`.
  vLLM's tok/sec comes substantially from CUDA graphs, FlashAttention/FlashInfer
  and fused Triton kernels, none of which port to wgpu/Vulkan on Asahi. What
  ports is the *scheduling*: chunked prefill, prefix caching, shared prefill.
  Worth recording so the reference is not overpromised.

---

## 5. Open questions from the c8parallel merge

- **C9-5a — Two Live pages in ambient-learner.** `blocked` on a decision ·
  `read`. Our side's `shell` module dispatches to the ten ported pages
  including `LiveView`. c8parallel's commit says the actually-running Live page
  is `egui-frontend/main.rs`'s `LiveState` + `live::live_ui`, and extracted
  `share_row_ui` precisely so the running page could call it *without* anyone
  deciding which implementation wins — its comment says it sidesteps the
  question. The merge was textually clean and now inherits both. **The question
  is still open and now has two callers.**

- **C9-5b — c8parallel's egui work is trusted, not checked.** `read`.
  Verified on the merged tree: scene-kit 58, voice-kit 43, weather-share 6,
  voiced-core 15, snapclone-vcard 10, snapclone-piggyback 10, snapclone-crypto
  14 (+1 ignored), snapclone-databus 8. **Not** verified: the egui frontends
  (ambient-learner, tomescope, voiceterm) and studio — which is most of what
  those 23 commits changed. Its own `PORT_STATUS` commits say it had a compiler
  and ran the suites; this side has not reproduced that.

---

## 6. Staged reference material (`/home/claude/wip/`, not committed)

Deliberately outside the repo rather than gitignored inside it: a gitignored
path can still be `git add -f`'d. **Lost on sandbox reset**; originals are
read-only under `/mnt/user-data/uploads/`.

## Session 2 — built but not wired

Each of these is a pure, tested decision layer with no caller. They are listed
together because they share a failure mode: a module that compiles, passes and
is never reached looks identical in the test count to one that is doing work.
`orphan_check.py` finds files no `mod` reaches; nothing finds a module reached
only by its own tests.

- **C9-8a — `logscope-fan::continuation` has no caller.** `verified` (grep).
  Decides keep-going vs stop from cheap facts, escalating to a small model only
  when genuinely ambiguous. Wiring means the chat loop firing a synthetic turn
  on `Decided(continue)` — a change to the send path.
  OPEN QUESTION, operator's call: should an auto-continuation appear in the
  transcript as a visible turn, or as an invisible nudge? Visible costs context
  every time and makes the log honest about why the model kept going; given the
  reason strings, visible is probably right.

- **C9-8b — `readmark` feeds nothing.** `in-progress` · `verified` (c10,
  `7ab8700b`). The LOGIC either side of the gap is now pinned to compose:
  `crates/logscope-fan/tests/readmark_feeds_continuation.rs` drives
  `open_questions` into `decide`'s `questions_are_flagged` through the public
  surface only, and needed nothing unexported — the halves fit. Both
  directions are covered (badgeable → `KeepGoingCheck`, unbadgeable → hard
  stop) and both fail on broken copies.
  Still open, and it is all renderer work: nothing supplies scroll fractions
  to `observe`, nothing draws the badge. **And a caveat no type can enforce** —
  `questions_are_flagged: true` is the caller's PROMISE to render, not a
  derivable fact. A caller that passes true and draws nothing gets the worst
  outcome available: the run carries on and the question is buried with no
  mark, which is worse than never having had the flag.
  ORIGINAL NOTE: Tracks how far the operator
  has gone past each node plus a watermark. `open_questions` returns the ids to
  badge and `since_watermark` the unreviewed span, so the graph renderer needs
  no logic of its own — but nothing supplies scroll fractions and nothing draws
  the badge. Until it does, `continuation`'s `questions_are_flagged` must stay
  FALSE, which is the conservative branch.

- **C9-8c — `powerbudget` is not sampled.** `verified`. Nothing reads
  `/sys/class/power_supply` into `Source`, no mesh lux is routed from the
  Fairphones via bo-suite/snapclone, and `continuation` does not call
  `auto_turn_cap`. Three shells reaching three subsystems. The Jackery outlet
  details and sun-scheduling sit on top of `Reserve`/`forecast_wh`.
  NOTE the deliberate disagreement with `voiceterm::battery_ok_for_background`,
  which defaults UNMETERED on a missing `/sys`. Both defaults are right for
  their caller; do not unify them without deciding which risk each is taking.

- **C9-8d — `scenepick` is not called.** `verified`. The 3D scene still has no
  pointer plumbed to it and the render pass does not draw the focus ring or the
  hover lift. This is what the cursor request needs; the cursor now has
  something to land on but nothing puts it there.

- **C9-8e — `capability::profile` has no caller.** `verified`. The per-worker
  contract the HiveMind doc makes mandatory. Spawning a worker with it changes
  the coordinator's dispatch — the file whose own header calls it "the hardest
  code in the engine to change safely".
  DEFERRED SUB-ITEM: the doc's `transitions` field is unimplemented, because
  phase transitions are the coordinator's decision and it is unclear a worker
  should be told them at all.

- **C9-8f — `sessiontime`'s `TimeContext` renders but is unread by the
  matrix.** `verified`. It IS wired into the chat prompt (b300aeaa), but the
  hive-mind modes do not consult it.

## Session 2 — engine work blocked on hardware

- **C9-9a — the dims resizing is unconfirmed.** `read`. Six commits derived the
  dims cache and ring from the object budget and computed `safe_k` 1 → 11
  against a requested 4. Whether the clamp stops firing is a hardware fact. The
  clamp line now prints `N dims + M pool`; `SUBMIT_BY_DIMS_WRAP` climbing
  sharply means the ring divisor is too tight.

- **C9-9b — `examples/immediates_probe` has not run on the M2.** `verified`
  here on llvmpipe, where immediates are EMULATED with uniforms — the least
  demanding possible witness. Its first four lines say whether Honeykrisp grants
  `Features::IMMEDIATES` and at what size. Thirty seconds of hardware time
  decides whether the whole dims structure can be DELETED rather than resized.

- **C9-9c — push constants: reachable, not attempted.** `verified` (API read
  out of the vendored crates). Touches every dispatch site and every WGSL entry
  point's uniform declaration; the failure mode is a driver silently accepting a
  wrong offset. A scheduling decision now, not a research question.

- **C9-9d — IQ1_S gibberish is unexplained.** `read`. Every type in that file
  has a dequantizer and IQ1_S matches llama.cpp line for line. The converting
  path became the default when native was made opt-in (0af8f77f) — my change,
  and the new suspect, because it previously panicked in prewarm and may never
  have completed on a real checkpoint. Two runs would narrow it: a NON-UD IQ1_S
  file, or the same file with `LOGSCOPE_NO_Q5K_INT8=1`.

- **C9-9e — diffusion-gemma: `expsplit` derives the offsets, nothing routes
  them.** `verified`. Also unresolved: which half is gate. `Split::GATE_FIRST`
  records the assumption in one place; wrong means SwiGLU activates the wrong
  half. Needs a forward pass to settle.

- **C9-9f — gemma4's `inp_gate`/`proj` side path is unidentified.** `read` from
  the dump. A 256-wide pair per block with no gemma3 counterpart, plus a THIRD
  post-norm. Wants the HF config rather than the GGUF.

## Session 2 — verification gaps I am aware of

- **C9-10a — six voiceterm commits, none seen.** The left edge went from three
  panels to one collapsible bar, the tab hold was corrected to the QML's 380ms,
  and sessions were merged in — all verified by compiler, tests and a panel
  counter. Headless egui layout tests now assert the widths and the collapse
  (d155f4de), which is three measured facts where there were none, and still not
  a screenshot.

- **C9-10b — `default_width` does not pin a width.** `verified` on egui 0.34.
  Five call sites in this tree name a width they do not get: voiceterm's
  settings (300), blocks-drawer (340) and tab column (168), and av1tuner's two
  400s. Documented in `uikit-egui` with a test; whether each wants `exact_width`
  or the drag is a per-surface decision needing a real screen.

- **C9-10c — `fleet_build.sh` has not run end to end.** `verified` that it
  fails: a full `logscope` run needs more disk than this box has —
  `apps/logscope/target` alone reached 4.0 GB. The member skip and the feature
  table are reasoned and spot-checked, not run. If the
  `[-p logscope-egui --features wgpu-engine]` line is absent from the output,
  that is the symptom.

- **C9-10d — the WGSL seam test I wrote is weaker than the one already there.**
  `verified` by probing. Catching a subtle seam needs comparing each edge texel
  against its own neighbourhood rather than against the tile's worst step. Not
  done, and `wrapd` stands as correct by construction rather than verified.

- **C9-10e — three fence parsers, one shared scanner, one adoption.**
  `verified`. `verdict` migrated; `parse_plan` DECLINED it deliberately (the
  scanner is CommonMark-correct and that site needs tolerance of a known
  malformation), and `parse_tools`' brace balance is stronger than any fence
  rule for its job. One of three is the honest result and the scanner's docs say
  so. What is NOT done is the strategy doc's actual ask: a single artifact
  envelope across every consumer.

- **C9-10f — WGSL gates: every crate has one, none shares.** `verified`. Six
  crates carry a near-identical naga test, deliberately, because the tree runs
  FOUR naga versions (0.19, 22, 27, 29) that disagree — naga 29 accepts `fn f16`
  and 22 rejects it. The duplication is load-bearing; recorded here so nobody
  "fixes" it into a shared kit.

- **C9-10g — the dupscan ratchet went a whole session unrun.** `verified`
  (c10), fixed in `0f3af0fa`. `logscope::dupscan`'s `duplicate_names_do_not_grow`
  asserts a ceiling on duplicate public names and is the guard against the
  seven duplicate implementations its own header documents. Its ceiling was set
  to 347 in `0c599e55`; the C9-8 modules (`continuation`, `readmark`,
  `powerbudget`, `sessiontime`) then landed in `crates/logscope/src/` and pushed
  the real count to 353. c9 shipped with the test failing, because only
  crypto-kit's tests were rerun against the baseline — the ratchet fires on
  `cargo test -p logscope`, and nothing ran it. Caught in c10 by running the
  suite; the 6 new collisions are all name-sharing (`Step`, `Command`,
  `Source`, `decide`, `stalled`), not duplicate implementations, so the ceiling
  was raised deliberately.

  **Considered and rejected: porting the scan to `maintenance/tools/` as a fast
  static checker** next to `lock_check.py`, so `verify.sh` would catch this at
  session start without a 38 s compile. A Python port would be a SECOND
  implementation of the duplicate-detector, in a language where nothing can
  ever catch the two drifting apart — the exact failure `dupscan.rs` exists to
  prevent, and its own header lists six of seven duplicates as found by
  accident. The mechanism worked as designed; it was one session late because
  the crate's tests were skipped, which is a procedure gap (CLAUDE.md rule 5c
  already covers it) rather than a tooling one. If it recurs, the answer is a
  CI gate that runs `cargo test -p logscope`, not a reimplementation.

- **C9-7a — snapclone exists TWICE, and one copy absorbed two of my fixes.**
  `measured` · blocked on a decision that is not mine to make.

  ```
                     files   commits   last commit
  apps/snapclone       754       165   2026-08-22 ae9f92a3
  system/snapclone     654         8   2026-08-22 9932142f  (mine)
  ```

  Content diff: 600 files identical, **52 differ**, 102 exist only in `apps/`
  (the entire `egui-frontend/`, ~100 files), and 2 only in `system/` — one of
  which is `log.txt`. So `apps/` is the live tree with the port in it, and
  `system/` is a stale near-subset.

  **The docs disagree, which is how this persisted.** `LAYOUT.md` names
  `system/` canonical — "system-tier services that daemonize: snapclone (DHT /
  identity / relay, the canonical Rust↔C++ FFI)" — while
  `manifest/projects.toml` points `[apps.snapclone-egui]` at
  `apps/snapclone/egui-frontend` and marks it `primary = true`. Both are
  accurate about their own concern and together they are a trap.

  **I fell in it twice.** `cd85b79a` (snapclone-nat) and `9932142f`
  (snapclone-crypto build.rs) both landed in `system/`, the tree nobody builds.
  Ported to `apps/` in a later commit, where both bugs were genuinely present —
  the stale `nat.start()` doctest, and a `build.rs` rewriting a committed
  header on every build. The tokio `sync` feature turned out to be fixed in
  `apps/` already, so only two of the three applied.

  **What is NOT decided, deliberately.** Which tree survives, and what happens
  to the 52 differing files. That is a merge across 42 crates plus C++ headers
  and CMake, and the two trees have diverged in `src/` (19 files), `rust/` (16)
  and `include/` (10). Deleting either without reading those 52 is the `iq3`
  mistake at scale — and unlike `iq3` there is no test suite standing behind
  the loser.

  The cheap half is already done and worth doing first regardless of the
  outcome: `maintenance/tools/orphan_check.py` reports the duplicated
  `camera-bridge/src/shared_frames.rs` in both trees, and the file-level diff
  above regenerates in seconds
  (`python3 -c` over both trees hashing every file — see the commit).

- **C9-6a — beatspace GAN smoke test.** `done` · `verified` (c10).
  The smoke test is implemented (754 lines in src/smoke.rs) and wired into
  the CLI at `beatspace smoke [--steps N] [--seed N]`. Runs on CPU only with
  no GPU or GUI dependencies (`--no-default-features`). Defaults: 12 steps,
  synthetic sine/saw/square waveforms, seed 7. Asserts four structural checks:
  params_moved ≥ 50% (gradients flowing), z_diversity > 1e-4 (outputs vary with
  latent — mode collapse detection, the primary check), z_repeatability == 0
  (eval deterministic), d_score_spread > 0 (discriminator not frozen). Verified
  locally: 12 steps completed in 4 seconds, z-diversity 0.0010, params 100%
  moved, all checks pass. The design is explicit about scope: structural validation
  only, no convergence or quality checks — a test that needs a good model would
  never run. The sensor-kit cadence producer (`ee2d00ae`) is ready for a consumer
  that BPM-matches it to generated beats.

- **C9-6b — HiveMind → logscope architecture.** `idea`.
  `HiveMind_Strategy_Logscope.md`: logscope as a typed engineering pipeline
  rather than agents-talking-to-agents. A LIAISON stage, capability contracts
  so a worker knows its own job, and adaptive-resolution context where sections
  like COMPILER RESULTS shrink or drop when irrelevant. Overlaps heavily with
  the existing `liaison.rs` / `liaison_prompt.rs` / `WORKER_CONTRACT.md`.

- **C9-6c — tokodon as the fediverse reference for voiceterm-egui.** `idea`.
  KDE's Qt/QML Mastodon client. C++/QML, so reading material here, not buildable.

- **C9-6d — img2threejs.** `idea`. Apache-2.0 agent skill, v1.4.4, with a
  Python `forge/` state machine gating each stage. Structurally close to what
  the suspenders and `skill-creator` do: a visible, steerable loop.

- **C9-6e — logscope reference set.** `idea`. Two arXiv PDFs (index-cache
  attention; RL for coding agents with a human in the loop), a
  `deepseek-v4-flash-0731` config/tokenizer set (config only, no weights), and
  `whatmorecanyouinfer.md` — the N vs N+1 state-comparison idea for long-horizon
  repo work, and fan-out as problem-space search.

- **C9-6f — AI research capabilities transcript.** `parked`. Model-hierarchy
  economics for a hypothetical 5M-LOC project: caching, per-tier routing, hard
  budget limits, no-retry-without-a-budget.

---

## 7. Small corrections

- **C9-7a — Root `TODO.md` kit test counts were stale.** `done` (this commit).
  snapcore is 71 tests, not 25; crypto-kit is 28 + 1 ignored, not 36. Both
  `verified` by running them. The offline claim in the same table was also
  wrong for voice-kit and is now fixed upstream — see below.

- **C9-7b — `shared/voice-kit/Cargo.lock` was stale.** `done`, via the
  c8parallel merge. The committed lock predated the `crypto-kit`
  dev-dependency, so `cargo test --offline` failed on a clean clone against
  `TODO.md`'s claim that every kit builds offline. Both sessions found it
  independently and produced the same blob (`6db17fc9`).

- **C9-7c — `maintenance/journal.txt` does not exist.** `idea` · `verified`.
  Root `TODO.md` opens by calling it the session catalog. Either it was never
  committed or it was removed; the reference is dangling either way.

- **C9-7d — Rust 1.75 is not installed here.** `parked` · `verified`.
  `NEWCOMER_DEVELOPER.md` says both 1.96 and 1.75 should be available because
  some benches need rustc < 1.81. Only 1.96 is installed (from the offline
  bundle); `static.rust-lang.org` is unreachable from this sandbox, so a second
  toolchain would need its own offline pack.
