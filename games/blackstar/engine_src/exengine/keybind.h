/* keybind
  Remappable action -> key binding system, ported in spirit from
  early-fps's Bindings design: actions are abstract (Forward, Transform,
  Fire...), keys are concrete SDL scancodes, and presets (QWERTY / Dvorak)
  seed the defaults. The game queries by ACTION, never by raw scancode,
  so rebinding is a one-line set() and the input code never changes.

  Extends the original with the Cybertron-specific actions (Transform,
  Fire) alongside the movement set.
*/

#ifndef EX_KEYBIND_H
#define EX_KEYBIND_H

#include <SDL2/SDL_scancode.h>

typedef enum {
  EX_ACT_FORWARD,
  EX_ACT_LEFT,
  EX_ACT_BACK,
  EX_ACT_RIGHT,
  EX_ACT_JUMP,
  EX_ACT_SPRINT,
  EX_ACT_CROUCH,
  EX_ACT_TRANSFORM,   // Cybertron: robot <-> vehicle
  EX_ACT_FIRE,        // Cybertron: shoot / damage a chunk
  EX_ACT_RELOAD,
  EX_ACT_COUNT
} ex_action_t;

typedef enum {
  EX_LAYOUT_QWERTY,
  EX_LAYOUT_DVORAK
} ex_kb_layout_t;

typedef struct {
  SDL_Scancode keys[EX_ACT_COUNT];
} ex_keybind_t;

/**
 * [ex_keybind_preset fill a binding set from a keyboard-layout preset]
 * Dvorak mirrors the QWERTY inverted-T by physical key position, matching
 * the early-fps design (forward on the key above the home-row left cluster).
 */
void ex_keybind_preset(ex_keybind_t *kb, ex_kb_layout_t layout);

/**
 * [ex_keybind_get resolve an action to its bound scancode]
 */
SDL_Scancode ex_keybind_get(const ex_keybind_t *kb, ex_action_t action);

/**
 * [ex_keybind_set rebind an action to a new scancode]
 */
void ex_keybind_set(ex_keybind_t *kb, ex_action_t action, SDL_Scancode key);

/**
 * [ex_keybind_down is the key for this action currently held?]
 * @param keys_down [the engine's ex_keys_down array]
 */
static inline int ex_keybind_down(const ex_keybind_t *kb, ex_action_t action, const uint8_t *keys_down)
{
  return keys_down[ex_keybind_get(kb, action)];
}

/**
 * [ex_action_name human-readable action label for a settings UI]
 */
const char* ex_action_name(ex_action_t action);

#endif // EX_KEYBIND_H
