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

  // --- Destiny owned-space zones -------------------------------------------
  // hand-place enemies at known distances and verify sticky zones + the
  // collapse-advance that the fix makes actually take effect.
  ex_chunk_world_t *zw = ex_chunk_world_new();
  vec3 pz = {0,1,0};
  ex_encounter_t z; ex_encounter_init(&z, EX_WIN_ELIMINATE, 0);
  // front (~6u), middle (~18u), back (~34u) -- three grunts per line
  for (int i=0;i<3;i++) ex_encounter_spawn(&z, EX_ENEMY_GRUNT, EX_ELEM_FIRE, (vec3){6.0f, 0, i*1.0f}, 100.0f);
  for (int i=0;i<3;i++) ex_encounter_spawn(&z, EX_ENEMY_GRUNT, EX_ELEM_FIRE, (vec3){18.0f,0, i*1.0f}, 100.0f);
  for (int i=0;i<3;i++) ex_encounter_spawn(&z, EX_ENEMY_GRUNT, EX_ELEM_FIRE, (vec3){34.0f,0, i*1.0f}, 100.0f);
  // first update assigns zones from spawn distance
  ex_encounter_update(&z, zw, pz, 1.0f/60.0f);
  int zc[3]={0,0,0};
  for (int i=0;i<z.count;i++) if (z.enemies[i].zone>=0 && z.enemies[i].zone<3) zc[z.enemies[i].zone]++;
  printf("zones after assign: front=%d mid=%d back=%d\n", zc[0],zc[1],zc[2]);
  assert(zc[0]==3 && zc[1]==3 && zc[2]==3);        // one line each

  // STICKY: a back-line enemy's zone must NOT be reshuffled just because a
  // later frame sees it at a slightly different distance.
  int back_zone_before = -1;
  for (int i=0;i<z.count;i++) if (z.enemies[i].position[0]>30.0f){ back_zone_before=z.enemies[i].zone; break; }
  z.enemies[8].position[0] = 5.0f;                  // yank one back enemy forward in space
  ex_encounter_update(&z, zw, pz, 1.0f/60.0f);
  assert(z.enemies[8].zone == back_zone_before);    // still tagged back -- sticky
  printf("sticky zone held under position change\n");

  // COLLAPSE-ADVANCE: wipe the front line -> the lines behind advance forward.
  for (int i=0;i<z.count;i++) if (z.enemies[i].zone==0) ex_enemy_damage(&z.enemies[i], 9999.0f);
  ex_encounter_update(&z, zw, pz, 1.0f/60.0f);      // update_zones collapses front
  int adv=0;
  for (int i=0;i<z.count;i++)
    if (z.enemies[i].state!=EX_ENEMY_DEAD && z.enemies[i].zone==0) adv++;  // survivors now in front
  printf("survivors advanced into front zone: %d\n", adv);
  assert(adv > 0);                                  // the advance actually STUCK (was the bug)
  ex_chunk_world_free(zw);

  // --- commander cohesion: killing a leader scatters its squad ---------------
  ex_chunk_world_t *cw = ex_chunk_world_new();
  vec3 pc = {0,1,0};
  ex_encounter_t cmd; ex_encounter_init(&cmd, EX_WIN_ELIMINATE, 0);
  // a commander at 20u with 5 troops ringed around it
  int ci = ex_encounter_spawn_squad(&cmd, EX_ENEMY_APEX, EX_ENEMY_GRUNT, EX_ELEM_FIRE,
                                    (vec3){20,0,0}, 400.0f, 100.0f, 5, 3.0f);
  assert(ci >= 0);
  assert(cmd.enemies[ci].is_commander == 1);
  int cmd_id = cmd.enemies[ci].id;
  // count troops bound to this commander
  int bound = 0;
  for (int i=0;i<cmd.count;i++) if (cmd.enemies[i].squad == cmd_id) bound++;
  printf("squad: commander + %d troops bound\n", bound);
  assert(bound == 5);

  // while the commander lives, troops are disciplined (no scatter)
  ex_encounter_update(&cmd, cw, pc, 1.0f/60.0f);
  int scattering = 0;
  for (int i=0;i<cmd.count;i++) if (cmd.enemies[i].scatter_t > 0.0f) scattering++;
  assert(scattering == 0);

  // KILL THE COMMANDER -> its squad scatters
  ex_enemy_damage(&cmd.enemies[ci], 99999.0f);
  ex_encounter_update(&cmd, cw, pc, 1.0f/60.0f);
  scattering = 0;
  for (int i=0;i<cmd.count;i++)
    if (cmd.enemies[i].squad == cmd_id && cmd.enemies[i].scatter_t > 0.0f) scattering++;
  printf("after commander death: %d troops scattering\n", scattering);
  assert(scattering == 5);            // the whole squad breaks

  // after the panic window they become demoralized (fight on, shaken)
  for (int t=0; t<5*60+10; t++) ex_encounter_update(&cmd, cw, pc, 1.0f/60.0f);
  int demoralized = 0;
  for (int i=0;i<cmd.count;i++)
    if (cmd.enemies[i].squad == cmd_id && cmd.enemies[i].state != EX_ENEMY_DEAD
        && cmd.enemies[i].demoralized) demoralized++;
  printf("after panic window: %d demoralized survivors\n", demoralized);
  assert(demoralized > 0);
  ex_chunk_world_free(cw);

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
