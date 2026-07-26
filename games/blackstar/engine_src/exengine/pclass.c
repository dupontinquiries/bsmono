#include "pclass.h"
#include "vehicle.h"

/* Vehicle enum order (from vehicle.h): CAR,TANK,JET,HOVER,INSECT,SPIDER,DRAGON,
 * DINO,RAPTOR,MECH,CYCLE,GUNSHIP,SCORPION,WOLF,HAWK,CRAB. Indices used below. */
static const ex_class_info_t CLASSES[EX_CLASS_COUNT] = {
  /* VANGUARD  */ { "VANGUARD", "heavy ground assault -- tanky, hits hard, slow",
                    { 1 /*TANK*/, 9 /*MECH*/, 0 /*CAR*/, 15 /*CRAB*/ },
                    1.40f, 0.85f, 110, 160, 140, 105 },
  /* INTERCEPT */ { "INTERCEPTOR", "aerial flanker -- fast, fragile, high mobility",
                    { 2 /*JET*/, 14 /*HAWK*/, 10 /*CYCLE*/, 3 /*HOVER*/ },
                    0.75f, 1.30f, 60, 100, 80, 110 },
  /* TITAN     */ { "TITAN", "beast bruiser -- balanced, relentless",
                    { 6 /*DRAGON*/, 7 /*DINO*/, 8 /*RAPTOR*/, 13 /*WOLF*/ },
                    1.20f, 1.00f, 95, 140, 120, 115 },
  /* SPECIALIST*/ { "SPECIALIST", "tech support -- utility, turrets, control",
                    { 11 /*GUNSHIP*/, 5 /*SPIDER*/, 12 /*SCORPION*/, 4 /*INSECT*/ },
                    0.90f, 1.05f, 80, 120, 100, 100 },
  /* REAVER    */ { "REAVER", "outer-rim predator -- harpoons, mobility, hit-and-run",
                    { 11 /*GUNSHIP*/, 14 /*HAWK*/, 10 /*CYCLE*/, 3 /*HOVER*/ },
                    0.90f, 1.20f, 70, 110, 90, 108 },
  /* WARDEN    */ { "WARDEN", "clone sentinel -- a wall that holds the line",
                    { 9 /*MECH*/, 15 /*CRAB*/, 1 /*TANK*/, 5 /*SPIDER*/ },
                    1.45f, 0.82f, 110, 160, 145, 108 },
  /* PYRE      */ { "PYRE", "fire clone -- a held star; devastating but fragile",
                    { 6 /*DRAGON*/, 2 /*JET*/, 14 /*HAWK*/, 8 /*RAPTOR*/ },
                    0.70f, 1.20f, 60, 100, 80, 112 },
  /* PRIME     */ { "PRIME", "the steward -- durable all-rounder who holds the line",
                    { 0 /*CAR*/, 1 /*TANK*/, 9 /*MECH*/, 3 /*HOVER*/ },
                    1.35f, 0.95f, 100, 150, 135, 112 },
  /* SCOUT     */ { "SCOUT", "recon -- fast and light, fragile, high mobility",
                    { 0 /*CAR*/, 10 /*CYCLE*/, 14 /*HAWK*/, 2 /*JET*/ },
                    0.72f, 1.35f, 55, 95, 75, 108 },
  /* SIEGE     */ { "SIEGE", "artillery bombard -- long-range shells, slow but sturdy",
                    { 1 /*TANK*/, 11 /*GUNSHIP*/, 15 /*CRAB*/, 9 /*MECH*/ },
                    1.25f, 0.75f, 105, 155, 135, 106 },
};

const ex_class_info_t *ex_class_info(ex_class_t c)
{
  if (c < 0 || c >= EX_CLASS_COUNT) c = EX_CLASS_VANGUARD;
  return &CLASSES[c];
}

int ex_class_allows_vehicle(ex_class_t c, int v)
{
  const ex_class_info_t *ci = ex_class_info(c);
  for (int i = 0; i < EX_CLASS_VEHICLES; i++)
    if (ci->vehicles[i] == v) return 1;
  return 0;
}

int ex_class_cycle_vehicle(ex_class_t c, int current, int dir)
{
  const ex_class_info_t *ci = ex_class_info(c);
  int idx = 0;
  for (int i = 0; i < EX_CLASS_VEHICLES; i++)
    if (ci->vehicles[i] == current) { idx = i; break; }
  idx = (idx + dir + EX_CLASS_VEHICLES) % EX_CLASS_VEHICLES;
  return ci->vehicles[idx];
}

int ex_class_clamp_bulk(ex_class_t c, int bulk)
{
  const ex_class_info_t *ci = ex_class_info(c);
  if (bulk < ci->bulk_min) return ci->bulk_min;
  if (bulk > ci->bulk_max) return ci->bulk_max;
  return bulk;
}
