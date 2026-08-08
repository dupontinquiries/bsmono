# Is a multi-level voxel grouping worth it here?

Measured rather than argued. `tools/voxel_probe.py` voxelises a model, merges
occupied cells into runs (the 1D core of the greedy meshing unendlich already
has in `world/surface.rs`), and reports size and fidelity together at several
grid resolutions — unendlich's `LodLevel::{Near,Mid,Far,VeryFar}` is the same
multi-level idea, already four levels deep.

## The headline

**Six full vehicle models, run-length voxels gzipped, against gzipped OBJ:**

| grid | run_gz | vs obj_gz | voxels | quant% | IoU |
|---:|---:|---:|---:|---:|---:|
| 32³ | 30,288 | **1.1%** | 13,868 | 2.706 | 0.953 |
| 64³ | 149,857 | **5.6%** | 66,695 | 1.353 | 0.953 |
| 128³ | 724,095 | **26.9%** | 316,510 | 0.677 | 0.955 |

Forty catalog parts behave the same way: 1.9% / 8.0% / 35.9% at 16³ / 32³ / 64³.

So yes — **promising, and by a wide margin**. At 64³ a vehicle costs about a
twentieth of its OBJ, and even at 128³ — where a cell is 0.68% of the model's
longest side — it is a quarter.

## What the two fidelity columns mean, and why there are two

`quant%` is the cell half-diagonal as a percentage of the model's longest side:
the **intrinsic bound**, since no detail finer than a cell survives whatever the
rasteriser does. `IoU` is intersection-over-union against a reference
voxelisation at twice the resolution, downsampled — it catches whether the
SHAPE is right, which quantisation alone does not.

**The first metric I wrote was a tautology and it took a re-read to notice.** It
measured "distance from each original vertex to the nearest occupied voxel" and
returned exactly 0.000 at every resolution — voxelisation marks every vertex's
own cell by construction, so the test could not fail. It would have reported
perfect fidelity forever. Both current metrics can and do come out imperfect.

## The result that decides the design

**IoU is flat at ~0.953 across every resolution.** Going from 32³ to 128³ costs
24× the bytes and buys nothing in shape agreement.

That is the interesting finding, and it is not what I expected. It says the
error is dominated by *surface thickness* — a shell voxelised at any resolution
disagrees with a finer shell mostly at its boundary — rather than by detail
being lost. Which in turn says:

* **Refining the grid is the wrong knob.** It is pure cost.
* The multi-level grouping should be a **hierarchy over one working resolution**
  (coarse blocks that resolve into finer ones only where occupancy is mixed),
  not the same model stored at four resolutions.
* 32³–64³ is the operating range. Below it quantisation starts to bite; above
  it, nothing improves.

## Where this meets the parts work

These are complementary, not alternatives, and the numbers suggest the order:

1. **Parts first.** The part library removes *repeated* geometry — a wheel
   stored once and placed six times. Voxelising first would voxelise all six.
2. **Voxels second, per part.** A part is small, rigid, and reused: exactly the
   thing worth storing once in a compact form. And a placement is a rigid
   transform, which applies to an occupancy grid as readily as to a mesh.

## Honest limits

* Surface voxelisation, not solid — these models are shells, and a flood fill
  would call any model with a seam entirely hollow or entirely solid depending
  on where it started.
* No materials or UVs yet. The corpus has textures, and this measures geometry
  only.
* Six models is a probe, not a corpus. The catalog run (40 parts) agrees, which
  is encouraging, but the full 1,012 should be measured before committing.
* Runs are merged on one axis. `surface.rs` merges rectangles across two, so the
  real greedy mesher should beat these numbers rather than match them.
