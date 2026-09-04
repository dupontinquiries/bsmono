# Entity picker disambiguation — built, never wired

Found via `wiring_check.py` (`picker_ui` in `detect.rs` flagged as an uncalled renderer), checked
by hand rather than trusted as a false positive: the whole subsystem is genuinely disconnected,
not a tool mistake.

## What exists, complete

- `EntityPicker` (`detect.rs`): holds a question, a list of `PickerRow`s (display text + the
  engine's own reason + a score), and an `open` flag.
- `EntityPicker::ask(reference, matches: &[ResolvedMatch])`: opens the picker on 2+ candidate
  matches, sorted by score, and **refuses to open on a single match** — a picker with one option
  is a confirmation dialog nobody asked for, and that refusal is already correct.
- `EntityPicker::choose(primary)`: resolves a selection back to the canonical id and closes.
- `picker_ui(ui, picker, colors)`: the actual egui rendering — question, rows with click targets,
  a "none of these" escape hatch. Reads as finished, not a sketch.

## What's missing

Every one of `EntityPicker`, `PickerRow`, `ResolvedMatch`, and `picker_ui` has **zero references
anywhere outside `detect.rs`** — grepped, not assumed. Nothing in voiceterm's actual voice-command
resolution ever constructs a `ResolvedMatch` or calls `ask()`. Whatever currently happens when a
spoken command references something ambiguously (multiple files/commands matching a fuzzy
reference) almost certainly resolves to a single best-guess match silently today, with no
disambiguation step at all — which is exactly the failure mode this whole subsystem exists to
prevent, and it's sitting unused.

## Why this wasn't wired blind

Wiring this needs finding the actual entity-resolution call site(s) in voiceterm's command
pipeline (wherever a spoken reference currently becomes one match) and routing the multi-candidate
case through `ask()` — which is a question about this app's own dispatch architecture (where does
resolution happen today, what does it look like when it's ambiguous, does the caller have a place
to await a picker choice before continuing) that a session without that context shouldn't guess at.
The picker itself needs no changes; the gap is entirely on the calling side.

## Next step for whoever picks this up

1. Find where a voice command's referenced entity (file, command, etc.) is currently resolved —
   likely near wherever the command is parsed/dispatched.
2. Confirm the hypothesis above: does it currently take the top match unconditionally, with no
   ambiguity check at all?
3. If so, thread the resolution's full candidate list through `EntityPicker::ask()` instead of
   picking one silently, and render `picker_ui()` when `picker.open` — likely in the same place
   `progress_ui` (this file's sibling function) already gets drawn each frame.
