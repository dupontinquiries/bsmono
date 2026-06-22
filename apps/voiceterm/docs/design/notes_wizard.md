# Notes / zettelkasten setup wizard (v0.1.710)

A guided flow to start a personal zettelkasten, with an optional spoken
walk-through. Front-loaded breadth: id schemes, layout choice, seed content,
voice narration, and a scaffolder — all in one feature.

## Architecture
- **src/zettel_id.rs** (pure, 7 tests) — id schemes:
  - timestamp (`202606071430`) via `timestamp_id` / `timestamp_id_from_unix`
    (with a dependency-free `civil_from_unix` UTC date conversion).
  - Folgezettel (`1`→`1a`→`1a1`, sibling `1a`→`1b`, carry `1z`→`1aa`) via
    `folge_child` / `folge_sibling`, plus `next_toplevel` and `is_valid_folge`.
- **src/workspace_wizard.rs** (pure, 8 tests) — everything decision-shaped:
  - `WizardConfig` (root, id_scheme, layout, seed toggles, voice).
  - `Step` state machine (Welcome→ChooseLocation→IdScheme→Layout→SeedContent
    →Review→Done) with next/prev/index/title + `can_advance` gating.
  - `slug` for filenames.
  - `scaffold_plan(cfg) -> Vec<FileSpec>` — the deterministic list of
    dirs/files to create (index, primer, daily template, `.web2qml-notes`
    marker; structured layout adds inbox/permanent/references/index dirs).
  - `voice_script()` / `narration_for(step)` — per-step narration + the
    phrases that advance each step.

## Wiring (main.rs qt_methods)
- `wizard_voice_script()` → the script as a wire (idx\ttitle\tsay\taccepts).
- `wizard_narration(step_idx)` → one step's narration.
- `wizard_next_id(scheme, arg)` → next id (timestamp uses the clock;
  folgezettel takes `child:<id>` / `sibling:<id>` / `top:<csv>`).
- `wizard_scaffold(config_wire)` → runs scaffold_plan, writes each file under
  root (atomic create_dir_all + write, never clobbers an existing note),
  returns `ok\t<n>\t<index-path>` or `err\t<msg>`.
- signal `notes_wizard_requested` opens the UI.

## QML (main.qml)
A modal `Popup` (`notesWizard`) with a `StackLayout` keyed to `step`, a
`FolderDialog` for location, radio/checkbox controls per step, a live voice-hint
line, and Back/Next(Create/Done) footer. `handleVoice(phrase)` maps walk-through
phrases to actions; the freeform voice dispatch routes to it while the wizard is
open, and opens the wizard on "set up my notes" / "new zettelkasten".

Voice narration plays through the guarded `_playSpeechPlan` hook — SILENT until
the audio/TTS layer is wired, by design.

## Status
Cores: unit-tested (15 tests). Wiring: compile-gate + qt_struct (510 fields) +
qml_lint clean. RUNTIME-UNTESTED: the QML wizard, FolderDialog, voice routing,
and actual file creation are not exercised in-sandbox (no GUI) — dogfood
confirms. The scaffold writer is straightforward fs and the plan it consumes is
fully tested.

## Backlog (parked)
- Use `wizard_next_id` from the editor's "new note" action to auto-name files.
- Remember the chosen notes root in settings so the editor/[[ ]] resolver and
  note_search default to it.
- A "convert existing folder" path (detect notes, infer scheme).
