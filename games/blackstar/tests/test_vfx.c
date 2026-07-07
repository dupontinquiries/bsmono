/* test_vfx.c -- effect pool lifetimes, bolt->impact chaining, debris physics. */
#include "vfx.h"
#include <stdio.h>
#include <assert.h>
#include <math.h>

int main(void) {
  ex_vfx_pool_t p; ex_vfx_init(&p);
  assert(ex_vfx_count(&p) == 0);

  // firing spawns a flash + a bolt
  float from[3]={0,1,0}, dir[3]={0,0,1};
  uint8_t col[3]={120,200,255};
  ex_vfx_spawn_shot(&p, from, dir, 30.0f, col);
  int after_shot = ex_vfx_count(&p);
  printf("after shot: %d effects (flash + bolt)\n", after_shot);
  assert(after_shot == 2);

  // advance: the flash expires fast, the bolt travels 30 units then bursts into impact debris
  int saw_impact_debris = 0;
  for (int i=0;i<200;i++){
    ex_vfx_update(&p, 1.0f/60.0f);
    // detect debris presence (impact spawns debris)
    for (int k=0;k<EX_VFX_MAX;k++)
      if (p.items[k].active && p.items[k].kind==EX_VFX_DEBRIS) saw_impact_debris=1;
  }
  printf("bolt burst into impact debris: %d\n", saw_impact_debris);
  assert(saw_impact_debris);

  // everything should eventually expire
  for (int i=0;i<300;i++) ex_vfx_update(&p, 1.0f/60.0f);
  printf("after long run: %d effects (expect 0)\n", ex_vfx_count(&p));
  assert(ex_vfx_count(&p) == 0);

  // death burst spawns many debris that fall and settle on the ground
  float at[3]={5,1,5};
  ex_vfx_spawn_death(&p, at, col);
  int n = ex_vfx_count(&p);
  printf("death burst: %d debris\n", n);
  assert(n >= 12);
  // run a bit; debris should stay at/above ground (y>=0)
  for (int i=0;i<120;i++){
    ex_vfx_update(&p, 1.0f/60.0f);
    for (int k=0;k<EX_VFX_MAX;k++)
      if (p.items[k].active) assert(p.items[k].pos[1] >= 0.0f);
  }
  printf("debris respected the ground plane\n");

  // pool never overflows past capacity
  for (int s=0;s<50;s++) ex_vfx_spawn_death(&p, at, col);
  assert(ex_vfx_count(&p) <= EX_VFX_MAX);
  printf("pool capped at %d\n", EX_VFX_MAX);

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
