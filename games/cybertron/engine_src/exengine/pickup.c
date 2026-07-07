#include "pickup.h"
#include <string.h>
#include <math.h>

void ex_pickup_field_init(ex_pickup_field_t *f){ memset(f,0,sizeof(*f)); }

int ex_pickup_add(ex_pickup_field_t *f, ex_pickup_kind kind, float x, float y, float z){
  if (f->count >= EX_MAX_PICKUPS) return -1;
  int i = f->count++;
  f->items[i].kind = kind;
  f->items[i].pos[0]=x; f->items[i].pos[1]=y; f->items[i].pos[2]=z;
  f->items[i].t = (float)i * 0.6f;
  f->items[i].alive = 1;
  return i;
}

void ex_pickup_update(ex_pickup_field_t *f, float dt){
  for (int i=0;i<f->count;i++) if (f->items[i].alive) f->items[i].t += dt;
}

float ex_pickup_bob(const ex_pickup_t *p){ return sinf(p->t*2.2f)*0.15f + 0.6f; }

int ex_pickup_try_collect(ex_pickup_field_t *f, float px, float py, float pz, float reach){
  for (int i=0;i<f->count;i++){
    ex_pickup_t *p = &f->items[i];
    if (!p->alive) continue;
    float dx=px-p->pos[0], dz=pz-p->pos[2];
    if (dx*dx+dz*dz > reach*reach) continue;
    if (fabsf(py - p->pos[1]) > 3.0f) continue;
    p->alive = 0;
    return (int)p->kind;
  }
  return -1;
}

void ex_pickup_color(ex_pickup_kind kind, uint8_t c[3]){
  switch(kind){
    case EX_PICKUP_ENERGON:   c[0]=70;c[1]=230;c[2]=120; break; // green
    case EX_PICKUP_SHIELD:    c[0]=96;c[1]=156;c[2]=255; break; // blue
    case EX_PICKUP_OVERCHARGE: c[0]=255;c[1]=200;c[2]=90; break; // gold
    case EX_PICKUP_MATRIX:    c[0]=255;c[1]=240;c[2]=170; break; // radiant gold-white
    default: c[0]=255;c[1]=255;c[2]=255; break;
  }
}
const char* ex_pickup_name(ex_pickup_kind kind){
  switch(kind){ case EX_PICKUP_ENERGON: return "energon"; case EX_PICKUP_SHIELD: return "shield";
    case EX_PICKUP_OVERCHARGE: return "overcharge"; case EX_PICKUP_MATRIX: return "the matrix"; default: return "?"; }
}
