/* test_movesim.c -- a headless movement/AI tuning harness.
 *
 * Re-implements the game's movement CORE (target-velocity controller + turn
 * radius + coast/aero drag + material grip + gravity/jump) on top of the shared
 * feel/material/vehicle modules, so scenarios run without a window and report
 * hard METRICS we can tune against (where does the body end up, how far did it
 * drift, did it reach the goal, how long, how much damage taken, etc.).
 *
 * Scenarios cover: drift, flight, run/walk/jump, point-to-point navigation with
 * obstacles, forced collisions, crossfire (damage avoidance), detection, and
 * single vs group runs.
 */
#include "exengine/feel.h"
#include "exengine/material.h"
#include "exengine/vehicle.h"
#include "exengine/move.h"
#include <stdio.h>
#include <math.h>
#include <string.h>
#include <assert.h>

/* ---------------- movement core (mirrors game.c) ---------------- */
typedef struct { float pos[3], vel[3], heading; int grounded; } body_t;

typedef struct { float c[3], h[3]; } aabb_t;   /* obstacle box */

typedef struct {
  float max_spd, accel, grip;   /* from vehicle handling / mode */
  int   flying;
  ex_material_id mat;
} move_params_t;

/* one obstacle-aware collision resolve (push the body out of any box it enters) */
static void resolve(body_t *b, const aabb_t *obs, int nobs, float r, int *hit){
  for (int i=0;i<nobs;i++){
    float dx=b->pos[0]-obs[i].c[0], dz=b->pos[2]-obs[i].c[2];
    float ox=obs[i].h[0]+r-fabsf(dx), oz=obs[i].h[2]+r-fabsf(dz);
    if (ox>0 && oz>0 && fabsf(b->pos[1]-obs[i].c[1])<obs[i].h[1]+1.5f){
      if (ox<oz){ b->pos[0]+=(dx>0?ox:-ox); b->vel[0]*=-0.1f; }
      else      { b->pos[2]+=(dz>0?oz:-oz); b->vel[2]*=-0.1f; }
      if (hit) (*hit)++;
    }
  }
}

/* wishx/wishz: desired move dir (world). jump: 1 to hop this step. */
static void step(body_t *b, float wishx, float wishz, float wishy, int jump,
                 const move_params_t *P, const aabb_t *obs, int nobs, int *hit, float dt){
  ex_move_horizontal(b->vel, wishx, wishz, P->max_spd, P->accel, P->grip, P->mat,
                     g_feel.mass, !P->flying, P->flying?1.0f:0.6f, 0.0f, dt);
  if (P->flying){
    float wl=sqrtf(wishx*wishx+wishz*wishz); float k=fminf(P->accel*dt,1.0f);
    float tvy=(wl>1e-4f?wishy:0.0f)*P->max_spd; b->vel[1]+=(tvy-b->vel[1])*k;
    float s3=sqrtf(b->vel[0]*b->vel[0]+b->vel[1]*b->vel[1]+b->vel[2]*b->vel[2]);
    float d=fminf(ex_feel_aero_k()*s3*dt,0.12f);
    b->vel[0]-=b->vel[0]*d; b->vel[1]-=b->vel[1]*d; b->vel[2]-=b->vel[2]*d;
  } else {
    b->vel[1]-=52.0f*dt;
    if (jump && b->grounded) b->vel[1]=18.0f;
  }
  b->pos[0]+=b->vel[0]*dt; b->pos[1]+=b->vel[1]*dt; b->pos[2]+=b->vel[2]*dt;
  if (b->pos[1]<=0.0f){ b->pos[1]=0.0f; if(b->vel[1]<0)b->vel[1]=0; b->grounded=1; } else b->grounded=0;
  resolve(b, obs, nobs, 1.0f, hit);
}

/* ---------------- metrics helpers ---------------- */
static float dist2d(const float *a,const float *b){ float dx=a[0]-b[0],dz=a[2]-b[2]; return sqrtf(dx*dx+dz*dz); }

/* ================= scenarios ================= */

/* TURN: can a heavy vehicle on a LOW-GRIP surface actually reverse direction?
 * (regression guard -- a too-tight turn radius once locked steering.) Returns
 * seconds to swing the heading 180 degrees, or 999 if it never gets there. */
static float run_turn180(ex_material_id mat, ex_vehicle_type vt){
  ex_vehicle_handling_t vh=ex_vehicle_handling(vt);
  move_params_t P={ 20.0f*vh.speed_mul, vh.accel, vh.grip, 0, mat };
  body_t b={{0,0,0},{0,0,0},0,1};
  for (int i=0;i<180;i++) step(&b,1,0,0,0,&P,NULL,0,NULL,1.0f/60.0f);
  for (int i=0;i<1200;i++){ step(&b,-1,0,0,0,&P,NULL,0,NULL,1.0f/60.0f);
    float ang=atan2f(b.vel[2],b.vel[0]);
    if (fabsf(ang) > 2.9f) return i/60.0f; }
  return 999.0f;
}

/* DRIFT: reach steady speed straight (+x), then command a 90-deg turn (+z) and
 * measure how far the body keeps sliding along the OLD axis, normalised by entry
 * speed so grip/material -- not top speed -- is compared. Higher = slidier. */
static float run_drift(ex_material_id mat, ex_vehicle_type vt){
  ex_vehicle_handling_t vh=ex_vehicle_handling(vt);
  move_params_t P={ 20.0f*vh.speed_mul, vh.accel, vh.grip, 0, mat };
  body_t b={{0,0,0},{0,0,0},0,1};
  for (int i=0;i<180;i++) step(&b,1,0,0,0,&P,NULL,0,NULL,1.0f/60.0f);
  float entry=sqrtf(b.vel[0]*b.vel[0]+b.vel[2]*b.vel[2]); if(entry<1.0f)entry=1.0f;
  float ex_=b.pos[0];
  /* command a +z turn; count OLD-axis (+x) travel until the heading swings 45deg.
   * a tight (grippy) turn covers little +x; a slidey one drifts far in +x. */
  for (int i=0;i<300;i++){ step(&b,0,1,0,0,&P,NULL,0,NULL,1.0f/60.0f);
    float ang=atan2f(b.vel[2],b.vel[0]); if(ang>0.785f) break; }
  return (b.pos[0]-ex_)/entry;
}

/* FLIGHT: full-throttle climb; report max altitude after 4s + terminal speed. */
static void run_flight(float *max_alt, float *term_spd){
  move_params_t P={ 26.0f, 8.0f, 0.85f, 1, EX_MAT_METAL };
  body_t b={{0,10,0},{0,0,0},0,0};
  *max_alt=0;
  for (int i=0;i<240;i++){ step(&b,1,0,1,0,&P,NULL,0,NULL,1.0f/60.0f); if(b.pos[1]>*max_alt)*max_alt=b.pos[1]; }
  *term_spd=sqrtf(b.vel[0]*b.vel[0]+b.vel[1]*b.vel[1]+b.vel[2]*b.vel[2]);
}

/* JUMP: hop from stand; report apex height + horizontal distance while running. */
static void run_jump(float *apex, float *range){
  move_params_t P={ 13.0f, 9.0f, 0.85f, 0, EX_MAT_METAL };
  body_t b={{0,0,0},{0,0,0},0,1};
  for (int i=0;i<60;i++) step(&b,1,0,0,0,&P,NULL,0,NULL,1.0f/60.0f);   /* build run speed */
  float sx=b.pos[0]; *apex=0;
  step(&b,1,0,0,1,&P,NULL,0,NULL,1.0f/60.0f);                          /* jump */
  for (int i=0;i<200 && !(i>3 && b.grounded);i++){ step(&b,1,0,0,0,&P,NULL,0,NULL,1.0f/60.0f); if(b.pos[1]>*apex)*apex=b.pos[1]; }
  *range=b.pos[0]-sx;
}

/* SPEED: time to cover 200 units from a standstill for a given vehicle. */
static float run_speed(ex_vehicle_type vt){
  ex_vehicle_handling_t vh=ex_vehicle_handling(vt);
  move_params_t P={ 20.0f*vh.speed_mul, vh.accel, vh.grip, 0, EX_MAT_METAL };
  body_t b={{0,0,0},{0,0,0},0,1};
  for (int i=0;i<3000;i++){ step(&b,1,0,0,0,&P,NULL,0,NULL,1.0f/60.0f); if(b.pos[0]>=200.0f) return i/60.0f; }
  return 999.0f;
}

/* NAVIGATION: weave to a goal through a slalom of offset obstacles (a momentum
 * body can't turn on a dime, so they're staggered to the sides). Reports arrival,
 * time, path length, gap. */
static int run_nav(float *t_out, float *path_out, float *gap_out){
  move_params_t P={ 18.0f, 7.0f, 0.85f, 0, EX_MAT_ROCK };
  aabb_t obs[3]={ {{50,0,13},{6,10,6}}, {{95,0,-13},{6,10,6}}, {{140,0,13},{6,10,6}} };
  body_t b={{0,0,0},{0,0,0},0,1};
  float goal[3]={190,0,0}; float path=0; float prev[3]={0,0,0};
  int arrived=0; int i;
  for (i=0;i<3600;i++){
    float dx=goal[0]-b.pos[0], dz=goal[2]-b.pos[2];
    float gl=sqrtf(dx*dx+dz*dz); if(gl>1e-3f){dx/=gl;dz/=gl;}
    /* gentle look-ahead avoidance: push away from a box we're about to clip */
    for (int o=0;o<3;o++){ float ox=obs[o].c[0]-b.pos[0], oz=obs[o].c[2]-b.pos[2];
      if (ox>0 && ox<26 && fabsf(oz)<11){ float w=(26.0f-ox)/26.0f; dz += (oz>0?-1.0f:1.0f)*w*0.9f; } }
    float wl=sqrtf(dx*dx+dz*dz); if(wl>1e-3f){dx/=wl;dz/=wl;}
    step(&b,dx,dz,0,0,&P,obs,3,NULL,1.0f/60.0f);
    path+=dist2d(b.pos,prev); prev[0]=b.pos[0];prev[1]=b.pos[1];prev[2]=b.pos[2];
    if (dist2d(b.pos,goal)<6.0f){ arrived=1; break; }
  }
  *t_out=i/60.0f; *path_out=path; *gap_out=dist2d(b.pos,goal);
  return arrived;
}

/* CROSSFIRE / damage avoidance: body runs a lane while a shot sweeps the lane
 * on a fixed cadence; a faster runner spends fewer frames in the danger band. */
static int run_crossfire(float move_mul){
  move_params_t P={ 20.0f*move_mul, 8.0f, 0.85f, 0, EX_MAT_METAL };
  body_t b={{0,0,-60},{0,0,0},0,1};
  int hits=0;
  for (int i=0;i<900;i++){
    step(&b,0,1,0,0,&P,NULL,0,NULL,1.0f/60.0f);   /* run down the lane (+z) */
    /* a kill-zone sweeps to z=0 at t=5s; you're hit if you're still in the band */
    float danger_z = -60.0f + (i/60.0f)*8.0f;      /* the wall of fire advances */
    if (fabsf(b.pos[2]-danger_z) < 2.0f) hits++;    /* caught by the sweep */
    if (b.pos[2] > 60.0f) break;                    /* cleared the lane */
  }
  return hits;
}

/* GROUP: N bodies chase a moving target; per-body speed variance makes packs
 * string out. Report final spread (mean distance from the pack centroid). */
static float run_group(int n){
  body_t b[16]; if(n>16)n=16;
  move_params_t Pp[16];
  for (int i=0;i<n;i++){ b[i].pos[0]=(i%4)*3.0f; b[i].pos[1]=0; b[i].pos[2]=(i/4)*3.0f;
    b[i].vel[0]=b[i].vel[1]=b[i].vel[2]=0; b[i].grounded=1;
    float mul=0.8f+0.4f*(i/(float)(n>1?n-1:1));   /* each unit a bit faster/slower */
    Pp[i]=(move_params_t){ 16.0f*mul, 6.0f, 0.85f, 0, EX_MAT_ROCK }; }
  float tgt[3]={0,0,0};
  for (int f=0; f<900; f++){ tgt[0]=120+40*sinf(f*0.008f); tgt[2]=40*cosf(f*0.008f);
    for (int i=0;i<n;i++){ float dx=tgt[0]-b[i].pos[0], dz=tgt[2]-b[i].pos[2];
      step(&b[i],dx,dz,0,0,&Pp[i],NULL,0,NULL,1.0f/60.0f); } }
  float cx=0,cz=0; for(int i=0;i<n;i++){cx+=b[i].pos[0];cz+=b[i].pos[2];} cx/=n; cz/=n;
  float spread=0; for(int i=0;i<n;i++){ float dx=b[i].pos[0]-cx,dz=b[i].pos[2]-cz; spread+=sqrtf(dx*dx+dz*dz);} 
  return spread/n;
}

#define CHECK(cond,msg) do{ if(!(cond)){ printf("  FAIL: %s\n",msg); fails++; } }while(0)

int main(void){
  int fails=0;
  printf("=== movement sim: drift/flight/jump/speed/nav/crossfire/group ===\n\n");

  float d_metal=run_drift(EX_MAT_METAL, EX_VEH_CAR);
  float d_ice  =run_drift(EX_MAT_ICE,   EX_VEH_CAR);
  float d_tank =run_drift(EX_MAT_METAL, EX_VEH_TANK);
  float d_hover=run_drift(EX_MAT_METAL, EX_VEH_HOVER);
  printf("DRIFT (slide/entry-spd):  car/metal %.2f   car/ice %.2f   tank/metal %.2f   hover/metal %.2f\n",
         d_metal,d_ice,d_tank,d_hover);
  CHECK(d_ice > d_metal, "ice should slide more than metal (material affects grip)");
  CHECK(d_hover > d_tank, "low-grip hover should drift more than a locked tank");
  { /* regression guard: heavy titan-like vehicles must be able to REVERSE on
       low-grip ground (the dino-on-sand bug). */
    float t_dino_sand=run_turn180(EX_MAT_SAND, EX_VEH_DINO);
    float t_tank_ice =run_turn180(EX_MAT_ICE,  EX_VEH_TANK);
    printf("TURN 180 (s):    dino/sand %.2f   tank/ice %.2f\n", t_dino_sand, t_tank_ice);
    CHECK(t_dino_sand < 5.0f, "a dino on sand must be able to turn around");
    CHECK(t_tank_ice  < 6.0f, "a tank on ice must be able to turn around");
  }
  { const ex_material_id mats[]={EX_MAT_METAL,EX_MAT_ROCK,EX_MAT_ICE,EX_MAT_SAND,EX_MAT_ORGANIC,EX_MAT_CRYSTAL};
    const char *mn[]={"metal","rock","ice","sand","organ","cryst"};
    const ex_vehicle_type vts[]={EX_VEH_CAR,EX_VEH_TANK,EX_VEH_CYCLE,EX_VEH_HOVER,EX_VEH_WOLF};
    const char *vn[]={"car","tank","cycle","hover","wolf"};
    printf("\n  DRIFT SWEEP (slide/entry-spd)\n            ");
    for (int m=0;m<6;m++) printf("%6s", mn[m]); printf("\n");
    for (int v=0; v<5; v++){ printf("  %-8s", vn[v]);
      for (int m=0;m<6;m++) printf("%6.2f", run_drift(mats[m], vts[v]));
      printf("\n"); }
    printf("\n  SPEED to 200u (s)  ");
    for (int v=0; v<5; v++) printf("%8s", vn[v]); printf("\n                     ");
    for (int v=0; v<5; v++) printf("%8.2f", run_speed(vts[v])); printf("\n\n");
  }

  float malt,tspd; run_flight(&malt,&tspd);
  printf("FLIGHT:          max alt %.1fu   terminal spd %.1f u/s\n", malt, tspd);
  CHECK(malt > 15.0f, "flight should climb");
  CHECK(tspd < 45.0f, "aero drag should cap flight speed");

  float apex,range; run_jump(&apex,&range);
  printf("JUMP:            apex %.2fu   run-jump range %.1fu\n", apex, range);
  CHECK(apex > 1.0f && apex < 12.0f, "jump apex in a sane band");
  CHECK(range > 2.0f, "a running jump covers ground");

  float s_cycle=run_speed(EX_VEH_CYCLE), s_tank=run_speed(EX_VEH_TANK);
  printf("SPEED to 200u:   cycle %.2fs   tank %.2fs\n", s_cycle, s_tank);
  CHECK(s_cycle < s_tank, "a cycle should out-accelerate a tank");

  float nt,np,ng; int ok=run_nav(&nt,&np,&ng);
  printf("NAV(obstacles):  arrived=%d   time %.1fs   path %.0fu   gap %.1fu\n", ok,nt,np,ng);
  CHECK(ok==1, "should reach the goal around obstacles");

  int h_slow=run_crossfire(0.5f), h_fast=run_crossfire(1.3f);
  printf("CROSSFIRE hits:  slow %d   fast %d\n", h_slow, h_fast);
  CHECK(h_fast <= h_slow, "a faster runner should take <= hits crossing the sweep");

  float sp4=run_group(4), sp12=run_group(12);
  printf("GROUP spread(u): 4-pack %.1f   12-pack %.1f  (started ~3u)\n", sp4, sp12);
  CHECK(sp12 > 6.0f && sp4 > 6.0f, "speed-varied packs string out from their tight start");

  printf("\n%s\n", fails==0 ? "ALL ASSERTIONS PASSED" : "SOME CHECKS FAILED");
  return fails==0 ? 0 : 1;
}
