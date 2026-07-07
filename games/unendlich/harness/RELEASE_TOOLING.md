# Release tooling — delta updates (FUTURE / launcher)

> Stored so a later version of this project can reuse the dev-time snapshot diff
> machinery as the basis for **shipped delta updates**. Not wired into anything
> yet — this is a forward note.

## What exists today (dev-time)
`harness/snapshot_diff.sh <base> <new> [out.patch]` produces a unified,
`git apply`-able patch between any two builds (zip or dir). Combined with
git-storing each build, `git diff <prev> <cur>` is already a full delta between
versions. This is how changes move between divergent trees during development.

## The release idea
The same base→new delta mechanism generalizes to player-facing updates: ship a
small patch from version N to N+1 instead of a full re-download. The launcher
computes/applies the delta the way `snapshot_diff.sh` + `git apply --reject`
does here, with integrity checks and a full-zip fallback.

## TODO — official game launcher
- [ ] Add a delta-update path to the **official game launcher**: detect installed
      version, fetch the N→N+1 patch, verify, apply, fall back to full download
      on mismatch. Reuse the snapshot-diff approach proven in `harness/`.
- [ ] Version stamping so the launcher knows base/new (the engine already prints
      a crate version, e.g. `unendlich-engine v0.24.x` — surface it to the
      launcher as the update base).
- [ ] Keep the dev `harness/snapshot_diff.sh` and the launcher's delta logic in
      sync / share code where practical.

(When you slow down from speed-mode and start tuning, this is the doc to revisit.)
