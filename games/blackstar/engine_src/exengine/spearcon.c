#include "spearcon.h"

void ex_spearcon_reset(ex_spearcon_pool_t *p)
{
  if (!p) return;
  for (int i = 0; i < EX_SPEARCON_MAX; i++) p->s[i].active = 0;
}

int ex_spearcon_plant(ex_spearcon_pool_t *p, const float pos[3], float fuse)
{
  if (!p) return -1;
  for (int i = 0; i < EX_SPEARCON_MAX; i++) {
    if (!p->s[i].active) {
      p->s[i].active = 1;
      p->s[i].pos[0] = pos[0]; p->s[i].pos[1] = pos[1]; p->s[i].pos[2] = pos[2];
      p->s[i].fuse = (fuse > 0.05f) ? fuse : 0.05f;
      p->s[i].t = 0.0f;
      return i;
    }
  }
  return -1;   /* pool full */
}

int ex_spearcon_update(ex_spearcon_pool_t *p, float dt, float out_det[][3], int max_out)
{
  if (!p) return 0;
  int n = 0;
  for (int i = 0; i < EX_SPEARCON_MAX; i++) {
    ex_spearcon_t *s = &p->s[i];
    if (!s->active) continue;
    s->t += dt;
    if (s->t >= s->fuse) {
      s->active = 0;
      if (out_det && n < max_out) {
        out_det[n][0] = s->pos[0]; out_det[n][1] = s->pos[1]; out_det[n][2] = s->pos[2];
      }
      n++;
    }
  }
  return n;
}

float ex_spearcon_progress(const ex_spearcon_t *s)
{
  if (!s || !s->active) return 0.0f;
  float p = s->t / s->fuse;
  if (p < 0.0f) p = 0.0f;
  if (p > 1.0f) p = 1.0f;
  return p;
}

int ex_spearcon_active_count(const ex_spearcon_pool_t *p)
{
  if (!p) return 0;
  int n = 0;
  for (int i = 0; i < EX_SPEARCON_MAX; i++) if (p->s[i].active) n++;
  return n;
}
