/* robot_parts
  Multi-part transformer bodies with procedural gait animation + a richer,
  more robot-like silhouette and customizable proportions.

  Gait adapted from unendlich pose.rs (phase-driven, speed-scaled, counter-
  phase limbs, body bob). Proportions adapted from the hero-forge BuildParams
  (height, bulk, head, shoulders, arm, leg) so the bot builder can reshape the
  body live. Each body is a set of boxes placed by body-local offset + scale +
  per-part tint; the game applies them to models.

  Pure math (floats), fully testable headless.
*/

#ifndef EX_ROBOT_PARTS_H
#define EX_ROBOT_PARTS_H

/* Cinematic body styling applied on top of the base layout (set per-entity before
 * ex_robot_layout_ex). Neutral defaults = the un-styled body. Fed from the JSON
 * styling layer via ex_gen_style. */
typedef struct {
  float arm_len;       /* arm length x (lithe long / heavy short)      default 1 */
  float leg_len;       /* leg length x                                 default 1 */
  float lean;          /* forward upper-body lean, degrees             default 0 */
  float hunch;         /* shoulder round-forward, 0..0.3               default 0 */
  float visor_narrow;  /* 0 open/friendly .. 1 slit/menacing           default .5 */
  float visor_angle;   /* brow tilt degrees (neg = angry down-brow)    default 0 */
  float idle_restless; /* 0 calm .. 1 twitchy (idle fidget amount)     default .4 */
  float swagger;       /* side-to-side walk roll x                     default 1 */
} ex_body_style_t;
extern ex_body_style_t ex_active_body_style;

#include <stdint.h>

// richer silhouette: chest + abdomen, head + visor, pauldrons, upper/fore
// arms, thighs/shins, back thruster.
typedef enum {
  EX_PART_CHEST = 0,
  EX_PART_ABDOMEN,
  EX_PART_HEAD,
  EX_PART_VISOR,
  EX_PART_SHOULDER_L,
  EX_PART_SHOULDER_R,
  EX_PART_ARM_L,
  EX_PART_ARM_R,
  EX_PART_THIGH_L,
  EX_PART_THIGH_R,
  EX_PART_SHIN_L,
  EX_PART_SHIN_R,
  EX_PART_BACK,
  EX_PART_HAND_L,
  EX_PART_HAND_R,
  EX_PART_FOOT_L,
  EX_PART_FOOT_R,
  EX_ROBOT_PARTS
} ex_part_id;

typedef enum {
  EX_BODY_PLAYER = 0,
  EX_BODY_GRUNT,
  EX_BODY_CORRUPTED,
  EX_BODY_HIJACK,
  EX_BODY_APEX,
  EX_BODY_HUMAN
} ex_body_kind;

// customizable proportions (from the bot builder). 1.0 = default.
typedef struct {
  float height;    // overall vertical scale
  float bulk;      // torso/limb thickness
  float head;      // head size
  float shoulders; // shoulder width + pauldron size
  float arm;       // arm length
  float leg;       // leg length
  int   accent_element; // 0..7 tints the accent parts (visor/crest/thruster)
  int   paint;          // 0..7 primary body paint scheme (0 = element default)
  int   finish;         // 0 matte, 1 metallic, 2 glossy
  int   alt_mode;       // vehicle type index this bot transforms into (for robot-mode remnants)
  int   corrupt_stage;  // 0 = n/a, 1..3 = infection stage (scales Corrupted distortion)
} ex_build_params_t;

static inline ex_build_params_t ex_build_default(void) {
  ex_build_params_t b = { 1.0f,1.0f,1.0f,1.0f,1.0f,1.0f, 1 };
  return b;
}

typedef struct {
  float offset[3];
  float scale;
  float sx, sy, sz;   // non-uniform scale hints (game may use uniform `scale`)
  uint8_t color[3];
  uint8_t shape;      // 0 = default part mesh, 1 = wheel disc (see EX_PSHAPE_*)
} ex_part_pose_t;

#define EX_PSHAPE_DEFAULT 0
#define EX_PSHAPE_WHEEL   1

#define EX_GAIT_CADENCE   0.95f
#define EX_LEG_SWING_MAX  1.05f
#define EX_ARM_SWING_MAX  0.62f
#define EX_BODY_BOB_MAX   0.19f

float ex_gait_advance(float phase, float speed, float dt);

/**
 * [ex_robot_layout fill poses for a body with gait + build params]
 */
void ex_robot_layout_ex(ex_part_pose_t poses[EX_ROBOT_PARTS], int vehicle,
                        ex_body_kind kind, const ex_build_params_t *bp,
                        float size_scale, float phase, float speed, int element);

/* back-compat simple form (default build params) */
void ex_robot_layout(ex_part_pose_t poses[EX_ROBOT_PARTS], int vehicle,
                     ex_body_kind kind, float size_scale, float phase,
                     float speed, int element);

#endif // EX_ROBOT_PARTS_H
