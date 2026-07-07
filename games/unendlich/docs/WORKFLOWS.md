# WORKFLOWS.md — Day-to-day usage of the unendlich launcher tools

This is the **how do I actually use these tools** doc. For *what* each
tool is, see the ship readmes (`docs/v0.24.2c.12-*-readme.md`). For
*why* the design is what it is, see `docs/DESIGN_NOTES.md`.

The goal of this doc: someone who finished the bootstrap and ran a
smoke test should know what command to type next, depending on what
they're trying to learn.

---

## 1. The four tools and when to reach for each

The launcher arc produced four user-facing commands. Each answers a
different question.

| Tool | Input | Output | Question it answers |
|---|---|---|---|
| `unendlich-launcher run <spec>` | One `.unendtest`, one version | One `.unendresult.json` + (maybe) `events.jsonl` | "Run this spec against this engine version and tell me what happened." |
| `unendlich-launcher compare --results <a> <b> [...]` | 2+ result files | Side-by-side metric table, driver-mismatch warning | "Did the thing I just changed make it faster or slower?" |
| `unendlich-launcher compare <spec> --versions @a,@b,...` | One spec, 2+ git refs | Same table, but runs each version first | "Has this metric been getting worse over the development arc?" |
| `unendlich-launcher run-suite` | A directory of specs | Per-spec PASS/FAIL summary | "Did anything regress across the full test set?" |
| `unendlich-launcher diag [<uuid>]` | Latest run (or specific UUID) | One-paste summary: spec/verdict/workload/zones/log tail | "What happened in that run?" — for sharing with collaborators. |
| `unendlich-analyze optimize <result>` | One result file | Ranked tables: where time goes, where tails hide | "Given this run, where should I focus optimization effort?" |
| `unendlich-analyze list` | `events.jsonl` (lots of runs) | One row per run | "Show me what runs I have on file." |
| `unendlich-analyze summarize <uuid>` | `events.jsonl` | Compact digest of one run's events | "I picked a run from `list` — tell me about it." |

**Mental shortcut:** the launcher is *about runs*. The analyzer is
*about results*. You usually want the launcher first; the analyzer
helps you read what the launcher produced.

## Sharing a run with a collaborator

c.13-instrumentation-followup added two ergonomic features that
together replace the old "scroll up in the terminal, copy paste,
attach result file" workflow:

1. **`~/.unendlich/latest-*` symlinks** updated after every run.
2. **`unendlich-launcher diag`** for one-paste summaries.

After any run, these symlinks exist:

```
~/.unendlich/latest-result.unendresult.json       → most recent result
~/.unendlich/latest-engine.log                    → most recent log
~/.unendlich/latest-events.jsonl                  → most recent events (if kept by triage)
~/.unendlich/latest-<spec_name>.unendresult.json  → per-spec latest
```

The per-spec symlink is important when running `run-suite` (multiple
specs in sequence) — the global `latest-*` only reflects the last
spec, but `latest-rd2_smoke.unendresult.json` always points at the
latest rd2_smoke run.

To get a one-paste run summary:

```bash
unendlich-launcher diag                      # uses latest
unendlich-launcher diag r-fd091c8a996c30db   # specific UUID
unendlich-launcher diag --log-lines 100      # default 60
```

Output: spec/version/duration/verdict, assertion verdicts, workload
context (loaded/rendering/snapshot/delta), top zones by mean ms,
tail of `engine.log`. Replaces hand-running `optimize` + `cat` +
`grep` + scrolling.

Engine stderr is captured to `engine.log` in each run directory
automatically — no need to use `2>&1 | tee` anymore.

---

## 2. Single-run diagnosis

**Use this when:** you saw something off in a smoke test, or you want
to understand the current state of perf at one specific commit.

```bash
# Step 1: run the spec.
./target/release/unendlich-launcher run tests/specs/rd2_smoke.unendtest --version @HEAD
```

Output ends with a "Run complete" box. Note the result path.

```bash
# Step 2: rank what dominates and what's volatile.
cargo run --release -p unendlich-analyze -- optimize \
    ~/.unendlich/runs/<run-uuid>/result.unendresult.json
```

You'll see two tables:

- **Where time is spent** — sorted by mean contribution. Top entries
  are the optimization targets.
- **Tail-latency volatility** — sorted by `p99/p50` ratio. Top entries
  have hidden spikes that may be dragging your tail even if median
  looks fine.

If both tables agree on the same zone (e.g. shadow is top of both),
that zone is *the* problem. If they disagree, you have two different
problems: one that's slow on average, one that's fine on average but
spiky.

---

## 3. A/B comparison

**Use this when:** you made a change and want to know if it helped.

The cleanest pattern: tag the baseline first, make your change,
compare.

```bash
# Step 1: tag the current state as baseline.
./commands/tag_current_version.sh   # creates v<current-cargo-version>

# Step 2: make your change. Bump Cargo.toml version (so the launcher
# treats it as a different commit), commit, tag.
# ... edit, commit, tag ...
./commands/tag_current_version.sh

# Step 3: compare.
./target/release/unendlich-launcher compare tests/specs/rd2_smoke.unendtest \
    --versions @v0.24.2-c.12-e,@v0.24.2-c.13-a
```

The launcher will run each version (or hit the cache if it already
has) and print the diff table.

**If you already have results on disk** and don't want to re-run:

```bash
./target/release/unendlich-launcher compare --results \
    ~/.unendlich/runs/r-baseline/result.unendresult.json \
    ~/.unendlich/runs/r-experiment/result.unendresult.json
```

The compare output shows every metric across columns, missing values
as `—`. The bottom row is `driver_hash`. **If that row shows
different hashes**, you crossed a Mesa or kernel-driver update
between the runs, and the perf delta might be the driver, not your
code. The tool will print an explicit warning when this happens.

---

## 4. Suite sweep

**Use this when:** you want to check the whole test set for
regressions, not just one spec.

```bash
./commands/launcher_smoke.sh --suite
```

Or directly:

```bash
./target/release/unendlich-launcher run-suite --version @HEAD
```

This runs every `.unendtest` under `tests/specs/` against the chosen
version. Output:

```
unendlich-launcher run-suite
  specs dir: /home/.../tests/specs
  version:   @HEAD

Commit: <40-hex>

  PASS  rd2_fifo_compat
  PASS  rd2_smoke
  FAIL  rd8_fifo_compat
  FAIL  rd8_immediate
  FAIL  rd8_low_upload
  FAIL  rd8_smoke

Total: 2 pass, 4 fail, 0 error
```

A failed spec doesn't stop the suite. Errors (couldn't build,
couldn't run) are reported separately from assertion failures.

The first invocation against a new version is slow (~60-90s of cargo
build the first time). Subsequent runs are instant from cache.

---

## 5. Reading the output

### The ranked-contribution table

```
cat      zone                mean_ms     p50_ms     p99_ms     max_ms   % of cat
cpu      wait_fences           2.955      1.600     49.534    235.856        77%
gpu      total                 2.740      1.311     41.259    223.600          —
gpu      shadow                1.577      0.666     26.435    143.080        58%
```

- `cat` is `gpu` or `cpu`.
- `% of cat` is the zone's mean as a fraction of total time in that
  category (excluding the `total` pseudo-zone, which is the
  denominator). A zone at 58% of GPU is doing more than half the
  GPU work.
- `mean_ms` is the headline number — total long-run cost.
- `p50_ms` is the typical frame. If much smaller than mean, the mean
  is being dragged up by rare spikes.
- `p99_ms` is the bad-but-not-catastrophic case. 1 in 100 frames is
  at least this slow.
- `max_ms` is the worst single frame seen. Treat as an indicator,
  not a number to optimize against — outliers happen.

**Useful comparison:** if `mean` ≈ `p50`, the workload is steady. If
`mean` >> `p50`, there are spikes hidden in the tail.

### The tail-latency table

```
cat      zone                 p50_ms     p99_ms    p99/p50
gpu      shadow                0.666     26.435      39.7×
cpu      wait_fences           1.600     49.534      31.0×
```

The `p99/p50` ratio is what you read first. Anything above ~5× has
non-trivial tail. Above 20× has serious tail.

The table filters to ratios ≥ 2× (otherwise it'd just list every
zone). Zones below that threshold are "steady" — their frames look
similar to each other.

`cpu wait_fences` tracking high here usually means **the CPU is
blocking on GPU**. It's a symptom, not a cause — look at the GPU
zone with the highest ratio for the real source.

### The driver_hash row in compare output

```
driver_hash        | drv-abc123 | drv-abc123 | drv-def456
                                                ⚠ driver epoch differs
```

The `driver_hash` is computed from vendor ID, device ID, driver
version raw, driver name, and driver info. **Same hash = same driver
state = comparable perf numbers.** Different hashes mean Mesa or
the kernel driver changed between runs, and any perf delta could be
the driver rather than your code.

If you see different hashes when comparing engine changes you didn't
expect to be driver-sensitive, re-run the older versions under the
current driver to make the comparison apples-to-apples. The launcher
caches binaries by commit hash but rebuilds happily when invalidated.

### The csm_capable row (c.13-scout+)

```
csm_capable        | —          | yes        | yes
```

Whether the driver supports `VK_KHR_multiview` with ≥4 layers — the
prerequisite for one-pass cascaded shadow maps. Shown when at least
one column has the info; omitted entirely when all columns are
pre-c.13.

The `—` is "result file predates the field." `yes`/`no` is the
probed answer. Capability differences across columns are
informational — unlike driver_hash mismatches, they don't trigger a
warning, because gaining capability across a driver upgrade is
expected.

This is what determined which c.17 strategy was viable.

### The triage verdict (in Run complete output)

```
│ triage:    healthy (5 steady windows under 33ms threshold)
```

Three possible verdicts:

- `healthy (...)` — every 1Hz steady-mode window had p50 below
  threshold (default 33ms = ~30fps). Events file deleted to save
  disk. The result file alone is enough.
- `INCIDENT — worst p50 was X.Yms > Z threshold` — at least one
  steady window dipped below threshold. Events file kept for
  forensic analysis.
- `kept (run too short to classify)` — fewer than 2 steady windows
  exist (run was <~5 seconds, or boot mode dominated). Events file
  kept by default; no judgment made.

Override the threshold with `UNENDLICH_TRIAGE_THRESHOLD_MS=20` for
stricter, or `=100` for laxer.

### The workload context section (c.13-instrumentation+)

`optimize` now prints a workload context block before the ranked
tables:

```
Workload context
────────────────
  loaded chunks      min=32 p50=33 max=34 (n=5 samples)
  rendering chunks   min=18 p50=24 max=24 (n=5 samples)
  start state         loaded_chunks=33 non_air_blocks=845620
  end state           loaded_chunks=34 non_air_blocks=867430
  delta               loaded_chunks=+1 non_air_blocks=+21810
```

This is the scale-of-the-problem context. Without it, "shadow_p50_ms
= 91ms" looks the same at rd=2 (24 chunks, 91ms is a disaster) and
rd=8 (200 chunks, 91ms might be expected). The workload context
makes the rd-distance / chunk-count / block-count visible alongside
timing data.

The `delta` row shows how much the world grew during measurement.
Large deltas indicate the workload was non-steady (lots of new
chunks loaded mid-measurement) — a confounder when comparing perf
across runs. Use `workload_blocks_delta` as an assertion key to
guard against this:

```toml
[expect]
workload_blocks_delta = { max = 1000 }   # world stable during measurement
```

### Warmup window (c.13-instrumentation+)

Set `warmup_ms` in a spec to delay the start of the measurement
window:

```toml
[engine]
duration_ms = 8000
warmup_ms = 3000
```

The engine runs for `warmup_ms` ms before the GPU+CPU timer lifetime
accumulators start gathering samples. During warmup: chunks generate,
pipelines compile, shadow caches fill. After warmup, those
perturbations don't contaminate the steady-state metrics.

For rd=8 workloads especially: a 3-second warmup makes the
`shadow_p50_ms` etc. dramatically more comparable across runs.

### Shadow bucket timing (c.13-instrumentation+)

Diagnostic-only. Set `UNENDLICH_SHADOW_BUCKETS=N` (typically 4 or 8)
to subdivide the shadow pass into N per-chunk-group buckets:

```bash
UNENDLICH_SHADOW_BUCKETS=4 \
    unendlich-launcher run tests/specs/rd8_smoke.unendtest --version @HEAD
```

This forces the per-chunk fallback path (overriding
`UNENDLICH_INDIRECT`) and writes GPU timestamps every N/chunks. Per
frame, a `shadow_buckets_timing` event records per-bucket
microseconds in `events.jsonl`.

Useful for diagnosing whether shadow cost is front-loaded (first
chunks dominate), uniformly distributed (workload is even), or
back-loaded (later chunks bigger / state grew).

**Don't compare bucket-mode timings to indirect-mode timings.** The
fallback path is slower; bucket mode is for understanding shadow
shape, not for measuring absolute perf.

---

## 6. Working with Claude during perf sessions

Practical notes for when you're doing perf work and want me to read
the data.

### What to paste back

**Most of the time, the `optimize` output is enough.** It's compact,
contains every diagnostic signal I need, and is human-readable. Copy
the whole thing — both tables and the summary — into chat. I'll read
the rankings, compare against earlier numbers, and propose next
steps.

For `compare` output: the table itself is the signal. Don't strip
the driver_hash row — that's how I notice if a Mesa update is
confounding the comparison.

For `run-suite`: the PASS/FAIL summary at the end is enough. If
something failed, then paste the `optimize` output for that
specific result file.

### What NOT to paste

- **Raw events.jsonl.** Hundreds of KB of structured logs. The
  `optimize` and `compare` commands already extract everything
  useful from these. Only send raw events if a tool fails to read a
  file, or if I specifically ask for them.
- **The "Run complete" box alone.** It only shows result paths and
  pass/fail. Useful for confirming a run finished, not for
  diagnosis.
- **Long stdout/stderr logs from the binary.** The structured zone
  summaries at shutdown contain the same information. If something
  panicked, paste the panic line + a few lines of context.

### What I can/can't see from each output

| What you paste | What I can tell | What I can't tell |
|---|---|---|
| `optimize` tables | Which zone dominates, where tails are, whether the workload is steady | What changed between two runs (need `compare` or two `optimize`s) |
| `compare` table | Whether a change improved or regressed perf, driver-mismatch | Detailed contribution within one run (need `optimize`) |
| `run-suite` summary | Whether anything regressed at all | Why anything regressed (need to drill into the failing spec) |
| Raw `events.jsonl` | Per-window detail, individual frame spikes in boot mode | (Slow to read — prefer the aggregated tools when they suffice) |
| `.unendresult.json` | Everything `optimize` shows, plus assertion details | (Same — `optimize` already extracts this) |

### A typical perf-session conversation

```
You:   "Here's the optimize output for rd8_smoke at HEAD: <paste>"
Me:    "Shadow dominates at 67% of GPU mean. Its p99/p50 is 92×, which
        explains the long tail. Suggest we try X."
You:   "Made the change, re-ran. <paste compare output>"
Me:    "Shadow p50 down 30%, p99 down 60%. Looks good. But cpu
        wait_fences went UP — let me check if we shifted the
        bottleneck. <analysis>"
```

Round-trip is short. We don't ship JSON files around. The CLI tools
do the extraction; chat carries the analysis.

### One useful instinct

If you're about to write "Claude, can you look at this run," do a
30-second `optimize` first. The output usually answers the question
you were about to ask, and if it doesn't, it focuses what you need
to ask. Fewer round-trips, better signal.

---

## Reference: where data lives

```
~/.unendlich/                              ← launcher state, NOT in repo
├── clones/<commit>/                       ← per-commit git clones
│   └── <full source tree>
├── versions/<commit>/                     ← cached binaries (persistent)
│   └── unendlich-world
└── runs/<run-uuid>/                       ← per-invocation outputs
    ├── result.unendresult.json            ← always written
    ├── events.jsonl                       ← kept if incident, deleted if healthy
    ├── incident_summary.json              ← always written (records triage verdict)
    └── world/                              ← per-run hermetic world state
```

The `versions/` cache is the persistent and important part. `clones/`
and `runs/` are scratch — they can be deleted at any time with no
data loss (the result.unendresult.json file is the only durable
artifact, and you should commit any you care about to a separate
location).

Free up disk:
```bash
rm -rf ~/.unendlich/clones/*               # forces fresh re-clone next run
rm -rf ~/.unendlich/runs/*                 # discards all run outputs
# Keep ~/.unendlich/versions/ unless you really want to rebuild everything.
```
