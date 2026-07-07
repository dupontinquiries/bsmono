#include "vehicle.h"
#include <math.h>
#include <string.h>

const char* ex_vehicle_name(ex_vehicle_type t){
  switch(t){
    case EX_VEH_CAR: return "racer"; case EX_VEH_TANK: return "tank";
    case EX_VEH_JET: return "jet";   case EX_VEH_HOVER: return "hover-skiff";
    case EX_VEH_INSECT: return "insect"; case EX_VEH_SPIDER: return "spider";
    case EX_VEH_DRAGON: return "dragon"; case EX_VEH_DINO: return "dino";
    case EX_VEH_RAPTOR: return "raptor";
    case EX_VEH_MECH: return "mech"; case EX_VEH_CYCLE: return "hover-cycle";
    case EX_VEH_GUNSHIP: return "gunship"; case EX_VEH_SCORPION: return "scorpion";
    case EX_VEH_WOLF: return "wolf"; case EX_VEH_HAWK: return "hawk";
    case EX_VEH_CRAB: return "crab"; default: return "?";
  }
}
int ex_vehicle_flies(ex_vehicle_type t){ return t==EX_VEH_JET || t==EX_VEH_HOVER || t==EX_VEH_DRAGON || t==EX_VEH_GUNSHIP || t==EX_VEH_HAWK || t==EX_VEH_CYCLE; }
int ex_vehicle_extra_parts(ex_vehicle_type t){
  switch(t){
    case EX_VEH_INSECT: return 6;  // six legs
    case EX_VEH_SPIDER: return 8;  // eight legs
    case EX_VEH_DRAGON: return 6;  // wings x2, neck, tail, 2 clawed feet
    case EX_VEH_DINO:   return 5;  // 4 legs + tail
    case EX_VEH_RAPTOR: return 3;  // 2 legs + tail
    case EX_VEH_MECH:   return 4;  // 2 legs + 2 arm-cannons
    case EX_VEH_CYCLE:  return 3;  // 2 wheels + fairing
    case EX_VEH_GUNSHIP:return 6;  // 2 wings, 2 rotors, 2 gun pods
    case EX_VEH_SCORPION:return 8; // 6 legs + 2 tail segs
    case EX_VEH_WOLF:   return 5;  // 4 legs + tail
    case EX_VEH_HAWK:   return 4;  // 2 wings, tail, beak
    case EX_VEH_CRAB:   return 6;  // 4 legs + 2 claws
    case EX_VEH_JET:    return 4;  // 2 wings, 2 engines
    case EX_VEH_TANK:   return 2;  // treads
    case EX_VEH_HOVER:  return 3;  // 3 thruster pods
    default:            return 4;  // wheels
  }
}

static void element_tint(int el, uint8_t c[3]){
  switch(el){
    case 1:c[0]=150;c[1]=210;c[2]=245;break; case 2:c[0]=225;c[1]=95;c[2]=45;break;
    case 3:c[0]=200;c[1]=215;c[2]=205;break; case 4:c[0]=235;c[1]=205;c[2]=85;break;
    case 5:c[0]=95;c[1]=165;c[2]=80;break;   case 6:c[0]=175;c[1]=140;c[2]=110;break;
    case 7:c[0]=85;c[1]=150;c[2]=210;break;  default:c[0]=140;c[1]=150;c[2]=165;break;
  }
}
static void P(ex_part_pose_t *p,float x,float y,float z,float sx,float sy,float sz){
  p->offset[0]=x;p->offset[1]=y;p->offset[2]=z; p->sx=sx;p->sy=sy;p->sz=sz; p->scale=(sx+sy+sz)/3.0f;
}
static void tint(ex_part_pose_t *p, const uint8_t c[3]){ p->color[0]=c[0];p->color[1]=c[1];p->color[2]=c[2]; }

void ex_vehicle_layout(ex_part_pose_t *poses, int cap, int *n_out,
                       ex_vehicle_type type, const ex_build_params_t *bp,
                       float size_scale, float phase, float speed,
                       int element, ex_body_kind kind)
{
  memset(poses, 0, sizeof(ex_part_pose_t)*cap);
  ex_build_params_t d = ex_build_default();
  if (!bp) bp = &d;
  float move_amp = fminf(speed/6.0f, 1.0f);
  uint8_t base[3]; element_tint(element, base);
  if (kind==EX_BODY_CORRUPTED||kind==EX_BODY_APEX){ base[0]=120;base[1]=90;base[2]=130; }
  uint8_t accent[3]; element_tint(bp->accent_element? bp->accent_element:element, accent);

  int n = 0;
  // --- core hull (reuse the first parts as a chassis/body/head) ---
  float BK = bp->bulk;
  switch (type) {
    case EX_VEH_DRAGON: case EX_VEH_DINO: case EX_VEH_RAPTOR:
    case EX_VEH_INSECT: case EX_VEH_SPIDER:
    case EX_VEH_MECH: case EX_VEH_SCORPION: case EX_VEH_WOLF:
    case EX_VEH_HAWK: case EX_VEH_CRAB: {
      // creature body: elongated torso + head on a neck + tail base
      P(&poses[n++], 0, 1.0f, 0.2f, 0.55f*BK, 0.48f, 0.78f);         // thorax (less elongated)
      P(&poses[n++], 0, 1.15f, 1.1f, 0.34f, 0.30f, 0.5f);          // head
      P(&poses[n++], 0, 1.2f, 1.45f, 0.28f, 0.10f, 0.22f);         // jaw/mandible base (accent)
      break;
    }
    default: {
      // vehicle body: low chassis + cabin
      P(&poses[n++], 0, 0.5f, 0.1f, 0.72f*BK, 0.28f, 1.0f);        // chassis
      P(&poses[n++], 0, 0.72f, 0.2f, 0.44f, 0.22f, 0.5f);         // cabin
      P(&poses[n++], 0, 0.78f, 0.55f, 0.4f, 0.06f, 0.12f);        // windshield (accent)
      break;
    }
  }

  // --- archetype extras ---
  float step = sinf(phase*4.0f)*0.25f*move_amp;   // leg skitter
  float beat = sinf(phase*6.0f);                  // wing/flap
  float roll = sinf(phase)*0.05f*move_amp;

  switch (type) {
    case EX_VEH_CAR:
      P(&poses[n++], -0.66f,0.26f, 0.6f, 0.22f,0.22f,0.22f);
      P(&poses[n++],  0.66f,0.26f, 0.6f, 0.22f,0.22f,0.22f);
      P(&poses[n++], -0.66f,0.26f,-0.6f, 0.22f,0.22f,0.22f);
      P(&poses[n++],  0.66f,0.26f,-0.6f, 0.22f,0.22f,0.22f);
      break;
    case EX_VEH_TANK:
      P(&poses[n++], -0.7f,0.35f,0, 0.22f,0.35f,1.1f);   // left tread
      P(&poses[n++],  0.7f,0.35f,0, 0.22f,0.35f,1.1f);   // right tread
      break;
    case EX_VEH_JET:
      P(&poses[n++], -1.1f,0.55f,-0.1f, 0.9f,0.05f,0.4f); // left wing
      P(&poses[n++],  1.1f,0.55f,-0.1f, 0.9f,0.05f,0.4f); // right wing
      P(&poses[n++], -0.4f,0.5f,-0.9f, 0.15f,0.15f,0.4f); // engine L
      P(&poses[n++],  0.4f,0.5f,-0.9f, 0.15f,0.15f,0.4f); // engine R
      break;
    case EX_VEH_HOVER:
      P(&poses[n++], -0.7f,0.15f,0.3f, 0.2f,0.12f,0.2f);
      P(&poses[n++],  0.7f,0.15f,0.3f, 0.2f,0.12f,0.2f);
      P(&poses[n++],  0.0f,0.15f,-0.8f, 0.2f,0.12f,0.2f);
      break;
    case EX_VEH_INSECT:
      for (int i=0;i<6;i++){ float sd=(i<3)?-1:1; float fz=((i%3)-1)*0.5f;
        float sw=sinf(phase*5.0f + i)*0.2f*move_amp;
        P(&poses[n++], sd*(0.7f), 0.5f+sw, fz, 0.06f, 0.4f, 0.06f); }
      break;
    case EX_VEH_SPIDER:
      for (int i=0;i<8;i++){ float sd=(i<4)?-1:1; float fz=((i%4)-1.5f)*0.4f;
        float sw=sinf(phase*5.0f + i*0.8f)*0.22f*move_amp;
        P(&poses[n++], sd*(0.8f), 0.55f+sw, fz, 0.05f, 0.45f, 0.05f); }
      break;
    case EX_VEH_DRAGON:
      P(&poses[n++], -1.3f,1.3f,0.0f+beat*0.3f, 1.1f,0.05f,0.6f); // left wing
      P(&poses[n++],  1.3f,1.3f,0.0f-beat*0.3f, 1.1f,0.05f,0.6f); // right wing
      P(&poses[n++], 0,1.0f,-1.4f, 0.2f,0.2f,0.9f);               // tail base
      P(&poses[n++], 0,0.9f,-2.2f + sinf(phase*3.0f)*0.3f, 0.12f,0.12f,0.6f); // tail tip (sways)
      P(&poses[n++], -0.35f,0.35f,0.8f+step, 0.12f,0.35f,0.12f);  // clawed foot L
      P(&poses[n++],  0.35f,0.35f,0.8f-step, 0.12f,0.35f,0.12f);  // clawed foot R
      break;
    case EX_VEH_DINO:
      P(&poses[n++], -0.4f,0.45f,0.6f+step, 0.16f,0.45f,0.16f);   // front legs
      P(&poses[n++],  0.4f,0.45f,0.6f-step, 0.16f,0.45f,0.16f);
      P(&poses[n++], -0.45f,0.5f,-0.6f-step, 0.18f,0.5f,0.18f);   // hind legs
      P(&poses[n++],  0.45f,0.5f,-0.6f+step, 0.18f,0.5f,0.18f);
      P(&poses[n++], 0,0.9f,-1.4f + sinf(phase*3.0f)*0.2f, 0.18f,0.18f,0.8f); // tail
      break;
    case EX_VEH_RAPTOR:
      P(&poses[n++], -0.3f,0.5f,0.1f+step*1.5f, 0.14f,0.5f,0.14f); // legs (fast)
      P(&poses[n++],  0.3f,0.5f,0.1f-step*1.5f, 0.14f,0.5f,0.14f);
      P(&poses[n++], 0,0.9f,-1.2f + sinf(phase*4.0f)*0.3f, 0.14f,0.14f,0.9f); // long tail
      break;
    case EX_VEH_MECH:
      P(&poses[n++], -0.4f,0.5f,0.0f+step, 0.2f,0.55f,0.2f);       // heavy legs
      P(&poses[n++],  0.4f,0.5f,0.0f-step, 0.2f,0.55f,0.2f);
      P(&poses[n++], -0.7f,1.1f,0.3f, 0.14f,0.14f,0.5f);          // arm cannons
      P(&poses[n++],  0.7f,1.1f,0.3f, 0.14f,0.14f,0.5f);
      break;
    case EX_VEH_CYCLE:
      P(&poses[n++], 0,0.3f,0.9f, 0.16f,0.3f,0.16f);              // front wheel
      P(&poses[n++], 0,0.3f,-0.9f, 0.16f,0.3f,0.16f);             // rear wheel
      P(&poses[n++], 0,0.7f,0.4f, 0.3f,0.06f,0.5f);               // fairing
      break;
    case EX_VEH_GUNSHIP:
      P(&poses[n++], -1.3f,0.9f,0, 1.0f,0.05f,0.5f);              // wings
      P(&poses[n++],  1.3f,0.9f,0, 1.0f,0.05f,0.5f);
      P(&poses[n++], -0.9f,1.2f,0, 0.4f,0.04f,0.4f);              // rotors
      P(&poses[n++],  0.9f,1.2f,0, 0.4f,0.04f,0.4f);
      P(&poses[n++], -0.6f,0.6f,0.8f, 0.14f,0.14f,0.5f);         // gun pods
      P(&poses[n++],  0.6f,0.6f,0.8f, 0.14f,0.14f,0.5f);
      break;
    case EX_VEH_SCORPION:
      for(int i=0;i<6;i++){ float sd=(i<3)?-1:1; float fz=((i%3)-1)*0.5f;
        float sw=sinf(phase*5.0f+i)*0.2f*move_amp;
        P(&poses[n++], sd*0.8f, 0.4f+sw, fz, 0.06f,0.35f,0.06f); }
      P(&poses[n++], 0,0.9f,-1.3f, 0.14f,0.14f,0.6f);            // tail base
      P(&poses[n++], 0,1.5f+sinf(phase*3.0f)*0.3f,-1.6f, 0.16f,0.2f,0.2f); // raised stinger
      break;
    case EX_VEH_WOLF:
      P(&poses[n++], -0.35f,0.4f,0.6f+step, 0.13f,0.4f,0.13f);   // legs
      P(&poses[n++],  0.35f,0.4f,0.6f-step, 0.13f,0.4f,0.13f);
      P(&poses[n++], -0.38f,0.42f,-0.6f-step, 0.14f,0.42f,0.14f);
      P(&poses[n++],  0.38f,0.42f,-0.6f+step, 0.14f,0.42f,0.14f);
      P(&poses[n++], 0,0.9f,-1.3f+sinf(phase*4.0f)*0.25f, 0.1f,0.1f,0.6f); // tail
      break;
    case EX_VEH_HAWK:
      P(&poses[n++], -1.4f,1.0f,0+beat*0.4f, 1.2f,0.04f,0.55f);  // broad wings
      P(&poses[n++],  1.4f,1.0f,0-beat*0.4f, 1.2f,0.04f,0.55f);
      P(&poses[n++], 0,0.9f,-1.4f, 0.2f,0.08f,0.6f);            // fanned tail
      P(&poses[n++], 0,1.1f,1.4f, 0.12f,0.12f,0.4f);           // beak
      break;
    case EX_VEH_CRAB:
      for(int i=0;i<4;i++){ float sd=(i<2)?-1:1; float fz=((i%2)*2-1)*0.5f;
        float sw=sinf(phase*5.0f+i)*0.18f*move_amp;
        P(&poses[n++], sd*0.9f, 0.3f+sw, fz, 0.06f,0.3f,0.06f); }
      P(&poses[n++], -0.7f,0.5f,0.8f, 0.2f,0.15f,0.25f);        // claws
      P(&poses[n++],  0.7f,0.5f,0.8f, 0.2f,0.15f,0.25f);
      break;
    default: break;
  }
  if (n > cap) n = cap;

  // tint: body base, windshield/mandible accent (part index 2), extras darker
  for (int i=0;i<n;i++){
    poses[i].offset[0]*=size_scale; poses[i].offset[1]*=size_scale; poses[i].offset[2]*=size_scale;
    poses[i].scale*=size_scale; poses[i].sx*=size_scale; poses[i].sy*=size_scale; poses[i].sz*=size_scale;
    tint(&poses[i], base);
  }
  if (n > 2) tint(&poses[2], accent); // accent the windshield/mandible

  *n_out = n;
}

ex_vehicle_handling_t ex_vehicle_handling(ex_vehicle_type t)
{
  switch (t) {
    case EX_VEH_CAR:     return (ex_vehicle_handling_t){ 1.20f, 7.0f, 0.72f };  /* drifty racer */
    case EX_VEH_TANK:    return (ex_vehicle_handling_t){ 0.68f, 3.2f, 1.00f };  /* heavy, locked */
    case EX_VEH_JET:     return (ex_vehicle_handling_t){ 1.12f, 7.5f, 0.85f };
    case EX_VEH_HOVER:   return (ex_vehicle_handling_t){ 0.95f, 4.5f, 0.45f };  /* floaty slide */
    case EX_VEH_INSECT:  return (ex_vehicle_handling_t){ 0.92f, 7.0f, 0.9f  };
    case EX_VEH_SPIDER:  return (ex_vehicle_handling_t){ 0.85f, 6.0f, 0.95f };
    case EX_VEH_DRAGON:  return (ex_vehicle_handling_t){ 1.10f, 6.0f, 0.8f  };
    case EX_VEH_DINO:    return (ex_vehicle_handling_t){ 0.80f, 3.8f, 1.0f  };
    case EX_VEH_RAPTOR:  return (ex_vehicle_handling_t){ 1.20f, 9.0f, 0.85f };
    case EX_VEH_MECH:    return (ex_vehicle_handling_t){ 0.72f, 3.2f, 1.0f  };
    case EX_VEH_CYCLE:   return (ex_vehicle_handling_t){ 1.35f, 10.0f,0.68f };  /* nimble + slidey */
    case EX_VEH_GUNSHIP: return (ex_vehicle_handling_t){ 0.90f, 4.5f, 0.8f  };
    case EX_VEH_SCORPION:return (ex_vehicle_handling_t){ 0.82f, 5.0f, 0.95f };
    case EX_VEH_WOLF:    return (ex_vehicle_handling_t){ 1.18f, 9.0f, 0.85f };
    case EX_VEH_HAWK:    return (ex_vehicle_handling_t){ 1.26f, 9.0f, 0.82f };
    case EX_VEH_CRAB:    return (ex_vehicle_handling_t){ 0.78f, 5.0f, 1.0f  };
    case EX_VEH_SENTRY:  return (ex_vehicle_handling_t){ 0.20f, 2.0f, 1.0f  };  /* rooted siege */
    case EX_VEH_WHEEL:   return (ex_vehicle_handling_t){ 1.30f, 4.0f, 0.55f };  /* momentum juggernaut */
    case EX_VEH_WRAITH:  return (ex_vehicle_handling_t){ 1.32f, 9.5f, 0.9f  };  /* stealth, nimble */
    case EX_VEH_MANTA:   return (ex_vehicle_handling_t){ 1.20f, 8.0f, 0.75f };  /* gliding ray */
    case EX_VEH_BULWARK: return (ex_vehicle_handling_t){ 0.62f, 3.5f, 0.9f  };  /* armoured flyer */
    default:             return (ex_vehicle_handling_t){ 1.0f,  6.0f, 0.85f };
  }
}
