/* test_gates.c -- buy-a-door map expansion economy. */
#include "gates.h"
#include <stdio.h>
#include <assert.h>

int main(void){
  printf("=== gates ===\n");
  ex_gate_t gates[EX_GATE_COUNT];
  ex_gates_init(gates);
  assert(ex_gates_extent(gates) == 60.0f);        // nothing open yet -> base arena
  assert(ex_gates_next_locked(gates) == 0);

  long wallet = 100;
  assert(!ex_gate_try_open(&gates[0], &wallet));  // can't afford gate 0 (cost 300)
  assert(wallet == 100 && !gates[0].opened);

  wallet = 300;
  assert(ex_gate_try_open(&gates[0], &wallet));   // exact cost -> opens
  assert(wallet == 0 && gates[0].opened);
  assert(ex_gates_extent(gates) == gates[0].extent);
  assert(ex_gates_next_locked(gates) == 1);

  assert(!ex_gate_try_open(&gates[0], &wallet));  // already open -> no double-spend
  assert(wallet == 0);

  wallet = 750;
  assert(ex_gate_try_open(&gates[1], &wallet));
  assert(wallet == 0 && ex_gates_extent(gates) == gates[1].extent);  // bigger wins

  wallet = 1500;
  assert(ex_gate_try_open(&gates[2], &wallet));
  assert(ex_gates_extent(gates) == gates[2].extent);
  assert(ex_gates_next_locked(gates) == -1);      // all open
  printf("  gate chain: %.0f -> %.0f -> %.0f\n", gates[0].extent, gates[1].extent, gates[2].extent);

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
