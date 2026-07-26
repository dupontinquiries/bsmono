# Game-feel / "juice" notes

Playtest feedback (paraphrased): the SYSTEMS are deep on the backend, but the
moment-to-moment PLAYING experience reads as lower quality -- the depth
doesn't surface, and with many layers running it's hard to gauge what's
happening. This doc tracks the feedback-layer work that makes the existing
depth *felt*, not just *present*.

## The principle
Depth the player can't perceive doesn't count as depth to the player. Every
system that fires should produce a legible, satisfying beat: you should FEEL
a hit land, SEE that it connected, and read the consequence instantly.

## Shipped (Update 225)
- **Hitstop** (impact freeze): a heavy hit briefly crunches time (`hitstop()`
  scales the gameplay dt for a few frames while the freeze timer ticks on REAL
  time). A KILL near-freezes (~0.09s), a solid non-killing hit gets a tiny
  bump (~0.05s, throttled with the damage-tick gate so sustained fire doesn't
  stutter). Camera/aim run on real dt above the squeeze, so looking around
  stays responsive during the freeze. This is the single highest-leverage
  game-feel technique -- it's what makes a hit read as "connecting."
- **Hit markers** (shot confirmation): the crosshair flashes from `+` to a
  bright white `X` on a landed hit, and a hot-orange `X` on a kill. Crisp,
  unmistakable "you connected" feedback -- the thing shooters live on and the
  game had none of.

## Honest limits
Both are math/logic-verified (values, timing, throttle) but their on-screen
FEEL is a genuine play-test call -- I can't see or feel the running game.
Hitstop strength (the squeeze amount + duration) and hit-marker flash length
are the tuning knobs most likely to need a pass with eyes on it.

## Next game-feel opportunities (unshipped)
- Floating damage numbers (quantify the hit, surface weapon/mod/element math
  the backend already computes but never shows).
- Screen-edge / directional damage indicators when the player is hit (the
  g_hit_src arrow exists; a full vignette pulse would read faster).
- Kill-specific VFX escalation (a bigger pop on a streak / champion kill).
- Weapon-specific recoil/kick curves so each form FEELS distinct in the hand,
  not just statistically.
- A readability/HUD hierarchy pass: with many systems firing, gate secondary
  messages so the important beat (kill, low-HP, objective) always wins the eye.
