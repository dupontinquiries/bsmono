# FEEL — what's a fix and what's just where it started

`docs/UNWIRING.md` already sorted the STRUCTURAL skydribble inheritance
(eighteen map styles, the ball/ring sandbox) into "deletable" versus
"campaign, keep." This is the same sort applied to `physics.rs`, `sim.rs`,
`scale.rs`, and `config.rs` — the numbers and mechanics, not the code
structure — because those files' own comments blur a distinction that
matters a lot for where this project is going.

## The direction

This project is not aiming to feel like Rocket League. It never had a
brief to. It began as skydribble, a Rocket-League-inspired sandbox, and
inherited that sandbox's physics wholesale when `blackstar-rs` grew out of
it (`docs/START_HERE.md`). The actual direction is **a realistic-plus-
Transformers pairing of physics and feel** — closer to how a war machine
that turns into a robot should feel than to how a car chasing a ball
should feel. Those are different briefs, and a lot of code currently
reads as if the RL brief were still the real one.

## The sort

**Keep — these are correctness, not genre fidelity:**
- Contact resolution, integration stability, flip-reset edge cases — bugs
  that would be bugs in any physics model, RL-flavoured or not.
- `gravity.rs`'s tube-profile fix (`true_tube` vs the old inverted
  `engine_tube`) — spin gravity being `a = ω²r`, zero on the axis, is
  physics, not a feel choice, and is now wired into `world.rs`.
- Anything a test in `physics::tests`/`sim::tests`/`gravity::tests` pins
  as a specific, derived-from-physical-law number.

**Malleable — currently RL's numbers/mechanics because that's where this
codebase started, not because matching RL is a goal:**
- `physics.rs`'s whole tuned baseline (100 uu = 1 m, gravity 6.5, boost/
  jump/dodge numbers, ball restitution, the car→ball "touch" impulse) —
  real numbers that produce a real, currently-shipped feel, and every one
  of them is a request for retuning once there's a concrete realistic/
  Transformers feel target to tune against, not a spec to preserve.
- `config.rs`'s "100%" baseline, which is just "whatever `physics.rs`
  currently ships," not "stock RL feel" as a destination.
- `sim.rs`'s whole ball vocabulary — dribble, carry, flick, the six
  hitbox presets — is soccar's own mechanic set. None of it is required
  by "realistic Transformers physics"; it stays exactly as long as this
  project wants a ball-sport layer at all, which is a design call this
  doc does not make.
- `scale.rs`'s RL-derived units (gravity 6.5 m/s², ball radius 0.9325 m)
  as the CONVERSION REFERENCE — already treated as historical by the
  file's own doc ("that is a car, because this began as a car game, and
  the fiction says it is a Transformer"); this doc just makes the same
  point one level up.

## What this doc does NOT do

It does not retune anything. Changing `physics.rs`'s numbers without a
concrete feel target and a way to actually play-test the result (a real
device, a real session) is exactly the "verified blind" mistake this
codebase's own docs warn against elsewhere (`space.rs`, `docs/
PRIORITIES.md`). This is the sort, so the next pass that DOES retune
something knows which numbers are load-bearing correctness and which are
free to move.
