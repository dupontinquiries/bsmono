/* coll_chunk
  Mutable, destructible collision geometry layered on top of
  exengine's static baked octree.

  The main level octree (math/octree.h) is built once via
  ex_octree_build() then flattened via ex_octree_finalize() --
  fast to query, but not meant to change at runtime.

  Chunks defined here are small, individually-toggleable pieces
  of collision geometry (a destructible wall, a bridge segment,
  a blast door) that live OUTSIDE the baked octree and are
  checked separately. Disabling a chunk is an O(1) flag flip,
  not a rebuild of the level's collision data.

  Server-authoritative: chunk state changes (damage, break,
  event-triggered open/close) should only ever be applied locally
  after being confirmed by the server/host, then replicated.
*/

#ifndef EX_COLL_CHUNK_H
#define EX_COLL_CHUNK_H

#include "math/mathlib.h"
#include "math/collision.h"
#include "math/octree.h"
#include "util/list.h"
#include <stdint.h>

#define EX_CHUNK_NAME_MAX 48

// MATERIAL CLASS + STRENGTH: gates how much a hit bites. Strength tiers follow
// the design spec: human/terrain 0-100, cybertronian 100-500, clone 1000-10000.
// A hit far below a material's strength is mostly shrugged off (you can scratch
// clone hull with a blaster, but not shred it), giving tiered destructibility.
typedef enum {
  EX_MATCLASS_HUMAN = 0,     // concrete, glass, cars, rebar
  EX_MATCLASS_TERRAIN,       // rock, ice, sand berms
  EX_MATCLASS_CYBERTRONIAN,  // Autobot/Decepticon armour, energon structures
  EX_MATCLASS_CLONE,         // clone-forged / Watch material -- near-indestructible
  EX_MATCLASS_COUNT
} ex_matclass;

static inline float ex_matclass_default_strength(ex_matclass m) {
  switch (m) {
    case EX_MATCLASS_HUMAN:        return 60.0f;
    case EX_MATCLASS_TERRAIN:      return 80.0f;
    case EX_MATCLASS_CYBERTRONIAN: return 280.0f;
    case EX_MATCLASS_CLONE:        return 4000.0f;
    default:                  return 60.0f;
  }
}

// bitmask -- which classes/damage sources can affect this chunk
typedef enum {
  EX_CHUNK_HIT_MINI      = 1 << 0,
  EX_CHUNK_HIT_STANDARD  = 1 << 1,
  EX_CHUNK_HIT_HEAVY     = 1 << 2,
  EX_CHUNK_HIT_TITAN     = 1 << 3,
  EX_CHUNK_HIT_VEHICLE_RAM = 1 << 4,
  EX_CHUNK_HIT_SPLASH    = 1 << 5,
  EX_CHUNK_HIT_ANY       = 0xFF
} ex_chunk_hit_mask;

typedef enum {
  EX_CHUNK_STATE_INTACT,     // default, full collision + render
  EX_CHUNK_STATE_DAMAGED,    // cosmetic/mid-state, collision still on
  EX_CHUNK_STATE_DESTROYED,  // collision off, destroyed mesh shown
  EX_CHUNK_STATE_SEALED,     // event-gated closed (e.g. blast door shut)
  EX_CHUNK_STATE_OPEN        // event-gated open (e.g. blast door open)
} ex_chunk_state;

typedef struct {
  char name[EX_CHUNK_NAME_MAX];  // unique id, e.g. "velatris_bridge_02"
  rect_t bounds;                 // AABB for broad-phase test

  // collision geometry for this chunk only (small triangle set)
  vec3 *tris;                    // flat array, 3 verts per triangle
  int tri_count;

  ex_chunk_state state;
  uint32_t hit_mask;              // what can damage/affect this chunk

  float hp, max_hp;               // hp <= 0 with DESTROYED-eligible -> destroyed
  int destructible;                // 0 = event-gated only, ignores hp/hit_mask

  ex_matclass material;      // what this chunk is made of (gates hit penetration)
  float strength;                  // damage-per-hit threshold to bite fully (see tiers above)

  // optional: event name that flips this chunk (open/seal/destroy)
  // empty string if purely damage-driven
  char trigger_event[EX_CHUNK_NAME_MAX];
  ex_chunk_state trigger_target_state;

  int dirty;                       // 1 if state changed this frame (for net sync / vfx)
} ex_coll_chunk_t;

typedef struct {
  ex_coll_chunk_t **chunks;
  int count, capacity;
} ex_chunk_world_t;

/**
 * [ex_chunk_world_new create an empty chunk world]
 */
ex_chunk_world_t* ex_chunk_world_new(void);

/**
 * [ex_chunk_world_free release all chunks and the world]
 */
void ex_chunk_world_free(ex_chunk_world_t *world);

/**
 * [ex_chunk_add register a new destructible/gated chunk]
 * @param world       [the chunk world]
 * @param name        [unique chunk id]
 * @param bounds      [AABB for broad-phase]
 * @param tris         [triangle verts, copied in]
 * @param tri_count    [number of triangles]
 * @param hp           [starting/max hp, ignored if destructible == 0]
 * @param hit_mask     [what can affect this chunk]
 * @param destructible [1 = hp/damage driven, 0 = event-only]
 * @return             [pointer to the new chunk, owned by world]
 */
ex_coll_chunk_t* ex_chunk_add(ex_chunk_world_t *world, const char *name,
                               rect_t bounds, vec3 *tris, int tri_count,
                               float hp, uint32_t hit_mask, int destructible);

/**
 * [ex_chunk_find lookup by name]
 */
ex_coll_chunk_t* ex_chunk_find(ex_chunk_world_t *world, const char *name);

/**
 * [ex_chunk_apply_damage server-authoritative damage application]
 * @param chunk      [target chunk]
 * @param amount     [damage amount]
 * @param source_mask [what kind of hit this is, checked against chunk->hit_mask]
 * @return           [1 if the chunk transitioned to DESTROYED this call]
 */
int ex_chunk_apply_damage(ex_coll_chunk_t *chunk, float amount, uint32_t source_mask);

/**
 * [ex_chunk_set_material tag a chunk's material class + strength. If strength
 *  <= 0, the class default is used. Governs how much each hit penetrates.]
 */
void ex_chunk_set_material(ex_coll_chunk_t *chunk, ex_matclass m, float strength);

/**
 * [ex_matclass_effective_damage how much of `amount` actually bites a material
 *  of the given strength. At/above strength -> full; below -> falls off
 *  sharply, with a hard floor so a pea-shooter can't chip clone armour.]
 */
float ex_matclass_effective_damage(float amount, float strength);

/**
 * [ex_chunk_set_state force a state change, e.g. from an event trigger]
 */
void ex_chunk_set_state(ex_coll_chunk_t *chunk, ex_chunk_state new_state);

/**
 * [ex_chunk_is_solid whether this chunk currently blocks movement]
 */
static inline int ex_chunk_is_solid(const ex_coll_chunk_t *chunk) {
  return chunk->state == EX_CHUNK_STATE_INTACT ||
         chunk->state == EX_CHUNK_STATE_DAMAGED ||
         chunk->state == EX_CHUNK_STATE_SEALED;
}

/**
 * [ex_chunk_world_collide broad+narrow phase against all active chunks]
 * Call this alongside the existing static-octree collision walk;
 * results feed into the same ex_coll_packet_t used for the level.
 * @param world  [the chunk world]
 * @param packet [collision packet, shared with static geometry pass]
 */
void ex_chunk_world_collide(ex_chunk_world_t *world, ex_coll_packet_t *packet);

/**
 * [ex_chunk_world_clear_dirty call once per frame after net sync/vfx applied]
 */
void ex_chunk_world_clear_dirty(ex_chunk_world_t *world);

#endif // EX_COLL_CHUNK_H
