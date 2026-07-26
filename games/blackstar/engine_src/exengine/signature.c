#include "signature.h"
#include <math.h>
#include <string.h>

const char *ex_signature_mesh(ex_signature_t s)
{
  switch (s) {
    case EX_SIG_WING_CANNON:  return "data/spine_wing_cannon.iqm";
    case EX_SIG_HEAVY_CANNON: return "data/spine_heavy_cannon.iqm";
    case EX_SIG_CREST:        return "data/spine_crest.iqm";
    case EX_SIG_CABLE:        return "data/spine_cable.iqm";
    case EX_SIG_HARPOON:      return "data/spine_harpoon.iqm";
    case EX_SIG_HALO:         return "data/spine_halo.iqm";
    case EX_SIG_SOLAR:        return "data/spine_solar.iqm";
    case EX_SIG_AEGIS:        return "data/spine_aegis.iqm";
    case EX_SIG_RECON:        return "data/spine_recon.iqm";
    case EX_SIG_MORTAR:       return "data/spine_mortar.iqm";
    case EX_SIG_RAZOR:        return "data/spine_razor.iqm";
    default:                  return "data/spine_wing_cannon.iqm";
  }
}

ex_signature_t ex_signature_for_class(int player_class)
{
  switch (player_class) {
    case 0: return EX_SIG_HEAVY_CANNON; // EX_CLASS_VANGUARD  -- siege
    case 1: return EX_SIG_WING_CANNON;  // EX_CLASS_INTERCEPTOR -- flight
    case 2: return EX_SIG_CREST;        // EX_CLASS_TITAN     -- beast crest
    case 3: return EX_SIG_CABLE;        // EX_CLASS_SPECIALIST -- utility tendril
    case 4: return EX_SIG_HARPOON;      // EX_CLASS_REAVER   -- outer-rim predator
    case 5: return EX_SIG_HALO;         // EX_CLASS_WARDEN   -- clone sentinel
    case 6: return EX_SIG_SOLAR;        // EX_CLASS_PYRE     -- fire clone
    case 7: return EX_SIG_AEGIS;        // EX_CLASS_PRIME    -- the steward
    case 8: return EX_SIG_RECON;        // EX_CLASS_SCOUT    -- bumblebee
    case 9: return EX_SIG_MORTAR;       // EX_CLASS_SIEGE    -- artillery bombard
    default:return EX_SIG_WING_CANNON;
  }
}

float ex_signature_fold_start(void) { return 0.45f; } // deploys in the back half

const char *ex_signature_name(ex_signature_t s)
{
  switch (s) {
    case EX_SIG_WING_CANNON:  return "wing-cannon";
    case EX_SIG_HEAVY_CANNON: return "heavy cannon";
    case EX_SIG_CREST:        return "crest";
    case EX_SIG_CABLE:        return "cable";
    case EX_SIG_HARPOON:      return "harpoon";
    case EX_SIG_HALO:         return "sentinel halo";
    case EX_SIG_SOLAR:        return "solar core";
    case EX_SIG_AEGIS:        return "aegis";
    case EX_SIG_RECON:        return "recon fins";
    case EX_SIG_MORTAR:       return "siege mortar";
    case EX_SIG_RAZOR:        return "razor fans";
    default:                  return "wing-cannon";
  }
}

ex_sig_stats_t ex_signature_stats(ex_signature_t s)
{
  // Each signature is a distinct playstyle with real tradeoffs (nothing strictly
  // dominant): every profile has at least one advantage and one drawback.
  switch (s) {
    // Interceptor: fast flanker -- quick refire + speed, but fragile.
    case EX_SIG_WING_CANNON:  return (ex_sig_stats_t){ 1.00f, 0.90f, 1.08f, 1.12f };
    // Vanguard: siege -- big hits, tanky, but slow to refire and to move.
    case EX_SIG_HEAVY_CANNON: return (ex_sig_stats_t){ 1.35f, 1.30f, 0.85f, 0.90f };
    // Titan: bruiser wall -- armoured + a damage edge, slightly slower.
    case EX_SIG_CREST:        return (ex_sig_stats_t){ 1.12f, 1.00f, 0.72f, 0.94f };
    // Specialist: rapid harasser -- fastest refire + mobile, low per-hit damage.
    case EX_SIG_CABLE:        return (ex_sig_stats_t){ 0.85f, 0.80f, 1.05f, 1.08f };
    // Reaver: outer-rim predator -- solid hits + mobility, a touch fragile.
    case EX_SIG_HARPOON:      return (ex_sig_stats_t){ 1.15f, 1.00f, 1.06f, 1.10f };
    // Warden: clone sentinel -- a wall. very tanky + steady, but slow.
    case EX_SIG_HALO:         return (ex_sig_stats_t){ 1.00f, 1.05f, 0.65f, 0.86f };
    // Pyre: fire clone -- glass cannon. devastating hits, very fragile, slow refire.
    case EX_SIG_SOLAR:        return (ex_sig_stats_t){ 1.55f, 1.35f, 1.30f, 0.92f };
    // Prime: the steward -- durable all-rounder; hits solid + very tanky, slower.
    case EX_SIG_AEGIS:        return (ex_sig_stats_t){ 1.10f, 1.00f, 0.70f, 0.90f };
    // Scout: recon harasser -- quickest refire + fastest, but fragile + low damage.
    case EX_SIG_RECON:        return (ex_sig_stats_t){ 0.85f, 0.78f, 1.12f, 1.18f };
    // Siege: hits hard but reloads slow; sturdy but ponderous.
    case EX_SIG_MORTAR:       return (ex_sig_stats_t){ 1.45f, 1.35f, 0.82f, 0.80f };
    case EX_SIG_RAZOR:        return (ex_sig_stats_t){ 1.15f, 0.90f, 1.18f, 1.12f };   // cutter: hits harder, moves quicker, thinner armour
    default:                  return (ex_sig_stats_t){ 1.00f, 1.00f, 1.00f, 1.00f };
  }
}

ex_sig_combat_t ex_signature_combat(ex_signature_t s)
{
  switch (s) {
    // Interceptor: paired wing bolts -- an extra spread shot, no crowd control.
    case EX_SIG_WING_CANNON:  return (ex_sig_combat_t){ 1, 0.0f,  0.0f, 0.0f };
    // Vanguard: siege -- heavy knockback + a small splash that staggers a cluster.
    case EX_SIG_HEAVY_CANNON: return (ex_sig_combat_t){ 0, 16.0f, 0.0f, 4.0f };
    // Titan: bruiser -- a solid shove on hit (no splash), reads as raw impact.
    case EX_SIG_CREST:        return (ex_sig_combat_t){ 0, 9.0f,  0.0f, 0.0f };
    // Specialist: utility -- tethers/roots the target for a beat (crowd control).
    case EX_SIG_CABLE:        return (ex_sig_combat_t){ 0, 0.0f,  1.6f, 0.0f };
    // Reaver: harpoon -- impaling knockback + a magnetic grip that slows.
    case EX_SIG_HARPOON:      return (ex_sig_combat_t){ 0, 12.0f, 0.8f, 0.0f };
    // Warden: sentinel -- a wide, gentle suppressive splash + light shove.
    case EX_SIG_HALO:         return (ex_sig_combat_t){ 0, 5.0f,  0.0f, 6.5f };
    // Pyre: solar -- a burning nova: knockback + a hot splash.
    case EX_SIG_SOLAR:        return (ex_sig_combat_t){ 0, 10.0f, 0.0f, 6.0f };
    // Prime: guardian -- a solid shove + a small rallying splash.
    case EX_SIG_AEGIS:        return (ex_sig_combat_t){ 0, 8.0f,  0.0f, 5.0f };
    // Scout: recon -- an extra bolt + a light sensor-tag slow.
    case EX_SIG_RECON:        return (ex_sig_combat_t){ 1, 0.0f,  0.6f, 0.0f };
    // Siege: artillery -- a hard shove and a wide airburst on every shell.
    case EX_SIG_MORTAR:       return (ex_sig_combat_t){ 0, 14.0f, 0.0f, 11.0f };
    case EX_SIG_RAZOR:        return (ex_sig_combat_t){ 0, 0.0f, 0.30f, 0.0f };   // melee bite: bonus close damage
    default:                  return (ex_sig_combat_t){ 0, 0.0f,  0.0f, 0.0f };
  }
}

int ex_signature_for_body(ex_body_kind kind)
{
  switch (kind) {
    case EX_BODY_GRUNT:     return EX_SIG_WING_CANNON;  // standard trooper
    case EX_BODY_CORRUPTED: return EX_SIG_CREST;        // twisted dorsal overgrowth
    case EX_BODY_HIJACK:    return EX_SIG_CABLE;        // thin swarm bug, whip antenna
    case EX_BODY_APEX:      return EX_SIG_HEAVY_CANNON; // boss with heavy armament
    case EX_BODY_PLAYER:    return EX_SIG_WING_CANNON;  // (player uses loadout instead)
    case EX_BODY_HUMAN:
    default:                return -1;                  // biological figure: no spine part
  }
}

/* vehicle silhouette category -- the signature adapts to the frame it rides. */
typedef enum { CAT_GROUND, CAT_FLYER, CAT_BEAST } sig_cat;
static sig_cat category_of(ex_vehicle_type v)
{
  if (ex_vehicle_flies(v)) return CAT_FLYER;
  switch (v) {
    case EX_VEH_INSECT: case EX_VEH_SPIDER: case EX_VEH_DRAGON:
    case EX_VEH_DINO:   case EX_VEH_RAPTOR: case EX_VEH_SCORPION:
    case EX_VEH_WOLF:   case EX_VEH_CRAB:
      return CAT_BEAST;
    default:
      return CAT_GROUND;
  }
}

static void SET(ex_part_pose_t *p, float x,float y,float z, float sx,float sy,float sz)
{
  p->offset[0]=x; p->offset[1]=y; p->offset[2]=z;
  p->sx=sx; p->sy=sy; p->sz=sz;
  p->scale=(sx+sy+sz)/3.0f;
  p->color[0]=p->color[1]=p->color[2]=200; // accent-ish default; game re-tints
}

void ex_signature_pose(ex_part_pose_t *robot_out, ex_part_pose_t *veh_out,
                       ex_signature_t s, ex_vehicle_type veh,
                       float size_scale, float phase, float speed)
{
  float amp  = fminf(speed/6.0f, 1.0f);
  float beat = sinf(phase*6.0f) * 0.06f * (0.3f + amp); // wing/tendril life
  ex_part_pose_t r, v;
  memset(&r, 0, sizeof r); memset(&v, 0, sizeof v);

  /* ---- ROBOT MODE: mounted high on the back, sized to the real mesh (not the
     tiny stock box). Each signature reads differently at rest. ---- */
  switch (s) {
    case EX_SIG_WING_CANNON:  SET(&r, 0, 1.50f, -0.40f, 0.62f, 0.50f, 0.34f); break;
    case EX_SIG_HEAVY_CANNON: SET(&r, 0, 1.52f, -0.46f, 0.46f, 0.46f, 0.62f); break;
    case EX_SIG_CREST:        SET(&r, 0, 1.72f, -0.20f, 0.52f, 0.60f, 0.40f); break;
    case EX_SIG_CABLE:        SET(&r, 0, 1.42f, -0.46f, 0.40f, 0.60f, 0.40f); break;
    case EX_SIG_HARPOON:      SET(&r, 0, 1.55f, -0.30f, 0.28f, 0.28f, 0.75f); break; // forward spear
    case EX_SIG_HALO:         SET(&r, 0, 1.85f, -0.25f, 0.60f, 0.55f, 0.30f); break; // high arc
    case EX_SIG_SOLAR:        SET(&r, 0, 1.70f, -0.30f, 0.42f, 0.42f, 0.42f); break; // held core
    case EX_SIG_AEGIS:        SET(&r, 0, 1.80f, -0.20f, 0.66f, 0.50f, 0.34f); break; // broad guardian arch
    case EX_SIG_RECON:        SET(&r, 0, 1.58f, -0.35f, 0.55f, 0.28f, 0.40f); break; // swept fins
    case EX_SIG_MORTAR:       SET(&r, 0, 1.60f, -0.42f, 0.54f, 0.58f, 0.54f); break; // heavy dorsal mortar
    case EX_SIG_RAZOR:        SET(&r, 0, 1.30f, -0.50f, 0.62f, 0.50f, 0.46f); break; // swept blade fans
    default:                  SET(&r, 0, 1.50f, -0.40f, 0.55f, 0.50f, 0.35f); break;
  }

  /* ---- VEHICLE MODE: folds onto the frame it rides. Category sets the base
     placement; the signature tunes the shape. This is the second bound. ---- */
  sig_cat cat = category_of(veh);
  if (cat == CAT_FLYER) {
    // rides the dorsal spine -- lifted + swept back, kept low+narrow so it
    // conforms to the airframe instead of blanketing it.
    switch (s) {
      case EX_SIG_WING_CANNON:  SET(&v, 0, 0.60f+beat, -0.15f, 0.62f, 0.10f, 0.42f); break;
      case EX_SIG_HEAVY_CANNON: SET(&v, 0, 0.66f,      -0.20f, 0.22f, 0.20f, 0.55f); break;
      case EX_SIG_CREST:        SET(&v, 0, 0.70f,      -0.35f, 0.16f, 0.30f, 0.48f); break;
      case EX_SIG_CABLE:        SET(&v, 0, 0.52f,      -0.70f, 0.16f, 0.16f, 0.70f+beat); break;
      case EX_SIG_HARPOON:      SET(&v, 0, 0.62f,      -0.25f, 0.20f, 0.16f, 0.72f); break; // lance along fuselage
      case EX_SIG_HALO:         SET(&v, 0, 0.70f,      -0.20f, 0.50f, 0.10f, 0.30f); break; // arc fin
      case EX_SIG_SOLAR:        SET(&v, 0, 0.66f,      -0.20f, 0.34f, 0.30f, 0.34f); break; // core
      case EX_SIG_AEGIS:        SET(&v, 0, 0.70f,      -0.25f, 0.50f, 0.12f, 0.30f); break; // flattened arch fin
      case EX_SIG_RECON:        SET(&v, 0, 0.62f,      -0.18f, 0.55f, 0.10f, 0.34f); break; // swept fin
      default:                  SET(&v, 0, 0.62f,      -0.20f, 0.4f,  0.14f, 0.5f);  break;
    }
  } else if (cat == CAT_BEAST) {
    // runs as a ridge along the creature's back -- slim, follows the spine.
    switch (s) {
      case EX_SIG_WING_CANNON:  SET(&v, 0, 0.95f, -0.30f, 0.36f, 0.22f, 0.55f); break;
      case EX_SIG_HEAVY_CANNON: SET(&v, 0, 0.98f, -0.25f, 0.26f, 0.26f, 0.55f); break;
      case EX_SIG_CREST:        SET(&v, 0, 1.02f, -0.20f, 0.30f, 0.40f, 0.52f); break; // natural fit
      case EX_SIG_CABLE:        SET(&v, 0, 0.90f, -0.55f, 0.16f, 0.18f, 0.62f+beat); break;
      case EX_SIG_HARPOON:      SET(&v, 0, 0.98f, -0.25f, 0.20f, 0.20f, 0.66f); break; // spine lance
      case EX_SIG_HALO:         SET(&v, 0, 1.05f, -0.22f, 0.44f, 0.16f, 0.30f); break; // dorsal arc
      case EX_SIG_SOLAR:        SET(&v, 0, 1.00f, -0.25f, 0.34f, 0.32f, 0.34f); break; // core on back
      case EX_SIG_AEGIS:        SET(&v, 0, 1.02f, -0.22f, 0.50f, 0.20f, 0.32f); break; // dorsal arch
      case EX_SIG_RECON:        SET(&v, 0, 0.95f, -0.30f, 0.40f, 0.16f, 0.40f); break; // dorsal fins
      default:                  SET(&v, 0, 0.95f, -0.30f, 0.32f, 0.26f, 0.5f);  break;
    }
  } else { // CAT_GROUND
    // clamps to the deck as a low spoiler / compact turret / dorsal ridge.
    switch (s) {
      case EX_SIG_WING_CANNON:  SET(&v, 0, 0.62f, -0.42f, 0.50f, 0.10f, 0.28f); break; // rear spoiler
      case EX_SIG_HEAVY_CANNON: SET(&v, 0, 0.70f,  0.00f, 0.26f, 0.26f, 0.55f); break; // turret
      case EX_SIG_CREST:        SET(&v, 0, 0.58f, -0.30f, 0.30f, 0.22f, 0.48f); break; // dorsal ridge
      case EX_SIG_CABLE:        SET(&v, 0, 0.50f, -0.48f, 0.30f, 0.14f, 0.40f); break; // coiled on deck
      case EX_SIG_HARPOON:      SET(&v, 0, 0.60f, -0.30f, 0.20f, 0.16f, 0.60f); break; // mounted lance
      case EX_SIG_HALO:         SET(&v, 0, 0.58f, -0.28f, 0.46f, 0.12f, 0.30f); break; // dorsal arc
      case EX_SIG_SOLAR:        SET(&v, 0, 0.60f, -0.20f, 0.32f, 0.30f, 0.32f); break; // core turret
      case EX_SIG_AEGIS:        SET(&v, 0, 0.62f, -0.30f, 0.55f, 0.14f, 0.30f); break; // broad guard spoiler
      case EX_SIG_RECON:        SET(&v, 0, 0.58f, -0.40f, 0.48f, 0.10f, 0.28f); break; // low fins
      default:                  SET(&v, 0, 0.62f, -0.40f, 0.4f,  0.14f, 0.35f); break;
    }
  }

  for (int k=0;k<3;k++){ r.offset[k]*=size_scale; v.offset[k]*=size_scale; }
  r.sx*=size_scale; r.sy*=size_scale; r.sz*=size_scale; r.scale*=size_scale;
  v.sx*=size_scale; v.sy*=size_scale; v.sz*=size_scale; v.scale*=size_scale;

  if (robot_out) *robot_out = r;
  if (veh_out)   *veh_out   = v;
}
