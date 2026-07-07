#include "absorb.h"

float ex_absorb_accumulate(float grow, float amount)
{
  if (grow < 1.0f) grow = 1.0f;
  if (amount < 0.0f) amount = 0.0f;
  float span = EX_ABSORB_GROW_MAX - 1.0f;               /* headroom range */
  float frac = (span > 0.0f) ? (grow - 1.0f) / span : 1.0f;  /* 0..1 fullness */
  grow += amount * (1.0f - frac);                        /* diminishing returns */
  if (grow > EX_ABSORB_GROW_MAX) grow = EX_ABSORB_GROW_MAX;
  return grow;
}

ex_absorb_stats_t ex_absorb_stats(float grow)
{
  float g = grow; if (g < 1.0f) g = 1.0f; if (g > EX_ABSORB_GROW_MAX) g = EX_ABSORB_GROW_MAX;
  float span = EX_ABSORB_GROW_MAX - 1.0f;
  float t = (span > 0.0f) ? (g - 1.0f) / span : 0.0f;    /* 0 at base, 1 at cap */
  ex_absorb_stats_t s;
  s.scale   = 1.0f + t * 0.80f;   /* up to +80% larger */
  s.dmg_mul = 1.0f + t * 1.00f;   /* up to +100% melee damage */
  return s;
}
