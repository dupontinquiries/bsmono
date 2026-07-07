/* test_custom.c -- detail greebles, vehicle archetypes, abilities, weapon mods. */
#include "detail.h"
#include "vehicle.h"
#include "ability.h"
#include "weaponmod.h"
#include "turret.h"
#include "enemy.h"
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include <string.h>

int main(void){
  printf("=== thousand-part detail ===\n");
  ex_detail_part_t parts[3] = {
    {{0,1.3f,0},0.6f,0.4f,0.4f,{140,150,165}},
    {{0,1.9f,0},0.26f,0.26f,0.26f,{160,170,185}},
    {{-0.6f,0.5f,0},0.2f,0.35f,0.2f,{120,130,145}},
  };
  ex_greeble_t g[EX_DETAIL_MAX];
  uint8_t accent[3]={235,205,85};
  int n = ex_detail_generate(g, EX_DETAIL_MAX, parts, 3, 1234, 1000, accent);
  printf("  generated %d greebles (asked 1000)\n", n);
  assert(n >= 900);
  // deterministic
  ex_greeble_t g2[EX_DETAIL_MAX];
  int n2 = ex_detail_generate(g2, EX_DETAIL_MAX, parts, 3, 1234, 1000, accent);
  assert(n2==n && memcmp(g,g2,sizeof(ex_greeble_t)*n)==0);
  // greebles sit near their parent part (bounded)
  for (int i=0;i<n;i++){ int p=g[i].parent; assert(p>=0&&p<3);
    float dx=fabsf(g[i].offset[0]-parts[p].offset[0]);
    assert(dx < parts[p].sx + 1.0f); }
  // more level -> more greebles
  int lo = ex_detail_generate(g,EX_DETAIL_MAX,parts,3,1,100,accent);
  int hi = ex_detail_generate(g,EX_DETAIL_MAX,parts,3,1,800,accent);
  assert(hi > lo);
  printf("  density scales: %d -> %d\n", lo, hi);

  printf("=== vehicle archetypes ===\n");
  for (int t=0;t<EX_VEH_COUNT;t++){
    ex_part_pose_t poses[EX_VEHICLE_MAX_PARTS]; int no=0;
    ex_vehicle_layout(poses, EX_VEHICLE_MAX_PARTS, &no, (ex_vehicle_type)t, NULL, 1.0f, 0.5f, 3.0f, 2, EX_BODY_PLAYER);
    printf("  %-10s parts=%d flies=%d\n", ex_vehicle_name((ex_vehicle_type)t), no, ex_vehicle_flies((ex_vehicle_type)t));
    assert(no >= 3);
  }
  // spider has more legs than raptor
  ex_part_pose_t a[EX_VEHICLE_MAX_PARTS],b[EX_VEHICLE_MAX_PARTS]; int na,nb;
  ex_vehicle_layout(a,EX_VEHICLE_MAX_PARTS,&na,EX_VEH_SPIDER,NULL,1,0,3,2,EX_BODY_PLAYER);
  ex_vehicle_layout(b,EX_VEHICLE_MAX_PARTS,&nb,EX_VEH_RAPTOR,NULL,1,0,3,2,EX_BODY_PLAYER);
  assert(na > nb);
  assert(ex_vehicle_flies(EX_VEH_DRAGON) && !ex_vehicle_flies(EX_VEH_TANK));

  printf("=== abilities ===\n");
  ex_ability_loadout_t l; ex_loadout_init(&l);
  ex_loadout_add(&l, EX_ABIL_SHIELD); ex_loadout_add(&l, EX_ABIL_DASH); ex_loadout_add(&l, EX_ABIL_TURRET);
  assert(l.count==3);
  assert(ex_ability_ready(&l,0));
  assert(ex_ability_trigger(&l,0));            // shield on
  assert(ex_ability_active(&l,0));
  assert(ex_ability_active_any(&l, EX_ABIL_SHIELD));
  assert(!ex_ability_ready(&l,0));             // now on cooldown
  // dash is instant-ish: trigger, expires, recharges
  assert(ex_ability_trigger(&l,1));
  for (int i=0;i<60*20;i++) ex_loadout_update(&l, 1.0f/60.0f);
  assert(ex_ability_ready(&l,0) && ex_ability_ready(&l,1));
  printf("  shield/dash/turret cycle ok\n");

  printf("=== weapon mods ===\n");
  ex_weapon_profile_t base = ex_weapon_profile(EX_FORM_METAL);
  ex_weapon_mods_t m; ex_weapon_mods_init(&m);
  ex_weapon_mods_add(&m, EX_MOD_LONG_BARREL);
  ex_weapon_mods_add(&m, EX_MOD_AMPLIFIER);
  ex_weapon_profile_t mod = ex_weapon_apply_mods(EX_FORM_METAL, &m);
  printf("  metal: dmg %.0f->%.0f range %.0f->%.0f\n", base.damage, mod.damage, base.range, mod.range);
  assert(mod.damage > base.damage && mod.range > base.range);
  // underbarrel turns a bolt into a burst with +pellet
  ex_weapon_mods_t u; ex_weapon_mods_init(&u); ex_weapon_mods_add(&u, EX_MOD_UNDERBARREL);
  ex_weapon_profile_t ub = ex_weapon_apply_mods(EX_FORM_WATER, &u);
  assert(ub.pellets > ex_weapon_profile(EX_FORM_WATER).pellets);
  // scope tightens spread on a spread weapon
  ex_weapon_mods_t sc; ex_weapon_mods_init(&sc); ex_weapon_mods_add(&sc, EX_MOD_SCOPE);
  assert(ex_weapon_apply_mods(EX_FORM_BLIZZARD,&sc).spread_deg < ex_weapon_profile(EX_FORM_BLIZZARD).spread_deg);


  printf("=== turret ===\n");
  ex_turret_t tur; float dpos[3]={0,0,0};
  ex_turret_deploy(&tur, dpos, 5.0f);
  assert(tur.active);
  ex_target_t tt[3] = {
    {{50,0,0},1.0f,100,1}, {{5,0,0},1.0f,101,1}, {{200,0,0},1.0f,102,1} };
  // picks nearest in range (id 101 at dist 5)
  int pick = ex_turret_pick(tt, 3, tur.pos, 60.0f);
  assert(pick>=0 && tt[pick].id==101);
  // out of range target not picked
  ex_target_t far1[1]={{{500,0,0},1.0f,9,1}};
  assert(ex_turret_pick(far1,1,tur.pos,60.0f)==-1);
  // fires over its lifetime then expires
  int shots=0; for(int i=0;i<60*6;i++){ if(ex_turret_update(&tur,1.0f/60.0f,tt,3)) shots++; }
  printf("  turret fired %d times then expired (active=%d)\n", shots, tur.active);
  assert(shots>0 && !tur.active);

  printf("=== cloak vs spark-resonance ===\n");
  ex_chunk_world_t *cw = ex_chunk_world_new();
  vec3 pl={0,1,3};
  ex_enemy_t grunt = ex_enemy_new_kind(1,(vec3){0,0,10},120,EX_ENEMY_GRUNT,EX_ELEM_ROCK);
  ex_enemy_t corr  = ex_enemy_new_kind(2,(vec3){0,0,10},120,EX_ENEMY_CORRUPTED,EX_ELEM_VINE);
  // cloak ON: grunt loses the player (stays idle), corrupted still hunts
  ex_enemy_set_cloak(1);
  for(int i=0;i<120;i++){ ex_enemy_update(&grunt,cw,pl,1.0f/60.0f); ex_enemy_update(&corr,cw,pl,1.0f/60.0f); }
  printf("  cloaked: grunt state=%s corrupted state=%s\n",
    ex_enemy_state_name(grunt.state), ex_enemy_state_name(corr.state));
  assert(grunt.state==EX_ENEMY_IDLE);          // fooled
  assert(corr.state!=EX_ENEMY_IDLE);           // spark resonance ignores cloak
  ex_enemy_set_cloak(0);
  ex_chunk_world_free(cw);

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
