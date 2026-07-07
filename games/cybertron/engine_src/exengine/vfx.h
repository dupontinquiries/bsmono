/* vfx
  A small pool of short-lived visual effects that make combat readable:
    - BOLT   : a projectile that travels muzzle->target then bursts (hitscan
               damage is applied instantly on fire; the bolt is the cosmetic
               tracer that sells the shot)
    - FLASH  : a brief muzzle flash at the shooter
    - IMPACT : a puff spawned where a shot lands (spawns DEBRIS)
    - DEBRIS : scattered bits with gravity (impact sparks + enemy death)

  All effects are uniform-scale points, because exengine models only carry a
  single scale float -- so no stretched beams, just fast-moving bolts and
  scattered bits. The pool is fixed-size and dependency-light (floats only),
  so the lifetime/integration logic is testable headless; the game maps each
  live effect onto a pooled model each frame.
*/

#ifndef EX_VFX_H
#define EX_VFX_H

#include <stdint.h>

#define EX_VFX_MAX 128

typedef enum {
  EX_VFX_NONE = 0,
  EX_VFX_BOLT,
  EX_VFX_FLASH,
  EX_VFX_IMPACT,
  EX_VFX_DEBRIS
} ex_vfx_kind;

typedef struct {
  ex_vfx_kind kind;
  float pos[3];
  float vel[3];
  float life, max_life;
  float scale;
  float end_dist;   // BOLT: distance at which it bursts
  float travelled;  // BOLT: distance covered so far
  int   gravity;    // DEBRIS: apply gravity
  uint8_t color[3]; // tint hint for the game (e.g. element color)
  int   active;
} ex_vfx_t;

typedef struct {
  ex_vfx_t items[EX_VFX_MAX];
  int      spawn_impacts; // internal: impacts queued by expiring bolts
} ex_vfx_pool_t;

void ex_vfx_init(ex_vfx_pool_t *p);

/**
 * [ex_vfx_spawn_shot muzzle flash + a travelling bolt toward a hit point]
 * @param from     [muzzle position]
 * @param dir      [normalized fire direction]
 * @param dist     [distance to the impact point]
 * @param col      [3-byte tint (element color)]
 */
void ex_vfx_spawn_shot(ex_vfx_pool_t *p, const float from[3], const float dir[3],
                       float dist, const uint8_t col[3]);

/**
 * [ex_vfx_spawn_impact a burst of debris at a point]
 */
void ex_vfx_spawn_impact(ex_vfx_pool_t *p, const float at[3], int count, const uint8_t col[3]);

/**
 * [ex_vfx_spawn_death a bigger debris burst for a destroyed enemy]
 */
void ex_vfx_spawn_death(ex_vfx_pool_t *p, const float at[3], const uint8_t col[3]);

/**
 * [ex_vfx_update advance all effects; bolts that reach their target burst]
 */
void ex_vfx_update(ex_vfx_pool_t *p, float dt);

/**
 * [ex_vfx_count number of active effects (for the game to map to models)]
 */
int ex_vfx_count(const ex_vfx_pool_t *p);

#endif // EX_VFX_H
