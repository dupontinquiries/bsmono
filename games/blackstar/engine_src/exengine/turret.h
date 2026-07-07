/* turret
  A deployable stationary auto-gun (the TURRET ability). Once deployed it
  acquires the nearest live target in range and fires on a cooldown for its
  lifetime, then expires. Pure logic (target pick + timers); the game applies
  damage and spawns the tracer VFX.
*/
#ifndef EX_TURRET_H
#define EX_TURRET_H

#include "combat.h"

typedef struct {
  float pos[3];
  float life;        // remaining active time
  float fire_timer;  // counts down to next shot
  float range;
  float damage;
  float cooldown;
  int   active;
  int   target_id;   // last acquired target (for the game to damage)
} ex_turret_t;

void ex_turret_deploy(ex_turret_t *t, const float pos[3], float life);

/**
 * [ex_turret_pick nearest live target within range; returns index or -1]
 */
int ex_turret_pick(const ex_target_t *targets, int n, const float pos[3], float range);

/**
 * [ex_turret_update tick life + fire timer; if a shot is ready and a target is
 *  in range, set target_id and return 1 (the game fires), else 0]
 */
int ex_turret_update(ex_turret_t *t, float dt, const ex_target_t *targets, int n);

#endif // EX_TURRET_H
