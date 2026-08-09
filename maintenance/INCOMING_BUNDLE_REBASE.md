# simscope-full.bundle could not be applied — and the cause is ours

## What happened

The bundle is incremental and names one prerequisite:

```
-e71cc7c3a4590bc1886894ae263d3e41eb242d15
   snapclone-dht: native OpenDHT port — sans-io Kademlia engine,
   wire codec, simulation suite
```

That commit is **in this repository**, as `5f1e4e5f`, dated 2026-07-28. Same
work, same subject, different hash.

**The history rewrites renumbered it.** Five `filter-repo` passes this session —
catalog `.iqm`, `apps/unendlich`, `apps/cybertron`, the textures, and the 512MB
model corpus — each rewrote every commit that followed. Any bundle built against
a pre-rewrite hash now names a commit that no longer exists under that name.

This was the known cost of the purge, and here is its first concrete bill.

## What can and cannot be salvaged

The pack extracts and 696 of its 777 objects index cleanly, but **81 are deltas
against base objects that live in the missing history**, so `git index-pack`
stops with "pack has 81 unresolved deltas". The content is not recoverable from
this file alone — not partially, not with effort. It needs its base.

## What to send instead

A self-contained bundle, built with no implied base:

```bash
git bundle create simscope-full.bundle --all
```

`--all` with a clean working repo produces a bundle whose verify says *"records a
complete history"* and which needs no prerequisite. That is what the bundles
going the other way from here have always been, which is why they keep applying.

If the work is small, a patch series is even simpler and is immune to this
entirely:

```bash
git format-patch 5f1e4e5f..main --stdout > simscope.patch
```

## The standing rule

**After a history rewrite, every outstanding incremental bundle against this
repo is dead.** Not degraded — dead, because the prerequisite hash cannot be
found and git will not guess. Anyone holding a clone from before the rewrite
should re-clone from the current bundle rather than try to fetch into the old
one.

The rewrites were worth it — 629.9MB to 464.7MB, under budget — but the cost is
paid in exactly this coin, and it will be paid again by anything else in flight.
