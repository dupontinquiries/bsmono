/* vehicle
  Vehicle-mode archetypes for the transform. The robot form is one silhouette;
  when a bot transforms it can take one of many vehicle configurations, each a
  distinct arrangement of the same body parts plus archetype-specific extras
  (wings, legs, mandibles, tail, treads). Beyond ground vehicles this includes
  creature-inspired builds: insect, spider, dragon, dino, raptor.

  Produces per-part poses (offset + non-uniform scale) for a given archetype,
  animated by a phase (wheels roll, wings beat, legs skitter, tail sways).
  Pure math, testable; the game applies the poses to models + a detail pass.
*/

#ifndef EX_VEHICLE_H
#define EX_VEHICLE_H

#include "robot_parts.h"

typedef enum {
  EX_VEH_CAR = 0,    // sleek ground racer
  EX_VEH_TANK,       // heavy treaded
  EX_VEH_JET,        // winged flyer
  EX_VEH_HOVER,      // hovering skiff
  EX_VEH_INSECT,     // six-legged skitterer
  EX_VEH_SPIDER,     // eight-legged crawler
  EX_VEH_DRAGON,     // winged serpent
  EX_VEH_DINO,       // heavy quadruped
  EX_VEH_RAPTOR,     // fast biped runner
  EX_VEH_MECH,       // heavy bipedal walker
  EX_VEH_CYCLE,      // agile hover-cycle
  EX_VEH_GUNSHIP,    // heavy armed flyer
  EX_VEH_SCORPION,   // armored crawler with a stinger tail
  EX_VEH_WOLF,       // fast quadruped beast
  EX_VEH_HAWK,       // swift winged raptor-flyer
  EX_VEH_CRAB,       // low, wide, clawed skitterer
  EX_VEH_SENTRY,     // stationary siege gun -- rooted, heavy, huge firepower
  EX_VEH_WHEEL,      // giant armoured rolling wheel -- momentum juggernaut
  EX_VEH_WRAITH,     // stealth aircraft -- fast, quiet, hard to track
  EX_VEH_MANTA,      // biomimetic ray-flyer -- gliding, ultra-agile
  EX_VEH_BULWARK,    // armoured gunship -- slow, heavily plated flyer
  EX_VEH_COUNT
} ex_vehicle_type;

const char* ex_vehicle_name(ex_vehicle_type t);

/* does this archetype fly? (affects camera/motion) */
int ex_vehicle_flies(ex_vehicle_type t);

/* Per-archetype handling feel. speed_mul scales top speed; accel is how fast
 * velocity reaches the target (responsiveness); grip is lateral hold (1=locked
 * to steering, <1 slides/drifts through turns). */
typedef struct { float speed_mul, accel, grip; } ex_vehicle_handling_t;
ex_vehicle_handling_t ex_vehicle_handling(ex_vehicle_type t);

/* how many extra limb parts (legs/wings) this archetype adds beyond the core */
int ex_vehicle_extra_parts(ex_vehicle_type t);

/**
 * [ex_vehicle_layout fill poses for a vehicle archetype at gait `phase`]
 * Writes into poses[EX_ROBOT_PARTS + extras]; caller must size accordingly.
 * @param n_out receives the total number of parts written.
 */
void ex_vehicle_layout(ex_part_pose_t *poses, int cap, int *n_out,
                       ex_vehicle_type type, const ex_build_params_t *bp,
                       float size_scale, float phase, float speed,
                       int element, ex_body_kind kind);

#define EX_VEHICLE_MAX_PARTS (EX_ROBOT_PARTS + 12)

#endif // EX_VEHICLE_H
