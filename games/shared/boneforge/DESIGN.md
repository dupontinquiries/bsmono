# Design principles

## 1. The robot form specialises away from what the vehicle does

The two forms divide the work; they do not duplicate it. The vehicle owns
speed and distance. The mass budget is fixed -- it is the same machine either
way -- so every kilogram a robot form spends on long running legs is a
kilogram not spent on reach, shoulders, bracing or a weapon.

This drives the DERIVED properties of any transformation, from any source:

  - Skeleton proportions come from a Role (Brawler, Gunner, ...), not from a
    single humanoid template. Legs are sized for footwork and for standing up
    out of the vehicle, not for stride.
  - Where the vehicle's mass goes in robot form follows the role: arms and
    shoulders for a brawler, shoulders and a braced stance for a gunner.
  - Stability is bought with FEET and stance, not with squat proportions.
    Measured: moving leg mass into chest and arms RAISES the centre of mass
    (Generic 50% -> Brawler 48%, barely moved). The gunner nearly doubles its
    brace (6% -> 11%) almost entirely through foot length.
  - The transform is the closing move. Drive in at speed, transform at contact
    range; the vehicle's momentum carries into the first strike.

What legs still must do, so they never shrink to nothing: stand the machine up
out of the folded vehicle, and carry the jump thrusters.

    role      legs   arm:leg   CoM    brace
    Generic    47%     0.91    50%     6%
    Brawler    29%     1.86    48%     9%
    Gunner     32%     1.41    48%    11%

Reference target: a compact, broad-bodied robot stands on legs ~27% of its height.
