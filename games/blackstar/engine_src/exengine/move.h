/* move.h -- the horizontal movement CORE, shared verbatim by the live game and
 * the headless tuning harness. It's pure math (no rendering, no input, no
 * entity), so factoring it out costs nothing at runtime but makes the harness
 * measure the REAL model instead of a copy.
 *
 * Model: target-velocity controller + turn radius (feel) + coast + surface grip
 * (material) + a grip-based DRIFT term so low-grip surfaces/vehicles actually
 * slide through turns (added in response to the harness showing the old model
 * barely distinguished ice from metal). */
#ifndef EX_MOVE_H
#define EX_MOVE_H

#include "feel.h"
#include "material.h"
#include <math.h>

/* Advance a horizontal velocity one step toward the wish direction.
 *   vel        : in/out velocity (xz used; y untouched)
 *   wishx,wishz: desired move direction (need not be normalised; 0 = coast)
 *   max_spd    : top speed for the current mode
 *   accel      : responsiveness (per-vehicle / per-mode)
 *   grip       : base lateral grip (per-vehicle); combined with the surface
 *   mat,mass   : surface material + actor mass (feed the friction model)
 *   in_vehicle : vehicles get turn-radius + drift; on-foot steers freely
 *   foot_area  : stance footprint (1 vehicle, 0.6 robot, 0.4 human) */
static inline void ex_move_horizontal(float vel[3], float wishx, float wishz,
    float max_spd, float accel, float grip, ex_material_id mat, float mass,
    int in_vehicle, float foot_area, float lean, float dt)
{
  float wl = sqrtf(wishx*wishx + wishz*wishz);
  if (wl > 1e-4f){ wishx/=wl; wishz/=wl; }
  float spd = sqrtf(vel[0]*vel[0] + vel[2]*vel[2]);
  float mgrip = ex_material_grip(mat, mass, spd, foot_area, lean);
  grip  *= mgrip;
  accel *= (0.5f + 0.5f*mgrip);

  float tvx = wishx*max_spd, tvz = wishz*max_spd;
  /* turn radius: rotate the VELOCITY VECTOR toward the wish at the turn rate.
   * Doing it directly (not via a chased target) means low-accel/heavy vehicles
   * still steer at their turn rate -- acceleration only governs SPEED. */
  if (in_vehicle && wl > 0.1f && spd > 2.0f){
    float cur = atan2f(vel[2], vel[0]), wsh = atan2f(wishz, wishx);
    float dh = wsh - cur; while(dh> (float)M_PI)dh-=2.0f*(float)M_PI; while(dh<-(float)M_PI)dh+=2.0f*(float)M_PI;
    float mt = ex_feel_turn_rate(spd, grip) * dt;
    if (dh> mt) dh= mt; if (dh<-mt) dh=-mt;
    float ch=cosf(dh), sh=sinf(dh);
    float nvx = vel[0]*ch - vel[2]*sh, nvz = vel[0]*sh + vel[2]*ch;
    vel[0]=nvx; vel[2]=nvz;                 /* heading now advanced by dh */
    tvx = cosf(cur+dh)*max_spd; tvz = sinf(cur+dh)*max_spd;   /* speed target ahead */
  }

  float oldvx = vel[0], oldvz = vel[2];
  float k = fminf(accel*dt, 1.0f);
  if (wl < 0.1f) k = fminf(ex_feel_coast_k()*dt, 1.0f);   /* coast off-throttle */
  vel[0] += (tvx - vel[0])*k;
  vel[2] += (tvz - vel[2])*k;

  /* DRIFT: on a low-grip surface/vehicle, retain some of the pre-step velocity
   * direction so the body SLIDES through the turn instead of railing to the new
   * heading. grip 1 -> no slide; grip ~0.3 (ice) -> pronounced slide. */
  if (in_vehicle && spd > 4.0f){
    float slide = (1.0f - grip); if (slide < 0.0f) slide = 0.0f; if (slide > 0.9f) slide = 0.9f;
    float b = slide * 0.30f;
    vel[0] = vel[0]*(1.0f-b) + oldvx*b;
    vel[2] = vel[2]*(1.0f-b) + oldvz*b;
  }
}

#endif
