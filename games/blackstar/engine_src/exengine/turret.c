#include "turret.h"
#include <math.h>
#include <string.h>

void ex_turret_deploy(ex_turret_t *t, const float pos[3], float life){
  memset(t,0,sizeof(*t));
  t->pos[0]=pos[0]; t->pos[1]=pos[1]+1.0f; t->pos[2]=pos[2];
  t->life=life; t->fire_timer=0.3f;
  t->range=60.0f; t->damage=18.0f; t->cooldown=0.4f;
  t->active=1; t->target_id=-1;
}

int ex_turret_pick(const ex_target_t *targets, int n, const float pos[3], float range){
  int best=-1; float bd=range*range;
  for (int i=0;i<n;i++){
    if (!targets[i].alive) continue;
    float dx=targets[i].center[0]-pos[0], dz=targets[i].center[2]-pos[2];
    float d=dx*dx+dz*dz;
    if (d<bd){ bd=d; best=i; }
  }
  return best;
}

int ex_turret_update(ex_turret_t *t, float dt, const ex_target_t *targets, int n){
  if (!t->active) return 0;
  t->life -= dt;
  if (t->life <= 0.0f){ t->active=0; return 0; }
  if (t->fire_timer > 0.0f){ t->fire_timer -= dt; return 0; }
  int idx = ex_turret_pick(targets, n, t->pos, t->range);
  if (idx < 0){ t->target_id=-1; return 0; }
  t->target_id = targets[idx].id;
  t->fire_timer = t->cooldown;
  return 1;
}
