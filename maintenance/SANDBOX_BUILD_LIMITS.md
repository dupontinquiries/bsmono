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
