# UI strategy — egui primary, Qt/GTK archived as kits

The egui front-ends are the go-forward UI for every app. The Qt/QML and GTK front-ends are **archived,
not retired**: they stay in the tree, keep building, and are factored into their own kits, because
(a) they're powerful UI libraries in their own right and (b) egui hasn't reached feature parity with them
on every surface yet. Nothing here deletes a working front-end.

## The three tiers

- **egui (primary).** Every app iterates here first. Shared widgets live in `shared/uikit/crates/uikit-egui`
  (motion, palette, dialogs, pickers, log/tile views). Colours/animation/behaviour track the Qt apps as the
  north star (see the `Night` palette matching gitmind's Qt surface).
- **qt-ui-kit (archived, maintained).** The reusable Qt/QML surface, kept for parity and reference.
- **gtk-ui-kit (archived, maintained).** The reusable GTK4/libadwaita surface, same footing.

## Frontend inventory

**`manifest/projects.toml` is canonical.** It is machine-read — gitmind's app
catalogue builds from it — so a table here would be a second answer to a
question that already has one, and the two would drift silently. They already
had: this file listed av1tuner as egui-only while the manifest still said
`runtime = "tbd"` ("GUI toolkit not yet confirmed") with no `primary` flag, so
the catalogue could not tell which frontend to build. Reconciled 2026-09-02;
the manifest was the stale side and was corrected.

To read the inventory:

```sh
# every app and which frontend is primary
grep -E '^\[apps\.|^primary|^runtime' manifest/projects.toml
```

`primary = true` marks the frontend that `cargo build --release` should
produce and that gitmind offers. An app with a Qt/QML or GTK surface alongside
its egui one carries the flag on the egui entry; the archived surface stays in
the tree and keeps building (see the tiers above) but is not what gets built
by default.

If you add a frontend, put it in the manifest. This file explains WHY the
tiers exist; it does not track WHICH apps are in them.


## Why kits, not deletion

A Qt/QML surface and a GTK surface are each a coherent, reusable UI library: theming, component set, and
interaction model. Collapsing each into a kit (rather than leaving per-app copies) means one place to
maintain the reference implementation, and a clean home to pull from when an egui surface needs to reach
parity with something the mature front-end already does well.

## New egui app: good defaults for "avoid boilerplate, but I'll want nice-to-haves later"

Written after a pass on bo-suite-egui that kept finding the same two failure shapes across a real app —
recorded here so the next app doesn't repeat them.

- **Depend on `uikit-egui` from the first commit, even for a "quick" app.** It's already in the tree;
  pulling it in costs a `Cargo.toml` line and buys palette/theme, toast, dialogs, adaptive layout, a11y
  focus, and gesture handling for free. The apps that skipped it and built their own toast/dialog/palette
  are the ones that later needed a rewrite to look consistent with the fleet.
- **But don't invent a widget for a generic need — check `shared/uikit/crates/uikit-egui` first.** This
  pass found `fuzzy.rs` (subsequence scoring, built for voiceterm's command palette) and `path_picker.rs`
  (a lister-agnostic directory walker, generalized from gitmind's SSH deploy browser) already solved
  problems that three different apps had each solved again on their own — one bespoke picker per app
  instead of one picker three apps use. Search the kit before writing a file/fuzzy/dialog/picker widget;
  if it's there, use it; if it's *almost* there, extend it in place (small, tested, additive) rather than
  forking it, so every consumer gets the improvement.
- **A command palette is `uikit_egui::command_bar::show`, and on a phone it is upside down.** Every app
  used to draw its own, and every one drew the desktop shape: a card floating top-centre, input first,
  matches below. Held in a hand that is backwards — the input is at the far end of the screen from the
  thumb and the on-screen keyboard, and the best match is furthest of all. The kit's surface decides
  this once: on a compact or touch surface (`Adaptive`) the card sits on the bottom edge, the input is
  its last row, and matches stack upward with the BEST one directly above the input; arrow keys follow
  the picture. On a desk it is the familiar card. Placement is about reach; the scrim is a separate
  question about coverage (`mode_for_rows`). The app supplies `Row`s and decides what choosing one does
  — nothing else. Pair it with `palette_flow` (Ctrl+P, idle-to-voice) and `cmdstrip::Hold` on a button
  a thumb can reach, because a palette bound only to Ctrl+P does not exist on a phone. Three faults the
  surface already carries fixes for, each found the hard way: rows must be `Button::truncate` (the
  widget overrides a LayoutJob's own wrap settings); the click that opens the palette is, that frame, a
  click outside it; and rows must use `clicked_by(Primary)`, because egui's arrow-key focus navigation
  runs before you can consume the arrows and Enter then "clicks" whichever row took focus. First
  consumers: `apps/stoat/crates/gui-egui/src/palette.rs`, `apps/vidkit-player`.
- **The recurring bug worth naming: a fully-tested dialog struct with nobody drawing it.** Three separate
  dialogs in one app (`AddToPlaylist`, `EditTrack`, `ScanPicker`) each had a complete, unit-tested
  lifecycle — `open`/`close`/validation/`payload` — a real call site that called `.open(...)`, and *no
  rendering function anywhere in the crate*. All the tests passed. Nothing on screen ever appeared. The
  tests exercised the model in isolation and never caught that the model had no view. Two things prevent
  this: (1) treat "state machine" and "renders it" as one unit of work — don't merge a dialog's model
  without its `_ui` function in the same change, even a minimal one; (2) run `tools/peek` before calling a
  feature done — it exists specifically to flag possibly-unwired modules, over-reports (treat hits as
  candidates), but a `.open()` call with no matching `_ui(...)` call site is exactly its blind spot turned
  into a real bug, and worth a manual grep (`is_open()` / `\.open(` without a sibling `_ui(` in the same
  file) when peek can't see it either.
- **A headless-egui frame test is cheap insurance against exactly that.** `egui::Context::default()` +
  two `ctx.run(...)` passes (a floating `Window` auto-sizes from the previous frame, so the first pass
  paints nothing — this is a test-harness detail, not something the shipped app inherits) + scraping
  `out.shapes` for `Shape::Text` is enough to assert a dialog actually draws its expected labels when
  open and nothing when closed. It would have caught all three dialogs above on the first commit that
  claimed to "add" them. Cheap enough to write for every new modal.
- **The `pending_X` request-slot pattern is the default shape for a Controller ↔ host boundary**, seen
  working repeatedly this pass (`pending_rescan`, `pending_cast`, `pending_playlist_create`,
  `pending_track_edit_save`, `pending_scan_with_roots`, `pending_remote_queue_push`): pure, synchronous,
  unit-testable `Controller` code sets an `Option<T>`/`bool` field describing what it wants; an impure
  host (`main.rs`/the `eframe::App`) drains it once per frame and does the actual I/O; a `confirm_*`/
  `*_result` method on `Controller` reconciles the answer back in. No `async`/`await` anywhere in the UI
  layer, and the business logic is testable with zero mocking — every test in this pass that touched
  request/response state did it by constructing a `Controller` and asserting on the plain field, not by
  spinning up a runtime.
- **Anything that touches disk or network gets its own thread + channel, never runs on the UI thread.**
  A scan, a cast attempt, a database write that might contend with another writer — spawn a
  `std::thread` owning an `mpsc::Sender`/`Receiver` pair, poll it non-blocking (`try_recv`) each frame.
  This is the same shape bo-media's own worker uses internally; reusing it at the app level instead of
  blocking `update()` is what keeps the frame rate honest under a slow operation.
- **A local SQLite (or similar) database read doesn't need to go through that worker if it's WAL-mode.**
  A second connection opened directly for reads is fast and safe *precisely because* WAL exists for this
  — only serialize the writes that need real ordering (a scan vs. a rating vs. a playlist edit can race
  harmlessly with a `busy_timeout`; a scan racing itself cannot). Don't reflexively route every read
  through the write-owning worker "to be safe" — that's the assumption that produced this app's original
  filename-guessing catalog instead of just opening a second connection to the real one.
- **Feature-gate the slow-to-compile optional dependency, not the whole capability.** bo-media's BPM/key
  analysis (`bliss-audio`, which pulls ffmpeg/symphonia) sits behind an `analysis` cargo feature so a base
  build stays fast and the app still works — those columns just stay null — without it. When a
  nice-to-have needs a heavy crate, this is cheaper than making the feature itself optional at the API
  level.

## Migration steps (per app, on a Qt/GTK-capable build host)

1. **egui is default.** Ensure the app's default/primary build path is the egui front-end. (Config only —
   e.g. gitmind already keeps both GUIs out of `default-members`, so the CLI stack builds without a
   toolchain; the GUI is opt-in and should default to `gui-egui`.)
2. **Lift the reusable surface.** Move the shared Qt/QML components into `shared/qt-ui-kit` (and GTK into
   `shared/gtk-ui-kit`); leave app-specific glue in the app, re-pointing its QML import / GTK resource
   paths at the kit.
3. **Keep it building.** The kit and the app must still compile and run on a Qt/GTK host — these steps are
   verified there, not in the headless CI sandbox (which has neither Qt nor GTK).
4. **Parity ledger.** Record, per surface, what the Qt/GTK front-end still does that the egui one doesn't,
   so egui iterations have a concrete target.

## Build/orchestration notes

- App→path resolution is discovered from the filesystem (apps/ vs games/), not hard-coded — a new
  app/game needs no store change.
- Apps needing a system toolchain belong in the hardware stage: **unendlich** (Vulkan + `glslc` shaders),
  **ambient-learner** (GTK4/libadwaita), the snapclone/photosync C++ stack, and opendht. The pure-cargo
  pass should either skip these or build them with a headless feature.
