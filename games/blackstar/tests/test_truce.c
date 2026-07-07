/* test_truce.c -- Megatron's truce trigger.
   Verifies: no truce without a boss; no truce while the boss is healthy; truce
   ignites once the boss is worn to/under the threshold. */
#include <stdio.h>
#include <assert.h>
#include "truce.h"
int main(void){
  // no boss -> never
  assert(ex_truce_active(0, 0.1f) == 0);
  assert(ex_truce_active(0, 1.0f) == 0);
  // boss present but healthy -> not yet
  assert(ex_truce_active(1, 1.0f) == 0);
  assert(ex_truce_active(1, EX_TRUCE_HP_TRIGGER + 0.01f) == 0);
  printf("healthy boss -> no truce OK\n");
  // boss worn to/under threshold -> truce
  assert(ex_truce_active(1, EX_TRUCE_HP_TRIGGER) == 1);
  assert(ex_truce_active(1, 0.3f) == 1);
  assert(ex_truce_active(1, 0.0f) == 1);
  printf("worn boss -> truce ignites OK\n");
  printf("\nTRUCE OK (boss present + worn down)\n");
  return 0;
}
