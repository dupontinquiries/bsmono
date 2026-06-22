# Running logscope (scene-view) on your device

`scene-view` is the renderer binary for the logscope mind-palace. The included build is a **self-contained
release build** (Rust libstd statically linked) for **Linux x86-64**.

## Requirements
- Linux x86-64, glibc ≥ 2.38 (run `ldd --version` to check).
- A windowing + GL stack: X11 (or XWayland) and libGL. On Debian/Ubuntu:
  `sudo apt install libgl1 libx11-6 libxkbcommon0 libxi6 libxcursor1 libxrandr2 libwayland-client0`
- A real GPU is used automatically if present (much better than the software renderer used while developing).

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
