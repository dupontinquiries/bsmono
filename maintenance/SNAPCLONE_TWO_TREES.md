# snapclone exists twice, and the copies have diverged

Found while auditing duplicate content: **623 files are byte-identical between
`apps/snapclone` and `system/snapclone`**, the largest structural duplication in
the repository. Both are registered in the manifest —
`[system.snapclone]` as the system-service half, `[apps.snapclone-egui]` at
`apps/snapclone/egui-frontend`.

## They are no longer the same

```
apps/snapclone    733 tracked files   last touched 2026-08-08
system/snapclone  669 tracked files   last touched 2026-06-27
62 differences
```

The differences concentrate exactly where the porting work reads from:

| area | differing files |
|---|---:|
| `src/ui` | 12 |
| `include/ui` | 8 |
| `src/core` | 3 |
| `rust/snapclone-dht/src` | 3 |

**`apps/` leads.** Sampled `src/ui/ContactsPage.cpp`: 1,854 lines against 1,847,
and the extra block is a `V32.49+` addition (`subTabIcon`, the pull-pill icon)
present in `apps/` and absent from `system/`. `system/` additionally carries a
`log.txt` and a `third_party` directory that `apps/` does not.

## Why this is not a deletion

The obvious cleanup — delete one — is wrong here for two reasons:

1. **It saves nothing.** Git already stores the 623 identical blobs once. This
   is the same trap that made three earlier "cleanups" in this work yield
   almost no bytes: duplication across paths is free in the pack.
2. **The trees are not interchangeable.** 62 files differ, and neither is a
   superset — `apps/` has newer UI code, `system/` has `third_party`. Deleting
   either loses something, so it is a merge question rather than a cleanup one,
   and merging two Qt trees is not a decision to take from a size audit.

## What is actually at risk

**Silent divergence.** Six weeks of edits landed in one copy only, and nothing
in the repo notices. The porting work reads Qt sources from `apps/snapclone`; if
someone fixes a bug in `system/snapclone/src/ui`, the port will never see it and
the parity audit will not flag it, because the audit reads one tree.

The cheap mitigation is knowing which is canonical, which is what this document
records: **`apps/snapclone` is the current Qt source; `system/snapclone` is the
older service-side copy.** The expensive one is reconciling them, which needs
someone who knows why the split exists.
