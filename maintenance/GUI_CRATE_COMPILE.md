# Why the last crate is slow, and it is not the dependency graph

You noticed the *final* crate in a GUI app dominating rather than its
dependencies, and that it happens in the apps with big main loops. That is a
sharper diagnosis than the one `--timings` gave me, and it points at a mechanism
that profile does not show.

## The mechanism

`codegen-units` splits a crate's codegen **by function**, and a function is
never split across units. So:

* **One 4,000-line function is one codegen unit.** Raising `codegen-units` does
  nothing for it — there is nothing to divide.
* LLVM's optimisation time is **superlinear in function size**: ten times longer
  costs more than ten times as much.
* Incremental compilation invalidates at codegen-unit granularity, so touching
  one line inside that function recompiles all of it.

The frontend — parse, type-check, borrow-check — is single-threaded per crate on
top of that. Both effects land on the same crate: the app's own, which is the
last one built.

## Measured: the function census

Largest single functions across the GUI crates:

```
4503 lines  spatial_ui       logscope-egui/src/tabs/chat.rs
 493 lines  generate         logscope-egui/src/backend.rs
 488 lines  update           voiceterm-egui/src/bin/voiceterm_egui.rs
 449 lines  atlantis         logscope-egui/src/scene3d.rs
 427 lines  load             logscope-egui/src/backend.rs
```

**`spatial_ui` is nine times the next largest** — one codegen unit that cannot
be parallelised or incrementally narrowed. That is the shape your eyeballing
found.

## The seams already exist

`spatial_ui` carries **fourteen labelled sections** — `── CINEMATIC DIRECTOR`,
`── PHYSICAL CONTROL DOCK`, `── IN-SCENE COMPOSER`, `── MESSAGE READING BOX` and
so on. The author already drew the boundaries and left them as comments.

Extraction is mechanical in shape but not trivial in practice: the body is
closure-nested with **46 `self.` references in one section alone**, so each cut
is a borrow-checking exercise rather than a copy and paste.

## What I could not measure here

This container has **`nproc` = 1**, and a clean `logscope-egui` build did not
finish in the time available — 326 dependency crates, then `ashpd` alone still
going. So there is **no before/after number for the extraction**, and inventing
one would be worse than having none.

The measurement worth taking on real hardware, in this order:

```bash
# 1. What does touching the big function cost?
cargo build -p logscope-egui                                   # warm first
touch apps/logscope/crates/logscope-egui/src/tabs/chat.rs
time cargo build -p logscope-egui

# 2. Same for a small file in the same crate, as a control
touch apps/logscope/crates/logscope-egui/src/app.rs
time cargo build -p logscope-egui

# 3. Where the time goes inside the crate (nightly)
cargo rustc -p logscope-egui -- -Z self-profile
```

If **(1) is much worse than (2)**, the codegen-unit theory holds for this crate
and the extraction earns its borrow-checker work. If they are **close**, the cost
is the single-threaded frontend over the whole crate, and the answer is
splitting the *crate* rather than the function.

Those are different refactors, which is exactly why the number comes first.
