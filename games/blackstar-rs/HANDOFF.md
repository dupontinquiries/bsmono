# Handoff — blackstar-rs session bundle

For whichever Claude picks this up next to merge/review. Read this first;
`TODO.md` is the full backlog (fixed/verified, found-not-fixed with exact
repro, design proposals) — this is just the map to it, plus the workflow
things worth knowing before touching anything.

## Git history is real, organized, and audited — not one giant commit

10 commits this session, each scoped to one theme (`git log --oneline`
shows them). Verified two ways before calling it done, both worth
repeating if you add more commits on top:

1. `git status --short --untracked-files=all` **from the repo root**, not
   just `games/blackstar-rs/`. A prior session in this same monorepo
   (`apps/aperture/HANDOFF.md`) hit a real bug from exactly this shortcut
   — `cd app && git add .` silently missed a file outside that directory,
   and it sat modified-but-uncommitted through 10 subsequent commits
   before anyone noticed. Checked here: repo-root status came back
   completely empty after the last commit. Re-check this before you
   merge, since it costs one command and the failure mode is silent.
2. `git show --stat` on every one of the 10 commits, comparing the actual
   changed-file list against what the commit message claims. All 10
   matched exactly — no stray files, no cross-contamination.

One commit (`main+orientsim: hover-mode fix, cinematics wiring, seat
diagnostic label`) is a deliberate bundle rather than a clean single-topic
change — `main.rs` had 10 mixed hunks across three unrelated features and
splitting them by hand risked introducing an error for the sake of
cosmetic history. Itemized clearly in that commit's own message instead.

## Build and test

```
cd games/blackstar-rs
cargo check --no-default-features   # fastest; what this session used most
cargo check --features audio        # needs libasound2-dev
cargo check                         # default features (audio + analog)
cargo test --bin skydribble --no-default-features   # 1259 tests, ~2.3s
cargo build --examples --no-default-features        # the 5 new diagnostic tools + existing ones
```

No display in this sandbox — **nothing here was ever visually verified**.
Every finding in `TODO.md` is backed by a compile, a passing test, or
reproducible CLI tool output, which is real signal but not a substitute
for someone actually running the game. If you have a real screen, that's
the single highest-value thing to do next — see `TODO.md`'s
"process notes" section for what to look at first.

## Before editing `TODO.md` further

It's 555 lines and has been edited many times this session. The single
most common mistake made while editing it (five separate times, per its
own "process notes") was `str_replace` matching only the *first line* of
a multi-line bullet and silently dropping the rest of that bullet's text.
`grep -c "^### \|^## "` should return 14 — if it doesn't after an edit,
something got truncated; `git diff TODO.md` will show exactly where.

## What's genuinely done vs. what needs you

Done, tested, verified via multiple independent checks: the fold pop-
detection fix, the skeleton core-selection fix, the hover/flight
declaration fix, the raid-boss JSON externalization, 612 real orientation
corrections (with one regression caught and reverted).

Explicitly NOT done, and shouldn't be attempted blind: any of the four
control-scheme proposals (jet pitch/VTOL, tank mode-cycling, racer weapon
toggle) — these need a controller in someone's hands, not more static
analysis. Also not done: retuning `TREAD_LOW`/`MIN_MATCHED`/`CORE_R` — all
three have real, measured evidence of a problem but no confident fix
without checking the change against several very different real chassis,
which needs the same thing (someone looking at it).
