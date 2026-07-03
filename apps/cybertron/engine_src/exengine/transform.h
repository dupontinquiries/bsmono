/* transform
  Robot <-> Vehicle mode state machine for playable characters.

  Each character owns two ex_model_t* (robot mesh, vehicle mesh)
  plus one ex_coll_chunk-style collider pair (robot collider AABB/
  capsule, vehicle collider AABB) and a camera profile per mode.
  This module owns switching between them: it does NOT own physics
  integration or input -- it exposes state + a request/lock API
  that the movement and camera systems read from.

  Locking model: transforming is not a cooldown-gated ability button,
  it's an instant toggle EXCEPT for a short forced lock-out after
  taking heavy damage (see EX_XFORM_LOCK_ON_HEAVY_HIT_SEC), so players
  can't transform-cancel out of every bad trade for free.
*/

#ifndef EX_TRANSFORM_H
#define EX_TRANSFORM_H

#include "render/model.h"
#include "math/octree.h"
#include "events.h"
#include <stdint.h>

#define EX_XFORM_BLEND_SEC             0.35f  // visual transform animation duration
#define EX_XFORM_LOCK_ON_HEAVY_HIT_SEC 1.2f    // forced lock-out after a heavy hit
#define EX_XFORM_MIN_INTERVAL_SEC      0.15f   // hard floor, prevents spam-toggling exploits

typedef enum {
  EX_MODE_ROBOT,
  EX_MODE_VEHICLE
} ex_xform_mode;

typedef enum {
  EX_XFORM_STATE_IDLE,        // settled in current mode, free to transform
  EX_XFORM_STATE_TRANSFORMING,// mid-blend, collider swap not yet applied
  EX_XFORM_STATE_LOCKED       // transform request denied (post-heavy-hit lockout)
} ex_xform_state;

// per-mode physical profile -- distinct collider + movement tuning
typedef struct {
  vec3 collider_half_extents;  // AABB half-size for this mode
  float move_speed;
  float turn_rate;              // radians/sec
  float camera_fov;
  float camera_height_offset;
} ex_xform_profile_t;

typedef struct {
  ex_model_t *robot_model;
  ex_model_t *vehicle_model;

  ex_xform_profile_t robot_profile;
  ex_xform_profile_t vehicle_profile;

  ex_xform_mode  mode;
  ex_xform_state state;

  float blend_timer;      // counts down during TRANSFORMING
  float lock_timer;       // counts down during LOCKED
  float since_last_xform; // for EX_XFORM_MIN_INTERVAL_SEC guard

  // size class scales collider + profile at spawn time, see transform.c
  int size_class; // 0=Mini 1=Standard 2=Heavy 3=Titan, matches design doc order

  ex_event_bus_t *event_bus; // optional, fires "character_transformed" for spectator/UI/net hooks
  int owner_id;               // player/entity id, passed as event int_arg
} ex_xform_t;

/**
 * [ex_xform_new create a transform controller for a character]
 * @param robot_model    [robot-mode model, ownership stays with caller]
 * @param vehicle_model  [vehicle-mode model, ownership stays with caller]
 * @param robot_profile  [robot-mode collider/movement tuning]
 * @param vehicle_profile[vehicle-mode collider/movement tuning]
 * @param size_class     [0..3, scales both profiles -- see transform.c]
 * @param owner_id        [entity/player id for event payloads]
 * @param event_bus        [optional, NULL if not networked/no listeners needed]
 */
ex_xform_t* ex_xform_new(ex_model_t *robot_model, ex_model_t *vehicle_model,
                          ex_xform_profile_t robot_profile, ex_xform_profile_t vehicle_profile,
                          int size_class, int owner_id, ex_event_bus_t *event_bus);

void ex_xform_free(ex_xform_t *x);

/**
 * [ex_xform_request called from input; may be denied]
 * @return [1 if the transform was accepted and began, 0 if denied]
 */
int ex_xform_request(ex_xform_t *x);

/**
 * [ex_xform_on_heavy_hit called from combat resolution when a "heavy" hit lands]
 * Forces LOCKED state for EX_XFORM_LOCK_ON_HEAVY_HIT_SEC, interrupting
 * any in-progress transform blend by snapping back to the pre-hit mode.
 */
void ex_xform_on_heavy_hit(ex_xform_t *x);

/**
 * [ex_xform_update advance timers/state, call once per frame]
 * @param x          [the controller]
 * @param delta_time []
 */
void ex_xform_update(ex_xform_t *x, float delta_time);

/**
 * [ex_xform_active_model convenience accessor for the renderer]
 */
static inline ex_model_t* ex_xform_active_model(ex_xform_t *x) {
  return x->mode == EX_MODE_ROBOT ? x->robot_model : x->vehicle_model;
}

/**
 * [ex_xform_active_profile convenience accessor for movement/camera]
 */
static inline ex_xform_profile_t* ex_xform_active_profile(ex_xform_t *x) {
  return x->mode == EX_MODE_ROBOT ? &x->robot_profile : &x->vehicle_profile;
}

/**
 * [ex_xform_can_transform quick query for UI (greys out the prompt, etc)]
 */
static inline int ex_xform_can_transform(const ex_xform_t *x) {
  return x->state == EX_XFORM_STATE_IDLE && x->since_last_xform >= EX_XFORM_MIN_INTERVAL_SEC;
}

#endif // EX_TRANSFORM_H
