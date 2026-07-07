/* cyclone -- the Watch's air form: a whirlwind.
 *
 * Rounds out the clone-power set (ice warp, fire star, water tsunami, electric
 * chain) with air. Cast it and a vortex spins up: caught enemies are swept into a
 * tangential orbit around its eye and dragged gently inward, whirled off their
 * attack for its duration, then flung loose. The orbiting force is the point --
 * nothing else in the game applies one.
 *
 * This module is the pure wind field: the velocity a vortex imparts to a point.
 * No globals, no rendering -> testable.
 */
#ifndef EX_CYCLONE_H
#define EX_CYCLONE_H

#define EX_CYCLONE_RADIUS   42.0f
#define EX_CYCLONE_DURATION 2.6f

/* Wind velocity for a point offset (dx,dz) from the vortex eye: a tangential
   swirl (counter-clockwise) plus a mild inward pull, both stronger near the eye
   and zero at/beyond `radius`. Writes out_vel (xz; y=0). Returns 1 if the point
   is inside the vortex (affected), 0 otherwise. */
int ex_cyclone_vel(float dx, float dz, float radius, float swirl, float pull,
                   float out_vel[3]);

#endif /* EX_CYCLONE_H */
