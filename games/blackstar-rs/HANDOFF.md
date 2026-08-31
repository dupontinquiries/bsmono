# Handoff — blackstar-rs session bundle

For whichever Claude picks this up next to merge/review. Read this first;
`TODO.md` is the full backlog (fixed/verified, found-not-fixed with exact
repro, design proposals) — this is just the map to it, plus the workflow
things worth knowing before touching anything.

## This session: 4 commits, all scoped, all verified against the full suite

```
2789b4c6 TODO: two investigations, no code changes -- orientsim guard is moot, boat crotch bug is real
2d73c035 chassis: a per-boss flag to opt out of the vehicle<->robot fold
929586bc turret: detect the barrel and its elevation axis, don't wire pitch yet
7127aa2f skeleton: a prim can be centroid-central and still reach outward
```

Each one picked from `TODO.md`'s existing backlog rather than something
newly discovered — the prior session had already done the harder work of
finding and precisely describing these gaps. Same two-part verification
as before, repeated after every commit this session:

1. `git status --short --untracked-files=all` **from the repo root**, not
   just `games/blackstar-rs/`. Checked after each of the 4 commits; came
   back empty every time (except the one file just committed). Keep doing
   this — see the previous version of this file, and `apps/aperture`'s
   own `HANDOFF.md`, for what it costs to skip it.
2. `git diff --stat -- <files>` before every commit, to confirm only the
   intended files were staged. Also full `cargo test --no-default-features
   --bin skydribble` after every code change (not just at the end) —
   catching a regression at commit N rather than commit N+3 is cheaper
   to diagnose. Full suite is 1264/1264 as of the last commit (was
   1261 at the start of this session; +3 new tests, 0 removed, 0 skipped).

**Correction to this session's own second commit** (`929586bc`): its
message claims "3 new turret:: unit tests." Checked while writing this
handoff (`git show 929586bc -- src/turret.rs | grep '#\[test\]'`) —
it's actually 2. The third thing that commit message counted was a
separate assertion inside one of those two tests (the pivot-location
check), not a standalone test function. Not going back to amend a
pushed... well, not-yet-pushed but already-real commit for a one-word
count error, but flagging it here rather than letting a wrong number
sit uncorrected, same as the prior session's own convention.

## Build and test

```
cd games/blackstar-rs
cargo check --no-default-features   # clean
cargo check --features analog       # clean (no system deps needed)
cargo check --features audio        # needs libasound2-dev -- NOT installable
cargo check                         # default features = audio + analog -- same blocker
cargo check --no-default-features --features gamepad   # needs libudev-dev -- NOT installable
cargo test --no-default-features --bin skydribble       # 1264 tests, ~2s
cargo build --examples --no-default-features            # all examples build clean
```

Confirmed this session: in the current sandbox, `apt-get install
libasound2-dev` 404s against the mirror (`security.ubuntu.com` doesn't
have that exact package version), so `audio`/default and `gamepad` cannot
be built or tested here at all — not a code problem, just missing system
packages this sandbox's network allowlist can't fetch. `--no-default-
features` and `--features analog` are unaffected and are what this
session's verification actually ran against.

No display in this sandbox either — **nothing here was ever visually
verified**, same as every prior session. Everything in `TODO.md` and in
this session's commits is backed by a compile, a passing test, or
reproducible CLI tool output. Real signal, not a substitute for someone
actually running the game.

## Before editing `TODO.md` further

It's now 712 lines (grew by 87 this session, two new findings appended
to existing entries rather than new top-level sections). `grep -c "^### \|
^## "` should return **14** — checked after every edit this session, held
steady. If it doesn't after your edit, something got truncated (the prior
session's own "process notes" section documents the exact `str_replace`
failure mode that caused this five times before: matching only the first
line of a multi-line bullet and silently dropping the rest).

## What's genuinely done vs. what needs you

**Done and verified this session**, each against real asset data or a
targeted regression test, not just synthetic fixtures:
- `skeleton::CORE_R` — a prim that's centroid-central but reaches far in
  one direction no longer wrongly becomes "core" (confirmed on `starlord`
  prim 34, the exact repro the prior session left).
- `turret::find` now detects a barrel prim and a horizontal elevation
  axis/pivot (confirmed on real `hussar_tank` geometry) — the yaw-only
  limitation from the prior session's notes is half-closed. Pitch is
  still not wired into the aim/draw path; see below.
- `Chassis::transforms` (default true) lets a `chassis.json` opt a boss
  out of the vehicle<->robot fold entirely — the mechanism `TODO.md`
  called "not implemented at all." Nothing currently uses it; that's a
  content decision for whoever picks a boss to flag, not a code gap.

**Investigated and deliberately left alone**, with the reasoning recorded
in `TODO.md` rather than a guessed fix:
- The proposed `orientsim.rs` commit-time `align::unify` guard is now
  moot — traced the actual load path and confirmed `model.rs`'s
  `sidecar_is_heuristic` already makes every file `orientsim.rs` can
  write inert to the real game loader, regardless of what's on disk.
- `boat_mistral_class_lhd` still fails the exact straddle/compact guard
  that was written for it (35 crotch hits, guard's own threshold is on
  the wrong side of this case's numbers by a wide margin: 0.26 measured
  against a <0.4 bar). Traced precisely why; did not attempt a third
  fix on a check this file already documents as having flip-flopped
  twice, with no labelled set to verify a new number against.

**Explicitly NOT done, and shouldn't be attempted blind** (unchanged from
the prior session's assessment — still true): any of the four
control-scheme proposals (jet pitch/VTOL, tank mode-cycling, racer weapon
toggle), actually wiring turret elevation into a real aim pitch (the
*detection* is done; picking elevation/depression limits and driving it
from camera/aim input is gameplay-feel work), and retuning
`TREAD_LOW`/`MIN_MATCHED`/`partdetect`'s Leg-tagging thresholds — all
need either a controller in someone's hands or a labelled set to check
a number against, neither of which exists in this sandbox.
