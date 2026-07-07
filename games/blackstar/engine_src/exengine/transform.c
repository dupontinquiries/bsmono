#include "transform.h"
#include <stdlib.h>
#include <string.h>

// size class scalars applied to collider extents + move speed at spawn,
// matching the design doc's Mini/Standard/Heavy/Titan tiers.
static const float ex_size_scale[4]      = { 0.5f, 1.0f, 1.7f, 3.0f };
static const float ex_size_speed_mod[4]  = { 1.15f, 1.0f, 0.85f, 0.65f }; // smaller = faster, Titan = slow

ex_xform_t* ex_xform_new(ex_model_t *robot_model, ex_model_t *vehicle_model,
                          ex_xform_profile_t robot_profile, ex_xform_profile_t vehicle_profile,
                          int size_class, int owner_id, ex_event_bus_t *event_bus)
{
  ex_xform_t *x = malloc(sizeof(ex_xform_t));
  memset(x, 0, sizeof(ex_xform_t));

  x->robot_model = robot_model;
  x->vehicle_model = vehicle_model;

  if (size_class < 0) size_class = 0;
  if (size_class > 3) size_class = 3;
  x->size_class = size_class;

  float scale = ex_size_scale[size_class];
  float speed_mod = ex_size_speed_mod[size_class];

  robot_profile.collider_half_extents[0] *= scale;
  robot_profile.collider_half_extents[1] *= scale;
  robot_profile.collider_half_extents[2] *= scale;
  robot_profile.move_speed *= speed_mod;

  vehicle_profile.collider_half_extents[0] *= scale;
  vehicle_profile.collider_half_extents[1] *= scale;
  vehicle_profile.collider_half_extents[2] *= scale;
  vehicle_profile.move_speed *= speed_mod;

  x->robot_profile = robot_profile;
  x->vehicle_profile = vehicle_profile;

  x->mode = EX_MODE_ROBOT;
  x->state = EX_XFORM_STATE_IDLE;
  x->blend_timer = 0.0f;
  x->lock_timer = 0.0f;
  x->since_last_xform = EX_XFORM_MIN_INTERVAL_SEC; // allow an immediate first transform

  x->owner_id = owner_id;
  x->event_bus = event_bus;

  return x;
}

void ex_xform_free(ex_xform_t *x)
{
  free(x);
}

int ex_xform_request(ex_xform_t *x)
{
  if (!ex_xform_can_transform(x))
    return 0;

  x->state = EX_XFORM_STATE_TRANSFORMING;
  x->blend_timer = EX_XFORM_BLEND_SEC;
  x->since_last_xform = 0.0f;

  // NOTE: actual mode flip + collider swap happens at the end of the
  // blend in ex_xform_update(), not here -- this only begins the blend.
  // The renderer/animation layer should crossfade robot_model <-> vehicle_model
  // over EX_XFORM_BLEND_SEC using ex_mix_pose()-style blending if the two
  // skeletons are compatible, or a simple visual transform FX otherwise.

  return 1;
}

void ex_xform_on_heavy_hit(ex_xform_t *x)
{
  // interrupts an in-progress transform and denies new requests for a while;
  // does NOT flip the character's current mode, it just cancels the blend.
  x->state = EX_XFORM_STATE_LOCKED;
  x->lock_timer = EX_XFORM_LOCK_ON_HEAVY_HIT_SEC;
  x->blend_timer = 0.0f;
}

void ex_xform_update(ex_xform_t *x, float delta_time)
{
  x->since_last_xform += delta_time;

  switch (x->state) {
    case EX_XFORM_STATE_TRANSFORMING:
      x->blend_timer -= delta_time;
      if (x->blend_timer <= 0.0f) {
        // blend complete -- commit the mode flip + collider swap now
        x->mode = (x->mode == EX_MODE_ROBOT) ? EX_MODE_VEHICLE : EX_MODE_ROBOT;
        x->state = EX_XFORM_STATE_IDLE;
        x->blend_timer = 0.0f;

        if (x->event_bus) {
          ex_event_fire(x->event_bus, "character_transformed",
                         x->owner_id, (float)x->mode);
        }
      }
      break;

    case EX_XFORM_STATE_LOCKED:
      x->lock_timer -= delta_time;
      if (x->lock_timer <= 0.0f) {
        x->state = EX_XFORM_STATE_IDLE;
        x->lock_timer = 0.0f;
      }
      break;

    case EX_XFORM_STATE_IDLE:
    default:
      break;
  }
}
