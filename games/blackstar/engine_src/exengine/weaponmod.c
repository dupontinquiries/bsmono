#include "weaponmod.h"
#include <string.h>

const char* ex_weapon_mod_name(ex_weapon_mod_id id){
  switch(id){
    case EX_MOD_LONG_BARREL: return "long barrel"; case EX_MOD_SCOPE: return "scope";
    case EX_MOD_EXT_MAG: return "ext mag"; case EX_MOD_UNDERBARREL: return "underbarrel";
    case EX_MOD_STABILIZER: return "stabilizer"; case EX_MOD_AMPLIFIER: return "amplifier";
    case EX_MOD_CHAIN_COIL: return "chain coil";
    case EX_MOD_ENERGON_CELL: return "energon cell"; case EX_MOD_RAPID_COIL: return "rapid coil";
    case EX_MOD_PIERCE_CORE: return "pierce core"; case EX_MOD_SCATTER_BARREL: return "scatter barrel";
    case EX_MOD_RIPPLE_COIL: return "ripple coil"; case EX_MOD_CRYO_ROUNDS: return "cryo rounds";
    default: return "none";
  }
}
void ex_weapon_mods_init(ex_weapon_mods_t *m){ memset(m,0,sizeof(*m)); }
int ex_weapon_mods_add(ex_weapon_mods_t *m, ex_weapon_mod_id id){
  if (m->count>=EX_WEAPON_MOD_SLOTS) return -1;
  m->mods[m->count++]=id; return m->count-1;
}

static float clampf(float v,float lo,float hi){ return v<lo?lo:(v>hi?hi:v); }

ex_weapon_profile_t ex_weapon_apply_mods(ex_watch_form_t form, const ex_weapon_mods_t *m){
  ex_weapon_profile_t p = ex_weapon_profile(form);
  if (!m) return p;
  for (int i=0;i<m->count;i++){
    switch(m->mods[i]){
      case EX_MOD_LONG_BARREL: p.range*=1.35f; p.damage*=1.12f; p.cooldown*=1.10f; break;
      case EX_MOD_SCOPE:       p.spread_deg*=0.6f; p.range*=1.20f; break;
      case EX_MOD_EXT_MAG:     p.cooldown*=0.78f; break;
      case EX_MOD_UNDERBARREL: p.pellets+=1; p.spread_deg+=4.0f; if(p.mode==EX_FIRE_BOLT)p.mode=EX_FIRE_BURST; break;
      case EX_MOD_STABILIZER:  p.spread_deg*=0.5f; break;
      case EX_MOD_AMPLIFIER:   p.damage*=1.30f; p.cooldown*=1.15f; break;
      case EX_MOD_CHAIN_COIL:  p.chains+=1; break;
      case EX_MOD_ENERGON_CELL:p.damage*=1.30f; p.cooldown*=1.12f; break;                     // energon overcharge
      case EX_MOD_RAPID_COIL:  p.cooldown*=0.68f; p.damage*=0.88f; break;                     // rapid-fire spray
      case EX_MOD_PIERCE_CORE:  p.mode=EX_FIRE_PIERCE; p.cooldown*=1.18f; p.range*=1.15f; break;// rail conversion
      case EX_MOD_SCATTER_BARREL:p.pellets+=2; p.spread_deg+=8.0f; if(p.mode==EX_FIRE_BOLT)p.mode=EX_FIRE_BURST; break; // scatter
      case EX_MOD_RIPPLE_COIL:  p.mode=EX_FIRE_RICOCHET; p.cooldown*=1.05f; break;                    // ricochet conversion
      case EX_MOD_CRYO_ROUNDS:  p.slow = (p.slow<0.5f?0.5f:p.slow)+0.2f; break;                       // chilling slow
      default: break;
    }
  }
  p.cooldown = clampf(p.cooldown, 0.08f, 3.0f);
  p.spread_deg = clampf(p.spread_deg, 0.0f, 40.0f);
  if (p.pellets < 1) p.pellets = 1;
  p.slow = clampf(p.slow, 0.0f, 0.95f);
  return p;
}
