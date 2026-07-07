# BIN PATCHES — r594: mob turn radius (+ optional eased-momentum turning)

The pure mechanism is shipped and harness-green in `sim::turning`
(`yaw_rate_cap`, `turn_radius`, `ease_yaw_g_limited`, with tests). This doc is
the **bin wiring**, which does not compile in-sandbox (glfw/winit/render), so
apply + `cargo build` on your end. Tagged **integration-verify**.

Context: the roamer loop in `bin/unendlich_fullgame.rs` currently turns each mob
at a *constant* capped rate:

```rust
// r503: large bodies turn slower (wider arc) — scale the per-tick turn cap
// down by body size so a dragon can't spin on a dime.
let max_step = *turn_rate * dt / (1.0 + axis_bias * 1.5);
cr.yaw = if threat.is_some() || pathing {
    unendlich_engine::sim::awareness::turn_toward(cr.yaw, w.heading, max_step)
} else {
    unendlich_engine::sim::cognition::steering::wander_turn(cr.yaw, w.heading, max_step, *turn_profile)
};
```

The cap is size-scaled (r503 `axis_bias`) but **not speed-scaled**: a mob turns
just as tightly at a sprint as at a crawl. Part A fixes exactly that.

---

## Part A — turn radius (NO struct change, low risk) — DO THIS FIRST

Replace *only* the `max_step` line above. The step distance actually taken this
tick (`dx`,`dz` are already in scope, computed just above by `sighted_step`)
gives the forward speed; feed it to `yaw_rate_cap`. Body size sets both the
sustainable lateral g (bigger ⇒ lower, so it carves wider) and the neck cap.

```rust
// r594: SPEED-COUPLED turn radius. Cap the yaw rate so a fast/large body carves
// a believable arc instead of pivoting on a dime, and a slow one still turns
// crisply. `yaw_rate_cap` is the inverse of `turn_g_force::g_force`:
// slow ⇒ neck-limited (snappy), fast ⇒ g-limited (wide). Composes with the
// r503 axis_bias size term.
let speed = (dx * dx + dz * dz).sqrt() / dt.max(1e-3);   // world units/sec
let max_g    = 0.9 / (1.0 + axis_bias * 2.0);            // big body ⇒ less g ⇒ wider
let neck_cap = *turn_rate / (1.0 + axis_bias * 1.5);     // preserves the r503 pivot cap
let max_step = unendlich_engine::sim::turning::yaw_rate_cap(speed, max_g, neck_cap) * dt;
cr.yaw = if threat.is_some() || pathing {
    unendlich_engine::sim::awareness::turn_toward(cr.yaw, w.heading, max_step)
} else {
    unendlich_engine::sim::cognition::steering::wander_turn(cr.yaw, w.heading, max_step, *turn_profile)
};
```

Notes
- `neck_cap` reproduces the *old* cap exactly at low speed, so standing/slow
  behaviour is unchanged; the only new effect is widening at speed.
- Tune `0.9` (base max-g) and the `2.0` size falloff to taste in playtest; higher
  `max_g` ⇒ tighter cornering allowed. `turn_radius(speed, max_step/dt)` (also in
  `sim::turning`) is handy to print in the debug HUD while tuning.
- The survey/idle branch at the second `wander_turn` site (≈line 8159) can keep
  its constant cap — it's an in-place look-around, not locomotion — or take the
  same treatment with `speed = 0.0` (⇒ neck-capped, identical to now).

---

## Part B — eased-momentum turning (OPTIONAL upgrade; needs one field)

Part A gives radius but still steps at the cap rate. For turns that *accelerate
out of rest and decelerate in* (no constant-velocity snap on state changes), use
`ease_yaw_g_limited`, which carries an angular velocity. This needs a per-mob
`yaw_vel`.

1. **Struct** (`struct Roamer`, ~line 117) — add a field:
   ```rust
   /// r594: carried angular velocity for eased (spring) turning. 0 = at rest.
   yaw_vel: f32,
   ```

2. **Construction literals** — 5 sites: `roamers.push(Roamer { … })` at ~3424,
   ~4254, ~5175, ~7114, and `offspring.push(Roamer { … })` at ~8681. Add to each:
   ```rust
   yaw_vel: 0.0,
   ```
   (The destructure at ~7311 ends in `..`, so it needs no change unless you want
   to read the field — see step 3.)

3. **Destructure** (~line 7311) — to mutate it in the loop, add `yaw_vel,` to the
   named bindings (it's currently absorbed by `..`). Bind it `mut` if your loop
   processes roamers by value and rebuilds; if it holds `&mut Roamer`, write back
   `<roamer>.yaw_vel` after the step instead.

4. **Turn site** — replace the Part-A `turn_toward`/`wander_turn` block with:
   ```rust
   // r594: eased + radius-limited turn. Momentum makes state changes
   // (idle→pursue→flee) read organically instead of snapping angular velocity.
   let (ny, nv) = unendlich_engine::sim::turning::ease_yaw_g_limited(
       cr.yaw, w.heading, yaw_vel, speed, dt, max_g, neck_cap,
   );
   cr.yaw = ny;
   yaw_vel = nv;   // (or <roamer>.yaw_vel = nv; if not destructured by value)
   ```
   `ease_yaw_g_limited` uses `sim::turning::{TURN_STIFFNESS,TURN_DAMPING}`; if a
   species wants a stiffer/looser neck, call `ease_yaw(...)` directly with custom
   constants and `yaw_rate_cap(speed, max_g, neck_cap)` as `max_rate`.

Behavioural note: Part B changes idle wander feel (it no longer uses
`wander_turn`'s detent/nonlinear map). If you want to keep the detented idle
sweep but eased *pursuit*, apply Part B only in the `threat.is_some() || pathing`
branch and leave `wander_turn` for idle.

---

## Players / mounts

The on-foot player yaw is mouse-look (instant) and should stay that way —
easing a first-person camera feels laggy. A **mounted** player, however, should
inherit the mount's turn radius: drive the mount's heading with the same
`ease_yaw_g_limited(mount.yaw, desired, mount.yaw_vel, mount_speed, dt, max_g,
neck_cap)` in the mount-control path (see `sim::mount_stamina`), so a galloping
mount corners wide exactly like a wild roamer. No player-struct change needed
beyond a `yaw_vel` on the mount state.

## Verify (integration tier)
- `cargo build --features render,window --bin unendlich-fullgame`
- `cargo test -p unendlich-engine --features render --lib sim::turning` (the
  primitive's own tests; ground truth over the sandbox `rustc --test`).
- In-game: sprint a roamer/dragon toward a target across open ground — it should
  swing wide, not pivot; walk it slowly — it should still turn crisply.
