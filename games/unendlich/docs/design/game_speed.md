# Server Game-Speed (Time Dilation)

> Part of the menu/sim work — see `MENU_SYSTEM.md` for the overview + status.


A server-authoritative speed multiplier (0x–10x) with a logarithmic slider and
a **closest-to-1x** cross-peer reconciliation rule. Model + UI are wired and
tested; binding the reconciled value to the live tick rate is the remaining
sim-integration step.

## Model — `sim/game_speed.rs` (pure, 8 tests)
- `pos_to_multiplier(pos)` — slider position [0,1] → multiplier. `0`→0x
  (freeze), `0.5`→1x (default/normal), `1.0`→10x; logarithmic (each half of
  the track is one decade: 0.1x … 1x … 10x).
- `multiplier_to_pos(mult)` — inverse.
- `DEFAULT_POS = 0.5` (1x).
- `reconcile(prefs) -> f32` — **the rule.** Given every participant's preferred
  multiplier, returns the one **closest to 1x** (distance measured in log space
  so 0.5x and 2x are equally far). So:
  - everyone 10x → 10x; everyone 0x → 0x;
  - mixed (1x + 10x) → 1x — one player at normal pulls the session to normal;
  - a freeze only wins if *everyone* freezes (any running speed beats a lone
    freeze); log-equidistant ties break to the slower (more conservative)
    value. Empty → 1x.

This is deliberately conservative for shared worlds: no one can unilaterally
speed up or freeze the world; the most-normal preference dominates. On a
dedicated host with a single operator preference, that preference just wins.

## UI (wired)
- `WorldRenderer.ui_game_speed_pos: f32` (default `DEFAULT_POS`), mirrored in
  the prepare closure like the other settings.
- A `menu::slider` on the **Gameplay** settings tab, label showing the live
  multiplier (`game speed (1.00x)`), with a note that it's server-authoritative
  and reconciles to closest-to-1x.
- A **per-world settings page** (settings tab 5) reached by a two-column icon
  button (⚙🌐, gear + globe) pushed to the right of the tab bar. It surfaces the
  game-speed + day-length as the world's server-authoritative knobs, and is the
  place future per-world rules (spawn, persistence) gather.

## NOT yet wired (sim integration — the remaining step)
The slider sets a *preference*; nothing yet applies it to the simulation:
1. **Gather peer prefs.** In a session, each participant's `ui_game_speed_pos`
   → multiplier is shipped (a tiny command/field on the existing world-authority
   / netcode path). On a solo/host world the local pref is the only one.
2. **Reconcile per tick.** Call `game_speed::reconcile(&prefs)` to get the live
   multiplier. Determinism: feed prefs in a fixed (sorted-by-NodeId) order so
   every peer computes the same multiplier (same rule as the r-series sorted
   input merges).
3. **Apply to the tick rate.** Scale how many sim steps the server advances per
   wall-second by the multiplier — 0x = advance nothing (freeze), 10x = ten
   steps where one would run. Because the sim is deterministic and step-based,
   this is a *rate* change (how often `step` runs), NOT a dt change inside the
   step — keep the fixed timestep so physics/rollback stay bit-exact; just run
   more/fewer fixed steps per frame. Client replicas follow the confirmed
   authoritative ticks as usual.
4. **Edge cases.** 0x means the world is frozen but the UI/menus still run
   (don't freeze input). Resuming from 0x must not fast-forward — the tick
   clock simply continues from where it paused.

Until step 3 lands, the slider + page are a faithful control surface that
stores and reconciles the value; it just doesn't change world speed yet.
