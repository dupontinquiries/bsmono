# Next-session handoff — multiplayer, the bug/perf backlog, and how to work here

You are the next Claude picking up **unendlich** (Rust voxel/dragon Vulkan engine,
~1031 modules, 7-crate workspace). This guide is the orientation the operator asked
me to leave you. Read it fully before touching code. It is long on purpose.

Canonical tree: `/home/claude/unendlich`. Pristine base: `/tmp/orig/unendlich`.
Prior-work catalog: `/mnt/transcripts/journal.txt` (transcripts r1..r329). Feature
ledger: `docs/WIRING_STATUS.md` (read the last ~8 `## rNNN` entries first).

---

## 1. Philosophy — how this project is worked

**The operator drives, you build, they validate on real hardware.** They run every
build on an Apple **M2 (Asahi/Honeykrisp, FP32 GPU path), trackpad**, and paste back
logs/screenshots/profiles. You are on Linux x86_64 with **no discrete GPU**. So:

- **You write Rust/GLSL partly blind.** You cannot see visuals, feel input latency,
  or measure real-GPU perf. Be honest about that line at all times.
- **Layered "sandwich" abstractions.** Keep a pure, backend-neutral core that is
  unit-testable, with thin adapters at the edges (the input layer is the model:
  `input_codes::{Keycode,MouseBtn,InputEvent}` in the core; glfw only in the bin's
  event loop + a feature-gated adapter). The operator explicitly values this.
- **Build testable pure-logic modules.** Net identity/mailbox, runtime_config,
  safe_tune, keybindings — all pure, all unit-tested. When the in-tree test binary
  can't link (see §2), flatten the module(s) into a standalone `rustc --test` file
  and run that. The two-sided DHT mailbox was verified this way: 12/12.
- **Diagnose risky work; don't rush it blind.** GPU/terrain/network changes get a
  written root-cause + a sequenced plan, not a blind patch labeled done. The terrain
  FP32/FP64 bug (§4) was diagnosed across two releases before anyone touched it.
- **Honesty suffix on every release** (full-tree zip, `present_files` it):
  `-CHECKED` = compiles / inert, `-UNVERIFIED` = behavior/visual/perf not run,
  `-VERIFIED` = logic actually executed (harness or, now, lavapipe). Never call
  something VERIFIED you didn't run.
- **The operator sends big multi-part batches** and likes game-themed touches
  (preset glyphs 🔋🌱🌲⛰🐉, etc.) and clean, testable code. Match that energy, but
  sequence honestly — deferring deep work with a clear plan is welcomed, not penalized.

---

## 2. Build / environment setup — do this FIRST, every session (the FS resets)

The sandbox filesystem resets between sessions. None of the installs below persist.
Run them at the start.

### 2a. Rust toolchain
Rust **1.96.0** is preinstalled at `/usr/local/bin/{rustc,cargo}` (a fixed install,
**no rustup**, no `rust-toolchain.toml`). Usually it's already there — check with
`rustc --version`. If a future image lacks it, install via rustup
(`curl --proto '=https' https://sh.rustup.rs -sSf | sh`) — `static.rust-lang.org`/
`sh.rustup.rs` may need allowlisting; the workspace builds on stable.

### 2b. Faster builds (mold + split-debuginfo) — see `docs/TOOLING_fast_linking.md`
Run `scripts/sandbox-fastbuild.sh` once. It installs mold+lld (apt; the ubuntu
mirror is allowlisted) and writes a **sandbox-only** `$CARGO_HOME/config.toml` with
`-fuse-ld=mold -C split-debuginfo=unpacked`. This speeds **linking** (`cargo test`,
`cargo build`) — **not** `cargo check`, which never links. `prefer-dynamic` is the
big size/link lever (~26× smaller test binaries) but the binary then needs
`libstd.so` at runtime; use it only for the test/run loop, never ship it.
**Never** put these flags in the repo's `.cargo/config.toml` — they'd break the
operator's M2 (no mold there; prefer-dynamic isn't portable).

### 2c. ⭐ llvmpipe / lavapipe — run the GPU paths IN SOFTWARE (the big unlock)
This sandbox has **no GPU**, but it has the Vulkan loader (`libvulkan.so.1`) **and a
software Vulkan ICD already registered**: `lvp_icd.json` = **lavapipe (llvmpipe)**.
Confirmed working: it enumerates as a `PHYSICAL_DEVICE_TYPE_CPU`, *llvmpipe (LLVM
20.1.2)*, Vulkan **1.4**. The engine loads Vulkan via `ash::Entry::load()`
(`render/context.rs`), so it will pick up lavapipe.

This potentially lets you **run and validate GPU paths that previous sessions could
only `cargo check`** — terrain FP32 generation, the octree GPU mesher, render passes
— without the M2. This is the single most valuable thing to try early.

Setup + smoke test:
```bash
apt-get install -y --no-install-recommends vulkan-tools mesa-vulkan-drivers >/dev/null 2>&1
mkdir -p /tmp/xdg && chmod 700 /tmp/xdg
export XDG_RUNTIME_DIR=/tmp/xdg
export VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json     # force lavapipe
export VK_ICD_FILENAMES=$VK_DRIVER_FILES                        # older loaders
vulkaninfo --summary | grep -i devicename     # -> llvmpipe (LLVM 20.1.2)
```
Then the purpose-built headless probe (its `required-features` are already correct):
```bash
cargo run --bin gpu-headless-probe --features headless     # render-core+gpu, NO window/glfw
```
Other headless entry points worth using: `examples/headless_run.rs`,
`examples/dht_join_probe.rs` (multiplayer!), and the many image-emitting examples
(`flora_patch_scene`, `cave_slice`, `flythrough_snapshot`, …) — these can produce
PNGs you can inspect, i.e. you may be able to *see* the grass/terrain bugs yourself.

**Honest caveats** (don't over-promise to the operator):
- lavapipe is CPU-slow; keep scenes/chunk counts tiny.
- It doesn't implement every extension/feature; expect some `VK_ERROR_FEATURE_NOT_PRESENT`
  / format gaps. Validate incrementally; a failure may be a lavapipe gap, not an
  engine bug.
- Feature plumbing bites: the `vk-harness` crate's own test pulls `unendlich-engine`
  **without** `gpu`/`render-core`, so it fails to compile until those features are
  threaded through. Prefer `gpu-headless-probe --features headless` as your first
  real run; fix harness feature wiring only if you need that harness.
- A green lavapipe run is still **software** — call it VERIFIED-on-llvmpipe, and note
  the operator should confirm on the M2 (different driver, FP behavior may differ —
  which is the whole terrain bug, see §4).

### 2d. The verify gates (in order of cost)
1. **Inner loop (no link):**
   `cargo check --bin unendlich-fullgame --features render,window,gpu,ui-overlay,audio`
   (~55–70s with a warm cache). This is the standard gate.
2. **Pure-logic tests (NEW since r328 — no glfw):**
   `cargo test --lib --features "render-core egui"` — links and runs `input::`,
   `keybindings::`, `net::identity`, `net::dht_mailbox`, `runtime_config`,
   `safe_tune`, etc. Keep this green; it's the regression net for the testable core.
3. **Shaders:** `glslangValidator --target-env vulkan1.2 <file.vert|.frag|.comp>`.
4. **Runtime (when feasible):** lavapipe headless (§2c) or a flattened
   `rustc --test` harness for a pure module.

### 2e. Release convention
Full-tree zip excluding build/junk, then `present_files`:
```bash
OUT=/mnt/user-data/outputs/unendlich-rNNN-<slug>-<SUFFIX>.zip
cd /home/claude/unendlich && zip -rq "$OUT" . -x "target/*" -x "*/target/*" -x ".git/*" -x "*.zip"
```
Add a dated `## rNNN` entry to `docs/WIRING_STATUS.md` for every release.

---

## 3. Multiplayer — set it up (the main task)

### 3a. The operator's design (the spec to honor)
- **unendlich-id**: the shareable handle you give others; via the network it locates
  your IP. It does **not** contain the IP. Others add your id to their **subscribers**
  list and your world's server appears for them.
- **Servers are tied to worlds/saves**; you host your world as a server **from the
  in-game menu**.
- **Whitelisting is in Settings once multiplayer is on, and ON by default**, because
  it protects your public IP via a **mailbox request system**: a joiner's request is
  held until you accept; the **IP is not sent to a requester until accepted**.
- Servers **approve names or rename joiners** (with a graphic), backed by per-world
  `whitelist.txt` + `names.txt`.
- Each instance also has **local/splitscreen players**, so whitelists **can't be
  global** — they're scoped to whichever local world is running.

### 3b. What already exists (read these before writing anything)
- `crates/unendlich-engine/src/sim/net/identity.rs` (r326, tested): `UnendlichId`
  (12-char, `XXXX-XXXX-XXXX`, `from_seed`), `Whitelist` (per-world `whitelist.txt`),
  `NameRegistry` (`names.txt`, `approve()->(name,was_renamed)` — the rename graphic),
  `Mailbox`+`JoinRequest`+`ReqState`, `Subscribers` (per-instance). IP-withholding
  invariant: `JoinRequest::address_if_accepted()` is `None` until accept.
- `crates/unendlich-engine/src/sim/net/dht_mailbox.rs` (r327, **VERIFIED 12/12**):
  two-sided serverless rendezvous. Each id owns a mailbox at DHT key `H(id)`.
  `send_request` → host's box; host `host_drain_requests` (auto-accepts whitelisted)
  / `host_accept` / `host_deny` → requester's box; `poll_response` → `Accepted{addr}`
  or `Denied`. `MailboxDht` trait + in-memory `LocalDht` (mirrors the
  `Transport`/`LoopTransport` split). Codec is hand-rolled + bounds-checked.
- `sim/net/session.rs` (rollback netcode), `transport.rs` (`UdpTransport` +
  `LoopTransport`), `wire.rs` (packet codec).
- `examples/dht_join_probe.rs` — a join probe you can actually run.

### 3c. Next steps, sequenced
1. **Real DHT node** implementing `MailboxDht` over `UdpTransport` (Kademlia/
   mainline-style: routing table, iterative lookup, `put`/`get` at `H(id)`), plus
   **STUN** to discover your own public `addr` for the `Accept`. Keep `LocalDht` as
   the test backend. The node itself can't be runtime-tested in-sandbox (no real
   network) — structure it behind the trait, unit-test the routing/codec with a
   simulated transport, and loopback-test two nodes in one process.
2. **Hardening (do NOT skip before shipping real networking):** mailboxes are
   world-writable, so **sign** every `MailMsg` with the sender's id-key, and **seal**
   the `Accept`'s `addr` to the requester's public key so relaying DHT nodes can't
   harvest IPs. This means **`UnendlichId` should carry a public key** — plan that
   into the id format now.
3. **Join → Session handoff:** when `poll_response` returns `Accepted{addr}`, connect
   `UdpTransport` to `addr` and start `session.rs` rollback. Wire the host side to
   spin up a session listener when hosting-from-menu.
4. **UI:** host-from-in-game-menu; server/subscribers list (added ids' worlds);
   a Whitelist settings page (visible only when multiplayer is on) with pending
   **approve/deny** + the **rename graphic**; add-by-unendlich-id. Match the glassy
   dropdown style (`/home/claude/ui-pkg/ui-package/` mockups; accent `#6fc3ff`).
   Menu UI lives in `render/world_renderer.rs` (~3000–4350; `settings_tab` match
   ~3648; staged `ui_*` fields pattern at ~450/528, init ~1463, stage ~3033,
   writeback ~4333).

Build the protocol pure + behind traits so it stays testable; leave true wire-level
behavior for loopback + the operator's two-machine test.

---

## 4. Bugs & performance gripes (locations + status + the lavapipe angle)

### 4a. ⭐ Terrain FP32/FP64 surface mismatch (highest-value bug)
**Symptom:** plants/mobs float in air; water sits below sea level; trees spawn on
air. **Root cause (diagnosed r324, not yet fixed):** the GPU compute generator fills
voxels in **FP32** (ported for Honeykrisp/Apple), but
`world/chunk_manager.rs::surface_y_at` (line ~1831) returns the **CPU FP64** noise
*prediction* (`self.generator.cpu().surface_y_at`). Flora placement, mob ground/
physics, and spawning all use that prediction, so FP32-generated terrain ≠ predicted
surface → things float, water fills below `SEA_LEVEL=64` (`world/constants.rs:23`).
Generators: `terrain_generator.rs` wraps `cpu_generator.rs` (FP64) + `gpu_generator.rs`
(FP32). **Fix directions (operator chooses/validates):** (a) spawning reads the
**actual top solid block** of the generated chunk, or (b) close CPU/GPU FP parity.
**NEW:** lavapipe (§2c) may let you **reproduce this in-sandbox** for the first time —
generate a chunk on the FP32 GPU path headless and compare to `surface_y_at`. That
turns a blind fix into a measured one. Try it.

### 4b. Grass/foliage sway
The wind shader sways by **green vertex color**, so it also catches solid grass-block
tops and can't anchor plant bases. `shaders/chunk.vert` (~88), `chunk.frag`
(`is_foliage`). Amplitude already tamed (r323/r324: `time_of_day` is seconds, not
0..1 — don't reintroduce the fast-wave bug). **Proper fix:** the **GPU octree mesher**
tags sway-eligible foliage vertices (a mesher change, since the operator runs
`GPU_MESHER=2`). Inspect headless renders to see it.

### 4c. Dragon body-warp animation
Head/wings balloon then shrink during animation. Look at `sim/dragon_forge.rs` (body
build) + the animation/pose path. Not yet diagnosed — start by rendering a dragon
headless and stepping the animation.

### 4d. Stutters (diagnosed r319, partially addressed)
`stutter1` = GPU streaming storm from fast vertical movement (upload bursts);
`stutter2` = roamer **overpopulation** (saw 73 vs cap 8) + flora scan cost. Revisit
the roamer cap enforcement and the upload scheduler (`runtime_config.rs` has
`max_uploads_per_frame`, `upload_budget_ms`; many perf flags default ON via
`runtime_flags`).

### 4e. Smaller items
- Leaf bushy genome + leaf-to-log adjacency: `world/plant_lifecycle.rs`,
  `plant_blueprint` (crown shape). Floating-trunk decay logic is in `leaf_decay.rs`.
- **Crawl**: `GameAction::Crawl` + per-character collider height — touches ~16
  `Character` construction sites; needs a real collider-height refactor, not a hack.
- Settings: wire the new graphics knobs **live** (render_scale/neural-upscale, shadow
  8192/16384, dynamic lights) via render-pipeline migration; currently only
  render-distance applies live (others are config-only). New Camera/Gameplay page
  (FOV foundation in `sim/settings_model.rs::Fov`).
- Doc numeric audits captured but `.docx` not rewritten: Null Crab gravity (should be
  ~30–50% normal, cave gravity near-zero/slightly-negative), Moonstone falloff
  (carrier ~60%, area 10–15%). Docs in `/home/claude/new-docs/`.

---

## 5. What NOT to do (hard-won)

- **Don't** commit `mold`/`prefer-dynamic`/`-fuse-ld` to the repo's
  `.cargo/config.toml`. It breaks the operator's M2 (no mold; non-portable binaries).
  Sandbox `$CARGO_HOME` only.
- **Don't** toggle `RUSTFLAGS` to compare linkers — it invalidates cargo's whole
  fingerprint and forces a **full rebuild** each time (overran a turn's budget). Set
  the linker once and leave it; to time just a link, delete the final binary and
  re-run with `RUSTFLAGS` unchanged.
- **Don't** ship networking/GPU/terrain code written blind and call it VERIFIED.
  Diagnose, put pure logic behind a trait, unit-test it, label `UNVERIFIED`, and —
  now — *try lavapipe* before declaring a GPU thing untestable.
- **Don't** reintroduce glfw (or any backend) into the core. Input flows through
  `input_codes` (`InputEvent`/`Keycode`/`MouseBtn`); glfw lives only in the bin event
  loop + the `#[cfg(feature="glfw")]` adapter. New input code must stay backend-
  neutral or you re-break `cargo test --lib`. (`input`/`input_capture`/`keybind_ui`/
  `player` are now un-gated and glfw-free — keep them that way.)
- **Don't** put raw Rust `\u{...}` escapes inside a non-raw Python heredoc — it breaks
  Python string parsing. Use literal chars or Python `\U0001F50B`-style escapes.
- **Don't** `create_file` over an existing path (it errors) — overwrite with
  `cat > path << 'EOF'`. For `str_replace`, anchor on a unique string and re-view
  after each edit (earlier views go stale).
- **Don't** confuse the gates: `cargo check` ≠ link (perf-link tooling won't speed
  it) and `cargo check` passing ≠ runtime-correct. `cargo test` can't link the full
  lib in some feature sets (the registries pull `render` + `input`); use
  `--features "render-core egui"`.
- **Don't** trust the prior-session summary on **mutable environment facts**. The FS
  resets — re-check what's installed (a summary claimed "mold installed" when a fresh
  sandbox had only `ld.gold`). Verify before relying.
- **Don't** over-format prose deliverables or pad with caveats; the operator wants
  dense, honest, skimmable docs and code.

---

## 6. Quick reference

**Commands**
```bash
# inner gate (no link)
cargo check --bin unendlich-fullgame --features render,window,gpu,ui-overlay,audio
# pure-logic tests (no glfw)
cargo test --lib --features "render-core egui"
# software-Vulkan headless run
XDG_RUNTIME_DIR=/tmp/xdg VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json \
  cargo run --bin gpu-headless-probe --features headless
# shader
glslangValidator --target-env vulkan1.2 crates/unendlich-engine/src/shaders/chunk.vert
# fast builds
bash scripts/sandbox-fastbuild.sh
```

**Key paths**
- Bin: `crates/unendlich-engine/src/bin/unendlich_fullgame.rs`
- Render: `render/world_renderer.rs` (menu UI), `render/context.rs` (Vulkan entry)
- Input core (glfw-free): `input.rs`, `input_codes.rs`, `keybindings.rs`,
  `keybind_ui.rs`, `player.rs`
- Net: `sim/net/{identity.rs, dht_mailbox.rs, session.rs, transport.rs, wire.rs}`
- World: `world/{terrain_generator,cpu_generator,gpu_generator,chunk_manager,
  surface,flora_placement,leaf_decay,plant_lifecycle,constants}.rs`
- Config/tune: `runtime_config.rs`, `safe_tune.rs`
- Shaders: `shaders/{chunk.vert,chunk.frag}` + `build.rs`
- Docs: `docs/WIRING_STATUS.md` (ledger), `docs/TOOLING_fast_linking.md`,
  `docs/TOOLING_sccache_nextest.md`, `docs/GLFW_DECOUPLING.md`, this file.
- UI mockups + design docs were operator UPLOADS unzipped to `/home/claude/ui-pkg/`
  and `/home/claude/new-docs/` — a fresh sandbox won't have them; ask the operator
  to re-send the UI package / design-doc zips if you need them.

**Mental model:** test what you can (pure logic + now lavapipe), diagnose what you
can't, label honestly, and let the operator's M2 be the final word on visuals, feel,
and the FP32 GPU path.

## STAGED / PLANNED (from r428–r434 operator direction)

### Water ladder #2 — TRANSPARENT WATER PASS (multi-session, device-tested per step; do NOT one-shot blind)
True see-through/refraction. Risky parts are all blind-unverifiable; stage so each step is testable:
 1. Mesher: emit water faces into a SEPARATE vertex range/buffer (tag water; keep opaque mesh otherwise). Test:
    world still renders identically with the water draw disabled.
 2. New pipeline: alpha-blend, depth-TEST but depth-WRITE off, drawn after opaque. Test: water shows, blended.
 3. Bind scene COLOUR + DEPTH as textures (descriptor-layout change — the risky one). Test opaque path unchanged.
 4. Shader: refract the scene sample by the wave normal; tint by Beer-Lambert over (sceneDepth - waterDepth).
    The depth grid (r432) + per-vertex water depth feed real thickness. Then underwater geometric warp (r427 was
    tint-only) can reuse the same scene sample in present.frag.
Gate the whole thing (UNENDLICH_WATER_TRANSPARENT); OFF = current opaque water exactly.

### PERF SETTINGS — live water knobs (needs plumbing first)
Water params are env-read ONCE at startup (immutable). For in-game knobs: make a shared `WaterSettings`
(atomics or Arc<Mutex>) the sim reads each tick; pass a handle into the settings UI (world_renderer.rs settings
window, Performance = tab 4). Add a Water SUBTAB/section with grouped sliders/toggles (Sim: radius/iters/settle_f/
budget; Cycle: evap/rain/ice/boil/steam; Visual: wave speed/depth-color/flow/underwater/erosion). UX: sub-tabs
inside Performance, or collapsing sections.

### MULTIPLAYER (operator direction)
- Distributed chunk-gen: world-owner-authorized peers generate chunks + run the optimizations so MP build/perf
  scales UP. Needs: a permission flag in the handshake (owner grants "gen authority"); authorized peers run the
  generator + stream results; RECONCILIATION when two authorities disagree (rare — last-writer / owner-wins /
  hash-vote). Equilibrium framing (operator): stable sim states are compressible — ship equilibria, not churn.
- World export/fork: serialize the server-world (seed + edits + sim state) to a file a player can take and host.
- Voice chat: feed sim/voice_spatial.rs from per-peer transforms; add an audio backend (capture/encode/mix with
  per-source gain+lowpass+reverb send) + a voice transport over the existing LAN channel. Device/network only.

## SERVER NETWORKING EXTRACTION (after r440)
unendlich-server ticks a headless world but has no transport yet. The LAN host (discovery beacon, JOIN/ACCEPT,
CH_HANDSHAKE/ENTITY/WORLDCMD replication) is inline in unendlich_fullgame.rs (~2150–2250, ~10080–10220). Factor
it into a reusable `sim::lan_host::Host` { bind, poll_joins, broadcast_transform, apply_worldcmd } the fullgame
bin AND the server bin both call. Then the server's tick loop drives interest points from connected players
(not origin), and distributed chunk-gen authority (owner-granted) + world export/fork plug in here. Also: gate
world_renderer.rs:526 `wizard_preview_tex` behind #[cfg(feature="egui")] to drop the server's egui dep.

## LIVE FLOW EROSION (after r442)
sim/current_force::erosion_rate is tested and ready. Wire it in the water sim write-back: where a floored
column's flow speed (water_flow magnitude) exceeds a threshold over a SOFT bed (Sand/Gravel/Dirt via
substrate_softness), erode the bed one step (Dirt/Grass→Sand→removed) on a small per-tick budget like the r433
shoreline erosion, so fast channels deepen over time. Keep it gated + budgeted; only the water sim should edit
terrain. Pair with deposition (drop sediment where flow slows) for mass balance if it looks too destructive.

## WIRING THE r443 CORES (all tested, none wired yet)
1. settle core: refactor the bin's inline settle to call sim::water_sim_core::ColumnGrid::settle (build the grid
   from the gather, run settle, read depth back). Device-verify the behaviour is identical, THEN new rules only
   ever change the tested core. This kills the drift risk.
2. erosion+sediment: in the write-back, for floored columns where current_force::erosion_rate(flow_speed,thr,
   softness,gain)>0 over a soft bed (Sand/Gravel/Dirt), erode one step on a small budget and sediment_map.erode;
   advect sediment along flow; deposit_excess where flow slows → place bed. Budgeted like r433. Test in core first.
3. lava-water: in the settle scan, when a Water and Lava cell are face-adjacent, apply water_react::
   lava_water_contact → set the lava cell Obsidian/Stone, optionally consume the water, emit a steam puff.
4. push-back/waves: sample the player AABB (1x2 via the ray system) + mobs + falling blocks; feed mass/speed to
   water_react::wave_amplitude and inject a transient bump into the flow/wave field at the body's column.

## WATER VISUAL DIRECTION (reasoned, operator asked)
Voxel identity is the signature — don't dissolve water into particles (expensive, loses the blocky volume that
makes it read as THIS game). Path, each device-verified: (a) TRANSPARENT + refractive draw pass so you see INTO
the volume — this is the single biggest cohesion win and unblocks everything; (b) 2x2 sub-voxel SURFACE layer
for silhouette only (waterline stops being a hard staircase) while the body stays voxel-blocky; (c) "reach"
animation as a surface-mesh lap on that sub-voxel layer. Transparent pass is the prerequisite; it's filed as
risky (descriptor/pipeline change can black-screen all rendering) → stage one device-verified step at a time.

## ITEMS WIRING (after r449)
The unblocker for most of the item system is an item-INSTANCE/metadata layer: today ItemStack is {ItemId,count}
with no per-item state, so durability/modifiers/affixes/rarity have nowhere to live. Add a lightweight instance
table (id → {durability, modifiers, rarity, name_key}) keyed off a stack handle, then wire in order:
item_durability (wear on mine/attack, break on 0), item_tooltip (compose from the instance + a name registry,
show on hotbar select), item_appraise/item_compare (value + A/B in the inventory UI), item_modifier/genetics
(roll on loot via loot_roll/legendary_loot). Cheap wins needing no metadata: inventory_txn (move/split stacks in
the open-inventory UI), loot_roll/loot_quantity/batch_loot_roll (route creature death drops through them like
blocks already are).

## BIOME/PLANT WIRING (after r450)
Plant spread is live (sim/flora_spread.rs → bin flora-spread tick, climate-coupled, default ON). Remaining
tractable RUNTIME plant wires (no worldgen change): plant_growth/plant_growth_env staged maturation of placed
saplings into trees over time (extend the existing plant_lifecycle tick); flora_harvest to yield seeds/extra
drops when the player breaks flora (route through the block-break loot path next to item_drop_resolve);
plant_root_system for spread that prefers cells near existing roots/water. BIOME side (biome_blend /
biome_transition / biome_temp_map) is worldgen-only and needs an ON-DEVICE session: the live path is the GPU
chunk_generation.comp, so biome blending must go in the shader (CPU cpu_generator blend is invisible while
Backend::Auto picks GPU). Treat like the transparent-water pass — stage one device-verified shader step at a time.

## PLANT WIRING (after r451)
flora_spread (r450) + flora_harvest bloom drops (r451) are live. Still-unwired plant cores need a GROWTH-STATE
layer to be meaningful: plant_growth/plant_growth_env staged maturation (sapling→tree) and flora_harvest's
regrow timer both want per-plant growth01 stored somewhere — today placed flora is a bare BlockId with no
instance growth field (BlockInstanceState exists for leaf-decay flags; a growth01 byte could ride there). Design
choice for the operator: (a) add a growth byte to BlockInstanceState and have the plant_lifecycle tick advance
placed saplings/young flora toward mature (then swap young→mature block, or place a tree via flora_blocks::tree),
or (b) keep flora stateless and skip maturation. plant_root_system + plant_propagation Gravity/Animal modes could
later vary spread shape by species. BIOME blend/transition still worldgen-GPU-only (on-device shader session).

## ADDING A BIOME OR WORLDGEN PLANT (on-device shader session — r455 scoping)
Live worldgen = GPU shaders/chunk_generation.comp. To add a BIOME, edit IN LOCKSTEP (repr must match exactly):
biomes.rs (enum + determine_biome + height_modifier + tree_profile) AND chunk_generation.comp (BIOME_* const +
biome_at_2d + biome_height_modifier table + tree decision). To add a worldgen PLANT/tree: blocks.rs BlockId (+
repr) + mesher.rs colour + TreeProfile + the shader RC40 tree consts + tree pass. Shader errors black-screen
worldgen (timebending does NOT recover it) → stage ONE biome/plant end-to-end at the machine, regen, verify,
repeat. Pure-Rust pieces (determine_biome mapping, height_modifier, tree_profile, mesher colour) can be written +
flatten-tested off-device first; only the shader edit + repr-sync needs the device. Safe alternative already
shipped: sim/pioneer_flora.rs adds runtime plant variety from existing blocks with no shader change — extend it
(more substrate/climate arms) for cheap visible variety anytime.

## BIOME GPU-SYNC (after r457) — make biomes 19..27 actually generate
chunk_generation.comp only knows biomes 0..18; Savanna/Jungle/Taiga/CrystalCave/LushCave/Mesa/Mangrove/
VolcanicWastes/FrozenOcean (19..27) are CPU-only and never appear in GPU worlds. To fix: add their BIOME_* consts
(match biomes.rs repr), port the CPU determine_biome arms (the hot/wet jungle, cold taiga, mesa, mangrove,
frozen-ocean, volcanic, cave splits) into the shader's determine_biome GLSL with identical thresholds, and add
their biome_height_modifier rows. glslang-validate (RC=0) each step. Logic mismatch = cosmetic (biome appears in
slightly different climate vs CPU), not a crash. HighlandSteppe(28) is already done as the template.
