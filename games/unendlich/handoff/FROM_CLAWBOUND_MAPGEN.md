# From clawbound's `mapgen`: four rules worth checking here

Written 2026-09-19 after building `games/clawbound-battleground/src/mapgen.rs`
on this crate's model. Three things went FROM here TO there (`complete_chain`
pairs, reply hygiene, `Job::readback` -> `Brief::readback`). These went the
other way. Each is a QUESTION to check against voxel-tools, not a claim
about it: I did not read enough of 46k lines to assert any of them.

1. **A batch stops at the first refusal.** A reply with several calls was
   written assuming each earlier one worked. `mapgen::run_chain` runs them in
   order, stops at the first refusal, and tells the model the rest was not
   run. Building on a placement that never happened is how a structure goes
   wrong three steps before anyone notices. *Check: when a plan line fails
   here, do the lines after it still execute?*

2. **Grouped writes are refused AS A UNIT.** `MapDraft::place` places a prop
   and its mirror; if the mirror is refused the original is rolled back and
   the refusal names the mirror. Nothing half-applies. *Check: `repeat`,
   symmetric and multi-part writes -- can one half land without the other?*

3. **Truncation by ARITY, not by keyword.** `llm::looks_truncated` matches a
   closed list of statement heads with fewer than 3 tokens. `mapgen::
   extract_calls` uses "no trailing newline AND the last call has fewer
   arguments than its tool needs", from a per-tool arity table. *Check: a
   statement that needs six arguments and was cut off after four -- three or
   more tokens, head in the list -- is it caught?*

4. **The rules apply to the author of the rules.** clawbound replays its own
   hand-authored map through the AI's door
   (`my_own_hand_authored_centre_passes_the_rules_an_ai_is_held_to`) and
   holds it to the AI's brief. It is a cheap test and it is the one that
   says whether the rules are reasonable. *Check: do the built-in archetypes
   / example programs pass `job::check_line` and the validators?*

Also seen, unrelated to the above: `voxel-tools/harness/src/paintbrush.rs`
and `unendlich-engine/src/voxel_chat.rs` call `looks_truncated` at lines
1258/1248 and 1719/1710 -- they look like two copies of one ~3000-line file.

## Found while checking: this crate's tests had not been running

`cargo test --lib` did not COMPILE at HEAD: `lib.rs:1768`, `r.abs()` on an
untyped integer literal (E0689). One token (`3i32`) fixes it, and is
committed. With it, 863 tests pass and TWO FAIL, both in `job::tests` -- the
multi-turn planning module -- and both look like they have never run:

* `an_unknown_material_is_refused_and_a_real_one_suggested`:
  `check_line(5,5,5,"brownwood")` returns `UnknownMaterial { suggestion: None }`.
  The refusal cannot say what to use instead, which is rule zero of this
  design: a refusal the author cannot act on is just a wall. The palette's
  nearest-name lookup either is not wired into `check_line` or has no match
  for a compound like brown+wood. NOT fixed.
* `bounds_accumulate_over_steps`: `Job::readback()` prints no extent at all,
  so the feature is missing. AND the test's expected string contradicts its
  own doc comment: it says bounds ACCUMULATE, then expects `-6,8,-6 to
  10,22,10`; the accumulated minimum of `[0,0,0]` and `[-6,8,-6]` is
  `-6,0,-6`. Fix the readback and the expectation together. NOT fixed.

Neither was touched: I had not read enough of `job.rs` to change it safely.
