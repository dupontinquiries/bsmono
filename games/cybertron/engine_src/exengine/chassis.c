#include "chassis.h"

static const ex_chassis_info_t CH[EX_CHASSIS_COUNT] = {
  /* name          h     bulk  arm   leg   head  shldr  stag  spin  dur   grb glow */
  { "STANDARD",   1.00f,1.00f,1.00f,1.00f,1.00f,1.00f, 0.50f,220.f,1.00f, 0, 1.0f },
  { "ANGULAR",    1.02f,0.88f,1.06f,1.02f,0.94f,1.05f, 0.38f,300.f,0.78f, 1, 1.35f },
  { "JUGGERNAUT", 0.96f,1.28f,1.10f,0.94f,0.90f,1.28f, 0.58f,160.f,1.35f, 2, 0.75f },
  { "STRIDER",    1.12f,0.82f,1.04f,1.12f,1.02f,0.90f, 0.48f,240.f,0.92f, 0, 1.1f },
};

const ex_chassis_info_t *ex_chassis_info(ex_chassis_t c)
{
  if (c < 0 || c >= EX_CHASSIS_COUNT) c = EX_CHASSIS_STANDARD;
  return &CH[c];
}
