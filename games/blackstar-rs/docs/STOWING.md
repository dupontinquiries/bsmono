# Where does the robot go?

Design notes on hiding the mechanism in vehicle mode, with the
measurements that bear on each option. Prompted by the observation that
Blackout does not tuck his body against the helicopter — he hides it
*inside* the hangar bay and engine housing.

Run the numbers yourself with:

    cargo run --example fold_audit -- <pose dir> --cavity

## What the measurements say

For each chassis: voxelise the hull, flood-fill the empty space from
outside, and whatever the fill cannot reach is enclosed interior — the
bay. Then compare that to the volume the limbs occupy.

    tank_vanquish   cavity 5282   limb volume 42587   ratio 0.12
    blackstar       cavity  306   limb volume  6114   ratio 0.05
    bot_lucifer     cavity  259   limb volume  8282   ratio 0.03

**The bay is roughly an order of magnitude too small.** That is the
central fact, and it is why no placement strategy has worked: there is
nowhere to put the limbs.

### Two caveats, both load-bearing

**The cavity number has not converged.** At a 40³ grid `tank_vanquish`
reported a ratio of 0.07; at 64³ it reports 0.12. A coarse voxelisation
leaks — the flood fill slips through gaps in the shell and eats interior
space that is really enclosed. Finer grids leak less and find more
cavity, and it is still climbing at 64³. Anyone acting on this should
push the resolution until the number stops moving. The ratio is
directionally right and its magnitude is not settled.

**The limbs are thin shells, not solid bodies.** Limb surface cells are
about 87% of limb volume cells, and both scale with G² rather than G³ as
the grid refines. These are plates and tubes enclosing air, not blocks
of metal. That distinction decides which of the ideas below can work.

## Option A — turtle and shell

Split the model: a skeleton that folds into a compact core stowed in the
engine box, plus a shell that is *not* part of the skeleton and stays
outside serving armour, weapon coverage and sightlines.

**What blocks it today:** `skeleton::compact` does not compact. It
translates each limb inward along its own axis by a fraction of its
reach — the limb slides toward the core and keeps every cubic
millimetre. Volume is conserved exactly, so no setting of its `factor`
can make limbs fit a bay an order of magnitude smaller.

Making this work needs machinery that does not exist: segmenting limbs at
joints and folding them onto themselves, or scaling them down during the
transform. Both are real features, not parameter changes.

**What makes it attractive:** the shell/skeleton split is a modelling
decision as much as a code one, and it matches how the reference reads.
Blackout's rotor mast, cockpit and tail are *shell* — they never fold,
they just have to look like a helicopter. Only the mechanism folds. The
current assets do not make that distinction, which is why `skeletonize`
classifies wheels and wings as limbs and the exposure metric cannot tell
a robot arm from a mudguard.

## Option B — high-energy shard transform

The shell energises, cubes up, breaks into magnetically bound voxels and
reflows. Faster unfolds cost more energy on some scale, so the
transform's duration becomes a combat resource.

**The thin-shell measurement is what makes this coherent.** Sliding a
limb has to move its whole swept volume, air included. Shattering it into
shards moves only the material, and these limbs are ~87% wall. A shard
repack is packing plates, not boxes, and the volume it needs is far
closer to the material volume than to the swept volume the placement
approaches keep failing against.

It also sidesteps the geometric problem entirely: shards do not need a
contiguous bay. They can fill the interstitial space between hull
components, which the cavity measurement above does not even count,
because it only tallies *enclosed* voids.

**Costs to go in with eyes open:** a shard simulation is a per-frame
physics problem where the current transform is a per-vertex lerp, and the
fold currently takes long enough that the excursion audit found parts
travelling most of a body-radius mid-transform. Adding N-body magnetics
to that is a different performance class.

## The energy budget idea

Making unfold speed cost energy is the part that ties the two options
together, and it is cheap to prototype because it needs no new geometry:

- slow / low energy → Option A behaviour, a deliberate mechanical fold
- fast / high energy → Option B, the shell shatters and reflows

That also gives the shard path a reason to exist beyond spectacle. If
both are implemented, the *same* chassis has two transforms with
different costs, and the choice is tactical.

## The staged decomposition

Both options above assume a single fold. Breaking the transform into
explicit stages changes the problem: see
[STAGED_TRANSFORM.md](STAGED_TRANSFORM.md). The volume gap becomes one
declared stage rather than something every other stage has to smuggle,
and the ratio measured here becomes that stage'''s target.

## What to measure next

1. **Push the cavity grid until the ratio converges.** The verdict for
   Option A rests on a number that is still moving.
2. **Interstitial space, not just enclosed cavity.** Option B can use
   gaps that are open to the outside but hidden from view; the current
   flood fill discards exactly those.
3. **Material volume with a real wall thickness**, rather than voxel
   surface cells, to size a shard budget honestly.
