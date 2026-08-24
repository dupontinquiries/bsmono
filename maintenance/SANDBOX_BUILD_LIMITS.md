# Why builds die silently here

**3 GB RAM, 1 CPU.** That is the constraint, and it presents as anything but.

## The symptom is never "out of memory"

Over this session the same underlying failure appeared as:

* `error: linking with 'cc' failed` + *"PLEASE submit a bug report to
  https://github.com/llvm/llvm-project/issues"* — an LLD crash backtrace.
* A build sitting on one dependency (`smithay-clipboard`, `aho-corasick`,
  `icu_normalizer`, `egui`) for fifteen minutes with **no process left running**.
* `failed to build archive: No space left on device`.

Only the third names a real cause, and it is the one that misled: clearing disk
fixed *that* instance, which made "the disk is full" look like the general
answer. It is not. A workspace build died at **5.2 GB free**.

Parallel `rustc` processes exceed 3 GB together and are killed. Nothing prints.

## What works

```
CARGO_BUILD_JOBS=1 cargo test -p <one-crate>
```

One crate at a time, one job. Verified against `apps/aperture`: the workspace
build died twice, and the same tree built and tested crate-by-crate with no
errors at all — 107 tests across four crates.

`cargo test --workspace` on a normal machine is fine. This is a sandbox
limitation, not a property of the code.

## Also worth doing

Target directories accumulate. Three left over from earlier work
(`/tmp/snapclone`, `/tmp/uk`, `/tmp/ut`) held **2.7 GB** between them. Removing a
`CARGO_TARGET_DIR` after its commit is cheap; leaving it turns the next build
into a disk failure on top of a memory one, and then the diagnosis has two
causes tangled together — which is exactly how this went.

## cargo-ailite: the profile is a disk lever, measured

`maintenance/tools/cargo-ailite` wraps cargo with a throwaway `ailite` profile
(inherits dev, `debug = 0`, `strip = "debuginfo"`, `incremental = false`),
`CARGO_BUILD_JOBS=1`, and `CARGO_INCREMENTAL=0`. Measured here on
`shared/crypto-kit`, separate target dirs so neither run warmed the other:

| | cold | target size | warm no-op |
|---|---:|---:|---:|
| `cargo check` | 10.6 s | **71 MB** | 40 ms |
| `cargo ailite check` | 9.7 s | **41 MB** | 37 ms |

**The 42% is the point, not the 8%.** Disk is what actually fails in this
sandbox: this session hit 100% twice, once mid-build, and the symptom was the
`failed to build archive` message this file already warns about. Halving every
target directory is worth more than the wall-clock difference.

One real trade-off, also measured: after touching a source file, plain check
re-ran in 77 ms and ailite in 119 ms, because `incremental = false`. Plain's
target also grew 71 → 73 MB from the incremental data while ailite's stayed at
41 MB. For a tight edit-check loop set `AILITE_INCREMENTAL=1`; for
build-once-and-move-on, which is most agent work here, the default is right.

## ailite-cache: the fix for target dirs dying with the sandbox

Registry dependencies are never mtime-checked by cargo, and only
workspace-local sources are — so if local sources carry the same fixed
timestamp when the cache is built and when it is restored, nothing recompiles.
`maintenance/tools/ailite-cache` does the pinning and the tar.

Verified by destroying the artifact directory AND re-stamping every source file
to `now`, which is what a reset does:

```
cold build into a fresh dir     8830 ms
restore + re-pin, then check      39 ms   0 units compiled
```

Restore must land at the SAME absolute path the cache was built at —
fingerprints embed paths.

## What did NOT reproduce: gate's fail-fast ordering

`cargo ailite gate` checks only the crates git reports as changed, serially,
stopping at the first failure. The claim is that cargo's scheduler favours
crates with many dependents, so a broken local leaf gets compiled last.

On a WARM tree it makes no difference. Measured on `apps/studio` (14 members)
with a real `compile_error!` injected into `studio-fx`:

```
full workspace check   340 ms
gate (git-detected)    307 ms
gate, explicit order   295 ms
```

All three are the same number. Cargo's freshness pass reaches the broken crate
almost immediately when everything else is already built, so there is no late
error to avoid. Gate earns its keep when a meaningful part of the graph is
COLD — which, paired with `ailite-cache restore`, is the case this sandbox
should rarely be in. Use it, but do not expect the warm loop to get faster.

`patches/cargo-ailite-schedule-first.patch` is the within-invocation version of
the same idea (`CARGO_SCHEDULE_FIRST` boosts named packages' unit cost, and
cargo's existing priority formula propagates the boost down the dependency
chain). It is unbuilt — it needs a cargo fork — and given the warm-tree result
above there is no measured reason to carry it here.

## apt works for some "missing" native deps — after an update

The baked package lists are stale: a bare `apt-get install` 404s on pool
URLs. `apt-get update` first, then `libgstreamer1.0-dev
libgstreamer-plugins-base1.0-dev` installed cleanly (c11; unblocked
camera-bridge compilation). Before adding a feature-gate escape for a new
native dep (CLAUDE.md §3), spend the thirty seconds checking whether apt has
it. GTK4/libadwaita/Qt6 remain absent from what has been tried.
