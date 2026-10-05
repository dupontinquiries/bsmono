# unendlich

A voxel world engine and game in Rust on Vulkan (`ash`): GPU-generated and GPU-meshed
chunks, a deterministic simulation core with rollback netcode, creatures, weather, water,
and an egui overlay for the menus and HUD. This directory is the whole project; the
engine is `crates/unendlich-engine`, and the playable binary is its
`unendlich-fullgame` bin.

This file is orientation only. The state of the project is in
[STATUS.md](STATUS.md) (what is verified, what was written and never run, what is
known incomplete); the open work is [BACKLOG.md](BACKLOG.md); the newest open threads
are in the monorepo's [maintenance/C21_TODO.md](../../maintenance/C21_TODO.md) §3b.

## Build and run

The engine's default features are the playable set (`gpu`, `render`, `window`, `audio`).

```sh
cd games/unendlich
# the game: main menu → singleplayer / LAN host / LAN join → the world
cargo run --release -p unendlich-engine --bin unendlich-fullgame --features ui-overlay
# a headless world host (no window, no GPU): `--help` lists the flags
cargo run --release -p unendlich-engine --bin unendlich-server \
    --no-default-features --features "headless egui" -- --name atlas --seed 42 --world-dir ./atlas
```

The engine's tests build without a GPU or a window:

```sh
CARGO_BUILD_JOBS=1 cargo test -p unendlich-engine --no-default-features --lib
```

(9130 pass headless as of c21; the default-features run adds the render tests and carries
two documented failures — `docs/KNOWN-TEST-FAILURES.md`, C21_TODO §3b. One build job: the
test crate's rustc peaks near 2.6 GB.)

## Where things are

- `crates/unendlich-engine/src/sim/` — the deterministic core: rollback (`rollback.rs`),
  the LAN session (`lan_discovery.rs` and `net/`: the host pump with admission and relay,
  the reliable channel for world edits, the wire codecs), world history points
  (`world_history.rs`), creatures and their `cognition/`, water.
- `crates/unendlich-engine/src/world/` — generation (CPU and the GPU compute path, kept in
  parity by `tools/density_parity_lint.py`), chunks and the chunk manager, flora, erosion,
  water cells; `src/storage.rs` is the on-disk region format, `src/weather.rs` the weather.
- `crates/unendlich-engine/src/render/` — the Vulkan renderer: GPU meshing, froxels,
  lights, the egui overlay (`egui_overlay.rs`).
- `crates/unendlich-engine/src/bin/unendlich_fullgame.rs` — the game loop, menus and
  LAN glue (one large file by design; see the pure `sim::` modules it composes).
- `crates/unendlich-engine/src/keybindings.rs` — the controls.
- `crates/unendlich-engine/src/bin/unendlich_server.rs` — the headless host.
- `crates/unendlich-ui` — the egui kit the overlay draws with; `crates/unendlich-dash` —
  the telemetry dashboard; `crates/unendlich-launcher`, `unendlich-eink`,
  `unendlich-analyze`, `voxel-tools`, `vk-harness` — tooling.
- `harness/` — the GPU and hygiene gates (`gpu_hw_check.sh` is the only source of a
  hardware timing; `RULESET.md`).
- `docs/` (166 files) and `handoff/` (120) — design notes and session handoffs. Many
  describe the engine as it was when they were written; check the code before acting on
  one (the monorepo's `CLAUDE.md` §5), and prefer `STATUS.md` for current claims.

## Multiplayer, briefly

LAN play is on by default (`UNENDLICH_LAN_MP=0` turns it off): a host announces its world on
the LAN discovery beacon, joiners pick it from the Multiplayer screen or dial it directly
with `UNENDLICH_LAN_HOST=<ip>[:port]` (port 9878). Block edits ride a reliable channel
(acked, resent, applied once in order); entities and avatars ride the unreliable one. The
dedicated server persists its world under `--world-dir` and autosaves. World history
points (the WORLD HISTORY menu) copy the world's region files under `rollbacks/<ts>/` and a
restore is staged, then applied on the next open with a safety point taken first.
