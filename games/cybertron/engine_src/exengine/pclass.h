/* pclass.h -- player classes / profiles. Four archetypes, each gating which
 * vehicle forms and build proportions are available, plus base HP/speed. */
#ifndef EX_PCLASS_H
#define EX_PCLASS_H

typedef enum {
  EX_CLASS_VANGUARD = 0,   // heavy ground assault: tanky, slower
  EX_CLASS_INTERCEPTOR,    // aerial flanker: fast, fragile
  EX_CLASS_TITAN,          // beast bruiser: balanced heavy
  EX_CLASS_SPECIALIST,     // tech/support: utility
  EX_CLASS_REAVER,         // outer-rim predator (Blackstar): harpoons, mobile, hit-and-run
  EX_CLASS_WARDEN,         // clone sentinel (the Morpher): a wall that holds the line
  EX_CLASS_PYRE,           // fire clone (Billy): a held star -- devastating but fragile
  EX_CLASS_PRIME,          // the steward (Optimus): durable all-rounder, holds the line
  EX_CLASS_SCOUT,          // recon (Bumblebee): fast, light, fragile
  EX_CLASS_COUNT
} ex_class_t;

#define EX_CLASS_VEHICLES 4   // gated vehicle options per class

typedef struct {
  const char *name;
  const char *tagline;
  int   vehicles[EX_CLASS_VEHICLES];  // allowed ex_vehicle_type indices
  float hp_mul;                       // base-HP multiplier
  float speed_mul;                    // movement multiplier
  int   bulk_min, bulk_max;           // build gating: allowed bulk range (percent)
  int   base_bulk, base_height;       // default proportions
} ex_class_info_t;

/* Get the (static) info for a class. */
const ex_class_info_t *ex_class_info(ex_class_t c);

/* Is vehicle index v allowed for class c? */
int ex_class_allows_vehicle(ex_class_t c, int v);

/* Snap a vehicle index to the nearest allowed one for the class (cycling by
 * dir = +1/-1 through the class's allowed set). */
int ex_class_cycle_vehicle(ex_class_t c, int current, int dir);

/* Clamp a build bulk value into the class's allowed range. */
int ex_class_clamp_bulk(ex_class_t c, int bulk);

#endif
