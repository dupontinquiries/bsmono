#include "vfx.h"
#include <string.h>
#include <math.h>

static uint32_t s_rng = 0x1234567u;
static float frand(void)
{
  s_rng ^= s_rng << 13; s_rng ^= s_rng >> 17; s_rng ^= s_rng << 5;
  return (float)(s_rng & 0xFFFFFF) / (float)0x1000000;
}
static float frand_s(void) { return frand() * 2.0f - 1.0f; }

void ex_vfx_init(ex_vfx_pool_t *p)
{
  memset(p, 0, sizeof(*p));
}

static ex_vfx_t* alloc(ex_vfx_pool_t *p)
{
  for (int i = 0; i < EX_VFX_MAX; i++)
    if (!p->items[i].active) { memset(&p->items[i], 0, sizeof(ex_vfx_t)); p->items[i].active = 1; return &p->items[i]; }
  return NULL; // pool full -> drop the effect
}

static void set_col(ex_vfx_t *v, const uint8_t col[3])
{
  if (col) { v->color[0]=col[0]; v->color[1]=col[1]; v->color[2]=col[2]; }
  else     { v->color[0]=255; v->color[1]=220; v->color[2]=120; }
}

void ex_vfx_spawn_shot(ex_vfx_pool_t *p, const float from[3], const float dir[3],
                       float dist, const uint8_t col[3])
{
  // muzzle flash
  ex_vfx_t *f = alloc(p);
  if (f) {
    f->kind = EX_VFX_FLASH;
    f->pos[0]=from[0]; f->pos[1]=from[1]; f->pos[2]=from[2];
    f->life = f->max_life = 0.08f;
    f->scale = 0.8f;
    set_col(f, col);
  }
  // travelling bolt
  ex_vfx_t *b = alloc(p);
  if (b) {
    const float speed = 115.0f;
    b->kind = EX_VFX_BOLT;
    b->pos[0]=from[0]; b->pos[1]=from[1]; b->pos[2]=from[2];
    b->vel[0]=dir[0]*speed; b->vel[1]=dir[1]*speed; b->vel[2]=dir[2]*speed;
    b->end_dist = dist;
    b->travelled = 0.0f;
    b->life = b->max_life = (dist / speed) + 0.01f;
    b->scale = 0.45f;
    set_col(b, col);
  }
}

void ex_vfx_spawn_impact(ex_vfx_pool_t *p, const float at[3], int count, const uint8_t col[3])
{
  for (int i = 0; i < count; i++) {
    ex_vfx_t *d = alloc(p);
    if (!d) return;
    d->kind = EX_VFX_DEBRIS;
    d->pos[0]=at[0]; d->pos[1]=at[1]; d->pos[2]=at[2];
    float sp = 3.0f + frand()*5.0f;
    d->vel[0]=frand_s()*sp; d->vel[1]=fabsf(frand())*sp; d->vel[2]=frand_s()*sp;
    d->gravity = 1;
    d->life = d->max_life = 0.4f + frand()*0.3f;
    d->scale = 0.08f + frand()*0.08f;
    set_col(d, col);
  }
}

void ex_vfx_spawn_death(ex_vfx_pool_t *p, const float at[3], const uint8_t col[3])
{
  // a bigger, chunkier burst
  for (int i = 0; i < 16; i++) {
    ex_vfx_t *d = alloc(p);
    if (!d) return;
    d->kind = EX_VFX_DEBRIS;
    d->pos[0]=at[0]; d->pos[1]=at[1]+1.0f; d->pos[2]=at[2];
    float sp = 4.0f + frand()*7.0f;
    d->vel[0]=frand_s()*sp; d->vel[1]=2.0f+fabsf(frand())*sp; d->vel[2]=frand_s()*sp;
    d->gravity = 1;
    d->life = d->max_life = 0.8f + frand()*0.6f;
    d->scale = 0.14f + frand()*0.16f;
    set_col(d, col);
  }
}

void ex_vfx_update(ex_vfx_pool_t *p, float dt)
{
  for (int i = 0; i < EX_VFX_MAX; i++) {
    ex_vfx_t *v = &p->items[i];
    if (!v->active) continue;

    v->life -= dt;

    if (v->kind == EX_VFX_BOLT) {
      float step[3] = { v->vel[0]*dt, v->vel[1]*dt, v->vel[2]*dt };
      v->pos[0]+=step[0]; v->pos[1]+=step[1]; v->pos[2]+=step[2];
      v->travelled += sqrtf(step[0]*step[0]+step[1]*step[1]+step[2]*step[2]);
      if (v->travelled >= v->end_dist || v->life <= 0.0f) {
        // burst into an impact at the current position
        ex_vfx_spawn_impact(p, v->pos, 6, v->color);
        v->active = 0;
      }
      continue;
    }

    if (v->kind == EX_VFX_DEBRIS && v->gravity) {
      v->vel[1] -= 22.0f * dt;
      v->pos[0]+=v->vel[0]*dt; v->pos[1]+=v->vel[1]*dt; v->pos[2]+=v->vel[2]*dt;
      if (v->pos[1] < 0.05f) { v->pos[1] = 0.05f; v->vel[1] *= -0.3f; v->vel[0]*=0.6f; v->vel[2]*=0.6f; }
    }

    if (v->life <= 0.0f) v->active = 0;
  }
}

int ex_vfx_count(const ex_vfx_pool_t *p)
{
  int n = 0;
  for (int i = 0; i < EX_VFX_MAX; i++) if (p->items[i].active) n++;
  return n;
}
