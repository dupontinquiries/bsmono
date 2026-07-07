/* material.h -- surface materials. Each carries the physical properties that
 * drive friction (with moisture + grit), and impact response (hardness decides
 * spark vs dust vs ring). Themes map to a ground material; the friction model
 * combines the material with the actor's mass, stance and lean. */
#ifndef EX_MATERIAL_H
#define EX_MATERIAL_H

#include <stdint.h>

typedef enum {
  EX_MAT_METAL = 0,   /* forged plating -- grippy, hard, sparks */
  EX_MAT_ROCK,        /* stone/waste -- firm, gritty */
  EX_MAT_ICE,         /* frozen -- slick, wet, rings */
  EX_MAT_SAND,        /* loose grit -- draggy but slides when fast */
  EX_MAT_ORGANIC,     /* reclaimed flesh-metal -- soft, damp, squelch */
  EX_MAT_CRYSTAL,     /* energon crystal -- hard, glows on impact */
  EX_MAT_COUNT
} ex_material_id;

typedef struct {
  const char *name;
  float friction;     /* base grip on this surface (1 = firm) */
  float restitution;  /* bounciness of impacts (0 dead .. 1 springy) */
  float hardness;     /* 0 soft (thud/dust) .. 1 hard (spark/ring) */
  float moisture;     /* 0 dry .. 1 wet (lowers friction, splashes) */
  float grit;         /* 0 smooth .. 1 loose (dust, grip falls off at speed) */
  uint8_t spark[3];   /* impact particle colour */
} ex_material_t;

const ex_material_t *ex_material(ex_material_id id);

/* Map a theme/map id to its ground material. */
ex_material_id ex_material_for_theme(const char *theme);

/* Effective grip on a surface for an actor:
 *   base material friction, reduced by moisture and by grit at speed, scaled by
 *   how planted the stance is (foot area) and reduced as the actor leans off
 *   its footprint. Returns a multiplier ~0.2 .. 1.3. */
float ex_material_grip(ex_material_id id, float mass, float speed,
                       float foot_area, float lean);

#endif
