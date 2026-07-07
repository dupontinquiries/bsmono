#include "truce.h"

int ex_truce_active(int boss_present, float boss_hp_frac)
{
  if (!boss_present) return 0;
  return (boss_hp_frac <= EX_TRUCE_HP_TRIGGER) ? 1 : 0;
}
