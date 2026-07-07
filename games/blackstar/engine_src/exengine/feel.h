/* feel.h -- ONE source of truth for physical "feel". A handful of intuitive
 * base dials (mass, spring frequency, damping ratio, g-visibility, camera lag,
 * grip, drag, turn) from which every concrete constant is DERIVED through real
 * physical relationships. Tune a base dial and the related values fall in line
 * automatically -- e.g. raising the spring frequency also stiffens the damping
 * (critical-damping relation), and raising mass widens turns, deepens
 * suspension travel and adds camera weight together. All current and future
 * feel-driven systems read these getters instead of hard-coded magic numbers. */
#ifndef EX_FEEL_H
#define EX_FEEL_H

typedef struct {
  float mass;          /* vehicle heft (1 = baseline). Heavier -> more travel,
                          wider turns, more camera weight, softer response. */
  float spring_hz;     /* suspension natural frequency (rad/s). Higher = stiffer. */
  float damp_ratio;    /* 1 = critically damped, <1 bouncy, >1 sluggish. */
  float g_show;        /* how strongly g-forces read (camera shift + body lean). */
  float cam_lag;       /* camera follow time constant (s). Higher = heavier lag. */
  float grip;          /* base lateral grip (1 = locked, <1 slides). */
  float drag;          /* base aero/coast drag. */
  float turn;          /* base turn rate (rad/s) at low speed. */
} ex_feel_t;

/* the live global config (defaults set in feel.c) */
extern ex_feel_t g_feel;

/* ---- derived spring-damper (per unit mass, so accel = these applied to error):
 *   accel = w^2 * (target - x)  -  2*zeta*w * v
 * w and zeta come straight from the dials, so tuning spring_hz retunes damping
 * automatically and the spring stays stable. --------------------------------- */
static inline float ex_feel_w(void)      { return g_feel.spring_hz; }
static inline float ex_feel_w2(void)     { return g_feel.spring_hz * g_feel.spring_hz; }
static inline float ex_feel_2zw(void)    { return 2.0f * g_feel.damp_ratio * g_feel.spring_hz; }

/* suspension travel magnitude under a given g (heavier + more g_show = more). */
static inline float ex_feel_susp_scale(void) { return 0.010f * g_feel.g_show * g_feel.mass; }
/* how far the camera is thrown under g (lateral/longitudinal), and its follow rate. */
static inline float ex_feel_cam_gshift(void) { return 0.020f * g_feel.g_show; }
static inline float ex_feel_cam_rate(void)   { return 1.0f / g_feel.cam_lag; }
/* horizon bank per unit suspension roll (reads the same g_show + mass feel). */
static inline float ex_feel_bank(void)       { return 3.5f * g_feel.g_show; }
/* turn rate at a given speed: base turn, widened by speed and mass, scaled by grip. */
static inline float ex_feel_turn_rate(float speed, float grip) {
  float r = (g_feel.turn / (1.0f + speed * 0.035f * g_feel.mass)) * (0.60f + grip * 0.40f);
  return r < 0.9f ? 0.9f : r;   /* floor so heavy/low-grip vehicles can always steer */
}
/* coast + aero drag scale straight off the drag dial. */
static inline float ex_feel_coast_k(void)    { return 0.7f * g_feel.drag; }
static inline float ex_feel_aero_k(void)     { return 0.016f * g_feel.drag; }

/* Integrate one spring-damper channel by dt using the shared feel constants.
 * pos/vel updated in place toward target. */
static inline void ex_feel_spring(float *pos, float *vel, float target, float dt) {
  float a = ex_feel_w2() * (target - *pos) - ex_feel_2zw() * (*vel);
  *vel += a * dt; *pos += (*vel) * dt;
}

#endif
