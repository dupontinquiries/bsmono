/* mapfsm
  Intent-driven map feature placement, ported from early-fps's fsm.rs.

  The core idea: a map feature is an *agent* that reads its Site (the
  environment at a spot -- sightline, slope, high-ground, traffic, wetness)
  and decides whether it should exist there and what form it takes. Instead
  of hand-placing cover, you scatter candidate sites and let each feature
  FSM gate itself in.

  In fsm.rs a grown feature returns a Dressing (mesh + obstacles). Here in
  the C engine the useful output is a *chunk spawn decision*: whether to
  place a destructible/cover chunk at this site, how big, and how tough --
  which plugs straight into coll_chunk.h. Visual dressing stays on the Rust
  side (via FFI) or the engine's own mesh code; this module owns the
  gameplay-affecting placement logic so it's testable headless.

  Two ways to drive it:
    1. Pure C: the ex_fsm_* functions below replicate fsm.rs's gating.
    2. Rust FFI: if built with the ffi bridge, ex_fsm_grow_ffi() calls the
       real fsm.rs through a C ABI so authoring stays in Rust.
*/

#ifndef EX_MAPFSM_H
#define EX_MAPFSM_H

#include "math/mathlib.h"
#include "coll_chunk.h"
#include <stdint.h>

// Environment query for one candidate spot -- mirrors fsm.rs::Site.
typedef struct {
  float x, z, ground_y;
  float sightline;     // longest clear sightline through here (world units)
  float danger_yaw;    // bearing of the most dangerous open sightline
  int   high_traffic;  // busy crossing where cover matters
  int   is_high_point; // local elevation maximum
  float slope;         // 0..1 terrain steepness
  int   wet;           // near water/mud
  int   indoor;        // enclosed
  uint32_t seed;
} ex_site_t;

// Which feature grew here (or none). Matches the fsm.rs feature set.
typedef enum {
  EX_FEATURE_NONE = 0,
  EX_FEATURE_SIGHTLINE_BREAKER, // cover to interrupt a long lane
  EX_FEATURE_VANTAGE,           // high-ground vantage
  EX_FEATURE_SENTINEL,          // fortified position on a slope over open ground
  EX_FEATURE_LOGISTICS_SPINE,   // tall landmark on the high point
  EX_FEATURE_WRECKAGE,          // sightline-breaking wreck on a long lane
  EX_FEATURE_SHELTER            // makeshift shelter on open, gentle ground
} ex_feature_t;

// The gameplay-affecting result of growing a feature: a chunk to spawn.
typedef struct {
  ex_feature_t feature;   // EX_FEATURE_NONE => nothing grows here
  int   spawn_chunk;      // 1 if this feature contributes a collision chunk
  int   destructible;     // 1 if the chunk can be destroyed
  float half_extents[3];  // chunk AABB half-size
  float hp;               // chunk hp (if destructible)
  uint32_t hit_mask;      // which hit types can damage it (ex_chunk_hit_mask)
} ex_fsm_result_t;

/**
 * [ex_fsm_grow run every feature FSM against a site, first that fits wins]
 * Pure-C port of fsm.rs's gating rules. Deterministic given the site.
 */
ex_fsm_result_t ex_fsm_grow(const ex_site_t *site);

/**
 * [ex_fsm_grow_into_world grow a site and, if it produces a chunk, add it
 *  to the given chunk world with a generated name. Returns the feature.]
 * @param world [chunk world to add to]
 * @param site  [the candidate spot]
 * @param index [used to make a unique chunk name, e.g. site index]
 */
ex_feature_t ex_fsm_grow_into_world(ex_chunk_world_t *world, const ex_site_t *site, int index);

/**
 * [ex_feature_name human-readable feature label]
 */
const char* ex_feature_name(ex_feature_t f);

#ifdef EX_HAVE_RUST_FSM
/**
 * [ex_fsm_grow_ffi grow via the real Rust fsm.rs over the C ABI]
 * Only available when linked with the librust_fsm bridge. Same result
 * shape; the Rust side is the source of truth for the gating + form.
 */
ex_fsm_result_t ex_fsm_grow_ffi(const ex_site_t *site);
#endif

#endif // EX_MAPFSM_H
