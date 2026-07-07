#include "transform.h"
#include <stdio.h>
#include <assert.h>

static void on_transformed(const ex_event_t *event, void *user_data)
{
  printf("[event] character_transformed owner=%d new_mode=%.0f\n",
         event->int_arg, event->float_arg);
}

int main(void)
{
  ex_event_bus_t *bus = ex_event_bus_new();
  ex_event_subscribe(bus, "character_transformed", on_transformed, NULL);

  ex_xform_profile_t robot   = { .collider_half_extents = {1,1,1}, .move_speed = 6.0f,  .turn_rate = 3.0f, .camera_fov = 70.0f, .camera_height_offset = 1.8f };
  ex_xform_profile_t vehicle = { .collider_half_extents = {2,1,3}, .move_speed = 18.0f, .turn_rate = 1.5f, .camera_fov = 85.0f, .camera_height_offset = 1.0f };

  // a Heavy-class character (size_class = 2)
  ex_xform_t *x = ex_xform_new(NULL, NULL, robot, vehicle, /*size_class*/ 2, /*owner_id*/ 7, bus);

  printf("initial mode=%d state=%d speed=%.2f (expect robot=0, idle=0, 6*0.85=5.10)\n",
         x->mode, x->state, x->robot_profile.move_speed);
  assert(x->mode == EX_MODE_ROBOT);

  // request transform, should begin blending
  int accepted = ex_xform_request(x);
  printf("request 1: accepted=%d state=%d (expect 1, TRANSFORMING=1)\n", accepted, x->state);
  assert(accepted == 1 && x->state == EX_XFORM_STATE_TRANSFORMING);

  // immediate second request during blend should be denied
  accepted = ex_xform_request(x);
  printf("request 2 (mid-blend): accepted=%d (expect 0)\n", accepted);
  assert(accepted == 0);

  // step time past the blend duration
  float dt = 0.1f;
  for (float t = 0; t < EX_XFORM_BLEND_SEC + dt; t += dt)
    ex_xform_update(x, dt);

  printf("after blend: mode=%d state=%d (expect vehicle=1, idle=0)\n", x->mode, x->state);
  assert(x->mode == EX_MODE_VEHICLE && x->state == EX_XFORM_STATE_IDLE);

  // simulate a heavy hit landing while idle in vehicle mode
  ex_xform_on_heavy_hit(x);
  printf("after heavy hit: state=%d mode=%d (expect LOCKED=2, mode unchanged=1)\n", x->state, x->mode);
  assert(x->state == EX_XFORM_STATE_LOCKED && x->mode == EX_MODE_VEHICLE);

  // request during lockout should be denied
  accepted = ex_xform_request(x);
  printf("request during lockout: accepted=%d (expect 0)\n", accepted);
  assert(accepted == 0);

  // step past the lockout window
  for (float t = 0; t < EX_XFORM_LOCK_ON_HEAVY_HIT_SEC + dt; t += dt)
    ex_xform_update(x, dt);

  printf("after lockout expires: state=%d (expect IDLE=0)\n", x->state);
  assert(x->state == EX_XFORM_STATE_IDLE);

  // now transform should be accepted again, and fire the event on completion
  accepted = ex_xform_request(x);
  for (float t = 0; t < EX_XFORM_BLEND_SEC + dt; t += dt)
    ex_xform_update(x, dt);
  printf("final mode=%d (expect robot=0)\n", x->mode);
  assert(x->mode == EX_MODE_ROBOT);

  printf("\nALL ASSERTIONS PASSED\n");

  ex_xform_free(x);
  ex_event_bus_free(bus);
  return 0;
}
