/* terrain_step.h -- the character-vs-AABB step/collision resolve, extracted so
 * both the game and the offline test harness run byte-identical logic. The
 * resolve is: for each solid box, if the character is within the XZ footprint
 * (expanded by the collider radius), either CLAMP onto its top surface (a step)
 * when the feet are near the top, or PUSH OUT along the least-penetration
 * horizontal axis. */
#ifndef EX_TERRAIN_STEP_H
#define EX_TERRAIN_STEP_H

typedef struct { float center[3]; float half[3]; } ex_step_box_t;

/* Step-up tolerance: feet within this distance below a box top snap up onto it. */
#define EX_STEP_UP_TOL   0.35f
/* Snap window above the top within which we still clamp down onto the surface. */
#define EX_STEP_SNAP_TOL 0.6f

/* Resolve pos/vel against one box. `prev_y` is the character's vertical position
 * BEFORE this frame's move, enabling a swept landing test (did the feet cross the
 * box top this frame?). Returns 1 if the character stood on its top. */
int ex_step_resolve_box(float pos[3], float vel[3], float pr, const ex_step_box_t *b, float prev_y);

/* Resolve against n boxes in order. `prev_y` as above. Returns 1 if grounded. */
int ex_step_resolve(float pos[3], float vel[3], float pr, const ex_step_box_t *boxes, int n, float prev_y);

/* Climb probe: is there a climbable WALL directly ahead of `pos` along the
 * horizontal facing (dx,dz)? A wall is a box whose face the character is pressed
 * against (within pr+reach), whose top rises at least `min_rise` above the
 * character's feet, and whose top the character is still below. Returns 1 and
 * writes the wall's top Y to *out_top (may be NULL). Pure geometry, no state. */
int ex_step_wall_ahead(const float pos[3], float dx, float dz, float pr,
                       const ex_step_box_t *boxes, int n,
                       float reach, float min_rise, float *out_top);

#endif
