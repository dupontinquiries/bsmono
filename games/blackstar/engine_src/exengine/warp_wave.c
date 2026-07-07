#include "warp_wave.h"

void ex_warp_wave_arm(ex_warp_wave_t *w, const float pos[3], float delay)
{
  if (!w) return;
  w->armed = 1;
  w->timer = delay;
  w->delay = (delay > 0.0001f) ? delay : 0.0001f;
  w->pos[0] = pos[0]; w->pos[1] = pos[1]; w->pos[2] = pos[2];
}

int ex_warp_wave_tick(ex_warp_wave_t *w, float dt)
{
  if (!w || !w->armed) return 0;
  w->timer -= dt;
  if (w->timer <= 0.0f) {
    w->armed = 0;
    w->timer = 0.0f;
    return 1;   /* fires exactly once */
  }
  return 0;
}

float ex_warp_wave_progress(const ex_warp_wave_t *w)
{
  if (!w || !w->armed) return 0.0f;
  float p = 1.0f - (w->timer / w->delay);
  if (p < 0.0f) p = 0.0f;
  if (p > 1.0f) p = 1.0f;
  return p;
}
