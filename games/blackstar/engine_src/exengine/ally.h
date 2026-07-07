/* ally -- an Outer-Rim companion who fights alongside you (Forgeheart).
 *
 * From the film script: the Outer-Rim autobots stand with you -- Forgeheart wades
 * in, flipping wreckage to crush ambushes. In-game this is a single friendly unit
 * that follows you when things are quiet and engages the nearest enemy when they
 * aren't, chipping in without carrying the fight. If it takes too much it goes
 * down, then pulls itself back together after a while.
 *
 * This module is the pure brain -- target choice + mode + attack cadence + the
 * downed/revive clock. Movement, damage, and rendering are the caller's job.
 */
#ifndef EX_ALLY_H
#define EX_ALLY_H

#define EX_ALLY_ENGAGE_RANGE 48.0f
#define EX_ALLY_ATTACK_RANGE  6.5f
#define EX_ALLY_ATTACK_CD     1.1f
#define EX_ALLY_LEASH        16.0f
#define EX_ALLY_DOWN_TIME    12.0f

typedef enum { EX_ALLY_FOLLOW = 0, EX_ALLY_ENGAGE = 1, EX_ALLY_DOWN = 2 } ex_ally_mode_t;

typedef struct {
  int   active;
  float pos[3];
  float hp, max_hp;
  int   target;      /* enemy index, -1 = none */
  float attack_cd;
  float down_t;      /* remaining downed time */
  ex_ally_mode_t mode;
} ex_ally_t;

void ex_ally_reset(ex_ally_t *a, const float pos[3], float hp);

/* Nearest alive enemy to `self` within `range`; -1 if none. out_d2 (nullable)
   gets the squared distance to it (or range^2 if none). */
int ex_ally_nearest(const float self[3], const float (*epos)[3], const int *alive,
                    int n, float range, float *out_d2);

/* One decision step. Updates mode/target/attack_cd/down clock. Writes the goal
   point it wants to move toward into goal_out, and sets *want_attack=1 on the
   frame it lands a hit on its target. Returns the target enemy index (-1 none). */
int ex_ally_think(ex_ally_t *a, const float player[3], const float (*epos)[3],
                  const int *alive, int n, float dt, float goal_out[3], int *want_attack);

#endif /* EX_ALLY_H */
