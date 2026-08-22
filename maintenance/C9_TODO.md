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

- **C9-0c — Still unmeasured: the blackstar checkpoint bundle.** `queued`.
  509 MiB in `wip/archives/`, `refs/heads/main` = `5d0ec126`. Run the same probe
  on it before deciding anything. Given step-counter's result, expect a large
  fraction to be pre-rewrite assets — but expect is not measured.

---

## 1. Incoming work not yet integrated

Current unmerged refs (`verified`):

```
incoming/road-warrior           +1    fetched this session, untouched
origin/incoming/launcher        +5    touches manifest/projects.toml
origin/incoming/trap            +6    strict prefix of trap-autotune
origin/incoming/trap-autotune  +23    apps/studio + shared/analog-kit
origin/incoming/trap-session   +23    byte-identical to trap-autotune (c9c51289)
```

- **C9-1a — road-warrior into `games/`.** `queued` · `verified` it fetched and
  is self-contained (1 commit, `316a0b25`). Root-level Rust already split
  `core/` + `shell/`, which is the layering kit extraction wants, so the
  eventual kit work has a seam to use. Not yet moved under `games/`.

- **C9-1b — trap line: merge or drop.** `queued` · `verified` the fork point is
  clean (`main` has not touched `apps/studio` or `shared/analog-kit` since),
  `read` that it is 48 files / ~9.8K lines. Three refs are one line of work:
  merge `trap-autotune` and the other two are covered. `REPO_SIZE.md` §4 names
  this as a precondition for the history rewrite.

- **C9-1c — launcher line.** `queued` · `verified` it touches
  `manifest/projects.toml`, which `main` also edits, so expect a conflict.
  Per `BRANCH_INDEX.md`, the manifest reconciliation is its own commit on top
  of the import, not part of it.

- **C9-1d — c8-003 sibling monorepo.** `done` (merge) / `queued` (notes) ·
  `verified`. Merged in `96e35966`: 31 commits, 888 KiB, one conflict in
  `av1tuner_core/src/scene.rs` where both sides independently added the same
  `audio_streams: Vec::new()` and ours also explained why — kept ours as the
  superset. Lands the app repoints TODO.md had open under "the payoff"
  (bo-suite/voiceterm/snapclone-audio/rusty-board rms and ct_eq onto
  audio-kit/net-kit, rusty-board model_path onto voice-kit) plus twenty
  projects compiled and tested for the first time.
  **Still open:** their capture notes are parked at
  `refs/notes/toolcalls-c8003`, not merged. 99 commits on their side carry
  notes against 138 on ours, over disjoint commits, so the merge is mechanical
  but not obviously lossless — and a capture log lost to a careless notes merge
  is the one thing `SKILL_session_capture.md` says cannot be reconstructed.

- **C9-1e — c4-858-delta is currently unusable.** `blocked` · `verified`.
  Incremental, prerequisite `a9a8cb0b` is **not in this repo** and is **not**
  the head of the blackstar checkpoint bundle (`5d0ec126`). It may be an
  ancestor inside that bundle's history — unpacking it is the way to find out,
  and that costs ~510 MiB of disk. This is exactly the failure
  `INCOMING_BUNDLE_REBASE.md` documents; the standing rule there says the fix
  is a self-contained re-export from the sending side.

---

## 2. Bugs found, not yet fixed

- **C9-2a — `snapclone-crypto`'s build.rs silently rewrites a committed
  source file.** `queued` · `verified` (reproduced, then reverted).
  `build.rs` regenerates `include/crypto/snapclone_crypto_ffi.h` **into the
  source tree** on every build. The committed header is hand-maintained: it
  carries `CryptoError_Ok`-style alias macros with a comment explaining that
  cbindgen's variant naming shifts between versions, and `cbindgen.toml` sets
  `QualifiedScreamingSnakeCase`. So regeneration replaces `CryptoError_Ok` with
  `CRYPTO_ERROR_OK` and **deletes the alias block the C++ side depends on**.
  `SNAPCLONE_SKIP_CBINDGEN=1` exists but is opt-*out*, so the default behaviour
  destroys the file. The c8parallel session hit this too and worked around it
  by hand rather than fixing it ("left that committed header untouched rather
  than risk a mismatched regeneration").
  **Proposed fix:** generate to `OUT_DIR` by default — cbindgen still runs, so
  it still catches a Rust API that cannot be expressed in C — and write the
  committed path only under an explicit `SNAPCLONE_WRITE_FFI_HEADER=1`. That
  inverts the default from *destroy* to *no-op*.
  Anyone building this crate before the fix lands should set
  `SNAPCLONE_SKIP_CBINDGEN=1` and check `git status` afterwards.

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
  a 369 MiB bundle into a measured 888 KiB and stopped a 459 MiB one from
  silently undoing the history rewrite. Both belong in `maintenance/tools/`
  next to `zips_to_history.sh`.

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
