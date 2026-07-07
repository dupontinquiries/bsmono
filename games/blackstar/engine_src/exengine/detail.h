/* detail
  Procedural greeble generator: turns a handful of base body parts into a
  richly detailed, thousand-part surface. For each base part it scatters small
  sub-parts -- plates, vents, rivets, spikes, antennae, panel ridges -- across
  the part's faces, deterministically per seed. The result reads as intricate
  mechanical detail instead of plain blocks.

  The generator can produce well over a thousand greebles; the game renders up
  to a per-frame budget. Pure math (floats + a hash rng), fully testable.
*/

#ifndef EX_DETAIL_H
#define EX_DETAIL_H

#include <stdint.h>

#define EX_DETAIL_MAX 1200

typedef enum {
  EX_GREEBLE_PLATE = 0,  // flat panel on a face
  EX_GREEBLE_RIVET,      // small stud
  EX_GREEBLE_VENT,       // thin slat
  EX_GREEBLE_SPIKE,      // protruding spine
  EX_GREEBLE_ANTENNA,    // thin tall rod
  EX_GREEBLE_RIDGE       // long raised ridge
} ex_greeble_shape;

typedef struct {
  float offset[3];   // body-local center
  float sx, sy, sz;  // half extents
  uint8_t color[3];
  uint8_t shape;
  int   parent;      // which base part it belongs to (for animation)
} ex_greeble_t;

// a base part the greebles attach to (matches robot_parts poses)
typedef struct {
  float offset[3];
  float sx, sy, sz;
  uint8_t color[3];
} ex_detail_part_t;

/**
 * [ex_detail_generate scatter greebles over the base parts]
 * @param out     [output greeble buffer]
 * @param max     [buffer capacity (<= EX_DETAIL_MAX)]
 * @param parts   [base parts to detail]
 * @param n_parts [number of base parts]
 * @param seed    [determinism seed]
 * @param level   [target detail count (density); clamped to max]
 * @param accent  [accent tint applied to some greebles]
 * @return number of greebles produced
 */
int ex_detail_generate(ex_greeble_t *out, int max,
                       const ex_detail_part_t *parts, int n_parts,
                       uint32_t seed, int level, const uint8_t accent[3]);

#endif // EX_DETAIL_H
