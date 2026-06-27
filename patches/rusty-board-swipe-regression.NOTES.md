# rusty-board — swipe-typing regression (breadcrumb for future swipe/touch-kit)

NOT an active task. Captured so the trail isn't lost.

## Symptom (per the author)
Swipe typing worked on early — not the first, but early — iterations of the
keyboard: a **strong, bug-free core**. Then performance work + swipe-leniency
tuning + feature additions broke it, and it **never recovered**. The full
featureset is present in the latest `apps/rusty-board/src/gestures/`, but the
known-good core lives somewhere back in git history.

## Where to look
The bug-free core is believed to be **between commits `876a0fa` and `7d3efea`**
(17 commits in that range) in the rusty-board git history (uploaded as
`rusty-board-git.zip`; `apps/rusty-board` in the monorepo is the latest, broken
HEAD). Note: none of the 17 commits in the range touch `swipe.rs`/`recognizer.rs`
by those names — the good-era code may have lived in differently-named files, so
inspect the whole `src/gestures/` (or its predecessor) at the range endpoints,
not just those two files.

## Approach when we pick this up
1. `git checkout 876a0fa` and `7d3efea`; build/run the keyboard at each, find the
   last commit where swipe still felt right (manual or a swipe-decode test corpus).
2. Diff that good-era path-decode + dictionary-recognizer against today's
   `swipe.rs` (path collection → snap-to-key → dedupe → raw word) +
   `recognizer.rs` (dictionary correction) + `cardinal.rs`. The regression is most
   likely in the leniency/speed-mode thresholds or the candidate scoring that the
   "feature additions" layered on.
3. Recover the core; re-apply only the features that don't regress it.

## Kit angle (future)
The reusable primitive is a **swipe/touch-kit**: gesture path (timed points) →
key sequence (snap + velocity-pivot detection) → candidate words (dictionary +
edit-distance), independent of the GTK/keyboard UI. Once the good core is
recovered it can be lifted, with rusty-board as the first consumer. Possibly
shared with any other touch-gesture surface.
