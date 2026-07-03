# Running logscope (scene-view) on your device

`scene-view` is the renderer binary for the logscope mind-palace. The included build is a **self-contained
release build** (Rust libstd statically linked) for **Linux x86-64**.

## Requirements
- Linux x86-64, glibc ≥ 2.38 (run `ldd --version` to check).
- A windowing + GL stack: X11 (or XWayland) and libGL. On Debian/Ubuntu:
  `sudo apt install libgl1 libx11-6 libxkbcommon0 libxi6 libxcursor1 libxrandr2 libwayland-client0`
- A real GPU is used automatically if present (much better than the software renderer used while developing).

## Two binaries
- `logscope` — the full app (chat + the 3D mind-palace; this is what has the Wayland opaque-clear fix).
- `scene-view` — the scene harness (renders one theme full-window; honours `LOGSCOPE_THEME` / `LOGSCOPE_CONVO`).

## Run it
```sh
chmod +x scene-view
LOGSCOPE_THEME="Mistfall" ./scene-view
```
Pick a scene with `LOGSCOPE_THEME=` one of:
`Atlantis · Mistfall · Slate · Reading Room · Archive · Old Library · Folding@home · Forest · Treehouse · Canopy · Cloudscape · Mansion Garden`

### Useful environment variables
- `LOGSCOPE_HOUR=0..24` — time of day (dawn ~7, noon ~13, dusk ~19, night ~22). NOTE: bright scenes (Mansion,
  Reading Room) look best in the evening (≈19–22); dark scenes are fine any time. See RULE_MAP.md.
- `LOGSCOPE_WORLD_ONLY=1` — render only the environment (hide the conversation overlay).
- `LOGSCOPE_SEASON=spring|summer|autumn|winter` (Forest).
- `LOGSCOPE_TUNE_SCALE`, `LOGSCOPE_TUNE_HEIGHT`, `LOGSCOPE_TUNE_DENSITY` — multipliers for room size / vertical
  spread / scatter density.
- `LOGSCOPE_COLOR_SALIENCE=0..1` — manually drive the Reading Room rug's colour (normally derived from how much
  the conversation talks about colour).
- `POS_ANGLE_INIT="yaw,pitch,dist"` — initial camera (higher pitch looks down; it auto-frames the graph).
- `LOGSCOPE_SAT`, `LOGSCOPE_EXP`, `LOGSCOPE_FOGM` — saturation / exposure / fog grade.

## If the binary won't run (wrong arch, older glibc, or non-Linux)
Build from the included source instead (needs the Rust toolchain, https://rustup.rs):
```sh
cd logscope-suite
cargo build --release --features gpu-scene --bin scene-view
./target/release/scene-view
```
Note: the repo's `.cargo/config.toml` sets `prefer-dynamic` + `mold` for fast dev iteration. That's fine for
running on the same machine you built on. For a PORTABLE binary, build with:
```sh
RUSTFLAGS="-C link-arg=-fuse-ld=mold" cargo build --release --features gpu-scene --bin scene-view
```
(or just drop `prefer-dynamic` from `.cargo/config.toml`). Without that, the binary depends on the toolchain's
`libstd-*.so` and won't run elsewhere.

## Camera
- The view auto-frames the conversation and now EASES as the strand breathes/grows (no more twitch).
- **▶ Tour** (top control bar) flies the camera along the conversation through the palace, gliding around
  columns and staying within the walls. Press **■ Stop tour** (or **⊹ Overview**) to return to free orbit.
- Drag to orbit, scroll to zoom, click a node to glide to it.

## New knobs (this build)
- Render fidelity: header dropdown now goes 1–16 (was 1–10); tiers 11–16 add up to 1k textures + denser geometry (real-GPU territory).
- Wind: `LOGSCOPE_WIND=<0..2.5>` base breeze strength (default 1.0), `LOGSCOPE_WIND_DIR=<degrees>` heading. Foliage (leaves, grass, moss) sways; trunks/stone stay rigid.

## New this build
- Fidelity dropdown now reaches **tier 16 = 2048px** textures (heavy CPU bake; real-GPU/strong machine).
- Each scene shows a **physical UI context panel** in its own material (wood/stone/glass/marble) — the start of the physical-UI concept.
- **Tree bend varies per scene**: Forest widely varied, Canopy calmer, Alley arches hard over the lane.

## Battery saver (Linux phones)
`LOGSCOPE_POWER=half` or `=quarter` renders the 3D scene at half/quarter resolution (¼ / 1⁄16 the fragment cost), caps texture tiers (≤6 / ≤3), thins procedural scatter, and culls more aggressively. `full` (default) = no change. Combine with the fidelity dropdown as usual; the UI always stays crisp.
Frustum culling is on by default (`LOGSCOPE_NO_CULL=1` disables it).
