/* weaponmod
  Weapon addon system. A weapon profile (from weapon.h) can be modified by up
  to a few addons that adjust its stats and add visual parts: barrels change
  range/damage, scopes tighten spread, extended mags cut cooldown, underbarrel
  launchers add pellets, stabilizers reduce spread, amplifiers raise damage.
  Pure stat math over ex_weapon_profile_t; testable.
*/
#ifndef EX_WEAPONMOD_H
#define EX_WEAPONMOD_H

#include "weapon.h"

typedef enum {
  EX_MOD_NONE = 0,
  EX_MOD_LONG_BARREL,   // +range, +damage, +cooldown
  EX_MOD_SCOPE,         // -spread, +range
  EX_MOD_EXT_MAG,       // -cooldown
  EX_MOD_UNDERBARREL,   // +1 pellet, +spread
  EX_MOD_STABILIZER,    // -spread
  EX_MOD_AMPLIFIER,     // +damage, +cooldown
  EX_MOD_CHAIN_COIL,    // +1 chain
  EX_MOD_ENERGON_CELL,  // energon overcharge: +damage, +cooldown
  EX_MOD_RAPID_COIL,    // rapid-fire spray: -cooldown, -damage
  EX_MOD_PIERCE_CORE,   // rail conversion: shots pierce a line, +cooldown, +range
  EX_MOD_SCATTER_BARREL,// scatter conversion: +pellets, +spread (a spread shot)
  EX_MOD_RIPPLE_COIL,   // ricochet conversion: rounds bounce off walls
  EX_MOD_CRYO_ROUNDS,   // adds a chilling slow to whatever it hits
  EX_MOD_COUNT
} ex_weapon_mod_id;

#define EX_WEAPON_MOD_SLOTS 3
typedef struct { ex_weapon_mod_id mods[EX_WEAPON_MOD_SLOTS]; int count; } ex_weapon_mods_t;

const char* ex_weapon_mod_name(ex_weapon_mod_id id);
void  ex_weapon_mods_init(ex_weapon_mods_t *m);
int   ex_weapon_mods_add(ex_weapon_mods_t *m, ex_weapon_mod_id id);

/* [ex_weapon_apply_mods return the base profile modified by the mod set] */
ex_weapon_profile_t ex_weapon_apply_mods(ex_watch_form_t form, const ex_weapon_mods_t *m);

#endif // EX_WEAPONMOD_H
