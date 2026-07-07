/* test_pickup.c -- pickups bob, collect on proximity, once each. */
#include "pickup.h"
#include <stdio.h>
#include <assert.h>
#include <math.h>

int main(void){
  ex_pickup_field_t f; ex_pickup_field_init(&f);
  ex_pickup_add(&f, EX_PICKUP_ENERGON, 10,0.6f,0);
  ex_pickup_add(&f, EX_PICKUP_SHIELD, -20,0.6f,5);
  ex_pickup_add(&f, EX_PICKUP_OVERCHARGE, 0,0.6f,30);
  assert(f.count==3);

  // far away: no collect
  assert(ex_pickup_try_collect(&f, 0,1,0, 2.2f) == -1);
  // stand on the energon
  int got = ex_pickup_try_collect(&f, 10,1,0, 2.2f);
  printf("collected kind=%d (expect %d energon)\n", got, EX_PICKUP_ENERGON);
  assert(got == EX_PICKUP_ENERGON);
  // can't collect the same one twice
  assert(ex_pickup_try_collect(&f, 10,1,0, 2.2f) == -1);
  assert(!f.items[0].alive);

  // bob animates over time
  float b0 = ex_pickup_bob(&f.items[1]);
  ex_pickup_update(&f, 0.5f);
  float b1 = ex_pickup_bob(&f.items[1]);
  assert(b0 != b1);

  // colors differ per kind
  uint8_t c1[3],c2[3]; ex_pickup_color(EX_PICKUP_ENERGON,c1); ex_pickup_color(EX_PICKUP_SHIELD,c2);
  assert(c1[0]!=c2[0] || c1[2]!=c2[2]);

  // capacity cap
  ex_pickup_field_t big; ex_pickup_field_init(&big);
  for(int i=0;i<40;i++) ex_pickup_add(&big, EX_PICKUP_ENERGON, i,0.6f,0);
  assert(big.count == EX_MAX_PICKUPS);
  printf("capacity capped at %d\n", EX_MAX_PICKUPS);

  printf("\nALL ASSERTIONS PASSED\n");
  return 0;
}
