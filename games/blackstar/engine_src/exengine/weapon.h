/* weapon
  The Watch weapon roster. The Morpher's Watch can embody eleven reliable
  elemental forms (Water, Tsunami, Ice, Fire, Magma, Blizzard, Metal,
  Titanium, Electricity, Wind, Tornado) plus Photon -- the apex form nobody
  has fully mapped. Per the design brief, "each form has a distinct combat
  application so they never feel redundant": so each form here maps to a
  weapon profile with its own fire mode, damage, range, spread, projectile
  speed and colour, and to a base element for the rock-paper-scissors
  advantage cycle in element.h.

  Also here: the star-fire charged shot (a Billy-inspired heavy burst) and
  the quantum gravity leap (a silent forward insertion that schedules a
  delayed ground wave -- no flash, no sound, the wave arrives seconds later
  and hits everything indifferently).

  Pure data + math (floats), fully testable headless.
*/

#ifndef EX_WEAPON_H
#define EX_WEAPON_H

#include "element.h"
#include <stdint.h>

typedef enum {
  EX_FORM_WATER = 0,
  EX_FORM_TSUNAMI,
  EX_FORM_ICE,
  EX_FORM_FIRE,
  EX_FORM_MAGMA,
  EX_FORM_BLIZZARD,
  EX_FORM_METAL,
  EX_FORM_TITANIUM,
  EX_FORM_ELECTRICITY,
  EX_FORM_WIND,
  EX_FORM_TORNADO,
  EX_FORM_PHOTON,     // apex form -- the unreached ceiling
  EX_FORM_COUNT
} ex_watch_form_t;

typedef enum {
  EX_FIRE_BOLT,   // single projectile
  EX_FIRE_BURST,  // shotgun spread of pellets
  EX_FIRE_BEAM,   // fast, long, near-instant
  EX_FIRE_WAVE,   // short-range AoE pulse
  EX_FIRE_LOB,    // arcing lobbed shot with lingering effect
  EX_FIRE_PIERCE, // railshot: passes through every enemy in a line
  EX_FIRE_FLAK,   // airburst: detonates at impact for splash
  EX_FIRE_PLASMA, // heavy electric ball -- a big AoE blast on impact
  EX_FIRE_RICOCHET,// round that bounces off a wall to a nearby enemy
  EX_FIRE_SUPPRESS // beam that heavily slows what it hits
} ex_fire_mode_t;

typedef struct {
  ex_fire_mode_t mode;
  float damage;        // per pellet/hit
  float range;
  float cooldown;      // seconds between shots
  float proj_speed;    // bolt travel speed (cosmetic tracer)
  float spread_deg;    // cone half-angle for BURST
  int   pellets;       // BURST pellet count (1 for others)
  int   chains;        // ELECTRICITY: extra targets a hit arcs to
  float slow;          // 0..1 movement slow applied to target (ICE/BLIZZARD)
  uint8_t color[3];    // tracer/impact colour
  ex_element_t element;// for elemental advantage
} ex_weapon_profile_t;

/**
 * [ex_weapon_profile the weapon profile for a Watch form]
 */
ex_weapon_profile_t ex_weapon_profile(ex_watch_form_t form);

/**
 * [ex_form_element base element of a form (for advantage math)]
 */
ex_element_t ex_form_element(ex_watch_form_t form);

const char* ex_form_name(ex_watch_form_t form);

// ---- star-fire: Billy's held star -- charge it, then release. A full-charge
// release is a true "star" (a radiant nova) that then locks out for a cost. ----
#define EX_STAR_CHARGE_SEC 1.4f
#define EX_STAR_FULL       0.85f   // charge at/above this releases a real star
#define EX_STAR_COOLDOWN   6.0f    // lockout after spending a full star (the cost)
typedef struct {
  float charge;    // 0..1, builds while held
  int   charging;
  float cooldown;  // >0 = spent, cannot charge (recently released a star)
} ex_starfire_t;
void  ex_starfire_begin(ex_starfire_t *s);
void  ex_starfire_update(ex_starfire_t *s, float dt, int held);
/* [ex_starfire_release returns damage multiplier (1..6) and resets. If the
    charge was a full star, sets *out_star=1 and starts the cooldown lockout.] */
float ex_starfire_release(ex_starfire_t *s, int *out_star);
/* [ex_starfire_ready 1 if it can begin charging (not on cooldown)] */
int   ex_starfire_ready(const ex_starfire_t *s);
/* [ex_starfire_cooldown_frac 0..1 remaining lockout, for HUD] */
float ex_starfire_cooldown_frac(const ex_starfire_t *s);

// ---- quantum gravity leap: silent insertion + delayed wave ----
#define EX_LEAP_WAVE_DELAY 2.2f   // seconds until the gravity wave lands
#define EX_LEAP_WAVE_RADIUS 14.0f
#define EX_LEAP_WAVE_DAMAGE 60.0f
typedef struct {
  int   pending;     // a wave is scheduled
  float timer;       // counts down to the wave
  float origin[3];   // where the wave will land (the arrival point)
  float distance;    // how far the leap was (scales wave)
} ex_gravleap_t;
/**
 * [ex_gravleap_fire teleport `dist` along `dir` from `from`; schedule the wave.
 *  Writes the new position into out_pos. Silent + instant (no visual at origin).]
 */
void ex_gravleap_fire(ex_gravleap_t *g, const float from[3], const float dir[3],
                      float dist, float out_pos[3]);
/**
 * [ex_gravleap_update ticks the timer; when it lands returns 1 and fills
 *  center/radius/damage for the caller to apply an AoE. Otherwise 0.]
 */
int ex_gravleap_update(ex_gravleap_t *g, float dt, float center[3], float *radius, float *damage);

#endif // EX_WEAPON_H
