# Wiring panes and collab into aperture-egui

`aperture_core::panes` (17 tests) and `aperture-collab` (47 tests) are
built, tested and declared as dependencies. **Nothing in the shell calls
either.** There is no split in the UI and no session to join.

This note exists because I stopped rather than started. The remaining
work is not a patch — it is a refactor of a 3,457-line `AppState` and a
460-line `editor_view`, and it turns on two decisions that should be
made deliberately rather than discovered halfway through. Written from
having read the code, so the next session argues with specifics.

## Where it actually attaches

* `AppState` holds **one** editor: `pub editor: Editor` and `pub path:
  Option<PathBuf>` (state.rs:246). Panes need a document MAP keyed by
  `panes::DocId`, with `editor`/`path` becoming a lookup on the focused
  pane's `View::doc`.
* `editor_view(ui, state, colors, dt, now_ms)` (view.rs:226) takes the
  whole state and calls `ui.max_rect()` for its area. Multi-pane needs
  it to take a **rect and a `&mut View`** instead, so it can be called
  once per pane from `panes.pane_rects(w, h)`.
* Camera, scroll and selection currently live on `AppState`. They are
  per-VIEW, not per-document — that split is already modelled in
  `panes::View { doc, top_line, sel }`, and moving them is most of the
  refactor.
* `state.rs`'s focus-clearing comment (view.rs:232) is a warning worth
  reading first: egui keeps a focused widget id after the widget stops
  being drawn, and that already caused a silently dead editor once. A
  pane that closes while focused is the same hazard with more ways in.

## What step 1 actually costs — measured

`.editor` appears **230 times** across `aperture-egui/src`. That number
alone would make it a large but mechanical change, every site
compiler-checked.

It understates it. `state.editor` is a FIELD, so the borrow checker
splits it from its siblings: `state.editor.insert(...)` and
`state.camera` can be live at once. An `editor()` / `editor_mut()`
accessor borrows **all of `state`**, so every site that currently holds
two field borrows at once stops compiling and needs restructuring — not
substitution.

That is the difference between a morning and a session, and it is the
concrete reason this note exists instead of a commit. Anyone starting it
should expect the borrow conflicts, not the 230 renames, to be the work.

Two ways to soften it, neither free:

* Keep `editor` as a field and make the pane map hold only the
  NON-focused documents, swapping on focus change the way voiceterm's
  `pane_host::swap_focus` does. Preserves split borrows entirely; costs
  a swap step and the class of bug voiceterm hit repeatedly, where the
  focused copy and the parked copy disagree.
* Move `camera` and the other per-view fields onto `panes::View` FIRST,
  in a separate commit, so fewer sites hold two borrows by the time
  `editor` moves.

## Decision 1 — does a split share the document?

`panes` already answers this and the answer is tested: **document state
is shared, view state is per-pane**. `split_showing_same` carries scroll
and selection over; `split_open` starts a fresh empty pane;
`open_in_focused` resets the view because the position meant something
in a different file.

What is NOT decided is which gesture the UI offers by default. VS Code
splits showing the same file; most tiling editors open an empty pane.
`panes` supports both and the shell has to pick one for the keybinding.

## Decision 2 — what does a remote caret look like?

`aperture-collab` resolves other people's carets to `(site, anchor,
head)` char indices, and `panes::caret_targets` maps them to every
visible pane showing that document. So the model side is done. The
questions left are visual:

* A caret line plus a name label, or a coloured selection band?
* Colour by site id (stable, arbitrary) or by join order (meaningful,
  unstable across reconnects)?
* What happens when two people are on the same character?

None of these have a right answer derivable from the code, which is why
this is a note and not a commit.

## The transport is also absent

`aperture-collab` produces and consumes `CrdtOp` values and can render
them as bytes (`wire.rs`), but nothing moves those bytes.
`studio-collab` already owns a carrier, session negotiation and DHT
rendezvous; the intended path is to wrap the wire strings as an opaque
payload there rather than write a second transport. Nobody has tried it.

## Order I would take it in

1. `AppState` gains `panes: PaneWorkspace` and a `BTreeMap<DocId,
   Editor>`, with `editor()`/`editor_mut()` accessors resolving through
   the focused pane. **No UI change** — one pane, everything still
   works. This is the big, boring, testable step.
2. `editor_view` takes a rect and a `&mut View`. Still one pane.
3. A split keybinding and per-pane rects from `pane_rects`. Now there
   are two.
4. Collab last, because it needs a peer and the other three do not.

Steps 1 and 2 are verifiable with the existing test suite plus a
screenshot showing nothing changed, which is exactly the property you
want from a refactor of this size.

## Verifying it

`apps/aperture` builds and runs headless — see
[docs/agents/HEADLESS_GUI_TESTING.md](../../docs/agents/HEADLESS_GUI_TESTING.md).
A 413 MB debug binary, about nine minutes, and its target tree is 3 GB,
so delete it afterwards; two egui apps do not fit in the sandbox at
once.

The editing core was checked that way and is sound: auto-close,
auto-indent, highlighting, cursor tracking and the dirty marker all
correct, each keystroke landing once. Whatever breaks during this
refactor will be new.
