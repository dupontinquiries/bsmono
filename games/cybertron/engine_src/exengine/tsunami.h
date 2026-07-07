/* tsunami -- the Watch's water form unleashed.
 *
 * From the film script the Watch cycles elemental forms; water's signature is the
 * tsunami -- a wall of water that surges out and sweeps everything before it. It
 * completes the clone-power triad alongside the ice leap and the fire star.
 *
 * Unlike the radial warp wave, this is DIRECTIONAL: a front that advances from an
 * origin along a heading, cresting forward until it spends itself. This module is
 * the pure front tracker -- arm it, tick it, read the front position. The caller
 * applies the shove/damage and draws the water.
 */
#ifndef EX_TSUNAMI_H
#define EX_TSUNAMI_H

#define EX_TSUNAMI_SPEED 55.0f    /* units/sec the crest advances */
#define EX_TSUNAMI_RANGE 92.0f    /* distance it travels before dissipating */

typedef struct {
  int   active;
  float origin[3];
  float dir[3];       /* normalized xz heading */
  float travelled;    /* how far the crest has advanced */
} ex_tsunami_t;

/* Arm a wave at `origin` heading `dir` (xz is normalized; a zero dir is ignored). */
void ex_tsunami_arm(ex_tsunami_t *t, const float origin[3], const float dir[3]);

/* Advance the crest by dt. Writes the current crest center into front_out.
   Returns 1 while active (including the frame it ends), 0 once spent. */
int ex_tsunami_tick(ex_tsunami_t *t, float dt, float front_out[3]);

/* Crest progress 0..1 across its range (0 if inactive). */
float ex_tsunami_progress(const ex_tsunami_t *t);

#endif /* EX_TSUNAMI_H */
