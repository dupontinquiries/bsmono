#include "ally.h"
#include <math.h>

void ex_ally_reset(ex_ally_t *a, const float pos[3], float hp)
{
  a->active = 1;
  a->pos[0] = pos[0]; a->pos[1] = pos[1]; a->pos[2] = pos[2];
  a->hp = a->max_hp = hp;
  a->target = -1; a->attack_cd = 0.0f; a->down_t = 0.0f;
  a->mode = EX_ALLY_FOLLOW;
}

int ex_ally_nearest(const float self[3], const float (*epos)[3], const int *alive,
                    int n, float range, float *out_d2)
{
  int best = -1; float bd = range * range;
  for (int i = 0; i < n; i++) {
    if (alive && !alive[i]) continue;
    float dx = epos[i][0] - self[0], dz = epos[i][2] - self[2];
    float d2 = dx*dx + dz*dz;
    if (d2 < bd) { bd = d2; best = i; }
  }
  if (out_d2) *out_d2 = (best >= 0) ? bd : range * range;
  return best;
}

int ex_ally_think(ex_ally_t *a, const float player[3], const float (*epos)[3],
                  const int *alive, int n, float dt, float goal_out[3], int *want_attack)
{
  if (want_attack) *want_attack = 0;
  goal_out[0] = a->pos[0]; goal_out[1] = a->pos[1]; goal_out[2] = a->pos[2];
  if (!a->active) return -1;

  /* downed: hold, then pull back together */
  if (a->hp <= 0.0f) {
    if (a->mode != EX_ALLY_DOWN) { a->mode = EX_ALLY_DOWN; a->down_t = EX_ALLY_DOWN_TIME; a->target = -1; }
    a->down_t -= dt;
    if (a->down_t <= 0.0f) { a->hp = a->max_hp; a->mode = EX_ALLY_FOLLOW; }
    return -1;
  }

  if (a->attack_cd > 0.0f) a->attack_cd -= dt;

  float d2;
  int tgt = ex_ally_nearest(a->pos, epos, alive, n, EX_ALLY_ENGAGE_RANGE, &d2);
  if (tgt >= 0) {
    a->mode = EX_ALLY_ENGAGE; a->target = tgt;
    goal_out[0] = epos[tgt][0]; goal_out[2] = epos[tgt][2];
    if (d2 <= EX_ALLY_ATTACK_RANGE * EX_ALLY_ATTACK_RANGE && a->attack_cd <= 0.0f) {
      if (want_attack) *want_attack = 1;
      a->attack_cd = EX_ALLY_ATTACK_CD;
    }
    return tgt;
  }

  /* nothing near: stick with the player, but only close the gap past the leash */
  a->mode = EX_ALLY_FOLLOW; a->target = -1;
  float px = player[0] - a->pos[0], pz = player[2] - a->pos[2];
  float pd = sqrtf(px*px + pz*pz);
  if (pd > EX_ALLY_LEASH) { goal_out[0] = player[0]; goal_out[2] = player[2]; }
  return -1;
}
