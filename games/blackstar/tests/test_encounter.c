/* test_encounter.c -- multi-enemy spawn, target list, win conditions. */
#include "encounter.h"
#include <stdio.h>
#include <assert.h>
#include <string.h>

int main(void){
  // eliminate mode: spawn a makeup, all must die to clear
  ex_encounter_t e; ex_encounter_init(&e, EX_WIN_ELIMINATE, 0);
  ex_encounter_spawn_makeup(&e, 4, 2, 0, -40,-40,40,40, 777);
  printf("spawned %d enemies (4 grunt + 2 corrupted)\n", e.count);
  assert(e.count == 6);
  // enemies are spread out (not all same position)
  int distinct = 0;
  for (int i=1;i<e.count;i++)
    if (e.enemies[i].position[0]!=e.enemies[0].position[0]) distinct=1;
  assert(distinct);

  // target list only includes living enemies
  ex_target_t tg[EX_MAX_ENEMIES];
  int nt = ex_encounter_targets(&e, tg, EX_MAX_ENEMIES);
  printf("targets alive: %d\n", nt);
  assert(nt == 6);

  // not cleared while enemies live
  ex_chunk_world_t *w = ex_chunk_world_new();
  vec3 player = {0,1,0};
  ex_encounter_update(&e, w, player, 1.0f/60.0f);
  assert(!e.cleared && ex_encounter_alive(&e)==6);

  // kill them all -> cleared
  for (int i=0;i<e.count;i++) ex_enemy_damage(&e.enemies[i], 9999.0f);
  ex_encounter_update(&e, w, player, 1.0f/60.0f);
  printf("after killing all: alive=%d cleared=%d\n", ex_encounter_alive(&e), e.cleared);
  assert(ex_encounter_alive(&e)==0 && e.cleared==1);
  // target list now empty
  assert(ex_encounter_targets(&e, tg, EX_MAX_ENEMIES)==0);

  // survival mode: cleared only when timer elapses, regardless of kills
  ex_encounter_t s; ex_encounter_init(&s, EX_WIN_SURVIVE, 3.0f);
  ex_encounter_spawn_makeup(&s, 3, 1, 0, -40,-40,40,40, 42);
  int ticks=0;
  while(!s.cleared && ticks < 60*10){ ex_encounter_update(&s, w, player, 1.0f/60.0f); ticks++; }
  printf("survival cleared after %.1fs\n", ticks/60.0f);
  assert(s.cleared && ticks >= 3*60-2 && ticks <= 3*60+2);

  // capacity cap
  ex_encounter_t big; ex_encounter_init(&big, EX_WIN_ELIMINATE, 0);
  ex_encounter_spawn_makeup(&big, 20, 20, 8, -40,-40,40,40, 1);
  printf("capacity: requested 40, got %d (cap %d)\n", big.count, EX_MAX_ENEMIES);
  assert(big.count == EX_MAX_ENEMIES);

  // find by id
  assert(ex_encounter_by_id(&e, 100) != NULL);
  assert(ex_encounter_by_id(&e, 9999) == NULL);


  // boss encounter
  ex_encounter_t be; ex_encounter_init(&be, EX_WIN_ELIMINATE, 0);
  ex_encounter_boss(&be, 0, 0, 1600.0f);
  assert(ex_encounter_boss_ptr(&be) != NULL);
  assert(ex_encounter_boss_ptr(&be)->kind == EX_ENEMY_APEX);
  ex_enemy_damage(ex_encounter_boss_ptr(&be), 9999.0f);
  assert(ex_encounter_boss_ptr(&be) == NULL); // dead -> no live boss
  printf("boss spawn/kill ok\n");

  ex_chunk_world_free(w);
  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
