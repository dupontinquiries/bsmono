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
