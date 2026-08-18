# The animal assets: what ships, what went away, and how to get it back

`assets/animals.web` is the game's model data now. The seventeen source `.iqm`
files it was built from were removed from the working tree in the commit that
added this file.

## What was removed

| | |
|---|---|
| `assets/animals/*.iqm` | 17 files, 18,993,376 bytes |
| replaced by `assets/animals.web` | 3,741,369 bytes (2.97 MB as a git object) |
| the same data zstd -19 | 2,811,122 bytes, kept out of the tree (see below) |

The pack carries exactly what the loader reads: positions and triangle indices,
quantised to 14 bits against each model's own bounding box. It does not carry
texcoords, normals, tangents, blend weights, joints, animation frames or the
IQM text table, because `iqm.rs` never read any of them — clawbound is
flat-shaded papercraft and derives facet normals from triangle winding.

## This is lossy, and in one direction

14-bit quantisation moves every vertex by up to 0.0031% of its model's own
span — a tenth of a millimetre on a three-unit statue, finer than a facet can
show. But it is **not reversible**. Re-packing at 16 bits, or with a different
encoder, needs the original IQMs; the web cannot be up-sampled back to them.
Everything below exists so that door stays open.

## Where the originals are

They remain in git history, at every commit up to and including the one before
this file was added. To get them back into a working tree:

```sh
git show <commit-before-removal>:games/clawbound/assets/animals -- | cat   # list
git checkout <commit-before-removal> -- games/clawbound/assets/animals     # restore
```

With them restored, `cargo run --release --example pack_animals` re-packs, and
`cargo test --test assetpack_real` re-runs the fidelity checks against the
source. Both are written to skip or explain rather than fail when the originals
are absent, which is their normal state.

**Keep a copy outside git as well.** History is the wrong place to depend on
for source art, precisely because the section below is about deleting it from
history.

## Reclaiming the 19 MB from history (optional, destructive, and separate)

Removing files from the working tree does not shrink a clone: every past commit
still references the blobs, so `git clone` still moves 19 MB of IQM. Reclaiming
that space means rewriting history:

```sh
# from the repository root, on a fresh clone, with every branch pushed somewhere safe
git filter-repo --invert-paths --path games/clawbound/assets/animals/
git reflog expire --expire=now --all && git gc --prune=now --aggressive
```

Three things to be clear about before running it:

1. **It rewrites every commit hash from the first one that touched those files.**
   Anyone with a clone has to re-clone; any bundle or patch built against the
   old hashes no longer applies. This is why the removal itself is an ordinary
   commit and the rewrite is not — a normal commit can be delivered as a patch
   on top of what you already have, and a rewrite cannot.
2. **It deletes the only in-repo copy of the source art.** Do step "keep a copy
   outside git" first, and verify the copy opens, before running this.
3. It is worth doing only if clone size actually matters to you. The working
   tree is already 19 MB lighter without it; the rewrite buys the same 19 MB
   off every historical clone.

## Rebuilding the pack

```sh
cargo run --release --example pack_animals          # writes assets/animals.web (+ .zst)
cargo run --release --example pack_verify           # round-trips it and times the load
```

`assets/animals.web.zst` is deliberately *not* committed. It is 0.93 MB smaller
on disk, but as a git object the raw web costs 2.97 MB against the container's
2.81 MB — 155 KB apart — while the container costs ~37 ms of pure-Rust zstd on
first touch against the raw web's ~4 ms. For a file that lives in the repo and
is read at startup, that trade goes the other way; the `.zst` is for shipping
loose files, and the packer still writes it.
