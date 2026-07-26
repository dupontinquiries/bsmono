#include "ability.h"
#include <string.h>

const char* ex_ability_name(ex_ability_id id){
  switch(id){
    case EX_ABIL_HOVER: return "hover"; case EX_ABIL_TURRET: return "turret";
    case EX_ABIL_SHIELD: return "shield"; case EX_ABIL_DASH: return "dash";
    case EX_ABIL_CLOAK: return "cloak"; case EX_ABIL_OVERCLOCK: return "overclock";
    case EX_ABIL_RESONANCE: return "resonance";
    case EX_ABIL_PULSE: return "pulse blast"; case EX_ABIL_BLINK: return "blink";
    default: return "none";
  }
}

void ex_ability_defaults(ex_ability_t *a, ex_ability_id id){
  a->id = id; a->cd_timer = 0.0f; a->active_timer = 0.0f;
  switch(id){
    case EX_ABIL_HOVER:     a->cooldown=8.0f;  a->duration=4.0f; break;
    case EX_ABIL_TURRET:    a->cooldown=14.0f; a->duration=8.0f; break;
    case EX_ABIL_SHIELD:    a->cooldown=10.0f; a->duration=5.0f; break;
    case EX_ABIL_DASH:      a->cooldown=3.0f;  a->duration=0.35f; break;
    case EX_ABIL_CLOAK:     a->cooldown=12.0f; a->duration=6.0f; break;
    case EX_ABIL_OVERCLOCK: a->cooldown=12.0f; a->duration=5.0f; break;
    case EX_ABIL_RESONANCE: a->cooldown=10.0f; a->duration=0.0f; break;   /* instant burst */
    case EX_ABIL_PULSE: a->cooldown=6.0f; a->duration=0.0f; break;
    case EX_ABIL_BLINK: a->cooldown=5.0f; a->duration=0.0f; break;
    default:                a->cooldown=1.0f;  a->duration=0.0f; break;
  }
}

void ex_loadout_init(ex_ability_loadout_t *l){ memset(l,0,sizeof(*l)); }

int ex_loadout_add(ex_ability_loadout_t *l, ex_ability_id id){
  if (l->count >= EX_ABILITY_SLOTS) return -1;
  int i = l->count++;
  ex_ability_defaults(&l->slots[i], id);
  return i;
}
void ex_loadout_set(ex_ability_loadout_t *l, int slot, ex_ability_id id){
  if (slot < 0 || slot >= EX_ABILITY_SLOTS) return;
  if (slot >= l->count) l->count = slot+1;
  ex_ability_defaults(&l->slots[slot], id);
}

int ex_ability_ready(const ex_ability_loadout_t *l, int slot){
  if (slot<0||slot>=l->count) return 0;
  const ex_ability_t *a=&l->slots[slot];
  return a->id!=EX_ABIL_NONE && a->cd_timer<=0.0f && a->active_timer<=0.0f;
}
int ex_ability_trigger(ex_ability_loadout_t *l, int slot){
  if (!ex_ability_ready(l,slot)) return 0;
  ex_ability_t *a=&l->slots[slot];
  a->active_timer = a->duration;
  a->cd_timer = a->cooldown + a->duration;
  return 1;
}
int ex_ability_active(const ex_ability_loadout_t *l, int slot){
  if (slot<0||slot>=l->count) return 0;
  return l->slots[slot].active_timer > 0.0f;
}
int ex_ability_active_any(const ex_ability_loadout_t *l, ex_ability_id id){
  for (int i=0;i<l->count;i++)
    if (l->slots[i].id==id && l->slots[i].active_timer>0.0f) return 1;
  return 0;
}
int ex_loadout_has(const ex_ability_loadout_t *l, ex_ability_id id){
  for (int i=0;i<l->count;i++) if (l->slots[i].id==id) return 1;
  return 0;
}
void ex_loadout_update(ex_ability_loadout_t *l, float dt){
  for (int i=0;i<l->count;i++){
    ex_ability_t *a=&l->slots[i];
    if (a->active_timer>0.0f) a->active_timer-=dt;
    if (a->cd_timer>0.0f) a->cd_timer-=dt;
  }
}
