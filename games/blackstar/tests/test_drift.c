/* test_drift.c -- the drift-sector rules: cadence, FULL theme coverage (the
   arithmetic that a jump-indexed stride gets wrong), name/seed determinism. */
#include "drift.h"
#include <stdio.h>
#include <assert.h>
#include <string.h>

int main(void){
  printf("=== drift sectors ===\n");
  /* cadence: exactly every 3rd jump, starting at jump 2 */
  int drifts = 0;
  for (int j = 0; j < 30; j++) drifts += ex_drift_is(j);
  assert(drifts == 10);
  assert(!ex_drift_is(0) && !ex_drift_is(1) && ex_drift_is(2) && ex_drift_is(5));

  /* base rotation: the first 12 drifts visit ALL 12 themes (stride 5 coprime to 12).
     This is the regression for the first draft's bug: a stride applied to the JUMP
     index (step 3) only ever visits 4 themes. */
  int seen[12] = {0};
  for (int k = 0; k < 12; k++) seen[ex_drift_base(k, 12)] = 1;
  for (int t = 0; t < 12; t++) assert(seen[t]);
  int jump_seen[12] = {0};
  for (int j = 2; j < 38; j += 3) jump_seen[(j * 7 + 3) % 12] = 1;   /* the buggy form */
  int cover = 0; for (int t = 0; t < 12; t++) cover += jump_seen[t];
  assert(cover == 4);   /* proves WHY the drift-index form is required */

  /* names: deterministic, and the greek cycle never repeats within 8 drifts */
  for (int k = 0; k < 8; k++)
    for (int k2 = k + 1; k2 < 8; k2++)
      assert(strcmp(ex_drift_greek(k), ex_drift_greek(k2)) != 0);
  assert(strcmp(ex_drift_greek(0), ex_drift_greek(8)) == 0);   /* cycle length 8 */

  /* seeds: deterministic per (jump, mission), distinct across a long patrol */
  for (int j = 0; j < 64; j++) {
    assert(ex_drift_seed(j, 3) == ex_drift_seed(j, 3));
    for (int j2 = j + 1; j2 < 64; j2++)
      assert(ex_drift_seed(j, 3) != ex_drift_seed(j2, 3));
    assert(ex_drift_seed(j, 3) != ex_drift_seed(j, 4));
  }
  printf("drift sector rules OK\n");
  return 0;
}
