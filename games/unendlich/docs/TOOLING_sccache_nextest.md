# Build & test tooling — sccache + cargo-nextest (coordinators & workers)

> For **in-sandbox** compile-time wins (mold/lld linker, `prefer-dynamic`,
> split-debuginfo) that work per-session and need no persistent machine, see
> `TOOLING_fast_linking.md`. Those speed linking; the two docs stack.

Two speedups, both for the *persistent* coordinator/worker machines (in the build
sandbox the prebuilt installers aren't network-allowlisted and source builds exceed the
per-turn budget, and the FS resets — so install these on your real boxes).

## 1. sccache — compile cache (shared across workers = the big win)

Each worker chat gets a FRESH copy of the same base tree, so they recompile the SAME
dependency graph from scratch. A shared sccache means worker #2..#N reuse worker #1's
compiled crates — first build pays, the rest are cache hits.

Install (persistent machine):
```
cargo install sccache --locked          # or: brew install sccache  /  prebuilt release
```
Enable for every cargo invocation (coordinator AND workers):
```
export RUSTC_WRAPPER=sccache
export SCCACHE_DIR="$HOME/.cache/sccache"      # point ALL workers at ONE shared dir
export SCCACHE_CACHE_SIZE="20G"
sccache --start-server
# sanity: sccache --show-stats   (watch "Cache hits" climb across worker builds)
```
Notes:
- Keep `CARGO_INCREMENTAL=0` (we already set it) — sccache and incremental fight; with a
  cache, incremental off is correct.
- The v4 lock-dance is unchanged: sccache caches compilation, not the lockfile step.
- Workers on the SAME machine should share `SCCACHE_DIR`. Across machines, point it at a
  shared/network path or use the s3/redis backend (`SCCACHE_REDIS=...`).
- Expected: the ~45s engine `cargo check` and ~2m bin builds drop to seconds on a warm
  cache when only one module changed.

## 2. cargo-nextest — faster test runs + a filterset selection DSL

nextest runs a process per test with much better parallelism than `cargo test`, and it
takes a Bazel/revset-style **filterset** so we can target exactly the tests a change
touches instead of the whole suite.

Install:
```
cargo install cargo-nextest --locked     # or prebuilt: curl -LsSf https://get.nexte.st/latest/<plat>
```
Coordinator gate (the green run = only the 4 known pre-existing failures):
```
cargo nextest run -E 'package(unendlich-engine)'                    # full engine, parallel
cargo nextest run -E 'test(dragon_) + test(mob_) + test(forge)'     # just the dragon/rig work
cargo nextest list  -E 'package(unendlich-engine)' --message-format json   # machine-readable
```
**Convention going forward:** dispatch briefs and the coordinator emit a **filterset**, not a
`cargo test <name>` line. e.g. a worker touching `sim::river_flow` reports
`-E 'test(river_flow)'`. The coordinator runs `-E 'package(unendlich-engine)'` for the gate.

CAVEAT: nextest does NOT run doctests. Keep those on the side:
```
cargo test --doc -p unendlich-engine
```
The harness (`harness/run.sh`) can call `cargo nextest run -E 'package(unendlich-engine)'`
for the lib gate and `cargo test --doc` separately; the HARNESS GREEN / GPU GATE GREEN
semantics are unchanged. The 4 known failures (loot eye-amber, mob blind, shader_artifacts
build-list, smoke_gpu pure-decay) are still the only allowed reds.

## Why not in the build sandbox
- `get.nexte.st` / sccache release hosts aren't in the network allowlist (only
  crates.io/github/pypi/npm), so prebuilt fetch fails.
- `cargo install` from crates.io compiles from source and exceeds the per-command time
  budget; the sandbox FS also resets between tasks, so it wouldn't persist.
- Net: install on the real coordinator/worker hosts; the sandbox keeps using plain
  `cargo` + the lock-dance.
