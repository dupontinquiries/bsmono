/* chassis.h -- frame/chassis variants for the microFSM-driven customizer. Each
 * frame multiplies the player's build proportions, tunes the transform fold
 * feel, and picks an emission-glow accent -- so two bots with the same sliders
 * still read as different machines. */
#ifndef EX_CHASSIS_H
#define EX_CHASSIS_H

typedef struct {
  const char *name;
  /* proportion multipliers applied on top of the user's sliders */
  float h_mul, bulk_mul, arm_mul, leg_mul, head_mul, shoulder_mul;
  /* transform fold style */
  float fold_stagger;    /* per-part stagger spread (0.35..0.6) */
  float fold_spin;       /* hinge spin magnitude in degrees (140..300) */
  float xform_dur_mul;   /* transform duration multiplier (0.7..1.4) */
  int   greeble_boost;   /* extra greeble detail levels (0..2) */
  float glow;            /* emission accent strength multiplier (0.6..1.6) */
} ex_chassis_info_t;

typedef enum {
  EX_CHASSIS_STANDARD = 0,
  EX_CHASSIS_ANGULAR,     /* sharp, lean, snappy fast fold */
  EX_CHASSIS_JUGGERNAUT,  /* broad, heavy, slow ponderous fold */
  EX_CHASSIS_STRIDER,     /* tall, slim, fluid fold */
  EX_CHASSIS_RAPTOR,      /* lean, long-legged predator: fast fold */
  EX_CHASSIS_BRUISER,     /* short, wide, huge arms: heavy fold */
  EX_CHASSIS_SENTINEL,    /* very tall, spindly watcher */
  EX_CHASSIS_COUNT
} ex_chassis_t;

const ex_chassis_info_t *ex_chassis_info(ex_chassis_t c);

#endif
