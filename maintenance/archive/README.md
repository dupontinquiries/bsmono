# maintenance/archive — code that is kept, not compiled

Source files removed from a crate but worth keeping readable, each carrying the
provenance a future reader needs to judge whether it is still useful.

## Why not just delete it

Git has the file either way. What git does not have is the *reason*, at a path
someone will stumble across. A deleted module is discoverable only by someone
who already suspects it existed and knows to run `git log --diff-filter=D`; an
archived one is discoverable by `grep`.

That matters most for the case this directory was created for: a module
superseded by a wider one, where the superseded copy is the clearest available
explanation of what the wider one had to absorb. `iq3.rs` documents IQ3_XXS's
format in twelve tests that `iq23.rs` never needed to restate.

## Why not leave it in `src/`

Because an undeclared file in `src/` is the worst of both. This session found
nineteen of them across the monorepo — never compiled, tests never run, and
still reading like the real implementation to anyone who greps. Two documents
ended up disagreeing about whether one of them existed. `maintenance/archive/`
is unambiguous: nothing here is built, and the header says so on line one.

`maintenance/` has no `Cargo.toml`, so cargo never sees these files and
`maintenance/tools/orphan_check.py` does not scan them.

## The header every entry must carry

`maintenance/tools/archive_check.py` enforces these five fields. It is the
whole point of the directory — an archive without provenance is a junk drawer.

```
// ARCHIVED: <one line: what this is>
// ORIGIN: <path it was removed from>
// REMOVED: <commit sha> <date>
// SUPERSEDED-BY: <what to read or use instead, or NOTHING>
// LAST-KNOWN-GOOD: <what was actually confirmed, when, and by what>
```

`LAST-KNOWN-GOOD` is the field that earns its keep and the easiest to write
dishonestly. It records **what was confirmed, not what is believed**. "14 tests
passing at <sha>, 2026-08-22" is a claim with a date and a means. "worked fine"
is not, and neither is silence — if nothing was ever confirmed, the field says
`NEVER CONFIRMED` and that is a useful thing to know before reviving it.

Distinguish carefully between *its tests passed* and *it ran against real
data*. Those are different claims and the archive is exactly where the
difference gets lost.

## Reviving something

Read `REMOVED` and diff against what superseded it before copying anything
back. The reason a file is here is usually that it diverged from a live
implementation, so its code is the *older* answer to a question someone has
since answered better — even where its tests are the better tests.

## Contents

| file | origin | superseded by |
|---|---|---|
| `gpu-gemm-iq3.rs` | `apps/logscope/crates/gpu-gemm/src/iq3.rs` | `iq23.rs` |
