#include <string.h>
#include <physfs.h>
#include "engine.h"
#include "inc/game.h"

#include "exengine/transform.h"
#include "exengine/coll_chunk.h"
#include "exengine/terrain_step.h"
#include "exengine/pclass.h"
#include "exengine/chassis.h"
#include "exengine/signature.h"
#include "exengine/voice.h"
#include "exengine/feel.h"
#include "exengine/material.h"
#include "exengine/move.h"
#include "exengine/genome.h"
#include "exengine/events.h"
#include "exengine/netcode.h"
#include "exengine/keybind.h"
#include "exengine/mapfsm.h"
#include "exengine/arena.h"
#include "exengine/combat.h"
#include "exengine/enemy.h"
#include "exengine/element.h"
#include "exengine/menu.h"
#include "exengine/robot_parts.h"
#include "exengine/campaign.h"
#include "exengine/vfx.h"
#include "exengine/weapon.h"
#include "exengine/encounter.h"
#include "exengine/pickup.h"
#include "exengine/detail.h"
#include "exengine/vehicle.h"
#include "exengine/ability.h"
#include "exengine/weaponmod.h"
#include "exengine/turret.h"
#include "exengine/mapgen.h"
#include "exengine/lore.h"
#include "exengine/warp_wave.h"
#include "exengine/tsunami.h"
#include "exengine/arc.h"
#include "exengine/cyclone.h"
#include "exengine/thorns.h"
#include "exengine/photon.h"
#include "exengine/matrix.h"
#include "exengine/spearcon.h"
#include "exengine/resonance.h"
#include "exengine/corrupt.h"
#include "exengine/graviton.h"
#include "exengine/siphon.h"
#include "exengine/truce.h"
#include "exengine/containment.h"
#include "exengine/absorb.h"
#include "exengine/ally.h"
#include "exengine/waves.h"

ex_fps_camera_t *camera;
ex_scene_t *scene;
// per-mode camera state (see use_camera below): each view mode keeps its own
// camera so switching modes never inherits the previous screen's view.
typedef enum { CAM_GAME=0, CAM_MENU, CAM_BUILDER, CAM_COUNT } cam_mode_t;
typedef struct { vec3 position, front, up; float yaw, pitch, fov; int inited; } cam_state_t;
static cam_state_t g_cam_state[CAM_COUNT];
static int g_cam_active = -1;
static void use_camera(cam_mode_t m);
ex_model_t *level_model, *box;
ex_entity_t *e;
ex_point_light_t *pl;
ex_source_t *sound;
static ex_source_t *g_sfx_fire=NULL,*g_sfx_turret=NULL,*g_sfx_hit=NULL,*g_sfx_expl=NULL,
                   *g_sfx_xform=NULL,*g_sfx_launch=NULL,*g_sfx_pickup=NULL,*g_sfx_jump=NULL;
static void sfx(ex_source_t *s){ if(s){ ex_sound_restart(s); ex_sound_play(s); } }
ex_font_t *font;
float move_speed = 30.0f;

// ---------------------------------------------------------------------------
//  game flow + loadout
// ---------------------------------------------------------------------------
static ex_menu_t     g_menu;
static ex_campaign_t g_campaign;
static ex_model_t   *g_boxtpl;
static ex_model_t   *g_tpl_sphere=NULL, *g_tpl_bevel=NULL, *g_tpl_limb=NULL, *g_tpl_head=NULL, *g_tpl_hand=NULL, *g_tpl_chest=NULL;
static ex_model_t   *g_tpl_back=NULL, *g_tpl_muscle=NULL;   // spine-grown parts (gen_spine.py)
static ex_model_t   *g_tpl_cannon=NULL, *g_tpl_crest=NULL, *g_tpl_cable=NULL; // per-class signatures
static ex_model_t   *g_tpl_harpoon=NULL, *g_tpl_halo=NULL, *g_tpl_solar=NULL; // Reaver / Warden / Pyre signatures
static ex_model_t   *g_tpl_aegis=NULL, *g_tpl_recon=NULL; // Prime / Scout signatures
static ex_model_t   *g_star_orb=NULL;   // Billy's held star -- grows/brightens while charging
static ex_signature_t g_player_sig = EX_SIG_WING_CANNON;    // player's class signature component
static ex_sig_stats_t g_sig_stats = {1.0f,1.0f,1.0f,1.0f};  // signature gameplay profile (set at deploy)
static ex_sig_combat_t g_sig_combat = {0,0.0f,0.0f,0.0f};   // signature on-hit combat identity
static int          g_enemy_sig[EX_MAX_ENEMIES];           // current BACK signature per enemy slot (mesh sync)
static ex_model_t   *g_base_hull;              // collision hull (floor+walls), invisible
#define NUM_THEMES 10
static ex_model_t   *g_theme_models[NUM_THEMES];
static const char   *g_theme_ids[NUM_THEMES] = { "wastes", "frostline", "emberpit", "vault", "foundry", "shield_edge", "mining", "outer_rim", "cybertron", "reclaim" };
static ex_keybind_t  g_keys;
static int           g_keys_layout = -1;   // last applied layout preset
static int           g_keys_loaded_from_config = 0;
static ex_event_bus_t   *g_bus;
static ex_chunk_world_t *g_chunks;
static ex_arena_t        g_arena;
static ex_model_t       *g_feature_models[EX_ARENA_MAX_FEATURES];

// player transformer
static ex_xform_t *g_avatar_xform;
static ex_watch_t  g_watch;
static ex_watch_form_t g_form = EX_FORM_ICE;   // current Watch form (11 + photon)
static int          g_form_unlocked_photon = 0; // photon apex gate
static float        g_form_cd = 0.0f;           // fire cooldown timer
static ex_starfire_t g_starfire;
static ex_gravleap_t g_gravleap;
static ex_model_t *g_player_parts[EX_ROBOT_PARTS];
static float       g_anim_t = 0.0f;
static float       g_phase_player=0, g_phase_grunt=0, g_phase_corr=0;
#define VFX_MODELS 96
static ex_vfx_pool_t g_vfx;
static ex_model_t   *g_vfx_models[VFX_MODELS];
static float       g_player_hp = 100.0f;
static ex_photon_t g_photon = {0,0,0};     // Photon Man: the Watch's auto-triggered last stand
static float       g_photon_pulse_cd = 0.0f; // cadence of light-form radiant pulses
static ex_matrix_t g_matrix = {0};           // the Matrix of Leadership -> Rodimus
static float       g_matrix_pulse_cd = 0.0f; // cadence of the Matrix-light pulses
static ex_spearcon_pool_t g_spearcons;               // Cryotek's sticky countdown charges
static ex_model_t *g_spearcon_models[EX_SPEARCON_MAX] = {0};  // shrinking-fuse telegraph discs
static float       g_spearcon_cd = 0.0f;             // throttle so charges don't spam
static float       g_player_max_hp = 100.0f;
static float       g_class_speed_mul = 1.0f;
static float       g_veh_grip = 0.85f;   // current vehicle lateral grip (drift feel)
static float       g_speed_frac = 0.0f;  // smoothed 0..1 speed fraction
static int         g_human_scale = 0;   // 0=robot titan, 1=human-scale (campaign)
static float       g_human_k = 0.30f;    // human size relative to a robot
// Apex boss choreography: a resonance pulse fires on a fixed cadence, cycling
// through four telegraphed attack types (a readable rhythmic fight).
static float       g_apex_pulse_t = 0.0f;
static int         g_apex_pulse_type = 0;
static float       g_apex_telegraph = 0.0f;
static float       g_player_slow = 0.0f;

// enemies: an encounter of up to EX_MAX_ENEMIES, each with a body of parts
static ex_encounter_t g_enc;
static ex_model_t    *g_enemy_bodies[EX_MAX_ENEMIES][EX_ROBOT_PARTS];
static float          g_enemy_hurt[EX_MAX_ENEMIES];
static float          g_enemy_phase[EX_MAX_ENEMIES];
static ex_ally_t      g_ally;                        // Forgeheart, the Outer-Rim companion
static ex_model_t    *g_ally_parts[EX_ROBOT_PARTS];  // his body
static float          g_ally_phase = 0.0f, g_ally_yaw = 0.0f;
static int            g_ally_present = 0;             // deployed this match?
// enemy physics feel: real velocity so villains carry momentum, drift on slick
// ground, and get knocked back by blasts (heavier kinds lumber + resist).
static float          g_enemy_vel[EX_MAX_ENEMIES][3];
static float          g_enemy_prev[EX_MAX_ENEMIES][3];
// vision + awareness: enemies face a heading, see within a cone + line of sight,
// and get startled when the player pops into view (e.g. around a corner).
static float          g_enemy_head[EX_MAX_ENEMIES];    // facing yaw
static int            g_enemy_aware[EX_MAX_ENEMIES];    // has acquired the player
static float          g_enemy_synced[EX_MAX_ENEMIES];   // resonance-field sync strength 0..1
static int            g_corrupt_gen[EX_MAX_ENEMIES];     // Corrupted generation per slot (-1 = untagged/not corrupted)
static int            g_lieut[EX_MAX_ENEMIES];            // Spearcon-Lieutenant grunt? (-1 untagged, 0 no, 1 yes)
static int            g_defected[EX_MAX_ENEMIES];         // Megatron's truce: this grunt turned on the boss
static int            g_truce_on = 0;                     // truce ignited this match (latched)
static int            g_raider[EX_MAX_ENEMIES];           // Blackstar Raider elite grunt? (-1 untagged, 0 no, 1 yes)
static float          g_raider_lunge[EX_MAX_ENEMIES];     // per-raider boarding-lunge cooldown
static int            g_ravager[EX_MAX_ENEMIES];          // Corrupted Ravager elite? (-1 untagged)
static int            g_slip[EX_MAX_ENEMIES];             // Slipstream (hijack assassin) elite? (-1 untagged)
static const char    *g_cur_mapid = "";                   // current map id (for territory-themed elites)
static int            g_contain_active = 0;               // failing-containment boundary in play this match
static float          g_contain_elapsed = 0.0f;           // time since the boundary began closing
static float          g_contain_close = 60.0f;            // seconds for the boundary to fully close
static float          g_contain_ring_cd = 0.0f;           // cadence of the containment boundary pulse
static int            g_champion_slot = -1;                // enemy slot promoted to a named mini-boss (-1 none)
static int            g_champion_spawned = 0;              // one named mini-boss per match (no re-promotion after death)
static int            g_champion_kind = 0;                 // 0 none, 1 Cryotek (ice), 2 Blackstar (scavenger king)
static float          g_champion_max = 1.0f;               // champion max hp (for the boss bar)
static float          g_champion_cd = 0.0f;               // champion special-attack cooldown
static int            g_corrupt_gen_next = 0;            // running generation counter (climbs as the fight goes on)
static float          g_resfield_cd = 0.0f;             // cadence of the field's visible pulse
static float          g_graviton_cd = 0.0f;             // cadence of the graviton well's boundary pulse
static float          g_siphon_fx_cd = 0.0f;            // cadence of the siphon tether beam
static float          g_enemy_startle[EX_MAX_ENEMIES]; // startle freeze timer
static float          g_enemy_losetimer[EX_MAX_ENEMIES];// time since last seen
static int            g_enemy_mode[EX_MAX_ENEMIES];     // 0 walk,1 run/drive,2 fly
static float          g_enemy_xform_pop[EX_MAX_ENEMIES];// transform flash
static float          g_enemy_web[EX_MAX_ENEMIES];    // webbed (slowed) timer
static float enemy_mass(int kind){
  switch (kind){ case EX_ENEMY_APEX: return 3.0f; case EX_ENEMY_CORRUPTED: return 1.6f;
                 case EX_ENEMY_HIJACK: return 0.6f; default: return 1.0f; }
}
static ex_pickup_field_t g_pickups;
static ex_model_t    *g_pickup_models[EX_MAX_PICKUPS];
static float          g_player_armor = 0.0f;
static float          g_overcharge_t = 0.0f;   // remaining overcharge time

static char  g_shot_msg[48] = "";
static float g_shot_timer = 0.0f;
static char  g_event_msg[64] = "";
static float g_event_msg_timer = 0.0f;
static int   g_match_live = 0;

static void element_color(ex_element_t el, uint8_t c[3])
{
  switch (el) {
    case EX_ELEM_ICE:      c[0]=150;c[1]=220;c[2]=255; break;
    case EX_ELEM_FIRE:     c[0]=255;c[1]=120;c[2]=40;  break;
    case EX_ELEM_AIR:      c[0]=220;c[1]=240;c[2]=220; break;
    case EX_ELEM_ELECTRIC: c[0]=255;c[1]=240;c[2]=120; break;
    case EX_ELEM_VINE:     c[0]=120;c[1]=220;c[2]=120; break;
    case EX_ELEM_ROCK:     c[0]=200;c[1]=170;c[2]=130; break;
    case EX_ELEM_WATER:    c[0]=120;c[1]=180;c[2]=255; break;
    default:               c[0]=255;c[1]=220;c[2]=120; break;
  }
}

static void on_map_event(const ex_event_t *event, void *user_data)
{
  snprintf(g_event_msg, sizeof(g_event_msg), "EVENT: %s", event->name);
  g_event_msg_timer = 4.0f;
}

// the player's effective signature: an explicit builder choice overrides the
// class default (loadout.signature < 0 means "use the class default").
static ex_signature_t player_signature(void)
{
  int s = g_menu.loadout.signature;
  if (s < 0) return ex_signature_for_class(g_menu.loadout.player_class);
  return (ex_signature_t)(s % EX_SIG_COUNT);
}

// map a class signature to its loaded template mesh (falls back to wing-cannon)
static ex_model_t *sig_template(ex_signature_t s)
{
  switch (s) {
    case EX_SIG_HEAVY_CANNON: return g_tpl_cannon ? g_tpl_cannon : g_tpl_back;
    case EX_SIG_CREST:        return g_tpl_crest  ? g_tpl_crest  : g_tpl_back;
    case EX_SIG_CABLE:        return g_tpl_cable  ? g_tpl_cable  : g_tpl_back;
    case EX_SIG_HARPOON:      return g_tpl_harpoon? g_tpl_harpoon: g_tpl_back;
    case EX_SIG_HALO:         return g_tpl_halo   ? g_tpl_halo   : g_tpl_back;
    case EX_SIG_SOLAR:        return g_tpl_solar  ? g_tpl_solar  : g_tpl_back;
    case EX_SIG_AEGIS:        return g_tpl_aegis  ? g_tpl_aegis  : g_tpl_back;
    case EX_SIG_RECON:        return g_tpl_recon  ? g_tpl_recon  : g_tpl_back;
    case EX_SIG_WING_CANNON:
    default:                  return g_tpl_back;
  }
}

static ex_model_t *part_template(int i)
{
  // map each robot part to a fitting smooth mesh
  switch (i) {
    case EX_PART_HEAD:
      return g_tpl_head ? g_tpl_head : g_boxtpl;                   // articulated robot head
    case EX_PART_HAND_L: case EX_PART_HAND_R:
      return g_tpl_hand ? g_tpl_hand : g_boxtpl;                   // mechanical fists
    case EX_PART_SHOULDER_L: case EX_PART_SHOULDER_R:
      return g_tpl_sphere ? g_tpl_sphere : g_boxtpl;               // rounded shoulder joints
    case EX_PART_ARM_L: case EX_PART_ARM_R:
    case EX_PART_THIGH_L: case EX_PART_THIGH_R:
    case EX_PART_SHIN_L: case EX_PART_SHIN_R:
      return g_tpl_limb ? g_tpl_limb : g_boxtpl;                    // tapered limbs
    case EX_PART_CHEST:
      return g_tpl_chest ? g_tpl_chest : (g_tpl_bevel ? g_tpl_bevel : g_boxtpl);  // paneled cockpit chest
    case EX_PART_BACK:
      return g_tpl_back ? g_tpl_back : (g_tpl_bevel ? g_tpl_bevel : g_boxtpl);  // spine-grown wing-cannon
    case EX_PART_ABDOMEN:
    case EX_PART_FOOT_L: case EX_PART_FOOT_R:
      return g_tpl_bevel ? g_tpl_bevel : g_boxtpl;                  // chamfered torso + feet
    default: return g_boxtpl;                                       // visor stays a crisp box
  }
}
static void spawn_body(ex_model_t *parts[EX_ROBOT_PARTS])
{
  for (int i = 0; i < EX_ROBOT_PARTS; i++) {
    parts[i] = ex_model_copy(part_template(i));
    parts[i]->is_lit = 1;
    parts[i]->is_shadow = 0;
    ex_scene_add_model(scene, parts[i]);
  }
}

static ex_build_params_t g_build; // player build (from bot customizer)
#define DETAIL_MODELS 420
static ex_model_t   *g_detail_models[DETAIL_MODELS];
static ex_vehicle_type g_vehicle = EX_VEH_CAR;   // player's vehicle archetype
static ex_ability_loadout_t g_abilities;
static ex_weapon_mods_t g_wmods;
static float g_dash_t = 0.0f;
static ex_waves_t g_waves;
static int g_wave_mode = 0;      // skirmish/survival use waves
static int g_prev_alive = 0;    // to detect kills for scoring
static int g_was_alive[EX_MAX_ENEMIES];
static ex_turret_t g_turret;
static ex_model_t *g_turret_model;
static ex_point_light_t *g_key_light=NULL, *g_fill_light=NULL;
static ex_point_light_t *g_form_light=NULL, *g_boss_light=NULL;
#define NUM_BOSS_SPIKES 8
static ex_model_t *g_boss_spikes[NUM_BOSS_SPIKES];
#define NUM_SKY 6
static ex_model_t *g_sky_models[NUM_SKY];
static const char *g_sky_ids[NUM_SKY]={"sky_space","sky_ice","sky_ember","sky_cyber","sky_waste","sky_jungle"};
static int g_active_sky = 0;
static float g_gravity_scale = 1.0f;   // < 1 on space maps (low-gravity combat)
static int g_xf_flashed = 0;
static float g_recoil = 0.0f;   // weapon kick, decays each frame
static float g_climb_budget = 0.0f;  // seconds of wall-climb left (refills on landing)
static float g_climb_msg_cd = 0.0f;  // throttle for the CLIMB hud cue
static float g_barrel_heat = 0.0f;   // builds with fire, glows hot
static float g_cam_trauma = 0.0f;    // camera shake energy (0..1), decays
static float g_turn_lean = 0.0f;     // smoothed body bank into turns
static float g_lean_fwd = 0.0f;      // forward/back accel lean
static float g_lean_side = 0.0f;     // drift bank
static float g_head_pitch = 0.0f;    // head look up/down tracking
static float g_last_aim_yaw = 0.0f;
static float g_body_yaw = 0.0f;   // body faces the MOVEMENT direction (camera is where you look)
#define EX_BODY_FACING_OFF (-1.5707963f)   // mesh forward is +Z; yaw is from +X -> -90deg to align facing
static int   g_was_airborne = 0;
static float g_land_impact = 0.0f;
static float g_land_squash = 0.0f;   // brief squash on landing
static int   g_jump_launch = 0;
static float g_part_spin[EX_ROBOT_PARTS];
static float g_part_pitch[EX_ROBOT_PARTS];
// ---- microFSM battle damage: per-part integrity, joint sag, break-off ragdoll ---
static float g_part_integ[EX_ROBOT_PARTS];    // 1 intact .. 0 destroyed
static int   g_part_detached[EX_ROBOT_PARTS]; // 1 = broken off (needs energy to regrow)
static float g_repair_energy = 0.0f;          // energy bank for major repairs (energon)
static float g_player_pos[3] = {0,0,0};
static ex_material_id g_ground_mat = EX_MAT_METAL;   // current surface
static float g_surface_grip = 1.0f;
static float g_resonance_cd = 0.0f;
static float g_pulse_cd = 0.0f, g_blink_cd = 0.0f;
static float g_spark_loud = 0.0f;   // spark-resonance "loudness": rises with activity, decays when quiet
static int g_turret_ammo = 0;
static int g_landing_active = 0;
static ex_warp_wave_t g_warp_wave = {0,0,0,{0,0,0}};   // inbound delayed gravity wave
static ex_tsunami_t g_tsunami = {0,{0,0,0},{0,0,0},0}; // water-form surge
static float        g_tsunami_cd = 0.0f;               // cooldown between surges
static float        g_arc_cd = 0.0f;                   // electric-form chain-lightning cooldown
static int          g_cyclone_on = 0;                  // air-form whirlwind active
static float        g_cyclone_t = 0.0f;                // whirlwind time remaining
static float        g_cyclone_cd = 0.0f;               // whirlwind cooldown
static float        g_cyclone_c[3] = {0,0,0};          // whirlwind eye (world xz)
static int          g_thorns_on = 0;                   // vine-form thorn field active
static float        g_thorns_age = 0.0f;               // thorn field age
static float        g_thorns_cd = 0.0f;                // thorn field cooldown
static float        g_thorns_c[3] = {0,0,0};           // thorn field centre (world xz)
static float        g_thorns_fx_cd = 0.0f;             // cadence of thorn spatter fx
static float g_landing_tgt[3] = {0,0,0}, g_landing_spin = 0.0f;
static ex_model_t *g_landing_ring = NULL;
// ---- environmental hazard zones: green toxic sludge (long DOT + handicap) and
//      thruster chambers (wind blast + ash). Placed per skirmish map. ----
#define NUM_HAZARDS 8
#define HAZ_NONE 0
#define HAZ_TOXIC 1
#define HAZ_WIND 2
static int   g_haz_type[NUM_HAZARDS];
static float g_haz_pos[NUM_HAZARDS][3], g_haz_rad[NUM_HAZARDS], g_haz_wind[NUM_HAZARDS][3];
static ex_model_t *g_haz_models[NUM_HAZARDS];
static int   g_haz_n = 0;
static float g_toxic_dot = 0.0f;      // seconds of toxic DOT remaining (up to 60)
static float g_map_gravity = 1.0f;    // per-map gravity scale for skirmish variety
static int   g_horror_mode = 0;   // dread level: heavy fog, stalkers, isolation
static float g_horror_key[3] = {0.2f,0.2f,0.25f};   // base key-light colour to flicker around
static float g_dread = 0.0f;       // 0..1 proximity dread meter
static int g_tune_active=0, g_tune_sel=0;   // live feel-tuning overlay
// ---- close-quarters clash: a quick-time key-sequence duel triggered when an
//      enemy closes to melee range. Nail the pattern for a finisher; fail and
//      take the hit. Uses arrow-key "movement patterns" as the input. ----
static int   g_clash_active=0, g_clash_enemy=-1, g_clash_len=0, g_clash_pos=0;
static int   g_clash_seq[5];                // 0=up,1=down,2=left,3=right
static float g_clash_timer=0, g_clash_cd=0; // window remaining; re-trigger cooldown
static const char *CLASH_GLYPH[4] = { "UP","DN","LT","RT" };
static const int   CLASH_SC[4] = { SDL_SCANCODE_UP, SDL_SCANCODE_DOWN, SDL_SCANCODE_LEFT, SDL_SCANCODE_RIGHT };
#define NUM_DEBRIS 48
typedef struct { float pos[3], vel[3], rot[3], rotvel[3], life, maxlife, scale; int active; } debris_t;
static debris_t    g_debris[NUM_DEBRIS];
static ex_model_t *g_debris_models[NUM_DEBRIS];
static int         g_debris_next = 0;
static unsigned    g_dbg_rng = 12345u;
static float dbg_frand(void){ g_dbg_rng=g_dbg_rng*1664525u+1013904223u; return ((g_dbg_rng>>9)&0xFFFF)/65535.0f; }
static void spawn_debris(float x,float y,float z,float sc,float r,float g,float b,float vboost){
  int i=g_debris_next; g_debris_next=(g_debris_next+1)%NUM_DEBRIS;
  debris_t *d=&g_debris[i]; d->active=1; d->maxlife=d->life=3600.0f; d->scale=sc;   // persist ~1hr as litter; ring buffer evicts oldest
  d->pos[0]=x; d->pos[1]=y; d->pos[2]=z;
  d->vel[0]=(dbg_frand()-0.5f)*10.0f*vboost; d->vel[1]=4.0f+dbg_frand()*9.0f*vboost; d->vel[2]=(dbg_frand()-0.5f)*10.0f*vboost;
  d->rot[0]=d->rot[1]=d->rot[2]=0;
  d->rotvel[0]=(dbg_frand()-0.5f)*16.0f; d->rotvel[1]=(dbg_frand()-0.5f)*16.0f; d->rotvel[2]=(dbg_frand()-0.5f)*16.0f;
  if (g_debris_models[i]){ g_debris_models[i]->tint[0]=r; g_debris_models[i]->tint[1]=g; g_debris_models[i]->tint[2]=b; }
}
static void debris_update(float dt){
  for (int i=0;i<NUM_DEBRIS;i++){ debris_t *d=&g_debris[i]; ex_model_t *m=g_debris_models[i]; if(!m) continue;
    if (!d->active){ m->position[1]=-1000.0f; continue; }
    d->vel[1]-=34.0f*g_map_gravity*dt;     // low-g worlds let debris hang + float (cinematic)
    for(int k=0;k<3;k++) d->pos[k]+=d->vel[k]*dt;
    if (d->pos[1] < d->scale){ d->pos[1]=d->scale; d->vel[1]*=-0.38f; d->vel[0]*=0.72f; d->vel[2]*=0.72f;
      for(int k=0;k<3;k++) d->rotvel[k]*=0.7f; }
    for(int k=0;k<3;k++) d->rot[k]+=d->rotvel[k]*dt;
    d->life-=dt; if(d->life<=0){ d->active=0; m->position[1]=-1000.0f; continue; }
    m->position[0]=d->pos[0]; m->position[1]=d->pos[1]; m->position[2]=d->pos[2];
    float sh=d->scale*fminf(d->life/0.6f,1.0f);      // shrink out at the end
    m->scale3[0]=sh; m->scale3[1]=sh; m->scale3[2]=sh;
    m->rotation[0]=d->rot[0]*57.3f; m->rotation[1]=d->rot[1]*57.3f; m->rotation[2]=d->rot[2]*57.3f;
  }
}
// something moving through the debris field (a bot, a bullet impact) kicks it up
static void kick_debris(float x, float z, float radius, float force){
  for (int i=0;i<NUM_DEBRIS;i++){ debris_t *d=&g_debris[i]; if(!d->active) continue;
    float dx=d->pos[0]-x, dz=d->pos[2]-z; float d2=dx*dx+dz*dz;
    if (d2 < radius*radius){ float dd=sqrtf(d2)+0.01f, t=1.0f-dd/radius;
      d->vel[0]+=dx/dd*force*t; d->vel[2]+=dz/dd*force*t; d->vel[1]+=force*t*0.6f;
      d->rotvel[0]+=force*t; }
  }
}
static float g_hit_flash = 0.0f;      // red screen flash on taking damage
static float g_hit_dir[3] = {0,0,0};  // world dir the last hit came from
static const char *g_hit_src = "";    // what hit us (for the HUD)
static long g_high_score = 0;
static void hide_body(ex_model_t **parts);
static void save_config(void);
static void load_config(void);
static void load_map_colliders(const char *mapid);
static void load_asset_aabbs(void);
static void auto_collider(const char *mesh, const float pos[3], float scale);
static void player_hit(float dmg, float fromx, float fromz, const char *src)
{
  dmg *= g_sig_stats.taken_mul;   // signature toughness (crest tanky, wings fragile)
  if (ex_photon_active(&g_photon)) dmg *= 0.12f;   // light form: near-untouchable
  if (ex_matrix_active(&g_matrix)) dmg *= 0.6f;    // Rodimus: hardened, not invincible
  float a = fminf(g_player_armor, dmg); g_player_armor -= a; g_player_hp -= (dmg - a);
  g_hit_flash = 0.5f; g_hit_src = src;
  sfx(g_sfx_hit);
  g_cam_trauma += 0.32f;   // jolt when hit
  g_hit_dir[0] = fromx; g_hit_dir[2] = fromz;
  // --- microFSM battle damage: chip parts, sag joints, break bits off ---
  int hits = 1 + (dmg > 25.0f ? 1 : 0);
  // extremities take the brunt (arms, shins, shoulders, head, back)
  static const int soft[] = { EX_PART_ARM_L,EX_PART_ARM_R,EX_PART_SHIN_L,EX_PART_SHIN_R,
                              EX_PART_SHOULDER_L,EX_PART_SHOULDER_R,EX_PART_HEAD,EX_PART_BACK,
                              EX_PART_HAND_L,EX_PART_HAND_R };
  int nsoft=(int)(sizeof(soft)/sizeof(soft[0]));
  for (int h=0; h<hits; h++){
    int pi = soft[(int)(dbg_frand()*nsoft) % nsoft];
    if (pi>=EX_ROBOT_PARTS || g_part_detached[pi]) continue;
    g_part_integ[pi] -= dmg*0.018f;
    if (g_part_integ[pi] <= 0.0f){ g_part_integ[pi]=0.0f; g_part_detached[pi]=1;
      spawn_debris(g_player_pos[0], g_player_pos[1]+2.0f, g_player_pos[2], 0.4f, 0.45f,0.5f,0.6f, 1.2f);
      snprintf(g_shot_msg,sizeof(g_shot_msg),"armour breach -- part severed"); g_shot_timer=1.4f; }
  }
  // scatter of small metal chips from the impact
  int chips = 2 + (int)(dbg_frand()*3);
  for (int c=0;c<chips;c++)
    spawn_debris(g_player_pos[0], g_player_pos[1]+1.5f, g_player_pos[2], 0.14f, 0.7f,0.6f,0.4f, 0.8f);
}

static long load_high_score(void){
  FILE *f = fopen("cybertron_highscore.dat","r");
  if(!f) return 0; long v=0; if(fscanf(f,"%ld",&v)!=1) v=0; fclose(f); return v;
}
// ---- config persistence (Linux-friendly XDG path) --------------------------
static void config_path(char *out, size_t n){
  const char *xdg = getenv("XDG_CONFIG_HOME");
  const char *home = getenv("HOME");
  if (xdg && *xdg) snprintf(out,n,"%s/cybertron", xdg);
  else if (home && *home) snprintf(out,n,"%s/.config/cybertron", home);
  else snprintf(out,n,".");
}
static void save_config(void){
  char dir[512]; config_path(dir,sizeof(dir));
  char cmd[600]; snprintf(cmd,sizeof(cmd),"mkdir -p '%s' 2>/dev/null", dir); (void)system(cmd);
  char path[600]; snprintf(path,sizeof(path),"%s/config.ini", dir);
  FILE *f=fopen(path,"w"); if(!f){ snprintf(g_shot_msg,sizeof(g_shot_msg),"config save failed"); g_shot_timer=2.0f; return; }
  fprintf(f,"# Cybertron config\n");
  fprintf(f,"layout=%d\n", (int)g_menu.loadout.layout);
  for (int i=0;i<EX_ACT_COUNT;i++) fprintf(f,"key%d=%d\n", i, (int)ex_keybind_get(&g_keys,(ex_action_t)i));
  fprintf(f,"gui_scale=%d\n", g_menu.gfx.gui_scale);
  fprintf(f,"frame_cap=%d\n", g_menu.gfx.frame_cap);
  fprintf(f,"brightness=%d\n", g_menu.gfx.brightness);
  fprintf(f,"volume=%d\n", g_menu.gfx.volume);
  fprintf(f,"sensitivity=%d\n", g_menu.gfx.sensitivity);
  fprintf(f,"render_scale=%d\n", g_menu.gfx.render_scale);
  fprintf(f,"show_fps=%d\n", g_menu.gfx.show_fps);
  fclose(f);
  snprintf(g_shot_msg,sizeof(g_shot_msg),"configuration stored"); g_shot_timer=2.0f;
}
static void load_config(void){
  char dir[512]; config_path(dir,sizeof(dir));
  char path[600]; snprintf(path,sizeof(path),"%s/config.ini", dir);
  FILE *f=fopen(path,"r"); if(!f) return;
  char line[128];
  while (fgets(line,sizeof(line),f)){
    int v; 
    if (sscanf(line,"layout=%d",&v)==1) g_menu.loadout.layout=(ex_kb_layout_t)v;
    else if (sscanf(line,"gui_scale=%d",&v)==1) g_menu.gfx.gui_scale=v;
    else if (sscanf(line,"frame_cap=%d",&v)==1) g_menu.gfx.frame_cap=v;
    else if (sscanf(line,"brightness=%d",&v)==1) g_menu.gfx.brightness=v;
    else if (sscanf(line,"volume=%d",&v)==1) g_menu.gfx.volume=v;
    else if (sscanf(line,"sensitivity=%d",&v)==1) g_menu.gfx.sensitivity=v;
    else if (sscanf(line,"render_scale=%d",&v)==1) g_menu.gfx.render_scale=v;
    else if (sscanf(line,"show_fps=%d",&v)==1) g_menu.gfx.show_fps=v;
    else { int idx; if (sscanf(line,"key%d=%d",&idx,&v)==2 && idx>=0 && idx<EX_ACT_COUNT)
             ex_keybind_set(&g_keys,(ex_action_t)idx,(SDL_Scancode)v); }
  }
  fclose(f);
  g_keys_loaded_from_config = 1;
}
static void save_high_score(long v){
  FILE *f = fopen("cybertron_highscore.dat","w");
  if(!f) return; fprintf(f,"%ld\n",v); fclose(f);
}
// fill the results screen + persist a new best, then route to RESULTS
static void end_wave_run(int victory){
  g_match_live = 0;
  hide_body(g_player_parts); for(int i=0;i<EX_MAX_ENEMIES;i++) hide_body(g_enemy_bodies[i]);
  for(int i=0;i<DETAIL_MODELS;i++) g_detail_models[i]->position[1]=-1000.0f;
  g_menu.res_victory = victory;
  g_menu.res_waves = g_waves.wave;
  g_menu.res_kills = g_waves.kills;
  g_menu.res_score = g_waves.score;
  if (g_high_score == 0) g_high_score = load_high_score();
  g_menu.res_new_best = (g_waves.score > g_high_score);
  if (g_menu.res_new_best){ g_high_score = g_waves.score; save_high_score(g_high_score); }
  g_menu.res_best = g_high_score;
  g_menu.state = EX_GS_RESULTS; g_menu.cursor = 0;
}
static void apply_theme_lighting(const char *mid, float brightness);
static ex_map_blueprint_t g_blueprint;
// 2D segment vs AABB (slab test) -- used for line-of-sight through cover
static int seg_aabb2d(float ax,float az,float bx,float bz,float mnx,float mnz,float mxx,float mxz){
  float dx=bx-ax, dz=bz-az, tmin=0.0f, tmax=1.0f;
  if (fabsf(dx)<1e-6f){ if(ax<mnx||ax>mxx) return 0; }
  else { float t1=(mnx-ax)/dx, t2=(mxx-ax)/dx; if(t1>t2){float t=t1;t1=t2;t2=t;} if(t1>tmin)tmin=t1; if(t2<tmax)tmax=t2; }
  if (fabsf(dz)<1e-6f){ if(az<mnz||az>mxz) return 0; }
  else { float t1=(mnz-az)/dz, t2=(mxz-az)/dz; if(t1>t2){float t=t1;t1=t2;t2=t;} if(t1>tmin)tmin=t1; if(t2<tmax)tmax=t2; }
  return tmin<=tmax;
}
// is the sightline from (ax,az) to (bx,bz) blocked by tall cover?
static int los_blocked(float ax,float az,float bx,float bz){
  for (int i=0;i<g_blueprint.n_vols;i++){ ex_map_vol_t *v=&g_blueprint.vols[i];
    if (v->half[1] < 3.0f) continue;   // low cover doesn't block a giant's sightline
    if (seg_aabb2d(ax,az,bx,bz, v->center[0]-v->half[0], v->center[2]-v->half[2],
                                 v->center[0]+v->half[0], v->center[2]+v->half[2])) return 1; }
  return 0;
}

static ex_model_t *g_vol_models[EX_MAP_MAX_VOLS];   // box (walls/platforms)
static ex_model_t *g_rock_models[EX_MAP_MAX_VOLS];  // smooth rock (cover/blocks)
static ex_model_t *g_spire_models[EX_MAP_MAX_VOLS]; // tapered spire (expose/vantage)
#define DECO_ROCK 14
#define DECO_TALL 10
static ex_model_t *g_deco_rock[DECO_ROCK];
static ex_model_t *g_deco_tall[DECO_TALL];
#define DECO_ARCH 2
static ex_model_t *g_deco_arch[DECO_ARCH];
#define DECO_TOWER 3
static ex_model_t *g_deco_tower[DECO_TOWER];
#define MAX_DECO_COL 40
static float g_deco_col[MAX_DECO_COL][6];   // {cx,cy,cz, hx,hy,hz}
static int   g_deco_col_n = 0;
#define MAX_MAP_COL 120
static float g_map_col[MAX_MAP_COL][6];     // themed-map blockers (from .col)
static int   g_map_col_n = 0;
// programmatic bounding boxes: per-mesh local AABB (from asset_aabb.txt) so any
// generated/microFSM asset gets a decent collider = local_half * placed scale.
#define MAX_ASSET_AABB 64
static char  g_aabb_name[MAX_ASSET_AABB][24];
static float g_aabb_cen[MAX_ASSET_AABB][3];
static float g_aabb_half[MAX_ASSET_AABB][3];
static int   g_aabb_n = 0;
#define MAX_AUTO_COL 140
static float g_auto_col[MAX_AUTO_COL][6];    // {cx,cy,cz, hx,hy,hz} auto colliders
static int   g_auto_col_n = 0;
static int   g_feature_col_base = 0;
#define NUM_CLUTTER 60
static ex_model_t *g_clutter[NUM_CLUTTER];
static float g_clutter_realy[NUM_CLUTTER];
// ---- mixed-scale co-play: human NPCs living on the map among the titan bots ---
#define NUM_HUMANS 8
typedef struct { float pos[3], vel[3], phase, tgt[2], retimer, downed; int active, fleeing; } human_npc_t;
static human_npc_t g_humans[NUM_HUMANS];
static ex_model_t *g_human_parts[NUM_HUMANS][4];   // head, torso, legs, pack
static int g_survivors_total=0, g_survivors_lost=0, g_protect_mission=0;
static void spawn_humans(void);
static void humans_update(float dt);
static void humans_hide(void);
static ex_entity_t *g_pe = NULL;
// expanding shockwave rings (visual)
#define NUM_SHOCKRINGS 6
static ex_model_t *g_shockrings[NUM_SHOCKRINGS];
static float g_sring_t[NUM_SHOCKRINGS], g_sring_max[NUM_SHOCKRINGS], g_sring_pos[NUM_SHOCKRINGS][3];
static int g_sring_active[NUM_SHOCKRINGS]={0}, g_sring_next=0;
// persistent ground craters / scorch marks from big impacts (fake fracture: flat
// decals rather than true mesh deformation, but they scar the battlefield).
#define NUM_CRATERS 24
static ex_model_t *g_craters[NUM_CRATERS];
static int g_crater_next=0;
static float g_crater_realy[NUM_CRATERS];
static void spawn_shockring(float x,float y,float z,float maxr){
  int i=g_sring_next; g_sring_next=(g_sring_next+1)%NUM_SHOCKRINGS;
  g_sring_active[i]=1; g_sring_t[i]=0; g_sring_max[i]=maxr;
  g_sring_pos[i][0]=x; g_sring_pos[i][1]=y+0.5f; g_sring_pos[i][2]=z;
}
static void shockrings_update(float dt){
  for (int i=0;i<NUM_SHOCKRINGS;i++){ ex_model_t *m=g_shockrings[i]; if(!m) continue;
    if(!g_sring_active[i]){ m->position[1]=-1000.0f; continue; }
    g_sring_t[i]+=dt*2.4f; float t=g_sring_t[i]; if(t>=1.0f){ g_sring_active[i]=0; m->position[1]=-1000.0f; continue; }
    float r=g_sring_max[i]*t; float fade=1.0f-t;
    m->position[0]=g_sring_pos[i][0]; m->position[1]=g_sring_pos[i][1]; m->position[2]=g_sring_pos[i][2];
    m->scale3[0]=r; m->scale3[1]=0.4f; m->scale3[2]=r;
    m->tint[0]=1.0f*fade; m->tint[1]=0.7f*fade; m->tint[2]=0.3f*fade; }
}   // player entity (for shockwave knockback)
// radial shockwave from a blast: flings debris up/out, shoves + damages enemies
// with distance falloff, and knocks the player.
static void apply_shockwave(float cx, float cy, float cz, float radius, float force, float dmg){
  (void)cy;
  for (int i=0;i<NUM_DEBRIS;i++){ debris_t *d=&g_debris[i]; if(!d->active) continue;
    float dx=d->pos[0]-cx, dz=d->pos[2]-cz; float dd=sqrtf(dx*dx+dz*dz);
    if (dd<radius){ float t=1.0f-dd/radius, inv=1.0f/fmaxf(dd,1.0f);
      d->vel[0]+=dx*inv*force*t; d->vel[2]+=dz*inv*force*t; d->vel[1]+=force*t*0.6f; } }
  for (int q=0;q<g_enc.count;q++){ ex_enemy_t *en=&g_enc.enemies[q]; if(en->state==EX_ENEMY_DEAD) continue;
    float dx=en->position[0]-cx, dz=en->position[2]-cz; float dd=sqrtf(dx*dx+dz*dz);
    if (dd<radius){ float t=1.0f-dd/radius, inv=1.0f/fmaxf(dd,1.0f);
      int wi=(int)(en-g_enc.enemies);
      if (wi>=0&&wi<EX_MAX_ENEMIES){ float im=force*t/enemy_mass(en->kind);
        g_enemy_vel[wi][0]+=dx*inv*im*2.0f; g_enemy_vel[wi][2]+=dz*inv*im*2.0f; g_enemy_hurt[wi]=0.16f; }
      ex_enemy_damage(en, dmg*t); } }
  if (g_pe){ float dx=g_pe->position[0]-cx, dz=g_pe->position[2]-cz; float dd=sqrtf(dx*dx+dz*dz);
    if (dd<radius){ float t=1.0f-dd/radius, inv=1.0f/fmaxf(dd,1.0f);
      g_pe->velocity[0]+=dx*inv*force*t*1.2f; g_pe->velocity[2]+=dz*inv*force*t*1.2f; g_pe->velocity[1]+=force*t*0.5f; } }
}
// live feel-tuning: F1 toggles; [ ] select a dial; - = adjust it. Changes hit
static void feel_tuner_update_draw(void){
  struct { const char*n; float*v; float step; } D[] = {
    {"mass",&g_feel.mass,0.1f},{"spring_hz",&g_feel.spring_hz,0.5f},
    {"damp_ratio",&g_feel.damp_ratio,0.05f},{"g_show",&g_feel.g_show,0.1f},
    {"cam_lag",&g_feel.cam_lag,0.02f},{"grip",&g_feel.grip,0.05f},
    {"drag",&g_feel.drag,0.1f},{"turn",&g_feel.turn,0.2f} };
  int N=8;
  if (ex_keys_down[SDL_SCANCODE_LEFTBRACKET]){ g_tune_sel=(g_tune_sel-1+N)%N; ex_keys_down[SDL_SCANCODE_LEFTBRACKET]=0; }
  if (ex_keys_down[SDL_SCANCODE_RIGHTBRACKET]){ g_tune_sel=(g_tune_sel+1)%N; ex_keys_down[SDL_SCANCODE_RIGHTBRACKET]=0; }
  if (ex_keys_down[SDL_SCANCODE_MINUS]){ *D[g_tune_sel].v-=D[g_tune_sel].step; if(*D[g_tune_sel].v<0.01f)*D[g_tune_sel].v=0.01f; ex_keys_down[SDL_SCANCODE_MINUS]=0; }
  if (ex_keys_down[SDL_SCANCODE_EQUALS]){ *D[g_tune_sel].v+=D[g_tune_sel].step; ex_keys_down[SDL_SCANCODE_EQUALS]=0; }
  ex_vga_setfg(90,240,180,255); ex_vga_print(4, 1, "FEEL TUNER F1");
  ex_vga_print(4, 2, "[ ]pick -=adj");
  for (int i=0;i<N;i++){ char b[28]; snprintf(b,sizeof(b),"%s%-9s%.2f",(i==g_tune_sel)?">":" ",D[i].n,*D[i].v);
    ex_vga_setfg(i==g_tune_sel?255:170, 255, i==g_tune_sel?150:200,255); ex_vga_print(4, 4+i, b); }
  ex_vga_setfg(255,255,255,255);
}
static void clash_update_draw(float dt);
static void spawn_crater(float x, float z, float size){
  int i=g_crater_next; g_crater_next=(g_crater_next+1)%NUM_CRATERS;
  ex_model_t *c=g_craters[i]; if(!c) return;
  const ex_material_t *m=ex_material(g_ground_mat);
  c->position[0]=x; c->position[1]=0.12f + (float)(i%3)*0.01f; c->position[2]=z; g_crater_realy[i]=c->position[1];
  c->scale3[0]=size; c->scale3[1]=size*0.14f; c->scale3[2]=size;   // relief for the noisy bowl
  c->rotation[1]=(float)((i*47)%360);
  // scorched base darkened by hardness; gritty ground leaves a paler dust crater
  float dark = m->hardness>0.6f ? 0.10f : 0.22f;
  c->tint[0]=dark + m->spark[0]/255.0f*0.08f;
  c->tint[1]=dark + m->spark[1]/255.0f*0.06f;
  c->tint[2]=dark + m->spark[2]/255.0f*0.06f;
  // dust plume + scattered chunks kicked out of the fresh crater
  uint8_t dust[3]={ (uint8_t)(m->spark[0]*0.7f+40), (uint8_t)(m->spark[1]*0.6f+40), (uint8_t)(m->spark[2]*0.6f+40) };
  ex_vfx_spawn_impact(&g_vfx, (vec3){x, 1.0f, z}, 10, dust);
  for (int k=0;k<6;k++) spawn_debris(x + (dbg_frand()-0.5f)*size, 0.5f, z + (dbg_frand()-0.5f)*size,
                                     0.12f+dbg_frand()*0.14f, m->spark[0]/255.0f,m->spark[1]/255.0f,m->spark[2]/255.0f, 1.0f);
}
static void spawn_explosion(float x,float y,float z,float scale,int chunks){
  // radial shockwave: fling debris, shove + damage enemies, knock the player.
  apply_shockwave(x, y, z, 22.0f*scale, 22.0f*scale, 16.0f*scale);
  spawn_shockring(x, y, z, 22.0f*scale);
  if (scale >= 0.9f) spawn_crater(x, z, 3.0f*scale);   // hard blasts scar the ground
  uint8_t oc[3]={255,180,80}, sc[3]={255,120,40};
  ex_vfx_spawn_impact(&g_vfx,(vec3){x,y,z},(int)(18*scale),oc);
  ex_vfx_spawn_death(&g_vfx,(vec3){x,y+1.0f,z},sc);
  for(int c=0;c<chunks;c++) spawn_debris(x,y+1.0f,z, 0.14f+dbg_frand()*0.30f*scale, 0.5f,0.45f,0.42f, 1.4f*scale);
  sfx(g_sfx_expl);
  { float dx=x-g_player_pos[0], dz=z-g_player_pos[2]; float d=sqrtf(dx*dx+dz*dz);
    g_cam_trauma += 0.5f*scale*fmaxf(0.0f, 1.0f - d/80.0f); }
}

// close-quarters clash QTE -- runs logic + draws the prompt each frame while active
static void clash_update_draw(float dt){
  if (g_clash_cd>0) g_clash_cd-=dt;
  if (!g_clash_active){
    if (g_clash_cd>0 || !g_pe || g_human_scale) return;
    int best=-1; float bd=1e9f;
    for (int q=0;q<g_enc.count;q++){ ex_enemy_t *en=&g_enc.enemies[q]; if(en->state==EX_ENEMY_DEAD)continue;
      float dx=en->position[0]-g_pe->position[0], dz=en->position[2]-g_pe->position[2]; float d=dx*dx+dz*dz;
      if (d<bd){bd=d;best=q;} }
    float R=7.0f;
    if (best>=0 && bd<R*R){
      g_clash_active=1; g_clash_enemy=best; g_clash_pos=0;
      g_clash_len=2+(int)(dbg_frand()*3);
      for(int k=0;k<g_clash_len;k++) g_clash_seq[k]=(int)(dbg_frand()*4)&3;
      g_clash_timer=1.2f+g_clash_len*0.5f; g_cam_trauma+=0.3f;
    }
    return;
  }
  g_clash_timer-=dt;
  ex_enemy_t *en=(g_clash_enemy>=0&&g_clash_enemy<g_enc.count)?&g_enc.enemies[g_clash_enemy]:NULL;
  if(!en||en->state==EX_ENEMY_DEAD){ g_clash_active=0; g_clash_cd=1.0f; return; }
  int pressed=-1;
  for(int k=0;k<4;k++) if(ex_keys_down[CLASH_SC[k]]){ pressed=k; ex_keys_down[CLASH_SC[k]]=0; }
  int fail=0;
  if(pressed>=0){
    if(pressed==g_clash_seq[g_clash_pos]){ g_clash_pos++;
      if(g_clash_pos>=g_clash_len){   // SUCCESS -> finisher
        float ep[3]={en->position[0],en->position[1],en->position[2]};
        ex_enemy_damage(en,9999.0f); spawn_explosion(ep[0],ep[1],ep[2],1.4f,10);
        snprintf(g_shot_msg,sizeof(g_shot_msg),"CLASH WON -- finisher!"); g_shot_timer=1.8f;
        g_cam_trauma+=0.6f; g_clash_active=0; g_clash_cd=2.0f; return; }
    } else fail=1;
  }
  if(g_clash_timer<=0.0f) fail=1;
  if(fail){
    float dx=g_pe->position[0]-en->position[0], dz=g_pe->position[2]-en->position[2]; float d=sqrtf(dx*dx+dz*dz)+0.01f;
    g_pe->velocity[0]+=dx/d*22.0f; g_pe->velocity[2]+=dz/d*22.0f;
    player_hit(18.0f, en->position[0], en->position[2], "clash lost");
    snprintf(g_shot_msg,sizeof(g_shot_msg),"CLASH LOST -- thrown back"); g_shot_timer=1.8f;
    g_clash_active=0; g_clash_cd=2.0f; return;
  }
  // draw the QTE prompt
  ex_vga_setfg(255,210,110,255); ex_vga_print(11, 6, "== CLASH ==");
  char row[48]=""; for(int k=0;k<g_clash_len;k++){ strcat(row,(k<g_clash_pos)?"OK":CLASH_GLYPH[g_clash_seq[k]]); strcat(row," "); }
  ex_vga_setfg(255,255,255,255); ex_vga_print(9, 8, row);
  float frac=g_clash_timer/(1.2f+g_clash_len*0.5f); int bar=(int)(frac*16); if(bar<0)bar=0; if(bar>16)bar=16;
  char tb[20]; for(int k=0;k<bar;k++) tb[k]='='; tb[bar]=0;
  ex_vga_setfg(255,120,80,255); ex_vga_print(9, 9, tb); ex_vga_setfg(255,255,255,255);
}


// ---- vehicle physics feel: g-forces drive a spring-damper suspension and a
//      weighted camera, so acceleration, braking, cornering and bumps are felt.
static float g_prev_vel[3] = {0,0,0};
static float g_gforce_long=0, g_gforce_lat=0;                 // smoothed g readings
static float g_susp_pitch=0, g_susp_pitch_v=0;               // suspension spring states
static float g_susp_roll=0,  g_susp_roll_v=0;
static float g_susp_heave=0, g_susp_heave_v=0;
static float g_cam_gf[3] = {0,0,0};                          // weighted camera g-force offset
static void vehicle_physics_update(ex_entity_t *e, float dt, int in_vehicle, float aim_yaw)
{
  if (dt < 1e-5f) return;
  float ax=(e->velocity[0]-g_prev_vel[0])/dt, ay=(e->velocity[1]-g_prev_vel[1])/dt, az=(e->velocity[2]-g_prev_vel[2])/dt;
  g_prev_vel[0]=e->velocity[0]; g_prev_vel[1]=e->velocity[1]; g_prev_vel[2]=e->velocity[2];
  float fwx=cosf(aim_yaw), fwz=sinf(aim_yaw), rgx=-sinf(aim_yaw), rgz=cosf(aim_yaw);
  float long_g = ax*fwx + az*fwz;    // + accelerate, - brake
  float lat_g  = ax*rgx + az*rgz;    // cornering
  g_gforce_long += (long_g - g_gforce_long)*fminf(dt*10.0f,1.0f);
  g_gforce_lat  += (lat_g  - g_gforce_lat )*fminf(dt*10.0f,1.0f);
  // weight transfer -> suspension targets (magnitude from the shared feel dials)
  float ss = ex_feel_susp_scale();
  float tgt_pitch = -g_gforce_long * ss;         // accel squats rear (nose up), brake dives
  float tgt_roll  =  g_gforce_lat  * ss * 1.3f;  // body rolls out of the corner
  float tgt_heave = -ay * ss * 0.4f;
  // procedural terrain bumps (flat ground, simulated): jostle scaled by speed
  if (in_vehicle && e->grounded==1){
    float spd=sqrtf(e->velocity[0]*e->velocity[0]+e->velocity[2]*e->velocity[2]);
    float sf=fminf(spd/20.0f,1.0f), bt=g_anim_t*(2.0f+spd*0.35f);
    tgt_heave += sinf(bt*3.1f + e->position[0]*0.25f)*0.020f*sf;
    tgt_pitch += sinf(bt*2.3f)*0.006f*sf;
    tgt_roll  += sinf(bt*2.9f + e->position[2]*0.2f)*0.008f*sf;
  }
  // spring-damper integrate -- one shared, critically-damped spring definition, so
  // tuning spring_hz / damp_ratio / mass in g_feel retunes ALL three channels.
  ex_feel_spring(&g_susp_pitch, &g_susp_pitch_v, tgt_pitch, dt);
  ex_feel_spring(&g_susp_roll,  &g_susp_roll_v,  tgt_roll,  dt);
  ex_feel_spring(&g_susp_heave, &g_susp_heave_v, tgt_heave, dt);
  // weighted camera g-force: push the camera opposite the acceleration (felt g's)
  float gsh = ex_feel_cam_gshift(), crate = ex_feel_cam_rate()*dt;
  float cam_tlat = -lat_g*gsh, cam_tlong = -long_g*gsh*0.9f, cam_theave = g_susp_heave;
  g_cam_gf[0] += (cam_tlat  - g_cam_gf[0])*fminf(crate,1.0f);
  g_cam_gf[1] += (cam_theave- g_cam_gf[1])*fminf(crate*1.3f,1.0f);
  g_cam_gf[2] += (cam_tlong - g_cam_gf[2])*fminf(crate,1.0f);
}
#define THEME_VIS_SCALE 16.0f
#define NUM_LAUNCH 6
static float g_launch[NUM_LAUNCH][3];
static int   g_launch_n = 0;
static ex_model_t *g_launch_models[NUM_LAUNCH];
#define NUM_DEF_TURRETS 10
typedef struct { float pos[3]; float angle; float cd; int active; } def_turret_t;
static def_turret_t g_deft[NUM_DEF_TURRETS];
static int g_deft_n = 0;
static ex_model_t *g_deft_models[NUM_DEF_TURRETS];
static ex_model_t *g_deft_barrels[NUM_DEF_TURRETS];
#define NUM_LIQUID 6
static ex_model_t *g_liquid[NUM_LIQUID];
static float g_liquid_col[3] = {0.3f,0.5f,1.0f};
static int   g_liquid_n = 0;
#define MAP_DETAIL 200
static ex_model_t *g_map_detail[MAP_DETAIL];
static ex_model_t *g_weapon_models[10]={0};   // by fire mode (bolt..suppress)
static ex_model_t *g_enemy_weapons[EX_MAX_ENEMIES]={0};
static ex_model_t *g_ship_model=NULL, *g_ship_glow[2]={0};
static int g_menu_scene_init=0;
#define NUM_PLANETS 5
static ex_model_t *g_planets[NUM_PLANETS];
static const char *g_planet_ids[NUM_PLANETS]={"planet_earth","planet_jupiter","planet_cybertron","planet_mars","planet_pluto"};
#define NUM_OBJ_PADS 3
static ex_model_t *g_obj_pads[NUM_OBJ_PADS];
static ex_model_t *g_muzzle=NULL;
static ex_model_t *g_cape=NULL; static float g_cape_sway[2]={0,0};
#define NUM_CONSOLES 3
static ex_model_t *g_consoles[NUM_CONSOLES];
static float g_console_pos[NUM_CONSOLES][3];
#define NUM_MOTES 44
static ex_model_t *g_motes[NUM_MOTES];
static float g_mote_p[NUM_MOTES][3];   // drift positions relative to player
static float g_mote_col[3] = {0.7f,0.7f,0.8f};
static float g_obj_pos[NUM_OBJ_PADS][3];
static float g_obj_cap[NUM_OBJ_PADS];    // capture progress 0..1
static int   g_obj_owner[NUM_OBJ_PADS];  // 0=neutral, 1=player
static float g_obj_score = 0.0f;         // accrued control score
static float g_control_target = 0.0f;    // >0: reach this control score to win
#define DEPLOY_TIME 2.6f
static float g_deploy_t = 0.0f;
// ---- story cutscenes (subtitles + procedural robot voice) ------------------
// a named cast for story cutscenes -- each has a display name + a voice
typedef struct { const char *name; int voice; } cut_char_t;
static const cut_char_t CAST[] = {
  { "WARDEN",  0 },   // the player's commander -- steady, baseline voice
  { "LARK",    2 },   // scout companion -- quick, higher voice
  { "THE MAW", 3 },   // the devourer / antagonist -- deep, slow voice
  { "REYES",   1 },   // human captain ally -- mid voice
};
#define NUM_CAST (int)(sizeof(CAST)/sizeof(CAST[0]))
typedef struct { int who; const char *text; } cut_line_t;
typedef struct { const char *title; cut_line_t lines[6]; int n; } cutscene_t;
static const cutscene_t CUTSCENES[] = {
  { "I. DOOMSDAY", {
    {1,"warden -- something under the ice just moved."},
    {0,"it is old. older than the war. it eats worlds."},
    {0,"the first fight is only for the right to fight again."},
    {1,"then we buy that right. i am with you."},
  }, 4 },
  { "II. DOMINION", {
    {2,"kneel, and i will call it mercy."},
    {0,"you offer order. a cage with the door left open."},
    {0,"so we believe we chose the bars ourselves."},
    {2,"clever ward. it will not save your little ones."},
  }, 4 },
  { "III. DESTINY", {
    {0,"i became what the hour demanded. it cost me."},
    {1,"and now? coordinates -- from somewhere far and waiting."},
    {0,"the wider dark opens its hand. we take it."},
  }, 3 },
  { "IV. FIRST LIGHT", {
    {3,"reyes, captain, human. and yes -- i see the giants."},
    {0,"beyond the shield the old rules fail, captain."},
    {3,"then teach me the new ones before we are both dead."},
    {1,"wonder and dread, same night. try not to die of either."},
  }, 4 },
  { "V. RECLAMATION", {
    {2,"the corridors breathe. do you feel it, ward?"},
    {0,"metal here remembers being alive. and resents us."},
    {3,"whatever we salvaged from the dark -- it is salvaging us."},
    {0,"then we cut it out. all of it. move."},
  }, 4 },
  { "VI. LAST DAWN", {
    {0,"everything scattered across six worlds gathers here."},
    {2,"i have waited an age for one of you worth ending."},
    {1,"be worth ending, warden. then do not end."},
    {0,"the last dawn is ours to keep. hold the line."},
  }, 4 },
};
#define NUM_CUTSCENES (int)(sizeof(CUTSCENES)/sizeof(CUTSCENES[0]))
static int   g_cut_active=0, g_cut_id=0, g_cut_idx=0, g_cut_voiced=0;
static float g_cut_t=0.0f;
static int   g_cut_after_deploy=0;   // start the match when the scene ends
static void cutscene_start(int id, int then_deploy){
  if(id<0||id>=NUM_CUTSCENES) return;
  g_cut_active=1; g_cut_id=id; g_cut_idx=0; g_cut_t=0; g_cut_voiced=0; g_cut_after_deploy=then_deploy;
}

static void pose_body(ex_model_t *parts[EX_ROBOT_PARTS], const vec3 origin,
                      int vehicle, ex_body_kind kind, float size_scale,
                      float phase, float speed, int element, float yaw_rad,
                      const ex_build_params_t *bp, int sig)
{
  ex_part_pose_t poses[EX_ROBOT_PARTS];
  ex_robot_layout_ex(poses, vehicle, kind, bp, size_scale, phase, speed, element);
  // signature BACK component: place it for this body's signature (robot mode),
  // or hide it (sig<0) for bodies that carry none. Keeps the layout's accent tint.
  if (!vehicle) {
    if (sig < 0) {
      poses[EX_PART_BACK].sx = poses[EX_PART_BACK].sy = poses[EX_PART_BACK].sz = 0.0f;
    } else {
      ex_part_pose_t sr;
      ex_signature_pose(&sr, NULL, (ex_signature_t)sig, EX_VEH_CAR, size_scale, phase, speed);
      for (int c=0;c<3;c++) sr.color[c] = poses[EX_PART_BACK].color[c];
      poses[EX_PART_BACK] = sr;
    }
  }
  float cy = cosf(yaw_rad), sy = sinf(yaw_rad);
  float yaw_deg = yaw_rad * 57.2957795f;
  for (int i = 0; i < EX_ROBOT_PARTS; i++) {
    float ox = poses[i].offset[0], oz = poses[i].offset[2];
    float rx = ox * cy - oz * sy;
    float rz = ox * sy + oz * cy;
    parts[i]->position[0] = origin[0] + rx;
    parts[i]->position[1] = origin[1] + poses[i].offset[1];
    parts[i]->position[2] = origin[2] + rz;
    parts[i]->rotation[1] = yaw_deg;
    // per-axis scale; swap x/z extents when facing sideways so limbs stay right
    float sxw = poses[i].sx, szw = poses[i].sz;
    parts[i]->scale3[0] = fabsf(cy)*sxw + fabsf(sy)*szw;
    parts[i]->scale3[1] = poses[i].sy;
    parts[i]->scale3[2] = fabsf(cy)*szw + fabsf(sy)*sxw;
    parts[i]->tint[0]=poses[i].color[0]/255.0f;
    parts[i]->tint[1]=poses[i].color[1]/255.0f;
    parts[i]->tint[2]=poses[i].color[2]/255.0f;
  }
}

static void hide_body(ex_model_t *parts[EX_ROBOT_PARTS])
{
  for (int i = 0; i < EX_ROBOT_PARTS; i++)
    parts[i]->position[1] = -1000.0f;
}

// build the match from the builder loadout
static void load_asset_aabbs(void)
{
  g_aabb_n = 0;
  if (!PHYSFS_exists("data/asset_aabb.txt")) return;
  PHYSFS_File *f = PHYSFS_openRead("data/asset_aabb.txt"); if(!f) return;
  PHYSFS_sint64 sz = PHYSFS_fileLength(f);
  if (sz<=0 || sz>65536){ PHYSFS_close(f); return; }
  char *buf = malloc(sz+1); PHYSFS_readBytes(f, buf, sz); buf[sz]='\0'; PHYSFS_close(f);
  char *line = strtok(buf, "\n");
  while (line && g_aabb_n < MAX_ASSET_AABB){
    char nm[24]; float c[3], h[3];
    if (sscanf(line,"%23s %f %f %f %f %f %f",nm,&c[0],&c[1],&c[2],&h[0],&h[1],&h[2])==7){
      int k=g_aabb_n++;
      snprintf(g_aabb_name[k],sizeof(g_aabb_name[k]),"%s",nm);
      for(int j=0;j<3;j++){ g_aabb_cen[k][j]=c[j]; g_aabb_half[k][j]=h[j]; }
    }
    line = strtok(NULL, "\n");
  }
  free(buf);
}
// look up a mesh's local AABB by name; returns 1 on hit
static int asset_aabb(const char *name, float cen[3], float half[3])
{
  for (int i=0;i<g_aabb_n;i++) if (!strcmp(g_aabb_name[i], name)){
    for(int j=0;j<3;j++){ cen[j]=g_aabb_cen[i][j]; half[j]=g_aabb_half[i][j]; }
    return 1;
  }
  return 0;
}
// register a world collider for a placed model: local AABB (by mesh name) scaled
// by a uniform scale about the model origin, offset to the model position.
static void auto_collider(const char *mesh, const float pos[3], float scale)
{
  if (g_auto_col_n >= MAX_AUTO_COL) return;
  float c[3], h[3];
  if (!asset_aabb(mesh, c, h)) { c[0]=c[1]=c[2]=0; h[0]=h[1]=h[2]=1.0f; }  // fallback unit box
  float *d = g_auto_col[g_auto_col_n++];
  d[0]=pos[0]+c[0]*scale; d[1]=pos[1]+c[1]*scale; d[2]=pos[2]+c[2]*scale;
  d[3]=h[0]*scale; d[4]=h[1]*scale; d[5]=h[2]*scale;
}
static void load_map_colliders(const char *mapid)
{
  g_map_col_n = 0;
  char path[128]; snprintf(path,sizeof(path),"data/%s.col", mapid);
  if (!PHYSFS_exists(path)) return;
  PHYSFS_File *f = PHYSFS_openRead(path); if(!f) return;
  PHYSFS_sint64 sz = PHYSFS_fileLength(f);
  if (sz<=0 || sz>65536){ PHYSFS_close(f); return; }
  char *buf = malloc(sz+1); PHYSFS_readBytes(f, buf, sz); buf[sz]='\0'; PHYSFS_close(f);
  char *line = strtok(buf, "\n");
  while (line && g_map_col_n < MAX_MAP_COL){
    float c[6];
    if (sscanf(line,"%f %f %f %f %f %f",&c[0],&c[1],&c[2],&c[3],&c[4],&c[5])==6){
      float *d = g_map_col[g_map_col_n++];
      for (int k=0;k<6;k++) d[k]=c[k]*THEME_VIS_SCALE;   // match the scaled visual
    }
    line = strtok(NULL, "\n");
  }
  free(buf);
}
static void start_match(void)
{
  const ex_loadout_t *lo = &g_menu.loadout;
  if (g_menu.layout_dirty || (g_keys_layout != (int)lo->layout && !g_keys_loaded_from_config)) {
    ex_keybind_preset(&g_keys, lo->layout); g_keys_layout = (int)lo->layout; g_menu.layout_dirty = 0;
  }
  if (g_menu.layout_dirty) g_keys_loaded_from_config = 0;  // explicit change wins
  { const ex_chassis_info_t *ch = ex_chassis_info((ex_chassis_t)lo->chassis);
    g_build.height=(lo->height/100.0f)*ch->h_mul; g_build.bulk=(lo->bulk/100.0f)*ch->bulk_mul; g_build.head=(lo->head/100.0f)*ch->head_mul;
    g_build.shoulders=(lo->shoulders/100.0f)*ch->shoulder_mul; g_build.arm=(lo->arm/100.0f)*ch->arm_mul; g_build.leg=(lo->leg/100.0f)*ch->leg_mul;
    g_build.accent_element=(int)lo->accent; g_build.paint=lo->paint; g_build.finish=lo->finish; g_build.alt_mode=lo->vehicle % EX_VEH_COUNT; }
  g_vehicle = (ex_vehicle_type)(lo->vehicle % EX_VEH_COUNT);
  g_player_sig = player_signature();   // builder choice overrides the class default
  g_sig_stats  = ex_signature_stats(g_player_sig);   // gameplay profile for this signature
  g_sig_combat = ex_signature_combat(g_player_sig);  // on-hit combat identity
  snprintf(g_shot_msg, sizeof(g_shot_msg), "SIGNATURE: %s", ex_signature_name(g_player_sig));
  g_shot_timer = 3.0f;
  ex_loadout_init(&g_abilities);
  for (int i=0;i<3;i++) if (lo->abil[i] != EX_ABIL_NONE) ex_loadout_add(&g_abilities, (ex_ability_id)lo->abil[i]);
  if (g_abilities.count==0) ex_loadout_add(&g_abilities, EX_ABIL_DASH);
  ex_weapon_mods_init(&g_wmods);
  if (lo->wmod  != EX_MOD_NONE) ex_weapon_mods_add(&g_wmods, (ex_weapon_mod_id)lo->wmod);
  if (lo->wmod2 != EX_MOD_NONE) ex_weapon_mods_add(&g_wmods, (ex_weapon_mod_id)lo->wmod2);
  if (lo->wmod3 != EX_MOD_NONE) ex_weapon_mods_add(&g_wmods, (ex_weapon_mod_id)lo->wmod3);
  g_watch = ex_watch_new(lo->element);
  g_form = (ex_watch_form_t)lo->element; if (g_form >= EX_FORM_COUNT) g_form = EX_FORM_ICE;
  memset(&g_starfire, 0, sizeof(g_starfire));
  memset(&g_gravleap, 0, sizeof(g_gravleap));
  g_form_cd = 0.0f;

  ex_xform_profile_t robot   = { .collider_half_extents={1,1,1}, .move_speed=6.0f,  .turn_rate=3.0f, .camera_fov=70, .camera_height_offset=1.8f };
  ex_xform_profile_t vehicle = { .collider_half_extents={2,1,3}, .move_speed=18.0f, .turn_rate=1.5f, .camera_fov=85, .camera_height_offset=1.0f };
  if (g_avatar_xform) ex_xform_free(g_avatar_xform);
  g_avatar_xform = ex_xform_new(NULL, NULL, robot, vehicle, lo->size_class, 1, g_bus);

  // class identity: durability + mobility come from the class multipliers. These
  // were previously never applied -- g_player_max_hp and g_class_speed_mul sat at
  // their defaults, so every class had identical HP and speed.
  { const ex_class_info_t *ci = ex_class_info((ex_class_t)lo->player_class);
    g_player_max_hp   = 100.0f * ci->hp_mul;     // Warden ~145, Pyre ~70
    g_class_speed_mul = ci->speed_mul; }
  g_player_hp = g_player_max_hp;                  // start at full health
  ex_photon_reset(&g_photon);
  ex_matrix_reset(&g_matrix);
  int n_grunt = 2, n_corr = 1, n_hijack = 0;
  g_control_target = 0.0f;
  g_protect_mission = 0;
  ex_win_mode_t wmode = EX_WIN_ELIMINATE;
  float survive_s = 0.0f;
  if (g_menu.in_campaign) {
    ex_mission_t *ms = &g_campaign.missions[g_menu.mission_index];
    n_grunt = ms->grunts; n_corr = ms->corrupted;
    n_hijack = (g_menu.mission_index >= 2) ? 2 + g_menu.mission_index : 0; // Hijacks appear later
    if (ms->objective == EX_OBJ_SURVIVE) { wmode = EX_WIN_SURVIVE; survive_s = ms->survive_seconds; }
    if (ms->objective == EX_OBJ_CONTROL) g_control_target = 60.0f;   // hold pads to score 60
    if (ms->objective == EX_OBJ_PROTECT) g_protect_mission = 1;
  } else if (g_menu.mode == EX_MODE_SURVIVAL) { wmode = EX_WIN_SURVIVE; survive_s = 60.0f; n_grunt=3; n_corr=2; n_hijack=4; }
  g_wave_mode = !g_menu.in_campaign;   // skirmish + survival = wave game loop
  if (g_wave_mode) {
    // endless in survival, 8 waves in skirmish
    ex_waves_init(&g_waves, (g_menu.mode==EX_MODE_SURVIVAL)?0:8);
    wmode = EX_WIN_ELIMINATE; survive_s = 0.0f;
    ex_encounter_init(&g_enc, wmode, survive_s);   // waves fill it
    g_prev_alive = 0;
  } else {
    ex_encounter_init(&g_enc, wmode, survive_s);
    ex_encounter_spawn_makeup(&g_enc, n_grunt, n_corr, n_hijack, -360,-360,360,360, 20260702u ^ (g_menu.mission_index*7+1));
    g_prev_alive = ex_encounter_alive(&g_enc);
  }
  // final campaign mission: the apex Corrupted boss stalks the centre
  int is_boss_mission = g_menu.in_campaign && (g_menu.mission_index == g_campaign.count - 1);
  if (is_boss_mission) ex_encounter_boss(&g_enc, 0.0f, 0.0f, 1600.0f);
  for (int i = 0; i < EX_MAX_ENEMIES; i++) g_enemy_phase[i] = 0.0f;
  // OUTER-RIM ALLY: Forgeheart joins from the Outer Rim onward (campaign, mission 2+).
  g_ally_present = (g_menu.in_campaign && g_menu.mission_index >= 1) ? 1 : 0;
  if (g_ally_present) {
    vec3 ap = { -6.0f, 0.0f, -6.0f };
    ex_ally_reset(&g_ally, ap, 260.0f);
    g_ally_phase = 0.0f; g_ally_yaw = 0.0f;
  } else hide_body(g_ally_parts);
  // procedural cover: flow-first graph -> best-of-N -> collidable blueprint
  { ex_map_graph_t bg;
    uint32_t bseed = ex_mapgraph_select_best(0xC0FFEE ^ (g_menu.mission_index*101+7), 160, 1300.0f, &bg);
    ex_mapgraph_to_blueprint(&bg, &g_blueprint, bseed);
    g_deco_col_n = 0;
    for (int i=0;i<NUM_PLANETS;i++) if(g_planets[i]) g_planets[i]->position[1]=-100000.0f;
    // human-scale cover clutter: drop the per-match colliders back to the fixed
    // arena features, then scatter small crates/barriers for the co-playing
    // humans (bots step over these; humans fight around them).
    for(int i=0;i<NUM_CLUTTER;i++) g_clutter_realy[i]=-1000.0f;
    g_auto_col_n = g_feature_col_base;
    { unsigned cr = 0x0C1A ^ (unsigned)(g_menu.mission_index*131 + g_menu.map_index*17 + 5);
      float reach = 1250.0f;
      for (int i=0;i<NUM_CLUTTER;i++){
        cr = cr*1664525u + 1013904223u; float ax = (((cr>>9)&0x3FF)/1023.0f*2.0f-1.0f)*reach;
        cr = cr*1664525u + 1013904223u; float az = (((cr>>9)&0x3FF)/1023.0f*2.0f-1.0f)*reach;
        cr = cr*1664525u + 1013904223u; float sz = 1.6f + ((cr>>11)&0xFF)/255.0f*1.8f;   // 1.6..3.4 (human-tall)
        cr = cr*1664525u + 1013904223u; int tall = ((cr>>7)&1);                            // some barriers, some crates
        ex_model_t *cm = g_clutter[i]; if(!cm) continue;
        cm->position[0]=ax; cm->position[2]=az;
        cm->scale3[0]=sz; cm->scale3[1]=tall? sz*1.5f : sz*0.7f; cm->scale3[2]=sz;
        cm->position[1]=cm->scale3[1]; g_clutter_realy[i]=cm->scale3[1];    // sit on the floor
        cm->tint[0]=0.42f; cm->tint[1]=0.45f; cm->tint[2]=0.52f;   // neutral crate grey
        float cp[3]={ax, cm->position[1], az};
        auto_collider("box", cp, sz);     // collidable: humans take cover, bots block too
      }
    }
    spawn_humans();   // co-play: scatter human NPCs across the map
    // objective spawn pads: three marked capture points (center-forward + flanks)
    { float rc[NUM_CONSOLES][2] = { {-200,-150}, {210,140}, {30,-260} };
      for (int i=0;i<NUM_CONSOLES;i++){ g_console_pos[i][0]=rc[i][0]; g_console_pos[i][1]=2.0f; g_console_pos[i][2]=rc[i][1];
        g_consoles[i]->position[0]=rc[i][0]; g_consoles[i]->position[1]=2.0f; g_consoles[i]->position[2]=rc[i][1];
        g_consoles[i]->scale3[0]=2.5f; g_consoles[i]->scale3[1]=1.6f; g_consoles[i]->scale3[2]=1.0f; } }
    { float op[NUM_OBJ_PADS][2] = { {0,330}, {-330,-180}, {330,-180} };
      for (int i=0;i<NUM_OBJ_PADS;i++){
        g_obj_pos[i][0]=op[i][0]; g_obj_pos[i][1]=0.2f; g_obj_pos[i][2]=op[i][1];
        g_obj_pads[i]->position[0]=op[i][0]; g_obj_pads[i]->position[1]=0.2f; g_obj_pads[i]->position[2]=op[i][1];
        g_obj_pads[i]->scale3[0]=5.0f; g_obj_pads[i]->scale3[1]=0.15f; g_obj_pads[i]->scale3[2]=5.0f;
      }
    }
    // ---- defensive turrets: slow-tracking lane guards (dodge them at speed) ----
    // liquid pools -- shimmering surfaces themed by map (lava/water/coolant/energon)
    { const char *lm2 = g_menu.in_campaign ? g_campaign.missions[g_menu.mission_index].map
                                           : g_theme_ids[g_menu.map_index % NUM_THEMES];
      if (!strcmp(lm2,"emberpit")) { g_liquid_col[0]=1.0f;g_liquid_col[1]=0.35f;g_liquid_col[2]=0.1f; }      // lava
      else if (!strcmp(lm2,"reclaim")||!strcmp(lm2,"frostline")) { g_liquid_col[0]=0.3f;g_liquid_col[1]=0.6f;g_liquid_col[2]=1.0f; } // water
      else if (!strcmp(lm2,"foundry")||!strcmp(lm2,"mining")) { g_liquid_col[0]=0.3f;g_liquid_col[1]=1.0f;g_liquid_col[2]=0.8f; }    // coolant
      else if (!strcmp(lm2,"shield_edge")||!strcmp(lm2,"vault")) { g_liquid_col[0]=1.0f;g_liquid_col[1]=0.4f;g_liquid_col[2]=0.9f; } // energon
      else { g_liquid_col[0]=0.35f;g_liquid_col[1]=0.5f;g_liquid_col[2]=0.9f; }
      g_liquid_n = NUM_LIQUID;
      unsigned qs = 0x11D ^ (g_menu.map_index*53+9);
      for (int i=0;i<NUM_LIQUID;i++){
        qs=qs*1664525u+1013904223u; float a=(qs%628)/100.0f;
        qs=qs*1664525u+1013904223u; float r=30.0f+(qs%160);
        qs=qs*1664525u+1013904223u; float sc=5.0f+(qs%80)/10.0f;
        g_liquid[i]->position[0]=cosf(a)*r; g_liquid[i]->position[1]=0.15f; g_liquid[i]->position[2]=sinf(a)*r;
        g_liquid[i]->scale3[0]=sc; g_liquid[i]->scale3[1]=0.12f; g_liquid[i]->scale3[2]=sc;
      }
    }
    g_deft_n = 0;
    { const char *tm = g_menu.in_campaign ? g_campaign.missions[g_menu.mission_index].map
                                          : g_theme_ids[g_menu.map_index % NUM_THEMES];
      if (!strcmp(tm,"reclaim")) {
        // guard the three lanes at staggered distances
        float lanes[3]={-45,0,45};
        for (int L=0;L<3 && g_deft_n<NUM_DEF_TURRETS;L++)
          for (int k=0;k<3 && g_deft_n<NUM_DEF_TURRETS;k++){
            def_turret_t *t=&g_deft[g_deft_n++];
            t->pos[0]=lanes[L]; t->pos[1]=0; t->pos[2]=-120.0f + k*120.0f;
            t->angle=0; t->cd=1.0f; t->active=1;
          }
      } else {
        // a few guard posts ringing the central megastructure
        for (int k=0;k<4 && g_deft_n<NUM_DEF_TURRETS;k++){
          def_turret_t *t=&g_deft[g_deft_n++]; float a=k*1.5708f;
          t->pos[0]=cosf(a)*30.0f; t->pos[1]=0; t->pos[2]=sinf(a)*30.0f;
          t->angle=a; t->cd=1.0f; t->active=1;
        }
      }
      for (int i=0;i<NUM_DEF_TURRETS;i++)
        g_deft_models[i]->position[1] = (i<g_deft_n)? 0.0f : -100000.0f;
    }
    // scatter glowing launch pads around the world for vertical traversal
    g_launch_n = NUM_LAUNCH;
    { unsigned ls = 0x1A03 ^ (g_menu.map_index*71+5);
      for (int i=0;i<NUM_LAUNCH;i++){
        ls=ls*1664525u+1013904223u; float a=(ls%628)/100.0f;
        ls=ls*1664525u+1013904223u; float r=40.0f+(ls%150);
        g_launch[i][0]=cosf(a)*r; g_launch[i][1]=0.3f; g_launch[i][2]=sinf(a)*r;
        g_launch_models[i]->position[0]=g_launch[i][0]; g_launch_models[i]->position[1]=0.3f; g_launch_models[i]->position[2]=g_launch[i][2];
        g_launch_models[i]->scale3[0]=3.0f; g_launch_models[i]->scale3[1]=0.3f; g_launch_models[i]->scale3[2]=3.0f;
        g_launch_models[i]->tint[0]=0.4f; g_launch_models[i]->tint[1]=1.0f; g_launch_models[i]->tint[2]=0.9f;
      }
    }
    // put the player at the graph's spawn node (real flow-first start point)
    e->position[0] = g_blueprint.spawn[0][0];
    e->position[2] = g_blueprint.spawn[0][2];
    e->position[1] = 2.0f;
    // start each match with a clean in-match camera facing the arena centre,
    // so it never inherits a stale aim from a previous match or a menu.
    g_cam_state[CAM_GAME].inited = 0;
    g_apex_pulse_t=0; g_apex_pulse_type=0; g_apex_telegraph=0; g_player_slow=0;
    for(int i=0;i<NUM_OBJ_PADS;i++){ g_obj_cap[i]=0; g_obj_owner[i]=0; } g_obj_score=0;
    for(int i=0;i<EX_ROBOT_PARTS;i++){ g_part_integ[i]=1.0f; g_part_detached[i]=0; } g_repair_energy=0;
    for(int i=0;i<EX_MAX_ENEMIES;i++){ g_enemy_vel[i][0]=g_enemy_vel[i][1]=g_enemy_vel[i][2]=0; g_enemy_aware[i]=0; g_enemy_startle[i]=0; g_enemy_losetimer[i]=0; g_enemy_mode[i]=0; g_enemy_head[i]=0; g_enemy_xform_pop[i]=0; g_enemy_web[i]=0; g_enemy_synced[i]=0; g_corrupt_gen[i]=-1; g_lieut[i]=-1; g_defected[i]=0; g_raider[i]=-1; g_raider_lunge[i]=0.0f; g_ravager[i]=-1; g_slip[i]=-1; }
    g_truce_on = 0; g_champion_slot = -1; g_champion_kind = 0; g_champion_cd = 0.0f; g_champion_spawned = 0;
    g_corrupt_gen_next = 0;
    for(int i=0;i<NUM_DEBRIS;i++){ g_debris[i].active=0; if(g_debris_models[i]) g_debris_models[i]->position[1]=-1000.0f; }
    for(int i=0;i<NUM_CRATERS;i++){ if(g_craters[i]) g_craters[i]->position[1]=-1000.0f; g_crater_realy[i]=-1000.0f; } g_crater_next=0;
    camera->yaw = atan2f(0.0f - e->position[2], 0.0f - e->position[0]) * 57.2957795f;
    camera->pitch = -6.0f;
    // theme the cover materials by the chosen map (film location flavour)
    const char *mid = g_menu.in_campaign ? g_campaign.missions[g_menu.mission_index].map
                                         : g_theme_ids[g_menu.map_index % NUM_THEMES];
    g_ground_mat = ex_material_for_theme(mid);   // surface material drives friction + sparks
    g_horror_mode = (g_menu.in_campaign && g_menu.mission_index==1); g_dread=0.0f;
    // ---- skirmish variety: per-map gravity + orientation feel + hazards ----
    g_toxic_dot = 0.0f; g_haz_n = 0;
    g_map_gravity = 1.0f;
    if (!g_menu.in_campaign) {
      int mv = g_menu.map_index % 6;
      // gravity variants: heavy world, feather-light world, and normal ones
      if (mv==1) g_map_gravity = 1.8f;        // dense core -- heavy, grounded
      else if (mv==2) g_map_gravity = 0.45f;  // low-g -- long floaty arcs
      else if (mv==4) g_map_gravity = 0.7f;   // light
      // TOXIC SLUDGE maps: pools of green DOT muck
      if (mv==3 || mv==5) {
        float sp[4][2] = { {-140,90}, {160,-120}, {60,200}, {-200,-60} };
        for (int i=0;i<4 && g_haz_n<NUM_HAZARDS;i++){ int h=g_haz_n++;
          g_haz_type[h]=HAZ_TOXIC; g_haz_pos[h][0]=sp[i][0]; g_haz_pos[h][1]=0.15f; g_haz_pos[h][2]=sp[i][1];
          g_haz_rad[h]=28.0f+i*6.0f; }
      }
      // THRUSTER CHAMBER maps: gale zones that blast wind + ash across the arena
      if (mv==0 || mv==2) {
        float wp[2][2] = { {-260,0}, {240,120} };
        float wd[2][2] = { {1,0.05f}, {-0.7f,0.7f} };
        for (int i=0;i<2 && g_haz_n<NUM_HAZARDS;i++){ int h=g_haz_n++;
          g_haz_type[h]=HAZ_WIND; g_haz_pos[h][0]=wp[i][0]; g_haz_pos[h][1]=2.0f; g_haz_pos[h][2]=wp[i][1];
          g_haz_rad[h]=120.0f; g_haz_wind[h][0]=wd[i][0]; g_haz_wind[h][1]=0; g_haz_wind[h][2]=wd[i][1]; }
      }
    }
    // position + colour the hazard decals
    for (int i=0;i<NUM_HAZARDS;i++){ ex_model_t *hm=g_haz_models[i]; if(!hm) continue;
      if (i<g_haz_n){ hm->position[0]=g_haz_pos[i][0]; hm->position[1]=g_haz_pos[i][1]; hm->position[2]=g_haz_pos[i][2];
        float r=g_haz_rad[i]; hm->scale3[0]=r; hm->scale3[1]=0.1f; hm->scale3[2]=r;
        if (g_haz_type[i]==HAZ_TOXIC){ hm->tint[0]=0.25f; hm->tint[1]=0.95f; hm->tint[2]=0.20f; }
        else { hm->tint[0]=0.5f; hm->tint[1]=0.45f; hm->tint[2]=0.4f; hm->scale3[1]=0.05f; } }
      else hm->position[1]=-1000.0f; }
    int theme_mat = 0; // wall
    if (!strcmp(mid,"foundry")||!strcmp(mid,"mining")||!strcmp(mid,"shield_edge")) theme_mat=2; // plate
    else if (!strcmp(mid,"wastes")||!strcmp(mid,"emberpit")) theme_mat=1; // rock
    for (int i = 0; i < g_blueprint.n_vols; i++)
      if (g_blueprint.vols[i].material==0) g_blueprint.vols[i].material = theme_mat;
    for (int i = 0; i < EX_MAP_MAX_VOLS; i++) {
      // hide all three pool slots, then show the one matching the role
      g_vol_models[i]->position[1] = -1000.0f;
      g_rock_models[i]->position[1] = -1000.0f;
      g_spire_models[i]->position[1] = -1000.0f;
      if (i < g_blueprint.n_vols) {
        ex_map_vol_t *v = &g_blueprint.vols[i];
        // material tint
        float tr,tg,tb;
        if (v->material==2){ tr=0.45f; tg=0.55f; tb=0.75f; }
        else if (v->material==1){ tr=0.62f; tg=0.5f; tb=0.38f; }
        else { tr=0.5f; tg=0.52f; tb=0.56f; }
        // pick the mesh: smooth rock for blocks/cover, tapered spire for
        // vantage, box for walls/platforms/channels.
        ex_model_t *m;
        if (v->role==EX_VOL_EXPOSE) m = g_spire_models[i];
        else if (v->role==EX_VOL_BLOCK || v->role==EX_VOL_LOWCOVER || v->role==EX_VOL_HIGHCOVER) m = g_rock_models[i];
        else m = g_vol_models[i];
        m->position[0]=v->center[0]; m->position[1]=v->center[1]; m->position[2]=v->center[2];
        if (m==g_vol_models[i]) {
          m->scale3[0]=v->half[0]; m->scale3[1]=v->half[1]; m->scale3[2]=v->half[2];
        } else {
          // rock/spire meshes are unit-ish; scale to the volume extents
          m->scale3[0]=v->half[0]; m->scale3[1]=v->half[1]; m->scale3[2]=v->half[2];
          // The rock mesh is centre-origin, so it must stay CENTERED on the
          // collider -- previously it was dropped by half its height, which left
          // the collider top (where you land) floating a half-height ABOVE the
          // visible rock. Only the tall spire (vantage) keeps the ground offset.
          if (m==g_spire_models[i]) m->position[1]=v->center[1]-v->half[1];
        }
        m->tint[0]=tr; m->tint[1]=tg; m->tint[2]=tb;
      }
    }
    // --- FSM structures get procedural greeble subcomponents (surface detail) ---
    { ex_detail_part_t dparts[EX_MAP_MAX_VOLS]; int ndp=0;
      for (int i=0;i<g_blueprint.n_vols && ndp<EX_MAP_MAX_VOLS;i++){
        ex_map_vol_t *v=&g_blueprint.vols[i];
        // detail only the large structures (towers, platforms, megastructure)
        if (v->half[0] < 3.0f && v->half[1] < 3.0f) continue;
        ex_detail_part_t *dp=&dparts[ndp++];
        dp->offset[0]=v->center[0]; dp->offset[1]=v->center[1]; dp->offset[2]=v->center[2];
        dp->sx=v->half[0]; dp->sy=v->half[1]; dp->sz=v->half[2];
        uint8_t mc[3]; if(v->material==2){mc[0]=115;mc[1]=140;mc[2]=190;}else if(v->material==1){mc[0]=160;mc[1]=128;mc[2]=97;}else{mc[0]=128;mc[1]=133;mc[2]=143;}
        dp->color[0]=mc[0]; dp->color[1]=mc[1]; dp->color[2]=mc[2];
      }
      static ex_greeble_t mg[MAP_DETAIL];
      uint8_t macc[3]={200,180,90};
      int ng = ex_detail_generate(mg, MAP_DETAIL, dparts, ndp, 0x5EED ^ (g_menu.map_index*13+1), MAP_DETAIL, macc);
      for (int i=0;i<ng;i++){ ex_model_t *gm=g_map_detail[i];
        gm->position[0]=mg[i].offset[0]; gm->position[1]=mg[i].offset[1]; gm->position[2]=mg[i].offset[2];
        gm->scale3[0]=mg[i].sx; gm->scale3[1]=mg[i].sy; gm->scale3[2]=mg[i].sz;
        gm->tint[0]=mg[i].color[0]/255.0f; gm->tint[1]=mg[i].color[1]/255.0f; gm->tint[2]=mg[i].color[2]/255.0f; }
      for (int i=ng;i<MAP_DETAIL;i++) g_map_detail[i]->position[1]=-100000.0f;
    }
    // --- periphery decoration: themed smooth props ringing the arena ---
    const char *dmid = g_menu.in_campaign ? g_campaign.missions[g_menu.mission_index].map
                                          : g_theme_ids[g_menu.map_index % NUM_THEMES];
    float rt[3]={0.55f,0.5f,0.45f}, tt[3]={0.6f,0.62f,0.7f};   // rock tint, tall tint
    if (!strcmp(dmid,"frostline")||!strcmp(dmid,"vault")) { rt[0]=0.6f;rt[1]=0.72f;rt[2]=0.85f; tt[0]=0.7f;tt[1]=0.85f;tt[2]=1.0f; }
    else if (!strcmp(dmid,"emberpit")) { rt[0]=0.28f;rt[1]=0.22f;rt[2]=0.22f; tt[0]=0.8f;tt[1]=0.35f;tt[2]=0.2f; }
    else if (!strcmp(dmid,"cybertron")) { rt[0]=0.7f;rt[1]=0.6f;rt[2]=0.3f; tt[0]=0.85f;tt[1]=0.72f;tt[2]=0.4f; }
    else if (!strcmp(dmid,"outer_rim")) { rt[0]=0.4f;rt[1]=0.4f;rt[2]=0.44f; tt[0]=0.5f;tt[1]=0.55f;tt[2]=0.65f; }
    else if (!strcmp(dmid,"wastes")||!strcmp(dmid,"mining")) { rt[0]=0.55f;rt[1]=0.42f;rt[2]=0.3f; tt[0]=0.6f;tt[1]=0.5f;tt[2]=0.38f; }
    unsigned ds = 0xDEC0 ^ (unsigned)g_active_sky*131 ^ (g_menu.map_index*17+3);
    for (int i=0;i<DECO_ROCK;i++){
      ds=ds*1664525u+1013904223u; float a=(ds%628)/100.0f;
      ds=ds*1664525u+1013904223u; float r=90.0f+(ds%130);
      ds=ds*1664525u+1013904223u; float sc=6.0f+(ds%120)/10.0f;
      g_deco_rock[i]->position[0]=cosf(a)*r; g_deco_rock[i]->position[2]=sinf(a)*r;
      g_deco_rock[i]->position[1]=sc*0.5f; g_deco_rock[i]->scale=sc;
      g_deco_rock[i]->tint[0]=rt[0]; g_deco_rock[i]->tint[1]=rt[1]; g_deco_rock[i]->tint[2]=rt[2];
      if (g_deco_col_n<MAX_DECO_COL){ float *c=g_deco_col[g_deco_col_n++];
        c[0]=g_deco_rock[i]->position[0]; c[1]=sc*0.5f; c[2]=g_deco_rock[i]->position[2];
        c[3]=sc*0.7f; c[4]=sc*0.5f; c[5]=sc*0.7f; }
    }
    for (int i=0;i<DECO_TALL;i++){
      ds=ds*1664525u+1013904223u; float a=(ds%628)/100.0f;
      ds=ds*1664525u+1013904223u; float r=110.0f+(ds%120);
      ds=ds*1664525u+1013904223u; float sc=10.0f+(ds%120)/10.0f;
      g_deco_tall[i]->position[0]=cosf(a)*r; g_deco_tall[i]->position[2]=sinf(a)*r;
      g_deco_tall[i]->position[1]=0.0f; g_deco_tall[i]->scale3[0]=sc*0.4f; g_deco_tall[i]->scale3[1]=sc*1.8f; g_deco_tall[i]->scale3[2]=sc*0.4f;
      g_deco_tall[i]->tint[0]=tt[0]; g_deco_tall[i]->tint[1]=tt[1]; g_deco_tall[i]->tint[2]=tt[2];
      if (g_deco_col_n<MAX_DECO_COL){ float *c=g_deco_col[g_deco_col_n++];
        c[0]=g_deco_tall[i]->position[0]; c[1]=sc*1.8f; c[2]=g_deco_tall[i]->position[2];
        c[3]=sc*0.5f; c[4]=sc*1.8f; c[5]=sc*0.5f; }
    }
    for (int i=0;i<DECO_ARCH;i++){
      float a = (i==0)? 0.7f : 3.85f;   // two opposite edges
      float r = 180.0f;
      g_deco_arch[i]->position[0]=cosf(a)*r; g_deco_arch[i]->position[2]=sinf(a)*r;
      g_deco_arch[i]->position[1]=0.0f; g_deco_arch[i]->scale=7.0f;
      g_deco_arch[i]->rotation[1]=(-a)*57.2957795f + 90.0f;
      g_deco_arch[i]->tint[0]=tt[0]*0.9f; g_deco_arch[i]->tint[1]=tt[1]*0.9f; g_deco_arch[i]->tint[2]=tt[2]*0.9f;
    }
    for (int i=0;i<DECO_TOWER;i++){
      float a = 1.9f + i*2.1f; float r = 200.0f + i*12.0f;
      g_deco_tower[i]->position[0]=cosf(a)*r; g_deco_tower[i]->position[2]=sinf(a)*r;
      g_deco_tower[i]->position[1]=0.0f; g_deco_tower[i]->scale = 7.0f + i*1.5f;
      g_deco_tower[i]->tint[0]=rt[0]*1.1f; g_deco_tower[i]->tint[1]=rt[1]*1.1f; g_deco_tower[i]->tint[2]=rt[2]*1.1f;
      if (g_deco_col_n<MAX_DECO_COL){ float *c=g_deco_col[g_deco_col_n++]; float sc=7.0f+i*1.5f;
        c[0]=g_deco_tower[i]->position[0]; c[1]=sc*2.0f; c[2]=g_deco_tower[i]->position[2];
        c[3]=sc*1.4f; c[4]=sc*2.0f; c[5]=sc*1.4f; }
    }
  }
  // scatter pickups around the arena
  ex_pickup_field_init(&g_pickups);
  g_player_armor = 0.0f; g_overcharge_t = 0.0f;
  { ex_pickup_kind kinds[3] = { EX_PICKUP_ENERGON, EX_PICKUP_SHIELD, EX_PICKUP_OVERCHARGE };
    unsigned rs = 0x51ED7 ^ (g_menu.mission_index*13+3);
    for (int i = 0; i < 8; i++) {
      rs = rs*1664525u + 1013904223u; float a = (rs>>8&0xFFFF)/65535.0f*6.2831853f;
      rs = rs*1664525u + 1013904223u; float r = 12.0f + (rs>>8&0xFFFF)/65535.0f*38.0f;
      ex_pickup_add(&g_pickups, kinds[i%3], cosf(a)*r, 0.6f, sinf(a)*r);
    }
    // THE MATRIX OF LEADERSHIP: a rare relic -- always present on the campaign's
    // final (boss) mission, and occasionally elsewhere. Placed off-center, away
    // from the arena middle where the boss stands.
    { int boss_mission = g_menu.in_campaign && (g_menu.mission_index == g_campaign.count - 1);
      rs = rs*1664525u + 1013904223u;
      if (boss_mission || (rs>>8 & 0xFFFF) < 5200)   // ~8% in non-boss matches
        ex_pickup_add(&g_pickups, EX_PICKUP_MATRIX, 0.0f, 0.9f, 58.0f);
    }
    // JUNGLE POWER-UP POCKETS: clusters of pickups out in the hazard/jungle
    // sections between the main engagement areas -- worth the detour.
    float pocket[4][2] = { {-360,-120},{345,150},{-120,375},{180,-375} };
    for (int pk=0; pk<4; pk++)
      for (int j=0; j<3; j++) {
        float ox = pocket[pk][0] + (j-1)*8.0f, oz = pocket[pk][1] + ((j*7)%3-1)*8.0f;
        ex_pickup_add(&g_pickups, (j==1)?EX_PICKUP_OVERCHARGE:kinds[(pk+j)%3], ox, 0.6f, oz);
      }
  }

  // choose the themed map to show (campaign mission id, or skirmish selection)
  const char *mapid = g_menu.in_campaign ? g_campaign.missions[g_menu.mission_index].map
                                         : g_theme_ids[g_menu.map_index % NUM_THEMES];
  for (int i = 0; i < NUM_THEMES; i++) {
    g_theme_models[i]->position[1] = (strcmp(mapid, g_theme_ids[i]) == 0) ? 0.0f : -10000.0f;
    g_theme_models[i]->scale = THEME_VIS_SCALE;   // fill the 480x480 world
  }
  g_cur_mapid = mapid;         // remember territory (themes Blackstar-raider vs lieutenant elites)
  // FAILING CONTAINMENT: the Pluto boundary draws inward. Active in survive
  // missions (closes right as the clock runs out) and on the shield-edge map
  // (the containment boundary itself).
  g_contain_elapsed = 0.0f; g_contain_ring_cd = 0.0f;
  if (wmode == EX_WIN_SURVIVE && survive_s > 1.0f){ g_contain_active = 1; g_contain_close = survive_s; }
  else if (!strcmp(mapid,"shield_edge")){ g_contain_active = 1; g_contain_close = 75.0f; }
  else g_contain_active = 0;
  load_map_colliders(mapid);   // themed-map blockers now collide

  e->position[0] = 0.0f; e->position[1] = 3.0f; e->position[2] = -14.0f;
  ex_sound_master_volume(g_menu.gfx.volume/100.0f);
  camera->sensitivity = g_menu.gfx.sensitivity * 0.01f;
  { const char *lm = g_menu.in_campaign ? g_campaign.missions[g_menu.mission_index].map
                                        : g_theme_ids[g_menu.map_index % NUM_THEMES];
    apply_theme_lighting(lm, g_menu.gfx.brightness/100.0f); }
  g_match_live = 1;
  g_deploy_t = DEPLOY_TIME;   // combat-drop intro before control hands over
  printf("[match] %s map=%s size=%s element=%s keys=%s grunts=%d corrupted=%d\n",
         g_menu.in_campaign ? "campaign" : ex_mode_name(g_menu.mode),
         g_menu.in_campaign ? g_campaign.missions[g_menu.mission_index].map : ex_map_name(g_menu.map_index),
         ex_size_class_name(lo->size_class), ex_element_name(lo->element),
         lo->layout == EX_LAYOUT_DVORAK ? "dvorak" : "qwerty", n_grunt, n_corr);
}

void game_init()
{
  scene = ex_scene_new(0);
  memcpy(scene->gravity, (vec3){0.0f, -0.1f, 0.0f}, sizeof(vec3));
  camera = ex_fps_camera_new(0.0f, 0.0f, 0.0f, 0.1f, 70.0f);

  // the generated 120x120 arena map (see tools/gen_arena.py)
  // shared collision hull (floor + perimeter walls). Kept for collision but
  // moved off-screen so it never renders -- collision is baked at the original
  // y=0 positions, so moving the model doesn't move the collision.
  g_base_hull = ex_iqm_load_model(scene, "data/arena_base.iqm", EX_KEEP_VERTICES);
  g_base_hull->is_shadow = 0;
  ex_scene_add_model(scene, g_base_hull);
  g_base_hull->position[1] = -10000.0f;
  level_model = g_base_hull;
  // themed visual maps (no collision); one shown at a time
  for (int i = 0; i < NUM_THEMES; i++) {
    char path[48]; snprintf(path, sizeof(path), "data/%s.iqm", g_theme_ids[i]);
    g_theme_models[i] = ex_iqm_load_model(scene, path, 0);
    g_theme_models[i]->is_shadow = 0;
    g_theme_models[i]->position[1] = -10000.0f;
    ex_scene_add_model(scene, g_theme_models[i]);
  }
  // skyboxes: large unlit cubes that follow the camera (one shown per theme)
  for (int i = 0; i < NUM_SKY; i++) {
    char sp[48]; snprintf(sp, sizeof(sp), "data/%s.iqm", g_sky_ids[i]);
    g_sky_models[i] = ex_iqm_load_model(scene, sp, 0);
    g_sky_models[i]->is_lit = 0; g_sky_models[i]->is_shadow = 0;
    g_sky_models[i]->scale = 1600.0f;
    g_sky_models[i]->position[1] = -100000.0f;
    ex_scene_add_model(scene, g_sky_models[i]);
  }

  e = ex_entity_new(scene, (vec3){0.5f, 1.0f, 0.5f});
  e->position[1] = 6.0f; e->position[2] = -20.0f;

  sound = ex_sound_load("data/ambient.ogg", EX_SOURCE_STREAMING, EX_SOURCE_LOOPING);
  ex_sound_master_volume(0.5f);
  if (sound) ex_sound_play(sound);          // once, not every frame
  // combat SFX (static WAV one-shots), boosted above the near-silent default gain
  g_sfx_fire  =ex_sound_load("data/sfx/fire.wav",     EX_SOURCE_STATIC,0);
  g_sfx_turret=ex_sound_load("data/sfx/turret.wav",   EX_SOURCE_STATIC,0);
  g_sfx_hit   =ex_sound_load("data/sfx/hit.wav",      EX_SOURCE_STATIC,0);
  g_sfx_expl  =ex_sound_load("data/sfx/explosion.wav",EX_SOURCE_STATIC,0);
  g_sfx_xform =ex_sound_load("data/sfx/transform.wav",EX_SOURCE_STATIC,0);
  g_sfx_launch=ex_sound_load("data/sfx/launch.wav",   EX_SOURCE_STATIC,0);
  g_sfx_pickup=ex_sound_load("data/sfx/pickup.wav",   EX_SOURCE_STATIC,0);
  g_sfx_jump  =ex_sound_load("data/sfx/jump.wav",     EX_SOURCE_STATIC,0);
  ex_sound_gain(g_sfx_fire,0.5f);   ex_sound_gain(g_sfx_turret,0.6f);
  ex_sound_gain(g_sfx_hit,0.6f);    ex_sound_gain(g_sfx_expl,0.8f);
  ex_sound_gain(g_sfx_xform,0.6f);  ex_sound_gain(g_sfx_launch,0.6f);
  ex_sound_gain(g_sfx_pickup,0.6f); ex_sound_gain(g_sfx_jump,0.45f);

  pl = ex_point_light_new((vec3){0,8,0}, (vec3){1.6f,1.35f,1.0f}, 0);  // warm key
  pl->is_shadow = 0;
  ex_scene_add_pointlight(scene, pl);
  g_key_light = pl;
  // dim cool fill so shadows read as moody blue rather than pure black
  ex_point_light_t *fill = ex_point_light_new((vec3){0,18,0}, (vec3){0.25f,0.3f,0.5f}, 0);
  fill->is_shadow = 0;
  ex_scene_add_pointlight(scene, fill);
  g_fill_light = fill;
  // a coloured light that follows the player, tinted by the active element form
  g_form_light = ex_point_light_new((vec3){0,3,0}, (vec3){0.4f,0.6f,1.0f}, 0);
  g_form_light->is_shadow = 0; ex_scene_add_pointlight(scene, g_form_light);
  // a menacing light on the Apex boss when it's alive
  g_boss_light = ex_point_light_new((vec3){0,-1000,0}, (vec3){0.9f,0.3f,1.0f}, 0);
  g_boss_light->is_shadow = 0; ex_scene_add_pointlight(scene, g_boss_light);
  { ex_model_t *sptpl = ex_iqm_load_model(scene, "data/spire.iqm", 0);
    for (int i=0;i<NUM_BOSS_SPIKES;i++){ g_boss_spikes[i]=ex_model_copy(sptpl);
      g_boss_spikes[i]->is_lit=1; g_boss_spikes[i]->is_shadow=0; g_boss_spikes[i]->position[1]=-1000.0f;
      ex_scene_add_model(scene, g_boss_spikes[i]); } }

  // real box mesh (box.iqm), template for robot parts & cover
  g_boxtpl = ex_iqm_load_model(scene, "data/box.iqm", 0);
  g_boxtpl->is_lit = 1; g_boxtpl->is_shadow = 0;
  g_boxtpl->position[1] = -1000.0f;
  ex_scene_add_model(scene, g_boxtpl);
  box = g_boxtpl;
  // smooth character-part templates: rounded joints/head, tapered limbs,
  // chamfered torso -> an articulated robot instead of a stack of boxes.
  g_tpl_head   = ex_iqm_load_model(scene, "data/phead.iqm", 0);
  g_tpl_hand   = ex_iqm_load_model(scene, "data/phand.iqm", 0);
  g_tpl_sphere = ex_iqm_load_model(scene, "data/psphere.iqm", 0);
  g_tpl_bevel  = ex_iqm_load_model(scene, "data/pbevel.iqm", 0);
  g_tpl_chest  = ex_iqm_load_model(scene, "data/pchest.iqm", 0);
  g_tpl_limb   = ex_iqm_load_model(scene, "data/plimb.iqm", 0);
  g_tpl_sphere->is_lit=1; g_tpl_sphere->is_shadow=0; g_tpl_sphere->position[1]=-1000.0f; ex_scene_add_model(scene,g_tpl_sphere);
  g_tpl_bevel->is_lit=1;  g_tpl_bevel->is_shadow=0;  g_tpl_bevel->position[1]=-1000.0f;  ex_scene_add_model(scene,g_tpl_bevel);
  g_tpl_limb->is_lit=1;   g_tpl_limb->is_shadow=0;   g_tpl_limb->position[1]=-1000.0f;   ex_scene_add_model(scene,g_tpl_limb);
  // spine-grown parts from the headless pipeline (tools/gen_spine.py) -- these
  // are the class SIGNATURE components (see signature.c); each class swaps the
  // BACK slot to its own at spawn.
  g_tpl_back   = ex_iqm_load_model(scene, "data/spine_wing_cannon.iqm", 0);   // Interceptor
  g_tpl_cannon = ex_iqm_load_model(scene, "data/spine_heavy_cannon.iqm", 0);  // Vanguard
  g_tpl_crest  = ex_iqm_load_model(scene, "data/spine_crest.iqm", 0);         // Titan
  g_tpl_cable  = ex_iqm_load_model(scene, "data/spine_cable.iqm", 0);         // Specialist
  g_tpl_muscle = ex_iqm_load_model(scene, "data/spine_muscle.iqm", 0);
  g_tpl_harpoon = ex_iqm_load_model(scene, "data/spine_harpoon.iqm", 0);  // Reaver
  g_tpl_halo    = ex_iqm_load_model(scene, "data/spine_halo.iqm", 0);     // Warden
  g_tpl_solar   = ex_iqm_load_model(scene, "data/spine_solar.iqm", 0);    // Pyre
  g_tpl_aegis   = ex_iqm_load_model(scene, "data/spine_aegis.iqm", 0);     // Prime
  g_tpl_recon   = ex_iqm_load_model(scene, "data/spine_recon.iqm", 0);     // Scout
  ex_model_t *g_sig_tpls[10] = { g_tpl_back, g_tpl_cannon, g_tpl_crest, g_tpl_cable, g_tpl_muscle,
                                g_tpl_harpoon, g_tpl_halo, g_tpl_solar, g_tpl_aegis, g_tpl_recon };
  for (int si=0; si<10; si++){ ex_model_t *m=g_sig_tpls[si]; if(m){ m->is_lit=1; m->is_shadow=0; m->position[1]=-1000.0f; ex_scene_add_model(scene,m); } }

  font = ex_font_load("data/fonts/OpenSans-Regular.ttf", "abcdefghijklmnopqrstuvwxyzHW!_");
  ex_vga_init();

  g_bus    = ex_event_bus_new();
  g_chunks = ex_chunk_world_new();
  ex_event_subscribe(g_bus, "wall_breached", on_map_event, NULL);

  // procedural cover across the big map (matches the 120x120 arena)
  ex_arena_generate(&g_arena, g_chunks, -50.0f, -50.0f, 50.0f, 50.0f, 0.0f, 6, 20260702u);
  printf("[arena] placed %d cover features\n", g_arena.count);
  for (int i = 0; i < g_arena.count; i++) {
    ex_model_t *fm = ex_model_copy(box);
    fm->is_lit = 1; fm->is_shadow = 0;
    fm->scale = g_arena.features[i].half_extents[1];
    memcpy(fm->position, g_arena.features[i].position, sizeof(vec3));
    fm->position[1] += g_arena.features[i].half_extents[1];
    ex_scene_add_model(scene, fm);
    g_feature_models[i] = fm;
    // arena cover features now collide (they had no player collision before):
    // box mesh scaled uniformly -> a matching auto AABB from the manifest.
    { float fp[3]={fm->position[0], fm->position[1], fm->position[2]};
      auto_collider("box", fp, g_arena.features[i].half_extents[1]); }
  }
  g_feature_col_base = g_auto_col_n;   // clutter colliders re-scatter above this each match

  // human-scale cover clutter: small crates/barriers the co-playing HUMANS use
  // for cover while the titan bots simply step over them. Scattered per match.
  for (int i=0;i<NUM_CLUTTER;i++){
    g_clutter[i] = ex_model_copy(g_boxtpl);
    g_clutter[i]->is_lit = 1; g_clutter[i]->is_shadow = 0;
    g_clutter[i]->position[1] = -1000.0f;
    ex_scene_add_model(scene, g_clutter[i]);
  }
  // human NPCs: tiny figures that share the map with the titans (co-play). Each
  // is a cheap 4-part humanoid (head/torso/legs/pack).
  for (int i=0;i<NUM_HUMANS;i++){
    ex_model_t *tpl[4] = { g_tpl_sphere?g_tpl_sphere:g_boxtpl, g_tpl_bevel?g_tpl_bevel:g_boxtpl,
                           g_tpl_limb?g_tpl_limb:g_boxtpl, g_boxtpl };
    for (int k=0;k<4;k++){ g_human_parts[i][k]=ex_model_copy(tpl[k]);
      g_human_parts[i][k]->is_lit=1; g_human_parts[i][k]->is_shadow=0;
      g_human_parts[i][k]->position[1]=-1000.0f; ex_scene_add_model(scene, g_human_parts[i][k]); }
    g_humans[i].active=0;
  }
  // ragdoll debris pool (broken-off part chunks + impact chips)
  for (int i=0;i<NUM_DEBRIS;i++){
    g_debris_models[i]=ex_model_copy(g_tpl_bevel?g_tpl_bevel:g_boxtpl);
    g_debris_models[i]->is_lit=1; g_debris_models[i]->is_shadow=0;
    g_debris_models[i]->position[1]=-1000.0f;
    ex_scene_add_model(scene, g_debris_models[i]);
  }
  { ex_model_t *rtpl = ex_iqm_load_model(scene, "data/glowdisc.iqm", 0);
    for (int i=0;i<NUM_SHOCKRINGS;i++){ g_shockrings[i]=ex_model_copy(rtpl);
      g_shockrings[i]->emissive_solid=1; g_shockrings[i]->is_lit=0; g_shockrings[i]->is_shadow=0;
      g_shockrings[i]->position[1]=-1000.0f; ex_scene_add_model(scene, g_shockrings[i]); }
    ex_model_t *ctpl = ex_iqm_load_model(scene, "data/crater.iqm", 0);
    for (int i=0;i<NUM_CRATERS;i++){ g_craters[i]=ex_model_copy(ctpl?ctpl:rtpl);
      g_craters[i]->is_lit=1; g_craters[i]->is_shadow=0; g_craters[i]->position[1]=-1000.0f;
      ex_scene_add_model(scene, g_craters[i]); }
    for (int i=0;i<NUM_HAZARDS;i++){ g_haz_models[i]=ex_model_copy(rtpl);
      g_haz_models[i]->emissive_solid=1; g_haz_models[i]->is_lit=0; g_haz_models[i]->is_shadow=0;
      g_haz_models[i]->position[1]=-1000.0f; ex_scene_add_model(scene, g_haz_models[i]); }
    g_landing_ring = ex_model_copy(rtpl); g_landing_ring->emissive_solid=1; g_landing_ring->is_lit=0;    g_landing_ring->is_shadow=0; g_landing_ring->position[1]=-1000.0f; ex_scene_add_model(scene, g_landing_ring);
    for (int i=0;i<EX_SPEARCON_MAX;i++){ g_spearcon_models[i]=ex_model_copy(rtpl);
      g_spearcon_models[i]->emissive_solid=1; g_spearcon_models[i]->is_lit=0; g_spearcon_models[i]->is_shadow=0;
      g_spearcon_models[i]->position[1]=-1000.0f; ex_scene_add_model(scene, g_spearcon_models[i]); } }
  g_star_orb = ex_model_copy(sig_template(EX_SIG_SOLAR) ? sig_template(EX_SIG_SOLAR) : g_tpl_back);
  if (g_star_orb){ g_star_orb->emissive_solid=1; g_star_orb->is_lit=0; g_star_orb->is_shadow=0;
    g_star_orb->position[1]=-1000.0f; ex_scene_add_model(scene, g_star_orb); }
  for (int i=0;i<EX_ROBOT_PARTS;i++){ g_part_integ[i]=1.0f; g_part_detached[i]=0; }

  spawn_body(g_player_parts);
  // swap the player's BACK slot to the class SIGNATURE component (enemies keep
  // the default). part_template gave everyone the wing-cannon; replace it here.
  { ex_model_t *st = sig_template(g_player_sig);
    if (st && st != g_player_parts[EX_PART_BACK]) {
      ex_scene_remove_model(scene, g_player_parts[EX_PART_BACK]);
      g_player_parts[EX_PART_BACK] = ex_model_copy(st);
      g_player_parts[EX_PART_BACK]->is_lit = 1;
      g_player_parts[EX_PART_BACK]->is_shadow = 0;
      ex_scene_add_model(scene, g_player_parts[EX_PART_BACK]);
    } }
  hide_body(g_player_parts);
  for (int i = 0; i < EX_MAX_ENEMIES; i++) { spawn_body(g_enemy_bodies[i]); hide_body(g_enemy_bodies[i]); }
  spawn_body(g_ally_parts); hide_body(g_ally_parts);
  { ex_model_t *st = sig_template(EX_SIG_HEAVY_CANNON);   // Forgeheart carries a heavy demolition rig
    if (st) { ex_scene_remove_model(scene, g_ally_parts[EX_PART_BACK]);
      g_ally_parts[EX_PART_BACK] = ex_model_copy(st);
      g_ally_parts[EX_PART_BACK]->is_lit=1; g_ally_parts[EX_PART_BACK]->is_shadow=0;
      ex_scene_add_model(scene, g_ally_parts[EX_PART_BACK]); } }
  for (int i = 0; i < EX_MAX_ENEMIES; i++) g_enemy_sig[i] = EX_SIG_WING_CANNON; // matches part_template BACK
  // greeble detail pool for the player body (thousand-part look, budget-capped)
  for (int i = 0; i < DETAIL_MODELS; i++) {
    g_detail_models[i] = ex_model_copy(box);
    g_detail_models[i]->is_lit = 1;
    g_detail_models[i]->is_shadow = 0;
    g_detail_models[i]->position[1] = -1000.0f;
    ex_scene_add_model(scene, g_detail_models[i]);
  }
  g_turret_model = ex_model_copy(box);
  g_turret_model->is_lit = 1; g_turret_model->is_shadow = 0; g_turret_model->position[1] = -1000.0f;
  ex_scene_add_model(scene, g_turret_model);
  memset(&g_turret, 0, sizeof(g_turret));
  ex_model_t *rocktpl  = ex_iqm_load_model(scene, "data/rock.iqm", 0);
  ex_model_t *spiretpl = ex_iqm_load_model(scene, "data/spire.iqm", 0);
  for (int i = 0; i < EX_MAP_MAX_VOLS; i++) {
    g_vol_models[i] = ex_model_copy(box);
    g_vol_models[i]->is_lit = 1; g_vol_models[i]->is_shadow = 0; g_vol_models[i]->position[1] = -1000.0f;
    ex_scene_add_model(scene, g_vol_models[i]);
    g_rock_models[i] = ex_model_copy(rocktpl);
    g_rock_models[i]->is_lit = 1; g_rock_models[i]->is_shadow = 0; g_rock_models[i]->position[1] = -1000.0f;
    ex_scene_add_model(scene, g_rock_models[i]);
    g_spire_models[i] = ex_model_copy(spiretpl);
    g_spire_models[i]->is_lit = 1; g_spire_models[i]->is_shadow = 0; g_spire_models[i]->position[1] = -1000.0f;
    ex_scene_add_model(scene, g_spire_models[i]);
  }
  ex_model_t *boultpl = ex_iqm_load_model(scene, "data/boulder.iqm", 0);
  ex_model_t *crystpl = ex_iqm_load_model(scene, "data/column.iqm", 0);
  for (int i = 0; i < DECO_ROCK; i++) {
    g_deco_rock[i] = ex_model_copy((i%2)?boultpl:rocktpl);
    g_deco_rock[i]->is_lit=1; g_deco_rock[i]->is_shadow=0; g_deco_rock[i]->position[1]=-1000.0f;
    ex_scene_add_model(scene, g_deco_rock[i]);
  }
  for (int i = 0; i < DECO_TALL; i++) {
    g_deco_tall[i] = ex_model_copy((i%2)?crystpl:spiretpl);
    g_deco_tall[i]->is_lit=1; g_deco_tall[i]->is_shadow=0; g_deco_tall[i]->position[1]=-1000.0f;
    ex_scene_add_model(scene, g_deco_tall[i]);
  }
  ex_model_t *archtpl = ex_iqm_load_model(scene, "data/arch.iqm", 0);
  for (int i = 0; i < DECO_ARCH; i++) {
    g_deco_arch[i] = ex_model_copy(archtpl);
    g_deco_arch[i]->is_lit=1; g_deco_arch[i]->is_shadow=0; g_deco_arch[i]->position[1]=-1000.0f;
    ex_scene_add_model(scene, g_deco_arch[i]);
  }
  { const char *wf[10]={"data/weapon.iqm","data/weapon_burst.iqm","data/weapon_beam.iqm","data/weapon_wave.iqm","data/weapon_lob.iqm",
                         "data/weapon_pierce.iqm","data/weapon_flak.iqm","data/weapon_plasma.iqm","data/weapon_ricochet.iqm","data/weapon_suppress.iqm"};
    for (int i=0;i<10;i++){ g_weapon_models[i]=ex_iqm_load_model(scene, wf[i], 0);
      g_weapon_models[i]->is_lit=1; g_weapon_models[i]->is_shadow=0; g_weapon_models[i]->position[1]=-1000.0f;
      ex_scene_add_model(scene, g_weapon_models[i]); } }
  { ex_model_t *towtpl = ex_iqm_load_model(scene, "data/tower.iqm", 0);
    for (int i=0;i<DECO_TOWER;i++){ g_deco_tower[i]=ex_model_copy(towtpl);
      g_deco_tower[i]->is_lit=1; g_deco_tower[i]->is_shadow=0; g_deco_tower[i]->position[1]=-1000.0f;
      ex_scene_add_model(scene, g_deco_tower[i]); } }
  for (int i=0;i<EX_MAX_ENEMIES;i++){ g_enemy_weapons[i]=ex_model_copy(g_weapon_models[0]);
    g_enemy_weapons[i]->is_lit=1; g_enemy_weapons[i]->is_shadow=0; g_enemy_weapons[i]->position[1]=-1000.0f;
    ex_scene_add_model(scene, g_enemy_weapons[i]); }
  { ex_model_t *disctpl = ex_iqm_load_model(scene, "data/glowdisc.iqm", 0);
    for (int i=0;i<NUM_LAUNCH;i++){ g_launch_models[i]=ex_model_copy(disctpl);
      g_launch_models[i]->emissive_solid=1; g_launch_models[i]->is_lit=0; g_launch_models[i]->is_shadow=0; g_launch_models[i]->position[1]=-100000.0f;
      ex_scene_add_model(scene, g_launch_models[i]); } }
  { ex_model_t *dttpl = ex_iqm_load_model(scene, "data/turret_def.iqm", 0);
    for (int i=0;i<NUM_DEF_TURRETS;i++){ g_deft_models[i]=ex_model_copy(dttpl);
      g_deft_models[i]->is_lit=1; g_deft_models[i]->is_shadow=0; g_deft_models[i]->position[1]=-100000.0f;
      ex_scene_add_model(scene, g_deft_models[i]); } }
  { ex_model_t *ldtpl = ex_iqm_load_model(scene, "data/glowdisc.iqm", 0);
    for (int i=0;i<NUM_LIQUID;i++){ g_liquid[i]=ex_model_copy(ldtpl);
      g_liquid[i]->emissive_solid=1; g_liquid[i]->is_lit=0; g_liquid[i]->is_shadow=0; g_liquid[i]->position[1]=-100000.0f;
      ex_scene_add_model(scene, g_liquid[i]); } }
  for (int i=0;i<MAP_DETAIL;i++){ g_map_detail[i]=ex_model_copy(box);
    g_map_detail[i]->is_lit=1; g_map_detail[i]->is_shadow=0; g_map_detail[i]->position[1]=-100000.0f;
    ex_scene_add_model(scene, g_map_detail[i]); }
  // menu backdrop: a ship cruising through space (Destiny-style destination select)
  g_ship_model = ex_iqm_load_model(scene, "data/ship.iqm", 0);
  g_ship_model->is_lit=1; g_ship_model->is_shadow=0; g_ship_model->position[1]=-100000.0f;
  ex_scene_add_model(scene, g_ship_model);
  for (int i=0;i<2;i++){ g_ship_glow[i]=ex_iqm_load_model(scene, "data/glowdisc.iqm", 0);
    g_ship_glow[i]->emissive_solid=1; g_ship_glow[i]->is_lit=0; g_ship_glow[i]->is_shadow=0; g_ship_glow[i]->position[1]=-100000.0f;
    ex_scene_add_model(scene, g_ship_glow[i]); }
  // planets for the deep-space menu backdrop
  for (int i=0;i<NUM_PLANETS;i++){ char pf[64]; snprintf(pf,sizeof(pf),"data/%s.iqm",g_planet_ids[i]);
    g_planets[i]=ex_iqm_load_model(scene, pf, 0);
    g_planets[i]->is_lit=1; g_planets[i]->is_shadow=0; g_planets[i]->position[1]=-100000.0f;
    ex_scene_add_model(scene, g_planets[i]); }
  { ex_model_t *odtpl2 = ex_iqm_load_model(scene, "data/glowdisc.iqm", 0);
    for (int i=0;i<NUM_CONSOLES;i++){ g_consoles[i]=ex_model_copy(odtpl2);
      g_consoles[i]->emissive_solid=1; g_consoles[i]->is_lit=0; g_consoles[i]->is_shadow=0; g_consoles[i]->position[1]=-100000.0f;
      ex_scene_add_model(scene, g_consoles[i]); } }
  { ex_model_t *odtpl = ex_iqm_load_model(scene, "data/glowdisc.iqm", 0);
    for (int i=0;i<NUM_OBJ_PADS;i++){ g_obj_pads[i]=ex_model_copy(odtpl);
      g_obj_pads[i]->emissive_solid=1; g_obj_pads[i]->is_lit=0; g_obj_pads[i]->is_shadow=0; g_obj_pads[i]->position[1]=-100000.0f;
      ex_scene_add_model(scene, g_obj_pads[i]); } }
  ex_loadout_init(&g_abilities);
  ex_loadout_add(&g_abilities, EX_ABIL_SHIELD);
  ex_loadout_add(&g_abilities, EX_ABIL_DASH);
  ex_loadout_add(&g_abilities, EX_ABIL_HOVER);
  ex_weapon_mods_init(&g_wmods);

  // vfx model pool (bolts / flashes / debris) -- unlit glow.iqm, tinted per effect
  ex_model_t *glowtpl = ex_iqm_load_model(scene, "data/glow.iqm", 0);
  glowtpl->is_lit = 0; glowtpl->is_shadow = 0; glowtpl->position[1] = -1000.0f;
  ex_scene_add_model(scene, glowtpl);
  ex_vfx_init(&g_vfx);
  g_muzzle = ex_model_copy(glowtpl); g_muzzle->emissive_solid=1; g_muzzle->is_lit=0; g_muzzle->is_shadow=0; g_muzzle->position[1]=-1000.0f; ex_scene_add_model(scene, g_muzzle);
  g_cape = ex_model_copy(part_template(0)); g_cape->is_lit=1; g_cape->is_shadow=0; g_cape->position[1]=-1000.0f; ex_scene_add_model(scene, g_cape);
  for (int i=0;i<NUM_MOTES;i++){ g_motes[i]=ex_model_copy(glowtpl); g_motes[i]->emissive_solid=1; g_motes[i]->is_lit=0; g_motes[i]->is_shadow=0;
    g_motes[i]->position[1]=-1000.0f; ex_scene_add_model(scene, g_motes[i]);
    unsigned r=i*2654435761u; g_mote_p[i][0]=((r>>8&0xFF)/255.0f-0.5f)*60.0f; g_mote_p[i][1]=((r>>16&0xFF)/255.0f)*30.0f; g_mote_p[i][2]=((r>>24&0xFF)/255.0f-0.5f)*60.0f; }
  for (int i = 0; i < VFX_MODELS; i++) {
    g_vfx_models[i] = ex_model_copy(glowtpl); g_vfx_models[i]->emissive_solid = 1;
    g_vfx_models[i]->is_lit = 0;
    g_vfx_models[i]->is_shadow = 0;
    g_vfx_models[i]->position[1] = -1000.0f;
    ex_scene_add_model(scene, g_vfx_models[i]);
  }
  // pickup models: glowing floor items
  for (int i = 0; i < EX_MAX_PICKUPS; i++) {
    g_pickup_models[i] = ex_model_copy(glowtpl);
    g_pickup_models[i]->is_lit = 0;
    g_pickup_models[i]->is_shadow = 0;
    g_pickup_models[i]->position[1] = -1000.0f;
    ex_scene_add_model(scene, g_pickup_models[i]);
  }

  // menu + default binds so menus navigate before a layout is chosen
  g_build = ex_build_default();
  ex_campaign_init(&g_campaign);
  ex_menu_init(&g_menu, &g_campaign, 10);
  ex_keybind_preset(&g_keys, EX_LAYOUT_QWERTY); g_keys_layout = EX_LAYOUT_QWERTY;
  load_config();   // restore saved keybinds + settings from the XDG config dir
  load_asset_aabbs();   // programmatic mesh bounding boxes
}

// menu navigation from raw keys: arrows always work; WASD/ESDF also work
static void spawn_wave_makeup(ex_wave_makeup_t mk)
{
  // reset the encounter and spawn the wave's enemies + their bodies
  ex_encounter_init(&g_enc, EX_WIN_ELIMINATE, 0.0f);
  ex_encounter_spawn_makeup(&g_enc, mk.grunts, mk.corrupted, mk.hijacks, -60,-60,60,60,
                            0x5A7E1u ^ (unsigned)(g_waves.wave*2654435761u));
  for (int i = 0; i < g_enc.count && i < EX_MAX_ENEMIES; i++) {
    spawn_body(g_enemy_bodies[i]); g_enemy_phase[i]=0.0f;
  }
  g_prev_alive = ex_encounter_alive(&g_enc);
  for(int i=0;i<EX_MAX_ENEMIES;i++) g_was_alive[i] = (i<g_enc.count && g_enc.enemies[i].state!=EX_ENEMY_DEAD);
}

static void apply_theme_lighting(const char *mid, float brightness)
{
  // each map gets its own key + fill colour so they read as distinct moods.
  float key[3]  = {1.6f,1.35f,1.0f};   // default warm
  float fill[3] = {0.25f,0.3f,0.5f};   // default cool
  if      (!strcmp(mid,"wastes"))     { key[0]=1.6f;key[1]=1.05f;key[2]=0.6f;  fill[0]=0.4f;fill[1]=0.28f;fill[2]=0.2f; }
  else if (!strcmp(mid,"frostline"))  { key[0]=0.85f;key[1]=1.05f;key[2]=1.5f; fill[0]=0.25f;fill[1]=0.35f;fill[2]=0.55f; }
  else if (!strcmp(mid,"emberpit"))   { key[0]=1.8f;key[1]=0.75f;key[2]=0.4f;  fill[0]=0.5f;fill[1]=0.2f;fill[2]=0.15f; }
  else if (!strcmp(mid,"vault"))      { key[0]=1.1f;key[1]=1.3f;key[2]=1.4f;   fill[0]=0.3f;fill[1]=0.4f;fill[2]=0.5f; }
  else if (!strcmp(mid,"foundry"))    { key[0]=1.5f;key[1]=1.15f;key[2]=0.75f; fill[0]=0.45f;fill[1]=0.3f;fill[2]=0.18f; }
  else if (!strcmp(mid,"shield_edge")){ key[0]=0.75f;key[1]=1.0f;key[2]=1.7f;  fill[0]=0.2f;fill[1]=0.35f;fill[2]=0.6f; }
  else if (!strcmp(mid,"mining"))     { key[0]=1.35f;key[1]=1.2f;key[2]=0.7f;  fill[0]=0.35f;fill[1]=0.3f;fill[2]=0.2f; }
  else if (!strcmp(mid,"outer_rim"))  { key[0]=0.8f;key[1]=0.9f;key[2]=1.2f;   fill[0]=0.15f;fill[1]=0.2f;fill[2]=0.4f; }
  else if (!strcmp(mid,"cybertron"))  { key[0]=1.6f;key[1]=1.35f;key[2]=0.85f; fill[0]=0.4f;fill[1]=0.35f;fill[2]=0.25f; }
  else if (!strcmp(mid,"reclaim"))    { key[0]=1.0f;key[1]=1.4f;key[2]=0.9f;  fill[0]=0.25f;fill[1]=0.4f;fill[2]=0.25f; }
  // background/sky void colour per theme (strong at-a-glance differentiator)
  extern float ex_clear_color[3]; extern float ex_fog_color[3]; extern float ex_fog_density;
  float bg[3]={0.02f,0.02f,0.03f};
  if      (!strcmp(mid,"wastes"))     { bg[0]=0.10f;bg[1]=0.07f;bg[2]=0.05f; }
  else if (!strcmp(mid,"frostline"))  { bg[0]=0.06f;bg[1]=0.10f;bg[2]=0.16f; }
  else if (!strcmp(mid,"emberpit"))   { bg[0]=0.14f;bg[1]=0.04f;bg[2]=0.03f; }
  else if (!strcmp(mid,"vault"))      { bg[0]=0.08f;bg[1]=0.10f;bg[2]=0.12f; }
  else if (!strcmp(mid,"foundry"))    { bg[0]=0.07f;bg[1]=0.06f;bg[2]=0.05f; }
  else if (!strcmp(mid,"shield_edge")){ bg[0]=0.02f;bg[1]=0.03f;bg[2]=0.10f; }
  else if (!strcmp(mid,"mining"))     { bg[0]=0.05f;bg[1]=0.05f;bg[2]=0.06f; }
  else if (!strcmp(mid,"outer_rim"))  { bg[0]=0.01f;bg[1]=0.01f;bg[2]=0.04f; }
  else if (!strcmp(mid,"cybertron"))  { bg[0]=0.10f;bg[1]=0.08f;bg[2]=0.04f; }
  else if (!strcmp(mid,"reclaim"))    { bg[0]=0.05f;bg[1]=0.10f;bg[2]=0.05f; }
  ex_clear_color[0]=bg[0]; ex_clear_color[1]=bg[1]; ex_clear_color[2]=bg[2];
  // choose the skybox that matches this theme
  if      (!strcmp(mid,"frostline")) g_active_sky=1;      // ice
  else if (!strcmp(mid,"emberpit")||!strcmp(mid,"foundry")) g_active_sky=2; // ember
  else if (!strcmp(mid,"cybertron")) g_active_sky=3;      // cyber
  else if (!strcmp(mid,"reclaim")) g_active_sky=5;        // jungle
  else if (!strcmp(mid,"wastes")||!strcmp(mid,"mining")) g_active_sky=4; // waste
  else g_active_sky=0;                                    // space (rim/shield/vault)
  // match fog to the sky/background so geometry melts into the horizon
  ex_fog_color[0]=bg[0]*2.2f; ex_fog_color[1]=bg[1]*2.2f; ex_fog_color[2]=bg[2]*2.2f;
  ex_fog_density = 0.006f;
  // COSMIC-SCALE THEMES: as the world tips toward the void, the atmosphere goes
  // colder and denser so distance reads as vast + pressing, not just far. Keeps
  // the palette dark (never brighter) -- a distinct register from the grounded
  // industrial themes, bending toward the cold cosmic mood.
  if (!strcmp(mid,"outer_rim")) {
    ex_fog_color[0]=0.02f; ex_fog_color[1]=0.03f; ex_fog_color[2]=0.06f;   // deep steel-void
    ex_fog_density = 0.010f;
  } else if (!strcmp(mid,"shield_edge")) {
    ex_fog_color[0]=0.03f; ex_fog_color[1]=0.05f; ex_fog_color[2]=0.11f;   // cold barrier-glow haze
    ex_fog_density = 0.0085f;
  }
  if (g_horror_mode) {
    // DREAD: the fog closes in, drains to a sick dark grey, and the light dies --
    // you can only see a short way, and something is out there.
    ex_fog_color[0]=0.06f; ex_fog_color[1]=0.07f; ex_fog_color[2]=0.08f;
    ex_fog_density = 0.045f;
    ex_clear_color[0]=0.02f; ex_clear_color[1]=0.025f; ex_clear_color[2]=0.03f;
    if (g_key_light){ g_key_light->color[0]*=0.4f; g_key_light->color[1]*=0.42f; g_key_light->color[2]*=0.5f;
      g_horror_key[0]=g_key_light->color[0]; g_horror_key[1]=g_key_light->color[1]; g_horror_key[2]=g_key_light->color[2]; }
    if (g_fill_light){ g_fill_light->color[0]*=0.3f; g_fill_light->color[1]*=0.32f; g_fill_light->color[2]*=0.4f; }
  }
  // low-gravity combat on the space maps (Dominion asteroid-belt sector)
  if (!strcmp(mid,"outer_rim") || !strcmp(mid,"shield_edge")) g_gravity_scale = 0.4f;
  else g_gravity_scale = 1.0f;
  if (g_key_light)  { g_key_light->color[0]=key[0]*brightness; g_key_light->color[1]=key[1]*brightness; g_key_light->color[2]=key[2]*brightness; }
  if (g_fill_light) { g_fill_light->color[0]=fill[0]; g_fill_light->color[1]=fill[1]; g_fill_light->color[2]=fill[2]; }
}

static void menu_backdrop(float dt);
static void builder_preview(float dt);
static void deploy_sequence(float dt);
static void cutscene_run(float dt);
static int menu_list_start_row(int rows);
static int menu_current_rows(void);
// Build the player's collision-box list (map volumes + map/deco colliders) once,
// shared by the step resolve and the climb wall-probe. Returns the box count.
static int build_player_boxes(ex_step_box_t *boxes, int cap)
{
  int nb = 0;
  for (int i = 0; i < g_blueprint.n_vols && nb < cap; i++) {
    ex_map_vol_t *v = &g_blueprint.vols[i];
    boxes[nb].center[0]=v->center[0]; boxes[nb].center[1]=v->center[1]; boxes[nb].center[2]=v->center[2];
    boxes[nb].half[0]=v->half[0]; boxes[nb].half[1]=v->half[1]; boxes[nb].half[2]=v->half[2]; nb++;
  }
  for (int i = 0; i < g_map_col_n && nb < cap; i++) { float *c=g_map_col[i];
    boxes[nb].center[0]=c[0]; boxes[nb].center[1]=c[1]; boxes[nb].center[2]=c[2];
    boxes[nb].half[0]=c[3]; boxes[nb].half[1]=c[4]; boxes[nb].half[2]=c[5]; nb++; }
  for (int i = 0; i < g_deco_col_n && nb < cap; i++) { float *c=g_deco_col[i];
    boxes[nb].center[0]=c[0]; boxes[nb].center[1]=c[1]; boxes[nb].center[2]=c[2];
    boxes[nb].half[0]=c[3]; boxes[nb].half[1]=c[4]; boxes[nb].half[2]=c[5]; nb++; }
  // arena cover FEATURES (auto colliders 0..feature_base) -- these were placed in
  // g_auto_col but never fed to the player step-resolve, so you couldn't jump onto
  // (or properly stand on) them. Include them now. Clutter above the base is left
  // out on purpose (small crates the titan steps over).
  for (int i = 0; i < g_feature_col_base && nb < cap; i++) { float *c=g_auto_col[i];
    boxes[nb].center[0]=c[0]; boxes[nb].center[1]=c[1]; boxes[nb].center[2]=c[2];
    boxes[nb].half[0]=c[3]; boxes[nb].half[1]=c[4]; boxes[nb].half[2]=c[5]; nb++; }
  return nb;
}

static void resolve_player_vs_volumes(ex_entity_t *ent, float dt)
{
  const float pr = g_human_scale ? 0.35f : 0.9f;   // player collider radius (xz)
  static ex_step_box_t boxes[EX_MAP_MAX_VOLS + MAX_MAP_COL + MAX_DECO_COL + MAX_AUTO_COL];
  int nb = build_player_boxes(boxes, EX_MAP_MAX_VOLS + MAX_MAP_COL + MAX_DECO_COL + MAX_AUTO_COL);
  float prev_y = ent->position[1] - ent->velocity[1]*dt;   // vertical pos before this frame's move
  int on_box = ex_step_resolve(ent->position, ent->velocity, pr, boxes, nb, prev_y);
  if (on_box) ent->grounded = 1;   // standing on a stair/platform counts as grounded (so you can jump)
}

// Is there a climbable wall ahead of the player along the horizontal dir (dx,dz)?
static int player_wall_ahead(float dx, float dz, const float pos[3], float *out_top)
{
  const float pr = g_human_scale ? 0.35f : 0.9f;
  static ex_step_box_t boxes[EX_MAP_MAX_VOLS + MAX_MAP_COL + MAX_DECO_COL + MAX_AUTO_COL];
  int nb = build_player_boxes(boxes, EX_MAP_MAX_VOLS + MAX_MAP_COL + MAX_DECO_COL + MAX_AUTO_COL);
  return ex_step_wall_ahead(pos, dx, dz, pr, boxes, nb, 0.5f, 1.0f, out_top);
}

// Camera boom slab-clip: if the boom ray (focus + dir*t) enters this AABB before
// `best`, return the entry distance so the camera pulls in short of it. Used so
// the third-person camera never booms INTO a wall/cover box and fills the screen.
static float boom_slab(const float focus[3], const float dir[3], float best,
                       float cx,float cy,float cz, float hx,float hy,float hz)
{
  float mn[3]={cx-hx-0.4f, cy-hy-0.4f, cz-hz-0.4f};
  float mx[3]={cx+hx+0.4f, cy+hy+0.4f, cz+hz+0.4f};
  float t0=0.0f, t1=best; int hit=1;
  for (int a=0;a<3;a++){
    float o=focus[a], d=dir[a];
    if (fabsf(d)<1e-5f){ if(o<mn[a]||o>mx[a]){hit=0;break;} }
    else { float ta=(mn[a]-o)/d, tb=(mx[a]-o)/d; if(ta>tb){float tmp=ta;ta=tb;tb=tmp;}
           if(ta>t0)t0=ta; if(tb<t1)t1=tb; if(t0>t1){hit=0;break;} }
  }
  if (hit && t0>0.0f && t0<best) return t0;
  return best;
}
static void menu_input(void)
{
  // controls rebinding: capture the next physical key for the chosen action
  if (g_menu.state == EX_GS_CONTROLS && g_menu.rebinding >= 0) {
    if (ex_keys_down[SDL_SCANCODE_ESCAPE]) { g_menu.rebinding = -1; ex_keys_down[SDL_SCANCODE_ESCAPE]=0; return; }
    for (int sc = 4; sc < SDL_NUM_SCANCODES; sc++) {
      if (ex_keys_down[sc]) {
        ex_keybind_set(&g_keys, (ex_action_t)g_menu.rebinding, (SDL_Scancode)sc);
        ex_keys_down[sc] = 0; g_menu.rebinding = -1; break;
      }
    }
    return; // swallow input while listening
  }
  if (g_menu.controls_reset) {
    g_menu.controls_reset = 0;
    ex_keybind_preset(&g_keys, g_menu.loadout.layout); g_keys_layout = (int)g_menu.loadout.layout;
  }
  if (g_menu.controls_store) {
    g_menu.controls_store = 0;
    save_config();   // write keybinds + settings to the XDG config dir
  }
  struct { SDL_Scancode k; ex_nav_t nav; } map[] = {
    { SDL_SCANCODE_UP, EX_NAV_UP },       { SDL_SCANCODE_DOWN, EX_NAV_DOWN },
    { SDL_SCANCODE_LEFT, EX_NAV_LEFT },   { SDL_SCANCODE_RIGHT, EX_NAV_RIGHT },
    { SDL_SCANCODE_W, EX_NAV_UP },        { SDL_SCANCODE_S, EX_NAV_DOWN },
    { SDL_SCANCODE_A, EX_NAV_LEFT },      { SDL_SCANCODE_D, EX_NAV_RIGHT },
    { SDL_SCANCODE_RETURN, EX_NAV_CONFIRM }, { SDL_SCANCODE_SPACE, EX_NAV_CONFIRM },
    { SDL_SCANCODE_BACKSPACE, EX_NAV_BACK },
  };
  for (size_t i = 0; i < sizeof(map)/sizeof(map[0]); i++) {
    if (ex_keys_down[map[i].k]) { ex_menu_nav(&g_menu, map[i].nav); ex_keys_down[map[i].k] = 0; }
  }
  // ESC goes back one screen in menus (and cancels a cutscene)
  if (ex_keys_down[SDL_SCANCODE_ESCAPE]) {
    ex_menu_nav(&g_menu, EX_NAV_BACK);
    ex_keys_down[SDL_SCANCODE_ESCAPE] = 0;
  }
  // mouse: hover highlights a row, click activates it
  int mx, my; uint32_t mb = SDL_GetMouseState(&mx, &my);
  int win_w = 1280, win_h = 720;
  SDL_Window *win = SDL_GL_GetCurrentWindow();
  if (win) SDL_GetWindowSize(win, &win_w, &win_h);
  // only let the mouse move the selection when it actually MOVED this frame,
  // otherwise a stationary cursor fights the keyboard every frame (which made
  // the arrow keys seem to "only work sometimes").
  static int last_mx = -1, last_my = -1;
  int mouse_moved = (mx != last_mx || my != last_my);
  last_mx = mx; last_my = my;
  int rows = menu_current_rows();
  if (rows > 0 && g_menu.rebinding < 0 && mouse_moved) {
    // the VGA overlay is a 16-row grid anchored top-left and scaled by
    // ex_vga_scale, so the grid spans only the top `sc` fraction of the window.
    // Map the mouse through that scale to get the true row.
    extern float ex_vga_scale; float sc = ex_vga_scale; if(sc<0.22f)sc=0.22f; if(sc>1.0f)sc=1.0f;
    int sr = menu_list_start_row(rows);
    float fy = ((float)my / (float)win_h) / sc;      // 0..1 within the grid
    int vga_row = (int)(fy * 16.0f);
    int hovered = vga_row - sr;
    if (hovered >= 0 && hovered < rows) {
      if (g_menu.state == EX_GS_CONTROLS) g_menu.cursor = hovered;
      else ex_menu_set_cursor(&g_menu, hovered);
    }
  }
  // left-click ANYWHERE acts as Enter on the current selection, so mouse users
  // never need to reach for the keyboard. (Edge-triggered so it fires once.)
  if (g_menu.rebinding < 0) {
    static int prev_down = 0;
    int down = (mb & SDL_BUTTON(SDL_BUTTON_LEFT)) != 0;
    if (down && !prev_down) ex_menu_click(&g_menu);
    prev_down = down;
  }
  if (g_menu.start_requested) { g_menu.start_requested = 0;
    // campaign missions open with their film's story cutscene, then deploy;
    // skirmish drops straight in.
    if (g_menu.in_campaign && g_menu.mission_index < NUM_CUTSCENES)
      cutscene_start(g_menu.mission_index, 1);
    else
      start_match();
  }
  if (g_menu.to_menu_requested) { g_menu.to_menu_requested = 0; g_match_live = 0;
    hide_body(g_player_parts); for(int i=0;i<EX_MAX_ENEMIES;i++){ hide_body(g_enemy_bodies[i]); if(g_enemy_weapons[i]) g_enemy_weapons[i]->position[1]=-1000.0f; } }
  if (g_menu.quit_requested) exit(0);
}

static int menu_list_start_row(int rows)
{
  // VGA grid is 16 text rows tall; keep the whole list (title at 1, sub at 2)
  // on screen by shifting the start up for long lists.
  int start = 4;
  if (start + rows > 16) start = 16 - rows;
  if (start < 3) start = 3;
  return start;
}
static int menu_current_rows(void)
{
  if (g_menu.state == EX_GS_CONTROLS) return EX_ACT_COUNT + 4;
  if (g_menu.state == EX_GS_CODEX) return 1;
  char tmp[8]; return ex_menu_line(&g_menu, 0, tmp, sizeof(tmp));
}
static int vga_print_wrapped(int col, int row, const char *text, int width)
{
  char line[64]; int ll=0, used=0, last_space=-1;
  for (int i=0; ; i++) {
    char c = text[i];
    if (c=='\0') { line[ll]='\0'; if(ll) ex_vga_print(col, row+used, line); used++; break; }
    line[ll]=c; if (c==' ') last_space=ll; ll++;
    if (ll >= width) {
      int brk = (last_space>0)? last_space : ll;
      char save=line[brk]; line[brk]='\0';
      ex_vga_print(col, row+used, line); used++;
      int rest = ll-(brk+ (save==' '?1:0));
      memmove(line, line+brk+(save==' '?1:0), rest); ll=rest; last_space=-1;
    }
  }
  return used;
}
static void draw_menu(void)
{
  ex_vga_clear();
  ex_vga_setfg(255,255,255,255); ex_vga_setbg(0,0,0,255);
  ex_vga_print(2, 1, "cybertron _ arena of gods");
  const char *sub = "arrows _ enter _ backspace";
  if (g_menu.state == EX_GS_BUILDER)  sub = "build your transformer";
  else if (g_menu.state == EX_GS_CUTSCENE) sub = "";
  else if (g_menu.state == EX_GS_SETTINGS) sub = "graphics";
  else if (g_menu.state == EX_GS_CONTROLS) sub = "controls";
  else if (g_menu.state == EX_GS_CODEX) sub = "codex _ left_right _ back";
  else if (g_menu.state == EX_GS_RESULTS) sub = g_menu.res_victory ? "victory" : "defeat";
  else if (g_menu.state == EX_GS_MISSIONS) sub = "campaign";
  else if (g_menu.state == EX_GS_CUSTOMIZE) sub = "customize bot _ mouse or arrows";
  else if (g_menu.state == EX_GS_MODE || g_menu.state == EX_GS_MAP) sub = "left_right to change";
  else if (g_menu.state == EX_GS_PAUSED) sub = "paused";
  ex_vga_setfg(160,160,180,255);
  ex_vga_print(2, 2, sub);
  char buf[64];
  if (g_menu.state == EX_GS_RESULTS) {
    char line[48];
    if (g_menu.res_victory) { ex_vga_setfg(120,255,160,255); ex_vga_print(6, 3, "ALL WAVES CLEARED"); }
    else { ex_vga_setfg(255,110,90,255); ex_vga_print(6, 3, "YOU WERE DESTROYED"); }
    ex_vga_setfg(220,220,230,255);
    snprintf(line,sizeof(line),"waves reached   %d", g_menu.res_waves); ex_vga_print(6, 6, line);
    snprintf(line,sizeof(line),"enemies downed  %d", g_menu.res_kills); ex_vga_print(6, 7, line);
    snprintf(line,sizeof(line),"score           %ld", g_menu.res_score); ex_vga_print(6, 8, line);
    if (g_menu.res_new_best) { ex_vga_setfg(255,220,90,255); ex_vga_print(6, 10, "* NEW HIGH SCORE *"); }
    else { ex_vga_setfg(160,160,180,255); snprintf(line,sizeof(line),"best            %ld", g_menu.res_best); ex_vga_print(6, 10, line); }
    ex_vga_setfg(140,220,140,255); ex_vga_print(6, 13, "[ click / enter to continue ]");
    return;
  }
  if (g_menu.state == EX_GS_CODEX) {
    const ex_lore_entry_t *le = ex_lore_entry(g_menu.codex_index);
    if (le) {
      ex_vga_setfg(120,255,180,255); ex_vga_print(1, 3, le->title);
      ex_vga_setfg(210,210,220,255);
      int rr = 5;
      for (int k=0;k<le->line_count;k++) rr += vga_print_wrapped(1, rr, le->lines[k], 30);
      char foot[32]; snprintf(foot,sizeof(foot),"%d / %d", g_menu.codex_index+1, ex_lore_count());
      ex_vga_setfg(140,140,160,255); ex_vga_print(1, 15, foot);
    }
    return;
  }
  if (g_menu.state == EX_GS_CONTROLS) {
    int nrows = EX_ACT_COUNT + 4;
    int sr = menu_list_start_row(nrows);
    for (int i = 0; i < nrows; i++) {
      if (i < EX_ACT_COUNT) {
        const char *kn = SDL_GetScancodeName(ex_keybind_get(&g_keys, (ex_action_t)i));
        if (g_menu.rebinding == i) snprintf(buf,sizeof(buf), "%-11s < press a key >", ex_action_name((ex_action_t)i));
        else snprintf(buf,sizeof(buf), "%-11s %s", ex_action_name((ex_action_t)i), (kn&&*kn)?kn:"?");
      } else if (i == EX_ACT_COUNT) snprintf(buf,sizeof(buf), "layout < %s >", g_menu.loadout.layout==EX_LAYOUT_DVORAK?"dvorak":"qwerty");
      else if (i == EX_ACT_COUNT+1) snprintf(buf,sizeof(buf), "reset to defaults");
      else if (i == EX_ACT_COUNT+2) snprintf(buf,sizeof(buf), "store configuration");
      else snprintf(buf,sizeof(buf), "back");
      int hl = (i == g_menu.cursor);
      ex_vga_setfg(hl?120:220, hl?255:220, hl?120:220, 255);
      ex_vga_print(4, sr + i, buf);
    }
    return;
  }
  int rows = ex_menu_line(&g_menu, 0, buf, sizeof(buf));
  int top = menu_list_start_row(rows);      // first grid row of the list
  int vis = 16 - top;                        // rows that actually fit on the 16-row grid
  if (vis < 1) vis = 1;
  int first = 0;                             // scroll so the cursor stays visible on long lists
  if (rows > vis) {
    first = g_menu.cursor - vis/2;
    if (first < 0) first = 0;
    if (first > rows - vis) first = rows - vis;
  }
  int shown = (rows < vis) ? rows : vis;
  for (int k = 0; k < shown; k++) {
    int i = first + k;
    ex_menu_line(&g_menu, i, buf, sizeof(buf));
    ex_vga_setfg(i == g_menu.cursor ? 120 : 220, i == g_menu.cursor ? 255 : 220, i == g_menu.cursor ? 120 : 220, 255);
    ex_vga_print(4, top + k, buf);
  }
  // scroll hints when the list runs off either end
  if (first > 0)          { ex_vga_setfg(120,140,170,255); ex_vga_print(28, top, "^"); }
  if (first + vis < rows) { ex_vga_setfg(120,140,170,255); ex_vga_print(28, top+shown-1, "v"); }
}

static void cutscene_run(float dt)
{
  const cutscene_t *cs = &CUTSCENES[g_cut_id];
  int who = cs->lines[g_cut_idx].who; if (who<0||who>=NUM_CAST) who=0;
  if (!g_cut_voiced) { ex_voice_say(cs->lines[g_cut_idx].text, CAST[who].voice); g_cut_voiced = 1; }
  g_cut_t += dt;
  float line_dur = 2.4f + (float)strlen(cs->lines[g_cut_idx].text) * 0.055f;   // reading time
  int advance = (g_cut_t > line_dur);
  if (ex_keys_down[SDL_SCANCODE_RETURN] || ex_keys_down[SDL_SCANCODE_SPACE]) {
    advance = 1; ex_keys_down[SDL_SCANCODE_RETURN]=0; ex_keys_down[SDL_SCANCODE_SPACE]=0; }
  { static int pd=0; int d=(SDL_GetMouseState(0,0)&SDL_BUTTON(SDL_BUTTON_LEFT))!=0; if(d&&!pd)advance=1; pd=d; }
  if (advance) {
    g_cut_idx++; g_cut_t = 0; g_cut_voiced = 0;
    if (g_cut_idx >= cs->n) { g_cut_active = 0; if (g_cut_after_deploy) start_match(); return; }
  }
  use_camera(CAM_MENU);
  menu_backdrop(dt);
  ex_vga_clear();
  ex_vga_setfg(120,150,210,255); ex_vga_print(2, 1, cs->title);
  // speaker name, then the line
  ex_vga_setfg(120,220,255,255); ex_vga_print(2, 11, CAST[who].name);
  ex_vga_setfg(235,235,245,255); ex_vga_print(2, 12, cs->lines[g_cut_idx].text);
  ex_vga_setfg(110,110,130,255); ex_vga_print(2, 14, "[ enter / click -- continue ]");
}

static void deploy_sequence(float dt)
{
  g_deploy_t -= dt;
  float p = 1.0f - g_deploy_t/DEPLOY_TIME; if(p<0)p=0; if(p>1)p=1;
  float e2 = p*p*(3.0f-2.0f*p);   // smoothstep ease

  // freeze the player as an idle body at the spawn point
  e->velocity[0]=e->velocity[1]=e->velocity[2]=0.0f;
  float ss = 0.5f + 0.35f*g_avatar_xform->size_class;
  pose_body(g_player_parts, e->position, 0, EX_BODY_PLAYER, ss, 0.0f, 0.0f, (int)ex_watch_active(&g_watch), 0.0f, &g_build, player_signature());

  // camera swoops from orbit down to the third-person start position
  vec3 sp = { e->position[0], e->position[1], e->position[2] };
  vec3 aerial = { sp[0]-6.0f, sp[1]+42.0f, sp[2]-32.0f };
  vec3 ground = { sp[0], sp[1]+2.5f, sp[2]-8.0f };
  camera->position[0]=aerial[0]+(ground[0]-aerial[0])*e2;
  camera->position[1]=aerial[1]+(ground[1]-aerial[1])*e2;
  camera->position[2]=aerial[2]+(ground[2]-aerial[2])*e2;
  vec3 look={sp[0],sp[1]+1.0f,sp[2]}, fwd; vec3_sub(fwd,look,camera->position); vec3_norm(fwd,fwd);
  camera->front[0]=fwd[0]; camera->front[1]=fwd[1]; camera->front[2]=fwd[2];

  // key light + sky track the camera
  pl->position[0]=sp[0]; pl->position[1]=sp[1]+6.0f; pl->position[2]=sp[2];
  for (int i=0;i<NUM_SKY;i++){ if(i==g_active_sky){ g_sky_models[i]->position[0]=camera->position[0]; g_sky_models[i]->position[1]=camera->position[1]; g_sky_models[i]->position[2]=camera->position[2]; } }

  // the dropship descends toward the spawn for the first 70%, then peels away
  if (g_ship_model){
    float dp = fminf(p/0.7f, 1.0f);
    float leave = (p>0.7f)? (p-0.7f)/0.3f : 0.0f;
    g_ship_model->position[0]=sp[0]+6.0f + leave*40.0f;
    g_ship_model->position[1]=sp[1]+38.0f - dp*30.0f + leave*30.0f;
    g_ship_model->position[2]=sp[2]-4.0f;
    g_ship_model->rotation[1]=200.0f; g_ship_model->rotation[2]=leave*25.0f;
    g_ship_model->scale=1.1f;
    g_ship_model->tint[0]=0.55f; g_ship_model->tint[1]=0.62f; g_ship_model->tint[2]=0.78f;
    for (int i=0;i<2;i++){ float pulse=0.6f+0.4f*sinf(g_anim_t*8.0f);
      g_ship_glow[i]->position[0]=g_ship_model->position[0]+((i?0.8f:-0.8f));
      g_ship_glow[i]->position[1]=g_ship_model->position[1]-0.1f;
      g_ship_glow[i]->position[2]=g_ship_model->position[2]-4.2f;
      g_ship_glow[i]->scale=0.35f*pulse;
      g_ship_glow[i]->tint[0]=0.6f; g_ship_glow[i]->tint[1]=0.85f; g_ship_glow[i]->tint[2]=1.0f; }
  }

  // HUD: DEPLOYING banner + destination
  const char *mid = g_menu.in_campaign ? g_campaign.missions[g_menu.mission_index].map
                                       : g_theme_ids[g_menu.map_index % NUM_THEMES];
  ex_vga_setfg(150,220,255,255); ex_vga_print(11, 6, "// DEPLOYING //");
  ex_vga_setfg(220,220,230,255); ex_vga_print(11, 8, mid);

  if (g_deploy_t <= 0.0f) {   // done: stow the dropship, hand over control
    g_deploy_t = 0.0f;
    if (g_ship_model) g_ship_model->position[1]=-100000.0f;
    for (int i=0;i<2;i++) if(g_ship_glow[i]) g_ship_glow[i]->position[1]=-100000.0f;
  }
}

static float g_prev_cam_y = 3.0f, g_prev_cam_d = 9.0f;

// ---- multiple cameras with separate persistent state -----------------------
// One shared ex_fps_camera is rendered, but each view MODE (in-match, front-end
// menu, builder/customize) keeps its own saved camera so switching modes never
// inherits "whatever was on screen last". On entering a mode we restore its
// saved camera (or leave the mode's own setup code to place it fresh) and clear
// any transient smoothing so there's no sweep from a stale pose.
static void use_camera(cam_mode_t m)
{
  if ((int)m == g_cam_active) return;
  if (g_cam_active >= 0) {                       // save the outgoing camera
    cam_state_t *o = &g_cam_state[g_cam_active];
    memcpy(o->position, camera->position, sizeof(vec3));
    memcpy(o->front, camera->front, sizeof(vec3));
    memcpy(o->up, camera->up, sizeof(vec3));
    o->yaw=camera->yaw; o->pitch=camera->pitch; o->fov=camera->fov; o->inited=1;
  }
  cam_state_t *n = &g_cam_state[m];
  if (n->inited) {                               // restore this mode's own camera
    memcpy(camera->position, n->position, sizeof(vec3));
    memcpy(camera->front, n->front, sizeof(vec3));
    memcpy(camera->up, n->up, sizeof(vec3));
    camera->yaw=n->yaw; camera->pitch=n->pitch; camera->fov=n->fov;
  }
  // clear the builder's turntable smoothing so it snaps to its framing on entry
  if (m == CAM_BUILDER) { g_prev_cam_y = -999.0f; g_prev_cam_d = -999.0f; }
  g_cam_active = (int)m;
}

static void builder_preview(float dt)
{
  // hide arena/enemies/ship; show a single idle bot rotating on a plinth
  for (int i=0;i<EX_MAX_ENEMIES;i++){ hide_body(g_enemy_bodies[i]); if(g_enemy_weapons[i]) g_enemy_weapons[i]->position[1]=-100000.0f; }
  for (int i=0;i<EX_MAP_MAX_VOLS;i++){ g_vol_models[i]->position[1]=-100000.0f; g_rock_models[i]->position[1]=-100000.0f; g_spire_models[i]->position[1]=-100000.0f; }
  if (level_model) level_model->position[1]=-100000.0f;
  for (int i=0;i<NUM_THEMES;i++) if(g_theme_models[i]) g_theme_models[i]->position[1]=-100000.0f;
  if (g_ship_model) g_ship_model->position[1]=-100000.0f;
  for (int i=0;i<2;i++) if(g_ship_glow[i]) g_ship_glow[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_PLANETS;i++) if(g_planets[i]) g_planets[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_LAUNCH;i++) if(g_launch_models[i]) g_launch_models[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_DEF_TURRETS;i++) if(g_deft_models[i]) g_deft_models[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_LIQUID;i++) if(g_liquid[i]) g_liquid[i]->position[1]=-100000.0f;

  apply_theme_lighting("cybertron", 1.0f);   // moody backdrop lighting
  g_active_sky = 3;                          // cyber space sky
  ex_clear_color[0]=0.03f; ex_clear_color[1]=0.03f; ex_clear_color[2]=0.06f;
  ex_fog_density = 0.0f;
  if (g_key_light){ g_key_light->position[0]=8; g_key_light->position[1]=14; g_key_light->position[2]=-6; }
  if (g_form_light) g_form_light->position[1]=-1000.0f;
  if (g_boss_light) g_boss_light->position[1]=-1000.0f;

  // build params from the live loadout -- normalize percentages the same way the
  // deploy path does (/100 and chassis muls), or the preview balloons ~100x.
  { const ex_chassis_info_t *ch = ex_chassis_info((ex_chassis_t)g_menu.loadout.chassis);
    g_build.height=(g_menu.loadout.height/100.0f)*ch->h_mul; g_build.bulk=(g_menu.loadout.bulk/100.0f)*ch->bulk_mul; g_build.head=(g_menu.loadout.head/100.0f)*ch->head_mul;
    g_build.shoulders=(g_menu.loadout.shoulders/100.0f)*ch->shoulder_mul; g_build.arm=(g_menu.loadout.arm/100.0f)*ch->arm_mul; g_build.leg=(g_menu.loadout.leg/100.0f)*ch->leg_mul;
    g_build.accent_element=(int)g_menu.loadout.accent;
    g_build.paint=g_menu.loadout.paint; g_build.finish=g_menu.loadout.finish; g_build.alt_mode=g_menu.loadout.vehicle % EX_VEH_COUNT; }

  // slow turntable
  float spin = g_anim_t*0.5f;
  vec3 origin = {0,0,0};
  int show_vehicle = (g_menu.state==EX_GS_CUSTOMIZE && g_menu.cursor==7);
  if (show_vehicle) {
    ex_part_pose_t vp[EX_VEHICLE_MAX_PARTS]; int nvp;
    ex_vehicle_layout(vp, EX_VEHICLE_MAX_PARTS, &nvp, (ex_vehicle_type)(g_menu.loadout.vehicle%EX_VEH_COUNT),
                      &g_build, 1.2f, spin, 2.0f, (int)g_menu.loadout.accent, EX_BODY_PLAYER);
    float cyw=cosf(spin), syw=sinf(spin);
    for (int i=0;i<EX_ROBOT_PARTS && i<nvp;i++){ ex_model_t *pm=g_player_parts[i];
      float ox=vp[i].offset[0], oz=vp[i].offset[2];
      pm->position[0]=ox*cyw-oz*syw; pm->position[1]=vp[i].offset[1]; pm->position[2]=ox*syw+oz*cyw;
      pm->rotation[1]=spin*57.2957795f; pm->scale3[0]=vp[i].sx; pm->scale3[1]=vp[i].sy; pm->scale3[2]=vp[i].sz;
      pm->tint[0]=vp[i].color[0]/255.0f; pm->tint[1]=vp[i].color[1]/255.0f; pm->tint[2]=vp[i].color[2]/255.0f; }
    for (int i=nvp;i<EX_ROBOT_PARTS;i++) g_player_parts[i]->position[1]=-100000.0f;
  } else {
    pose_body(g_player_parts, origin, 0, EX_BODY_PLAYER, 1.2f, 0.0f, 0.0f, (int)g_menu.loadout.accent, -spin, &g_build, player_signature());
  }

  // focus height per edited section -> camera pans to that part
  float focus_y = 2.4f, dist = 9.0f;
  if (g_menu.state==EX_GS_CUSTOMIZE) {
    switch (g_menu.cursor) {
      case 0: focus_y=2.4f; dist=10.0f; break;   // height (whole)
      case 1: focus_y=2.6f; dist=7.0f; break;    // bulk (torso)
      case 2: focus_y=4.2f; dist=5.0f; break;    // head
      case 3: focus_y=3.4f; dist=6.0f; break;    // shoulders
      case 4: focus_y=2.8f; dist=6.0f; break;    // arms
      case 5: focus_y=1.1f; dist=6.0f; break;    // legs
      case 7: focus_y=2.2f; dist=12.0f; break;   // vehicle (pull back + up)
      case 11:focus_y=2.8f; dist=5.5f; break;    // weapon
      default: focus_y=2.4f; dist=9.0f; break;
    }
  }
  // smooth pan
  if (g_prev_cam_y < -900.0f) { g_prev_cam_y = focus_y; g_prev_cam_d = dist; }  // fresh entry: snap
  g_prev_cam_y += (focus_y - g_prev_cam_y) * fminf(dt*4.0f, 1.0f);
  g_prev_cam_d += (dist   - g_prev_cam_d) * fminf(dt*4.0f, 1.0f);
  float orbit = g_anim_t*0.25f;
  // cinematic 3/4 view from ABOVE, looking down at the subject (never underneath)
  vec3 cam = { sinf(orbit)*g_prev_cam_d, g_prev_cam_y + g_prev_cam_d*0.55f + 2.0f, cosf(orbit)*g_prev_cam_d };
  memcpy(camera->position, cam, sizeof(vec3));
  vec3 look={0, g_prev_cam_y*0.7f, 0}, fwd; vec3_sub(fwd,look,cam); vec3_norm(fwd,fwd);
  camera->front[0]=fwd[0]; camera->front[1]=fwd[1]; camera->front[2]=fwd[2];
  pl->position[0]=6; pl->position[1]=12; pl->position[2]=6;
  for (int i=0;i<NUM_SKY;i++){ if(i==g_active_sky){ g_sky_models[i]->position[0]=cam[0]; g_sky_models[i]->position[1]=cam[1]; g_sky_models[i]->position[2]=cam[2]; } else g_sky_models[i]->position[1]=-100000.0f; }
  // a planet hangs behind the bot as a backdrop (Cybertron)
  if (g_planets[2]){ g_planets[2]->position[0]=30; g_planets[2]->position[1]=18; g_planets[2]->position[2]=45;
    g_planets[2]->scale=16.0f; g_planets[2]->rotation[1]=g_anim_t*3.0f;
    g_planets[2]->tint[0]=g_planets[2]->tint[1]=g_planets[2]->tint[2]=1.0f; }
  for (int i=0;i<NUM_PLANETS;i++) if(i!=2 && g_planets[i]) g_planets[i]->position[1]=-100000.0f;
}

static void menu_backdrop(float dt)
{
  // hide the arena/player/enemies so only the ship + starfield show
  hide_body(g_player_parts);
  for (int i=0;i<EX_MAX_ENEMIES;i++){ hide_body(g_enemy_bodies[i]); if(g_enemy_weapons[i]) g_enemy_weapons[i]->position[1]=-100000.0f; }
  for (int i=0;i<10;i++) if(g_weapon_models[i]) g_weapon_models[i]->position[1]=-100000.0f;
  for (int i=0;i<EX_MAP_MAX_VOLS;i++){ g_vol_models[i]->position[1]=-100000.0f; g_rock_models[i]->position[1]=-100000.0f; g_spire_models[i]->position[1]=-100000.0f; }
  if (level_model) level_model->position[1]=-100000.0f;
  for (int i=0;i<NUM_THEMES;i++) if(g_theme_models[i]) g_theme_models[i]->position[1]=-100000.0f;
  for (int i=0;i<DETAIL_MODELS;i++) g_detail_models[i]->position[1]=-100000.0f;
  for (int i=0;i<MAP_DETAIL;i++) if(g_map_detail[i]) g_map_detail[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_LAUNCH;i++) if(g_launch_models[i]) g_launch_models[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_DEF_TURRETS;i++) if(g_deft_models[i]) g_deft_models[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_LIQUID;i++) if(g_liquid[i]) g_liquid[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_OBJ_PADS;i++) if(g_obj_pads[i]) g_obj_pads[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_CONSOLES;i++) if(g_consoles[i]) g_consoles[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_CLUTTER;i++) if(g_clutter[i]) g_clutter[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_DEBRIS;i++) if(g_debris_models[i]) g_debris_models[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_SHOCKRINGS;i++) if(g_shockrings[i]) g_shockrings[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_CRATERS;i++) if(g_craters[i]) g_craters[i]->position[1]=-100000.0f;
  for (int i=0;i<NUM_HAZARDS;i++) if(g_haz_models[i]) g_haz_models[i]->position[1]=-100000.0f;
  if(g_landing_ring) g_landing_ring->position[1]=-100000.0f;
  g_landing_active = 0; g_warp_wave.armed = 0;   // clear any inbound gravity wave
  g_tsunami.active = 0; g_tsunami_cd = 0.0f; g_arc_cd = 0.0f; g_cyclone_on = 0; g_cyclone_cd = 0.0f; g_thorns_on = 0; g_thorns_cd = 0.0f;
  memset(&g_starfire, 0, sizeof(g_starfire));
  if(g_star_orb) g_star_orb->position[1]=-100000.0f;
  hide_body(g_ally_parts); g_ally_present = 0;
  ex_spearcon_reset(&g_spearcons); g_spearcon_cd = 0.0f;
  for (int i=0;i<EX_SPEARCON_MAX;i++) if(g_spearcon_models[i]) g_spearcon_models[i]->position[1]=-100000.0f;
  humans_hide();
  for (int i=0;i<NUM_MOTES;i++) if(g_motes[i]) g_motes[i]->position[1]=-100000.0f;
  for (int i=0;i<g_arena.count;i++) if(g_feature_models[i]) g_feature_models[i]->position[1]=-100000.0f;
  for (int i=0;i<EX_MAX_PICKUPS;i++) if(g_pickup_models[i]) g_pickup_models[i]->position[1]=-100000.0f;
  if (g_muzzle) g_muzzle->position[1]=-100000.0f;
  if (g_cape) g_cape->position[1]=-100000.0f;
  // hide the periphery decoration too (these were the stray "boxes")
  for (int i=0;i<DECO_ROCK;i++) if(g_deco_rock[i]) g_deco_rock[i]->position[1]=-100000.0f;
  for (int i=0;i<DECO_TALL;i++) if(g_deco_tall[i]) g_deco_tall[i]->position[1]=-100000.0f;
  for (int i=0;i<DECO_ARCH;i++) if(g_deco_arch[i]) g_deco_arch[i]->position[1]=-100000.0f;
  for (int i=0;i<DECO_TOWER;i++) if(g_deco_tower[i]) g_deco_tower[i]->position[1]=-100000.0f;

  // ALWAYS deep space for the front-end (the ground skies looked wrong here).
  if (g_key_light){ g_key_light->color[0]=1.1f; g_key_light->color[1]=1.1f; g_key_light->color[2]=1.2f; }
  if (g_fill_light){ g_fill_light->color[0]=0.25f; g_fill_light->color[1]=0.25f; g_fill_light->color[2]=0.35f; }
  ex_clear_color[0]=0.02f; ex_clear_color[1]=0.02f; ex_clear_color[2]=0.05f;
  g_active_sky = 0;                            // sky_space
  ex_fog_density = 0.0f;
  if (g_key_light){ g_key_light->position[0]=10; g_key_light->position[1]=20; g_key_light->position[2]=-10; }
  if (g_form_light) g_form_light->position[1]=-1000.0f;
  if (g_boss_light) g_boss_light->position[1]=-1000.0f;

  // a slow cinematic camera drift; the ship cruises ahead of us
  float t = g_anim_t;
  vec3 cam = { sinf(t*0.15f)*3.0f, 3.0f + sinf(t*0.22f)*0.6f, -14.0f };
  memcpy(camera->position, cam, sizeof(vec3));
  vec3 look = { 0.0f + sinf(t*0.1f)*2.0f, 2.0f, 6.0f };
  vec3 fwd; vec3_sub(fwd, look, cam); vec3_norm(fwd, fwd);
  camera->front[0]=fwd[0]; camera->front[1]=fwd[1]; camera->front[2]=fwd[2];

  // keep the space skybox centred on the camera
  for (int i=0;i<NUM_SKY;i++){
    if (i==g_active_sky){ g_sky_models[i]->position[0]=cam[0]; g_sky_models[i]->position[1]=cam[1]; g_sky_models[i]->position[2]=cam[2]; }
    else g_sky_models[i]->position[1]=-100000.0f;
  }
  // planets hang in the distance, each slowly spinning, at fixed world spots
  { const float px[NUM_PLANETS]={ -120, 150,  45, -70, 110 };
    const float py[NUM_PLANETS]={   40,  72, -22,  95, -50 };
    const float pz[NUM_PLANETS]={  160, 270, 120, 320, 220 };
    const float ps[NUM_PLANETS]={   30,  58,  26,  16,  12 };
    // distinct worlds: earth blue-green, jupiter tan, cybertron steel, mars rust, pluto ice
    const float pt[NUM_PLANETS][3]={ {0.45f,0.7f,1.0f}, {0.95f,0.82f,0.6f}, {0.7f,0.78f,0.9f},
                                     {1.0f,0.5f,0.35f}, {0.8f,0.88f,1.0f} };
    for (int i=0;i<NUM_PLANETS;i++){ ex_model_t *pl2=g_planets[i];
      pl2->position[0]=px[i] + sinf(g_anim_t*0.05f + i)*4.0f;   // faint parallax drift
      pl2->position[1]=py[i] + sinf(g_anim_t*0.04f + i*2.0f)*2.0f;
      pl2->position[2]=pz[i];
      pl2->scale=ps[i]; pl2->rotation[1]=g_anim_t*(4.0f+i*1.5f); pl2->rotation[0]=18.0f;   // tilted axis
      pl2->tint[0]=pt[i][0]; pl2->tint[1]=pt[i][1]; pl2->tint[2]=pt[i][2]; }
  }

  // the ship: gentle bank + bob, cruising just ahead
  if (g_ship_model){
    g_ship_model->position[0]=sinf(t*0.3f)*2.0f;
    g_ship_model->position[1]=2.0f + sinf(t*0.5f)*0.5f;
    g_ship_model->position[2]=5.0f + sinf(t*0.18f)*1.5f;         // slow in/out cruise
    g_ship_model->rotation[1]=180.0f + sinf(t*0.3f)*12.0f;       // face away, banking harder
    g_ship_model->rotation[2]=sinf(t*0.4f)*10.0f;                // roll into the bank
    g_ship_model->rotation[0]=sinf(t*0.5f)*3.0f;                 // slight pitch bob
    g_ship_model->scale=1.35f;
    g_ship_model->tint[0]=0.62f; g_ship_model->tint[1]=0.70f; g_ship_model->tint[2]=0.86f;  // brighter hull
    for (int i=0;i<2;i++) if (g_ship_glow[i]){
      float pulse=0.7f+0.3f*sinf(t*7.0f+i*1.3f);
      // engine glow banks with the ship
      float bank = sinf(t*0.3f)*12.0f * 0.0174533f;
      float ex_=((i?0.8f:-0.8f));
      g_ship_glow[i]->position[0]=g_ship_model->position[0]+ex_*cosf(bank);
      g_ship_glow[i]->position[1]=g_ship_model->position[1]-0.1f+ex_*sinf(bank);
      g_ship_glow[i]->position[2]=g_ship_model->position[2]-4.2f;
      g_ship_glow[i]->scale=(0.32f+0.10f*pulse)*1.0f;            // flare with thrust
      g_ship_glow[i]->tint[0]=0.55f*pulse; g_ship_glow[i]->tint[1]=0.85f*pulse; g_ship_glow[i]->tint[2]=1.0f;
    }
  }
}

static void humans_hide(void){
  for (int i=0;i<NUM_HUMANS;i++) for(int k=0;k<4;k++) if(g_human_parts[i][k]) g_human_parts[i][k]->position[1]=-1000.0f;
}
static void spawn_humans(void){
  unsigned r = 0x77A9 ^ (unsigned)(g_menu.mission_index*53 + g_menu.map_index*29 + 3);
  int count = 5 + (g_menu.in_campaign ? g_menu.mission_index : 2);   // more survivors deeper in
  if (count>NUM_HUMANS) count=NUM_HUMANS;
  g_survivors_total=count; g_survivors_lost=0;
  for (int i=0;i<NUM_HUMANS;i++){
    if (i<count){
      r=r*1664525u+1013904223u; float ax=(((r>>9)&0x3FF)/1023.0f*2-1)*550.0f;
      r=r*1664525u+1013904223u; float az=(((r>>9)&0x3FF)/1023.0f*2-1)*550.0f;
      g_humans[i].active=1; g_humans[i].fleeing=0; g_humans[i].phase=0; g_humans[i].retimer=0; g_humans[i].downed=0;
      g_humans[i].pos[0]=ax; g_humans[i].pos[1]=0; g_humans[i].pos[2]=az;
      g_humans[i].vel[0]=g_humans[i].vel[1]=g_humans[i].vel[2]=0;
      g_humans[i].tgt[0]=ax; g_humans[i].tgt[1]=az;
    } else g_humans[i].active=0;
  }
}
// wander + flee-from-titans AI, then pose each human as a small walking figure
static void humans_update(float dt){
  for (int i=0;i<NUM_HUMANS;i++){
    human_npc_t *h=&g_humans[i];
    if (!h->active){ for(int k=0;k<4;k++) g_human_parts[i][k]->position[1]=-1000.0f; continue; }
    // find the nearest THREAT (a live bot enemy, or the player if a titan)
    float best=1e9f, tx=0, tz=0; int threat=0; float enemy_best=1e9f;
    for (int q=0;q<g_enc.count;q++){ ex_enemy_t *en=&g_enc.enemies[q]; if(en->state==EX_ENEMY_DEAD) continue;
      float dx=en->position[0]-h->pos[0], dz=en->position[2]-h->pos[2]; float d2=dx*dx+dz*dz;
      if (d2<enemy_best) enemy_best=d2;
      if (d2<best){ best=d2; tx=en->position[0]; tz=en->position[2]; threat=1; } }
    // a titan-scale PLAYER is also something small people run from
    if (!g_human_scale && e){ float dx=e->position[0]-h->pos[0], dz=e->position[2]-h->pos[2]; float d2=dx*dx+dz*dz;
      if (d2<best){ best=d2; tx=e->position[0]; tz=e->position[2]; threat=1; } }
    h->fleeing = (threat && best < 60.0f*60.0f);
    // --- downed / rescue window: an enemy reaching a human knocks it down; you
    //     have a few seconds to drive the attacker off before it's lost ---
    if (h->downed > 0.0f) {
      h->downed -= dt;
      int enemy_near = (enemy_best < 14.0f*14.0f);
      if (!enemy_near) {                       // rescued -- the threat is gone
        h->downed = 0.0f; h->retimer = 0.0f;
        snprintf(g_shot_msg,sizeof(g_shot_msg),"survivor rescued"); g_shot_timer=1.2f;
      } else if (h->downed <= 0.0f) {          // ran out of time with an enemy on them
        uint8_t rc[3]={200,60,60}; ex_vfx_spawn_impact(&g_vfx,(vec3){h->pos[0],1.0f,h->pos[2]},14,rc);
        h->active=0; g_survivors_lost++;
        snprintf(g_shot_msg,sizeof(g_shot_msg),"a survivor was lost! (%d)", g_survivors_lost); g_shot_timer=2.0f;
        for(int k=0;k<4;k++) g_human_parts[i][k]->position[1]=-1000.0f;
        continue;
      }
      // prone pose on the ground; blink red for distress
      float blink=(sinf(g_anim_t*10.0f)>0)?1.0f:0.4f;
      ex_model_t **Pd=g_human_parts[i];
      Pd[0]->position[0]=h->pos[0]; Pd[0]->position[1]=0.25f; Pd[0]->position[2]=h->pos[2]; Pd[0]->scale=0.28f;
      Pd[0]->tint[0]=1.0f*blink;Pd[0]->tint[1]=0.3f*blink;Pd[0]->tint[2]=0.3f*blink;
      Pd[1]->position[0]=h->pos[0]; Pd[1]->position[1]=0.2f; Pd[1]->position[2]=h->pos[2];
      Pd[1]->scale3[0]=0.55f;Pd[1]->scale3[1]=0.18f;Pd[1]->scale3[2]=0.35f; Pd[1]->rotation[1]=0;
      Pd[1]->tint[0]=0.30f;Pd[1]->tint[1]=0.42f;Pd[1]->tint[2]=0.55f;
      Pd[2]->position[1]=-1000.0f; Pd[3]->position[1]=-1000.0f;
      continue;
    }
    if (enemy_best < 4.0f*4.0f) { h->downed = 5.0f;   // knocked down (rescue window opens)
      snprintf(g_shot_msg,sizeof(g_shot_msg),"survivor down -- drive it off!"); g_shot_timer=1.5f;
      continue;
    }
    float spd;
    if (h->fleeing){                       // run directly away from the nearest titan
      float dx=h->pos[0]-tx, dz=h->pos[2]-tz; float d=sqrtf(dx*dx+dz*dz)+0.001f;
      h->tgt[0]=h->pos[0]+dx/d*40.0f; h->tgt[1]=h->pos[2]+dz/d*40.0f; spd=9.0f;
    } else {                               // idle wander toward a roaming target
      h->retimer-=dt;
      if (h->retimer<=0.0f){ unsigned r=(unsigned)(g_anim_t*1000)+i*777; r=r*1103515245u+12345u;
        h->tgt[0]=h->pos[0]+(((r>>9)&0xFF)/255.0f*2-1)*40.0f;
        h->tgt[1]=h->pos[2]+(((r>>16)&0xFF)/255.0f*2-1)*40.0f; h->retimer=2.0f+((r>>3)&3); }
      spd=3.2f;
    }
    // steer toward target
    float dx=h->tgt[0]-h->pos[0], dz=h->tgt[1]-h->pos[2]; float d=sqrtf(dx*dx+dz*dz);
    if (d>0.5f){ h->vel[0]=dx/d*spd; h->vel[2]=dz/d*spd; } else { h->vel[0]*=0.8f; h->vel[2]*=0.8f; }
    h->pos[0]+=h->vel[0]*dt; h->pos[2]+=h->vel[2]*dt;
    float mv=sqrtf(h->vel[0]*h->vel[0]+h->vel[2]*h->vel[2]);
    h->phase += mv*dt*1.6f;
    // pose the 4-part figure (facing travel), tiny relative to the titans
    float yaw = (mv>0.1f)? atan2f(h->vel[2],h->vel[0]) : 0.0f;
    float HV=0.9f, bob=sinf(h->phase*2.0f)*0.06f*fminf(mv/3.0f,1.0f);
    ex_model_t **P=g_human_parts[i];
    // head
    P[0]->position[0]=h->pos[0]; P[0]->position[1]=HV*1.75f+bob; P[0]->position[2]=h->pos[2];
    P[0]->scale=HV*0.32f; P[0]->tint[0]=0.80f;P[0]->tint[1]=0.66f;P[0]->tint[2]=0.55f;     // skin
    // torso
    P[1]->position[0]=h->pos[0]; P[1]->position[1]=HV*1.15f+bob; P[1]->position[2]=h->pos[2];
    P[1]->scale3[0]=HV*0.42f;P[1]->scale3[1]=HV*0.60f;P[1]->scale3[2]=HV*0.30f; P[1]->rotation[1]=yaw*57.2958f;
    P[1]->tint[0]=0.30f;P[1]->tint[1]=0.42f;P[1]->tint[2]=0.55f;                            // jacket
    // legs (bob with gait)
    P[2]->position[0]=h->pos[0]; P[2]->position[1]=HV*0.5f; P[2]->position[2]=h->pos[2];
    P[2]->scale3[0]=HV*0.34f;P[2]->scale3[1]=HV*0.55f;P[2]->scale3[2]=HV*0.30f; P[2]->rotation[1]=yaw*57.2958f;
    P[2]->rotation[0]=sinf(h->phase*2.0f)*14.0f*fminf(mv/3.0f,1.0f);
    P[2]->tint[0]=0.22f;P[2]->tint[1]=0.24f;P[2]->tint[2]=0.30f;
    // pack on the back
    float bx=-cosf(yaw)*HV*0.28f, bz=-sinf(yaw)*HV*0.28f;
    P[3]->position[0]=h->pos[0]+bx; P[3]->position[1]=HV*1.2f+bob; P[3]->position[2]=h->pos[2]+bz;
    P[3]->scale3[0]=HV*0.22f;P[3]->scale3[1]=HV*0.34f;P[3]->scale3[2]=HV*0.18f; P[3]->rotation[1]=yaw*57.2958f;
    P[3]->tint[0]=0.18f;P[3]->tint[1]=0.20f;P[3]->tint[2]=0.22f;
  }
}

void game_update(double dt, double ft)
{
  g_anim_t += dt;
  // apply the frame-cap / vsync setting when it changes
  { extern int ex_frame_cap; static int applied = -999;
    if (g_menu.gfx.frame_cap != applied) {
      applied = g_menu.gfx.frame_cap;
      ex_frame_cap = g_menu.gfx.frame_cap;
      SDL_GL_SetSwapInterval(g_menu.gfx.frame_cap == 0 ? 1 : 0);  // 0=vsync
    }
  }
  if (g_cut_active) { cutscene_run((float)dt); return; }
  {
  }
  { extern float ex_vga_scale; ex_vga_scale = g_menu.gfx.gui_scale/100.0f; }

  // Esc pauses while actively playing; in menus, menu_input handles ESC=back
  if (g_menu.state == EX_GS_PLAYING && ex_keys_down[SDL_SCANCODE_H]) {
    ex_keys_down[SDL_SCANCODE_H]=0; g_human_scale=!g_human_scale;
    if (g_human_scale){ e->radius[0]=0.28f; e->radius[1]=0.5f; e->radius[2]=0.28f; }
    else              { e->radius[0]=0.5f;  e->radius[1]=1.0f; e->radius[2]=0.5f;  }
    e->position[1]+=2.0f;   // nudge up so the smaller ellipsoid re-settles on the floor
    snprintf(g_shot_msg,sizeof(g_shot_msg), g_human_scale?"human scale":"titan scale"); g_shot_timer=1.5f;
  }
  if (g_menu.state == EX_GS_PLAYING && ex_keys_down[SDL_SCANCODE_ESCAPE]) {
    ex_menu_pause_toggle(&g_menu);
    ex_keys_down[SDL_SCANCODE_ESCAPE] = 0;
  }

  if (g_menu.state != EX_GS_PLAYING) {
    // PAUSED / RESULTS keep the live world frozen behind the overlay -- running
    // the cinematic backdrop here would relocate all the map geometry and leave
    // it hidden after unpausing. Only the true front-end menus swap the scene.
    if (g_menu.state == EX_GS_PAUSED || g_menu.state == EX_GS_RESULTS) {
      menu_input();
      draw_menu();
      return;
    }
    if (g_menu.state == EX_GS_BUILDER || g_menu.state == EX_GS_CUSTOMIZE) {
      use_camera(CAM_BUILDER);                 // builder gets its own camera
      builder_preview((float)dt);              // live bot preview + camera pan
    } else {
      use_camera(CAM_MENU);                    // front-end gets its own camera
      menu_backdrop((float)dt);                // cinematic ship-in-space menu
    }
    menu_input();
    draw_menu();
    return;
  }

  if (g_deploy_t > 0.0f) { deploy_sequence((float)dt); return; }
  use_camera(CAM_GAME);           // in-match camera, restored independently of the menus
  ex_fps_camera_update(camera);   // apply mouse-look FIRST so the body faces the current aim (not last frame's)
  ex_entity_update(e, dt);
  resolve_player_vs_volumes(e, (float)dt);
  { float ay2 = atan2f(camera->front[2], camera->front[0]);
    vehicle_physics_update(e, (float)dt, (g_avatar_xform->mode==EX_MODE_VEHICLE), ay2); }
  g_pe = e; g_player_pos[0]=e->position[0]; g_player_pos[1]=e->position[1]; g_player_pos[2]=e->position[2];
  // spark-resonance loudness: decays toward quiet, rises with how fast you move.
  { float psp=sqrtf(e->velocity[0]*e->velocity[0]+e->velocity[2]*e->velocity[2]);
    g_spark_loud -= (float)dt*0.9f; if(g_spark_loud<0) g_spark_loud=0;
    float moved = fminf(psp/20.0f, 1.0f); if (moved > g_spark_loud) g_spark_loud = moved;
    // if you're loud AND something is locked onto your spark, the screen throbs
    // faintly -- the resonance reaching back toward you.
    int hunted=0; for(int hi=0;hi<g_enc.count;hi++){ if(g_enc.enemies[hi].state==EX_ENEMY_DEAD) continue;
      if(g_enemy_aware[hi] && (g_enc.enemies[hi].kind==EX_ENEMY_CORRUPTED||g_enc.enemies[hi].kind==EX_ENEMY_APEX)){ hunted=1; break; } }
    if (hunted && g_spark_loud>0.4f) g_cam_trauma += g_spark_loud*0.012f*(0.5f+0.5f*sinf(g_anim_t*4.5f)); }
  // ---- PERF: distance + behind-camera culling for the big static populations.
  //      Off-screen clutter/craters/features are parked below the world so the
  //      renderer skips them -- essential now the arenas are much larger. ----
  { float cx=camera->position[0], cz=camera->position[2];
    float fx=camera->front[0], fz=camera->front[2];
    const float CULL2 = 640.0f*640.0f;
    for (int i=0;i<NUM_CLUTTER;i++){ ex_model_t *m=g_clutter[i]; if(!m) continue;
      float ry=g_clutter_realy[i]; if (ry<-900.0f) continue;     // wasn't placed
      float dx=m->position[0]-cx, dz=m->position[2]-cz; float d2=dx*dx+dz*dz;
      int behind = (dx*fx+dz*fz) < -30.0f;
      int culled = (d2>CULL2) || (behind && d2>90.0f*90.0f);
      m->position[1] = culled ? -100000.0f : ry;
    }
    for (int i=0;i<NUM_CRATERS;i++){ ex_model_t *m=g_craters[i]; if(!m) continue;
      if (g_crater_realy[i] < -900.0f) continue;
      float dx=m->position[0]-cx, dz=m->position[2]-cz;
      m->position[1] = (dx*dx+dz*dz>CULL2) ? -100000.0f : g_crater_realy[i];
    }
  }
  // SCRAPE: sliding fast across the ground throws sparks on hard materials, dust
  // on soft ones (and nothing much on slick ice -- it just slides).
  if (e->grounded==1) {
    float ay3 = atan2f(camera->front[2], camera->front[0]);
    float rgx=-sinf(ay3), rgz=cosf(ay3);
    float lat = fabsf(e->velocity[0]*rgx + e->velocity[2]*rgz);   // sideways slide speed
    float spd = sqrtf(e->velocity[0]*e->velocity[0]+e->velocity[2]*e->velocity[2]);
    const ex_material_t *m = ex_material(g_ground_mat);
    static float scrape_t=0; scrape_t-=(float)dt;
    if (spd>7.0f && lat>3.5f && scrape_t<=0.0f) {
      scrape_t = 0.05f;
      vec3 cp = { e->position[0], e->position[1]-1.0f, e->position[2] };
      uint8_t col[3]={m->spark[0],m->spark[1],m->spark[2]};
      int n = (m->hardness>0.6f)? 4 : 2;                 // hard -> more, brighter sparks
      if (m->grit>0.5f){ col[0]=200;col[1]=180;col[2]=150; }  // gritty -> dust colour
      ex_vfx_spawn_impact(&g_vfx, cp, n, col);
      if (m->hardness>0.6f && (int)(g_anim_t*8)%3==0) sfx(g_sfx_hit);  // grind/ring
    }
  }
  debris_update((float)dt);
  shockrings_update((float)dt);
  { float psp=sqrtf(e->velocity[0]*e->velocity[0]+e->velocity[2]*e->velocity[2]);
    if (psp>4.0f) kick_debris(e->position[0], e->position[2], 5.0f, psp*0.15f); }
  // FOOTSTEP DUST: a giant striding bot kicks a small material-coloured puff each
  // step (synced to the slow gait), selling the weight. Grounded + on foot only.
  { static float g_last_step_ph = 0.0f;
    int on_foot = (g_avatar_xform->mode != EX_MODE_VEHICLE) && !g_human_scale;
    float psp = sqrtf(e->velocity[0]*e->velocity[0]+e->velocity[2]*e->velocity[2]);
    if (on_foot && e->grounded==1 && psp>3.0f) {
      float ph = g_phase_player;
      // a foot plants at each half-cycle (phase crosses 0 or PI)
      int crossed = ( (g_last_step_ph < 3.14159f) != (ph < 3.14159f) );
      if (crossed) {
        const ex_material_t *m = ex_material(g_ground_mat);
        uint8_t dc[3]={ (uint8_t)(m->spark[0]*0.6f+50), (uint8_t)(m->spark[1]*0.6f+50), (uint8_t)(m->spark[2]*0.6f+50) };
        float side = (ph < 3.14159f) ? 1.0f : -1.0f;
        float fx = e->position[0] - sinf(atan2f(e->velocity[2],e->velocity[0]))*side*1.2f;
        float fz = e->position[2] + cosf(atan2f(e->velocity[2],e->velocity[0]))*side*1.2f;
        ex_vfx_spawn_impact(&g_vfx, (vec3){fx, 0.3f, fz}, 3, dc);
      }
      g_last_step_ph = ph;
    }
  }
  // --- self-repair + energy repair of the microFSM parts ---
  for (int i=0;i<EX_ROBOT_PARTS;i++){
    if (g_part_detached[i]){
      if (g_repair_energy >= 25.0f){ g_repair_energy-=25.0f; g_part_detached[i]=0; g_part_integ[i]=0.45f;
        spawn_debris(g_player_pos[0], g_player_pos[1]+2.0f, g_player_pos[2], 0.1f, 0.3f,0.9f,1.0f, 0.5f);
        snprintf(g_shot_msg,sizeof(g_shot_msg),"part regrown"); g_shot_timer=1.0f; }
    } else if (g_part_integ[i] < 1.0f){
      // minor damage self-heals, but only partway (to 0.7) without repair energy
      float cap = (g_repair_energy>0.0f) ? 1.0f : 0.7f;
      if (g_part_integ[i] < cap){ g_part_integ[i]+=0.05f*(float)dt; if(g_part_integ[i]>cap)g_part_integ[i]=cap; }
    }
  }
  // owning an objective pad trickles repair energy (map control = a foothold)
  { int held=0; for(int i=0;i<NUM_OBJ_PADS;i++) if(g_obj_owner[i]==1) held++;
    if (held>0) g_repair_energy = fminf(200.0f, g_repair_energy + held*4.0f*(float)dt); }
  // launch pads: running onto one near the ground flings you skyward
  for (int i=0;i<g_launch_n;i++){
    float dx=e->position[0]-g_launch[i][0], dz=e->position[2]-g_launch[i][2];
    if (dx*dx+dz*dz < 16.0f && e->position[1] < 4.0f && e->velocity[1] <= 1.0f) {
      e->velocity[1] = 62.0f;
      e->velocity[0] += camera->front[0]*20.0f;
      e->velocity[2] += camera->front[2]*20.0f;
      uint8_t lc[3]={120,255,230}; ex_vfx_spawn_impact(&g_vfx, e->position, 6, lc);
      snprintf(g_shot_msg,sizeof(g_shot_msg),"LAUNCH!"); g_shot_timer=0.8f; sfx(g_sfx_launch);
    }
  }
  // atmospheric motes: slow drifting embers/dust that wrap around the player,
  // tinted by theme -- gives the air depth and motion.
  for (int i=0;i<NUM_MOTES;i++){ if(!g_motes[i])continue;
    g_mote_p[i][1] += (0.4f + (i%5)*0.15f) * dt;             // rise slowly
    g_mote_p[i][0] += sinf(g_anim_t*0.3f + i)*0.3f*dt;       // gentle sway
    if (g_mote_p[i][1] > 32.0f) g_mote_p[i][1] -= 34.0f;     // wrap top->bottom
    float px=e->position[0]+g_mote_p[i][0], pz=e->position[2]+g_mote_p[i][2];
    // wrap horizontally so motes stay in a box around the player
    float rx=px-e->position[0], rz=pz-e->position[2];
    if (rx> 30.0f) g_mote_p[i][0]-=60.0f; if (rx<-30.0f) g_mote_p[i][0]+=60.0f;
    if (rz> 30.0f) g_mote_p[i][2]-=60.0f; if (rz<-30.0f) g_mote_p[i][2]+=60.0f;
    g_motes[i]->position[0]=e->position[0]+g_mote_p[i][0];
    g_motes[i]->position[1]=g_mote_p[i][1];
    g_motes[i]->position[2]=e->position[2]+g_mote_p[i][2];
    float tw=0.4f+0.6f*fabsf(sinf(g_anim_t*1.5f+i*0.6f));
    g_motes[i]->scale3[0]=g_motes[i]->scale3[1]=g_motes[i]->scale3[2]=0.10f+0.05f*tw;
    g_motes[i]->tint[0]=g_mote_col[0]*tw; g_motes[i]->tint[1]=g_mote_col[1]*tw; g_motes[i]->tint[2]=g_mote_col[2]*tw;
  }
  // control-panel consoles pulse amber inside the interior rooms
  for (int i=0;i<NUM_CONSOLES;i++){ if(!g_consoles[i])continue; float cpz=0.5f+0.5f*fabsf(sinf(g_anim_t*3.0f+i*1.3f));
    g_consoles[i]->tint[0]=1.0f*cpz; g_consoles[i]->tint[1]=0.7f*cpz; g_consoles[i]->tint[2]=0.2f*cpz; }
  // ---- capturable objective pads: stand on one to take it; hold to score ----
  for (int i=0;i<NUM_OBJ_PADS;i++){
    if(!g_obj_pads[i]) continue;
    float dx=e->position[0]-g_obj_pos[i][0], dz=e->position[2]-g_obj_pos[i][2];
    float pdist2 = dx*dx+dz*dz;
    float CAPR = g_human_scale ? 10.0f : 22.0f;   // capture radius (tighter for humans)
    int player_on = (pdist2 < CAPR*CAPR);
    // is any enemy contesting the pad?
    int enemy_on = 0;
    for (int q=0;q<g_enc.count && !enemy_on;q++){ ex_enemy_t *en=&g_enc.enemies[q];
      if (en->state==EX_ENEMY_DEAD) continue;
      float ex_=en->position[0]-g_obj_pos[i][0], ez=en->position[2]-g_obj_pos[i][2];
      if (ex_*ex_+ez*ez < CAPR*CAPR) enemy_on=1; }
    // advance/revert capture
    if (player_on && !enemy_on)      g_obj_cap[i] += dt*0.6f;
    else if (enemy_on && !player_on) g_obj_cap[i] -= dt*0.5f;
    if (g_obj_cap[i]>1.0f) g_obj_cap[i]=1.0f;
    if (g_obj_cap[i]<0.0f) g_obj_cap[i]=0.0f;
    int was = g_obj_owner[i];
    g_obj_owner[i] = (g_obj_cap[i] >= 1.0f) ? 1 : (g_obj_cap[i] <= 0.0f ? 0 : was);
    if (g_obj_owner[i]==1 && was==0){ snprintf(g_shot_msg,sizeof(g_shot_msg),"objective captured"); g_shot_timer=1.5f; g_cam_trauma+=0.15f; }
    if (g_obj_owner[i]==0 && was==1){ snprintf(g_shot_msg,sizeof(g_shot_msg),"objective lost!"); g_shot_timer=1.5f; }
    // owned pads accrue score + trickle-heal the holder standing on them
    if (g_obj_owner[i]==1){ g_obj_score += dt; if(player_on) g_player_hp=fminf(g_player_max_hp,g_player_hp+dt*4.0f); }
    // colour: neutral grey -> (capturing) amber contest -> owned cyan
    float pz = 0.6f+0.4f*sinf(g_anim_t*2.5f+i*2.1f);
    if (g_obj_owner[i]==1){ g_obj_pads[i]->tint[0]=0.15f*pz; g_obj_pads[i]->tint[1]=0.95f*pz; g_obj_pads[i]->tint[2]=1.0f*pz; }
    else if (enemy_on && player_on){ float fl=(sinf(g_anim_t*18.0f)>0)?1.0f:0.4f; g_obj_pads[i]->tint[0]=1.0f*fl; g_obj_pads[i]->tint[1]=0.6f*fl; g_obj_pads[i]->tint[2]=0.1f*fl; }
    else { float c=g_obj_cap[i]; g_obj_pads[i]->tint[0]=(0.4f+ -0.25f*c)*pz; g_obj_pads[i]->tint[1]=(0.4f+0.55f*c)*pz; g_obj_pads[i]->tint[2]=(0.45f+0.55f*c)*pz; }
  }
  // gently pulse the launch pads
  for (int i=0;i<g_launch_n;i++){ float pulse=0.7f+0.3f*sinf(g_anim_t*4.0f+i);
    g_launch_models[i]->tint[0]=0.4f*pulse; g_launch_models[i]->tint[1]=1.0f*pulse; g_launch_models[i]->tint[2]=0.9f*pulse; }
  // liquid pools shimmer (wave the emissive tint)
  for (int i=0;i<g_liquid_n;i++){ float w=0.75f+0.25f*sinf(g_anim_t*2.0f + i*1.7f);
    g_liquid[i]->tint[0]=g_liquid_col[0]*w; g_liquid[i]->tint[1]=g_liquid_col[1]*w; g_liquid[i]->tint[2]=g_liquid_col[2]*w; }
  // ---- toxic sludge + thruster wind hazard zones ----
  for (int i=0;i<g_haz_n;i++){
    float dx=e->position[0]-g_haz_pos[i][0], dz=e->position[2]-g_haz_pos[i][2];
    float r=g_haz_rad[i];
    if (dx*dx+dz*dz > r*r) continue;
    if (g_haz_type[i]==HAZ_TOXIC && e->position[1] < 3.0f){
      g_toxic_dot = 60.0f;   // refresh: a full minute of poisoning
      if (g_hit_flash < 0.12f){ g_hit_flash=0.18f; g_hit_src="TOXIC SLUDGE"; }
    } else if (g_haz_type[i]==HAZ_WIND){
      // thruster gale: shove the player along the wind + kick up ash
      float f=(1.0f - sqrtf(dx*dx+dz*dz)/r);
      e->velocity[0]+=g_haz_wind[i][0]*38.0f*f*(float)dt;
      e->velocity[2]+=g_haz_wind[i][2]*38.0f*f*(float)dt;
      if (((int)(g_anim_t*30))%2==0){ uint8_t ash[3]={130,120,110};
        vec3 ap={e->position[0]+g_haz_wind[i][0]*6.0f, e->position[1]+2.0f+dbg_frand()*3.0f, e->position[2]+g_haz_wind[i][2]*6.0f};
        ex_vfx_spawn_impact(&g_vfx, ap, 3, ash); }
    }
  }
  // toxic DOT: once poisoned, health bleeds and movement is handicapped for the
  // full duration even after leaving the sludge.
  if (g_toxic_dot > 0.0f){ g_toxic_dot -= (float)dt;
    g_player_hp -= 6.0f*(float)dt;                 // damage over time
    g_player_slow = fmaxf(g_player_slow, 1.2f);    // severe movement handicap
    if (g_hit_flash < 0.05f) g_hit_flash = 0.08f;
  }
  // ENVIRONMENTAL HAZARD: hot pools (lava/energon, i.e. red or high-red tint)
  // chip your health while you stand in them, near ground level.
  if (g_liquid_col[0] > 0.8f && e->position[1] < 2.5f) {
    for (int i=0;i<g_liquid_n;i++){
      float dx=e->position[0]-g_liquid[i]->position[0], dz=e->position[2]-g_liquid[i]->position[2];
      float rr=g_liquid[i]->scale3[0];
      if (dx*dx+dz*dz < rr*rr) {
        g_player_hp -= 14.0f*dt;   // hazard damage over time
        if (g_hit_flash < 0.15f) { g_hit_flash=0.2f; g_hit_src="HAZARD"; }
        break;
      }
    }
  }
  // defensive turrets: slowly rotate toward the player; fire when aligned + in
  // range. The slow tracking is the point -- outrun their aim in a vehicle.
  for (int i=0;i<g_deft_n;i++){
    def_turret_t *t=&g_deft[i]; if(!t->active) continue;
    float dx=e->position[0]-t->pos[0], dz=e->position[2]-t->pos[2];
    float dist=sqrtf(dx*dx+dz*dz);
    float target=atan2f(dz,dx);
    // rotate toward target at a slow rate (wrap-aware)
    float da=target-t->angle; while(da>3.14159f)da-=6.2832f; while(da<-3.14159f)da+=6.2832f;
    float rate=1.1f*dt; if(da>rate)da=rate; if(da<-rate)da=-rate; t->angle+=da;
    if(t->cd>0) t->cd-=dt;
    float aim_err=target-t->angle; while(aim_err>3.14159f)aim_err-=6.2832f; while(aim_err<-3.14159f)aim_err+=6.2832f;
    if (dist<70.0f && fabsf(aim_err)<0.22f && t->cd<=0.0f){
      t->cd=0.9f;
      vec3 from={t->pos[0]+cosf(t->angle)*1.5f, t->pos[1]+1.2f, t->pos[2]+sinf(t->angle)*1.5f};
      vec3 dir={dx/dist,0.05f,dz/dist};
      uint8_t tc[3]={255,120,90}; ex_vfx_spawn_shot(&g_vfx, from, dir, dist, tc);
      player_hit(10.0f, t->pos[0], t->pos[2], "TURRET");
    }
    // render: base + head rotate as a unit toward the aim
    g_deft_models[i]->position[0]=t->pos[0]; g_deft_models[i]->position[1]=0; g_deft_models[i]->position[2]=t->pos[2];
    g_deft_models[i]->rotation[1]=-(t->angle)*57.2957795f;
    g_deft_models[i]->scale=1.6f;
    g_deft_models[i]->tint[0]=0.7f; g_deft_models[i]->tint[1]=0.55f; g_deft_models[i]->tint[2]=0.5f;
  }
  // ---- third-person chase camera with collision pull-in ----
  // Aim direction stays camera->front (mouse look). The camera booms out behind
  // the player along -front; if the boom would pass through a cover volume or
  // the arena wall, pull it in so the camera never clips into geometry.
  {
    vec3 focus = { e->position[0], e->position[1] + 2.2f, e->position[2] };
    float boom = g_human_scale ? 3.4f : 8.5f;   // tighter, lower camera at human scale
    if (g_avatar_xform->mode == EX_MODE_VEHICLE) boom += g_speed_frac * 3.5f;   // pull back at speed for a sense of rush
    vec3 dir = { -camera->front[0], -camera->front[1], -camera->front[2] };
    float best = boom;
    // pull in against blueprint cover volumes (slab test on the boom ray)
    for (int i = 0; i < g_blueprint.n_vols; i++) {
      ex_map_vol_t *v = &g_blueprint.vols[i];
      best = boom_slab(focus, dir, best, v->center[0],v->center[1],v->center[2],
                       v->half[0],v->half[1],v->half[2]);
    }
    // ALSO pull in against the map + deco auto-colliders. Previously only the
    // blueprint vols were tested, so the boom could slide INTO map/deco boxes
    // and a giant block filled the view (it's world geometry, so it didn't move
    // with the bot). Testing all collidable boxes fixes that.
    for (int i = 0; i < g_map_col_n; i++){ float *c=g_map_col[i];
      best = boom_slab(focus, dir, best, c[0],c[1],c[2], c[3],c[4],c[5]); }
    for (int i = 0; i < g_deco_col_n; i++){ float *c=g_deco_col[i];
      best = boom_slab(focus, dir, best, c[0],c[1],c[2], c[3],c[4],c[5]); }
    // keep inside the arena walls (half-size ~84)
    const float S = 1420.0f;
    for (int step=1; step<=8; step++){
      float t = boom * step/8.0f; if (t>best) break;
      float px=focus[0]+dir[0]*t, pz=focus[2]+dir[2]*t;
      if (fabsf(px)>S || fabsf(pz)>S){ best = t - 0.3f; break; }
    }
    if (best < 1.2f) best = 1.2f;
    camera->position[0]=focus[0]+dir[0]*best;
    camera->position[1]=focus[1]+dir[1]*best;
    camera->position[2]=focus[2]+dir[2]*best;
    // trauma-based screen shake: offset the camera along screen-right/up so the
    // framing shakes without changing the aim direction. Squared falloff feels
    // punchy up close and settles smoothly.
    if (g_cam_trauma > 0.001f) {
      float sh = g_cam_trauma * g_cam_trauma;
      float tt = g_anim_t * 47.0f;
      float ox = (sinf(tt) + 0.6f*sinf(tt*2.3f)) * sh * 1.1f;
      float oy = (sinf(tt*1.7f+1.3f) + 0.6f*sinf(tt*3.1f)) * sh * 1.1f;
      vec3 rt; vec3_mul_cross(rt, camera->front, camera->up); vec3_norm(rt, rt);
      camera->position[0] += rt[0]*ox + camera->up[0]*oy;
      camera->position[1] += rt[1]*ox + camera->up[1]*oy;
      camera->position[2] += rt[2]*ox + camera->up[2]*oy;
    }
  }
  pl->position[0] = e->position[0]; pl->position[1] = e->position[1] + 6.0f; pl->position[2] = e->position[2];
  // form light: sits at the player, coloured by the current Watch form; flares
  // brighter for a moment on each shot (a coloured muzzle glow on the world).
  if (g_form_light) {
    ex_weapon_profile_t fw = ex_weapon_profile(g_form);
    float flare = 1.0f + g_recoil*2.5f + (g_overcharge_t>0?0.6f:0.0f) + g_spark_loud*0.5f;   // spark flares when loud
    g_form_light->position[0]=e->position[0]; g_form_light->position[1]=e->position[1]+2.2f; g_form_light->position[2]=e->position[2];
    g_form_light->color[0]=(fw.color[0]/255.0f)*1.5f*flare;
    g_form_light->color[1]=(fw.color[1]/255.0f)*1.5f*flare;
    g_form_light->color[2]=(fw.color[2]/255.0f)*1.5f*flare;
  }

  // ---- transform + watch ----
  ex_xform_update(g_avatar_xform, dt);
  ex_watch_update(&g_watch, dt);

  if (ex_keybind_down(&g_keys, EX_ACT_TRANSFORM, ex_keys_down) && !g_human_scale) {
    // the origami fold reconfigures the LIMBS into the alt-mode. A severed
    // structural limb can't fold, so the transform is blocked with a reason.
    static const int structural[] = { EX_PART_THIGH_L,EX_PART_THIGH_R,EX_PART_SHIN_L,
                                      EX_PART_SHIN_R,EX_PART_ARM_L,EX_PART_ARM_R };
    static const char *snames[] = { "left thigh","right thigh","left shin",
                                    "right shin","left arm","right arm" };
    int blocked = -1;
    for (int q=0;q<6;q++) if (structural[q]<EX_ROBOT_PARTS && g_part_detached[structural[q]]) { blocked=q; break; }
    if (blocked >= 0) {
      snprintf(g_shot_msg,sizeof(g_shot_msg),"CAN'T TRANSFORM: %s severed", snames[blocked]);
      g_shot_timer = 2.2f; sfx(g_sfx_hit); g_cam_trauma += 0.08f;   // error jolt
    } else {
      ex_xform_request(g_avatar_xform);
      sfx(g_sfx_xform);
    }
    ex_keys_down[ex_keybind_get(&g_keys, EX_ACT_TRANSFORM)] = 0;
  }
  if (ex_keys_down[SDL_SCANCODE_B]) {        // element cycle
    ex_watch_cycle(&g_watch);
    ex_keys_down[SDL_SCANCODE_B] = 0;
  }

  int in_vehicle = (g_avatar_xform->mode == EX_MODE_VEHICLE);
  float size_scale = 0.5f + 0.35f * g_avatar_xform->size_class;
  if (g_human_scale) size_scale *= g_human_k;   // human-scale campaign sections
  float aim_yaw = atan2f(camera->front[2], camera->front[0]);
  // BODY faces where you MOVE (arrow keys), the camera is where you LOOK. Turn the
  // body smoothly toward the velocity heading; hold facing when nearly stopped.
  { float ms = sqrtf(e->velocity[0]*e->velocity[0]+e->velocity[2]*e->velocity[2]);
    if (ms > 2.0f){ float tgt = atan2f(e->velocity[2], e->velocity[0]);
      float d = tgt - g_body_yaw; while(d>3.14159f)d-=6.2831853f; while(d<-3.14159f)d+=6.2831853f;
      g_body_yaw += d * fminf((float)dt*8.0f, 1.0f); }
    while(g_body_yaw>3.14159f)g_body_yaw-=6.2831853f; while(g_body_yaw<-3.14159f)g_body_yaw+=6.2831853f;
  }
  float pspeed = sqrtf(e->velocity[0]*e->velocity[0] + e->velocity[2]*e->velocity[2]);
  g_phase_player = ex_gait_advance(g_phase_player, pspeed, dt);
  int pel = (int)ex_watch_active(&g_watch);
  // speed-based FOV + camera pull-back from ACTUAL velocity, for a real sense of
  // velocity that builds as you accelerate (vehicles rip), plus a firing punch-in.
  { float vmag = sqrtf(e->velocity[0]*e->velocity[0] + e->velocity[2]*e->velocity[2]);
    float topsp = in_vehicle ? 26.0f : 14.0f;
    g_speed_frac += (fminf(vmag/topsp, 1.0f) - g_speed_frac) * fminf(dt*4.0f, 1.0f);
    float base = in_vehicle ? 82.0f : 72.0f;
    float want = base + g_speed_frac * (in_vehicle?30.0f:20.0f) - g_recoil * 6.0f;
    camera->fov += (want - camera->fov) * fminf(dt*7.0f, 1.0f);
    ex_fps_camera_resize(camera); }

  // Build the player's base poses: robot silhouette, or the chosen vehicle
  // archetype (car/tank/jet/insect/spider/dragon/dino/raptor/hover) when
  // transformed. Then scatter a greeble detail pass over those parts for the
  // thousand-part detailed look (rendered up to a budget).
  ex_part_pose_t base_poses[EX_VEHICLE_MAX_PARTS];
  int n_base;
  if (in_vehicle) {
    ex_vehicle_layout(base_poses, EX_VEHICLE_MAX_PARTS, &n_base, g_vehicle, &g_build,
                      size_scale, g_phase_player, pspeed, pel, EX_BODY_PLAYER);
  } else {
    ex_body_kind pbk = g_human_scale ? EX_BODY_HUMAN : EX_BODY_PLAYER;
    ex_robot_layout_ex(base_poses, 0, pbk, &g_build, size_scale,
                       g_phase_player, pspeed, pel);
    n_base = EX_ROBOT_PARTS;
  }
  // place the base parts (rotate to face aim), reusing the player part models
  // transform animation: spin the whole body + squash through the change
  float xf_spin = 0.0f, xf_squash = 1.0f - g_land_squash*0.5f;   // cushion on landing
  for (int i=0;i<EX_ROBOT_PARTS;i++){ g_part_spin[i]=0.0f; g_part_pitch[i]=0.0f; }
  if (g_avatar_xform->state == EX_XFORM_STATE_TRANSFORMING) {
    float bp01 = 1.0f - g_avatar_xform->blend_timer / EX_XFORM_BLEND_SEC; // 0->1
    xf_squash = 1.0f - 0.18f*sinf(bp01*3.14159f);    // slight whole-body squash
    // --- true part-by-part morph: interpolate every part between the robot
    //     and vehicle configurations, staggered so parts fold in sequence,
    //     each spinning quickly then settling (no teleport at the finish). ---
    ex_part_pose_t rob[EX_VEHICLE_MAX_PARTS], veh[EX_VEHICLE_MAX_PARTS];
    int nveh;
    ex_robot_layout_ex(rob, 0, EX_BODY_PLAYER, &g_build, size_scale, g_phase_player, pspeed, pel);
    ex_vehicle_layout(veh, EX_VEHICLE_MAX_PARTS, &nveh, g_vehicle, &g_build,
                      size_scale, g_phase_player, pspeed, pel, EX_BODY_PLAYER);
    // NOTE: during TRANSFORMING, x->mode is still the mode we're LEAVING (it only
    // flips when the blend completes). So `in_vehicle` means "currently a vehicle,
    // morphing back to robot". Pick src/dst accordingly or the animation reverses.
    ex_part_pose_t *src = in_vehicle ? veh : rob;   // the mode we're leaving (the current one)
    ex_part_pose_t *dst = in_vehicle ? rob : veh;   // the mode we're entering
    int nsrc = in_vehicle ? nveh : EX_ROBOT_PARTS;
    int ndst = in_vehicle ? EX_ROBOT_PARTS : nveh;
    n_base = (ndst > nsrc ? ndst : nsrc);
    if (n_base > EX_VEHICLE_MAX_PARTS) n_base = EX_VEHICLE_MAX_PARTS;
    for (int i=0;i<n_base;i++){
      const ex_chassis_info_t *chf = ex_chassis_info((ex_chassis_t)g_menu.loadout.chassis);
      float st = fmodf(i*0.085f, chf->fold_stagger);   // chassis-tuned stagger spread
      float pp = (bp01 - st) / (1.0f - chf->fold_stagger);
      if (pp<0) pp=0; if (pp>1) pp=1;
      float e2 = pp*pp*(3.0f-2.0f*pp);                  // smoothstep ease
      int hs = (i < nsrc), hd = (i < ndst);
      ex_part_pose_t a = hs ? src[i] : (hd?dst[i]:src[i]);
      ex_part_pose_t b = hd ? dst[i] : (hs?src[i]:dst[i]);
      float fs = hs ? 1.0f : 0.0f, fd = hd ? 1.0f : 0.0f;  // fade appearing/leaving parts
      float fade = fs + (fd - fs) * e2;
      base_poses[i].offset[0] = a.offset[0] + (b.offset[0]-a.offset[0])*e2;
      base_poses[i].offset[1] = a.offset[1] + (b.offset[1]-a.offset[1])*e2;
      base_poses[i].offset[2] = a.offset[2] + (b.offset[2]-a.offset[2])*e2;
      // --- action-origami fold: the part DETACHES outward from the body, arcs,
      //     crimps (compresses), then refolds into its target -- a mechanism
      //     articulating into a new base, rather than a spin in place. ---
      float detach = sinf(pp*3.14159f);                 // 0 at ends, 1 mid-fold
      float ox=(a.offset[0]+b.offset[0])*0.5f, oz=(a.offset[2]+b.offset[2])*0.5f;
      float ol=sqrtf(ox*ox+oz*oz); if(ol<0.001f) ol=0.001f;
      base_poses[i].offset[0] += (ox/ol) * detach * 1.5f;              // hinge outward
      base_poses[i].offset[2] += (oz/ol) * detach * 1.5f;
      base_poses[i].offset[1] += detach * (0.55f + (i%3)*0.28f) * ((i&1)?1.0f:-0.55f); // arc
      float crimp = 1.0f - detach*0.28f;                // compress mid-fold
      base_poses[i].sx = (a.sx + (b.sx-a.sx)*e2) * fade;
      base_poses[i].sy = (a.sy + (b.sy-a.sy)*e2) * fade * crimp;
      base_poses[i].sz = (a.sz + (b.sz-a.sz)*e2) * fade;
      // near-invisible fading parts collapse to EXACTLY zero so the render hide is
      // deterministic -- otherwise the scale hovers around the cull threshold and
      // the part flickers appear/disappear during the fold.
      if (fade < 0.05f) { base_poses[i].sx = base_poses[i].sy = base_poses[i].sz = 0.0f; }
      for (int c=0;c<3;c++) base_poses[i].color[c] = (uint8_t)(a.color[c] + (b.color[c]-a.color[c])*e2);
      // hinge rotation on two axes -- a fold, not a free spin; eases to 0 at settle
      // damaged-but-attached parts fold raggedly: the weak joint judders and
      // lags, so a beaten-up robot's transform looks visibly strained.
      float pintg = (i<EX_ROBOT_PARTS)? g_part_integ[i] : 1.0f;
      float ragged = (pintg < 0.75f) ? (0.75f-pintg) : 0.0f;
      float judder = ragged * 60.0f * sinf(g_anim_t*40.0f + i*1.7f);
      g_part_spin[i]  = detach * chf->fold_spin * ((i&1)?1.0f:-1.0f) + judder;
      g_part_pitch[i] = detach * 200.0f * ((i&2)?1.0f:-1.0f) + judder*0.5f;
    }
    if (!g_xf_flashed) {                                 // one-shot spark burst
      uint8_t sc[3]={200,220,255}; ex_vfx_spawn_death(&g_vfx, e->position, sc); g_xf_flashed = 1;
    }
  } else g_xf_flashed = 0;

  // --- SIGNATURE COMPONENT (class back-part): keep it present + correctly
  //     placed in BOTH silhouettes, folding between two explicit poses so it
  //     never fades to nothing on transform (the generic morph would, since the
  //     vehicle layout doesn't claim the BACK index). Deploys LATE in the fold. ---
  {
    ex_part_pose_t sig_rob, sig_veh;
    ex_signature_pose(&sig_rob, &sig_veh, g_player_sig, g_vehicle,
                      size_scale, g_phase_player, pspeed);
    ex_part_pose_t *from = in_vehicle ? &sig_rob : &sig_veh; // mode we're leaving
    ex_part_pose_t *to   = in_vehicle ? &sig_veh : &sig_rob; // mode we're entering
    ex_part_pose_t fin;
    if (g_avatar_xform->state == EX_XFORM_STATE_TRANSFORMING) {
      float bp01 = 1.0f - g_avatar_xform->blend_timer / EX_XFORM_BLEND_SEC; // 0->1
      float st = ex_signature_fold_start();
      float pp = (bp01 - st) / (1.0f - st); if (pp<0) pp=0; if (pp>1) pp=1;
      float e2 = pp*pp*(3.0f-2.0f*pp);                   // smoothstep
      for (int k=0;k<3;k++) fin.offset[k] = from->offset[k] + (to->offset[k]-from->offset[k])*e2;
      fin.sx = from->sx + (to->sx-from->sx)*e2;
      fin.sy = from->sy + (to->sy-from->sy)*e2;
      fin.sz = from->sz + (to->sz-from->sz)*e2;
      float detach = sinf(pp*3.14159f);                  // arc up + hinge as it deploys
      fin.offset[1] += detach*0.28f;
      g_part_spin[EX_PART_BACK]  = detach * 240.0f;
      g_part_pitch[EX_PART_BACK] = detach * 130.0f;
    } else {
      fin = in_vehicle ? sig_veh : sig_rob;
    }
    // tint: keep the accent the layout already assigned to BACK; if it was faded
    // to black (vehicle layout left it zero), borrow the body base colour.
    for (int c=0;c<3;c++) fin.color[c] = base_poses[EX_PART_BACK].color[c];
    if (fin.color[0]==0 && fin.color[1]==0 && fin.color[2]==0)
      for (int c=0;c<3;c++) fin.color[c] = base_poses[EX_PART_CHEST].color[c];
    base_poses[EX_PART_BACK] = fin;
    if (n_base <= EX_PART_BACK) n_base = EX_PART_BACK + 1;  // ensure it's drawn in vehicle mode
  }
  { float dyaw = aim_yaw - g_last_aim_yaw;
    while (dyaw> 3.14159f) dyaw-=6.2831853f; while (dyaw<-3.14159f) dyaw+=6.2831853f;
    g_last_aim_yaw = aim_yaw;
    float target_lean = fmaxf(-14.0f, fminf(14.0f, dyaw*200.0f));   // subtle bank into turn (deg)
    g_turn_lean += (target_lean - g_turn_lean) * fminf(dt*8.0f, 1.0f);

    // --- DYNAMIC GAIT LEAN: read the player's velocity in facing-space and lean
    //     the body accordingly, pivoting at the feet. Forward lean when driving
    //     ahead, back when braking; bank into lateral (drift) motion. This is
    //     where friction shows up -- a hard direction change leaves the body
    //     leaning into the slide until velocity catches up to the new facing. ---
    float fwx=cosf(g_body_yaw), fwz=sinf(g_body_yaw);   // body facing forward (world xz)
    float rgx=-sinf(g_body_yaw), rgz=cosf(g_body_yaw);  // body facing right
    float vfwd = e->velocity[0]*fwx + e->velocity[2]*fwz;   // speed along facing
    float vlat = e->velocity[0]*rgx + e->velocity[2]*rgz;   // lateral (drift) speed
    float tgt_fwd  = fmaxf(-0.28f, fminf(0.28f,  vfwd*0.012f));  // lean-forward factor
    float tgt_side = fmaxf(-0.45f, fminf(0.45f,  vlat*0.016f*(2.0f-g_veh_grip)));  // drift bank (low grip leans more)
    g_lean_fwd  += (tgt_fwd  - g_lean_fwd ) * fminf(dt*6.0f, 1.0f);
    g_lean_side += (tgt_side - g_lean_side) * fminf(dt*6.0f, 1.0f);
    // head tracks the camera's up/down look, independent of the legs
    float cam_pitch = asinf(fmaxf(-1.0f, fminf(1.0f, camera->front[1]))) * 57.2957795f;
    g_head_pitch += (cam_pitch*0.55f - g_head_pitch) * fminf(dt*10.0f, 1.0f);

    float bry=g_body_yaw+EX_BODY_FACING_OFF;
    float cy=cosf(bry), sy=sinf(bry); float ydeg=bry*57.2957795f + xf_spin;
    int xforming = (g_avatar_xform->state == EX_XFORM_STATE_TRANSFORMING);
    float lean_amt = xforming ? 0.0f : 1.0f;   // don't fight the fold animation
    for (int i=0;i<EX_ROBOT_PARTS && i<n_base;i++){
      ex_model_t *pm=g_player_parts[i];
      // microFSM damage: broken parts vanish (their debris is flying/fallen);
      // damaged-but-attached parts sag at the joint (hang loose).
      if (i<EX_ROBOT_PARTS && g_part_detached[i]) { pm->position[1]=-1000.0f; continue; }
      float integ = (i<EX_ROBOT_PARTS)? g_part_integ[i] : 1.0f;
      float sag = (integ < 0.6f) ? (0.6f-integ) : 0.0f;   // 0..0.6 droop
      // lean the body: upper parts shift in facing-space proportional to height
      // (pivot at the feet), so the whole silhouette tips into motion/drift.
      float lh = base_poses[i].offset[1]; if (lh<0) lh=0;
      float ox=base_poses[i].offset[0] + lh*g_lean_side*lean_amt;
      float oz=base_poses[i].offset[2] + lh*g_lean_fwd*lean_amt;
      pm->position[0]=e->position[0]+ox*cy-oz*sy;
      pm->position[1]=e->position[1]+base_poses[i].offset[1] - sag*0.35f;   // droop down
      pm->position[2]=e->position[2]+ox*sy+oz*cy;
      pm->rotation[1]=ydeg + g_part_spin[i];
      pm->rotation[0]=g_part_pitch[i] + sag*55.0f;                          // hinge hangs forward
      // head pitches to track the aim; the rest inherits the drift bank roll
      if ((i==EX_PART_HEAD||i==EX_PART_VISOR) && g_part_spin[i]==0.0f){
        pm->rotation[0]=g_head_pitch;
        // head turns toward the CAMERA (where you look), clamped, while the body
        // faces the movement direction.
        float dy = aim_yaw - g_body_yaw; while(dy>3.14159f)dy-=6.2831853f; while(dy<-3.14159f)dy+=6.2831853f;
        dy = fmaxf(-1.05f, fminf(1.05f, dy));   // +-60 deg
        pm->rotation[1] = ydeg + g_part_spin[i] + dy*57.2957795f;
      }
      if (g_part_spin[i]==0.0f) pm->rotation[2]=g_turn_lean + sag*40.0f*((i&1)?1.0f:-1.0f);
      pm->scale3[0]=fabsf(cy)*base_poses[i].sx+fabsf(sy)*base_poses[i].sz;
      pm->scale3[1]=base_poses[i].sy*xf_squash;
      pm->scale3[2]=fabsf(cy)*base_poses[i].sz+fabsf(sy)*base_poses[i].sx;
      // if a part's scale collapses to ~0 (a fading transform part), HIDE it --
      // otherwise the renderer falls back to the uniform scale (1.0) and a full
      // -size box flickers in over the transformation.
      if (pm->scale3[0]<0.02f && pm->scale3[1]<0.02f && pm->scale3[2]<0.02f) {
        pm->position[1] = -100000.0f;
      }
      pm->tint[0]=base_poses[i].color[0]/255.0f; pm->tint[1]=base_poses[i].color[1]/255.0f; pm->tint[2]=base_poses[i].color[2]/255.0f;
      pm->emission_strength = ex_chassis_info((ex_chassis_t)g_menu.loadout.chassis)->glow;  // frame drives glow
      { float pg = ex_photon_glow(&g_photon);   // Photon Man: blaze white and self-lit
        if (pg > 0.0f) {
          pm->tint[0] += (1.0f-pm->tint[0])*pg; pm->tint[1] += (1.0f-pm->tint[1])*pg; pm->tint[2] += (1.0f-pm->tint[2])*pg;
          pm->emission_strength += 3.0f*pg;
        } }
      { float mg = ex_matrix_glow(&g_matrix);   // Rodimus: radiant gold
        if (mg > 0.0f) {
          pm->tint[0] += (1.0f-pm->tint[0])*mg; pm->tint[1] += (0.86f-pm->tint[1])*mg; pm->tint[2] += (0.45f-pm->tint[2])*mg;
          pm->emission_strength += 2.4f*mg;
        } }
    }
    for (int i=n_base;i<EX_ROBOT_PARTS;i++) g_player_parts[i]->position[1]=-1000.0f;
    // CAPE: a flowing panel off the upper back that trails and sways with motion.
    if (g_cape && !g_human_scale) {
      float H = g_build.height;
      // lag the sway behind the player's velocity for a trailing-cloth feel
      float tvx = -e->velocity[0]*0.01f, tvz = -e->velocity[2]*0.01f;
      g_cape_sway[0] += (tvx - g_cape_sway[0]) * fminf((float)dt*4.0f,1.0f);
      g_cape_sway[1] += (tvz - g_cape_sway[1]) * fminf((float)dt*4.0f,1.0f);
      float back = -0.34f;   // sits behind the torso (local -z)
      float bx = sinf(g_anim_t*1.5f)*0.03f;   // idle flutter
      g_cape->position[0] = e->position[0] + (-sy)*back + g_cape_sway[0]*20.0f;
      g_cape->position[1] = e->position[1] + (1.05f*H);
      g_cape->position[2] = e->position[2] + (cy)*back + g_cape_sway[1]*20.0f;
      g_cape->rotation[1] = ydeg;
      g_cape->rotation[0] = 14.0f + (g_cape_sway[0]+g_cape_sway[1])*30.0f + bx*40.0f;  // trails back, lifts with speed
      g_cape->scale3[0] = 0.52f*g_build.shoulders; g_cape->scale3[1] = 0.95f*H; g_cape->scale3[2] = 0.05f;
      // cape reads in the accent colour, a touch darker than the trim
      uint8_t ac[3]; { int el=g_build.accent_element;
        switch(el){ case 1: ac[0]=90;ac[1]=140;ac[2]=180; break; case 2: ac[0]=170;ac[1]=60;ac[2]=35; break;
          case 3: ac[0]=140;ac[1]=155;ac[2]=145; break; case 5: ac[0]=70;ac[1]=120;ac[2]=60; break;
          default: ac[0]=150;ac[1]=130;ac[2]=60; } }
      g_cape->tint[0]=ac[0]/255.0f; g_cape->tint[1]=ac[1]/255.0f; g_cape->tint[2]=ac[2]/255.0f;
    } else if (g_cape) g_cape->position[1] = -100000.0f;
    // arm-cannon in the right hand, aimed forward, tinted by the active form
    for (int wi=0; wi<10; wi++) g_weapon_models[wi]->position[1] = -1000.0f;
    if (g_muzzle && (in_vehicle)) g_muzzle->position[1] = -1000.0f;
    if (!in_vehicle) {
      ex_weapon_profile_t vp = ex_weapon_apply_mods(g_form, &g_wmods);   // design reflects addons
      int wm = (int)vp.mode; if (wm<0||wm>=10) wm=0;   // per-mode gun model
      ex_model_t *w = g_weapon_models[wm];
      float hx = base_poses[EX_PART_HAND_R].offset[0], hz = base_poses[EX_PART_HAND_R].offset[2];
      float rk = g_recoil*0.5f;
      w->position[0] = e->position[0] + hx*cy - hz*sy + camera->front[0]*(0.6f-rk);
      w->position[1] = e->position[1] + base_poses[EX_PART_HAND_R].offset[1] + g_recoil*0.15f;
      w->position[2] = e->position[2] + hx*sy + hz*cy + camera->front[2]*(0.6f-rk);
      w->rotation[1] = ydeg;
      // weapon DESIGN: addons reshape the cannon -- barrels lengthen it, scatter
      // fattens it, power cells make it run hot even cold.
      float barrel=1.0f, girth=1.0f, glow=0.0f;
      for (int mi=0; mi<g_wmods.count; mi++){ switch(g_wmods.mods[mi]){
        case EX_MOD_LONG_BARREL: case EX_MOD_PIERCE_CORE: barrel+=0.5f; break;
        case EX_MOD_SCOPE: barrel+=0.2f; break;
        case EX_MOD_SCATTER_BARREL: case EX_MOD_UNDERBARREL: girth+=0.35f; break;
        case EX_MOD_AMPLIFIER: case EX_MOD_ENERGON_CELL: glow+=0.5f; break;
        default: break; } }
      float ws = 0.5f * size_scale;
      w->scale3[0]=ws*girth; w->scale3[1]=ws*girth; w->scale3[2]=ws*barrel;
      // barrel heat: builds with sustained fire, cools when idle. The weapon
      // glows from its form colour toward white-hot orange as it heats up.
      float baseR=vp.color[0]/255.0f, baseG=vp.color[1]/255.0f, baseB=vp.color[2]/255.0f;
      float h = g_barrel_heat; if(h>1.0f)h=1.0f;
      h = fminf(1.0f, h + glow*0.4f);           // power cells run it hot even at rest
      w->tint[0] = baseR + (1.0f-baseR)*h;
      w->tint[1] = baseG + (0.45f-baseG)*h;
      w->tint[2] = baseB + (0.10f-baseB)*h;
      w->emissive_solid = (h>0.55f) ? 1 : 0;   // glow when hot (or amped)
      w->is_lit = (h>0.55f) ? 0 : 1;
      // muzzle flash: a bright burst at the barrel tip, sized by recoil
      if (g_muzzle) {
        if (g_recoil > 0.05f) {
          float fl = g_recoil * 1.1f;
          g_muzzle->position[0] = w->position[0] + camera->front[0]*1.3f;
          g_muzzle->position[1] = w->position[1] + camera->front[1]*1.3f;
          g_muzzle->position[2] = w->position[2] + camera->front[2]*1.3f;
          g_muzzle->scale3[0]=g_muzzle->scale3[1]=g_muzzle->scale3[2]=fl;
          g_muzzle->tint[0]=1.0f; g_muzzle->tint[1]=0.85f; g_muzzle->tint[2]=0.5f;
        } else g_muzzle->position[1] = -1000.0f;
      }
    } else {
      // VEHICLE MODE: the weapon is MOUNTED ON THE MODEL, not floating by the
      // camera. Structured vehicles (jet/racer/tank) fix it to the chassis; beast
      // modes bind it to the HEAD. It copies the bound part's world position AND
      // rotation, so when that part animates (a beast head turning/bobbing, a
      // fender rolling) the gun rides and rotates with it.
      ex_weapon_profile_t vp = ex_weapon_profile(g_form);
      int wm = (int)vp.mode; if (wm<0||wm>=10) wm=0;   // per-mode gun model
      ex_model_t *w = g_weapon_models[wm];
      int beast = (g_vehicle==EX_VEH_DINO||g_vehicle==EX_VEH_RAPTOR||g_vehicle==EX_VEH_WOLF||
                   g_vehicle==EX_VEH_SCORPION||g_vehicle==EX_VEH_DRAGON||g_vehicle==EX_VEH_SPIDER||
                   g_vehicle==EX_VEH_INSECT||g_vehicle==EX_VEH_CRAB);
      int bind = beast ? 1 : 0;                 // vehicle layout: 0=chassis, 1=head
      ex_model_t *bp = g_player_parts[bind];
      // local mount offset from the bound part, rotated into world by its yaw so
      // the gun tracks the part's facing. Beast: forward from the muzzle; others:
      // a hull-top forward hardpoint.
      float ox = 0.0f,
            oy = (beast ? 0.10f : 0.32f) * size_scale,
            oz = (beast ? 0.55f : 0.70f) * size_scale;
      float byaw = bp->rotation[1] * 0.0174532925f;
      float bcy = cosf(byaw), bsy = sinf(byaw);
      w->position[0] = bp->position[0] + (ox*bcy - oz*bsy);
      w->position[1] = bp->position[1] + oy;
      w->position[2] = bp->position[2] + (ox*bsy + oz*bcy);
      w->rotation[0] = bp->rotation[0];         // inherit the part's pitch (head aim)
      w->rotation[1] = bp->rotation[1];         // and yaw (turns with the part)
      w->rotation[2] = bp->rotation[2];
      w->scale = 0.5f * size_scale;
      float baseR=vp.color[0]/255.0f, baseG=vp.color[1]/255.0f, baseB=vp.color[2]/255.0f;
      float h = g_barrel_heat; if(h>1.0f)h=1.0f;
      w->tint[0]=baseR+(1.0f-baseR)*h; w->tint[1]=baseG+(0.45f-baseG)*h; w->tint[2]=baseB+(0.10f-baseB)*h;
      w->emissive_solid = (h>0.55f) ? 1 : 0;
      w->is_lit = (h>0.55f) ? 0 : 1;
    }
  }
  // greeble detail pass over the base parts
  {
    ex_detail_part_t dp[EX_VEHICLE_MAX_PARTS];
    int ndp = n_base < EX_VEHICLE_MAX_PARTS ? n_base : EX_VEHICLE_MAX_PARTS;
    for (int i=0;i<ndp;i++){
      dp[i].offset[0]=base_poses[i].offset[0]; dp[i].offset[1]=base_poses[i].offset[1]; dp[i].offset[2]=base_poses[i].offset[2];
      dp[i].sx=base_poses[i].sx; dp[i].sy=base_poses[i].sy; dp[i].sz=base_poses[i].sz;
      dp[i].color[0]=base_poses[i].color[0]; dp[i].color[1]=base_poses[i].color[1]; dp[i].color[2]=base_poses[i].color[2];
    }
    uint8_t acc[3];
    { int el=g_build.accent_element;
      switch(el){ case 1: acc[0]=150;acc[1]=210;acc[2]=245; break; case 2: acc[0]=225;acc[1]=95;acc[2]=45; break;
        case 3: acc[0]=200;acc[1]=215;acc[2]=205; break; case 4: acc[0]=235;acc[1]=205;acc[2]=85; break;
        case 5: acc[0]=95;acc[1]=165;acc[2]=80; break; case 6: acc[0]=175;acc[1]=140;acc[2]=110; break;
        case 7: acc[0]=85;acc[1]=150;acc[2]=210; break; default: acc[0]=235;acc[1]=205;acc[2]=85; }
      float gl=ex_chassis_info((ex_chassis_t)g_menu.loadout.chassis)->glow;
      for(int gi=0;gi<3;gi++){ int v=(int)(acc[gi]*gl); acc[gi]=(uint8_t)(v>255?255:v); } }
    static ex_greeble_t greebles[DETAIL_MODELS];
    int ng = ex_detail_generate(greebles, DETAIL_MODELS, dp, ndp, 0xB07 ^ (unsigned)g_vehicle, DETAIL_MODELS, acc);
    if (g_human_scale) ng = 0;   // humans have no mechanical greeble detail
    float bry=g_body_yaw+EX_BODY_FACING_OFF;
    float cy=cosf(bry), sy=sinf(bry); float ydeg=bry*57.2957795f;
    for (int i=0;i<ng;i++){
      ex_model_t *gm=g_detail_models[i];
      float ox=greebles[i].offset[0], oz=greebles[i].offset[2];
      gm->position[0]=e->position[0]+ox*cy-oz*sy;
      gm->position[1]=e->position[1]+greebles[i].offset[1];
      gm->position[2]=e->position[2]+ox*sy+oz*cy;
      gm->rotation[1]=ydeg;
      gm->scale3[0]=fabsf(cy)*greebles[i].sx+fabsf(sy)*greebles[i].sz;
      gm->scale3[1]=greebles[i].sy;
      gm->scale3[2]=fabsf(cy)*greebles[i].sz+fabsf(sy)*greebles[i].sx;
      gm->tint[0]=greebles[i].color[0]/255.0f; gm->tint[1]=greebles[i].color[1]/255.0f; gm->tint[2]=greebles[i].color[2]/255.0f;
    }
    for (int i=ng;i<DETAIL_MODELS;i++) g_detail_models[i]->position[1]=-1000.0f;
    if (g_cape) g_cape->position[1]=-1000.0f;   // no cape in vehicle mode
  }
  (void)aim_yaw;

  // ---- signature abilities, gated to their lore-appropriate Watch forms ----
  ex_element_t form_el = ex_form_element(g_form);
  int is_fire_form = (form_el == EX_ELEM_FIRE);       // fire / magma
  int is_ice_form  = (form_el == EX_ELEM_ICE);        // ice / blizzard
  int is_water_form = (form_el == EX_ELEM_WATER);     // water form
  int is_electric_form = (form_el == EX_ELEM_ELECTRIC); // electric form
  int is_air_form = (form_el == EX_ELEM_AIR);           // air form
  // Whirlwind (the Watch's air form): press C to spin up a vortex ahead of you
  // that sweeps enemies into an orbit and off their attack for a few seconds.
  if (g_cyclone_cd > 0.0f) g_cyclone_cd -= (float)dt;
  if (is_air_form && ex_keys_down[SDL_SCANCODE_C] && !g_cyclone_on && g_cyclone_cd <= 0.0f && !g_human_scale) {
    g_cyclone_c[0] = e->position[0] + camera->front[0]*18.0f;
    g_cyclone_c[1] = e->position[1];
    g_cyclone_c[2] = e->position[2] + camera->front[2]*18.0f;
    g_cyclone_on = 1; g_cyclone_t = EX_CYCLONE_DURATION; g_cyclone_cd = 6.5f;
    snprintf(g_shot_msg,sizeof(g_shot_msg),"WHIRLWIND"); g_shot_timer=1.4f;
    g_cam_trauma += 0.3f; sfx(g_sfx_expl);
    ex_keys_down[SDL_SCANCODE_C]=0;
  }
  if (g_cyclone_on) {
    g_cyclone_t -= (float)dt;
    if (g_cyclone_t <= 0.0f) g_cyclone_on = 0;
    else {
      // swirling dust ring at the eye
      spawn_shockring(g_cyclone_c[0], 0.5f, g_cyclone_c[2], EX_CYCLONE_RADIUS*0.9f);
      float ph = g_cyclone_t*6.0f; uint8_t wc[3]={210,230,245};
      for (int k=0;k<3;k++){ float a=ph+k*2.094f, rr=EX_CYCLONE_RADIUS*0.55f;
        ex_vfx_spawn_impact(&g_vfx,(vec3){g_cyclone_c[0]+cosf(a)*rr, e->position[1]+1.4f, g_cyclone_c[2]+sinf(a)*rr},3,wc); }
    }
  }
  int is_vine_form = (form_el == EX_ELEM_VINE);         // vine / nature form
  // Thorn field (the Watch's vine form): press X and brambles erupt around you,
  // spreading outward to hold ground -- anything in them is torn and snared.
  if (g_thorns_cd > 0.0f) g_thorns_cd -= (float)dt;
  if (is_vine_form && ex_keys_down[SDL_SCANCODE_X] && !g_thorns_on && g_thorns_cd <= 0.0f && !g_human_scale) {
    g_thorns_c[0]=e->position[0]; g_thorns_c[1]=e->position[1]; g_thorns_c[2]=e->position[2];
    g_thorns_on = 1; g_thorns_age = 0.0f; g_thorns_cd = 8.0f;
    snprintf(g_shot_msg,sizeof(g_shot_msg),"THORN FIELD"); g_shot_timer=1.4f; sfx(g_sfx_expl);
    ex_keys_down[SDL_SCANCODE_X]=0;
  }
  if (g_thorns_on) {
    g_thorns_age += (float)dt;
    if (g_thorns_age >= EX_THORNS_DURATION) g_thorns_on = 0;
    else {
      float trad = ex_thorns_radius(g_thorns_age, EX_THORNS_RADIUS, EX_THORNS_GROW);
      for (int q=0;q<g_enc.count;q++){ ex_enemy_t *en=&g_enc.enemies[q]; if(en->state==EX_ENEMY_DEAD) continue;
        float dx=en->position[0]-g_thorns_c[0], dz=en->position[2]-g_thorns_c[2];
        float inten = ex_thorns_intensity(sqrtf(dx*dx+dz*dz), trad);
        if (inten > 0.0f){ ex_enemy_damage(en, (6.0f+10.0f*inten)*(float)dt);   // torn, worse near the centre
          if (g_enemy_web[q] < 0.6f) g_enemy_web[q] = 0.6f;                       // snared (reuses the slow system)
          g_enemy_hurt[q] = fmaxf(g_enemy_hurt[q], 0.08f); } }
      g_thorns_fx_cd -= (float)dt;
      if (g_thorns_fx_cd <= 0.0f){ spawn_shockring(g_thorns_c[0], 0.35f, g_thorns_c[2], trad);
        uint8_t vc[3]={90,180,80};
        for (int k=0;k<4;k++){ float a=g_thorns_age*3.0f+k*1.571f, rr=trad*0.7f;
          ex_vfx_spawn_impact(&g_vfx,(vec3){g_thorns_c[0]+cosf(a)*rr,0.4f,g_thorns_c[2]+sinf(a)*rr},3,vc); }
        g_thorns_fx_cd = 0.5f; }
    }
  }
  // Chain lightning (the Watch's electric form): press F to loose a bolt that
  // leaps from the nearest foe to the next, arcing through a whole cluster.
  if (g_arc_cd > 0.0f) g_arc_cd -= (float)dt;
  if (is_electric_form && ex_keys_down[SDL_SCANCODE_F] && g_arc_cd <= 0.0f && !g_human_scale) {
    static float epos[EX_MAX_ENEMIES][3]; static int ealive[EX_MAX_ENEMIES];
    int en_n = g_enc.count; if (en_n > EX_MAX_ENEMIES) en_n = EX_MAX_ENEMIES;
    for (int q=0;q<en_n;q++){ ex_enemy_t *en=&g_enc.enemies[q];
      epos[q][0]=en->position[0]; epos[q][1]=en->position[1]; epos[q][2]=en->position[2];
      ealive[q] = (en->state!=EX_ENEMY_DEAD) ? 1 : 0; }
    int chain[EX_ARC_JUMPS]; float org[3]={e->position[0],e->position[1],e->position[2]};
    int nc = ex_arc_chain(org, epos, ealive, en_n, EX_ARC_RANGE, EX_ARC_JUMPS, chain);
    if (nc > 0) {
      uint8_t lc[3]={255,240,120};
      float prev[3]={e->position[0], e->position[1]+1.4f, e->position[2]};
      float dmg = 34.0f;
      for (int k=0;k<nc;k++){ ex_enemy_t *en=&g_enc.enemies[chain[k]];
        vec3 to={en->position[0], en->position[1]+1.2f, en->position[2]};
        vec3 seg={to[0]-prev[0], to[1]-prev[1], to[2]-prev[2]};
        float sl=sqrtf(seg[0]*seg[0]+seg[1]*seg[1]+seg[2]*seg[2]);
        if (sl>1e-3f){ vec3 sd={seg[0]/sl,seg[1]/sl,seg[2]/sl}; ex_vfx_spawn_shot(&g_vfx,(vec3){prev[0],prev[1],prev[2]},sd,sl,lc); }
        ex_enemy_damage(en, dmg);
        g_enemy_hurt[chain[k]]=0.18f; g_enemy_startle[chain[k]]=0.4f;      // arc stuns briefly
        ex_vfx_spawn_impact(&g_vfx, to, 10, lc);
        prev[0]=to[0]; prev[1]=to[1]; prev[2]=to[2];
        dmg *= 0.82f;                                                       // weakens down the chain
      }
      snprintf(g_shot_msg,sizeof(g_shot_msg),"CHAIN LIGHTNING x%d", nc); g_shot_timer=1.2f;
      g_cam_trauma += 0.3f; sfx(g_sfx_expl);
      g_arc_cd = 4.5f;
    }
    ex_keys_down[SDL_SCANCODE_F]=0;
  }
  // Star-fire (Billy, the Fire Clone: absorb & fire a star). Charge by holding
  // RELOAD while in a fire-element form; the charge multiplies the next shot.
  {
    int charging = is_fire_form && ex_keybind_down(&g_keys, EX_ACT_RELOAD, ex_keys_down);
    ex_starfire_update(&g_starfire, dt, charging);
    if (charging && g_starfire.charge < 0.05f) ex_starfire_begin(&g_starfire);
    // the held star: a growing, brightening core out in front of the hand
    if (g_star_orb){
      float ch=g_starfire.charge;
      if (g_starfire.charging && ch>0.02f){
        float fx=camera->front[0], fz=camera->front[2]; float fl=sqrtf(fx*fx+fz*fz); if(fl>1e-3f){fx/=fl;fz/=fl;}
        g_star_orb->position[0]=e->position[0]+fx*1.7f;
        g_star_orb->position[1]=e->position[1]+1.5f;
        g_star_orb->position[2]=e->position[2]+fz*1.7f;
        float r=0.25f+ch*1.25f; g_star_orb->scale3[0]=g_star_orb->scale3[1]=g_star_orb->scale3[2]=r;
        g_star_orb->tint[0]=1.0f; g_star_orb->tint[1]=0.75f+0.25f*ch; g_star_orb->tint[2]=0.25f+0.55f*ch;
      } else g_star_orb->position[1]=-100000.0f;
    }
  }
  // Tsunami (the Watch's water form): press V to surge a wall of water forward --
  // a directional crest that shoves and drowns everything in its lane.
  if (g_tsunami_cd > 0.0f) g_tsunami_cd -= (float)dt;
  if (is_water_form && ex_keys_down[SDL_SCANCODE_V] && !g_tsunami.active && g_tsunami_cd <= 0.0f && !g_human_scale) {
    vec3 org={e->position[0], e->position[1], e->position[2]};
    vec3 dir={camera->front[0], 0.0f, camera->front[2]};
    ex_tsunami_arm(&g_tsunami, org, dir);
    g_tsunami_cd = 7.0f;
    snprintf(g_shot_msg,sizeof(g_shot_msg),"TSUNAMI"); g_shot_timer=1.6f;
    g_cam_trauma += 0.4f; sfx(g_sfx_expl);
    ex_keys_down[SDL_SCANCODE_V]=0;
  }
  if (g_tsunami.active) {
    vec3 front;
    ex_tsunami_tick(&g_tsunami, (float)dt, front);
    float fr=18.0f;                              // crest half-width
    for (int q=0;q<g_enc.count;q++){ ex_enemy_t *en=&g_enc.enemies[q]; if(en->state==EX_ENEMY_DEAD) continue;
      float dx=en->position[0]-front[0], dz=en->position[2]-front[2];
      if (dx*dx+dz*dz < fr*fr){ ex_enemy_damage(en, 42.0f*(float)dt);
        int wi=(int)(en-g_enc.enemies);
        if (wi>=0&&wi<EX_MAX_ENEMIES){ g_enemy_vel[wi][0]+=g_tsunami.dir[0]*70.0f*(float)dt;
          g_enemy_vel[wi][2]+=g_tsunami.dir[2]*70.0f*(float)dt; g_enemy_hurt[wi]=0.16f; } } }
    for (int i=0;i<NUM_DEBRIS;i++){ debris_t *dbr=&g_debris[i]; if(!dbr->active) continue;
      float dx=dbr->pos[0]-front[0], dz=dbr->pos[2]-front[2];
      if (dx*dx+dz*dz < fr*fr){ dbr->vel[0]+=g_tsunami.dir[0]*40.0f*(float)dt; dbr->vel[2]+=g_tsunami.dir[2]*40.0f*(float)dt; dbr->vel[1]+=18.0f*(float)dt; } }
    spawn_shockring(front[0], 0.4f, front[2], fr);
    { uint8_t wc[3]={120,190,255};
      float px=-g_tsunami.dir[2], pz=g_tsunami.dir[0];    // across the crest
      for (int k=-2;k<=2;k++){ vec3 wp={front[0]+px*k*6.0f, 1.4f, front[2]+pz*k*6.0f};
        ex_vfx_spawn_impact(&g_vfx, wp, 5, wc); } }
  }
  // Quantum warp jump (Ice Clones). Tap K in an ice form to enter an interactive
  // TOP-DOWN landing view: pick a drop zone, then slam down with a late-arrival
  // impact (shockwave + crater + ice burst).
  if (is_ice_form && ex_keys_down[SDL_SCANCODE_K] && !g_landing_active) {
    g_landing_active = 1;
    g_landing_tgt[0] = e->position[0] + camera->front[0]*45.0f;
    g_landing_tgt[2] = e->position[2] + camera->front[2]*45.0f;
    g_landing_tgt[1] = 0.3f; g_landing_spin = 0.0f;
    snprintf(g_shot_msg,sizeof(g_shot_msg),"WARP: select landing zone"); g_shot_timer=1.8f;
    ex_keys_down[SDL_SCANCODE_K]=0;
  }
  if (g_landing_active) {
    // drive the reticle across the map (WASD), clamped to the arena
    float mv = 90.0f * (float)dt;
    if (ex_keys_down[SDL_SCANCODE_W]) g_landing_tgt[2]-=mv;
    if (ex_keys_down[SDL_SCANCODE_S]) g_landing_tgt[2]+=mv;
    if (ex_keys_down[SDL_SCANCODE_A]) g_landing_tgt[0]-=mv;
    if (ex_keys_down[SDL_SCANCODE_D]) g_landing_tgt[0]+=mv;
    const float LB=1380.0f;
    if (g_landing_tgt[0]<-LB)g_landing_tgt[0]=-LB; if(g_landing_tgt[0]>LB)g_landing_tgt[0]=LB;
    if (g_landing_tgt[2]<-LB)g_landing_tgt[2]=-LB; if(g_landing_tgt[2]>LB)g_landing_tgt[2]=LB;
    g_landing_spin += (float)dt*2.5f;
    // confirm with click or K -> WARP + impact
    int confirm = (ex_keys_down[SDL_SCANCODE_K]) ||
                  ((SDL_GetMouseState(0,0)&SDL_BUTTON(SDL_BUTTON_LEFT))!=0);
    if (confirm) {
      // SILENT INSERTION: the leap itself is undetectable -- no shockwave, no
      // crack in space. One frame you're here, then you're there. Only a faint
      // crystalline shimmer marks the arrival.
      e->position[0]=g_landing_tgt[0]; e->position[2]=g_landing_tgt[2];
      e->velocity[0]=e->velocity[2]=0; e->velocity[1]=-8.0f;   // settle gently
      { uint8_t ic[3]={190,225,255}; ex_vfx_spawn_impact(&g_vfx,(vec3){g_landing_tgt[0],1.2f,g_landing_tgt[2]},6,ic); }
      // ...but gravity has to catch up. Arm the wave to arrive a few seconds
      // later at the landing point -- you know it's coming and can clear out,
      // but you cannot stop or shape it, and it doesn't care who's standing there.
      ex_warp_wave_arm(&g_warp_wave, (vec3){g_landing_tgt[0], g_landing_tgt[1], g_landing_tgt[2]}, 4.0f);
      g_landing_active = 0;
      snprintf(g_shot_msg,sizeof(g_shot_msg),"WARP -- gravity wave inbound"); g_shot_timer=1.8f;
      ex_keys_down[SDL_SCANCODE_K]=0;
    }
  }
  // DELAYED GRAVITY WAVE: telegraph the inbound wave with a tightening ring at
  // the landing point, then fire it when the clock runs out. LIGO-like: the
  // ground "breathes" outward rather than simply exploding.
  if (g_warp_wave.armed) {
    float prog = ex_warp_wave_progress(&g_warp_wave);   // 0 -> 1 as it closes in
    if (g_landing_ring) {
      float rr = 46.0f * (1.0f - prog) + 6.0f;           // tightens as it nears
      g_landing_ring->position[0]=g_warp_wave.pos[0];
      g_landing_ring->position[1]=0.35f;
      g_landing_ring->position[2]=g_warp_wave.pos[2];
      g_landing_ring->scale3[0]=g_landing_ring->scale3[2]=rr;
      g_landing_ring->scale3[1]=0.4f;
      float pulse = 0.5f + 0.5f*sinf(g_anim_t*(8.0f+prog*22.0f));  // beats faster near impact
      g_landing_ring->tint[0]=0.55f+0.45f*pulse; g_landing_ring->tint[1]=0.75f; g_landing_ring->tint[2]=1.0f;
    }
    if (ex_warp_wave_tick(&g_warp_wave, dt)) {
      float wx=g_warp_wave.pos[0], wz=g_warp_wave.pos[2], wr=46.0f;
      apply_shockwave(wx, g_warp_wave.pos[1], wz, wr, 40.0f, 40.0f);   // hits friend and foe alike
      spawn_shockring(wx, 0.4f, wz, wr);
      spawn_shockring(wx, 0.4f, wz, wr*0.62f);
      spawn_shockring(wx, 0.4f, wz, wr*0.32f);
      spawn_crater(wx, wz, 5.5f);
      { uint8_t ic[3]={170,220,255}; ex_vfx_spawn_impact(&g_vfx,(vec3){wx,1.5f,wz},34,ic);
        for(int c=0;c<14;c++) spawn_debris(wx,1.0f,wz,0.22f,0.6f,0.8f,1.0f,1.5f); }
      g_cam_trauma += 0.9f; sfx(g_sfx_expl);
      if (g_landing_ring) g_landing_ring->position[1]=-100000.0f;   // clear the telegraph
      snprintf(g_shot_msg,sizeof(g_shot_msg),"GRAVITY WAVE"); g_shot_timer=1.6f;
    }
  }

  // Resonance pulse (signature devourer power, ported from the source's Apex
  // resonance): tap G to emit a shockwave that flings nearby enemies + debris
  // outward and damages them. Cooldown-gated.
  if (ex_keys_down[SDL_SCANCODE_G] && g_resonance_cd <= 0.0f && !g_human_scale) {
    apply_shockwave(e->position[0], e->position[1], e->position[2], 32.0f, 28.0f, 22.0f);
    spawn_shockring(e->position[0], e->position[1], e->position[2], 32.0f);
    uint8_t rc[3]={90,230,255}; ex_vfx_spawn_impact(&g_vfx, (vec3){e->position[0],e->position[1]+1.0f,e->position[2]}, 22, rc);
    sfx(g_sfx_expl); g_cam_trauma += 0.5f;
    snprintf(g_shot_msg,sizeof(g_shot_msg),"RESONANCE PULSE"); g_shot_timer=1.4f;
    g_resonance_cd = 10.0f;
    ex_keys_down[SDL_SCANCODE_G]=0;
  }
  if (g_resonance_cd > 0.0f) g_resonance_cd -= dt;

  // Pulse blast (T): a focused forward cone shockwave -- shorter range, aimed.
  if (ex_keys_down[SDL_SCANCODE_T] && g_pulse_cd <= 0.0f && !g_human_scale) {
    float fx=camera->front[0], fz=camera->front[2]; float fl=sqrtf(fx*fx+fz*fz); if(fl>1e-3f){fx/=fl;fz/=fl;}
    float cx=e->position[0]+fx*14.0f, cz=e->position[2]+fz*14.0f;   // burst out in front
    apply_shockwave(cx, e->position[1], cz, 20.0f, 24.0f, 26.0f); g_spark_loud=1.0f;
    spawn_shockring(cx, e->position[1], cz, 20.0f);
    uint8_t pc[3]={255,150,90}; ex_vfx_spawn_impact(&g_vfx,(vec3){cx,e->position[1]+1.0f,cz},16,pc);
    sfx(g_sfx_expl); g_cam_trauma+=0.3f;
    snprintf(g_shot_msg,sizeof(g_shot_msg),"PULSE BLAST"); g_shot_timer=1.2f; g_pulse_cd=6.0f;
    ex_keys_down[SDL_SCANCODE_T]=0;
  }
  if (g_pulse_cd > 0.0f) g_pulse_cd -= dt;

  // Blink (Y): short-range teleport along the aim -- a stealth reposition.
  if (ex_keys_down[SDL_SCANCODE_Y] && g_blink_cd <= 0.0f) {
    float fx=camera->front[0], fz=camera->front[2]; float fl=sqrtf(fx*fx+fz*fz); if(fl>1e-3f){fx/=fl;fz/=fl;}
    uint8_t bc[3]={120,180,255}; ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0],e->position[1]+1,e->position[2]},14,bc);
    e->position[0]+=fx*30.0f; e->position[2]+=fz*30.0f;
    ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0],e->position[1]+1,e->position[2]},14,bc);
    sfx(g_sfx_xform); snprintf(g_shot_msg,sizeof(g_shot_msg),"BLINK"); g_shot_timer=1.0f; g_blink_cd=5.0f;
    ex_keys_down[SDL_SCANCODE_Y]=0;
  }
  if (g_blink_cd > 0.0f) g_blink_cd -= dt;

  // ---- fire: hitscan scaled by element ----
  if (g_form_cd > 0.0f) g_form_cd -= dt;   // recover the per-form fire cooldown
  if (g_recoil > 0.0f) g_recoil -= dt*2.2f; if (g_recoil<0) g_recoil=0;
  g_barrel_heat -= dt*0.6f; if (g_barrel_heat<0) g_barrel_heat=0; if (g_barrel_heat>1.2f) g_barrel_heat=1.2f;
  g_cam_trauma -= dt*1.5f; if (g_cam_trauma<0) g_cam_trauma=0; if (g_cam_trauma>1.0f) g_cam_trauma=1.0f;
  g_player_slow -= dt; if (g_player_slow<0) g_player_slow=0;
  for (int i=0;i<EX_MAX_ENEMIES;i++){ g_enemy_hurt[i]-=dt*6.0f; if(g_enemy_hurt[i]<0) g_enemy_hurt[i]=0; }
  if (g_hit_flash > 0.0f) g_hit_flash -= dt;
  // landing squash: when we hit the ground after being airborne, cushion it
  if (g_was_airborne && e->velocity[1] < 0) g_land_impact = -e->velocity[1];
  if (g_was_airborne && e->grounded == 1) { g_land_squash = 0.28f;
    // a heavy landing after a big drop scars the ground and kicks up debris
    if (g_land_impact > 22.0f && !g_human_scale) {
      spawn_crater(e->position[0], e->position[2], 2.2f);
      const ex_material_t *m=ex_material(g_ground_mat);
      uint8_t col[3]={m->spark[0],m->spark[1],m->spark[2]};
      for (int c=0;c<5;c++) spawn_debris(e->position[0], e->position[1]-0.8f, e->position[2], 0.12f, col[0]/255.0f,col[1]/255.0f,col[2]/255.0f, 0.7f);
      g_cam_trauma += 0.25f;
    }
  }
  g_was_airborne = (e->grounded != 1);
  if (g_land_squash > 0.0f) g_land_squash -= dt*2.5f; if (g_land_squash<0) g_land_squash=0;
  int fire_held = ex_keybind_down(&g_keys, EX_ACT_FIRE, ex_keys_down)
                  || (SDL_GetMouseState(NULL,NULL) & SDL_BUTTON(SDL_BUTTON_LEFT));
  // ---- SPECIALIST ALT-MODE WEAPONS: some vehicle forms fire a signature attack
  //      instead of the standard bolt (spider web, dino fire breath, scorpion
  //      stinger + dual cannons). ----
  int specialist_veh = in_vehicle && (g_vehicle==EX_VEH_SPIDER||g_vehicle==EX_VEH_DINO||g_vehicle==EX_VEH_SCORPION);
  if (fire_held && g_form_cd <= 0.0f && specialist_veh) {
    ex_target_t tg[EX_MAX_ENEMIES]; int nt = ex_encounter_targets(&g_enc, tg, EX_MAX_ENEMIES);
    vec3 fdir = { camera->front[0], camera->front[1], camera->front[2] };
    vec3 muz  = { e->position[0], e->position[1]+1.5f, e->position[2] };
    float fx=camera->front[0], fz=camera->front[2]; float fl=sqrtf(fx*fx+fz*fz); if(fl>1e-3f){fx/=fl;fz/=fl;}
    if (g_vehicle==EX_VEH_SPIDER) {
      // WEB SHOT: sticky bolt that damages + slows (roots) the struck enemy
      ex_hit_t h = ex_combat_raycast(g_chunks, tg, nt, muz, fdir, 170.0f);
      uint8_t wc[3]={225,232,240}; ex_vfx_spawn_shot(&g_vfx, muz, fdir, h.dist, wc);
      if (h.kind==EX_HIT_ENEMY){ ex_enemy_t *t=ex_encounter_by_id(&g_enc, h.index);
        if (t){ ex_enemy_damage(t, 14.0f); int ti=(int)(t-g_enc.enemies);
          if(ti>=0&&ti<EX_MAX_ENEMIES){ g_enemy_web[ti]=3.0f; g_enemy_hurt[ti]=0.2f; }
          float ip[3]={muz[0]+fdir[0]*h.dist, muz[1]+fdir[1]*h.dist, muz[2]+fdir[2]*h.dist};
          ex_vfx_spawn_impact(&g_vfx,(vec3){ip[0],ip[1],ip[2]},10,wc); } }
      snprintf(g_shot_msg,sizeof(g_shot_msg),"WEB SHOT"); g_shot_timer=0.8f; g_form_cd=0.55f;
    } else if (g_vehicle==EX_VEH_DINO) {
      // FIRE BREATH: a short forward cone of flame torching everything in front
      for (int i=0;i<g_enc.count;i++){ ex_enemy_t *en=&g_enc.enemies[i]; if(en->state==EX_ENEMY_DEAD) continue;
        float dx=en->position[0]-e->position[0], dz=en->position[2]-e->position[2];
        float d=sqrtf(dx*dx+dz*dz); if(d>34.0f||d<0.1f) continue;
        if ((dx/d)*fx+(dz/d)*fz > 0.6f){ ex_enemy_damage(en, 22.0f*(float)dt*6.0f);
          g_enemy_hurt[i]=0.15f;
          uint8_t fc[3]={255,(uint8_t)(120+ (rand()%80)),40};
          ex_vfx_spawn_impact(&g_vfx,(vec3){en->position[0],en->position[1]+1.0f,en->position[2]},3,fc); } }
      // flame plume out of the maw
      for (int k=0;k<4;k++){ float t=(float)k/4.0f*30.0f; uint8_t fc[3]={255,(uint8_t)(150-k*20),30};
        ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0]+fx*(6.0f+t), e->position[1]+1.6f, e->position[2]+fz*(6.0f+t)},2,fc); }
      sfx(g_sfx_fire); g_barrel_heat+=0.05f; g_cam_trauma+=0.05f;
      snprintf(g_shot_msg,sizeof(g_shot_msg),"FIRE BREATH"); g_shot_timer=0.6f; g_form_cd=0.10f;
    } else { // SCORPION: dual cannons + a close stinger strike
      for (int s=-1;s<=1;s+=2){ float a=s*0.06f; float ca=cosf(a),sa=sinf(a);
        vec3 d2={ fdir[0]*ca - fdir[2]*sa, fdir[1], fdir[0]*sa + fdir[2]*ca };
        ex_hit_t h=ex_combat_raycast(g_chunks, tg, nt, muz, d2, 180.0f);
        uint8_t cc[3]={255,200,120}; ex_vfx_spawn_shot(&g_vfx, muz, d2, h.dist, cc);
        if (h.kind==EX_HIT_ENEMY){ ex_enemy_t *t=ex_encounter_by_id(&g_enc,h.index);
          if(t){ ex_enemy_damage(t,12.0f); int ti=(int)(t-g_enc.enemies); if(ti>=0&&ti<EX_MAX_ENEMIES) g_enemy_hurt[ti]=0.15f; } } }
      // stinger: heavy hit on a very close enemy in front
      for (int i=0;i<g_enc.count;i++){ ex_enemy_t *en=&g_enc.enemies[i]; if(en->state==EX_ENEMY_DEAD) continue;
        float dx=en->position[0]-e->position[0], dz=en->position[2]-e->position[2]; float d=sqrtf(dx*dx+dz*dz);
        if (d<9.0f && (dx/(d+0.01f))*fx+(dz/(d+0.01f))*fz>0.3f){ int killed=ex_enemy_damage(en,40.0f); g_enemy_hurt[i]=0.2f;
          if(killed){ spawn_explosion(en->position[0],en->position[1],en->position[2],1.0f,6); sfx(g_sfx_expl);} break; } }
      sfx(g_sfx_fire); g_recoil=0.4f; g_cam_trauma+=0.08f;
      snprintf(g_shot_msg,sizeof(g_shot_msg),"DUAL CANNON + STINGER"); g_shot_timer=0.7f; g_form_cd=0.5f;
    }
  }
  if (fire_held && g_form_cd <= 0.0f && !specialist_veh) {
    ex_weapon_profile_t wp = ex_weapon_apply_mods(g_form, &g_wmods);
    wp.damage   *= g_sig_stats.dmg_mul;    // signature offense (heavy cannon hits hard)
    wp.cooldown *= g_sig_stats.fire_mul;   // signature refire (cable fastest, cannon slow)
    if (ex_photon_active(&g_photon)) { wp.damage *= 1.9f; wp.cooldown *= 0.7f; }  // light form: overwhelming
    if (ex_matrix_active(&g_matrix)) { wp.damage *= 1.7f; wp.cooldown *= 0.65f; } // Rodimus: offensive spike
    ex_target_t tgts[EX_MAX_ENEMIES];
    int ntg = ex_encounter_targets(&g_enc, tgts, EX_MAX_ENEMIES);
    vec3 muzzle = { e->position[0], e->position[1] + 1.5f, e->position[2] };
    int was_star = 0;
    float star = ex_starfire_release(&g_starfire, &was_star);
    if (g_overcharge_t > 0.0f) star *= 1.6f;
    if (was_star) {
      // BILLY FIRES A STAR: a radiant nova bursts out ahead -- huge, wide, and it
      // costs him (a lockout follows). Placed far enough forward not to burn you.
      float fx=camera->front[0], fz=camera->front[2]; float fl=sqrtf(fx*fx+fz*fz); if(fl>1e-3f){fx/=fl;fz/=fl;}
      float nx=e->position[0]+fx*40.0f, nz=e->position[2]+fz*40.0f;
      apply_shockwave(nx, e->position[1], nz, 30.0f, 32.0f, 120.0f);   // star-tier damage
      spawn_shockring(nx, 0.5f, nz, 30.0f); spawn_shockring(nx, 0.5f, nz, 18.0f);
      { uint8_t wc[3]={255,240,180}; ex_vfx_spawn_impact(&g_vfx,(vec3){nx,1.4f,nz},40,wc); }
      for(int c=0;c<12;c++) spawn_debris(nx,1.0f,nz,0.9f,0.85f,0.6f,0.3f,1.4f);
      g_cam_trauma += 1.1f; g_spark_loud=1.0f; sfx(g_sfx_expl);
      snprintf(g_shot_msg,sizeof(g_shot_msg),"STAR RELEASED"); g_shot_timer=2.2f;
    }
    if (ex_ability_active_any(&g_abilities, EX_ABIL_OVERCLOCK)) wp.cooldown *= 0.5f;
    int pellets = (wp.mode == EX_FIRE_BURST) ? wp.pellets : 1;
    pellets += g_sig_combat.extra_pellets; if (pellets > 8) pellets = 8;  // signature dual/spread fire
    float sig_spread = wp.spread_deg;
    if (g_sig_combat.extra_pellets > 0 && sig_spread < 4.0f) sig_spread = 6.0f;  // give the added bolt a gap
    g_form_cd = wp.cooldown;

    for (int s = 0; s < pellets; s++) {
      vec3 dir = { camera->front[0], camera->front[1], camera->front[2] };
      if (pellets > 1) {
        float a = ((float)s/(pellets-1) - 0.5f) * 2.0f * (sig_spread*0.0174533f);
        float ca=cosf(a), sa=sinf(a);
        float nx = dir[0]*ca - dir[2]*sa, nz = dir[0]*sa + dir[2]*ca; dir[0]=nx; dir[2]=nz;
      }
      float fire_range = wp.range * 1.8f;   // longer reach for the bigger maps
      if (wp.mode == EX_FIRE_PIERCE) {
        // railshot: a wall stops it, but it passes through every enemy on the way.
        ex_hit_t gh = ex_combat_raycast(g_chunks, tgts, 0, muzzle, dir, fire_range);
        float capd = (gh.kind==EX_HIT_CHUNK) ? gh.dist : fire_range;
        ex_vfx_spawn_shot(&g_vfx, muzzle, dir, capd, wp.color);
        int nhit=0;
        for (int c=0;c<g_enc.count;c++){ ex_enemy_t *o=&g_enc.enemies[c]; if(o->state==EX_ENEMY_DEAD) continue;
          float rx=o->position[0]-muzzle[0], ry=o->position[1]-muzzle[1], rz=o->position[2]-muzzle[2];
          float t = rx*dir[0]+ry*dir[1]+rz*dir[2];
          if (t<=0.0f || t>capd) continue;
          float cxp=muzzle[0]+dir[0]*t, cyp=muzzle[1]+dir[1]*t, czp=muzzle[2]+dir[2]*t;
          float px=o->position[0]-cxp, py=o->position[1]-cyp, pz=o->position[2]-czp;
          if (sqrtf(px*px+py*py+pz*pz) < ex_enemy_visual_radius(o)+1.3f){
            float dmg = wp.damage * star * ex_element_advantage(wp.element,o->element);
            int killed=ex_enemy_damage(o,dmg); int oi=(int)(o-g_enc.enemies);
            if(oi>=0&&oi<EX_MAX_ENEMIES) g_enemy_hurt[oi]=0.16f;
            uint8_t rc[3]={210,225,255}; ex_vfx_spawn_impact(&g_vfx,(vec3){o->position[0],o->position[1]+1.0f,o->position[2]},7,rc);
            if(killed){ ex_vfx_spawn_death(&g_vfx,o->position,wp.color); spawn_explosion(o->position[0],o->position[1],o->position[2],1.0f,6); }
            nhit++;
          }
        }
        if (nhit>0){ snprintf(g_shot_msg,sizeof(g_shot_msg),"RAILGUN -- pierced %d", nhit); g_shot_timer=1.4f; }
        g_recoil=0.5f; g_barrel_heat+=0.2f; g_spark_loud=fminf(1.0f,g_spark_loud+0.4f);
        sfx(g_sfx_fire); g_cam_trauma+=0.12f;
        continue;   // this pellet is fully resolved
      }
      ex_hit_t hit = ex_combat_raycast(g_chunks, tgts, ntg, muzzle, dir, fire_range);
      ex_vfx_spawn_shot(&g_vfx, muzzle, dir, hit.dist, wp.color);
      // impact sparks at whatever the bolt hit (enemy or geometry)
      if (hit.kind != EX_HIT_NONE) {
        float ip[3]={ muzzle[0]+dir[0]*hit.dist, muzzle[1]+dir[1]*hit.dist, muzzle[2]+dir[2]*hit.dist };
        uint8_t sparkc[3]={255,225,150};
        ex_vfx_spawn_impact(&g_vfx, ip, (hit.kind==EX_HIT_ENEMY)?7:5, sparkc);
        kick_debris(ip[0], ip[2], 6.0f, 12.0f);   // rounds kick up nearby debris
        if (wp.mode == EX_FIRE_FLAK) {             // airburst: splash everything nearby
          float fr2=13.0f*13.0f;
          for (int c=0;c<g_enc.count;c++){ ex_enemy_t *o=&g_enc.enemies[c]; if(o->state==EX_ENEMY_DEAD) continue;
            float sdx=o->position[0]-ip[0], sdz=o->position[2]-ip[2];
            if (sdx*sdx+sdz*sdz<=fr2){ ex_enemy_damage(o, wp.damage*star*0.55f);
              int oi=(int)(o-g_enc.enemies); if(oi>=0&&oi<EX_MAX_ENEMIES) g_enemy_hurt[oi]=0.16f; } }
          spawn_shockring(ip[0], ip[1], ip[2], 13.0f);
          uint8_t fc[3]={255,150,60}; ex_vfx_spawn_impact(&g_vfx,(vec3){ip[0],ip[1],ip[2]},16,fc);
          g_cam_trauma += 0.15f;
        }
        if (wp.mode == EX_FIRE_PLASMA) {           // heavy electric ball -> a big blast
          float pr=22.0f, pr2=pr*pr;
          for (int c=0;c<g_enc.count;c++){ ex_enemy_t *o=&g_enc.enemies[c]; if(o->state==EX_ENEMY_DEAD) continue;
            float sdx=o->position[0]-ip[0], sdz=o->position[2]-ip[2]; float d2=sdx*sdx+sdz*sdz;
            if (d2<=pr2){ float dd=sqrtf(d2), fall=1.0f-dd/pr;
              ex_enemy_damage(o, wp.damage*star*(0.5f+0.5f*fall));
              int oi=(int)(o-g_enc.enemies); if(oi>=0&&oi<EX_MAX_ENEMIES){ g_enemy_hurt[oi]=0.18f;
                if(dd>1e-3f){ g_enemy_vel[oi][0]+=sdx/dd*22.0f; g_enemy_vel[oi][2]+=sdz/dd*22.0f; } } } }
          spawn_shockring(ip[0], ip[1], ip[2], pr); spawn_shockring(ip[0], ip[1], ip[2], pr*0.6f);
          uint8_t pc[3]={150,190,255}; ex_vfx_spawn_impact(&g_vfx,(vec3){ip[0],ip[1],ip[2]},34,pc);
          g_cam_trauma += 0.4f; g_spark_loud=fminf(1.0f,g_spark_loud+0.5f);
          snprintf(g_shot_msg,sizeof(g_shot_msg),"PLASMA BLAST"); g_shot_timer=1.2f;
        }
        if (wp.mode == EX_FIRE_RICOCHET && hit.kind == EX_HIT_CHUNK) {   // bounce off the wall to a nearby foe
          int best=-1; float bd=30.0f*30.0f;
          for (int c=0;c<g_enc.count;c++){ ex_enemy_t *o=&g_enc.enemies[c]; if(o->state==EX_ENEMY_DEAD) continue;
            float sdx=o->position[0]-ip[0], sdz=o->position[2]-ip[2]; float d2=sdx*sdx+sdz*sdz;
            if (d2<bd){ bd=d2; best=c; } }
          if (best>=0){ ex_enemy_t *o=&g_enc.enemies[best];
            vec3 bfrom={ip[0],ip[1],ip[2]}, bdir={o->position[0]-ip[0], (o->position[1]+1.0f)-ip[1], o->position[2]-ip[2]};
            float bl=sqrtf(bdir[0]*bdir[0]+bdir[1]*bdir[1]+bdir[2]*bdir[2]); if(bl>1e-3f){bdir[0]/=bl;bdir[1]/=bl;bdir[2]/=bl;}
            ex_vfx_spawn_shot(&g_vfx, bfrom, bdir, bl, wp.color);
            int killed=ex_enemy_damage(o, wp.damage*star*ex_element_advantage(wp.element,o->element)*0.85f);
            g_enemy_hurt[best]=0.16f;
            uint8_t rc[3]={210,210,220}; ex_vfx_spawn_impact(&g_vfx,(vec3){o->position[0],o->position[1]+1.0f,o->position[2]},7,rc);
            if(killed){ ex_vfx_spawn_death(&g_vfx,o->position,wp.color); sfx(g_sfx_expl); }
            snprintf(g_shot_msg,sizeof(g_shot_msg),"RICOCHET"); g_shot_timer=1.0f;
          }
        }
      }
    g_recoil = 0.35f;   // kick the weapon back
    g_barrel_heat += 0.14f;   // barrel heats up with sustained fire
    g_spark_loud = fminf(1.0f, g_spark_loud+0.35f);   // gunfire lights up your spark signature
    sfx(g_sfx_fire);
    g_cam_trauma += 0.06f;   // subtle recoil shake
      if (hit.kind == EX_HIT_ENEMY) {
        ex_enemy_t *t = ex_encounter_by_id(&g_enc, hit.index);
        if (t) {
          // damage falloff: full up close, tapering to 45% at extreme range
          float nearr = fire_range*0.3f, dmg_fall = 1.0f;
          if (hit.dist > nearr) dmg_fall = 1.0f - 0.55f*fminf((hit.dist-nearr)/(fire_range-nearr), 1.0f);
          float dmg = wp.damage * star * ex_element_advantage(wp.element, t->element) * dmg_fall;
          int killed = ex_enemy_damage(t, dmg);
          { int ti=(int)(t-g_enc.enemies);
            if (ti>=0 && ti<EX_MAX_ENEMIES){
              g_enemy_hurt[ti]=0.16f;
              if (wp.slow>0.0f && g_enemy_web[ti] < wp.slow*3.0f) g_enemy_web[ti] = wp.slow*3.0f;  // ice / suppression freeze
              // signature on-hit identity: siege knockback + tether slow/root
              if (g_sig_combat.knockback>0.0f){
                float kdx=t->position[0]-e->position[0], kdz=t->position[2]-e->position[2];
                float kd=sqrtf(kdx*kdx+kdz*kdz); if(kd>1e-3f){kdx/=kd;kdz/=kd;}
                g_enemy_vel[ti][0]+=kdx*g_sig_combat.knockback;
                g_enemy_vel[ti][2]+=kdz*g_sig_combat.knockback;
              }
              if (g_sig_combat.slow>0.0f && g_enemy_web[ti]<g_sig_combat.slow) g_enemy_web[ti]=g_sig_combat.slow;
            } }
          // signature splash: nearby enemies take partial damage (heavy cannon)
          if (g_sig_combat.splash>0.0f){
            float sr2=g_sig_combat.splash*g_sig_combat.splash;
            for (int c=0;c<g_enc.count;c++){ ex_enemy_t *o=&g_enc.enemies[c];
              if (o==t || o->state==EX_ENEMY_DEAD) continue;
              float sdx=o->position[0]-t->position[0], sdz=o->position[2]-t->position[2];
              if (sdx*sdx+sdz*sdz<=sr2){ ex_enemy_damage(o, dmg*0.4f);
                int oi=(int)(o-g_enc.enemies); if(oi>=0&&oi<EX_MAX_ENEMIES) g_enemy_hurt[oi]=0.16f; } }
          }
          snprintf(g_shot_msg, sizeof(g_shot_msg), killed ? "%s DOWN" : "%s hit %.0f",
                   t->kind==EX_ENEMY_CORRUPTED?"corrupted":"grunt", killed?(float)0:dmg);
          if (killed) { ex_vfx_spawn_death(&g_vfx, t->position, wp.color); sfx(g_sfx_expl);
            spawn_explosion(t->position[0], t->position[1], t->position[2], 1.0f, 6);
            float ddx=t->position[0]-e->position[0], ddz=t->position[2]-e->position[2];
            float dd=sqrtf(ddx*ddx+ddz*ddz); g_cam_trauma += 0.35f*fmaxf(0.0f,1.0f-dd/60.0f); }
          if (wp.chains > 0) { // arc to another live enemy
            for (int c=0;c<g_enc.count;c++){ ex_enemy_t *o=&g_enc.enemies[c];
              if (o!=t && o->state!=EX_ENEMY_DEAD){ ex_enemy_damage(o, dmg*0.5f); { int oi=(int)(o-g_enc.enemies); if(oi>=0&&oi<EX_MAX_ENEMIES) g_enemy_hurt[oi]=0.16f; } break; } }
          }
        }
      } else if (hit.kind == EX_HIT_CHUNK) {
        ex_coll_chunk_t *ch = g_chunks->chunks[hit.index];
        int destroyed = ex_chunk_apply_damage(ch, wp.damage*star, EX_CHUNK_HIT_HEAVY);
        if (destroyed) { ex_event_fire(g_bus, "wall_breached", 0, 0.0f);
          if (hit.index < g_arena.count && g_feature_models[hit.index])
            g_feature_models[hit.index]->position[1] = -1000.0f; }
        snprintf(g_shot_msg, sizeof(g_shot_msg), destroyed?"cover destroyed":"cover %.0f", ch->hp);
      }
    }
    if (star > 1.5f) snprintf(g_shot_msg, sizeof(g_shot_msg), "STAR-FIRE x%.0f", star);
    g_shot_timer = 2.0f;
  }

  // ---- turret: auto-fire at nearest enemy while deployed ----
  // ammo variants (cycle with U): 0 standard 1 heal 2 rocket 3 mine 4 arc 5 flak
  if (g_turret.active && ex_keys_down[SDL_SCANCODE_U]) {
    g_turret_ammo=(g_turret_ammo+1)%6; ex_keys_down[SDL_SCANCODE_U]=0;
    static const char *an[]={"standard","heal","rocket","mine","arc","flak"};
    snprintf(g_shot_msg,sizeof(g_shot_msg),"turret: %s", an[g_turret_ammo]); g_shot_timer=1.2f;
  }
  if (g_turret.active) {
    ex_target_t tt[EX_MAX_ENEMIES]; int ntt = ex_encounter_targets(&g_enc, tt, EX_MAX_ENEMIES);
    if (g_turret_ammo==1) {
      // HEAL BEAM: mends the player + nearby survivors instead of shooting
      float dx=e->position[0]-g_turret.pos[0], dz=e->position[2]-g_turret.pos[2];
      if (dx*dx+dz*dz < 60.0f*60.0f){ g_player_hp=fminf(g_player_max_hp, g_player_hp+14.0f*(float)dt);
        if (((int)(g_anim_t*20))%3==0){ uint8_t hc[3]={120,255,160};
          ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0],e->position[1]+2.0f,e->position[2]},2,hc); } }
    } else if (g_turret_ammo==3) {
      // MINE PULSE: emits a shockwave ring on a slow cadence
      static float mine_t=0; mine_t-=(float)dt;
      if (mine_t<=0.0f){ mine_t=2.5f; apply_shockwave(g_turret.pos[0],g_turret.pos[1],g_turret.pos[2], 26.0f, 20.0f, 20.0f);
        spawn_shockring(g_turret.pos[0],g_turret.pos[1],g_turret.pos[2],26.0f); sfx(g_sfx_expl); }
    } else if (ex_turret_update(&g_turret, dt, tt, ntt)) {
      ex_enemy_t *tg = ex_encounter_by_id(&g_enc, g_turret.target_id);
      if (tg) {
        vec3 d={tg->position[0]-g_turret.pos[0],tg->position[1]+1.0f-g_turret.pos[1],tg->position[2]-g_turret.pos[2]};
        float l=sqrtf(d[0]*d[0]+d[1]*d[1]+d[2]*d[2]); if(l>0.001f){d[0]/=l;d[1]/=l;d[2]/=l;}
        if (g_turret_ammo==2) {   // ROCKET: explosive round with a shockwave
          uint8_t rc[3]={255,160,90}; ex_vfx_spawn_shot(&g_vfx, g_turret.pos, d, l, rc);
          spawn_explosion(tg->position[0],tg->position[1],tg->position[2],1.1f,6);
          ex_enemy_damage(tg, g_turret.damage*1.5f);
        } else if (g_turret_ammo==4) {   // ARC: zaps the target then jumps to 2 nearby foes
          uint8_t lc[3]={255,240,120}; ex_vfx_spawn_shot(&g_vfx, g_turret.pos, d, l, lc);
          ex_enemy_damage(tg, g_turret.damage);
          float prev[3]={tg->position[0],tg->position[1]+1.0f,tg->position[2]}; int jumps=0;
          for (int c=0;c<g_enc.count && jumps<2;c++){ ex_enemy_t *o=&g_enc.enemies[c];
            if (o==tg || o->state==EX_ENEMY_DEAD) continue;
            float ax=o->position[0]-prev[0], az=o->position[2]-prev[2];
            if (ax*ax+az*az < 26.0f*26.0f){ vec3 to={o->position[0],o->position[1]+1.0f,o->position[2]};
              vec3 ad={to[0]-prev[0],to[1]-prev[1],to[2]-prev[2]}; float al=sqrtf(ad[0]*ad[0]+ad[1]*ad[1]+ad[2]*ad[2]);
              if(al>1e-3f){ad[0]/=al;ad[1]/=al;ad[2]/=al; ex_vfx_spawn_shot(&g_vfx,(vec3){prev[0],prev[1],prev[2]},ad,al,lc);}
              ex_enemy_damage(o, g_turret.damage*0.7f);
              int oi=(int)(o-g_enc.enemies); if(oi>=0&&oi<EX_MAX_ENEMIES) g_enemy_hurt[oi]=0.14f;
              prev[0]=to[0];prev[1]=to[1];prev[2]=to[2]; jumps++; } }
        } else if (g_turret_ammo==5) {   // FLAK: airburst splashing the target's cluster
          uint8_t fc[3]={255,150,60}; ex_vfx_spawn_shot(&g_vfx, g_turret.pos, d, l, fc);
          ex_enemy_damage(tg, g_turret.damage);
          float fr2=12.0f*12.0f;
          for (int c=0;c<g_enc.count;c++){ ex_enemy_t *o=&g_enc.enemies[c]; if(o==tg||o->state==EX_ENEMY_DEAD) continue;
            float sdx=o->position[0]-tg->position[0], sdz=o->position[2]-tg->position[2];
            if (sdx*sdx+sdz*sdz<=fr2){ ex_enemy_damage(o, g_turret.damage*0.5f);
              int oi=(int)(o-g_enc.enemies); if(oi>=0&&oi<EX_MAX_ENEMIES) g_enemy_hurt[oi]=0.14f; } }
          spawn_shockring(tg->position[0],tg->position[1],tg->position[2],12.0f);
        } else {                  // STANDARD automatic rounds
          uint8_t tc[3]={120,220,255}; ex_vfx_spawn_shot(&g_vfx, g_turret.pos, d, l, tc);
          ex_enemy_damage(tg, g_turret.damage);
        }
      }
    }
    g_turret_model->position[0]=g_turret.pos[0]; g_turret_model->position[1]=g_turret.pos[1];
    g_turret_model->position[2]=g_turret.pos[2]; g_turret_model->scale=0.6f;
    // tint the turret by ammo (blue std, green heal, orange rocket, red mine)
    static const float ac[6][3]={{0.5f,0.85f,1.0f},{0.4f,1.0f,0.6f},{1.0f,0.6f,0.3f},{1.0f,0.4f,0.4f},{1.0f,0.94f,0.5f},{1.0f,0.6f,0.24f}};
    g_turret_model->tint[0]=ac[g_turret_ammo][0]; g_turret_model->tint[1]=ac[g_turret_ammo][1]; g_turret_model->tint[2]=ac[g_turret_ammo][2];
    if (!g_turret.active) g_turret_model->position[1] = -1000.0f;
  } else g_turret_model->position[1] = -1000.0f;

  // ---- wave game loop (skirmish/survival): score kills, advance waves ----
  if (g_wave_mode) {
    // score each enemy that transitioned to dead this frame, by its kind
    for (int i=0;i<g_enc.count;i++){
      int alive = (g_enc.enemies[i].state != EX_ENEMY_DEAD);
      if (g_was_alive[i] && !alive) ex_waves_on_kill(&g_waves, (int)g_enc.enemies[i].kind);
      g_was_alive[i] = alive;
    }
    int alive_now = ex_encounter_alive(&g_enc);
    g_prev_alive = alive_now;
    ex_waves_update(&g_waves, alive_now, dt);
    if (g_waves.pending_spawn) {
      ex_wave_makeup_t mk;
      if (ex_waves_begin_next(&g_waves, &mk)) spawn_wave_makeup(mk);
    }
    if (g_waves.finished) {
      g_form_unlocked_photon = 1;
      end_wave_run(1);   // victory -> results screen
      return;
    }
  }

  // ---- enemies: update all, render each, resolve melee/ranged onto player ----
  // HORROR STALKERS: remember where each enemy was, let the AI move them, then
  // FREEZE any that are inside the player's view cone -- they only advance when
  // you aren't looking. The nearest one feeds a dread meter.
  static float stalk_last[EX_MAX_ENEMIES][3];
  if (g_horror_mode)
    for (int q=0;q<g_enc.count;q++){ stalk_last[q][0]=g_enc.enemies[q].position[0];
      stalk_last[q][1]=g_enc.enemies[q].position[1]; stalk_last[q][2]=g_enc.enemies[q].position[2]; }
  // remember where every enemy was, so we can turn the AI's teleport-style move
  // into a physical one with momentum.
  for (int q=0;q<g_enc.count;q++){ g_enemy_prev[q][0]=g_enc.enemies[q].position[0];
    g_enemy_prev[q][1]=g_enc.enemies[q].position[1]; g_enemy_prev[q][2]=g_enc.enemies[q].position[2]; }
  ex_encounter_update(&g_enc, g_chunks, e->position, dt);
  // MEGATRON'S TRUCE: once the Unicron-tier boss is worn down enough that it
  // threatens everyone, the Decepticon grunts turn on it and fight beside you.
  { ex_enemy_t *tb = ex_encounter_boss_ptr(&g_enc);
    int present = tb && tb->state != EX_ENEMY_DEAD;
    float frac = (present && tb->max_hp > 0.0f) ? tb->hp / tb->max_hp : 1.0f;
    if (!g_truce_on && ex_truce_active(present, frac)) {
      g_truce_on = 1;
      snprintf(g_shot_msg,sizeof(g_shot_msg),"MEGATRON: a truce -- against THAT."); g_shot_timer=3.0f;
    }
  }
  // RESONANCE FIELD (Blackstar): an aware apex/elite is a COORDINATOR -- it
  // projects a field that syncs nearby enemies into acting as one. Collect the
  // active coordinators first, then sync everyone inside their fields below.
  float g_res_coords[EX_MAX_ENEMIES][3]; int g_res_ncoord = 0;
  for (int q=0;q<g_enc.count;q++){ ex_enemy_t *cen=&g_enc.enemies[q];
    if (cen->state!=EX_ENEMY_DEAD && cen->kind==EX_ENEMY_APEX && g_enemy_aware[q]){
      g_res_coords[g_res_ncoord][0]=cen->position[0];
      g_res_coords[g_res_ncoord][1]=cen->position[1];
      g_res_coords[g_res_ncoord][2]=cen->position[2]; g_res_ncoord++; }
  }
  // pulse the field outward from each coordinator so it reads on screen
  g_resfield_cd -= (float)dt;
  if (g_res_ncoord>0 && g_resfield_cd<=0.0f){
    for (int c=0;c<g_res_ncoord;c++) spawn_shockring(g_res_coords[c][0], 0.5f, g_res_coords[c][2], EX_RESONANCE_RADIUS);
    g_resfield_cd = 1.1f;
  }
  // ---- ENEMY PHYSICS: momentum toward the AI's intent (heavier = sluggish),
  //      material drag underfoot, plus decaying knockback from blasts. ----
  if ((float)dt > 1e-5f)
  for (int q=0;q<g_enc.count;q++){ ex_enemy_t *en=&g_enc.enemies[q]; if(en->state==EX_ENEMY_DEAD){ g_enemy_vel[q][0]=g_enemy_vel[q][2]=0; g_corrupt_gen[q]=-1; g_lieut[q]=-1; g_defected[q]=0; g_raider[q]=-1; g_ravager[q]=-1; g_slip[q]=-1;
      if (q==g_champion_slot){ snprintf(g_shot_msg,sizeof(g_shot_msg), g_champion_kind==1 ? "CRYOTEK falls -- the ice line breaks" : "BLACKSTAR falls -- the raiders scatter"); g_shot_timer=3.0f; g_champion_slot=-1; g_champion_kind=0; }
      continue; }
    float mass=enemy_mass(en->kind);
    float ivx=(en->position[0]-g_enemy_prev[q][0])/(float)dt;   // AI-intended velocity
    float ivz=(en->position[2]-g_enemy_prev[q][2])/(float)dt;
    float k=fminf((7.0f/mass)*(float)dt, 1.0f);                 // heavier turns/stops slower
    // COVER-SEEKING: a hurt enemy breaks toward the nearest wall/cover volume,
    // so the enclosed rooms + berms become tactical instead of scenery. Grunts
    // and hijackers duck; heavy Apex/Corrupted press on regardless.
    if (g_enemy_hurt[q] > 0.02f && (en->kind==EX_ENEMY_GRUNT || en->kind==EX_ENEMY_HIJACK)) {
      float bestd=1e9f, bx=0, bz=0; int found=0;
      for (int vi=0; vi<g_blueprint.n_vols; vi++){ ex_map_vol_t *v=&g_blueprint.vols[vi];
        if (v->role!=EX_VOL_LOWCOVER && v->role!=EX_VOL_HIGHCOVER && v->role!=EX_VOL_CONCEAL) continue;
        float dx=v->center[0]-en->position[0], dz=v->center[2]-en->position[2];
        float d2=dx*dx+dz*dz; if (d2<bestd && d2>4.0f){ bestd=d2; bx=v->center[0]; bz=v->center[2]; found=1; } }
      if (found && bestd < 70.0f*70.0f){
        // hide on the side of the cover away from the player
        float px=en->position[0]-e->position[0], pz=en->position[2]-e->position[2];
        float pl=sqrtf(px*px+pz*pz)+0.01f; px/=pl; pz/=pl;
        float tx=bx+px*2.2f, tz=bz+pz*2.2f;
        ivx = (tx-en->position[0])*2.0f; ivz = (tz-en->position[2])*2.0f;   // override intent toward cover
      }
    }
    g_enemy_vel[q][0]+=(ivx-g_enemy_vel[q][0])*k;
    g_enemy_vel[q][2]+=(ivz-g_enemy_vel[q][2])*k;
    float mgrip=ex_material_grip(g_ground_mat, mass, sqrtf(g_enemy_vel[q][0]*g_enemy_vel[q][0]+g_enemy_vel[q][2]*g_enemy_vel[q][2]), 0.7f, 0.0f);
    float drag=1.0f-fminf((1.2f-mgrip)*(float)dt, 0.5f);        // slick ground -> they slide
    g_enemy_vel[q][0]*=drag; g_enemy_vel[q][2]*=drag;
    en->position[0]=g_enemy_prev[q][0]+g_enemy_vel[q][0]*(float)dt;   // physical move (with momentum + knockback)
    en->position[2]=g_enemy_prev[q][2]+g_enemy_vel[q][2]*(float)dt;
    // ---- VISION + AWARENESS: face the heading, see within a cone + line of sight ----
    float dpx=e->position[0]-en->position[0], dpz=e->position[2]-en->position[2];
    float pdist=sqrtf(dpx*dpx+dpz*dpz);
    float sp=sqrtf(g_enemy_vel[q][0]*g_enemy_vel[q][0]+g_enemy_vel[q][2]*g_enemy_vel[q][2]);
    if (sp>1.5f) g_enemy_head[q]=atan2f(g_enemy_vel[q][2], g_enemy_vel[q][0]);
    float hx=cosf(g_enemy_head[q]), hz=sinf(g_enemy_head[q]);
    float facing = (pdist>0.5f)? (dpx/pdist*hx + dpz/pdist*hz) : 1.0f;
    int can_see = (pdist<95.0f) && (facing>0.25f) &&
                  !los_blocked(en->position[0],en->position[2], e->position[0],e->position[2]);
    if (g_human_scale) can_see = can_see && pdist<45.0f;   // a human is small + hard to spot
    // SPARK RESONANCE: the Corrupted hunt by a spark's activity, not just sight.
    // Firing, sprinting and abilities make you "loud" -- sensed through walls at
    // range; going still + quiet lets you slip past even a nearby hunter.
    int resonant_hunt = (en->kind==EX_ENEMY_CORRUPTED || en->kind==EX_ENEMY_APEX);
    if (resonant_hunt && g_spark_loud > 0.15f){
      float sense = 40.0f + g_spark_loud*90.0f;       // loud spark -> felt far off
      if (pdist < sense) { g_enemy_aware[q]=1; g_enemy_losetimer[q]=0.0f; can_see=1; }
    }
    if (can_see){
      if (!g_enemy_aware[q]){ g_enemy_aware[q]=1; g_enemy_startle[q]=0.5f; g_enemy_xform_pop[q]=0.3f;
        g_enemy_hurt[q]=0.12f; }   // SURPRISE: it just caught sight of you (flash + flinch)
      g_enemy_losetimer[q]=0.0f;
    } else if (g_enemy_aware[q]){ g_enemy_losetimer[q]+=(float)dt;
      if (g_enemy_losetimer[q]>3.5f) g_enemy_aware[q]=0; }   // lost track -> searching again
    // RESONANCE SYNC: a coordinator's field pulls this unit into the collective.
    // The moment it's first caught, it hesitates (recalibrates) -- then shares the
    // coordinator's fix on the player and stops losing track.
    { float prev = g_enemy_synced[q]; g_enemy_synced[q] = 0.0f;
      if (en->kind != EX_ENEMY_APEX && g_res_ncoord>0){
        float s = ex_resonance_strength(g_res_coords, g_res_ncoord, en->position, EX_RESONANCE_RADIUS);
        if (s > 0.0f){
          g_enemy_synced[q] = s;
          if (prev <= 0.0f && g_enemy_startle[q] <= 0.0f) g_enemy_startle[q] = 0.35f; // recalibrate beat
          g_enemy_aware[q]=1; g_enemy_losetimer[q]=0.0f;   // acts in sync -- never loses you
        }
      }
    }
    // CORRUPTED GENERATIONAL DRIFT (Jhiaxus's experiment exceeding its models):
    // tag each Corrupted with a generation as it appears -- climbing over the
    // fight -- and let later gens be more autonomous: less hesitation now, quicker
    // + bigger + darker below and in the render.
    ex_corrupt_drift_t cdrift = {1.0f,1.0f,0.0f,0.0f};
    if (en->kind==EX_ENEMY_CORRUPTED){
      if (g_corrupt_gen[q] < 0) g_corrupt_gen[q] = g_corrupt_gen_next++;
      cdrift = ex_corrupt_drift(g_corrupt_gen[q]);
      if (g_enemy_startle[q] > 0.0f) g_enemy_startle[q] *= (1.0f - cdrift.aggression*0.6f);
      // ABSORB METAL TO GROW: a corrupted unit consumes loose wreckage nearby --
      // its mass (and reach, and bite) swells the longer the debris piles up.
      if (en->grow < 1.0f) en->grow = 1.0f;
      float reach = en->radius*en->grow + 3.5f, reach2 = reach*reach;
      int eaten = 0;
      for (int d=0; d<NUM_DEBRIS && eaten<2; d++){ debris_t *dbr=&g_debris[d]; if(!dbr->active) continue;
        float ex=dbr->pos[0]-en->position[0], ez=dbr->pos[2]-en->position[2];
        if (ex*ex+ez*ez <= reach2){
          dbr->active=0;                                  // consumed
          en->grow = ex_absorb_accumulate(en->grow, 0.10f);
          en->hp = fminf(en->max_hp, en->hp + 6.0f);      // metal repairs it a little
          eaten++;
        }
      }
      if (eaten){ uint8_t ac[3]={150,90,180}; ex_vfx_spawn_impact(&g_vfx,(vec3){en->position[0],en->position[1]+1.0f,en->position[2]},5,ac); }
    }
    // SPEARCON LIEUTENANTS (Shardclaw & Icerender's kind): ~1 in 3 grunts is an
    // elite lieutenant -- tougher, wears the harpoon rig (render), and is the one
    // that plants spearcons. Tag once per slot, stable for the match.
    // NAMED MINI-BOSS: on Cryotek's ice line (frostline) or Blackstar's raider
    // turf (outer_rim), the first grunt rises as that warlord -- not a rank grunt.
    if (g_champion_slot < 0 && !g_champion_spawned && en->kind==EX_ENEMY_GRUNT){
      int knd = strstr(g_cur_mapid,"frost") ? 1 : (strstr(g_cur_mapid,"outer_rim") ? 2 : 0);
      if (knd){
        g_champion_slot = q; g_champion_kind = knd; g_champion_spawned = 1;
        en->max_hp *= 8.0f; en->hp = en->max_hp; g_champion_max = en->max_hp;
        g_lieut[q]=0; g_raider[q]=0;   // the champion, not a rank elite
        snprintf(g_shot_msg,sizeof(g_shot_msg), knd==1 ? "CRYOTEK holds the ice line"
                                                        : "BLACKSTAR -- the scavenger king");
        g_shot_timer=2.6f;
      }
    }
    if (en->kind==EX_ENEMY_GRUNT && g_lieut[q] < 0){
      unsigned h = (unsigned)(q*2654435761u) ^ 0x9E3779B9u;   // stable per-slot hash
      int is_elite = ((h >> 13) % 3u == 0u);
      // Blackstar's frontier (outer rim / frostline) fields RAIDERS; elsewhere the
      // ice faction fields Spearcon-Lieutenants. A grunt is at most one.
      int blackstar_turf = (strstr(g_cur_mapid,"outer_rim")!=NULL);   // frostline is Cryotek's ice line
      if (is_elite && blackstar_turf){
        g_raider[q]=1; g_lieut[q]=0; en->max_hp *= 1.6f; en->hp *= 1.6f;   // tough boarder
      } else {
        g_raider[q]=0; g_lieut[q] = is_elite ? 1 : 0;
        if (g_lieut[q]==1){ en->max_hp *= 1.7f; en->hp *= 1.7f; }          // elites take more killing
      }
    }
    // CORRUPTED RAVAGER: ~1-in-3 Corrupted is a pre-grown elite (Jhiaxus's worst).
    if (en->kind==EX_ENEMY_CORRUPTED && g_ravager[q] < 0){
      unsigned h=(unsigned)(q*40503u) ^ 0x5bd1e995u;
      g_ravager[q] = ((h>>11)%3u==0u) ? 1 : 0;
      if (g_ravager[q]==1){ en->max_hp*=1.5f; en->hp*=1.5f;
        if (en->grow < 1.6f) en->grow = 1.6f; }        // starts already grown (bigger + harder-hitting)
    }
    // SLIPSTREAM: ~1-in-3 Hijack is an assassin elite -- fast, flanking.
    if (en->kind==EX_ENEMY_HIJACK && g_slip[q] < 0){
      unsigned h=(unsigned)(q*2246822519u) ^ 0xA5A5A5A5u;
      g_slip[q] = ((h>>12)%3u==0u) ? 1 : 0;
      if (g_slip[q]==1){ en->max_hp*=1.4f; en->hp*=1.4f; }
    }
    if (g_enemy_startle[q]>0.0f){ g_enemy_startle[q]-=(float)dt;   // flinch: hesitate a beat
      g_enemy_vel[q][0]*=0.25f; g_enemy_vel[q][2]*=0.25f;
      en->position[0]=g_enemy_prev[q][0]; en->position[2]=g_enemy_prev[q][2]; }
    // ---- FORM-TIED MODE: score walk/run/fly for the situation; transform on change ----
    int can_fly = (en->kind==EX_ENEMY_HIJACK);
    int newmode=0;
    if (g_enemy_aware[q] && g_enemy_startle[q]<=0.0f){
      if (can_fly && pdist>40.0f) newmode=2;        // fly to close a big gap
      else if (pdist>25.0f)       newmode=1;        // run/drive to approach fast
      else                        newmode=0;        // walk/brawl up close
    }
    if (newmode!=g_enemy_mode[q]){ g_enemy_mode[q]=newmode; g_enemy_xform_pop[q]=0.3f; }   // transform trigger
    if (g_enemy_mode[q]==2){ float th=e->position[1]+3.0f;   // fly: gain altitude + speed
      en->position[1]+=(th-en->position[1])*fminf((float)dt*2.0f,1.0f);
      g_enemy_vel[q][0]*=1.15f; g_enemy_vel[q][2]*=1.15f; }
    else if (g_enemy_mode[q]==1){ g_enemy_vel[q][0]*=1.08f; g_enemy_vel[q][2]*=1.08f;   // run faster
      en->position[1]+=(0.0f-en->position[1])*fminf((float)dt*3.0f,1.0f); }
    else en->position[1]+=(0.0f-en->position[1])*fminf((float)dt*3.0f,1.0f);
    if (g_enemy_synced[q] > 0.0f){ float b = 1.0f + g_enemy_synced[q]*0.30f;  // move as one, a step quicker
      g_enemy_vel[q][0]*=b; g_enemy_vel[q][2]*=b; }
    if (cdrift.speed_mul > 1.0f){ g_enemy_vel[q][0]*=cdrift.speed_mul; g_enemy_vel[q][2]*=cdrift.speed_mul; }  // evolved corrupted move faster
    // MEGATRON'S TRUCE: defected grunts ignore you and charge the boss instead.
    if (g_truce_on && en->kind==EX_ENEMY_GRUNT) g_defected[q] = 1;
    if (g_defected[q]) {
      ex_enemy_t *tb = ex_encounter_boss_ptr(&g_enc);
      if (tb && tb->state != EX_ENEMY_DEAD) {
        float tx=tb->position[0]-en->position[0], tz=tb->position[2]-en->position[2];
        float tl=sqrtf(tx*tx+tz*tz);
        if (tl > 9.0f){ float spd=15.0f; g_enemy_vel[q][0]=tx/tl*spd; g_enemy_vel[q][2]=tz/tl*spd; } // charge in
        else { g_enemy_vel[q][0]*=0.3f; g_enemy_vel[q][2]*=0.3f; }                                  // brawl at its feet
      }
    }
    // BLACKSTAR RAIDER: an aggressive boarder -- periodically LUNGES at you to
    // close the gap (distinct from the lieutenant's area-denial spearcons).
    if (g_raider[q]==1 && !g_defected[q]){
      if (g_raider_lunge[q] > 0.0f) g_raider_lunge[q] -= (float)dt;
      float rdx=e->position[0]-en->position[0], rdz=e->position[2]-en->position[2];
      float rd=sqrtf(rdx*rdx+rdz*rdz);
      if (g_raider_lunge[q] <= 0.0f && g_enemy_aware[q] && rd>10.0f && rd<48.0f && !g_human_scale){
        float inv = 1.0f/(rd>1e-3f?rd:1.0f);
        g_enemy_vel[q][0] += rdx*inv*60.0f; g_enemy_vel[q][2] += rdz*inv*60.0f;   // boarding charge
        g_raider_lunge[q] = 3.5f;
        uint8_t ac[3]={255,150,60}; ex_vfx_spawn_impact(&g_vfx, en->position, 10, ac);
      }
    }
    // SLIPSTREAM: a fast flanking assassin -- circles you instead of charging head-on.
    if (g_slip[q]==1 && !g_defected[q]){
      float dx=e->position[0]-en->position[0], dz=e->position[2]-en->position[2];
      float d=sqrtf(dx*dx+dz*dz);
      if (g_enemy_aware[q] && d>14.0f && d<62.0f && !g_human_scale){
        float px=-dz, pz=dx; float pl=sqrtf(px*px+pz*pz); if(pl>1e-3f){px/=pl;pz/=pl;}
        float dir=(q&1)?1.0f:-1.0f;                                  // stable circling side
        g_enemy_vel[q][0]+=px*dir*28.0f*(float)dt; g_enemy_vel[q][2]+=pz*dir*28.0f*(float)dt;
        g_enemy_vel[q][0]*=1.15f; g_enemy_vel[q][2]*=1.15f;          // and quick on their feet
      }
    }
    // AIR WHIRLWIND: enemies inside the vortex are swept into its orbit (velocity
    // overridden by the wind field) and battered off their attack while it lasts.
    if (g_cyclone_on && !g_defected[q]){
      float cvx[3];
      if (ex_cyclone_vel(en->position[0]-g_cyclone_c[0], en->position[2]-g_cyclone_c[2],
                         EX_CYCLONE_RADIUS, 34.0f, 10.0f, cvx)){
        g_enemy_vel[q][0]=cvx[0]; g_enemy_vel[q][2]=cvx[2];   // caught in the whirl
        ex_enemy_damage(en, 8.0f*(float)dt); g_enemy_hurt[q]=0.10f;
      }
    }
    // NAMED MINI-BOSS special: Cryotek's radial ICE NOVA (shoves you back) or
    // Blackstar's HARPOON PULL (a barbed spear that reels you in to be boarded).
    if (q==g_champion_slot && !g_defected[q]){
      if (g_champion_cd > 0.0f) g_champion_cd -= (float)dt;
      float dx=e->position[0]-en->position[0], dz=e->position[2]-en->position[2];
      float d=sqrtf(dx*dx+dz*dz);
      if (g_champion_cd<=0.0f && g_enemy_aware[q] && d<62.0f && !g_human_scale){
        if (g_champion_kind==1){
          spawn_shockring(en->position[0], 0.5f, en->position[2], 34.0f);
          spawn_shockring(en->position[0], 0.5f, en->position[2], 20.0f);
          uint8_t ic[3]={150,220,255}; ex_vfx_spawn_impact(&g_vfx,(vec3){en->position[0],en->position[1]+2.0f,en->position[2]},30,ic);
          if (d<34.0f && d>0.1f){ e->velocity[0]+=dx/d*30.0f; e->velocity[2]+=dz/d*30.0f;   // shoved back
            player_hit(16.0f, en->position[0], en->position[2], "CRYOTEK -- ICE NOVA"); }
          g_champion_cd = 5.0f;
        } else {
          vec3 gm={en->position[0],en->position[1]+1.5f,en->position[2]};
          vec3 gd={dx,0.05f,dz}; float gl=sqrtf(gd[0]*gd[0]+gd[2]*gd[2]); if(gl>1e-3f){gd[0]/=gl;gd[2]/=gl;}
          uint8_t hc[3]={230,80,90}; ex_vfx_spawn_shot(&g_vfx, gm, gd, d, hc);
          if (d>6.0f && d<55.0f){ e->velocity[0]-=dx/d*46.0f; e->velocity[2]-=dz/d*46.0f;   // reeled in
            player_hit(12.0f, en->position[0], en->position[2], "BLACKSTAR -- HARPOON"); }
          g_champion_cd = 4.2f;
        }
        g_cam_trauma += 0.4f;
      }
    }
    if (g_enemy_xform_pop[q]>0.0f) g_enemy_xform_pop[q]-=(float)dt;
    if (g_enemy_web[q]>0.0f){ g_enemy_web[q]-=(float)dt;   // webbed: crawl, and shed white strands
      g_enemy_vel[q][0]*=0.25f; g_enemy_vel[q][2]*=0.25f;
      if(((int)(g_anim_t*12))%4==0){ uint8_t wc[3]={220,228,240}; ex_vfx_spawn_impact(&g_vfx,(vec3){en->position[0],en->position[1]+1.0f,en->position[2]},1,wc);} }
    if (sp>3.0f) kick_debris(en->position[0], en->position[2], 4.5f, sp*0.12f);   // bots scatter debris
  }
  if (g_horror_mode) {
    float pfx=camera->front[0], pfz=camera->front[2];
    { float l=sqrtf(pfx*pfx+pfz*pfz); if(l>1e-4f){pfx/=l;pfz/=l;} }
    float nearest=1e9f;
    for (int q=0;q<g_enc.count;q++){ ex_enemy_t *en=&g_enc.enemies[q]; if(en->state==EX_ENEMY_DEAD) continue;
      float dx=en->position[0]-e->position[0], dz=en->position[2]-e->position[2]; float d=sqrtf(dx*dx+dz*dz);
      if (d<nearest) nearest=d;
      if (d>0.5f){ float fx=dx/d, fz=dz/d; float facing=fx*pfx+fz*pfz;
        // observed = roughly in front of you and close enough to make out in the fog
        if (facing > 0.45f && d < 75.0f){ en->position[0]=stalk_last[q][0]; en->position[2]=stalk_last[q][2];
          g_enemy_vel[q][0]=g_enemy_vel[q][2]=0.0f; }   // frozen: kill momentum too
        else if (facing <= 0.45f && d < 16.0f) {         // UNOBSERVED + close -> it lunges
          g_enemy_vel[q][0] += -fx*45.0f*(float)dt*60.0f*(float)dt; g_enemy_vel[q][2] += -fz*45.0f*(float)dt*60.0f*(float)dt;
          en->position[0] -= fx*22.0f*(float)dt; en->position[2] -= fz*22.0f*(float)dt;   // fast close
          if (d < 5.0f){ player_hit(16.0f, en->position[0], en->position[2], "it struck from the dark"); g_dread=1.0f; } }
      }
    }
    // dread rises as the nearest stalker closes; feeds a heartbeat camera pulse
    float target = (nearest<90.0f)? (1.0f - nearest/90.0f) : 0.0f;
    g_dread += (target - g_dread) * fminf((float)dt*2.0f, 1.0f);
    g_cam_trauma += g_dread*g_dread * 0.02f * (0.5f+0.5f*sinf(g_anim_t*6.0f));   // heartbeat
    // FLICKER: the failing lights stutter -- worse as the dread rises
    if (g_key_light){ float fl = 1.0f;
      float n = sinf(g_anim_t*47.0f)*sinf(g_anim_t*13.3f);
      if (n > 0.6f - g_dread*0.4f) fl = 0.35f + 0.3f*sinf(g_anim_t*90.0f);   // stutter dark
      g_key_light->color[0]=g_horror_key[0]*fl; g_key_light->color[1]=g_horror_key[1]*fl; g_key_light->color[2]=g_horror_key[2]*fl; }
  }
  humans_update((float)dt);   // co-play human NPCs
  for (int i = 0; i < g_enc.count; i++) {
    ex_enemy_t *en = &g_enc.enemies[i];
    if (en->state == EX_ENEMY_DEAD) { hide_body(g_enemy_bodies[i]); if(g_enemy_weapons[i]) g_enemy_weapons[i]->position[1]=-1000.0f; continue; }
    float sp = (en->state==EX_ENEMY_CHASE) ? en->move_speed : 0.0f;
    g_enemy_phase[i] = ex_gait_advance(g_enemy_phase[i], sp, dt);
    float yaw = atan2f(e->position[2]-en->position[2], e->position[0]-en->position[0]);
    ex_body_kind bk = (en->kind==EX_ENEMY_APEX)? EX_BODY_APEX :
                      (en->kind==EX_ENEMY_CORRUPTED)? EX_BODY_CORRUPTED :
                      (en->kind==EX_ENEMY_HIJACK)? EX_BODY_HIJACK : EX_BODY_GRUNT;
    float ss = (en->kind==EX_ENEMY_APEX)? ex_enemy_visual_radius(en)*1.1f :
               (en->kind==EX_ENEMY_CORRUPTED)? (ex_enemy_visual_radius(en)/1.1f)*1.1f*ex_absorb_stats(en->grow).scale :
               (en->kind==EX_ENEMY_HIJACK)? 0.8f : 1.0f;
    if (i==g_champion_slot) ss *= 2.3f;   // CRYOTEK towers over his grunts
    // evolved Corrupted are visibly larger/more mutated
    ex_corrupt_drift_t rdrift = (en->kind==EX_ENEMY_CORRUPTED && g_corrupt_gen[i]>=0)
                                ? ex_corrupt_drift(g_corrupt_gen[i]) : (ex_corrupt_drift_t){1.0f,1.0f,0.0f,0.0f};
    ss *= rdrift.scale;
    if (en->kind==EX_ENEMY_APEX && g_apex_telegraph>0.0f) {
      float charge=(1.2f-g_apex_telegraph)/1.2f; ss *= 1.0f + charge*0.18f;   // swell as it charges a pulse
    }
    // TRANSFORM POP: a quick squash-and-flare when the enemy switches gait/mode
    // (walk<->run<->fly) so the form change reads at a glance.
    if (g_enemy_xform_pop[i] > 0.0f) {
      float p = g_enemy_xform_pop[i]/0.3f;                 // 1 -> 0
      ss *= 1.0f + sinf(p*3.14159f)*0.22f;                 // flare out then settle
    }
    if (g_enemy_mode[i]==2) ss *= 1.0f;                    // (flyers already lifted in physics)
    // GENOME: each enemy gets a stable, seeded proportion genome so a crowd of
    // the same kind no longer shares one silhouette. Mild spread keeps the rig
    // intact; the seed is fixed per slot+kind so an individual looks consistent.
    ex_genome_t egm = ex_genome_from_seed((uint64_t)(i*2654435761u) ^ ((uint64_t)en->kind*40503u), 0.5f);
    ex_build_params_t ebp = ex_build_default();
    ebp.bulk      = ex_gene(&egm, EX_GENE_BULK);
    ebp.height    = ex_gene(&egm, EX_GENE_HEIGHT);
    ebp.head      = ex_gene(&egm, EX_GENE_HEAD);
    ebp.shoulders = ex_gene(&egm, EX_GENE_SHOULDER);
    ebp.arm       = ex_gene(&egm, EX_GENE_ARM);
    ebp.leg       = ex_gene(&egm, EX_GENE_LEG);
    ebp.alt_mode  = (int)((egm.g[EX_GENE_TAIL]*97.0f)) % 21;   // stable pseudo alt-mode -> varied remnants
    // per-body SIGNATURE: enemies read distinctly (corrupted crest, hijack cable,
    // apex heavy cannon, grunt wing-cannon). Swap the BACK mesh when the kind's
    // signature changes; pose_body then hides it for bodies with none (sig<0).
    int esig = ex_signature_for_body(bk);
    if (en->kind==EX_ENEMY_GRUNT && g_lieut[i]==1) esig = EX_SIG_HARPOON;   // lieutenant rig
    if (en->kind==EX_ENEMY_GRUNT && g_raider[i]==1) esig = EX_SIG_HALO;      // Blackstar raider rig
    if (en->kind==EX_ENEMY_CORRUPTED && g_ravager[i]==1) esig = EX_SIG_SOLAR; // Ravager: an absorbed molten core
    if (en->kind==EX_ENEMY_HIJACK && g_slip[i]==1) esig = EX_SIG_RECON;       // Slipstream: sleek assassin fins
    if (i==g_champion_slot) esig = (g_champion_kind==1) ? EX_SIG_CREST : EX_SIG_HARPOON;  // Cryotek crown / Blackstar boarding spear
    if (esig >= 0 && esig != g_enemy_sig[i]) {
      ex_model_t *st = sig_template((ex_signature_t)esig);
      if (st && st != g_enemy_bodies[i][EX_PART_BACK]) {
        ex_scene_remove_model(scene, g_enemy_bodies[i][EX_PART_BACK]);
        g_enemy_bodies[i][EX_PART_BACK] = ex_model_copy(st);
        g_enemy_bodies[i][EX_PART_BACK]->is_lit = 1;
        g_enemy_bodies[i][EX_PART_BACK]->is_shadow = 0;
        ex_scene_add_model(scene, g_enemy_bodies[i][EX_PART_BACK]);
      }
      g_enemy_sig[i] = esig;
    }
    pose_body(g_enemy_bodies[i], en->position, 0, bk, ss, g_enemy_phase[i], sp, (int)en->element, -yaw, &ebp, esig);
    if (g_enemy_hurt[i] > 0.0f) {   // damage flash: blow the body toward white
      float fl = g_enemy_hurt[i]/0.16f; if(fl>1)fl=1;
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_enemy_bodies[i][b];
        pb->tint[0]+= (1.0f-pb->tint[0])*fl; pb->tint[1]+= (1.0f-pb->tint[1])*fl; pb->tint[2]+= (1.0f-pb->tint[2])*fl; }
    }
    if (en->kind==EX_ENEMY_GRUNT && g_lieut[i]==1 && g_enemy_hurt[i] <= 0.0f) {  // lieutenant: cold steel-blue elite
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_enemy_bodies[i][b];
        pb->tint[0]+= (0.45f-pb->tint[0])*0.4f; pb->tint[1]+= (0.60f-pb->tint[1])*0.4f; pb->tint[2]+= (0.80f-pb->tint[2])*0.4f; }
    }
    if (en->kind==EX_ENEMY_GRUNT && g_raider[i]==1 && !g_defected[i] && g_enemy_hurt[i] <= 0.0f) {  // Blackstar raider: scavenger amber
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_enemy_bodies[i][b];
        pb->tint[0]+= (0.95f-pb->tint[0])*0.45f; pb->tint[1]+= (0.55f-pb->tint[1])*0.45f; pb->tint[2]+= (0.18f-pb->tint[2])*0.45f; }
    }
    if (en->kind==EX_ENEMY_CORRUPTED && g_ravager[i]==1 && !g_defected[i] && g_enemy_hurt[i] <= 0.0f) {  // Ravager: molten magma
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_enemy_bodies[i][b];
        pb->tint[0]+= (1.0f-pb->tint[0])*0.5f; pb->tint[1]+= (0.42f-pb->tint[1])*0.5f; pb->tint[2]+= (0.12f-pb->tint[2])*0.5f; }
    }
    if (en->kind==EX_ENEMY_HIJACK && g_slip[i]==1 && !g_defected[i] && g_enemy_hurt[i] <= 0.0f) {  // Slipstream: pale silver-violet
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_enemy_bodies[i][b];
        pb->tint[0]+= (0.80f-pb->tint[0])*0.45f; pb->tint[1]+= (0.78f-pb->tint[1])*0.45f; pb->tint[2]+= (0.95f-pb->tint[2])*0.45f; }
    }
    if (i==g_champion_slot && g_enemy_hurt[i] <= 0.0f) {   // champion glow
      float tr=(g_champion_kind==1)?0.55f:0.70f, tg=(g_champion_kind==1)?0.80f:0.12f, tb=(g_champion_kind==1)?1.0f:0.22f;
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_enemy_bodies[i][b];
        pb->tint[0]+= (tr-pb->tint[0])*0.6f; pb->tint[1]+= (tg-pb->tint[1])*0.6f; pb->tint[2]+= (tb-pb->tint[2])*0.6f;
        pb->emission_strength += 0.5f; }
    }
    if (g_defected[i] && g_enemy_hurt[i] <= 0.0f) {   // truce: allied cyan glow
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_enemy_bodies[i][b];
        pb->tint[0]+= (0.30f-pb->tint[0])*0.55f; pb->tint[1]+= (0.85f-pb->tint[1])*0.55f; pb->tint[2]+= (1.0f-pb->tint[2])*0.55f; }
    }
    if (g_enemy_synced[i] > 0.0f && g_enemy_hurt[i] <= 0.0f) {   // resonance sync: a cold violet sheen
      float sy = g_enemy_synced[i]*0.55f;
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_enemy_bodies[i][b];
        pb->tint[0]+= (0.55f-pb->tint[0])*sy; pb->tint[1]+= (0.35f-pb->tint[1])*sy; pb->tint[2]+= (0.95f-pb->tint[2])*sy; }
    }
    if (rdrift.darken > 0.0f && g_enemy_hurt[i] <= 0.0f) {   // evolved corrupted drift toward black
      float dk = rdrift.darken;
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_enemy_bodies[i][b];
        pb->tint[0]*=(1.0f-dk); pb->tint[1]*=(1.0f-dk); pb->tint[2]*=(1.0f-dk*0.7f); }  // keep a faint violet
    }
    // ranged kinds (grunt/apex) hold a visible weapon aimed at the player
    if (g_enemy_weapons[i]) {
      if (en->kind==EX_ENEMY_GRUNT || en->kind==EX_ENEMY_APEX) {
        ex_model_t *w = g_enemy_weapons[i];
        float tdx=e->position[0]-en->position[0], tdz=e->position[2]-en->position[2];
        float tl=sqrtf(tdx*tdx+tdz*tdz); if(tl<0.001f)tl=1; tdx/=tl; tdz/=tl;
        // right-hand offset (perpendicular to aim) + forward toward player
        w->position[0]=en->position[0] + (-tdz)*0.55f*ss + tdx*0.5f;
        w->position[1]=en->position[1] + 1.15f*ss;
        w->position[2]=en->position[2] + ( tdx)*0.55f*ss + tdz*0.5f;
        w->rotation[1]=(-atan2f(tdz,tdx))*57.2957795f;
        w->scale=(en->kind==EX_ENEMY_APEX)?0.9f:0.5f;
        if (en->kind==EX_ENEMY_APEX){ w->tint[0]=0.7f; w->tint[1]=0.3f; w->tint[2]=0.8f; }
        else { w->tint[0]=0.7f; w->tint[1]=0.3f; w->tint[2]=0.28f; }
      } else g_enemy_weapons[i]->position[1]=-1000.0f;
    }
    // boss: ground-slam AoE + summon Hijack scouts
    if (en->kind == EX_ENEMY_APEX) {
      if (g_boss_light && en->state != EX_ENEMY_DEAD) {
        g_boss_light->position[0]=en->position[0]; g_boss_light->position[1]=en->position[1]+4.0f; g_boss_light->position[2]=en->position[2];
        float pulse = 0.8f + 0.4f*sinf(g_anim_t*3.0f);
        // during a telegraph the boss light flares white-hot as a warning
        float tele = (g_apex_telegraph>0.0f)? (1.0f + (1.2f-g_apex_telegraph)*2.5f) : 1.0f;
        g_boss_light->color[0]=1.1f*pulse*tele; g_boss_light->color[1]=0.3f*pulse*tele; g_boss_light->color[2]=1.3f*pulse*tele;
        // spiked crown: a ring of blades around the boss's shoulders, flaring
        // outward and glowing hotter as a pulse charges -- a distinct silhouette.
        float crown_r = ex_enemy_visual_radius(en)*2.4f;
        float top = en->position[1] + ex_enemy_visual_radius(en)*3.2f;
        for (int k=0;k<NUM_BOSS_SPIKES;k++){ ex_model_t *sp2=g_boss_spikes[k];
          float a = (float)k/NUM_BOSS_SPIKES*6.2831853f + g_anim_t*0.4f;
          sp2->position[0]=en->position[0]+cosf(a)*crown_r;
          sp2->position[1]=top;
          sp2->position[2]=en->position[2]+sinf(a)*crown_r;
          sp2->scale3[0]=0.5f; sp2->scale3[1]=1.6f+tele*0.6f; sp2->scale3[2]=0.5f;
          sp2->rotation[0]=35.0f; sp2->rotation[1]=a*57.3f;            // fan outward
          sp2->tint[0]=0.5f+0.5f*(tele-1.0f); sp2->tint[1]=0.15f; sp2->tint[2]=0.6f;
        }
      } else if (en->state==EX_ENEMY_DEAD) {
        for (int k=0;k<NUM_BOSS_SPIKES;k++) if(g_boss_spikes[k]) g_boss_spikes[k]->position[1]=-1000.0f;
      }
      // ---- resonance-pulse choreography: cadence + telegraph + cycling type ----
      if (en->state != EX_ENEMY_DEAD) {
        const float PULSE_PERIOD = 9.0f;
        g_apex_pulse_t += (float)dt;
        g_apex_telegraph = (g_apex_pulse_t > PULSE_PERIOD-1.2f) ? (PULSE_PERIOD - g_apex_pulse_t) : 0.0f;
        if (g_apex_telegraph>0.0f && g_shot_timer<=0.0f){
          const char *warn[4]={"resonance field charging","kinetic hurl charging","plasma burst charging","graviton well charging"};
          snprintf(g_shot_msg,sizeof(g_shot_msg),"! %s !", warn[g_apex_pulse_type]); g_shot_timer=0.3f;
        }
        if (g_apex_pulse_t >= PULSE_PERIOD) {
          g_apex_pulse_t = 0.0f;
          float bx=en->position[0], bz=en->position[2], by=en->position[1];
          float dx=e->position[0]-bx, dz=e->position[2]-bz; float dist=sqrtf(dx*dx+dz*dz); if(dist<0.1f)dist=0.1f;
          uint8_t pc[3]={200,120,255};
          switch (g_apex_pulse_type) {
            case 0: // RESONANCE FIELD -- paralysis: slows the player if in range
              ex_vfx_spawn_impact(&g_vfx, en->position, 44, pc);
              if (dist<55.0f){ g_player_slow=2.5f; snprintf(g_shot_msg,sizeof(g_shot_msg),"RESONANCE FIELD -- systems slowed"); g_shot_timer=2.0f; }
              break;
            case 1: // KINETIC HURL -- a heavy debris projectile at the player
              { vec3 from={bx,by+3.0f,bz}, dir={dx/dist,0.05f,dz/dist};
                ex_vfx_spawn_shot(&g_vfx, from, dir, dist, pc);
                if (dist<95.0f) player_hit(30.0f, bx, bz, "KINETIC HURL"); }
              break;
            case 2: // PLASMA BURST -- a fan of bolts sweeping the player's lane
              { float base=atan2f(dz,dx);
                for(int k=-2;k<=2;k++){ float ang=base+k*0.18f; vec3 from={bx,by+2.0f,bz}, dir={cosf(ang),0.04f,sinf(ang)};
                  ex_vfx_spawn_shot(&g_vfx, from, dir, 65.0f, pc); }
                if (dist<60.0f) player_hit(22.0f, bx, bz, "PLASMA BURST"); }
              break;
            case 3: // GRAVITON WELL -- yanks the player toward the boss
              ex_vfx_spawn_impact(&g_vfx, en->position, 32, pc);
              if (dist<85.0f){ e->velocity[0]+=(-dx/dist)*38.0f; e->velocity[2]+=(-dz/dist)*38.0f;
                snprintf(g_shot_msg,sizeof(g_shot_msg),"GRAVITON WELL -- pulled in!"); g_shot_timer=1.5f; }
              break;
          }
          g_apex_pulse_type = (g_apex_pulse_type+1)%4;
          g_cam_trauma += 0.45f;
        }
      }
      if (en->wants_slam) {
        uint8_t sc[3]={170,90,200}; ex_vfx_spawn_impact(&g_vfx, en->position, 28, sc);
        vec3 dd={e->position[0]-en->position[0],0,e->position[2]-en->position[2]};
        if (sqrtf(dd[0]*dd[0]+dd[2]*dd[2]) < 16.0f) {
          player_hit(40.0f, en->position[0], en->position[2], "APEX SLAM"); }
        snprintf(g_shot_msg,sizeof(g_shot_msg),"APEX SLAM!"); g_shot_timer=1.5f;
      }
      if (en->wants_summon) {
        int added=0;
        for (int q=0; q<3 && g_enc.count<EX_MAX_ENEMIES; q++) {
          float a=(float)q/3*6.283f; vec3 sp={en->position[0]+cosf(a)*6, 0, en->position[2]+sinf(a)*6};
          int idx=ex_encounter_spawn(&g_enc, EX_ENEMY_HIJACK, EX_ELEM_VINE, sp, 45.0f);
          if(idx>=0){ spawn_body(g_enemy_bodies[idx]); g_enemy_phase[idx]=0; added++; }
        }
        if(added){ snprintf(g_shot_msg,sizeof(g_shot_msg),"apex summons scouts"); g_shot_timer=1.5f; }
      }
    }
    // damage to player
    if (en->wants_to_fire) {
      if (en->kind==EX_ENEMY_CORRUPTED) {
        { player_hit(ex_enemy_melee_damage(en) * ex_absorb_stats(en->grow).dmg_mul, en->position[0], en->position[2], en->kind==EX_ENEMY_CORRUPTED?"CORRUPTED":"MELEE"); }
        uint8_t vc[3]={150,90,180}; ex_vfx_spawn_impact(&g_vfx, e->position, 8, vc);
      } else if (ex_enemy_has_los(en, g_chunks, e->position)) {
        if (g_defected[i] && en->kind==EX_ENEMY_GRUNT) {
          // TRUCE: this grunt has turned -- it strikes the boss, not you.
          ex_enemy_t *tb = ex_encounter_boss_ptr(&g_enc);
          if (tb && tb->state != EX_ENEMY_DEAD) {
            float bx=tb->position[0]-en->position[0], bz=tb->position[2]-en->position[2];
            float bl=sqrtf(bx*bx+bz*bz);
            if (bl < 15.0f) {
              ex_enemy_damage(tb, 8.0f);
              int ti=(int)(tb-g_enc.enemies); if(ti>=0&&ti<EX_MAX_ENEMIES) g_enemy_hurt[ti]=0.12f;
              vec3 gm={en->position[0],en->position[1]+1.2f,en->position[2]};
              vec3 gd={bx,0.1f,bz}; float gl=sqrtf(gd[0]*gd[0]+gd[2]*gd[2]); if(gl>1e-3f){gd[0]/=gl;gd[2]/=gl;}
              uint8_t bc[3]={120,200,255}; ex_vfx_spawn_shot(&g_vfx, gm, gd, bl, bc);
            }
          }
        } else {
        { player_hit((en->kind==EX_ENEMY_HIJACK)?4.0f:8.0f, en->position[0], en->position[2], en->kind==EX_ENEMY_HIJACK?"HIJACK":"GRUNT"); }
        vec3 gm={en->position[0],en->position[1]+1.2f,en->position[2]};
        vec3 gd; vec3_sub(gd, e->position, gm); float gl=vec3_len(gd);
        if (gl>0.001f){ vec3_scale(gd,gd,1.0f/gl); uint8_t rc[3]={255,90,70}; ex_vfx_spawn_shot(&g_vfx, gm, gd, gl, rc); }
        // SPEARCON: the ice faction lobs a sticky charge onto the player's spot.
        // It clings and counts down -- you have to vacate before it detonates.
        if (en->kind==EX_ENEMY_GRUNT && g_lieut[i]==1 && g_spearcon_cd<=0.0f && !g_human_scale && gl<70.0f){
          vec3 sp={e->position[0], 0.3f, e->position[2]};
          if (ex_spearcon_plant(&g_spearcons, sp, 2.2f) >= 0){
            g_spearcon_cd = 4.5f;
            uint8_t ic[3]={150,220,255}; ex_vfx_spawn_shot(&g_vfx, gm, gd, gl, ic);
            ex_vfx_spawn_impact(&g_vfx,(vec3){sp[0],0.6f,sp[2]},8,ic);
            snprintf(g_shot_msg,sizeof(g_shot_msg),"SPEARCON stuck -- move!"); g_shot_timer=1.4f;
          }
        }
        }
      }
    }
  }

  // SPEARCONS: tick the sticky charges. Each is telegraphed by a shrinking,
  // quickening disc (ice -> white-hot); when the fuse runs out it detonates in a
  // small radius, hurting the player if they didn't clear out in time.
  {
    g_spearcon_cd -= (float)dt;
    float det[EX_SPEARCON_MAX][3];
    int nd = ex_spearcon_update(&g_spearcons, (float)dt, det, EX_SPEARCON_MAX);
    for (int d=0; d<nd; d++){
      apply_shockwave(det[d][0], det[d][1], det[d][2], 9.0f, 20.0f, 0.0f);   // knock only; player dmg below
      spawn_shockring(det[d][0], 0.4f, det[d][2], 9.0f);
      uint8_t ic[3]={205,235,255}; ex_vfx_spawn_impact(&g_vfx,(vec3){det[d][0],0.8f,det[d][2]},16,ic);
      float dx=e->position[0]-det[d][0], dz=e->position[2]-det[d][2];
      if (dx*dx+dz*dz < 9.0f*9.0f) player_hit(20.0f, det[d][0], det[d][2], "SPEARCON");
      g_cam_trauma += 0.3f; sfx(g_sfx_expl);
    }
    for (int i=0;i<EX_SPEARCON_MAX;i++){
      ex_model_t *m=g_spearcon_models[i]; if(!m) continue;
      const ex_spearcon_t *s=&g_spearcons.s[i];
      if (!s->active){ m->position[1]=-100000.0f; continue; }
      float pr=ex_spearcon_progress(s);
      m->position[0]=s->pos[0]; m->position[1]=0.32f; m->position[2]=s->pos[2];
      float rad = 9.0f*(1.0f-pr)+2.0f;                 // ring tightens onto the charge
      m->scale3[0]=m->scale3[2]=rad; m->scale3[1]=0.4f;
      float beat=0.5f+0.5f*sinf(g_anim_t*(6.0f+pr*30.0f));  // pulse quickens toward the blast
      m->tint[0]=0.55f+0.45f*(pr*0.5f+beat*0.5f); m->tint[1]=0.85f-0.25f*pr; m->tint[2]=1.0f-0.35f*pr;
    }
  }

  // FORGEHEART, the Outer-Rim companion: follows when it's quiet, wades into the
  // nearest enemy when it isn't, and flips wreckage to crush it. Goes down if
  // swarmed, then pulls himself back together.
  if (g_ally_present) {
    static int prev_ally_mode = EX_ALLY_FOLLOW;
    float epos[EX_MAX_ENEMIES][3]; int alive[EX_MAX_ENEMIES];
    int nn = (g_enc.count < EX_MAX_ENEMIES) ? g_enc.count : EX_MAX_ENEMIES;
    for (int q=0;q<nn;q++){ epos[q][0]=g_enc.enemies[q].position[0]; epos[q][1]=g_enc.enemies[q].position[1];
      epos[q][2]=g_enc.enemies[q].position[2]; alive[q]=(g_enc.enemies[q].state!=EX_ENEMY_DEAD); }
    float goal[3]; int want_atk=0;
    int tgt = ex_ally_think(&g_ally, e->position, epos, alive, nn, (float)dt, goal, &want_atk);
    if (g_ally.mode==EX_ALLY_DOWN && prev_ally_mode!=EX_ALLY_DOWN){ snprintf(g_shot_msg,sizeof(g_shot_msg),"FORGEHEART DOWN"); g_shot_timer=2.0f; }
    if (g_ally.mode!=EX_ALLY_DOWN && prev_ally_mode==EX_ALLY_DOWN){ snprintf(g_shot_msg,sizeof(g_shot_msg),"FORGEHEART BACK UP"); g_shot_timer=2.0f; }
    prev_ally_mode = g_ally.mode;

    if (g_ally.mode != EX_ALLY_DOWN) {
      float gx=goal[0]-g_ally.pos[0], gz=goal[2]-g_ally.pos[2]; float gd=sqrtf(gx*gx+gz*gz);
      if (gd>0.4f){ float step=fminf(11.0f*(float)dt, gd); g_ally.pos[0]+=gx/gd*step; g_ally.pos[2]+=gz/gd*step;
        g_ally_yaw=atan2f(gx,gz); g_ally_phase += step*0.20f; }
      g_ally.pos[1]=0.0f;
      if (want_atk && tgt>=0 && tgt<g_enc.count){ ex_enemy_t *t=&g_enc.enemies[tgt];
        if (t->state!=EX_ENEMY_DEAD){
          ex_enemy_damage(t, 34.0f);
          if (tgt<EX_MAX_ENEMIES){ g_enemy_hurt[tgt]=0.16f;
            float kx=t->position[0]-g_ally.pos[0], kz=t->position[2]-g_ally.pos[2]; float kd=sqrtf(kx*kx+kz*kz); if(kd>1e-3f){kx/=kd;kz/=kd;}
            g_enemy_vel[tgt][0]+=kx*14.0f; g_enemy_vel[tgt][2]+=kz*14.0f; }
          for(int c=0;c<4;c++) spawn_debris(t->position[0],1.0f,t->position[2],0.45f,0.5f,0.55f,0.6f,1.0f);
          uint8_t fc[3]={255,200,120}; ex_vfx_spawn_impact(&g_vfx,(vec3){t->position[0],1.2f,t->position[2]},8,fc);
          sfx(g_sfx_expl);
        }
      }
      int near=0; for(int q=0;q<nn;q++){ if(!alive[q])continue; float dx=epos[q][0]-g_ally.pos[0],dz=epos[q][2]-g_ally.pos[2]; if(dx*dx+dz*dz<25.0f) near++; }
      if (near>0) g_ally.hp -= near*6.0f*(float)dt; else if (g_ally.hp<g_ally.max_hp) g_ally.hp += 10.0f*(float)dt;

      ex_build_params_t abp = ex_build_default(); abp.bulk=1.25f; abp.height=1.1f; abp.accent_element=EX_ELEM_FIRE; abp.paint=2;
      float aspd = (g_ally.mode==EX_ALLY_ENGAGE)?7.0f:3.0f;
      pose_body(g_ally_parts, g_ally.pos, 0, EX_BODY_GRUNT, 1.18f, g_ally_phase, aspd, EX_ELEM_FIRE, g_ally_yaw, &abp, EX_SIG_HEAVY_CANNON);
      for (int b=0;b<EX_ROBOT_PARTS;b++){ ex_model_t *pb=g_ally_parts[b];   // friendly gold sheen
        pb->tint[0]+=(0.95f-pb->tint[0])*0.35f; pb->tint[1]+=(0.75f-pb->tint[1])*0.35f; pb->tint[2]+=(0.35f-pb->tint[2])*0.35f; }
    } else {
      hide_body(g_ally_parts);   // downed: off the field until he recovers
    }
  }

  // FAILING CONTAINMENT: the Pluto boundary draws inward over the match; beyond it
  // is the consuming void. It bites harder the further out you stray, forcing you
  // toward the shrinking core -- survival becomes a fight for space.
  if (g_contain_active && !g_human_scale){
    g_contain_elapsed += (float)dt;
    float crad = ex_containment_radius(g_contain_elapsed, EX_CONTAIN_R0, EX_CONTAIN_R1, g_contain_close);
    float pdist = sqrtf(e->position[0]*e->position[0] + e->position[2]*e->position[2]);
    float breach = ex_containment_breach(pdist, crad);
    g_contain_ring_cd -= (float)dt;
    if (g_contain_ring_cd <= 0.0f){ spawn_shockring(0.0f, 0.6f, 0.0f, crad); g_contain_ring_cd = 1.2f; }
    if (breach > 0.0f){
      g_player_hp -= (2.5f + breach*0.35f) * (float)dt;          // the void bites, worse further out
      g_hit_flash = fmaxf(g_hit_flash, 0.15f);
      uint8_t vc[3]={150,90,200}; ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0],e->position[1]+1.0f,e->position[2]},5,vc);
      if (g_shot_timer <= 0.0f){ snprintf(g_shot_msg,sizeof(g_shot_msg),"CONTAINMENT BREACH -- get back inside!"); g_shot_timer=1.0f; }
    }
  }

  // UNICRON'S GRAVITON WELL: the apex boss bends space around itself -- a steady
  // inward pull on you (and loose debris), strongest near the center. You have to
  // fight it to hold range; it collapses the moment the boss dies.
  {
    ex_enemy_t *boss = ex_encounter_boss_ptr(&g_enc);
    if (boss && boss->state != EX_ENEMY_DEAD && !g_human_scale){
      // ENERGY SIPHON: the devourer feeds. Linger in close and it drains your
      // spark and mends itself with the take -- strongest point-blank, nothing at
      // the rim. Break range to stop it. (The well below drags you into range.)
      g_siphon_fx_cd -= (float)dt;
      { float sdx=e->position[0]-boss->position[0], sdz=e->position[2]-boss->position[2];
        float sd=sqrtf(sdx*sdx+sdz*sdz);
        float rate=ex_siphon_rate(sd, EX_SIPHON_RADIUS, EX_SIPHON_RATE);
        if (rate>0.0f && g_match_live){
          float drained=rate*(float)dt;
          g_player_hp -= drained;                                   // drains you
          boss->hp = fminf(boss->max_hp, boss->hp + drained*0.5f);  // feeds it
          g_hit_flash = fmaxf(g_hit_flash, 0.10f);
          if (g_siphon_fx_cd<=0.0f){
            vec3 from={boss->position[0],boss->position[1]+2.5f,boss->position[2]};
            vec3 bd={ -sdx, 0.05f, -sdz }; float dl=sqrtf(bd[0]*bd[0]+bd[2]*bd[2]); if(dl>1e-3f){bd[0]/=dl;bd[2]/=dl;}
            uint8_t vc[3]={195,90,235}; ex_vfx_spawn_shot(&g_vfx, from, bd, sd, vc);
            ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0],e->position[1]+1.0f,e->position[2]},6,vc);
            g_siphon_fx_cd=0.25f;
            if (g_shot_timer<=0.0f){ snprintf(g_shot_msg,sizeof(g_shot_msg),"SIPHONED -- break range!"); g_shot_timer=1.0f; }
          }
        }
      }
      float dir[3];
      float pull = ex_graviton_pull(boss->position, e->position, EX_GRAVITON_RADIUS, 14.0f, dir);
      if (pull > 0.0f){
        e->velocity[0] += dir[0]*pull*(float)dt;
        e->velocity[2] += dir[2]*pull*(float)dt;
        // drag loose debris inward for a visible swirl toward the boss
        for (int i=0;i<NUM_DEBRIS;i++){ debris_t *dbr=&g_debris[i]; if(!dbr->active) continue;
          float ddx=boss->position[0]-dbr->pos[0], ddz=boss->position[2]-dbr->pos[2];
          float dd=sqrtf(ddx*ddx+ddz*ddz);
          if (dd>1.0f && dd<EX_GRAVITON_RADIUS){ float pf=(1.0f-dd/EX_GRAVITON_RADIUS)*22.0f;
            dbr->vel[0]+=ddx/dd*pf*(float)dt; dbr->vel[2]+=ddz/dd*pf*(float)dt; }
          // THE REASSEMBLER: wreckage dragged to the core is consumed -- reforged
          // into hull. Feeds the boss's existing absorb/grow (size + melee scale
          // via ex_enemy_visual_radius/damage) and mends a little hp. Capped by
          // ex_absorb_accumulate's ceiling, so it swells but never runs away.
          if (dd < 7.0f && boss->grow < EX_ABSORB_GROW_MAX - 0.01f){
            dbr->active=0; if(g_debris_models[i]) g_debris_models[i]->position[1]=-100000.0f;
            boss->grow = ex_absorb_accumulate(boss->grow, 0.06f);
            boss->hp = fminf(boss->max_hp, boss->hp + 8.0f);
            uint8_t rc[3]={200,150,255}; ex_vfx_spawn_impact(&g_vfx,(vec3){boss->position[0],boss->position[1]+2.5f,boss->position[2]},6,rc);
          }
        }
        g_graviton_cd -= (float)dt;
        if (g_graviton_cd <= 0.0f){                                   // field boundary pulse
          spawn_shockring(boss->position[0], 0.5f, boss->position[2], EX_GRAVITON_RADIUS);
          uint8_t gc[3]={150,110,220}; ex_vfx_spawn_impact(&g_vfx,(vec3){boss->position[0],boss->position[1]+2.0f,boss->position[2]},18,gc);
          g_graviton_cd = 1.5f;
        }
      }
    }
  }

  // PHOTON MAN -- the Watch's ceiling. Not a button: when the bearer is about to
  // be overwhelmed, the Watch decides the moment and steps in for one short,
  // overwhelming window, then goes cold for the rest of the life.
  {
    float hpf = (g_player_max_hp > 0.0f) ? (g_player_hp / g_player_max_hp) : 0.0f;
    int aware = 0; for (int i=0;i<EX_MAX_ENEMIES;i++) if (g_enemy_aware[i]) aware++;
    int under_threat = (g_hit_flash > 0.05f) || (aware >= 2);
    int eligible = (!g_human_scale) && (!g_landing_active) && g_match_live;
    if (ex_photon_update(&g_photon, hpf, under_threat, eligible, (float)dt)) {
      // the Watch pulls him back from the edge and floods the field with light
      g_player_hp = fmaxf(g_player_hp, g_player_max_hp * 0.4f);
      apply_shockwave(e->position[0], e->position[1], e->position[2], 40.0f, 30.0f, 24.0f);
      spawn_shockring(e->position[0], e->position[1], e->position[2], 40.0f);
      { uint8_t wc[3]={255,255,245}; ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0],e->position[1]+1.5f,e->position[2]},40,wc); }
      g_cam_trauma += 1.0f; sfx(g_sfx_xform);
      snprintf(g_shot_msg,sizeof(g_shot_msg),"PHOTON MAN -- the watch's ceiling"); g_shot_timer=3.0f;
    }
    if (ex_photon_active(&g_photon)) {
      // solid light: periodic radiant pulses that scour nearby enemies
      g_photon_pulse_cd -= (float)dt;
      if (g_photon_pulse_cd <= 0.0f) {
        apply_shockwave(e->position[0], e->position[1], e->position[2], 26.0f, 20.0f, 30.0f);
        spawn_shockring(e->position[0], e->position[1], e->position[2], 26.0f);
        uint8_t wc[3]={255,255,240}; ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0],e->position[1]+1.2f,e->position[2]},14,wc);
        g_photon_pulse_cd = 0.9f;
      }
    }
    // RODIMUS (the Matrix): a claimed, offensive apotheosis. While it burns, the
    // Matrix light pulses out and scours nearby enemies (offense, vs Photon's
    // defensive light). Boosts are applied at the damage/speed hooks.
    ex_matrix_update(&g_matrix, (float)dt);
    if (ex_matrix_active(&g_matrix)) {
      g_matrix_pulse_cd -= (float)dt;
      if (g_matrix_pulse_cd <= 0.0f) {
        apply_shockwave(e->position[0], e->position[1], e->position[2], 30.0f, 18.0f, 26.0f);
        spawn_shockring(e->position[0], e->position[1], e->position[2], 30.0f);
        uint8_t gc[3]={255,238,160}; ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0],e->position[1]+1.3f,e->position[2]},12,gc);
        g_matrix_pulse_cd = 1.1f;
      }
    }
  }

  // lose / win
  if (g_player_hp <= 0.0f) {
    g_player_hp = 0.0f;
    if (g_wave_mode) { end_wave_run(0); return; }  // defeat -> results screen
    g_match_live = 0;
    // DEATH RAGDOLL: blow every remaining body part off as tumbling debris + a
    // big secondary explosion at the wreck.
    for (int i=0;i<EX_ROBOT_PARTS;i++){ if(g_part_detached[i]) continue;
      spawn_debris(e->position[0], e->position[1]+1.5f, e->position[2], 0.35f, 0.45f,0.5f,0.6f, 1.6f); }
    spawn_explosion(e->position[0], e->position[1]+1.0f, e->position[2], 2.2f, 14);
    hide_body(g_player_parts); for(int i=0;i<EX_MAX_ENEMIES;i++){ hide_body(g_enemy_bodies[i]); if(g_enemy_weapons[i]) g_enemy_weapons[i]->position[1]=-1000.0f; }
    for(int i=0;i<DETAIL_MODELS;i++) g_detail_models[i]->position[1]=-1000.0f;
    g_menu.state = g_menu.in_campaign ? EX_GS_MISSIONS : EX_GS_BUILDER; g_menu.cursor = 0;
    return;
  }
  if (((g_enc.cleared) || (g_control_target>0.0f && g_obj_score>=g_control_target)) && !g_wave_mode) {
    g_form_unlocked_photon = 1;
    g_match_live = 0;
    hide_body(g_player_parts); for(int i=0;i<EX_MAX_ENEMIES;i++){ hide_body(g_enemy_bodies[i]); if(g_enemy_weapons[i]) g_enemy_weapons[i]->position[1]=-1000.0f; }
    if (g_menu.in_campaign) {
      ex_mission_t *ms = &g_campaign.missions[g_menu.mission_index];
      ex_campaign_complete(&g_campaign, g_menu.mission_index);
      ex_menu_begin_cutscene(&g_menu, ms->outro, ms->outro_slides, EX_GS_MISSIONS);
    } else { g_menu.state = EX_GS_BUILDER; g_menu.cursor = 0; }
    return;
  }

  if (g_shot_timer > 0.0f) g_shot_timer -= dt;
  if (g_event_msg_timer > 0.0f) g_event_msg_timer -= dt;

  // ---- movement: target-velocity controller (direction-independent, capped) ----
  ex_element_profile_t mp = ex_element_profile(ex_watch_active(&g_watch));
  int flying = in_vehicle && ex_vehicle_flies(g_vehicle);
  // clean top speeds per mode (tuned down from the old runaway impulse model)
  float max_spd = 13.0f;                             // robot on foot
  if (in_vehicle && !flying) max_spd = 20.0f;        // ground racer
  if (flying)                max_spd = 34.0f;        // jet (faster)
  max_spd *= g_sig_stats.speed_mul;                  // signature mobility
  if (ex_photon_active(&g_photon)) max_spd *= 1.3f;  // light form: fast
  if (ex_matrix_active(&g_matrix)) max_spd *= 1.25f; // Rodimus: swift
  max_spd *= mp.move_mul * g_class_speed_mul;
  if (in_vehicle) max_spd *= ex_vehicle_handling(g_vehicle).speed_mul;   // per-archetype top speed
  if (g_player_slow > 0.0f) max_spd *= 0.40f;   // caught in the resonance field
  if (g_human_scale) max_spd *= 0.45f;          // human pace (no titan strides)

  // camera-relative basis, flattened for ground movement
  vec3 fwd; fwd[0]=camera->front[0]; fwd[1]=flying?camera->front[1]:0.0f; fwd[2]=camera->front[2];
  { float l=sqrtf(fwd[0]*fwd[0]+fwd[1]*fwd[1]+fwd[2]*fwd[2]); if(l>1e-4f){fwd[0]/=l;fwd[1]/=l;fwd[2]/=l;} }
  vec3 rgt; vec3_mul_cross(rgt, camera->front, camera->up); rgt[1]=0.0f;
  { float l=sqrtf(rgt[0]*rgt[0]+rgt[2]*rgt[2]); if(l>1e-4f){rgt[0]/=l;rgt[2]/=l;} }

  // accumulate a wish direction from the held keys
  vec3 wish={0,0,0};
  if (ex_keybind_down(&g_keys, EX_ACT_FORWARD, ex_keys_down)) { wish[0]+=fwd[0]; wish[1]+=fwd[1]; wish[2]+=fwd[2]; }
  if (ex_keybind_down(&g_keys, EX_ACT_BACK,    ex_keys_down)) { wish[0]-=fwd[0]; wish[1]-=fwd[1]; wish[2]-=fwd[2]; }
  if (ex_keybind_down(&g_keys, EX_ACT_RIGHT,   ex_keys_down)) { wish[0]+=rgt[0]; wish[2]+=rgt[2]; }
  if (ex_keybind_down(&g_keys, EX_ACT_LEFT,    ex_keys_down)) { wish[0]-=rgt[0]; wish[2]-=rgt[2]; }
  { float l=sqrtf(wish[0]*wish[0]+wish[1]*wish[1]+wish[2]*wish[2]); if(l>1e-4f){wish[0]/=l;wish[1]/=l;wish[2]/=l;} }
  if (g_clash_active || g_landing_active){ wish[0]=wish[1]=wish[2]=0; }   // locked in clash / landing pick

  // target horizontal velocity, then accelerate toward it (same in every direction)
  float accel = (e->grounded==1 || flying) ? 9.0f : 3.5f;   // slower steering mid-air
  if (in_vehicle && !flying) accel = 6.0f;
  g_veh_grip = 0.85f;
  if (in_vehicle) {
    ex_vehicle_handling_t vh = ex_vehicle_handling(g_vehicle);
    accel    = vh.accel;          // per-archetype responsiveness (heavy vs nimble)
    g_veh_grip = vh.grip;
  }
  float wishmag = sqrtf(wish[0]*wish[0]+wish[2]*wish[2]);
  // JET directional speed: a jet thrusts hard FORWARD, drifts less to the SIDE,
  // and reverses slowly. Scale the top speed by how forward-aligned the wish is.
  float dir_spd = max_spd;
  if (flying && wishmag > 0.1f) {
    float fwdc = (wish[0]*fwd[0] + wish[2]*fwd[2]) / wishmag;   // 1 fwd .. 0 side .. -1 back
    dir_spd = max_spd * ((fwdc >= 0.0f) ? (0.62f + 0.58f*fwdc)   // side 0.62 -> forward 1.20
                                        : (0.62f + 0.42f*fwdc)); // back down to 0.20
  }
  { float foot = in_vehicle ? 1.0f : (g_human_scale ? 0.4f : 0.6f);
    float spd = sqrtf(e->velocity[0]*e->velocity[0]+e->velocity[2]*e->velocity[2]);
    g_surface_grip = ex_material_grip(g_ground_mat, g_feel.mass, spd, foot, g_lean_side); // scrape sparks
    // THE SHARED MOVEMENT CORE -- the exact code the tuning harness measures:
    // target velocity + turn radius + coast + surface grip + drift.
    ex_move_horizontal(e->velocity, wish[0], wish[2], dir_spd, accel, g_veh_grip,
                       g_ground_mat, g_feel.mass, in_vehicle && !flying, foot, g_lean_side, (float)dt);
  }
  float k = fminf(accel*dt, 1.0f);

  if (flying) {
    // vertical: follow the aim pitch component, plus explicit climb/dive
    float tvy = wish[1]*max_spd;
    if (ex_keybind_down(&g_keys, EX_ACT_JUMP,   ex_keys_down)) tvy += max_spd*0.7f;
    if (ex_keybind_down(&g_keys, EX_ACT_CROUCH, ex_keys_down)) tvy -= max_spd*0.7f;
    e->velocity[1] += (tvy - e->velocity[1]) * k;
    e->velocity[1] -= 2.0f*dt;                         // faint sink so idle drifts down
    // aerodynamic drag: air resistance rises with speed (adds glide + a soft cap)
    { float sp=sqrtf(e->velocity[0]*e->velocity[0]+e->velocity[1]*e->velocity[1]+e->velocity[2]*e->velocity[2]);
      float d=fminf(ex_feel_aero_k()*sp*dt, 0.12f); e->velocity[0]-=e->velocity[0]*d; e->velocity[1]-=e->velocity[1]*d; e->velocity[2]-=e->velocity[2]*d; }
  } else {
    e->velocity[1] -= (52.0f * g_gravity_scale * g_map_gravity * dt);  // gravity (per-map)
    // --- WALL CLIMB: on foot, look/press toward a wall and hold JUMP to scale
    //     it. A limited budget (refills on landing) prevents infinite ascents;
    //     you crest the ledge and can then step onto it. ---
    if (e->grounded == 1) g_climb_budget = 4.0f;   // generous refill -- rarely run out mid-climb
    if (!in_vehicle && !g_human_scale && e->grounded != 1 && g_climb_budget > 0.0f &&
        ex_keybind_down(&g_keys, EX_ACT_JUMP, ex_keys_down)) {
      float fdx = camera->front[0], fdz = camera->front[2];
      float wtop;
      if (player_wall_ahead(fdx, fdz, e->position, &wtop)) {
        float dl = sqrtf(fdx*fdx + fdz*fdz); if (dl > 1e-4f){ fdx/=dl; fdz/=dl; }
        // near the top, ease off so you arc over the ledge instead of rocketing up
        float near_top = (e->position[1] > wtop - 0.35f) ? 0.65f : 1.0f;
        // SURFACE GRIP: slick surfaces climb a touch weaker, but only a touch --
        // the hold stays reliable so you very rarely slip off. (Was much harsher.)
        float grip = 0.80f + 0.20f*g_surface_grip;   // ~0.83 on ice .. 1.0 on metal
        e->velocity[1] = 8.8f * near_top * grip;              // scale the wall
        e->velocity[0] = e->velocity[0]*0.4f + fdx*2.2f;      // hug it (kill outward drift)
        e->velocity[2] = e->velocity[2]*0.4f + fdz*2.2f;
        g_climb_budget -= (float)dt * (1.0f + (1.0f-grip)*0.5f);  // drains only ~1.1x on ice
        g_was_airborne = 1;
        if (g_climb_msg_cd <= 0.0f){
          snprintf(g_shot_msg,sizeof(g_shot_msg), grip<0.9f?"CLIMB -- slick":"CLIMB");
          g_shot_timer=0.6f; g_climb_msg_cd=1.5f; }
      }
    }
    if (g_climb_msg_cd > 0.0f) g_climb_msg_cd -= (float)dt;
    if (ex_keybind_down(&g_keys, EX_ACT_JUMP, ex_keys_down) && e->grounded == 1) {
      // ADAPTIVE JUMP: scales with transform MODE and bot SHAPE. Legged alt-modes
      // spring; wheeled/tracked barely hop; agile light frames leap high, heavy
      // bulky titans clear little. So the same key means different things per bot.
      float jv = 18.0f;   // robot baseline
      if (in_vehicle) {
        ex_vehicle_handling_t vh = ex_vehicle_handling(g_vehicle);
        int legged = (g_vehicle==EX_VEH_RAPTOR||g_vehicle==EX_VEH_WOLF||g_vehicle==EX_VEH_INSECT||
                      g_vehicle==EX_VEH_SPIDER||g_vehicle==EX_VEH_DINO||g_vehicle==EX_VEH_SCORPION||
                      g_vehicle==EX_VEH_CRAB||g_vehicle==EX_VEH_DRAGON);
        jv = legged ? 20.0f*vh.speed_mul : 9.0f;   // legs bound, wheels/tracks hop
      }
      jv *= (0.65f + 0.45f*g_class_speed_mul);   // agile class higher, titan lower
      jv *= (1.30f - 0.32f*g_build.bulk);        // bulk drags the jump down
      jv /= (0.75f + 0.25f*g_feel.mass);         // mass drags it down
      if (g_gravity_scale*g_map_gravity < 1.0f) jv *= 1.4f;   // more air in low-g
      e->velocity[1] = jv;
      g_jump_launch = 1; sfx(g_sfx_jump); g_cam_trauma += 0.06f + jv*0.004f;   // bigger leap kicks harder
    }
  }
  move_speed = max_spd;  // kept for anims that read the nominal speed

  // ---- pickups: bob/spin, collect on touch, map to glow models ----
  if (g_overcharge_t > 0.0f) g_overcharge_t -= dt;
  ex_pickup_update(&g_pickups, dt);
  { int got = ex_pickup_try_collect(&g_pickups, e->position[0], e->position[1], e->position[2], 2.2f);
    if (got != 0) sfx(g_sfx_pickup);
    if (got == EX_PICKUP_ENERGON)   { g_player_hp = fminf(g_player_max_hp, g_player_hp + 30.0f); g_repair_energy = fminf(200.0f, g_repair_energy + 60.0f); snprintf(g_shot_msg,sizeof(g_shot_msg),"+energon"); g_shot_timer=1.5f; }
    else if (got == EX_PICKUP_SHIELD){ g_player_armor = fminf(60.0f, g_player_armor + 40.0f); snprintf(g_shot_msg,sizeof(g_shot_msg),"+shield"); g_shot_timer=1.5f; }
    else if (got == EX_PICKUP_OVERCHARGE){ g_overcharge_t = 10.0f; snprintf(g_shot_msg,sizeof(g_shot_msg),"OVERCHARGE!"); g_shot_timer=1.5f; }
    else if (got == EX_PICKUP_MATRIX){
      // the Matrix opens: a golden light bursts out (clears the space), and the
      // bearer rises as Rodimus for a spell.
      ex_matrix_claim(&g_matrix);
      g_player_hp = fminf(g_player_max_hp, g_player_hp + g_player_max_hp*0.35f);
      apply_shockwave(e->position[0], e->position[1], e->position[2], 44.0f, 30.0f, 60.0f);
      spawn_shockring(e->position[0], e->position[1], e->position[2], 44.0f);
      spawn_shockring(e->position[0], e->position[1], e->position[2], 28.0f);
      { uint8_t gc[3]={255,240,170}; ex_vfx_spawn_impact(&g_vfx,(vec3){e->position[0],e->position[1]+1.6f,e->position[2]},44,gc); }
      g_cam_trauma += 1.0f; sfx(g_sfx_xform);
      snprintf(g_shot_msg,sizeof(g_shot_msg),"THE MATRIX OPENS -- RODIMUS"); g_shot_timer=3.0f;
    }
  }
  for (int i = 0; i < g_pickups.count; i++) {
    ex_pickup_t *pk = &g_pickups.items[i];
    if (!pk->alive) { g_pickup_models[i]->position[1] = -1000.0f; continue; }
    uint8_t c[3]; ex_pickup_color(pk->kind, c);
    g_pickup_models[i]->position[0]=pk->pos[0];
    g_pickup_models[i]->position[1]=ex_pickup_bob(pk);
    g_pickup_models[i]->position[2]=pk->pos[2];
    g_pickup_models[i]->rotation[1]=pk->t*90.0f;
    g_pickup_models[i]->scale=0.4f;
    g_pickup_models[i]->tint[0]=c[0]/255.0f; g_pickup_models[i]->tint[1]=c[1]/255.0f; g_pickup_models[i]->tint[2]=c[2]/255.0f;
  }

  // ---- vfx: advance and map live effects onto the model pool ----
  if (g_boss_light) g_boss_light->position[1] = -1000.0f;   // hidden unless an Apex is alive
  ex_vfx_update(&g_vfx, dt);
  {
    int mi = 0;
    for (int i = 0; i < EX_VFX_MAX && mi < VFX_MODELS; i++) {
      ex_vfx_t *v = &g_vfx.items[i];
      if (!v->active) continue;
      ex_model_t *vm = g_vfx_models[mi++];
      vm->position[0]=v->pos[0]; vm->position[1]=v->pos[1]; vm->position[2]=v->pos[2];
      float s = v->scale * (v->kind==EX_VFX_FLASH ? (0.5f + v->life/v->max_life) : 1.0f);
      if (v->kind==EX_VFX_BOLT) {   // elongated glowing streak along travel dir
        vm->scale3[0]=s*1.4f; vm->scale3[1]=s*1.4f; vm->scale3[2]=s*3.5f;
        float vl=sqrtf(v->vel[0]*v->vel[0]+v->vel[2]*v->vel[2]);
        vm->rotation[1] = (vl>0.01f)? -atan2f(v->vel[2],v->vel[0])*57.2957795f : 0.0f;
      } else { vm->scale3[0]=vm->scale3[1]=vm->scale3[2]=0.0f; vm->scale = s*1.6f; }
      vm->tint[0]=v->color[0]/255.0f; vm->tint[1]=v->color[1]/255.0f; vm->tint[2]=v->color[2]/255.0f;
    }
    for (int i = mi; i < VFX_MODELS; i++) g_vfx_models[i]->position[1] = -1000.0f;
  }

  ex_scene_update(scene, dt);
  // (mouse-look already applied at the top of the frame)
  // keep the active skybox centred on the camera (infinite-distance feel)
  for (int i = 0; i < NUM_SKY; i++) {
    if (i == g_active_sky) {
      g_sky_models[i]->position[0]=camera->position[0];
      g_sky_models[i]->position[1]=camera->position[1];
      g_sky_models[i]->position[2]=camera->position[2];
    } else g_sky_models[i]->position[1] = -100000.0f;
  }

  // ---- third-person chase camera: watch your robot transform ----
  // ex_fps_camera_update just computed camera->front from the mouse and built
  // a first-person view. Re-place the camera behind + above the player and
  // rebuild the view so the character is visible in front of us.
  if (g_landing_active) {
    // TOP-DOWN LANDING VIEW: hang the camera high over the chosen drop zone,
    // looking straight down, so you can place the warp precisely.
    camera->position[0]=g_landing_tgt[0]; camera->position[1]=140.0f; camera->position[2]=g_landing_tgt[2]+0.01f;
    vec3 ctr={g_landing_tgt[0], 0.0f, g_landing_tgt[2]};
    vec3 upz={0,0,-1};
    mat4x4_look_at(camera->matrices.view, camera->position, ctr, upz);
    mat4x4_invert(camera->matrices.inverse_view, camera->matrices.view);
    // pulsing reticle ring at the target
    if (g_landing_ring){ float pu=0.6f+0.4f*sinf(g_landing_spin*3.0f);
      g_landing_ring->position[0]=g_landing_tgt[0]; g_landing_ring->position[1]=0.5f; g_landing_ring->position[2]=g_landing_tgt[2];
      g_landing_ring->scale3[0]=42.0f; g_landing_ring->scale3[1]=0.3f; g_landing_ring->scale3[2]=42.0f;
      g_landing_ring->rotation[1]=g_landing_spin*40.0f;
      g_landing_ring->tint[0]=0.5f*pu; g_landing_ring->tint[1]=0.8f*pu; g_landing_ring->tint[2]=1.0f*pu; }
  } else {
    if (g_landing_ring) g_landing_ring->position[1]=-1000.0f;
    const float cam_dist = 7.0f, cam_up = 3.0f;
    vec3 eye = { e->position[0], e->position[1] + 2.0f, e->position[2] };
    vec3 back; vec3_scale(back, camera->front, -cam_dist);
    vec3_add(camera->position, eye, back);
    camera->position[1] += cam_up;
    // vehicle g-force: shift the camera in camera-space by the weighted g offset
    // so accel/brake/cornering are FELT as the camera loading up under force.
    vec3 rt; vec3_mul_cross(rt, camera->front, camera->up); vec3_norm(rt, rt);
    camera->position[0] += rt[0]*g_cam_gf[0] + camera->front[0]*g_cam_gf[2];
    camera->position[1] += g_cam_gf[1]*4.0f;
    camera->position[2] += rt[2]*g_cam_gf[0] + camera->front[2]*g_cam_gf[2];
    // bank the horizon into corners (suspension roll) via a rolled up-vector
    vec3 up2 = { camera->up[0], camera->up[1], camera->up[2] };
    // bank tuning: -15% globally, another -50% when not airborne (loose banks
    // suit jets, not grounded bots), and scaled by the map's gravity so low-g
    // worlds feel floatier and heavy worlds feel planted (gravity facing).
    int cam_flying = (g_avatar_xform->mode==EX_MODE_VEHICLE) && ex_vehicle_flies(g_vehicle);
    float bank_mul = 0.42f * (cam_flying ? 1.0f : 0.5f) * (0.55f + 0.45f*g_map_gravity);  // another -50%
    { float r = g_susp_roll * ex_feel_bank() * bank_mul;
      r = fmaxf(-0.18f, fminf(0.18f, r));   // hard clamp so hard corners don't snowball (was too exponential)
      vec3 fxu; vec3_mul_cross(fxu, camera->front, up2);
      float cr=cosf(r), sr=sinf(r);
      up2[0]=up2[0]*cr+fxu[0]*sr; up2[1]=up2[1]*cr+fxu[1]*sr; up2[2]=up2[2]*cr+fxu[2]*sr; }
    vec3 center; vec3_add(center, camera->position, camera->front);
    mat4x4_look_at(camera->matrices.view, camera->position, center, up2);
    mat4x4_invert(camera->matrices.inverse_view, camera->matrices.view);
  }

  // ---- HUD ----
  ex_vga_clear();
  // boss bar: the named mini-boss (Cryotek) or the apex devourer, if present
  {
    const char *bn=NULL; float bf=0.0f;
    if (g_champion_slot>=0 && g_champion_slot<g_enc.count && g_enc.enemies[g_champion_slot].state!=EX_ENEMY_DEAD){
      bn=(g_champion_kind==1)?"CRYOTEK":"BLACKSTAR"; bf = g_champion_max>0.0f ? g_enc.enemies[g_champion_slot].hp/g_champion_max : 0.0f;
    } else {
      ex_enemy_t *ab = ex_encounter_boss_ptr(&g_enc);
      if (ab && ab->state!=EX_ENEMY_DEAD){ bn="THE DEVOURER"; bf = ab->max_hp>0.0f ? ab->hp/ab->max_hp : 0.0f; }
    }
    if (bn){
      if (bf<0.0f)bf=0.0f; if(bf>1.0f)bf=1.0f;
      int bw=24, x0=(32-bw)/2, nl=(int)strlen(bn), nx=(32-nl)/2; if(nx<0)nx=0;
      ex_vga_setfg(190,225,255,255); ex_vga_print((size_t)nx,0,bn);
      int filled=(int)(bf*bw+0.5f); if(filled>bw)filled=bw; if(filled<0)filled=0;
      char full[40], empty[40];
      for(int k=0;k<filled;k++) full[k]='#'; full[filled]=0;
      for(int k=0;k<bw-filled;k++) empty[k]='.'; empty[bw-filled]=0;
      ex_vga_setfg(120,210,255,255); ex_vga_print((size_t)x0,1,full);
      ex_vga_setfg(70,80,95,255);    ex_vga_print((size_t)(x0+filled),1,empty);
    }
  }
  char buf[64];
  ex_vga_setfg(255,255,255,255); ex_vga_setbg(0,0,0,255);
  sprintf(buf, "hp %.0f%s%s", g_player_hp,
          g_player_armor>0?"  shield ":"", g_player_armor>0?"":"");
  ex_vga_print(2, 1, buf);
  { const ex_class_info_t *ci = ex_class_info((ex_class_t)g_menu.loadout.player_class);
    ex_vga_setfg(150,180,220,255); ex_vga_print(2, 2, ci->name); ex_vga_setfg(255,255,255,255); }
  { int held=0; for(int i=0;i<NUM_OBJ_PADS;i++) if(g_obj_owner[i]==1) held++;
    char ob[40];
    if (g_control_target>0.0f) snprintf(ob,sizeof(ob),"HOLD  ctrl %d/%d  %d/%d", held, NUM_OBJ_PADS, (int)g_obj_score, (int)g_control_target);
    else snprintf(ob,sizeof(ob),"ctrl %d/%d  score %d", held, NUM_OBJ_PADS, (int)g_obj_score);
    ex_vga_setfg(120,230,255,255); ex_vga_print(2, 3, ob); ex_vga_setfg(255,255,255,255); }
  if (g_protect_mission && g_survivors_total>0 && g_survivors_lost>=g_survivors_total && g_match_live){
    player_hit(9999.0f, e->position[0], e->position[2], "ALL SURVIVORS LOST"); }
  if (g_survivors_total>0){ char sb[28]; int alive=g_survivors_total-g_survivors_lost;
    snprintf(sb,sizeof(sb),"survivors %d/%d", alive, g_survivors_total);
    ex_vga_setfg(alive<g_survivors_total? 255:150, 200, alive<g_survivors_total?120:200,255);
    ex_vga_print(2, 4, sb); ex_vga_setfg(255,255,255,255); }
  { int broken=0; for(int i=0;i<EX_ROBOT_PARTS;i++) if(g_part_detached[i]) broken++;
    if (broken>0 || g_repair_energy>0.5f){ char eb[30];
      snprintf(eb,sizeof(eb),"repair %d%%  broken %d", (int)g_repair_energy, broken);
      ex_vga_setfg(120,255,200,255); ex_vga_print(2, 5, eb); ex_vga_setfg(255,255,255,255); } }
  if (ex_keys_down[SDL_SCANCODE_F1]){ g_tune_active=!g_tune_active; ex_keys_down[SDL_SCANCODE_F1]=0; }
  if (g_tune_active) feel_tuner_update_draw();
  clash_update_draw((float)dt);
  if (g_menu.gfx.debug) {
    // profiler + state readout (gated on the settings toggle). Kept inside the
    // 32x16 VGA grid (rows 0-15, cols 0-31) so it stays on the HUD, not the floor.
    static float favg=0; favg += ((float)(1.0/fmax(ft,1e-4)) - favg)*0.1f;
    int en_alive=0; for(int i=0;i<g_enc.count;i++) if(g_enc.enemies[i].state!=EX_ENEMY_DEAD) en_alive++;
    int dbg_active=0; for(int i=0;i<NUM_DEBRIS;i++) if(g_debris[i].active) dbg_active++;
    float spd=sqrtf(e->velocity[0]*e->velocity[0]+e->velocity[2]*e->velocity[2]);
    char db[18];
    ex_vga_setfg(255,240,120,255);
    snprintf(db,sizeof(db),"%.0ffps %.1fms", favg, ft*1000.0);   ex_vga_print(18,11,db);
    snprintf(db,sizeof(db),"e%d dbr%d", en_alive, dbg_active);    ex_vga_print(18,12,db);
    snprintf(db,sizeof(db),"spd%.0f g%.1f", spd, g_map_gravity);  ex_vga_print(18,13,db);
    snprintf(db,sizeof(db),"col%d", g_auto_col_n);                ex_vga_print(18,14,db);
    ex_vga_setfg(255,255,255,255);
  }
  if (g_landing_active) {
    ex_vga_print(7, 8, "WASD move   click/K drop"); ex_vga_setfg(255,255,255,255);
  }
  if (g_toxic_dot > 0.0f){ char tb[28]; snprintf(tb,sizeof(tb),"TOXIC %ds", (int)g_toxic_dot);
    ex_vga_setfg(120,255,90,255); ex_vga_print(2, 7, tb); ex_vga_setfg(255,255,255,255); }
  // SPARK SIGNATURE meter: how detectable you are to resonance-hunting Corrupted.
  { int bars = (int)(g_spark_loud*8.0f + 0.5f); if(bars>8)bars=8;
    char sm[16]="SPRK "; for(int b=0;b<8;b++) sm[5+b] = (b<bars)?'|':'.'; sm[13]=0;
    if (g_spark_loud>0.66f) ex_vga_setfg(255,80,60,255);
    else if (g_spark_loud>0.33f) ex_vga_setfg(255,200,80,255);
    else ex_vga_setfg(90,200,120,255);
    ex_vga_print(2, 8, sm); ex_vga_setfg(255,255,255,255); }
  if (g_horror_mode && g_dread > 0.08f) {
    // a proximity warning that intensifies as something closes in the fog
    const char *w = (g_dread>0.75f)? "!! IT IS HERE !!" : (g_dread>0.4f)? "-- CONTACT NEAR --" : "signal .. faint";
    int fl = (g_dread>0.6f && ((int)(g_anim_t*8)&1)) ? 255 : 180;
    ex_vga_setfg(fl, (int)(120*(1.0f-g_dread)), (int)(120*(1.0f-g_dread)), 255);
    ex_vga_print(10, 6, w); ex_vga_setfg(255,255,255,255);
  }
  if (g_player_armor>0){ ex_vga_setfg(96,156,255,255); sprintf(buf,"shield %.0f",g_player_armor); ex_vga_print(14,1,buf); }
  if (g_overcharge_t>0){ ex_vga_setfg(255,200,90,255); sprintf(buf,"OVERCHARGE %.0fs",g_overcharge_t); ex_vga_print(2,11,buf); }
  // damage feedback: red screen-edge flash + directional marker + source label
  if (g_hit_flash > 0.0f) {
    int a=(int)(g_hit_flash*200); if(a>200)a=200;
    ex_vga_setfg(255,40,40,255);
    for (int c=0;c<32;c++){ ex_vga_print(c,0,"-"); ex_vga_print(c,15,"-"); }
    for (int rr=0;rr<16;rr++){ ex_vga_print(0,rr,"|"); ex_vga_print(31,rr,"|"); }
    // arrow toward the hit source, relative to facing
    float rel = atan2f(g_hit_dir[2]-e->position[2], g_hit_dir[0]-e->position[0]) - atan2f(camera->front[2],camera->front[0]);
    const char *arw = "^"; float d=rel;
    while(d>3.14159f)d-=6.2832f; while(d<-3.14159f)d+=6.2832f;
    if (d>-0.785f&&d<0.785f) arw=">"; else if (d>=0.785f&&d<2.356f) arw="v";
    else if (d<=-0.785f&&d>-2.356f) arw="^"; else arw="<";
    ex_vga_setfg(255,90,90,255); ex_vga_print(15,10,arw);
    ex_vga_print(13,11,g_hit_src);
  }
  ex_vga_setfg(255,255,255,255);
  { extern float ex_vga_scale; float sc = ex_vga_scale; if(sc<0.22f)sc=0.22f; if(sc>1.0f)sc=1.0f;
    int cxc = (int)(16.0f/sc); int cyr = (int)(8.0f/sc);
    if (cxc>31) cxc=31; if (cyr>15) cyr=15;
    ex_vga_print(cxc, cyr, "+"); }  // crosshair at true screen centre

  // ---- navigation radar (top-right): the world is huge, so show enemies, the
  // central megastructure, and your facing relative to your position. ----
  {
    const int cc = 27, cr = 4, R = 3;      // radar centre col/row + radius
    const float range = 140.0f;            // world units mapped to R cells
    // faint border corners
    ex_vga_setfg(70,110,90,255);
    ex_vga_print(cc-R-1, cr-R-1, "."); ex_vga_print(cc+R+1, cr-R-1, ".");
    ex_vga_print(cc-R-1, cr+R+1, "."); ex_vga_print(cc+R+1, cr+R+1, ".");
    // objective pads (cyan O)
    ex_vga_setfg(90,220,255,255);
    for (int oi=0;oi<NUM_OBJ_PADS;oi++){
      float dx=g_obj_pos[oi][0]-e->position[0], dz=g_obj_pos[oi][2]-e->position[2];
      int bc=cc+(int)(dx/range*R+(dx<0?-0.5f:0.5f)), br=cr+(int)(dz/range*R+(dz<0?-0.5f:0.5f));
      if (bc>=cc-R&&bc<=cc+R&&br>=cr-R&&br<=cr+R) ex_vga_print(bc,br,"O");
    }
    // central megastructure marker (world 0,0)
    { float dx=0-e->position[0], dz=0-e->position[2];
      int bc=cc+(int)(dx/range*R+0.5f), br=cr+(int)(dz/range*R+0.5f);
      if (bc>=cc-R&&bc<=cc+R&&br>=cr-R&&br<=cr+R){ ex_vga_setfg(200,180,90,255); ex_vga_print(bc,br,"#"); } }
    // enemy blips
    ex_vga_setfg(255,90,80,255);
    for (int i=0;i<g_enc.count;i++){ ex_enemy_t *en=&g_enc.enemies[i];
      if (en->state==EX_ENEMY_DEAD) continue;
      float dx=en->position[0]-e->position[0], dz=en->position[2]-e->position[2];
      int bc=cc+(int)(dx/range*R+ (dx<0?-0.5f:0.5f));
      int br=cr+(int)(dz/range*R+ (dz<0?-0.5f:0.5f));
      if (bc<cc-R)bc=cc-R; if (bc>cc+R)bc=cc+R; if (br<cr-R)br=cr-R; if (br>cr+R)br=cr+R;
      ex_vga_print(bc,br, en->kind==EX_ENEMY_APEX?"A":"*"); }
    // player at centre
    ex_vga_setfg(120,255,160,255); ex_vga_print(cc,cr,"@");
  }
  ex_vga_setfg(255,255,255,255);
  ex_vga_setfg(255,255,0,255);
  sprintf(buf, "fps %.0f", 1.0/ft); ex_vga_print(2, 2, buf);

  const char *mode = in_vehicle ? "vehicle" : "robot";
  const char *st = g_avatar_xform->state == EX_XFORM_STATE_TRANSFORMING ? "morphing" :
                   g_avatar_xform->state == EX_XFORM_STATE_LOCKED ? "locked" : "";
  ex_vga_setfg(0,255,255,255);
  sprintf(buf, "mode %s %s", mode, st); ex_vga_print(2, 4, buf);
  ex_element_t ael = ex_watch_active(&g_watch);
  sprintf(buf, "element %s", ael==EX_ELEM_NONE ? "attuning" : ex_element_name(ael));
  ex_vga_print(2, 5, buf);

  ex_vga_setfg(255,180,80,255);
  if (g_wave_mode) {
    if (g_waves.intermission > 0.0f)
      sprintf(buf, "WAVE %d cleared!  next in %.0f   score %ld", g_waves.wave, g_waves.intermission, g_waves.score);
    else
      sprintf(buf, "WAVE %d%s   enemies %d   score %ld", g_waves.wave,
              g_waves.total_waves?"":"  (endless)", ex_encounter_alive(&g_enc), g_waves.score);
  } else if (g_enc.mode == EX_WIN_SURVIVE)
    sprintf(buf, "SURVIVE  %.0fs   enemies %d", g_enc.survive_timer, ex_encounter_alive(&g_enc));
  else
    sprintf(buf, "enemies left  %d / %d", ex_encounter_alive(&g_enc), g_enc.count);
  ex_vga_print(2, 7, buf);
  // note deepest corrupted stage present, for the horror read
  { int deepest=0; for(int i=0;i<g_enc.count;i++){ ex_enemy_t*en=&g_enc.enemies[i];
      if(en->kind==EX_ENEMY_CORRUPTED && en->state!=EX_ENEMY_DEAD && en->stage>deepest) deepest=en->stage; }
    if(deepest){ ex_vga_setfg(200,120,220,255);
      sprintf(buf,"corruption: %s", ex_enemy_stage_name(deepest)); ex_vga_print(2,8,buf); } }

  { ex_enemy_t *boss = ex_encounter_boss_ptr(&g_enc);
    if (boss) {
      ex_vga_setfg(200,80,220,255);
      int pct = (int)(boss->hp / boss->max_hp * 20.0f);
      char bar[40]; int bi=0; bar[bi++]='[';
      for(int k=0;k<20;k++) bar[bi++] = (k<pct)?'#':'-';
      bar[bi++]=']'; bar[bi]=0;
      char line[64]; snprintf(line,sizeof(line),"APEX CORRUPTED  P%d %s", boss->phase, bar);
      ex_vga_print(20, 1, line);
    }
  }
  if (g_shot_timer > 0.0f)      { ex_vga_setfg(200,200,255,255); ex_vga_print(2, 10, g_shot_msg); }
  if (g_event_msg_timer > 0.0f) { ex_vga_setfg(120,255,120,255); ex_vga_print(2, 11, g_event_msg); }
}

void game_draw()
{
  ex_scene_draw(scene, 0, 0, 0, 0, &camera->matrices);
  ex_fps_camera_resize(camera);
  ex_vga_render();
}

void game_exit()
{
  if (g_avatar_xform) ex_xform_free(g_avatar_xform);
  ex_chunk_world_free(g_chunks);
  ex_event_bus_free(g_bus);
  ex_scene_destroy(scene);
  ex_vga_destroy();
  printf("Exiting\n");
}

void game_keypressed(uint32_t key) {}
void game_mousepressed(uint8_t button)
{
  (void)button;   // firing reads the held mouse/key state directly (full-auto)
}
void game_mousemoition(int xrel, int yrel) {}
void game_mousewheel(int32_t x, int32_t y)
{
  (void)x;
  if (y == 0) return;
  if (g_menu.state == EX_GS_PLAYING) {
    // scroll to change the active Watch form
    int n = g_form_unlocked_photon ? EX_FORM_COUNT : EX_FORM_COUNT-1;
    g_form = (ex_watch_form_t)(((int)g_form + (y>0?1:n-1)) % n);
  } else {
    // scroll adjusts the highlighted value left/right across every menu
    ex_menu_nav(&g_menu, y > 0 ? EX_NAV_RIGHT : EX_NAV_LEFT);
  }
}
void game_resize(uint32_t width, uint32_t height) { ex_scene_resize(scene, width, height); }
