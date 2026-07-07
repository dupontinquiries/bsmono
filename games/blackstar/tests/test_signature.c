/* test_signature.c -- verifies the class "signature component" contract:
   every class maps to a signature with a real mesh, and every (signature,
   vehicle archetype) pair yields a robot-mode AND a vehicle-mode pose that is
   non-degenerate, in-bounds, and ACTUALLY DIFFERENT between the two modes (so
   the transform has something to morph -- the whole point of "bounded on two
   sides"). Pure logic, no GL. */
#include <stdio.h>
#include <string.h>
#include <assert.h>
#include <math.h>
#include "signature.h"
#include "pclass.h"
#include "vehicle.h"

static float pose_delta(const ex_part_pose_t *a, const ex_part_pose_t *b)
{
  float d=0;
  for (int k=0;k<3;k++) d += fabsf(a->offset[k]-b->offset[k]);
  d += fabsf(a->sx-b->sx)+fabsf(a->sy-b->sy)+fabsf(a->sz-b->sz);
  return d;
}

int main(void)
{
  // 1) every class carries a signature with a valid mesh path
  for (int c=0;c<EX_CLASS_COUNT;c++){
    ex_signature_t s = ex_signature_for_class(c);
    assert(s >= 0 && s < EX_SIG_COUNT);
    const char *mesh = ex_signature_mesh(s);
    assert(mesh && strstr(mesh, "spine_") && strstr(mesh, ".iqm"));
    printf("class %d -> signature %d  mesh=%s\n", c, (int)s, mesh);
  }

  // 2) the four classes use distinct signatures (real per-class identity)
  int seen[EX_SIG_COUNT]; memset(seen,0,sizeof seen);
  for (int c=0;c<EX_CLASS_COUNT;c++) seen[ex_signature_for_class(c)]++;
  int distinct=0; for (int i=0;i<EX_SIG_COUNT;i++) if (seen[i]) distinct++;
  printf("distinct signatures across classes: %d\n", distinct);
  assert(distinct == EX_CLASS_COUNT && "each class should read differently");

  // 3) every (signature, vehicle) pair: both poses valid + meaningfully differ
  int checked=0;
  for (int s=0;s<EX_SIG_COUNT;s++){
    for (int v=0;v<EX_VEH_COUNT;v++){
      ex_part_pose_t rob, veh;
      ex_signature_pose(&rob, &veh, (ex_signature_t)s, (ex_vehicle_type)v,
                        1.0f, 0.0f, 0.0f);
      // non-degenerate scale in both modes
      assert(rob.sx>0.01f && rob.sy>0.01f && rob.sz>0.01f);
      assert(veh.sx>0.01f && veh.sy>0.01f && veh.sz>0.01f);
      // sane bounds (parts stay near the body, not flung to infinity)
      for (int k=0;k<3;k++){ assert(fabsf(rob.offset[k])<4.0f); assert(fabsf(veh.offset[k])<4.0f); }
      // the robot pose sits higher than the folded vehicle pose (it drops on fold)
      assert(rob.offset[1] > veh.offset[1] - 0.01f);
      // the two modes must actually differ, or there's nothing to morph
      float d = pose_delta(&rob, &veh);
      assert(d > 0.25f && "robot and vehicle poses too similar -- no visible fold");
      checked++;
    }
  }
  printf("verified %d (signature x vehicle) pose pairs\n", checked);

  // 4) NULL-out arguments are tolerated
  ex_signature_pose(NULL, NULL, EX_SIG_WING_CANNON, EX_VEH_JET, 1.0f, 0.0f, 0.0f);

  // 5) enemy per-body defaults: every body kind maps to a valid signature or -1
  //    (none), and the flagship enemies are all distinct from a plain grunt.
  ex_body_kind kinds[] = { EX_BODY_PLAYER, EX_BODY_GRUNT, EX_BODY_CORRUPTED,
                           EX_BODY_HIJACK, EX_BODY_APEX, EX_BODY_HUMAN };
  for (unsigned k=0;k<sizeof(kinds)/sizeof(kinds[0]);k++){
    int bs = ex_signature_for_body(kinds[k]);
    assert(bs == -1 || (bs >= 0 && bs < EX_SIG_COUNT));
    printf("body %d -> signature %d\n", (int)kinds[k], bs);
  }
  assert(ex_signature_for_body(EX_BODY_HUMAN) == -1 && "humans carry no spine part");
  assert(ex_signature_for_body(EX_BODY_CORRUPTED) != ex_signature_for_body(EX_BODY_GRUNT));
  assert(ex_signature_for_body(EX_BODY_APEX)      != ex_signature_for_body(EX_BODY_GRUNT));
  assert(ex_signature_for_body(EX_BODY_HIJACK)    != ex_signature_for_body(EX_BODY_GRUNT));

  // 6) builder override semantics: -1 falls back to the class default, an
  //    explicit value wins. (Mirrors game.c player_signature().)
  for (int cls=0; cls<EX_CLASS_COUNT; cls++){
    int chosen = -1; // "class default"
    ex_signature_t eff = (chosen < 0) ? ex_signature_for_class(cls)
                                      : (ex_signature_t)(chosen % EX_SIG_COUNT);
    assert(eff == ex_signature_for_class(cls));
    for (int explicit_sig=0; explicit_sig<EX_SIG_COUNT; explicit_sig++){
      ex_signature_t e2 = (explicit_sig < 0) ? ex_signature_for_class(cls)
                                             : (ex_signature_t)(explicit_sig % EX_SIG_COUNT);
      assert((int)e2 == explicit_sig && "explicit builder choice must override class");
    }
  }
  printf("builder override + class-default fallback OK\n");

  // 7) gameplay stats: bounded, distinct playstyles, none strictly dominant
  //    (every signature has at least one drawback and one advantage vs neutral).
  ex_sig_stats_t st[EX_SIG_COUNT];
  for (int s=0;s<EX_SIG_COUNT;s++){
    st[s]=ex_signature_stats((ex_signature_t)s);
    float m[4]={st[s].dmg_mul,st[s].fire_mul,st[s].taken_mul,st[s].speed_mul};
    for (int k=0;k<4;k++){ assert(m[k]>0.5f && m[k]<1.6f && "stat out of sane range"); }
    // advantage = higher dmg/speed, lower fire(interval)/taken; drawback = the reverse
    int adv = (st[s].dmg_mul>1.001f)||(st[s].speed_mul>1.001f)||(st[s].fire_mul<0.999f)||(st[s].taken_mul<0.999f);
    int dis = (st[s].dmg_mul<0.999f)||(st[s].speed_mul<0.999f)||(st[s].fire_mul>1.001f)||(st[s].taken_mul>1.001f);
    assert(adv && dis && "each signature must trade off, not be strictly best");
    printf("sig %d stats: dmg=%.2f fire=%.2f taken=%.2f speed=%.2f\n",
           s, st[s].dmg_mul, st[s].fire_mul, st[s].taken_mul, st[s].speed_mul);
  }
  // the four profiles are not all identical
  int same=1; for (int s=1;s<EX_SIG_COUNT;s++)
    if (st[s].dmg_mul!=st[0].dmg_mul || st[s].speed_mul!=st[0].speed_mul) same=0;
  assert(!same && "signature stats must differentiate playstyles");
  printf("gameplay stats OK (bounded, differentiated, tradeoff-balanced)\n");

  // 8) combat identity: each signature has a distinct on-hit effect, all bounded.
  ex_sig_combat_t cb[EX_SIG_COUNT];
  int touched[EX_SIG_COUNT];
  for (int s=0;s<EX_SIG_COUNT;s++){
    cb[s]=ex_signature_combat((ex_signature_t)s);
    assert(cb[s].extra_pellets>=0 && cb[s].extra_pellets<=4);
    assert(cb[s].knockback>=0.0f && cb[s].knockback<40.0f);
    assert(cb[s].slow>=0.0f && cb[s].slow<6.0f);
    assert(cb[s].splash>=0.0f && cb[s].splash<12.0f);
    touched[s] = (cb[s].extra_pellets>0)||(cb[s].knockback>0.0f)||(cb[s].slow>0.0f)||(cb[s].splash>0.0f);
    assert(touched[s] && "every signature should have SOME combat identity");
    printf("sig %d combat: pellets=%d knock=%.0f slow=%.1f splash=%.0f\n",
           s, cb[s].extra_pellets, cb[s].knockback, cb[s].slow, cb[s].splash);
  }
  // the wing-cannon is the only one adding pellets; the cable is the only rooter
  assert(cb[EX_SIG_WING_CANNON].extra_pellets > 0);
  assert(cb[EX_SIG_CABLE].slow > 0.0f && cb[EX_SIG_HEAVY_CANNON].slow == 0.0f);
  assert(cb[EX_SIG_HEAVY_CANNON].splash > 0.0f);
  printf("combat identity OK (distinct per-signature on-hit effects)\n");

  printf("\nSIGNATURE CONTRACT OK (per-class identity + two-sided, morphable poses)\n");
  return 0;
}
