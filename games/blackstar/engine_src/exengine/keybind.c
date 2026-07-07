#include "keybind.h"
#include <stdint.h>

void ex_keybind_preset(ex_keybind_t *kb, ex_kb_layout_t layout)
{
  if (layout == EX_LAYOUT_DVORAK) {
    // IMPORTANT: SDL scancodes are PHYSICAL key positions, independent of the
    // OS keyboard layout. On a Dvorak layout the ". o e u" inverted-T lives at
    // the physical E / S / D / F positions (physical E types '.', S types 'o',
    // D types 'e', F types 'u'). So the Dvorak preset binds those scancodes --
    // binding SDL_SCANCODE_PERIOD would grab the physical period key, which
    // types 'v' on Dvorak and sits nowhere near the movement hand.
    kb->keys[EX_ACT_FORWARD]   = SDL_SCANCODE_E;      // types '.'
    kb->keys[EX_ACT_LEFT]      = SDL_SCANCODE_S;      // types 'o'
    kb->keys[EX_ACT_BACK]      = SDL_SCANCODE_D;      // types 'e'
    kb->keys[EX_ACT_RIGHT]     = SDL_SCANCODE_F;      // types 'u'
    kb->keys[EX_ACT_JUMP]      = SDL_SCANCODE_SPACE;
    kb->keys[EX_ACT_SPRINT]    = SDL_SCANCODE_LSHIFT;
    kb->keys[EX_ACT_CROUCH]    = SDL_SCANCODE_LCTRL;
    kb->keys[EX_ACT_TRANSFORM] = SDL_SCANCODE_R;      // types 'p', right of '.'
    kb->keys[EX_ACT_FIRE]      = SDL_SCANCODE_G;      // types 'i', inner column
    kb->keys[EX_ACT_RELOAD]    = SDL_SCANCODE_W;      // types ','
  } else {
    kb->keys[EX_ACT_FORWARD]   = SDL_SCANCODE_W;
    kb->keys[EX_ACT_LEFT]      = SDL_SCANCODE_A;
    kb->keys[EX_ACT_BACK]      = SDL_SCANCODE_S;
    kb->keys[EX_ACT_RIGHT]     = SDL_SCANCODE_D;
    kb->keys[EX_ACT_JUMP]      = SDL_SCANCODE_SPACE;
    kb->keys[EX_ACT_SPRINT]    = SDL_SCANCODE_LSHIFT;
    kb->keys[EX_ACT_CROUCH]    = SDL_SCANCODE_C;
    kb->keys[EX_ACT_TRANSFORM] = SDL_SCANCODE_T;
    kb->keys[EX_ACT_FIRE]      = SDL_SCANCODE_G;
    kb->keys[EX_ACT_RELOAD]    = SDL_SCANCODE_R;
  }
}

SDL_Scancode ex_keybind_get(const ex_keybind_t *kb, ex_action_t action)
{
  if (action < 0 || action >= EX_ACT_COUNT)
    return SDL_SCANCODE_UNKNOWN;
  return kb->keys[action];
}

void ex_keybind_set(ex_keybind_t *kb, ex_action_t action, SDL_Scancode key)
{
  if (action >= 0 && action < EX_ACT_COUNT)
    kb->keys[action] = key;
}

const char* ex_action_name(ex_action_t action)
{
  switch (action) {
    case EX_ACT_FORWARD:   return "Forward";
    case EX_ACT_LEFT:      return "Left";
    case EX_ACT_BACK:      return "Back";
    case EX_ACT_RIGHT:     return "Right";
    case EX_ACT_JUMP:      return "Jump";
    case EX_ACT_SPRINT:    return "Sprint";
    case EX_ACT_CROUCH:    return "Crouch";
    case EX_ACT_TRANSFORM: return "Transform";
    case EX_ACT_FIRE:      return "Fire";
    case EX_ACT_RELOAD:    return "Reload";
    default:               return "?";
  }
}
