# What can be removed losslessly — audited, mostly "no"

The bundle is ~635MB against a 500MB target. The web is measured at 55MB
replacing 512MB of baked models, but at 0.826 silhouette IoU that swap is
**lossy**, so this is the separate question: what can go without losing
anything?

## Packed weight, from `git filter-repo --analyze`

| directory | packed |
|---|---:|
| `games/blackstar/engine_src/data` | 302 MB |
| `games/blackstar-rs/assets/bots` | 172 MB |
| `games/cybertron/engine_src/data` | 82 MB |

Everything else is noise by comparison — the whole of `apps/` is 86MB packed.

## Candidates examined

**`blackstar-rs/assets/bots` (172MB packed, 349MB on disk) — NOT removable.**
This looked like the answer. `tools/export-roster.py` generates it, reading
`imported_*.iqm` from `DATA = blackstar/engine_src/data` — a corpus we are
keeping anyway, so the inputs survive. But the exporter shells out to
`FV = blackstar/rust/foldview`, and **`games/blackstar/rust/` contains
assetkit, contentlint and enginecore and no foldview**. The generator is not in
the tree, so the output cannot be regenerated from it. Removing these would be a
loss, not a cleanup.

Same shape as the `.iqm` mistake earlier in this work: "derived" is only safe
when the deriver is present and proven to run.

**Static libraries (16MB) — not removable.** `libSDL2.a` and friends under
`engine_src/lib/win` are prebuilt Windows binaries. Regenerating them needs the
SDL2 source and a Windows toolchain, neither of which is here.

**`.spv` shaders (550KB) — removable in principle**, sources are alongside, but
the amount is not worth a build-step dependency.

**1,111 duplicate blobs — no win.** Identical content at multiple paths. Git
already stores one copy in the pack, so deleting the duplicates changes the
checkout and not the bundle.

**Tracked build directory — removed.** `gyroball/.../libiio/_build/` held 7
files including two `.a`. Genuinely regenerable by building libiio. ~350KB.

## Conclusion

**There is no large lossless win left.** The repo is big because it holds three
model corpora and their textures, and all three are content. The 500MB target is
reachable only through the web — and only once its fidelity is good enough that
replacing the baked models is not a loss.

That makes the remaining work clear: **improving reconstruction accuracy IS the
repo-size work.** They stopped being separate tasks at 0.826.

---

# Second pass — three more removals, 6MB, and why so little

Pushed harder rather than accepting the first pass's conclusion. Three things
came out, all genuinely lossless:

**1,000 catalog `.iqm` (16MB).** The earlier attempt at this deletion was wrong
because the engine loads `data/*.iqm`. The catalog is different, checked three
ways: `engine_src` never mentions `catalog` at all; `bot_generate.py` and
`detail_apply.py` only ever *write* .iqm into `data/`, never read the catalog's;
and `obj_to_iqm.py` regenerates them exactly, proven on all 1,000 with the real
`INTERQUAKEMODEL\0` header. The rebuild command is in `catalog/README.md`.

**`apps/unendlich` (49MB packed) and `apps/cybertron`.** Both moved to `games/`
in July. The tree has not carried them since; history still did.

**`gyroball/.../libiio/_build`.** A tracked build directory.

## The result, and the lesson repeated

Bundle **635MB → 629MB**. Six megabytes for 65MB of "packed size" removed.

This is the third time in this work that `filter-repo --analyze`'s packed column
has overpromised, and the reason is the same each time: **it reports per-blob
compression, and git's pack already delta-compresses near-identical blobs across
the repository.** `apps/unendlich` was nearly free because `games/unendlich`
holds the same content; the duplicate catalog `.iqm` were cheap for the same
reason.

The corollary is worth stating once: **in a repo where content has been moved
rather than changed, deleting the old copy saves almost nothing.** Only removing
content that exists *nowhere else* moves the number.

## Which leaves exactly one lever

The three corpora are unique content — `blackstar/engine_src/data` 302MB,
`blackstar-rs/assets/bots` 172MB, `cybertron/engine_src/data` 82MB. Nothing
duplicates them, so removing them would move the number, and nothing regenerates
them from what is in the tree.

So the 500MB target still runs through the web, and the web still needs its
0.826 to become "indistinguishable" before that swap is a cleanup rather than a
loss. **Lossless cleanup is now exhausted at single-digit megabytes per pass.**

---

# Third pass — the pack is already optimal

The one canonical lossless lever left was a full delta-search repack, skipped
earlier because it timed out:

```
git repack -a -d -f --window=250 --depth=250
```

Default git packs with `--window=10 --depth=50`. Twenty-five times the search
window, run to completion over the whole repository:

```
before : 629.85 MiB
after  : 629.85 MiB
```

**No change.** Which is itself the answer: the pack is dominated by `.iqm`,
`.png` and `.bin`, all already-compressed or high-entropy binary. Delta
compression has nothing left to find between them, so no packing parameter
helps.

Also fixed here: `refs/remotes/origin/HEAD` pointed at a null sha1 after the
history rewrites, and would have been inherited by every clone from these
bundles. `git fsck --no-dangling` is now clean.

## The lossless question is settled

Four passes, measured each time:

| pass | action | saved |
|---|---|---:|
| 1 | untracked `target/` dirs (disk only, not in bundle) | 0 MB |
| 1 | audit: SDL2 `.a`, `.spv`, duplicate blobs — all rejected | 0 MB |
| 2 | catalog `.iqm`, `apps/unendlich`, `apps/cybertron`, `_build` | 6 MB |
| 3 | maximum-effort repack | 0 MB |

**629MB, and every remaining megabyte is unique content that nothing in the tree
can regenerate.** There is no lossless path to 500MB. The 129MB gap is exactly
the size of the model corpora, and the only thing that removes them without loss
is a reconstruction good enough to replace them.
