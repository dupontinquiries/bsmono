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

## Found while checking: this crate's tests had not been running -- NOW FIXED

`cargo test --lib` did not COMPILE at HEAD (`lib.rs:1768`, `.abs()` on an
untyped literal). Fixed in 6b4d41603. That exposed two failures in
`job::tests`, both since fixed; the suite is 866/866.

* `bounds_accumulate_over_steps`: the CODE WAS RIGHT, the test expected the
  last piece's bounds instead of the union. **Correction:** an earlier
  version of this note said `readback()` printed no extent at all. That was
  wrong -- I had truncated the failure message with `head -10` and drew a
  conclusion from the half I could see.
* `an_unknown_material_is_refused_and_a_real_one_suggested`: two real
  faults in `palette::suggest` -- it could not see inside a run-together
  compound (`brownwood`), and it let a name hit outvote a named family
  (`darkstone` -> dark_oak_planks), against this module's own "family
  dominates" rule. The second only showed when the test asked for the right
  FAMILY rather than `is_some()`.

The four questions above are still open; none has been checked.
