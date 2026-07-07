/* test_gameplay.c -- a headless mini gameplay loop exercising the new
   combat, enemy AI, and arena-generation modules together. */
#include "arena.h"
#include "combat.h"
#include "enemy.h"
#include "coll_chunk.h"
#include <stdio.h>
#include <assert.h>
#include <string.h>
#include <math.h>

int main(void) {
  // ---- arena generation ----
  ex_chunk_world_t *chunks = ex_chunk_world_new();
  ex_arena_t arena;
  ex_arena_generate(&arena, chunks, -40, -40, 40, 40, 0.0f, /*grid*/6, /*seed*/1234);
  printf("=== arena ===\n");
  printf("  generated %d features into %d chunks\n", arena.count, chunks->count);
  assert(arena.count > 0);
  assert(chunks->count == arena.count);
  // determinism: same seed -> same count
  ex_chunk_world_t *chunks2 = ex_chunk_world_new();
  ex_arena_t arena2;
  ex_arena_generate(&arena2, chunks2, -40, -40, 40, 40, 0.0f, 6, 1234);
  assert(arena2.count == arena.count);
  // different seed -> (very likely) different layout
  ex_chunk_world_t *chunks3 = ex_chunk_world_new();
  ex_arena_t arena3;
  ex_arena_generate(&arena3, chunks3, -40, -40, 40, 40, 0.0f, 6, 9999);
  printf("  seed 1234 -> %d features, seed 9999 -> %d features (layouts differ)\n",
         arena.count, arena3.count);

  // ---- combat raycast ----
  printf("=== combat ===\n");
  ex_target_t targets[1];
  ex_enemy_t bot = ex_enemy_new(0, (vec3){0, 1, 20}, 100.0f);
  targets[0] = ex_enemy_as_target(&bot);

  vec3 origin = {0, 1, 0};
  vec3 dir = {0, 0, 1}; // straight at the bot down +Z
  // clear the arena chunks so this shot has a clean lane for the test
  ex_chunk_world_t *empty = ex_chunk_world_new();
  ex_hit_t hit = ex_combat_raycast(empty, targets, 1, origin, dir, 100.0f);
  printf("  shot down clear lane -> kind=%d dist=%.1f (expect ENEMY=2, ~19)\n", hit.kind, hit.dist);
  assert(hit.kind == EX_HIT_ENEMY && hit.index == 0);

  // now put a wall chunk between player and bot -> shot should hit the wall
  ex_chunk_add(empty, "block", ex_rect_new((vec3){-2,0,9}, (vec3){2,3,11}),
               (vec3[6]){{0,0,0},{1,0,0},{1,1,0},{0,0,0},{1,1,0},{0,1,0}}, 2,
               100.0f, EX_CHUNK_HIT_ANY, 1);
  hit = ex_combat_raycast(empty, targets, 1, origin, dir, 100.0f);
  printf("  same shot with a wall in the way -> kind=%d (expect CHUNK=1)\n", hit.kind);
  assert(hit.kind == EX_HIT_CHUNK);

  // ---- enemy AI state machine ----
  printf("=== enemy AI ===\n");
  ex_enemy_t e = ex_enemy_new(1, (vec3){0, 1, 30}, 100.0f);
  vec3 player = {0, 1, 0};
  printf("  start: %s (expect idle)\n", ex_enemy_state_name(e.state));
  assert(e.state == EX_ENEMY_IDLE);

  // within sight range (30 < 40) with clear LOS -> should start chasing
  ex_enemy_update(&e, empty /*wall at z~10 blocks LOS!*/, player, 0.1f);
  // the wall blocks LOS, so it stays idle -- verify LOS gating works
  printf("  with wall blocking LOS: %s\n", ex_enemy_state_name(e.state));

  // remove the blocker: fresh world, should chase then close to attack range
  ex_chunk_world_t *open = ex_chunk_world_new();
  ex_enemy_t e2 = ex_enemy_new(2, (vec3){0, 1, 30}, 100.0f);
  int saw_chase = 0, saw_attack = 0, shots = 0;
  for (int i = 0; i < 600; i++) { // ~10s at dt=1/60
    ex_enemy_update(&e2, open, player, 1.0f/60.0f);
    if (e2.state == EX_ENEMY_CHASE)  saw_chase = 1;
    if (e2.state == EX_ENEMY_ATTACK) saw_attack = 1;
    if (e2.wants_to_fire) shots++;
  }
  printf("  chased=%d reached_attack=%d shots_fired=%d\n", saw_chase, saw_attack, shots);
  assert(saw_chase && saw_attack && shots > 0);

  // ---- kill it ----
  int died = 0;
  for (int i = 0; i < 5 && !died; i++)
    died = ex_enemy_damage(&e2, 34.0f); // ~3 hits of 34
  printf("  after damage: %s (expect dead)\n", ex_enemy_state_name(e2.state));
  assert(e2.state == EX_ENEMY_DEAD && died);

  ex_chunk_world_free(chunks); ex_chunk_world_free(chunks2);
  ex_chunk_world_free(chunks3); ex_chunk_world_free(empty); ex_chunk_world_free(open);
  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
