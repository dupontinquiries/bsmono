# DISPATCH PACKET — r334 (6 threads)

Six threads from the operator's list: menus/submenus, gamemode-switch button, world-management
(open-to-DHT), water improvements, water physics, fog + shader. All read
`docs/dispatch_r334/SHARED_RULES.md` FIRST. Base = `unendlich-r334-merged-features-CHECKED.zip`.
Pure cores, no bin/glfw, in-sandbox-validatable; return changed `.rs` + MANIFEST + any PNG/shader-
validation proof. Each carries a FILE ALLOW-LIST; all six edit disjoint files.

**Merge order:** `gamemode → worldmgmt → menus → water-physics → water-visual → fog`
(models first; menus hosts the gamemode/worldmgmt entries; visual/shader last).

---

## T-gamemode — gamemode switch (Survival↔Creative) as a model + button
**WID:** `r334-gamemode`
**Why:** `GameMode{Survival,Creative}` exists only in `sim::command_parse` (text command). The
operator wants to **switch gamemode via a button**. Build the pure model so a button/menu entry can
toggle it and systems can read "what this mode allows".
**Build:** new `sim::game_mode` (`crates/unendlich-engine/src/sim/game_mode.rs`):
- re-export / wrap `command_parse::GameMode`; `GameModeState{ current }` with `toggle()`,
  `set(mode)`, `cycle()`.
- `caps(mode) -> GameModeCaps { creative_inventory, flight, instant_break, invulnerable,
  spawn_legendaries }` — the per-mode capability table (grounds `gamemode_trait_system.extracted.md`).
  Creative ⇒ creative inventory (the legendary palette!) + flight + instant break; Survival ⇒ none.
**Anchors:** `sim/command_parse.rs:33` (`GameMode`), `docs/design/gamemode_trait_system.extracted.md`,
`item::legendary_palette()` (creative shows it when `caps.creative_inventory`).
**Verify:** ≥6 tests (toggle/cycle round-trips; caps differ per mode; creative enables the legendary
palette gate; deterministic). Self-contained → `rustc --test`.
**Bin handoff (`HANDOFF_gamemode_button.md`):** propose the HUD/menu button + keybind that calls
`toggle()`, and where the creative picker reads `caps.creative_inventory`. Propose, don't wire.

---

## T-worldmgmt — world-management settings incl. "Open to DHT" (consumes the MP layer)
**WID:** `r334-worldmgmt`
**Why:** the multiplayer pure layer is in (`session_mode` SessionRole/WorldAdvert/ServerBrowser/
HostFlow, `net::{net_channels, chunk_stream, entity_snapshot, identity, dht_mailbox}`). The operator
wants a **world-management settings page** with an **"Open to DHT"** toggle that flips a running
singleplayer world to a host (DHT off in SP, on when hosting — `MULTIPLAYER_WORLD_JOIN §3`).
**Build:** new `sim::world_management` (`crates/unendlich-engine/src/sim/world_management.rs`):
- `WorldMgmtSettings{ open_to_dht: bool, whitelist_on: bool (default true), max_players: u8,
  world_name: String }` with validated setters (clamp max_players; non-empty name).
- `enum HostTransition { StayLocal, GoHost(WorldAdvert), CloseHost }` +
  `apply_open_to_dht(settings, self_id, role) -> HostTransition` — flipping `open_to_dht` true on a
  `SessionRole::Local` world yields `GoHost` with a `session_mode`-built `WorldAdvert`
  (`make_advert`), flipping false yields `CloseHost`. The DHT bootstrap/announce IO is **bin glue**;
  this decides the transition + builds the advert from the settings.
- `apply_kind`-style note: toggling DHT is a session lifecycle change (not live render) — flag it.
**Anchors:** `sim/session_mode.rs` (`SessionRole`@13, `WorldAdvert`@23, `ServerBrowser`,
`HostFlow`/`make_advert`/`go_live`), `sim/net/identity.rs` (`UnendlichId`, `Whitelist`),
`handoff/MULTIPLAYER_WORLD_JOIN.md §3` (DHT lifecycle), `sim/net_channels.rs` (the host channels).
**Verify:** ≥6 tests (open_to_dht true on Local → GoHost with a well-formed advert; false → CloseHost;
already-host stays; whitelist defaults on; max_players clamps; name validation; deterministic).
`cargo test --features "render-core egui" --lib` or flatten with session_mode mounted.
**Bin handoff (`HANDOFF_worldmgmt.md`):** propose the World-Management settings page (Open to DHT /
Whitelist / Max Players / Name) + the DHT bootstrap+announce loop the `GoHost` transition triggers
(the real-DHT-node step from NEXT_SESSION_GUIDE §3c). Propose, don't wire.

---

## T-menus — nested submenus + the new screens (router/layout scaffolding)
**WID:** `r334-menus`
**Why:** `menu_nav` already has `MenuItem::{action,submenu,back}` + `MenuNav::move_focus`, and
`menu_router`/`menu_screen_layout`/`title_flow` exist, but the new screens (Multiplayer, World
Management, the gamemode entry, the settings sub-pages from r330) aren't composed into a navigable
submenu tree. Build the **pure nav/router scaffolding** that hosts them.
**Build:** extend `sim::menu_router` + `sim::menu_screen_layout` (additive, under a `// r334-menus`
header):
- a submenu tree builder composing the screens into `MenuItem` trees (root → Singleplayer / Host /
  Join / Settings → sub-pages), using the existing `MenuNav` submenu/back machinery.
- `enter_submenu`/`back` routing in `menu_router` so `NavEvent::Activated(submenu)` pushes a screen
  and `Back` pops — a small screen stack. Pure; no rendering.
- placeholder entry hooks for the gamemode toggle (T-gamemode) and the World-Management page
  (T-worldmgmt) so the coordinator wires their models into these slots on merge.
**Anchors:** `sim/menu_nav.rs` (`MenuItem`@34-40, `MenuNav`@107, `move_focus`@113, `NavEvent`@54),
`sim/menu_router.rs`, `sim/menu_screen_layout.rs` (the r330 `camera_gameplay_layout` pattern),
`sim/title_flow.rs` (`Screen` enum), `sim/pause_overlay.rs`.
**Verify:** ≥6 tests (enter pushes a screen + focus resets; back pops to the parent + restores focus;
a deep submenu path round-trips; back at root is a no-op/exit; disabled entries skip; deterministic).
`cargo test --features "render-core egui" --lib` or flatten with menu_nav/title_flow stubs.
**Watch:** ONE `Screen` enum addition max if needed (shared scaffolding — flag it); prefer hosting new
pages as submenu trees over new `Screen` variants. Don't touch the bin overlay.

---

## T-water-physics — buoyancy / swimming / flow forces
**WID:** `r334-water-physics`
**Why:** `sim::water_physics` + `sim::buoyancy` exist but the operator wants water **physics**
improvements. Extend the pure water-physics core: buoyancy by submerged fraction, swim
accel/drag, and directional flow push (rivers/waterfalls).
**Build:** in `sim::water_physics` / `sim::buoyancy` (additive, `// r334-water-physics` header):
- `buoyant_force(submerged_frac01, body_volume, fluid_density) -> f32` (Archimedes; clamp; finite).
- `swim_step(vel, input_dir, in_water01, dt, params) -> vel'` — water drag + swim accel, no NaN.
- `flow_push(flow_vec, in_water01, dt) -> impulse` — current/waterfall lateral force.
- a `WaterPhysicsParams` table (drag, swim accel, buoyancy gain) with sane defaults.
**Anchors:** `sim/water_physics.rs`, `sim/buoyancy.rs`, `sim/water_field.rs`, `sim/collision.rs`/
`physics.rs` (read-only — propose the integration point), `docs/WATER.md`.
**Verify:** ≥6 tests (fully submerged > half-submerged buoyancy; out of water → 0; swim drag bounds
terminal velocity; flow push scales with in_water; params clamp; deterministic). Self-contained →
`rustc --test`. Propose the `physics.rs` integration in `HANDOFF_water_physics.md`.

---

## T-water-visual — depth color / caustics / foam / waves (improvements)
**WID:** `r334-water-visual`
**Why:** water **visual** improvements. The shader-feeding math lives in `sim::water_depth_color`,
`water_caustics`, `foam_threshold`, `gerstner_wave` — tune/extend for better-looking water.
**Build:** in those modules (additive, `// r334-water-visual` header) — e.g. depth-based color ramp
(shallow→deep tint + absorption), shoreline foam by depth+slope, multi-octave Gerstner swell, caustic
intensity from sun angle + depth. Keep each a pure fn the shader/uniform path consumes.
**Anchors:** `sim/water_depth_color.rs`, `sim/water_caustics.rs`, `sim/foam_threshold.rs`,
`sim/gerstner_wave.rs`, `sim/batch_gerstner.rs`, `docs/WATER.md`. If you author/edit a water shader,
it MUST be a NON-off-limits file (NOT `chunk.frag`/`froxel_fill.comp`) and pass `glslangValidator -V`
+ `spirv-val` with the committed `.spv`/`.hash` regenerated.
**Verify:** ≥6 tests (depth ramp monotonic + bounded; foam triggers at the shoreline band only;
Gerstner sums octaves without NaN; caustic intensity 0 at night; deterministic). Self-contained →
`rustc --test`. Optional PNG (a CPU water-surface swatch) per SHARED_RULES §3.
**Watch:** pure math feeding the existing water pass — no new render pipeline. Coordinate with
T-water-physics only via docs (different files: visual vs physics).

---

## T-fog — height fog / atmosphere blending + a validatable shader pass
**WID:** `r334-fog`
**Why:** "fog + shader stuff". The fog math is in `sim::height_fog`, `fog_blend`,
`atmosphere_scatter`, `godray_math`, `scatter_inscatter`. **`froxel_fill.comp`/`chunk.frag` are
COORDINATOR-OWNED — off-limits.** Improve the pure fog blending + (optionally) a non-off-limits shader.
**Build:** in those `sim::` modules (additive, `// r334-fog` header):
- richer height-fog: exponential height falloff + distance blend that matches the froxel volumetrics'
  look (so the cheap per-pixel fog and the froxel fog agree at the seam).
- sun-tinted in-scatter / godray weight from sun angle; a `FogParams` table.
- OPTIONAL shader: if you author a fog/atmosphere pass, it must be a NON-off-limits file (e.g. a new
  `shaders/height_fog.frag` or extend `chunk_gbuffer.frag`), passing `glslangValidator -V` +
  `spirv-val`, `.spv`/`.hash` regenerated. Do NOT touch `chunk.frag`/`froxel_fill.comp`.
**Anchors:** `sim/height_fog.rs`, `sim/fog_blend.rs`, `sim/atmosphere_scatter.rs`,
`sim/godray_math.rs`, `sim/scatter_inscatter.rs`, `docs/` fog/atmosphere notes; `mathx`
(r332) for clamp/lerp/smoothstep — reuse it, don't re-define.
**Verify:** ≥6 tests (fog density monotonic with distance + height; in-scatter 0 at night; godray
weight peaks toward the sun; blend matches froxel at the seam within tolerance; params clamp;
deterministic). Self-contained → `rustc --test`. Ship shader-validation output if you author one.
**Watch:** the froxel parity is the subtle bit — keep the cheap fog consistent with the volumetric
fog the coordinator owns; flag any seam in the handoff.

---

### Coordinator checklist
- [ ] Cut `worker-<WID>-r334.zip` = base + `WORKER_ID` + brief + `SHARED_RULES.md`.
- [ ] On return merge `gamemode → worldmgmt → menus → water-physics → water-visual → fog`; reconcile
      mod lines; wire the proposed bin entries (gamemode button, world-mgmt page + DHT announce loop,
      menu submenu hosting, water physics integration) on-device; roll `unendlich-r335-merged.zip`.
- [ ] worldmgmt's "Open to DHT" is the gate to the real-DHT-node bin work (NEXT_SESSION_GUIDE §3c) —
      it consumes net_channels/chunk_stream/entity_snapshot/session_mode; coordinator continues the
      MP IO glue there.
