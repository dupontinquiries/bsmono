/* signature.h -- class "signature components": the spine-grown parts
   (tools/gen_spine.py) treated as first-class transformer components that are
   bounded on BOTH silhouettes. Each player class carries a distinct signature
   part on the BACK slot, and this module defines, for every (signature, vehicle
   archetype) pair, WHERE that part sits in robot mode and in vehicle mode -- so
   the transform morphs it between two real placements instead of letting it fade
   to nothing (which is what happens to any robot part the vehicle layout doesn't
   claim).

   Pure math (offsets + non-uniform scale), no GL -- testable headless. */
#ifndef EX_SIGNATURE_H
#define EX_SIGNATURE_H

#include "robot_parts.h"
#include "vehicle.h"

typedef enum {
  EX_SIG_WING_CANNON = 0,  // Interceptor: paired wing-cannon
  EX_SIG_HEAVY_CANNON,     // Vanguard: stepped dorsal siege cannon
  EX_SIG_CREST,            // Titan: horned dorsal crest / spine ridge
  EX_SIG_CABLE,            // Specialist: coiled utility tendril
  EX_SIG_HARPOON,          // Reaver: barbed plasma harpoon / boarding spear
  EX_SIG_HALO,             // Warden: sentinel array -- an arced dorsal ring
  EX_SIG_SOLAR,            // Pyre: a held star -- radiant core with corona spikes
  EX_SIG_AEGIS,            // Prime: a broad guardian arch over the shoulders
  EX_SIG_RECON,            // Scout: light swept sensor fins
  EX_SIG_MORTAR,           // Siege: heavy shoulder mortar (gear breech + flared mouth)
  EX_SIG_RAZOR,            // Executioner: twin swept dorsal blade fans
  EX_SIG_COUNT
} ex_signature_t;

/* data/ .iqm path for a signature's mesh (matches tools/gen_spine.py output). */
const char *ex_signature_mesh(ex_signature_t s);

/* which signature a player class (ex_class_t 0..3) carries. */
ex_signature_t ex_signature_for_class(int player_class);

/* default signature for a non-player body kind, so enemies read distinctly.
   Returns -1 for bodies that should carry NO signature (e.g. human). */
int ex_signature_for_body(ex_body_kind kind);

/* Fill the robot-mode and vehicle-mode pose for a signature on a given vehicle
   archetype. Both are pre-multiplied by size_scale. `phase`/`speed` add a little
   life (wing bob / tendril sway). Either out pointer may be NULL. */
void ex_signature_pose(ex_part_pose_t *robot_out, ex_part_pose_t *veh_out,
                       ex_signature_t s, ex_vehicle_type veh,
                       float size_scale, float phase, float speed);

/* Staging: the signature deploys LATE in the fold (wings snap open last), so it
   reads as the finishing flourish. Returns the [0..1] start point of its blend
   window within the overall transform. */
float ex_signature_fold_start(void);

/* short display name for the HUD (e.g. "wing-cannon"). */
const char *ex_signature_name(ex_signature_t s);

/* Gameplay profile: each signature trades offense / refire / toughness /
   mobility so the choice changes how a bot plays, not just how it looks.
   Multipliers around 1.0; <1 is "less", >1 is "more" of that axis. For
   taken_mul, <1 is TANKIER (takes less). */
typedef struct {
  float dmg_mul;    // primary weapon damage
  float fire_mul;   // refire interval (<1 = fires faster)
  float taken_mul;  // incoming damage (<1 = tankier)
  float speed_mul;  // move speed
} ex_sig_stats_t;

ex_sig_stats_t ex_signature_stats(ex_signature_t s);

/* Combat identity: a distinct on-hit effect per signature, layered on top of the
 * stat profile, using the engine's existing hit primitives (extra pellets, an
 * enemy knockback impulse, a slow/root duration, a splash radius). */
typedef struct {
  int   extra_pellets;  // additional spread bolts per shot (wing-cannon dual fire)
  float knockback;      // impulse shoved onto a struck enemy (siege)
  float slow;           // seconds of root/slow applied to the target (tether)
  float splash;         // AoE radius: nearby enemies take partial damage
} ex_sig_combat_t;

ex_sig_combat_t ex_signature_combat(ex_signature_t s);

#endif // EX_SIGNATURE_H
