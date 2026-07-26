/* test_gait.c -- procedural gait: phase advances with speed, legs counter-swing,
   body bobs, standing is still, element tints differ. */
#include "robot_parts.h"
#include <stdio.h>
#include <assert.h>
#include <math.h>

int main(void){
  // standing still: phase does not advance (no marching in place)
  float p = 1.23f;
  float still = ex_gait_advance(p, 0.0f, 0.1f);
  assert(fabsf(still - p) < 1e-6f);
  // moving: phase advances
  float moved = ex_gait_advance(p, 4.0f, 0.1f);
  assert(moved != p);
  printf("phase: still=%.3f moved=%.3f\n", still, moved);

  // at speed, legs swing in counter-phase (opposite signs somewhere in cycle)
  int sawL=0, sawR=0, opposite=0;
  for(float ph=0; ph<6.28f; ph+=0.2f){
    ex_part_pose_t po[EX_ROBOT_PARTS];
    ex_robot_layout(po,0,EX_BODY_PLAYER,1.0f,ph,5.0f,1);
    float l=po[EX_PART_SHIN_L].offset[2], r=po[EX_PART_SHIN_R].offset[2];
    if(fabsf(l)>0.01f) sawL=1; if(fabsf(r)>0.01f) sawR=1;
    if(l*r < 0.0f) opposite=1;
  }
  printf("legs swing L=%d R=%d counter-phase=%d\n", sawL, sawR, opposite);
  assert(sawL && sawR && opposite);

  // standing: legs do not swing
  ex_part_pose_t st[EX_ROBOT_PARTS];
  ex_robot_layout(st,0,EX_BODY_PLAYER,1.0f,1.0f,0.0f,1);
  assert(fabsf(st[EX_PART_SHIN_L].offset[2]) < 1e-5f);
  printf("standing legs still: ok\n");

  // body bobs while moving, not while still
  float bob_move=0, bob_still=0;
  for(float ph=0; ph<6.28f; ph+=0.3f){
    ex_part_pose_t m[EX_ROBOT_PARTS], s2[EX_ROBOT_PARTS];
    ex_robot_layout(m,0,EX_BODY_PLAYER,1.0f,ph,5.0f,1);
    ex_robot_layout(s2,0,EX_BODY_PLAYER,1.0f,ph,0.0f,1);
    bob_move += fabsf(m[EX_PART_CHEST].offset[1]-1.35f);
    bob_still += fabsf(s2[EX_PART_CHEST].offset[1]-1.35f);
  }
  printf("torso bob: moving=%.3f still=%.3f\n", bob_move, bob_still);
  // idle has a small breath/sway; moving must bob substantially MORE (not zero-idle)
  assert(bob_move > 0.5f && bob_move > bob_still * 2.0f);

  // element tints differ (ice vs fire)
  ex_part_pose_t ice[EX_ROBOT_PARTS], fire[EX_ROBOT_PARTS];
  ex_robot_layout(ice,0,EX_BODY_PLAYER,1.0f,0.0f,0.0f,1);
  ex_robot_layout(fire,0,EX_BODY_PLAYER,1.0f,0.0f,0.0f,2);
  int diff = ice[EX_PART_CHEST].color[0]!=fire[EX_PART_CHEST].color[0] ||
             ice[EX_PART_CHEST].color[2]!=fire[EX_PART_CHEST].color[2];
  printf("ice torso rgb=(%d,%d,%d) fire=(%d,%d,%d)\n",
    ice[EX_PART_CHEST].color[0],ice[EX_PART_CHEST].color[1],ice[EX_PART_CHEST].color[2],
    fire[EX_PART_CHEST].color[0],fire[EX_PART_CHEST].color[1],fire[EX_PART_CHEST].color[2]);
  assert(diff);

  // vehicle tucks head down + forward vs robot
  ex_part_pose_t rob[EX_ROBOT_PARTS], veh[EX_ROBOT_PARTS];
  ex_robot_layout(rob,0,EX_BODY_PLAYER,1.0f,0.0f,0.0f,1);
  ex_robot_layout(veh,1,EX_BODY_PLAYER,1.0f,0.0f,0.0f,1);
  assert(veh[EX_PART_HEAD].offset[1] < rob[EX_PART_HEAD].offset[1]);


  // build params reshape the body: taller build raises the head
  ex_part_pose_t tall[EX_ROBOT_PARTS], shortb[EX_ROBOT_PARTS];
  ex_build_params_t bt = ex_build_default(); bt.height = 1.5f;
  ex_build_params_t bs = ex_build_default(); bs.height = 0.7f;
  ex_robot_layout_ex(tall,0,EX_BODY_PLAYER,&bt,1.0f,0.0f,0.0f,1);
  ex_robot_layout_ex(shortb,0,EX_BODY_PLAYER,&bs,1.0f,0.0f,0.0f,1);
  assert(tall[EX_PART_HEAD].offset[1] > shortb[EX_PART_HEAD].offset[1]);
  // bulkier build widens the chest
  ex_part_pose_t lean[EX_ROBOT_PARTS], buff[EX_ROBOT_PARTS];
  ex_build_params_t bl = ex_build_default(); bl.bulk=0.7f;
  ex_build_params_t bb = ex_build_default(); bb.bulk=1.6f;
  ex_robot_layout_ex(lean,0,EX_BODY_PLAYER,&bl,1.0f,0.0f,0.0f,1);
  ex_robot_layout_ex(buff,0,EX_BODY_PLAYER,&bb,1.0f,0.0f,0.0f,1);
  assert(buff[EX_PART_CHEST].sx > lean[EX_PART_CHEST].sx);
  printf("build params reshape body: tall head y=%.2f vs short %.2f; buff chest w=%.2f vs lean %.2f\n",
    tall[EX_PART_HEAD].offset[1], shortb[EX_PART_HEAD].offset[1],
    buff[EX_PART_CHEST].sx, lean[EX_PART_CHEST].sx);


  // --- distinct per-kind silhouettes ---
  {
    ex_part_pose_t pl[EX_ROBOT_PARTS], gr[EX_ROBOT_PARTS], co[EX_ROBOT_PARTS], hj[EX_ROBOT_PARTS], ap[EX_ROBOT_PARTS];
    ex_robot_layout(pl,0,EX_BODY_PLAYER,1.0f,0.3f,3.0f,4);
    ex_robot_layout(gr,0,EX_BODY_GRUNT,1.0f,0.3f,3.0f,4);
    ex_robot_layout(co,0,EX_BODY_CORRUPTED,1.0f,0.3f,3.0f,4);
    ex_robot_layout(hj,0,EX_BODY_HIJACK,1.0f,0.3f,3.0f,4);
    ex_robot_layout(ap,0,EX_BODY_APEX,1.0f,0.3f,3.0f,4);
    float apex_head = ap[EX_PART_HEAD].offset[1], hij_head = hj[EX_PART_HEAD].offset[1];
    float base_head = pl[EX_PART_HEAD].offset[1];
    // apex towers over the player; hijack is much shorter
    assert(apex_head > base_head*1.2f);
    assert(hij_head < base_head*0.9f);
    // apex is far broader than a hijack (chest width)
    assert(ap[EX_PART_CHEST].sx > hj[EX_PART_CHEST].sx*2.0f);
    // corrupted is asymmetric: right arm much bigger than left
    assert(co[EX_PART_ARM_R].sx > co[EX_PART_ARM_L].sx*1.8f);
    printf("silhouettes distinct: apexH=%.2f playerH=%.2f hijackH=%.2f corrArmR=%.2f corrArmL=%.2f\n",
      apex_head, base_head, hij_head, co[EX_PART_ARM_R].sx, co[EX_PART_ARM_L].sx);
  }

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
