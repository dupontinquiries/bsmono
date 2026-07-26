/* test_weapon.c -- the Watch's 11 forms have distinct combat profiles;
   starfire charges; gravity leap teleports + schedules a delayed wave;
   Corrupted escalate through stages. */
#include "weapon.h"
#include "weaponmod.h"
#include "enemy.h"
#include "element.h"
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include <string.h>

int main(void){
  printf("=== Watch forms (distinct profiles) ===\n");
  int burst=0, beam=0, wave=0, lob=0, bolt=0, pierce=0, flak=0, plasma=0, ricochet=0, suppress=0;
  for (int f=0; f<EX_FORM_COUNT; f++){
    ex_weapon_profile_t p = ex_weapon_profile((ex_watch_form_t)f);
    printf("  %-12s mode=%d dmg=%.0f range=%.0f cd=%.2f pellets=%d chains=%d\n",
      ex_form_name((ex_watch_form_t)f), p.mode, p.damage, p.range, p.cooldown, p.pellets, p.chains);
    assert(p.damage > 0 && p.range > 0 && p.cooldown > 0);
    switch(p.mode){case EX_FIRE_BURST:burst++;break;case EX_FIRE_BEAM:beam++;break;
      case EX_FIRE_WAVE:wave++;break;case EX_FIRE_LOB:lob++;break;
      case EX_FIRE_PIERCE:pierce++;break;case EX_FIRE_FLAK:flak++;break;
      case EX_FIRE_PLASMA:plasma++;break;case EX_FIRE_RICOCHET:ricochet++;break;
      case EX_FIRE_SUPPRESS:suppress++;break;default:bolt++;}
  }
  // roster uses a variety of fire modes (not all identical)
  printf("  modes: bolt=%d burst=%d beam=%d wave=%d lob=%d | pierce=%d flak=%d plasma=%d ricochet=%d suppress=%d\n",
         bolt,burst,beam,wave,lob,pierce,flak,plasma,ricochet,suppress);
  assert(burst>=2 && beam>=1 && wave>=1 && lob>=1);
  assert(pierce>=1 && flak>=1 && plasma>=1 && ricochet>=1 && suppress>=1);   // all bullet types present
  // Photon is the apex: highest damage of all forms
  float photon = ex_weapon_profile(EX_FORM_PHOTON).damage, best=0;
  for(int f=0; f<EX_FORM_PHOTON; f++) best = fmaxf(best, ex_weapon_profile((ex_watch_form_t)f).damage);
  assert(photon > best);
  printf("  photon dmg %.0f > best mundane %.0f\n", photon, best);
  // metal fires faster than titanium (rapid vs tank)
  assert(ex_weapon_profile(EX_FORM_METAL).cooldown < ex_weapon_profile(EX_FORM_TITANIUM).cooldown);
  // ice applies slow, fire doesn't
  assert(ex_weapon_profile(EX_FORM_ICE).slow > 0 && ex_weapon_profile(EX_FORM_FIRE).slow == 0);
  // electricity chains
  assert(ex_weapon_profile(EX_FORM_ELECTRICITY).chains > 0);
  // forms map onto the element advantage cycle
  assert(ex_form_element(EX_FORM_FIRE)==EX_ELEM_FIRE && ex_form_element(EX_FORM_BLIZZARD)==EX_ELEM_ICE);

  printf("=== new fire modes + weapon addons ===\n");
  // titanium is the piercing railgun; fire is the flak cannon
  assert(ex_weapon_profile(EX_FORM_TITANIUM).mode==EX_FIRE_PIERCE);
  assert(ex_weapon_profile(EX_FORM_FIRE).mode==EX_FIRE_FLAK);
  assert(ex_weapon_profile(EX_FORM_ELECTRICITY).mode==EX_FIRE_PLASMA);
  assert(ex_weapon_profile(EX_FORM_METAL).mode==EX_FIRE_RICOCHET);
  assert(ex_weapon_profile(EX_FORM_ICE).mode==EX_FIRE_SUPPRESS && ex_weapon_profile(EX_FORM_ICE).slow>0.5f);
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m); ex_weapon_mods_add(&m, EX_MOD_RIPPLE_COIL);
    assert(ex_weapon_apply_mods(EX_FORM_WATER,&m).mode==EX_FIRE_RICOCHET); }
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m); ex_weapon_mods_add(&m, EX_MOD_CRYO_ROUNDS);
    assert(ex_weapon_apply_mods(EX_FORM_WATER,&m).slow > 0.0f && "cryo rounds add slow"); }
  // WFC/FOC-style addons adjust the profile as advertised
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m); ex_weapon_mods_add(&m, EX_MOD_ENERGON_CELL);
    ex_weapon_profile_t base = ex_weapon_profile(EX_FORM_WATER);
    ex_weapon_profile_t en = ex_weapon_apply_mods(EX_FORM_WATER, &m);
    assert(en.damage > base.damage && "energon cell raises damage"); }
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m); ex_weapon_mods_add(&m, EX_MOD_RAPID_COIL);
    assert(ex_weapon_apply_mods(EX_FORM_WATER,&m).cooldown < ex_weapon_profile(EX_FORM_WATER).cooldown); }
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m); ex_weapon_mods_add(&m, EX_MOD_PIERCE_CORE);
    assert(ex_weapon_apply_mods(EX_FORM_WATER,&m).mode==EX_FIRE_PIERCE && "pierce core converts to railshot"); }
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m); ex_weapon_mods_add(&m, EX_MOD_SCATTER_BARREL);
    assert(ex_weapon_apply_mods(EX_FORM_WATER,&m).pellets > ex_weapon_profile(EX_FORM_WATER).pellets); }
  // three addons stack into the three slots
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m);
    ex_weapon_mods_add(&m, EX_MOD_ENERGON_CELL); ex_weapon_mods_add(&m, EX_MOD_LONG_BARREL); ex_weapon_mods_add(&m, EX_MOD_SCOPE);
    assert(m.count==3);
    ex_weapon_profile_t p = ex_weapon_apply_mods(EX_FORM_WATER,&m);
    assert(p.damage > ex_weapon_profile(EX_FORM_WATER).damage && p.range > ex_weapon_profile(EX_FORM_WATER).range);
    assert(ex_weapon_mods_add(&m, EX_MOD_AMPLIFIER) < 0 && "only three slots"); }
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m); ex_weapon_mods_add(&m, EX_MOD_FLAK_CORE);
    assert(ex_weapon_apply_mods(EX_FORM_WATER,&m).mode==EX_FIRE_FLAK && "flak core -> airburst"); }
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m); ex_weapon_mods_add(&m, EX_MOD_PLASMA_CORE);
    ex_weapon_profile_t p=ex_weapon_apply_mods(EX_FORM_WATER,&m);
    assert(p.mode==EX_FIRE_PLASMA && p.chains>ex_weapon_profile(EX_FORM_WATER).chains && "plasma core -> heavy blast +chain"); }
  { ex_weapon_mods_t m; ex_weapon_mods_init(&m); ex_weapon_mods_add(&m, EX_MOD_SUPPRESS_CORE);
    ex_weapon_profile_t p=ex_weapon_apply_mods(EX_FORM_WATER,&m);
    assert(p.mode==EX_FIRE_SUPPRESS && p.range>ex_weapon_profile(EX_FORM_WATER).range && "suppress core -> long freeze beam"); }
  printf("  pierce/flak forms + energon/rapid/pierce/scatter addons + 3-slot stack OK\n");

  printf("=== star-fire charge ===\n");
  ex_starfire_t sf; memset(&sf,0,sizeof(sf)); ex_starfire_begin(&sf);
  float m0 = 0; // no charge
  ex_starfire_update(&sf, 0.0f, 1); m0 = 1.0f; // baseline ~1x
  for (int i=0;i<200;i++) ex_starfire_update(&sf, 1.0f/60.0f, 1);
  int was_star = 0;
  float full = ex_starfire_release(&sf, &was_star);
  printf("  full-charge multiplier = %.1fx (star=%d)\n", full, was_star);
  assert(full > 5.0f && sf.charge == 0.0f);
  assert(was_star == 1 && "full charge releases a real star");
  // spending a star starts a lockout: cannot charge until it cools off
  assert(!ex_starfire_ready(&sf) && ex_starfire_cooldown_frac(&sf) > 0.9f);
  ex_starfire_begin(&sf);                       // ignored while cooling
  ex_starfire_update(&sf, 1.0f/60.0f, 1);
  assert(sf.charge == 0.0f && "no charging during cooldown");
  for (int i=0;i<600;i++) ex_starfire_update(&sf, 1.0f/60.0f, 0);  // wait it out
  assert(ex_starfire_ready(&sf));
  // a partial charge releases a boosted bolt but NOT a star (no lockout)
  ex_starfire_begin(&sf);
  for (int i=0;i<30;i++) ex_starfire_update(&sf, 1.0f/60.0f, 1);   // ~0.36 charge
  int partial_star = 1; float partial = ex_starfire_release(&sf, &partial_star);
  assert(partial > 1.0f && partial < 5.0f && partial_star == 0 && ex_starfire_ready(&sf));
  printf("  partial release = %.2fx (star=%d), no lockout OK\n", partial, partial_star);
  (void)m0;

  printf("=== quantum gravity leap ===\n");
  ex_gravleap_t gl; memset(&gl,0,sizeof(gl));
  float from[3]={0,1,0}, dir[3]={0,0,1}, out[3];
  ex_gravleap_fire(&gl, from, dir, 40.0f, out);
  printf("  leapt to (%.0f,%.0f,%.0f), wave pending=%d\n", out[0],out[1],out[2],gl.pending);
  assert(fabsf(out[2]-40.0f)<0.01f && gl.pending==1);
  // wave doesn't land immediately
  float c[3],r,d; assert(ex_gravleap_update(&gl, 0.5f, c,&r,&d)==0);
  // ...but lands after the delay, indifferent AoE at arrival point
  int landed=0; for(int i=0;i<200 && !landed;i++) landed=ex_gravleap_update(&gl,1.0f/60.0f,c,&r,&d);
  printf("  wave landed: center z=%.0f radius=%.0f damage=%.0f\n", c[2],r,d);
  assert(landed && fabsf(c[2]-40.0f)<0.01f && r>0 && d>0);

  printf("=== Corrupted stage escalation ===\n");
  ex_enemy_t corr = ex_enemy_new_kind(1,(vec3){0,1,50},200.0f,EX_ENEMY_CORRUPTED,EX_ELEM_VINE);
  assert(corr.stage==1 && corr.transmits==1);
  float d1 = ex_enemy_melee_damage(&corr);
  ex_chunk_world_t *w = ex_chunk_world_new();
  vec3 player={0,1,0};
  // run ~30s; should reach stage 3
  for(int i=0;i<30*60;i++) ex_enemy_update(&corr, w, player, 1.0f/60.0f);
  float d3 = ex_enemy_melee_damage(&corr);
  printf("  stage %d (%s), melee %.0f->%.0f, grow=%.2f, radius %.2f->%.2f\n",
    corr.stage, ex_enemy_stage_name(corr.stage), d1, d3, corr.grow, 1.1f, ex_enemy_visual_radius(&corr));
  assert(corr.stage==3 && d3 > d1 && ex_enemy_visual_radius(&corr) > 1.1f);
  ex_chunk_world_free(w);


  printf("=== apex boss phases ===\n");
  ex_enemy_t apex = ex_enemy_new_kind(500,(vec3){0,0,0},1600.0f,EX_ENEMY_APEX,EX_ELEM_VINE);
  assert(apex.phase==1 && apex.max_hp==1600.0f && apex.radius>=3.0f);
  ex_chunk_world_t *bw = ex_chunk_world_new(); vec3 pl={0,1,5};
  // phase 1: no slam yet
  int slammed=0, summoned=0;
  for(int i=0;i<60;i++){ ex_enemy_update(&apex,bw,pl,1.0f/60.0f); if(apex.wants_slam)slammed=1; }
  assert(!slammed); // phase 1 doesn't slam
  // drop to phase 2 -> slams begin
  ex_enemy_damage(&apex, 700.0f);
  for(int i=0;i<60*8;i++){ ex_enemy_update(&apex,bw,pl,1.0f/60.0f); if(apex.wants_slam)slammed=1; }
  assert(apex.phase==2 && slammed);
  // drop to phase 3 -> summons begin
  ex_enemy_damage(&apex, 600.0f);
  for(int i=0;i<60*14;i++){ ex_enemy_update(&apex,bw,pl,1.0f/60.0f); if(apex.wants_summon)summoned=1; }
  assert(apex.phase==3 && summoned);
  printf("  phase progression 1->2(slam)->3(summon) ok; melee scales to %.0f\n", ex_enemy_melee_damage(&apex));
  assert(ex_enemy_melee_damage(&apex) > 30.0f);
  ex_chunk_world_free(bw);

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
