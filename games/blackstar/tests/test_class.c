#include "exengine/pclass.h"
#include "exengine/chassis.h"
#include <stdio.h>
#include <assert.h>
int main(void){
  // each class has 4 distinct allowed vehicles + valid multipliers
  for (int c=0;c<EX_CLASS_COUNT;c++){
    const ex_class_info_t *ci = ex_class_info(c);
    assert(ci->name && ci->hp_mul>0.5f && ci->speed_mul>0.5f);
    for (int i=0;i<EX_CLASS_VEHICLES;i++)
      assert(ex_class_allows_vehicle(c, ci->vehicles[i]));
  }
  // cycling stays within the class's set
  int v = ex_class_info(EX_CLASS_INTERCEPTOR)->vehicles[0];
  for (int i=0;i<10;i++){ v = ex_class_cycle_vehicle(EX_CLASS_INTERCEPTOR, v, 1);
    assert(ex_class_allows_vehicle(EX_CLASS_INTERCEPTOR, v)); }
  // bulk clamps to class range
  assert(ex_class_clamp_bulk(EX_CLASS_VANGUARD, 40) == ex_class_info(EX_CLASS_VANGUARD)->bulk_min);
  assert(ex_class_clamp_bulk(EX_CLASS_INTERCEPTOR, 200) == ex_class_info(EX_CLASS_INTERCEPTOR)->bulk_max);
  for (int i=0;i<EX_CHASSIS_COUNT;i++){ const ex_chassis_info_t *ci=ex_chassis_info(i);
    assert(ci->name && ci->bulk_mul>0.5f && ci->fold_stagger>0.2f && ci->xform_dur_mul>0.5f); }
  printf("chassis: %d frame variants OK\n", EX_CHASSIS_COUNT);
  printf("class system: %d profiles, gated vehicles + builds OK\n", EX_CLASS_COUNT);
  printf("ALL ASSERTIONS PASSED\n");
  return 0;
}
