#include "weapon.h"
#include <math.h>
#include <string.h>

ex_element_t ex_form_element(ex_watch_form_t form)
{
  switch (form) {
    case EX_FORM_WATER:       case EX_FORM_TSUNAMI:  return EX_ELEM_WATER;
    case EX_FORM_ICE:         case EX_FORM_BLIZZARD: return EX_ELEM_ICE;
    case EX_FORM_FIRE:        case EX_FORM_MAGMA:    return EX_ELEM_FIRE;
    case EX_FORM_METAL:       case EX_FORM_TITANIUM: return EX_ELEM_ROCK;
    case EX_FORM_ELECTRICITY:                        return EX_ELEM_ELECTRIC;
    case EX_FORM_WIND:        case EX_FORM_TORNADO:  return EX_ELEM_AIR;
    case EX_FORM_PHOTON:                             return EX_ELEM_NONE; // transcends
    default:                                         return EX_ELEM_NONE;
  }
}

ex_weapon_profile_t ex_weapon_profile(ex_watch_form_t form)
{
  ex_weapon_profile_t p;
  memset(&p, 0, sizeof(p));
  p.mode = EX_FIRE_BOLT; p.damage = 40; p.range = 200; p.cooldown = 0.22f;
  p.proj_speed = 70; p.spread_deg = 0; p.pellets = 1; p.chains = 0; p.slow = 0;
  p.element = ex_form_element(form);

  switch (form) {
    case EX_FORM_WATER: // flexible baseline
      p.damage=42; p.range=200; p.cooldown=0.16f; p.proj_speed=75;
      p.color[0]=90;p.color[1]=170;p.color[2]=255; break;
    case EX_FORM_TSUNAMI: // short knockback wave
      p.mode=EX_FIRE_WAVE; p.damage=55; p.range=16; p.cooldown=0.2f; p.proj_speed=40;
      p.color[0]=70;p.color[1]=150;p.color[2]=230; break;
    case EX_FORM_ICE: // suppression freeze-beam: heavy slow, long reach
      p.mode=EX_FIRE_SUPPRESS; p.damage=30; p.range=260; p.cooldown=0.26f; p.proj_speed=120; p.slow=0.8f;
      p.color[0]=150;p.color[1]=220;p.color[2]=255; break;
    case EX_FORM_FIRE: // airbursting fireball -- direct hit + splash
      p.mode=EX_FIRE_FLAK; p.damage=50; p.range=120; p.cooldown=0.24f; p.proj_speed=65;
      p.color[0]=255;p.color[1]=120;p.color[2]=40; break;
    case EX_FORM_MAGMA: // lobbed lingering heavy
      p.mode=EX_FIRE_LOB; p.damage=80; p.range=90; p.cooldown=0.3f; p.proj_speed=45;
      p.color[0]=230;p.color[1]=90;p.color[2]=30; break;
    case EX_FORM_BLIZZARD: // spread cone, slows
      p.mode=EX_FIRE_BURST; p.damage=16; p.range=70; p.cooldown=0.35f; p.proj_speed=70;
      p.spread_deg=12; p.pellets=6; p.slow=0.35f;
      p.color[0]=190;p.color[1]=230;p.color[2]=255; break;
    case EX_FORM_METAL: // ricocheting slugs -- bounce off walls into cover-huggers
      p.mode=EX_FIRE_RICOCHET; p.damage=26; p.range=180; p.cooldown=0.14f; p.proj_speed=95;
      p.color[0]=180;p.color[1]=185;p.color[2]=195; break;
    case EX_FORM_TITANIUM: // armour-piercing railgun -- one line, through everything
      p.mode=EX_FIRE_PIERCE; p.damage=90; p.range=180; p.cooldown=0.5f; p.proj_speed=150;
      p.color[0]=150;p.color[1]=160;p.color[2]=175; break;
    case EX_FORM_ELECTRICITY: // PLASMA CANNON -- a heavy electric ball, big blast (WFC riot cannon)
      p.mode=EX_FIRE_PLASMA; p.damage=60; p.range=220; p.cooldown=0.6f; p.proj_speed=90;
      p.chains=2; p.color[0]=140;p.color[1]=180;p.color[2]=255; break;
    case EX_FORM_WIND: // fast cheap, mobility
      p.damage=18; p.range=160; p.cooldown=0.22f; p.proj_speed=110;
      p.color[0]=210;p.color[1]=235;p.color[2]=215; break;
    case EX_FORM_TORNADO: // wide spread AoE
      p.mode=EX_FIRE_BURST; p.damage=20; p.range=90; p.cooldown=1.0f; p.proj_speed=80;
      p.spread_deg=22; p.pellets=8;
      p.color[0]=200;p.color[1]=220;p.color[2]=210; break;
    case EX_FORM_PHOTON: // apex: instant beam, massive
      p.mode=EX_FIRE_BEAM; p.damage=140; p.range=400; p.cooldown=0.9f; p.proj_speed=220;
      p.color[0]=255;p.color[1]=250;p.color[2]=230; break;
    default: p.color[0]=255;p.color[1]=220;p.color[2]=120; break;
  }
  return p;
}

const char* ex_form_name(ex_watch_form_t form)
{
  switch (form) {
    case EX_FORM_WATER:       return "water";
    case EX_FORM_TSUNAMI:     return "tsunami";
    case EX_FORM_ICE:         return "ice";
    case EX_FORM_FIRE:        return "fire";
    case EX_FORM_MAGMA:       return "magma";
    case EX_FORM_BLIZZARD:    return "blizzard";
    case EX_FORM_METAL:       return "metal";
    case EX_FORM_TITANIUM:    return "titanium";
    case EX_FORM_ELECTRICITY: return "electricity";
    case EX_FORM_WIND:        return "wind";
    case EX_FORM_TORNADO:     return "tornado";
    case EX_FORM_PHOTON:      return "PHOTON";
    default:                  return "?";
  }
}

// ---- star-fire ----
void ex_starfire_begin(ex_starfire_t *s) {
  if (s->cooldown > 0.0f) return;   // spent -- can't hold a new star yet
  s->charge = 0.0f; s->charging = 1;
}
void ex_starfire_update(ex_starfire_t *s, float dt, int held)
{
  if (s->cooldown > 0.0f) {          // cooling down: tick it, no charging
    s->cooldown -= dt; if (s->cooldown < 0.0f) s->cooldown = 0.0f;
    s->charging = 0; s->charge = 0.0f; return;
  }
  if (!held) { s->charging = 0; return; }
  s->charging = 1;
  s->charge += dt / EX_STAR_CHARGE_SEC;
  if (s->charge > 1.0f) s->charge = 1.0f;
}
float ex_starfire_release(ex_starfire_t *s, int *out_star)
{
  int star = (s->charge >= EX_STAR_FULL);
  float mul = 1.0f + s->charge * 5.0f; // 1x .. 6x at full charge
  if (star) s->cooldown = EX_STAR_COOLDOWN;   // a real star costs a lockout
  if (out_star) *out_star = star;
  s->charge = 0.0f; s->charging = 0;
  return mul;
}
int ex_starfire_ready(const ex_starfire_t *s) { return s->cooldown <= 0.0f; }
float ex_starfire_cooldown_frac(const ex_starfire_t *s)
{
  float f = s->cooldown / EX_STAR_COOLDOWN;
  return f < 0.0f ? 0.0f : (f > 1.0f ? 1.0f : f);
}

// ---- quantum gravity leap ----
void ex_gravleap_fire(ex_gravleap_t *g, const float from[3], const float dir[3],
                      float dist, float out_pos[3])
{
  // instant, silent insertion: move to arrival point immediately
  out_pos[0] = from[0] + dir[0] * dist;
  out_pos[1] = from[1] + dir[1] * dist;
  out_pos[2] = from[2] + dir[2] * dist;
  // schedule the delayed gravity wave at the arrival point
  g->pending = 1;
  g->timer = EX_LEAP_WAVE_DELAY;
  g->origin[0] = out_pos[0]; g->origin[1] = out_pos[1]; g->origin[2] = out_pos[2];
  g->distance = dist;
}
int ex_gravleap_update(ex_gravleap_t *g, float dt, float center[3], float *radius, float *damage)
{
  if (!g->pending) return 0;
  g->timer -= dt;
  if (g->timer > 0.0f) return 0;
  // the wave lands: indifferent AoE, intensity scales a little with leap distance
  g->pending = 0;
  center[0]=g->origin[0]; center[1]=g->origin[1]; center[2]=g->origin[2];
  float scale = 1.0f + (g->distance / 60.0f);
  *radius = EX_LEAP_WAVE_RADIUS * scale;
  *damage = EX_LEAP_WAVE_DAMAGE * scale;
  return 1;
}
