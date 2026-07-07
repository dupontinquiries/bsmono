/* example_usage.c
  Shows how the chunk world + event bus wire together for two
  of the design doc's scenarios:
    1. A Heavy-class melee hit chipping away a destructible wall.
    2. An "airship_destroyed" event permanently sealing/opening a path.

  This is illustrative wiring, not a full game loop.
*/

#include "coll_chunk.h"
#include "events.h"
#include <stdio.h>

// called when the "airship_destroyed" event fires for the mid lane
static void on_airship_destroyed(const ex_event_t *event, void *user_data)
{
  ex_coll_chunk_t *bridge = (ex_coll_chunk_t*)user_data;

  if (event->int_arg != 0 /* lane == mid */)
    return;

  printf("[event] airship destroyed over mid lane -- wreckage seals bridge '%s'\n", bridge->name);
  ex_chunk_set_state(bridge, EX_CHUNK_STATE_SEALED);
}

int main(void)
{
  ex_chunk_world_t *world = ex_chunk_world_new();
  ex_event_bus_t *bus = ex_event_bus_new();

  // a simple two-triangle quad standing in for a destructible wall
  vec3 wall_tris[6] = {
    {0,0,0}, {2,0,0}, {2,3,0},
    {0,0,0}, {2,3,0}, {0,3,0},
  };
  rect_t wall_bounds = ex_rect_new((vec3){0,0,-0.1f}, (vec3){2,3,0.1f});

  ex_coll_chunk_t *wall = ex_chunk_add(world, "velatris_wall_heavy_01",
                                        wall_bounds, wall_tris, 2,
                                        /*hp*/ 300.0f,
                                        /*hit_mask*/ EX_CHUNK_HIT_HEAVY | EX_CHUNK_HIT_TITAN | EX_CHUNK_HIT_VEHICLE_RAM,
                                        /*destructible*/ 1);

  // a bridge that's only ever changed by the airship-destroyed event, not damage
  ex_coll_chunk_t *bridge = ex_chunk_add(world, "velatris_bridge_02",
                                          ex_rect_new((vec3){10,0,0}, (vec3){14,1,4}),
                                          wall_tris, 2, /*hp*/ 0.0f,
                                          /*hit_mask*/ 0, /*destructible*/ 0);

  ex_event_subscribe(bus, "airship_destroyed", on_airship_destroyed, bridge);

  // scenario 1: a Standard-class hit does nothing (wrong hit_mask)
  ex_chunk_apply_damage(wall, 150.0f, EX_CHUNK_HIT_STANDARD);
  printf("after Standard hit: hp=%.0f state=%d (should be unchanged)\n", wall->hp, wall->state);

  // a Heavy-class hit does damage
  ex_chunk_apply_damage(wall, 150.0f, EX_CHUNK_HIT_HEAVY);
  printf("after Heavy hit: hp=%.0f state=%d (expect DAMAGED=1)\n", wall->hp, wall->state);

  // finishing blow destroys it
  int destroyed = ex_chunk_apply_damage(wall, 200.0f, EX_CHUNK_HIT_HEAVY);
  printf("after 2nd Heavy hit: hp=%.0f state=%d destroyed_this_call=%d (expect DESTROYED=2, 1)\n",
         wall->hp, wall->state, destroyed);

  // scenario 2: mid-lane airship destroyed fires and seals the bridge
  ex_event_fire(bus, "airship_destroyed", /*lane=mid*/ 0, 0.0f);
  printf("bridge state after event: %d (expect SEALED=3)\n", bridge->state);

  ex_chunk_world_free(world);
  ex_event_bus_free(bus);
  return 0;
}
