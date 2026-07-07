/* pickup
  Floor collectibles that bob/spin/glow and round out the combat loop -- you
  push forward to grab them between waves. Adapted from early-fps pickups.rs
  (health / armor / ammo), themed to Cybertron: energon (health), shield
  (armor), and overcharge (a temporary damage boost). Pure logic + a bob/spin
  animation clock; the game maps each live pickup onto a glowing model.
*/

#ifndef EX_PICKUP_H
#define EX_PICKUP_H

#include <stdint.h>

#define EX_MAX_PICKUPS 22

typedef enum {
  EX_PICKUP_ENERGON = 0,  // heals
  EX_PICKUP_SHIELD,       // temporary armor
  EX_PICKUP_OVERCHARGE,   // temporary damage boost
  EX_PICKUP_MATRIX        // the Matrix of Leadership -- a claimed relic (Rodimus)
} ex_pickup_kind;

typedef struct {
  ex_pickup_kind kind;
  float pos[3];
  float t;        // bob/spin clock
  int   alive;
} ex_pickup_t;

typedef struct {
  ex_pickup_t items[EX_MAX_PICKUPS];
  int count;
} ex_pickup_field_t;

void ex_pickup_field_init(ex_pickup_field_t *f);
int  ex_pickup_add(ex_pickup_field_t *f, ex_pickup_kind kind, float x, float y, float z);
void ex_pickup_update(ex_pickup_field_t *f, float dt);

/**
 * [ex_pickup_try_collect if the player at (px,pz) is close enough to a live
 *  pickup, consume it and return its kind (>=0); else return -1]
 */
int ex_pickup_try_collect(ex_pickup_field_t *f, float px, float py, float pz, float reach);

void ex_pickup_color(ex_pickup_kind kind, uint8_t c[3]);
const char* ex_pickup_name(ex_pickup_kind kind);

/* bob offset for rendering */
float ex_pickup_bob(const ex_pickup_t *p);

#endif // EX_PICKUP_H
