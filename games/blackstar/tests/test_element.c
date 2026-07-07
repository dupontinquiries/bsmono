/* test_element.c -- elemental advantage cycle, the Watch, and Corrupted AI. */
#include "element.h"
#include "enemy.h"
#include "coll_chunk.h"
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include <string.h>

int main(void) {
  printf("=== element advantage cycle ===\n");
  // each element should have exactly two strong and two weak matchups
  for (int a = EX_ELEM_ICE; a < EX_ELEM_COUNT; a++) {
    int strong = 0, weak = 0, neutral = 0;
    for (int d = EX_ELEM_ICE; d < EX_ELEM_COUNT; d++) {
      float m = ex_element_advantage(a, d);
      if (m > 1.1f) strong++; else if (m < 0.9f) weak++; else neutral++;
    }
    printf("  %-8s: %d strong, %d weak, %d neutral\n", ex_element_name(a), strong, weak, neutral);
    assert(strong == 2 && weak == 2);
  }
  // fire beats vine/air region, loses to water/rock region -- just sanity that it's not symmetric
  assert(ex_element_advantage(EX_ELEM_FIRE, EX_ELEM_ICE) != ex_element_advantage(EX_ELEM_ICE, EX_ELEM_FIRE));

  printf("=== element profiles differ ===\n");
  ex_element_profile_t fire = ex_element_profile(EX_ELEM_FIRE);
  ex_element_profile_t elec = ex_element_profile(EX_ELEM_ELECTRIC);
  printf("  fire dmg x%.2f range x%.2f | electric dmg x%.2f range x%.2f\n",
         fire.damage_mul, fire.range_mul, elec.damage_mul, elec.range_mul);
  assert(fire.damage_mul > elec.damage_mul);   // fire hits harder
  assert(elec.range_mul > fire.range_mul);      // electric reaches further

  printf("=== the Watch (element switching) ===\n");
  ex_watch_t w = ex_watch_new(EX_ELEM_ICE);
  assert(ex_watch_active(&w) == EX_ELEM_ICE);
  int ok = ex_watch_switch(&w, EX_ELEM_FIRE);
  printf("  switch ice->fire accepted=%d, active during attune=%s\n", ok, ex_element_name(ex_watch_active(&w)));
  assert(ok == 1);
  assert(ex_watch_active(&w) == EX_ELEM_NONE); // attuning -> no element active
  // can't switch again mid-attune
  assert(ex_watch_switch(&w, EX_ELEM_ROCK) == 0);
  // advance past attune
  for (float t = 0; t < EX_WATCH_ATTUNE_SEC + 0.1f; t += 0.1f) ex_watch_update(&w, 0.1f);
  printf("  after attune: active=%s (expect fire)\n", ex_element_name(ex_watch_active(&w)));
  assert(ex_watch_active(&w) == EX_ELEM_FIRE);
  // cycle
  ex_watch_cycle(&w);
  for (float t = 0; t < EX_WATCH_ATTUNE_SEC + 0.1f; t += 0.1f) ex_watch_update(&w, 0.1f);
  printf("  after cycle: active=%s (expect air)\n", ex_element_name(ex_watch_active(&w)));
  assert(ex_watch_active(&w) == EX_ELEM_AIR);

  printf("=== Corrupted enemy (horror: no LOS, relentless) ===\n");
  ex_chunk_world_t *world = ex_chunk_world_new();
  // wall between corrupted and player -- a Grunt would stay idle; Corrupted won't
  ex_chunk_add(world, "wall", ex_rect_new((vec3){-3,0,9}, (vec3){3,4,11}),
               (vec3[6]){{0,0,0},{1,0,0},{1,1,0},{0,0,0},{1,1,0},{0,1,0}}, 2, 100.0f, EX_CHUNK_HIT_ANY, 1);
  vec3 player = {0,1,0};
  ex_enemy_t grunt = ex_enemy_new_kind(1, (vec3){0,1,30}, 100.0f, EX_ENEMY_GRUNT, EX_ELEM_ICE);
  ex_enemy_t corr  = ex_enemy_new_kind(2, (vec3){0,1,30}, 100.0f, EX_ENEMY_CORRUPTED, EX_ELEM_FIRE);
  ex_enemy_update(&grunt, world, player, 0.1f);
  ex_enemy_update(&corr,  world, player, 0.1f);
  printf("  behind a wall: grunt=%s corrupted=%s\n",
         ex_enemy_state_name(grunt.state), ex_enemy_state_name(corr.state));
  assert(grunt.state == EX_ENEMY_IDLE);   // no LOS -> idle
  assert(corr.state == EX_ENEMY_CHASE);   // ignores LOS -> hunting

  // corrupted closes distance over time
  float start_dist = corr.position[2] - player[2];
  for (int i = 0; i < 300; i++) ex_enemy_update(&corr, world, player, 1.0f/60.0f);
  float end_dist = corr.position[2] - player[2];
  printf("  corrupted closed from %.0f to %.0f units\n", start_dist, end_dist);
  assert(end_dist < start_dist);

  ex_chunk_world_free(world);
  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
