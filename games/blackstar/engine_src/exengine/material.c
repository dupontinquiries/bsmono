#include "material.h"
#include <string.h>
#include <math.h>

static const ex_material_t MATS[EX_MAT_COUNT] = {
  /* name        fric  rest  hard  moist grit  spark        */
  { "metal",     1.05f,0.35f,1.00f,0.00f,0.05f, {255,225,150} },
  { "rock",      0.90f,0.20f,0.85f,0.05f,0.45f, {200,170,130} },
  { "ice",       0.32f,0.45f,0.90f,0.85f,0.02f, {180,220,255} },
  { "sand",      0.78f,0.08f,0.30f,0.10f,0.90f, {210,190,140} },
  { "organic",   0.70f,0.15f,0.25f,0.65f,0.20f, {150,220,140} },
  { "crystal",   0.95f,0.55f,1.00f,0.00f,0.10f, { 90,230,255} },
};

const ex_material_t *ex_material(ex_material_id id){
  if (id<0 || id>=EX_MAT_COUNT) id=EX_MAT_METAL;
  return &MATS[id];
}

ex_material_id ex_material_for_theme(const char *theme){
  if (!theme) return EX_MAT_METAL;
  if (strstr(theme,"ice")||strstr(theme,"frost"))                 return EX_MAT_ICE;
  if (strstr(theme,"waste")||strstr(theme,"dune")||strstr(theme,"desert")) return EX_MAT_SAND;
  if (strstr(theme,"ember")||strstr(theme,"foundry")||strstr(theme,"forge")) return EX_MAT_METAL;
  if (strstr(theme,"reclaim")||strstr(theme,"jungle")||strstr(theme,"organic")) return EX_MAT_ORGANIC;
  if (strstr(theme,"energon")||strstr(theme,"crystal"))            return EX_MAT_CRYSTAL;
  if (strstr(theme,"cyber")||strstr(theme,"vault")||strstr(theme,"metal")) return EX_MAT_METAL;
  return EX_MAT_ROCK;
}

float ex_material_grip(ex_material_id id, float mass, float speed,
                       float foot_area, float lean){
  const ex_material_t *m = ex_material(id);
  float g = m->friction;
  g *= (1.0f - m->moisture*0.55f);                 /* wet -> slippery */
  g *= (1.0f - m->grit * fminf(speed/24.0f,1.0f)*0.5f); /* loose grit lets go at speed */
  g *= (0.6f + foot_area*0.5f);                    /* a wide planted stance grips more */
  g *= (1.0f - fminf(fabsf(lean),0.6f)*0.5f);      /* leaning off the footprint loses grip */
  g *= (0.85f + 0.15f*fminf(mass,2.0f));           /* heavier presses down -> a bit more grip */
  if (g<0.15f) g=0.15f; if (g>1.4f) g=1.4f;
  return g;
}
