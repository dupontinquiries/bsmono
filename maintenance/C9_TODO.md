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

- **C9-2c — Sweep for other feature-unification hiders.** `queued` · `read`.
  `snapclone-nat` declared no tokio `sync` feature and compiled anyway, because
  `snapclone-databus` and `snapclone-relay` enable it and a whole-workspace
  build unifies features. c8parallel independently hit the same class in
  `scene-kit`. A `cargo check -p <each>` sweep, one crate at a time, is the
  test; there is no reason to think two instances is all of them.
  **Note the asymmetry with C9-2e:** the stale-lock class got a cheap static
  checker because absence is visible in a text file. Feature unification is not
  — it needs a real per-crate resolve, which is minutes per crate here. This one
  stays a sweep, not a tool.

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

- **C9-3a — `cap.sh`: pin `CAPTURE_LOG` across shells.** `queued` · `verified`.
  `session_capture.sh` defaults `CAPTURE_LOG` to `${TMPDIR}/session-capture-$$.jsonl`,
  and `$$` is the sourcing shell's pid. An agent harness runs every command in
  a **fresh shell**, so each invocation opens a new log, the post-commit mark's
  same-log test never matches, and each commit gets only its own invocation's
  commands. This is visible in the history that shipped: the note on `fbcfd6a7`
  has exactly two lines. Pinning `CAPTURE_LOG` to one path fixes it and the
  notes this session carry full command sequences.
  This is failure mode #2 in `SKILL_session_capture.md` ("a new shell"), which
  documents the symptom but not this specific interaction with the pid-derived
  default. **Either fix the default or document the pin.**

- **C9-3b — `export-bundle.sh`: the `c9-NNN-<summary>.bundle` convention.**
  `queued` · syntax-checked and slug-normaliser `verified`; **the export path
  itself is unrun**.
  Does three things worth keeping: `--all` with no implied base (per
  `INCOMING_BUNDLE_REBASE.md`); explicitly appends `refs/notes/toolcalls`,
  which `--all` does **not** include; refuses a dirty tree and fails hard over
  500 MiB.

- **C9-3c — Bundle survey + incremental-cost probe.** `queued` (promoted from
  `idea`) · `verified` on three bundles.
  Two scripts. The survey answers "self-contained or incremental, and do I have
  the prerequisite" without fetching. The probe answers "what would this
  actually cost", via an alternates-backed scratch repo and
  `git repack -a -d -l`. The probe is the one that earned its keep: it turned
  a 369 MiB bundle into a measured 888 KiB and stopped two large ones from
  silently undoing the history rewrite. Both belong in `maintenance/tools/`
  next to `zips_to_history.sh` — `lock_check.py` landed there this session and
  these are the same kind of thing, so there is now a precedent and no excuse.

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

- **C9-4a — MLA layer loading.** `queued` · `read`, and **not verifiable in
  this sandbox**.
  The remaining gap is narrower than `archcfg`'s note implies. `Model.mla` is
  already populated (lib.rs:12075), `BlockWeights.mla` is already populated
  (lib.rs:~11738), and `block_decode_mla` is complete. What blocks it is one
  early `return Err` in the dense `wqkv` construction that fires *before* the
  layer loop reaches the MLA branch. The dense attention fields are not
  `Option`, so an MLA layer needs placeholders in `wq`/`wk`/`wv`/`wo`/`wqkv` —
  which is why the guard in `fa6eb6b9` landed first.
  **Verification honesty:** no Vulkan ICD, no `/dev/dri`, and the loader tests
  all `return` early on a missing `/tmp/tiny-*.gguf`. Writing this here would
  land in the same "unverified by compilation" category as the voiceterm and
  rusty-board repoints in `TODO.md`. `tools/tiny-variants/gen_glmdsa.py`
  reproduces the failure in 3 layers at d=64 and is the fixture to use on
  hardware.

- **C9-4b — MLA would double-allocate its KV cache.** `idea` · `read`.
  `decode_step_full` indexes `c.k[li]`/`c.v[li]` before the MLA match, so an
  MLA model appears to allocate a full dense KV cache alongside the compressed
  latent — defeating the ~7× compression that is the entire point of MLA. I
  have not checked whether `new_cache` skips them when `m.mla` is set. Worth
  settling *before* C9-4a, since it may change what loading should build.

- **C9-4c — No test exercises `load_gguf` at all.** `queued` · `verified`.
  All 1155 gpu-gemm tests are pure; every loader test guards on a fixture file
  that does not exist here and returns. That is the reason both MLA gaps went
  unnoticed, and it is why the two tests added this session are *structural*
  (`include_str!` over the source) rather than behavioural. A committed tiny
  GGUF fixture would convert a whole class of these into real tests.

- **C9-4d — Prefix caching over the existing fork/COW.** `idea` · `read`.
  `crates/logscope/src/kvcache.rs` (note: the *logscope* crate, not gpu-gemm)
  already has PagedAttention-style ref-counted blocks with `fork` and
  copy-on-write — the primitive prefix caching needs. What is missing is the
  coordinator above it; vLLM's `v1/core/kv_cache_coordinator.py` is the
  reference. This is also what `SERVING.md`'s "fleet endpoint /
  `run_fleet_trie` shared prefill" next-step needs. Pure and testable here.

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

- **C9-6a — beatspace GAN smoke test.** `in-progress` — the next task.
  From `beatspace-listeners-analogy.md`: multiple discriminators as simulated
  listeners; tiered models raced scout-and-wolfpack style; hash-to-musical-legend
  checkpoint naming; and a CPU-runnable smoke test on synthetic sine/saw/square
  waves to catch mode collapse after every architecture change. **The only item
  in the whole staged set that is self-contained and fully verifiable in this
  sandbox** — pure CPU, no GPU, no GUI. `apps/beatspace` exists but is 17 files.
  **New cross-reference:** `sensor-kit`'s `gait::Features::cadence_hz` (landed in
  `ee2d00ae`) already reports dominant walking cadence, gated to 0.5–4.0 Hz.
  ×60 is steps/min, which is the quantity the BPM-matching idea would key on.
  The producer exists; what is missing is a consumer.

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
