# C20 — open threads

The session that followed `c19unified-003-fleet`. Each item is marked with how
it was established: **verified** (run or measured), **read** (checked against
source), or **assumed**. Companion: `MERGE_GUIDE_C20.md` (what landed, and how
each piece was checked).

## Investigated, not changed

These apps were read closely and no code was changed. The findings are here so
the next session does not have to re-derive them. Details:
`docs/PULSAR_RECONCILIATION.md` (checked against source).

### voiceterm
- **read** -- The zettelkasten is complete: `zettel_id.rs` (timestamp ids or
  folgezettel), `zbrackets.rs` (`[[links]]`), `link_graph.rs` / `backlinks.rs`,
  and the egui `graph_view.rs`. `games/blackstar-rs/tools/models/ingest.py
  zettel` writes into exactly that format.
- **read** -- Paste an image into markdown: the `.assets/` convention and
  `manpages::resolve_one_img_src` already exist. The only missing piece is the
  clipboard read. Shared with aperture.
- **read** -- Diagrams: `nifty-core::mermaid` plus `layered_layout.rs`
  (Sugiyama, with real crossing reduction) are present and ahead of what the
  old packages did.
- **read** -- `docs/design/editor_vision.md` and `note_editor.md` (the Typora-
  style live-preview blend) are the open design; nothing was built against them.

### aperture
- **read** -- Three real gaps, in this order (`docs/PULSAR_RECONCILIATION.md` §3, §5):
  1. **Colour swatches in the right margin** beside hex/rgb/hsl literals. The
     cheapest win, and it proves the margin mechanism with no sizing question.
  2. **An SVG painter** over `egui::Painter` for the subset `nifty-core::svg`
     emits: rects, rounded rects, lines, polylines, text. This is the actual
     blocker for inline diagrams: the producer exists, and nothing renders it.
  3. **Inline diagrams from comments**, sized from their own comment block and
     painted into the margin without changing row spans. Designed in §1, not built.
- **read** -- Read `teletype-diy` for its failure modes before `aperture-collab`
  grows. Its CRDT assumptions do not match "an edit is a value that carries its
  inverse".

### tomescope
- **verified** -- Demo book added: `apps/tomescope/demo/Elementals.epub`, with a
  designed cover and rebuild scripts.
- **read** -- Flashcards are `study.rs` `Deck{cards:[Flashcard]}`, persisted to
  `~/.config/tomescope/flashcards.json` by `persist.rs`. A converter exists but
  lives outside the app: `games/blackstar-rs/tools/models/ingest.py cards`
  (flashcard markdown -> that JSON). **TODO:** an in-app import, or move the
  converter to `apps/tomescope/tools/`.
- **read** -- EPUB loads through `formats_epub.rs` (container.xml -> OPF ->
  spine / nav).
- **assumed** -- coursekit's notes would make a useful tomescope import too; not
  checked against tomescope's formats.

## blackstar-rs -- fixed in C20
- **verified** -- `vocab::tests::the_licensed_vocabulary_census_only_goes_down`
  passes at the base and was broken by this session's own `3bb74dcfd` ("group the
  catalogue"). It added an `IP_MARKS` list to `main.rs` that raised five counts
  by one each. The list is legitimate -- it shows licensed models as their own
  catalogue family -- so it moved to `vocab.rs`, the one file the census exempts,
  and the counts are back to the base values. (A session note first said the
  census failed at the base: that was measured after `3bb74dcfd`, not at
  `aa5482537`. Wrong, and corrected here.)

## blackstar-rs -- found, not fixed (present at the base commit)
- **verified** -- `src/rig.rs:96` is corrupted: `Transform::the Charge { matrix }`
  should be `Transform::Matrix`. Only the `rigpose` example includes it, so
  `cargo test` without `--bins --tests` fails to compile that example.
- **read** -- A hurt left leg never limps. `anim::Locomotion::condition` clamps
  `Body::limp()` to 0..1, but `limp()` is signed (positive means the right leg
  is hurt), so left-leg damage is dropped. `rigmotion` reads the sign
  correctly; the gait machine does not.
- **verified** -- On a sandbox capped at ~19 GB of disk, full debug builds of
  this crate fail by running out of disk, not memory.
  `maintenance/tools/cargo-ailite test` builds and passes.

## blackstar-rs rig -- next steps
- **verified** -- `rigmotion` drives a 22-bone rig every frame (5 tests). The
  renderer does not read `rig_frame` yet. Next: skin a bot mesh to
  `rig_frame.joints`.
- **read** -- The rig height is fixed at 3.9 m in `main.rs`; derive it from the
  active chassis. `Gait::Land`, `Climb`, `Crouch` and `WallHug` map to idle.
- **read** -- Ranged shots are placed from the current muzzle, which is exact
  only while aiming steadily. Nothing is hit.
- **read** -- Weapons are right hand only. There is no blending between clips,
  and no layering (a weapon hand while walking).
- **verified** -- Viewers: Folly's pods touch its shin spikes (4% at t=1). A
  small joint sphere shows at the flyer's nose tip at t=0. Wedge paths are
  solved for orion only (run `rigkit/harness/wedge.js` for the others).
- **verified** -- partnet: a network beats nearest-known-part only on keep-or-
  dissolve, with three vehicles to learn from. Re-run `rigkit/partnet/partnet.py`
  as vehicles are added.

## Placed, not re-run here
- **read** -- `tools/gitsweep`, `chunkup`, `chatgrep`, `coursekit` and the
  `models/` tools were delivered in earlier sessions. Here they only compiled
  (`py_compile`); they were not re-run on real data.
