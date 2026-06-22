# ENTITY BODY SEGMENTATION — large-body turn-arc + per-section cliff lerp
For the entity-wiring phase (dragons through the entity pass). Two behaviors a large body needs,
both pure-core-able now and wired on the M2 later. Compose with already-built cores:
`flex_bend` (r1 S-curve), `strafe_face`/`foot_ik` (r1), `attach_frame` (r3),
`large_body_grounding`/`squeeze` (r4), `step_height`/`perception` (r5), `body_scale`/`grounding`.

## 1. Turn-arc (body segmentation)
A big dragon must not yaw rigidly — the body should ARC through a turn, more curvature for longer/
bulkier builds. Distribute the heading change across the spine segment chain: per-segment yaw ∝
segment length / total, with a travelling lag so the head leads and the tail follows. Amplitude
scales with `BodyScale` (bigger ⇒ wider arc) and inversely with turn radius. Reads as a creature
banking through the turn rather than pivoting on a point.

  pure core (queued):
  body_arc(heading_delta: f32, seg_len: &[f32], size: f32) -> Vec<f32>
    → per-segment yaw offsets; sum tracks heading_delta; head-leads/tail-lags travelling curve;
      amplitude grows with size; bounded; deterministic.

## 2. Per-section cliff lerp (different amplitudes per body section)
The existing "lerp the body up cliffs" smoothing should apply DIFFERENT amplitudes to different
body sections, driven by two signals:
  (a) distance to the nearest planted leg — sections over a foot track the ground tightly; sections
      far from any leg follow terrain loosely (more lerp), so the torso glides instead of snapping.
  (b) the cliffside air-gap — the 1-block air-vs-solid difference at the edge. A section overhanging
      air holds its body line (cantilever over the edge); a section over solid follows terrain.
Result: a dragon cresting a cliff bends its body over the lip naturally — no clipping, no snap.

  pure core (queued):
  segment_lerp(leg_dist: &[f32], air_gap: &[f32], terrain_y: &[f32], body_y: &[f32], stiffness: f32)
    -> Vec<f32>
    → per-section vertical target = blend(body_y, terrain_y) where the blend amplitude grows with
      leg_dist and is held back by air_gap (overhang ⇒ keep body line); continuous; bounded; deterministic.

## Integration (M2, not sandbox)
The entity vertex pass applies per-segment yaw (body_arc) + per-section vertical offset
(segment_lerp) on top of the gait pose. Sim position stays authoritative (R-determinism wall).
