/* test_ally.c -- Forgeheart's companion brain.
   Verifies nearest-target selection, engage vs follow modes, attack cadence,
   and the downed -> revive clock. */
#include <stdio.h>
#include <assert.h>
#include <math.h>
#include "ally.h"

int main(void)
{
  ex_ally_t a;
  float home[3] = {0,0,0};
  ex_ally_reset(&a, home, 200.0f);
  assert(a.active && a.hp == 200.0f && a.mode == EX_ALLY_FOLLOW);

  float player[3] = {0,0,0};
  float goal[3]; int atk;

  // no enemies -> follow, stays put when near the player
  int t = ex_ally_think(&a, player, NULL, NULL, 0, 0.1f, goal, &atk);
  assert(t == -1 && a.mode == EX_ALLY_FOLLOW && atk == 0);

  // player walks off past the leash -> ally's goal becomes the player
  float far_player[3] = {40,0,0};
  ex_ally_think(&a, far_player, NULL, NULL, 0, 0.1f, goal, &atk);
  assert(fabsf(goal[0]-40.0f) < 1e-3f && "follows player past the leash");
  printf("follow mode + leash OK\n");

  // an enemy in engage range -> engage it; goal is the enemy
  float epos[2][3] = { {10,0,0}, {200,0,0} };
  int alive[2] = {1,1};
  t = ex_ally_think(&a, player, epos, alive, 2, 0.1f, goal, &atk);
  assert(t == 0 && a.mode == EX_ALLY_ENGAGE);
  assert(fabsf(goal[0]-10.0f) < 1e-3f && "moves toward the target");
  printf("engage nearest OK (picked enemy 0)\n");

  // nearest picks the closer of two
  float epos2[2][3] = { {30,0,0}, {5,0,0} };
  t = ex_ally_think(&a, player, epos2, alive, 2, 0.1f, goal, &atk);
  assert(t == 1 && "picks the closer enemy");

  // in attack range -> lands a hit, then respects the cooldown
  a.pos[0]=0; a.pos[2]=0; a.attack_cd=0;
  float close[1][3] = { {4,0,0} }; int al1[1]={1};
  t = ex_ally_think(&a, player, close, al1, 1, 0.016f, goal, &atk);
  assert(t == 0 && atk == 1 && "attacks in range");
  int atk2; ex_ally_think(&a, player, close, al1, 1, 0.016f, goal, &atk2);
  assert(atk2 == 0 && "cooldown blocks immediate re-attack");
  printf("attack cadence OK\n");

  // dead enemies are ignored
  int aldead[1] = {0};
  t = ex_ally_think(&a, player, close, aldead, 1, 0.1f, goal, &atk);
  assert(t == -1 && a.mode == EX_ALLY_FOLLOW && "ignores dead enemies");

  // downed -> revives after the down clock
  a.hp = 0.0f;
  ex_ally_think(&a, player, close, al1, 1, 0.1f, goal, &atk);
  assert(a.mode == EX_ALLY_DOWN && atk == 0 && "goes down at 0 hp");
  for (int i=0;i<200;i++) ex_ally_think(&a, player, NULL, NULL, 0, 0.1f, goal, &atk);
  assert(a.hp == a.max_hp && a.mode == EX_ALLY_FOLLOW && "revives after down time");
  printf("downed -> revive OK\n");

  printf("\nALLY OK (target, engage/follow, cadence, revive)\n");
  return 0;
}
