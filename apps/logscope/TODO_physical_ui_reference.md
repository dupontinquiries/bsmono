# The physical UI, against the reference images

Reference set: `reference-images-logscope-v2` (observatory, mistfall, canopy, atlantis,
mansion-garden, cat-cave, forest-village, alley). Two kinds of image in it, and telling them
apart matters:

* **observatory-01/02, atlantis, mansion-garden, canopy** are TARGET renders — what the app
  should look like.
* **mistfall-01** is a SCREENSHOT of the current egui build. Comparing those two is where the
  work is.

## What the target does that the build does not

**Node labels are plates, not floating text.** In the target the graph's labels are opaque
rounded cards with a short stem to their node, ringing the cluster, each one legible on its
own. Nothing overlaps.

**The chrome is carved, not drawn.** Panels are plates with scored edges and small-caps serif
headings, and the 3D scene sits INSIDE that frame rather than behind flat widgets. The build
already has `physkin` for this per scene; the target is a good deal further along.

**Quick actions are labelled with their shortcut** (⌘N, ⌘S, ⌘I, ⌘R) on the plate itself.

## CORRECTION: the screenshot is not current, and the box is not wrong

I wrote below that mistfall-01 proves a live defect. Two checks say otherwise, and both were
available before I claimed it:

**The screenshot predates this build.** Its right panel has four tabs — Files, Graph, Context,
Prompting. The build has five: `RightPage` gained `Engine` earlier in this session. So the
image is at best a snapshot of an older tree, and "the current build" was my assumption from
the filename, not something I checked.

**The suspect I named is innocent.** I said `px_extents` probably under-measures because it
estimates from a character count. Measured: `px_extents` computes `chars * 6.0 * cell *
per_px`, and `scene_kit::mesh3d::label_cells` — which actually draws the label — lays it out
as `chars * 6.0 * scale`. Same constant, same count, computed from the same final string
(`chars` comes from `label_of(...)`, after the role tag and elision, not from the `LABEL_CHARS`
cap I guessed at). The model and the draw agree about the box.

So: the overlap in that image may already be fixed, and if it is not, the cause is somewhere
other than the extents. Do not start from my hypothesis.

**What would settle it**, in order:
- [ ] A fresh screenshot of the current build in the same scene. One image ends this.
- [ ] Failing that, point `layout_probe::analyse` at the spatial pane — it reports text
      overlap directly and has never been run against the scene. That is a test, not an
      opinion, and it is the thing I should have written instead of a hypothesis.

## The defect as reported in mistfall-01 (see the correction above)

Graph node labels COLLIDE — "82t: 6." sits on top of "82t: 6. How can we make thi", and three
more overlap around the strand. This is not a missing feature: `nodelabel::visible` is a real
collision culler and `chat_spatial` calls it.

So the declutter runs and the labels still overlap, which narrows it to the BOX the culler is
given:

- [ ] `nodelabel::px_extents` estimates a label's screen box as `chars * 6.0 * cell * per_px`
      wide by `7.0 * cell * per_px` tall. That is an estimate from a character COUNT, not a
      measurement of the drawn galley. If it under-measures, boxes miss in the model while the
      text collides on screen — exactly the symptom.
- [ ] Measure it: lay out a real label at the font the scene draws with, at a known depth, and
      compare against `px_extents` for the same string. One number settles whether the
      estimate is the fault.
- [ ] If it is: feed the culler the galley's own size. `uiverify` can then assert the fix in
      pixels, and `layout_probe::analyse` already reports text overlap directly — the harness
      for this exists and has never been pointed at the scene.

A caution from the module's own header, which has been wrong about this before: `nodelabel`
carries a "CORRECTED CLAIM" about `outward` not reducing collisions. Measure before changing.

## Not attempted

The visual gap between the mistfall screenshot and the observatory render is a design pass,
not a defect list — colour, plate geometry, type. It wants doing against the images with a
person looking, not inferred from them.
