# c4-parallel — performance + backend patches (staged, NOT yet applied)

Backend/perf work to **expand the tree-graph design** (parallel fan-out) and **improve
model compatibility + tokens/sec**. Staged here for a later turn. All three were
dry-run checked with `git apply --check` against the current base (0623j) and **apply
clean**. The modules they add are not yet in the tree (only `devloop.rs` from the earlier
round is already merged), so these are additive.

> Apply with **`git apply`** (not `patch`) — the new-file hunks need git's `new file mode`
> headers. Full design + module API reference is in `HANDOFF_distributed_fleet.md`.

## Two tracks, one combined option

| file | scope | adds | new files / touched |
|---|---|---|---|
| `HANDOFF_distributed_fleet.patch` | `crates/logscope-egui` | distributed fleet + voice + speculation (13 modules) | 10 new / 12 touched |
| `HANDOFF_engine_perf.patch` | `crates/logscope` | engine perf + model support (11 modules) | 11 new / 12 touched |
| `HANDOFF_consolidated.patch` | **both crates** | everything above in one apply (25 files) | 22 new / 25 touched |

Pick **either** the consolidated patch (one shot) **or** the two per-track patches (one
track at a time). Don't apply the consolidated *and* a per-track patch — they overlap.

### Track 1 — distributed fleet / orchestration (`logscope-egui`)
Protocol + scheduler for multi-machine fan-out, plus the voice/speculation stack:
`enlist` (join protocol, `MAX_FLEET=10`), `dispatch` (capacity-aware scheduler + lease
sweep), `bridge` (project↔Task), `ollama` + `localexec` executors, `speculate`
(anticipatory investigation lanes + `LeadCache`), `voicefan` / `voice_ingest` / `reaction`
(speaker-stack → tree fanout, steer/stop from live speech), `explain` (code-grounded
explanation planner — the keystone), `ctxplan` (token-budgeted context assembly).
Always-on, no `gui` dep. The remaining *integration* work (a headless `logscope-node`
bin, real-clock wiring, runner `:enlist`/`:fanout`) is described in the `.md`.

### Track 2 — engine perf + model compatibility (`logscope`)
Pure planning/sizing layers beneath the GPU kernels: `specdecode` (speculative decoding,
greedy + sampled, lossless), `multimodal` (vision-language prompt assembly), `batching`
(continuous batching), `kvcache` (paged KV + COW prefix sharing), `arch` (GGUF →
Qwen/DeepSeek/MoE/MLA detection + honest Full/Partial/Unsupported verdict), `gemmplan`
(dispatch grids + the >65535-workgroup fold + split-K), `dedup` (content-defined model
compression), `moe` (MoE routing), `mla` (DeepSeek latent attention dims), `rope`
(metadata RoPE/YaRN scaling), `quantfmt` (GGUF quant sizing). The kernels themselves are
the seam (GPU hardware); these are the testable cores.

## Apply + verify (on a box with the toolchain)

```bash
GIT='git --git-dir=ai-git --work-tree=.'
# one-shot:
$GIT apply patches/c4-parallel/HANDOFF_consolidated.patch
# OR per-track:
$GIT apply patches/c4-parallel/HANDOFF_distributed_fleet.patch
$GIT apply patches/c4-parallel/HANDOFF_engine_perf.patch

rustcheck --fast crates/logscope-egui/src/*.rs crates/logscope/src/*.rs   # structure
cargo test -p logscope-egui --lib    # fleet/voice/speculation modules green
cargo test -p logscope --lib         # engine modules green
#   (note: logscope::shorten::test_drm_example is a known PRE-EXISTING failure, unrelated)
```

Reported green counts as shipped: consolidated → 146 logscope-egui + 67 logscope tests.

## Status / decision for later
- **Not applied yet** — pending the user's go-ahead and a turn of its own. The current
  priority thread is the mind-palace **visual** build-out (0623j still has scene work to
  do); applying a large backend patch is a separate, substantial change.
- When applied, the visual work and these patches are **orthogonal** (different crates /
  always-on modules), so order between them doesn't matter for compilation.
