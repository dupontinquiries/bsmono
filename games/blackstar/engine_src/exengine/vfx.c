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
{ ex_vfx_spawn_shot_style(p, from, dir, dist, col, 0); }

void ex_vfx_spawn_shot_style(ex_vfx_pool_t *p, const float from[3], const float dir[3],
                             float dist, const uint8_t col[3], int style)
{
  ex_vfx_spawn_shot_style_v(p, from, dir, dist, col, style, NULL);
}

void ex_vfx_spawn_shot_style_v(ex_vfx_pool_t *p, const float from[3], const float dir[3],
                               float dist, const uint8_t col[3], int style, const float shooter_vel[3])
{
  // muzzle flash
  ex_vfx_t *f = alloc(p);
  if (f) {
    f->kind = EX_VFX_FLASH;
    f->pos[0]=from[0]; f->pos[1]=from[1]; f->pos[2]=from[2];
    f->life = f->max_life = (style==7) ? 0.13f : 0.08f;
    /* per-weapon muzzle presence: the sunspitter blooms, the haildriver spits */
    f->scale = 0.8f * ((style==7)?1.7f : (style==6)?1.35f : (style==2)?1.2f : (style==9)?0.6f : 1.0f);
    set_col(f, col);
  }
  // travelling bolt
  ex_vfx_t *b = alloc(p);
  if (b) {
    /* per-mode ballistic feel: plasma lumbers, burst/suppress snap. Floor raised
     * so no round trails a boosted flyer (~42 u/s top). */
    float speed = 150.0f;
    if (style==7) speed = 95.0f;                    /* plasma: heavy, watchable, still outruns a jet */
    else if (style==2) speed = 340.0f;              /* beam: a LANCE -- near-instant light */
    else if (style==1 || style==9) speed = 165.0f;  /* burst / suppress: snappy */
    else if (style==8) speed = 175.0f;              /* ricochet: a whipcrack */
    else if (style==6) speed = 115.0f;              /* flak: chunky but not sluggish */
    else if (style==0) speed = 150.0f;              /* neutron blaster: authoritative */
    b->kind = EX_VFX_BOLT;
    b->pos[0]=from[0]; b->pos[1]=from[1]; b->pos[2]=from[2];
    /* inherit the shooter's velocity so a shot fired from a moving platform
     * tracks with it (no crawl/drift), then normalise back to the mode speed
     * along the aim so range/lifetime stay correct. */
    float vx=dir[0]*speed, vy=dir[1]*speed, vz=dir[2]*speed;
    if (shooter_vel){ vx+=shooter_vel[0]; vy+=shooter_vel[1]; vz+=shooter_vel[2]; }
    float vm=sqrtf(vx*vx+vy*vy+vz*vz);
    if (vm>1e-3f){ float k=speed/vm; vx*=k; vy*=k; vz*=k; }   /* keep muzzle speed constant */
    b->vel[0]=vx; b->vel[1]=vy; b->vel[2]=vz;
    b->end_dist = dist;
    b->travelled = 0.0f;
    b->life = b->max_life = (dist / speed) + 0.01f;
    b->scale = (style==7) ? 0.62f : 0.38f;
    b->style = style;
    set_col(b, col);
  }
}

void ex_vfx_spawn_smoke(ex_vfx_pool_t *p, const float at[3], const uint8_t col[3], float scale)
{
  ex_vfx_t *k = alloc(p);
  if (!k) return;
  k->kind = EX_VFX_SMOKE;
  k->pos[0]=at[0]; k->pos[1]=at[1]; k->pos[2]=at[2];
  k->vel[0]=0.0f; k->vel[1]=1.6f; k->vel[2]=0.0f;   /* lazy rise */
  k->life = k->max_life = 0.85f;
  k->scale = scale;
  set_col(k, col);
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
      float prevtrav = v->travelled;
      v->pos[0]+=step[0]; v->pos[1]+=step[1]; v->pos[2]+=step[2];
      v->travelled += sqrtf(step[0]*step[0]+step[1]*step[1]+step[2]*step[2]);
      /* trails by style: flak sheds grey SMOKE every ~5u; plasma leaves a glow
         wake every ~4u (own colour); lob handled by the game (arcing body). */
      { float scad2 = (v->style==0||v->style==1||v->style==2||v->style==5||v->style==8||v->style==9) ? 3.0f : 0.0f;
        if (scad2 > 0.0f && ((int)(v->travelled/scad2) != (int)(prevtrav/scad2))) {
          ex_vfx_t *st2 = alloc(p);
          if (st2){ st2->kind = EX_VFX_STREAK;
            st2->pos[0]=v->pos[0]; st2->pos[1]=v->pos[1]; st2->pos[2]=v->pos[2];
            st2->vel[0]=v->vel[0]; st2->vel[1]=v->vel[1]; st2->vel[2]=v->vel[2];   /* orientation only */
            st2->life = st2->max_life = (v->style==2)?0.30f:0.20f;                 /* the lance lingers */
            st2->scale = (v->style==5)?0.30f:0.24f;
            st2->style = v->style;
            set_col(st2, v->color); }
        } }
      { float cad2 = (v->style==6) ? 5.0f : (v->style==7) ? 4.0f : 0.0f;
        if (cad2 > 0.0f && ((int)(v->travelled/cad2) != (int)(prevtrav/cad2))) {
          uint8_t tc3[3];
          if (v->style==6){ tc3[0]=90; tc3[1]=88; tc3[2]=84; }               /* powder smoke */
          else { tc3[0]=v->color[0]; tc3[1]=v->color[1]; tc3[2]=v->color[2]; } /* plasma wake */
          ex_vfx_spawn_smoke(p, v->pos, tc3, (v->style==6)?0.5f:0.38f);
        } }
      if (v->travelled >= v->end_dist || v->life <= 0.0f) {
        // burst into an impact at the current position
        ex_vfx_spawn_impact(p, v->pos, 6, v->color);
        v->active = 0;
      }
      continue;
    }

    if (v->kind == EX_VFX_STREAK) { /* stationary afterimage: only fades */ }

    if (v->kind == EX_VFX_SMOKE) {
      v->pos[0]+=v->vel[0]*dt; v->pos[1]+=v->vel[1]*dt; v->pos[2]+=v->vel[2]*dt;
      v->vel[1] *= (1.0f - 0.8f*dt);   /* the rise stalls as it cools */
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
