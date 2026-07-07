#include "exengine/material.h"
#include <stdio.h>
#include <assert.h>
int main(void){
  // ice is much more slippery than metal
  float gm=ex_material_grip(EX_MAT_METAL,1,0,1,0), gi=ex_material_grip(EX_MAT_ICE,1,0,1,0);
  assert(gi < gm*0.6f);
  // loose sand loses grip at speed
  float s0=ex_material_grip(EX_MAT_SAND,1,0,1,0), s1=ex_material_grip(EX_MAT_SAND,1,24,1,0);
  assert(s1 < s0);
  // leaning off the footprint reduces grip
  float f0=ex_material_grip(EX_MAT_METAL,1,10,1,0.0f), f1=ex_material_grip(EX_MAT_METAL,1,10,1,0.6f);
  assert(f1 < f0);
  // theme mapping
  assert(ex_material_for_theme("ice_field")==EX_MAT_ICE);
  assert(ex_material_for_theme("cybertron")==EX_MAT_METAL);
  printf("material: friction from moisture/grit/stance/lean OK\n");
  printf("ALL ASSERTIONS PASSED\n");
  return 0;
}
