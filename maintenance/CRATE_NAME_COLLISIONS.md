# 21 crate names live at two paths

Found while chasing "some apps still don't show in gitmind". They **do** show —
the problem is the opposite of missing.

## The mechanism

`repo_map::scan_repo` ends with:

```rust
out.dedup_by(|a, b| a.name == b.name && a.path == b.path);
```

Both fields, so two crates sharing a NAME at different PATHS are kept — correctly,
since they are different crates. But the cockpit lists by name, so the pair reads
as one entry appearing twice, or as the wrong one having won. Nothing is absent;
the list is ambiguous.

`EXCLUDED_ROOTS` only helps at the top level (`vendor/`), and these collisions
are nested — `games/blackstar-rs/vendor/netcode` is not `vendor/netcode`.

## The three shapes, and they need different answers

**1. Vendored copies.** `games/blackstar-rs/vendor/{netcode,touchkit,snapcore}`
duplicate `shared/`. Tracked in `VENDORED_DRIFT.md`. These should be *excluded*
from the build matrix — building a copy no app links reports its failures as the
real crate's.

**2. `apps/` versus `system/`.** `snapclone-firewall`, `snapclone-menu`,
`snapclone-network-diag` and others exist under both. One of these is the live
one; the other is a stale move. **This needs a decision, not a rule** — and it is
the most likely cause of "I built it and nothing changed".

**3. Genuine independent crates.** `peek` at `apps/gitmind/crates/peek` and
`tools/peek`; `av1tuner_core` in `av1tuner` and `av1tuner-m1`. These are
different things that happen to share a name, and renaming one is the fix.

## Why blackstar looks fragmented

It is two projects, not one fragmented one:

* `games/blackstar` — the **C engine** plus its Rust tooling (assetkit,
  contentlint, enginecore, the model tools). 10 crates.
* `games/blackstar-rs` — the **Rust port** (`skydribble`) plus three vendored
  crates. 4 crates.

That split is deliberate. The vendored three are the drift risk, not the split.

## What to do

`EXCLUDED_ROOTS` should match a path SEGMENT rather than a top-level name, so a
nested `vendor/` is excluded wherever it appears. That fixes shape 1 mechanically
and shrinks the list by three.

Shapes 2 and 3 need someone to say which copy is real. A tool cannot guess that,
and guessing is how the wrong one gets built for a month.
