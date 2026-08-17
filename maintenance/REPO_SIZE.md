# Repo size: what is free, what needs a rewrite, and what to decide first

Measured on 2026-08-17, on a clone with the launcher, blackstar and studio-trap
branches fetched. Numbers are **packed** bytes from `git verify-pack -v`, not
working-tree sizes — those differ enormously for already-compressed files, and
using the wrong one is how a cleanup plan promises savings it cannot deliver.

## 1. `git gc` — 966 MiB to 551 MiB, free

The biggest single number here, and it costs nothing:

```
before   966.11 MiB   6 packs
after    551.29 MiB   1 pack
```

No history rewritten, no objects lost. That was duplication from fetching four
branches and merging two of them in one session: each fetch writes its own
pack, loose objects accumulate, and nothing consolidates them until a `gc`.

**Any clone that has been fetched into repeatedly is carrying some version of
this.** Run `git gc` before concluding anything about size, and run it again
after the rewrite below, because a rewrite leaves the old objects behind until
it is collected.

## 2. The rewrite candidates, measured

All versions across all history, packed:

| path | packed | why it can go |
|---|---:|---|
| `games/blackstar/catalog/parts_web*.bin.gz` | 42.5 MB | superseded by the `.zst`, payload proven identical |
| `apps/logscope/logscope-linux-x64` | 6.3 MB | build artifact, `cross` rebuilds it |
| `games/blackstar-rs/assets/bots/*/f0?/scene.bin` | 11.2 MB | superseded by `bots.pack`, verified at 0.00076% error |
| **total** | **60.0 MB** | |

That is larger than a working-tree estimate suggests, because history holds
multiple revisions and **already-compressed blobs do not delta against each
other** — `parts_web_full.bin.gz` alone accounts for 34.8 MB across its
revisions, each costing nearly its full size.

`551 − 60 ≈ 491 MB`, so a rewrite of these three lands under 500 MB.

### What must NOT be removed

* `games/blackstar-rs/assets/bots/*/w0?/scene.bin` — 4.4 MB, and the only copy.
  The packer refuses walk strips (`pose float counts differ`: poses in a walk
  cycle have different vertex counts, so one shared quantisation box cannot
  serve them and `xform::Morph`'s vertex-i-to-vertex-i lerp does not hold).
  There is no `walk.pack` to fall back to and the loader names the panic.
* The fold poses themselves. They were baked — all 1,454 pose glTFs were
  scanned and none carries `skins` or `animations` — and the rigged `.iqm`
  sources named in each `chassis.json` are **not in this repo**. `src/rig.rs`
  and `examples/rigpose.rs` can compose poses from a rig at runtime, but not
  without the rig. Until those sources are found, the packs are archive, not
  cache.

## 3. Why not to do it yet

**The generator is still adding incompressible blobs.** The `.zst` catalogs
just committed are 35.5 MB, and every rebuild adds ~31 MB permanently, exactly
as the `.gz` files did. Reclaiming 60 MB once against a generator adding 31 MB
per rebuild is not a durable win. The durable fix is deciding that generated
catalogs do not belong in git — LFS, a release artefact, or a build step — and
that decision should be made **before** the rewrite, so the rewrite can drop
the whole class rather than today's instances.

**A rewrite invalidates every commit hash from the earliest touched commit
forward.** In flight right now: `incoming/launcher` (the presentation work
still to port into hatch), and three studio-trap refs. Rewriting now means
rebasing all of them onto new history. The natural moment is after the launcher
port lands and the trap branch is merged or dropped.

## 4. The order that makes sense

1. `git gc` everywhere. Free, immediate, no risk. **Do this now.**
2. Decide whether generated catalogs (`parts_web*.bin.*`) stay tracked. This is
   the decision that determines whether step 4 is worth doing once or forever.
3. Land the launcher port; merge or drop the trap branches. Get the in-flight
   work to a point where rebasing is not needed.
4. One BFG or `git filter-repo` pass removing the three paths above, plus
   whatever class step 2 identified. Then `git gc --prune=now`.
5. Everyone re-clones. That is the actual cost of step 4 and the reason to do
   it once.

## 5. Where the remaining weight is

After the three candidates, by project (packed, all history):

```
263 MB  games/blackstar-rs      bots.pack (216 MB), pose glTF, car .glb
 86 MB  games/blackstar         catalogs, representatives
 57 MB  games/cybertron         textures (stuff.png 5.4, norm_stuff.png 5.1)
 46 MB  games/unendlich         music stems, art targets
 13 MB  apps/bo-suite
 10 MB  games/clawbound         .iqm animals
```

`blackstar-rs` is half the repo and most of it is `bots.pack` — which is
already the compressed form, and archive rather than cache until the `.iqm`
sources turn up. If they do, that 216 MB becomes regenerable and the arithmetic
changes completely. That is the one question worth answering before any of
this: **where are the rigged sources?**
