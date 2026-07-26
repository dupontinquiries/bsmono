#include "robot_parts.h"
#include <math.h>
#include <string.h>

/* neutral default -- an un-styled body; the game overwrites this per-entity */
ex_body_style_t ex_active_body_style = { 1.0f, 1.0f, 0.0f, 0.0f, 0.5f, 0.0f, 0.4f, 1.0f };

#ifndef M_TAU
#define M_TAU 6.28318530718f
#endif

float ex_gait_advance(float phase, float speed, float dt)
{
  float s = (speed > 0.0f ? speed : 0.0f);
  // Physically-based cadence: stride FREQUENCY rises with the square root of speed
  // (the Froude relation for legged locomotion) and saturates, so the legs take slower,
  // longer strides at speed instead of whirling. Stride LENGTH (leg_swing amp) grows to
  // carry the ground the slower cadence no longer does. Overall ~40% slower than before.
  float freq = EX_GAIT_CADENCE * 1.70f * sqrtf(s);
  if (freq > 7.4f) freq = 7.4f;                    // a sprint never spins the legs
  float next = phase + freq * dt;
  next = fmodf(next, M_TAU);
  if (next < 0.0f) next += M_TAU;
  return next;
}

static void element_tint(int el, uint8_t c[3])
{
  switch (el) {
    case 1: c[0]=150;c[1]=210;c[2]=245; break; // ice
    case 2: c[0]=225;c[1]=95; c[2]=45;  break; // fire
    case 3: c[0]=200;c[1]=215;c[2]=205; break; // air
    case 4: c[0]=235;c[1]=205;c[2]=85;  break; // electric
    case 5: c[0]=95; c[1]=165;c[2]=80;  break; // vine
    case 6: c[0]=175;c[1]=140;c[2]=110; break; // rock
    case 7: c[0]=85; c[1]=150;c[2]=210; break; // water
    default:c[0]=140;c[1]=150;c[2]=165; break; // steel
  }
}

static void P(ex_part_pose_t *p, float x, float y, float z, float sx, float sy, float sz)
{
  p->offset[0]=x; p->offset[1]=y; p->offset[2]=z;
  p->sx=sx; p->sy=sy; p->sz=sz;
  p->scale = (sx+sy+sz)/3.0f; // uniform fallback
}

static float leg_swing(float phase, int odd, float speed)
{
  // Stride LENGTH grows with speed (no early cap) so faster movement means LONGER strides,
  // not just faster ones -- covering the ground the slower cadence gives up. A mild second
  // harmonic breaks the clean sine into a quicker plant + slower lift, so the stride reads
  // organic and weighted rather than a metronome. Zero at a standstill.
  float amp = fminf(speed / 6.0f, 1.5f) * EX_LEG_SWING_MAX;
  float p = phase + (odd ? (float)M_PI : 0.0f);
  return amp * (sinf(p) + 0.12f * sinf(2.0f * p));
}

void ex_robot_layout_ex(ex_part_pose_t poses[EX_ROBOT_PARTS], int vehicle,
                        ex_body_kind kind, const ex_build_params_t *bp,
                        float size_scale, float phase, float speed, int element)
{
  memset(poses, 0, sizeof(ex_part_pose_t)*EX_ROBOT_PARTS);
  ex_build_params_t d = ex_build_default();
  if (!bp) bp = &d;

  float H = bp->height, BK = bp->bulk, HD = bp->head;
  float SH = bp->shoulders, AR = bp->arm, LG = bp->leg;
  AR *= ex_active_body_style.arm_len;   // cinematic limb proportions (lithe long-legged
  LG *= ex_active_body_style.leg_len;   // predators vs heavy short-limbed bruisers)
  /* armlen archetype scalar applied after presets below */
  // ---- BODY ARCHETYPE: reshape the base rig by build. Driven by the alt-mode so
  //      a racer/beast reads bottom-heavy (the wheelbase/haunches fold into long
  //      calves + a short stocky torso), while other frames go wide-body /
  //      short-leg / short-torso. Separate CALF vs THIGH + TORSO scalars let the
  //      silhouette actually change, not just uniform size. ----
  float calf=1.0f, thigh=1.0f, torso=1.0f, torsoW=1.0f, stance=1.0f, armlen=1.0f;
  if (kind==EX_BODY_PLAYER || kind==EX_BODY_GRUNT || kind==EX_BODY_HIJACK) {
    int alt = bp->alt_mode;
    switch (alt) {
      /* CAR/CYCLE + DINO/RAPTOR/DRAGON/SCORPION/WOLF: LONG CALF, short stocky torso,
         the vehicle mass tucked into the lower legs -> a coiled, bottom-heavy look. */
      case 0: case 11:
      case 6: case 7: case 5: case 12: case 15:
        calf=1.55f; thigh=0.82f; torso=0.78f; torsoW=1.18f; armlen=0.92f; stance=1.05f; break;
      /* TANK/MECH/SENTRY/BULWARK: WIDE BODY, SHORT LEG, short torso -- a squat siege
         frame planted low and broad. */
      case 1: case 9: case 16: case 19:
        calf=0.85f; thigh=0.80f; torso=0.85f; torsoW=1.55f; armlen=1.10f; stance=1.35f; break;
      /* JET/FLYERS: tall + LONG LEG, slim torso -- a lean, aerodynamic riser. */
      case 2: case 8: case 14: case 17: case 18:
        calf=1.25f; thigh=1.15f; torso=0.95f; torsoW=0.86f; armlen=1.05f; stance=0.92f; break;
      default: /* balanced humanoid, but still calf-forward per the concept */
        calf=1.30f; thigh=0.88f; torso=0.86f; torsoW=1.06f; break;
    }
  }

  // per-kind silhouette: each enemy type reads differently at a glance.
  float hunch = 0.0f;
  switch (kind) {
    case EX_BODY_HIJACK:    // small, spindly swarm BUG: oversized sensor head, insect limbs, scuttling crouch
      H*=0.66f; BK*=0.52f; AR*=1.22f; LG*=1.18f; HD*=1.60f; SH*=0.55f; hunch=0.34f; break;
    case EX_BODY_APEX:      // towering, hunched, heavily-armoured boss
      H*=1.55f; BK*=1.95f; AR*=1.55f; LG*=1.18f; HD*=0.8f; SH*=2.05f; hunch=0.30f; break;
    case EX_BODY_CORRUPTED: // twisted, hunched, over-grown
      H*=1.14f; BK*=1.28f; AR*=1.22f; LG*=1.0f;  HD*=1.05f; SH*=1.35f; hunch=0.32f; break;
    case EX_BODY_HUMAN:     // slender biological figure: narrow, upright, no bulk
      H*=1.02f; BK*=0.42f; AR*=0.80f; LG*=1.08f; HD*=0.92f; SH*=0.68f; hunch=0.04f; break;
    default: break;         // grunt / player baseline
  }

  float move_amp = fminf(speed/6.0f, 1.0f);
  float bob = move_amp * EX_BODY_BOB_MAX * sinf(2.0f*phase);
  // idle breathing + subtle weight sway when standing still, so robots read as
  // alive rather than frozen statues.
  float idle = 1.0f - move_amp;
  float restless = 0.4f + ex_active_body_style.idle_restless;    // twitchy vs calm idle
  bob += idle * 0.045f * restless * sinf(phase*1.1f);            // slow chest rise/fall
  float idle_sway = idle * 0.03f * restless * sinf(phase*0.7f);  // gentle side weight shift
  float twitch = (kind==EX_BODY_CORRUPTED||kind==EX_BODY_APEX)? sinf(phase*9.0f)*0.10f + sinf(phase*23.0f)*0.05f : 0.0f;

  uint8_t base[3]; element_tint(element, base);
  if (kind==EX_BODY_HUMAN){ base[0]=90;base[1]=95;base[2]=110; }   // muted suit grey-blue
  else if (kind==EX_BODY_CORRUPTED || kind==EX_BODY_APEX){ base[0]=120;base[1]=90;base[2]=130; }
  if (kind==EX_BODY_APEX){ base[0]=90;base[1]=50;base[2]=100; }
  else if (kind==EX_BODY_GRUNT){ base[0]=(base[0]+90)/2; base[1]=(base[1]+90)/2; base[2]=(base[2]+95)/2; }
  else if (kind==EX_BODY_HIJACK){ base[0]=130;base[1]=150;base[2]=70; } // sickly swarm
  // PLAYER PAINT: a chosen primary scheme overrides the element-default body colour
  if (kind==EX_BODY_PLAYER && bp && bp->paint > 0){
    static const uint8_t PAINT[8][3] = {
      {140,150,165},  // 0 steel (default, unused here)
      {190, 40, 40},  // 1 crimson
      { 40, 90,200},  // 2 cobalt
      { 30,150, 90},  // 3 emerald
      {220,150, 30},  // 4 amber
      {150, 60,190},  // 5 violet
      { 40, 45, 55},  // 6 gunmetal
      {225,225,235},  // 7 pearl
    };
    int pi = bp->paint & 7; base[0]=PAINT[pi][0]; base[1]=PAINT[pi][1]; base[2]=PAINT[pi][2];
  }
  // FINISH: matte darkens + flattens; metallic is the default; glossy brightens.
  if (kind==EX_BODY_PLAYER && bp){
    if (bp->finish==0){ for(int c=0;c<3;c++) base[c]=(uint8_t)(base[c]*0.82f); }
    else if (bp->finish==2){ for(int c=0;c<3;c++){ int v=base[c]+40; base[c]=(uint8_t)(v>255?255:v); } }
  }
  uint8_t accent[3]; element_tint(bp->accent_element? bp->accent_element : element, accent);

  if (!vehicle) {
    float lz = leg_swing(phase,0,speed), rz = leg_swing(phase,1,speed);
    float alz = -lz*(EX_ARM_SWING_MAX/EX_LEG_SWING_MAX), arz = -rz*(EX_ARM_SWING_MAX/EX_LEG_SWING_MAX);
    float hip = 0.85f*H*LG*0.5f;   // hip height baseline

    // torso: chest (broad, tapered) + abdomen (narrower)
    P(&poses[EX_PART_CHEST],   idle_sway, (1.35f*H)+bob, 0,  0.50f*BK*torsoW, 0.40f*H*torso, 0.32f*BK*torsoW);
    P(&poses[EX_PART_ABDOMEN], 0, (1.02f*H)+bob, 0,  0.34f*BK*torsoW, 0.26f*H*torso, 0.26f*BK*torsoW);
    // head + visor accent
    P(&poses[EX_PART_HEAD],    0, (1.85f*H)+bob, 0,  0.26f*HD, 0.26f*HD, 0.26f*HD);
    P(&poses[EX_PART_VISOR],   0, (1.87f*H)+bob, 0.16f*HD, 0.20f*HD, 0.07f*HD, 0.06f*HD);
    // pauldrons (shoulders) -- broad + raised for a powerful heroic silhouette
    P(&poses[EX_PART_SHOULDER_L], -0.62f*SH, (1.50f*H)+bob, -0.02f, 0.27f*SH, 0.23f, 0.29f*SH);
    P(&poses[EX_PART_SHOULDER_R],  0.62f*SH, (1.50f*H)+bob, -0.02f, 0.27f*SH, 0.23f, 0.29f*SH);
    // forearms swing (counter-phase)
    P(&poses[EX_PART_ARM_L], -0.62f*SH, (1.05f*H)+bob, arz*AR, 0.17f*BK, 0.40f*AR, 0.17f*BK);
    P(&poses[EX_PART_ARM_R],  0.62f*SH, (1.05f*H)+bob, alz*AR, 0.17f*BK, 0.40f*AR, 0.17f*BK);
    // thighs + shins (swing)
    P(&poses[EX_PART_THIGH_L], -0.24f*stance, (0.72f*H)+bob*0.5f, lz*0.5f*LG, 0.20f*BK, 0.32f*LG*thigh, 0.20f*BK);
    P(&poses[EX_PART_THIGH_R],  0.24f*stance, (0.72f*H)+bob*0.5f, rz*0.5f*LG, 0.20f*BK, 0.32f*LG*thigh, 0.20f*BK);
    P(&poses[EX_PART_SHIN_L],  -0.24f*stance, (0.30f*H),          lz*LG,       0.17f*BK, 0.30f*LG*calf, 0.17f*BK);
    P(&poses[EX_PART_SHIN_R],   0.24f*stance, (0.30f*H),          rz*LG,       0.17f*BK, 0.30f*LG*calf, 0.17f*BK);
    // back thruster / crest
    P(&poses[EX_PART_BACK],    0, (1.45f*H)+bob, -0.34f, 0.24f, 0.34f, 0.18f);
    // hands at the ends of the forearms, feet at the base of the shins
    P(&poses[EX_PART_HAND_L], -0.62f*SH, (0.78f*H)+bob, arz*AR, 0.14f*BK, 0.16f*BK, 0.14f*BK);
    P(&poses[EX_PART_HAND_R],  0.62f*SH, (0.78f*H)+bob, alz*AR, 0.14f*BK, 0.16f*BK, 0.14f*BK);
    P(&poses[EX_PART_FOOT_L], -0.24f*stance, (0.08f*H), lz*LG+0.10f, 0.18f*BK, 0.10f*BK, 0.28f*BK);
    P(&poses[EX_PART_FOOT_R],  0.24f*stance, (0.08f*H), rz*LG+0.10f, 0.18f*BK, 0.10f*BK, 0.28f*BK);

    // hunched kinds lean the head + chest forward and drop the stance
    if (hunch > 0.0f) {
      poses[EX_PART_HEAD].offset[2]  += hunch;      poses[EX_PART_HEAD].offset[1] -= hunch*0.5f;
      poses[EX_PART_VISOR].offset[2] += hunch;      poses[EX_PART_VISOR].offset[1] -= hunch*0.5f;
      poses[EX_PART_CHEST].offset[2] += hunch*0.6f; poses[EX_PART_CHEST].offset[1] -= hunch*0.3f;
    }

    // CINEMATIC POSTURE: a forward lean tilting the whole upper body from the waist
    // (aggressive/predatory bots lean in; noble ones stand upright) + a styling hunch
    // that rounds the shoulders forward, layered on any kind-based hunch above.
    { float lr = ex_active_body_style.lean * 0.01745329f;
      float tanl = (lr > -1.5f && lr < 1.5f) ? tanf(lr) : 0.0f;
      float waist = 1.0f*H;
      static const int up[] = { EX_PART_CHEST, EX_PART_HEAD, EX_PART_VISOR, EX_PART_BACK,
                                EX_PART_SHOULDER_L, EX_PART_SHOULDER_R,
                                EX_PART_ARM_L, EX_PART_ARM_R, EX_PART_HAND_L, EX_PART_HAND_R };
      for (unsigned k=0;k<sizeof(up)/sizeof(up[0]);k++){
        float above = poses[up[k]].offset[1] - waist;
        if (above > 0.0f) poses[up[k]].offset[2] += above * tanl;
      }
      float sh = ex_active_body_style.hunch;
      if (sh > 0.0f) {
        poses[EX_PART_SHOULDER_L].offset[2] += sh;      poses[EX_PART_SHOULDER_R].offset[2] += sh;
        poses[EX_PART_ARM_L].offset[2]      += sh*0.6f; poses[EX_PART_ARM_R].offset[2]      += sh*0.6f;
        poses[EX_PART_HEAD].offset[2]       += sh*0.5f; poses[EX_PART_VISOR].offset[2]      += sh*0.5f;
      }
      // VISOR = the face. Narrowing squeezes the eye-band into a menacing slit and
      // pulls it in; the brow angle tips it down (angry) or up (open/noble).
      float nrw = ex_active_body_style.visor_narrow, ang = ex_active_body_style.visor_angle;
      poses[EX_PART_VISOR].sy *= (1.0f - nrw*0.55f);
      poses[EX_PART_VISOR].sx *= (1.0f - nrw*0.18f);
      poses[EX_PART_VISOR].offset[1] += ang*0.004f;
      poses[EX_PART_VISOR].offset[2] += -ang*0.004f;
      // SWAGGER = the walk roll. Heavy bots roll shoulder-to-shoulder as they stride.
      float swag = (ex_active_body_style.swagger - 1.0f) * move_amp * 0.06f * sinf(phase);
      poses[EX_PART_CHEST].offset[0]      += swag;      poses[EX_PART_HEAD].offset[0]  += swag*1.1f;
      poses[EX_PART_SHOULDER_L].offset[0] += swag;      poses[EX_PART_SHOULDER_R].offset[0] += swag;
      poses[EX_PART_VISOR].offset[0]      += swag*1.1f;
    }
    if (kind==EX_BODY_CORRUPTED){
      // genuinely WARPED + asymmetric, ESCALATING BY STAGE: a just-turned bot is
      // only subtly wrong; a fully-overgrown one is the full lopsided horror.
      // sf scales every distortion 0.4 (stage 1) -> 1.0 (stage 3).
      float sf = (bp->corrupt_stage==1)? 0.40f : (bp->corrupt_stage==2)? 0.70f
               : (bp->corrupt_stage>=3)? 1.00f : 0.85f;   // 0/unknown -> mid-heavy default
      #define CZ(x) ((x)*sf)   /* scale a distortion by stage */
      poses[EX_PART_HEAD].offset[0]+=CZ(0.34f)+twitch; poses[EX_PART_HEAD].offset[1]-=CZ(0.16f);  // head lolls aside + too low
      poses[EX_PART_VISOR].offset[0]+=CZ(0.34f)+twitch; poses[EX_PART_VISOR].offset[1]-=CZ(0.16f);
      poses[EX_PART_ARM_R].offset[1]=0.50f+twitch; poses[EX_PART_ARM_R].offset[0]+=CZ(0.14f);
      poses[EX_PART_ARM_R].sx*=1.0f+CZ(0.85f); poses[EX_PART_ARM_R].sy*=1.0f+CZ(1.0f); poses[EX_PART_ARM_R].sz*=1.0f+CZ(0.85f); // bloated dragging arm
      poses[EX_PART_ARM_L].sx*=1.0f-CZ(0.45f); poses[EX_PART_ARM_L].sy*=1.0f+CZ(0.25f); poses[EX_PART_ARM_L].offset[1]+=CZ(0.12f); // withered stump
      poses[EX_PART_SHOULDER_R].sx*=1.0f+CZ(0.7f); poses[EX_PART_SHOULDER_R].sy*=1.0f+CZ(0.7f); poses[EX_PART_SHOULDER_R].offset[1]-=CZ(0.18f); // heavy side sags
      poses[EX_PART_SHOULDER_L].sx*=1.0f-CZ(0.3f); poses[EX_PART_SHOULDER_L].offset[1]+=CZ(0.16f);   // light side hiked -> twisted torso
      poses[EX_PART_THIGH_L].offset[0]-=CZ(0.16f); poses[EX_PART_SHIN_L].offset[0]-=CZ(0.20f);  // legs splay lopsided
      poses[EX_PART_THIGH_R].offset[0]+=CZ(0.08f); poses[EX_PART_SHIN_R].offset[0]+=CZ(0.05f);
      poses[EX_PART_BACK].offset[1]+=0.4f; poses[EX_PART_BACK].offset[0]+=CZ(0.22f);         // growth shoved off to one side
      poses[EX_PART_BACK].sx*=1.0f+CZ(1.0f); poses[EX_PART_BACK].sy*=1.0f+CZ(0.7f);
      #undef CZ
    } else if (kind==EX_BODY_APEX){
      // hulking: massive pauldrons, heavy forward-hanging arms, armour crest
      poses[EX_PART_ARM_L].offset[1]-=0.1f; poses[EX_PART_ARM_R].offset[1]-=0.1f;
      poses[EX_PART_SHOULDER_L].sy*=1.4f; poses[EX_PART_SHOULDER_R].sy*=1.4f;
      poses[EX_PART_BACK].offset[1]+=0.5f; poses[EX_PART_BACK].sx*=2.2f; poses[EX_PART_BACK].sy*=1.8f; poses[EX_PART_BACK].sz*=1.6f;
      poses[EX_PART_CHEST].sx*=1.15f;
    } else if (kind==EX_BODY_HIJACK){
      // INSECTILE swarm bug: raised mantis forelimbs, spindly legs, bulbous sensor
      poses[EX_PART_ARM_L].offset[1]+=0.48f+twitch; poses[EX_PART_ARM_R].offset[1]+=0.48f-twitch;  // forelimbs held high
      poses[EX_PART_ARM_L].offset[0]-=0.26f; poses[EX_PART_ARM_R].offset[0]+=0.26f;                 // splayed wide
      poses[EX_PART_ARM_L].sx*=0.7f; poses[EX_PART_ARM_R].sx*=0.7f;                                 // thin spindly arms
      poses[EX_PART_ARM_L].sz*=0.7f; poses[EX_PART_ARM_R].sz*=0.7f;
      poses[EX_PART_SHIN_L].sx*=0.72f; poses[EX_PART_SHIN_R].sx*=0.72f;                             // thin insect legs
      poses[EX_PART_SHIN_L].sz*=0.72f; poses[EX_PART_SHIN_R].sz*=0.72f;
      poses[EX_PART_THIGH_L].offset[0]-=0.10f; poses[EX_PART_THIGH_R].offset[0]+=0.10f;             // wide scuttle stance
      poses[EX_PART_HEAD].sy*=1.32f; poses[EX_PART_HEAD].sx*=1.18f; poses[EX_PART_HEAD].sz*=1.18f;  // oversized sensor cluster
      poses[EX_PART_HEAD].offset[1]+=0.06f+twitch*0.5f;                                             // craned forward, jittery
    }
  } else {
    // vehicle: tuck into a low wedge; parts become hull panels + wheels
    float roll = sinf(phase)*0.04f*move_amp;
    P(&poses[EX_PART_CHEST],   0, 0.55f, 0.1f, 0.72f*BK, 0.28f, 0.9f);
    P(&poses[EX_PART_ABDOMEN], 0, 0.42f,-0.6f, 0.6f*BK,  0.24f, 0.5f);
    P(&poses[EX_PART_HEAD],    0, 0.66f, 0.7f, 0.34f, 0.20f, 0.4f);   // cab
    P(&poses[EX_PART_VISOR],   0, 0.72f, 0.95f,0.30f, 0.06f, 0.10f);  // windshield
    P(&poses[EX_PART_SHOULDER_L], -0.66f, 0.5f, 0.2f, 0.12f, 0.24f, 0.5f); // fenders
    P(&poses[EX_PART_SHOULDER_R],  0.66f, 0.5f, 0.2f, 0.12f, 0.24f, 0.5f);
    P(&poses[EX_PART_ARM_L], -0.62f, 0.26f+roll, 0.55f, 0.22f,0.22f,0.22f);  // wheels
    P(&poses[EX_PART_ARM_R],  0.62f, 0.26f-roll, 0.55f, 0.22f,0.22f,0.22f);
    P(&poses[EX_PART_THIGH_L],-0.62f, 0.26f-roll,-0.55f, 0.24f,0.24f,0.24f);
    P(&poses[EX_PART_THIGH_R], 0.62f, 0.26f+roll,-0.55f, 0.24f,0.24f,0.24f);
    P(&poses[EX_PART_SHIN_L], -0.4f, 0.5f, -0.9f, 0.1f,0.1f,0.1f);   // exhausts
    P(&poses[EX_PART_SHIN_R],  0.4f, 0.5f, -0.9f, 0.1f,0.1f,0.1f);
    P(&poses[EX_PART_BACK],    0, 0.78f, -0.3f, 0.3f, 0.2f, 0.3f);   // turret hump
    // stow hands/feet as small hull nubs in vehicle mode
    P(&poses[EX_PART_HAND_L], -0.5f, 0.4f, 0.4f, 0.08f, 0.08f, 0.12f);
    P(&poses[EX_PART_HAND_R],  0.5f, 0.4f, 0.4f, 0.08f, 0.08f, 0.12f);
    P(&poses[EX_PART_FOOT_L], -0.3f, 0.2f, -0.7f, 0.10f, 0.08f, 0.14f);
    P(&poses[EX_PART_FOOT_R],  0.3f, 0.2f, -0.7f, 0.10f, 0.08f, 0.14f);
  }

  // size scale + tint (accent parts get the accent colour, brighter)
  // --- ALT-MODE REMNANTS: in robot mode, echo the chosen vehicle so each
  //     transformer reads as distinct (car-former keeps a wheel + fenders, a
  //     flyer keeps a wing shard, a beast-former gets clawed arms + a spine
  //     ridge). Applied to the humanoid silhouette only. ---
  if (kind==EX_BODY_PLAYER && !vehicle && bp) {
    switch (bp->alt_mode) {
      case 0: case 11: /* CAR / CYCLE */
        poses[EX_PART_BACK].sx*=1.4f; poses[EX_PART_BACK].sy*=0.7f; poses[EX_PART_BACK].sz*=1.5f;   // stowed wheel
        poses[EX_PART_SHOULDER_L].sz*=1.5f; poses[EX_PART_SHOULDER_R].sz*=1.5f;                     // fender shoulders
        poses[EX_PART_HEAD].sx*=1.08f; poses[EX_PART_HEAD].sy*=0.92f;                               // compact rounded helm
        break;
      case 2: case 8: case 14: case 17: case 18: case 19: /* JET/GUNSHIP/HAWK/WRAITH/MANTA/BULWARK */
        poses[EX_PART_BACK].offset[1]+=0.25f; poses[EX_PART_BACK].sx*=2.4f; poses[EX_PART_BACK].sy*=0.35f; // folded wing
        poses[EX_PART_SHOULDER_L].sx*=1.3f; poses[EX_PART_SHOULDER_R].sx*=1.3f;
        poses[EX_PART_HEAD].sx*=0.85f; poses[EX_PART_HEAD].sz*=1.25f;                               // sleek aero head
        poses[EX_PART_HEAD].offset[2]+=0.05f;                                                        // swept forward
        break;
      case 6: case 7: case 5: case 12: case 15: /* DINO/RAPTOR/DRAGON/SCORPION/WOLF */
        poses[EX_PART_HAND_L].sz*=1.8f; poses[EX_PART_HAND_R].sz*=1.8f;   // claws
        poses[EX_PART_BACK].sy*=1.4f; poses[EX_PART_BACK].sz*=1.6f;       // spine ridge
        poses[EX_PART_HEAD].sz*=1.15f;                                    // snouted head
        poses[EX_PART_HEAD].offset[2]+=0.04f; poses[EX_PART_HEAD].sy*=0.94f;  // jut forward, lower brow -- predator read
        break;
      case 1: case 9: case 16: /* TANK/MECH/SENTRY -- heavy siege plating */
        poses[EX_PART_SHOULDER_L].sy*=1.5f; poses[EX_PART_SHOULDER_R].sy*=1.5f;
        poses[EX_PART_CHEST].sx*=1.12f; poses[EX_PART_BACK].sx*=1.6f;
        poses[EX_PART_HEAD].sx*=1.25f; poses[EX_PART_HEAD].sy*=0.85f;                               // wide flat siege helm
        break;
      default: break;
    }
  }
  for (int i=0;i<EX_ROBOT_PARTS;i++){
    poses[i].offset[0]*=size_scale; poses[i].offset[1]*=size_scale; poses[i].offset[2]*=size_scale;
    poses[i].scale*=size_scale; poses[i].sx*=size_scale; poses[i].sy*=size_scale; poses[i].sz*=size_scale;
    poses[i].color[0]=base[0]; poses[i].color[1]=base[1]; poses[i].color[2]=base[2];
    // rung-5 weathering (docs/ART_BIBLE.md): deterministic per-panel paint wear.
    // Hash of part slot + alt-mode -> each plate's coat varies a few percent, so
    // the body reads as panels with history, not one molded coat; each alt-mode
    // family wears its own pattern. Subtle by design: 0.94x .. 1.06x.
    {
      uint32_t wh = ((uint32_t)i * 2654435761u) ^ ((uint32_t)(vehicle+1) * 40503u);
      float wear = 0.94f + (float)((wh >> 24) & 0xFF) / 255.0f * 0.12f;
      float wr = poses[i].color[0]*wear, wg = poses[i].color[1]*wear, wb = poses[i].color[2]*wear;
      poses[i].color[0] = (uint8_t)(wr > 255.0f ? 255.0f : wr);
      poses[i].color[1] = (uint8_t)(wg > 255.0f ? 255.0f : wg);
      poses[i].color[2] = (uint8_t)(wb > 255.0f ? 255.0f : wb);
    }
    // two-tone: the lower body (thighs/shins/feet) reads a shade darker on the
    // player, so the paint scheme looks designed rather than a flat coat.
    if (kind==EX_BODY_PLAYER && bp && (i==EX_PART_THIGH_L||i==EX_PART_THIGH_R||
        i==EX_PART_SHIN_L||i==EX_PART_SHIN_R||i==EX_PART_HAND_L||i==EX_PART_HAND_R)){
      poses[i].color[0]=(uint8_t)(base[0]*0.62f); poses[i].color[1]=(uint8_t)(base[1]*0.62f); poses[i].color[2]=(uint8_t)(base[2]*0.62f);
    }
  }
  // accents: visor, back, and a chest crest read in the accent element colour
  int acc[3] = { EX_PART_VISOR, EX_PART_BACK, EX_PART_HEAD };
  for (int k=0;k<3;k++){
    int idx=acc[k];
    poses[idx].color[0]=accent[0]; poses[idx].color[1]=accent[1]; poses[idx].color[2]=accent[2];
  }
}

void ex_robot_layout(ex_part_pose_t poses[EX_ROBOT_PARTS], int vehicle,
                     ex_body_kind kind, float size_scale, float phase,
                     float speed, int element)
{
  ex_build_params_t d = ex_build_default();
  d.accent_element = element;
  ex_robot_layout_ex(poses, vehicle, kind, &d, size_scale, phase, speed, element);
}
