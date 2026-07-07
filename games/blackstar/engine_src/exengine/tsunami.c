#include "tsunami.h"
#include <math.h>

void ex_tsunami_arm(ex_tsunami_t *t, const float origin[3], const float dir[3])
{
  if (!t) return;
  float dl = sqrtf(dir[0]*dir[0] + dir[2]*dir[2]);
  if (dl < 1e-4f) return;                       /* need a heading */
  t->dir[0] = dir[0]/dl; t->dir[1] = 0.0f; t->dir[2] = dir[2]/dl;
  t->origin[0] = origin[0]; t->origin[1] = origin[1]; t->origin[2] = origin[2];
  t->travelled = 0.0f;
  t->active = 1;
}

int ex_tsunami_tick(ex_tsunami_t *t, float dt, float front_out[3])
{
  if (!t || !t->active) {
    if (front_out && t) { front_out[0]=t->origin[0]; front_out[1]=t->origin[1]; front_out[2]=t->origin[2]; }
    return 0;
  }
  t->travelled += EX_TSUNAMI_SPEED * dt;
  float d = t->travelled; if (d > EX_TSUNAMI_RANGE) d = EX_TSUNAMI_RANGE;
  if (front_out) {
    front_out[0] = t->origin[0] + t->dir[0]*d;
    front_out[1] = t->origin[1];
    front_out[2] = t->origin[2] + t->dir[2]*d;
  }
  if (t->travelled >= EX_TSUNAMI_RANGE) { t->active = 0; }   /* spent this frame */
  return 1;
}

float ex_tsunami_progress(const ex_tsunami_t *t)
{
  if (!t || !t->active) return 0.0f;
  float p = t->travelled / EX_TSUNAMI_RANGE;
  return p < 0.0f ? 0.0f : (p > 1.0f ? 1.0f : p);
}
