# Games — deep analysis (content increase · kit integration · consolidation)

Scope: every game in the ecosystem, analyzed one at a time for (1) which copy has the most content
and where it should live, (2) how it's wired to the shared kits and what integration remains, and
(3) the shared-networking handoff. Work done in the consolidated tree (apps/ game duplicates already
retired; all games now under `games/`).

**Legend for kit columns:** ✓ wired · ✗ not wired · n/a not applicable (wrong language/stack).

| game | canonical | content verdict | sensor-kit | touchkit | gpu-kit | shared-net | notes |
|---|---|---|---|---|---|---|---|
| unendlich | `games/unendlich` (2096f, 40M) | superset of apps/; **r637 drop is +22f newer** | ✗ | ✗ | ✗ (dynamic present-mode) | own `sim/net` + DHT; handoff pending | rebase r637 "sway-water" delta |
| blackstar | `games/blackstar` (736f, 94M) | **ahead** of apps/cybertron *and* cmake drop | n/a (C engine) | n/a | n/a | own C `netcode.c`/`net_transport` | Rust kits don't apply |
| paperbound | `games/paperbound` (40f) | **stub→full**: drop (40f) replaced 2-file stub | n/a (Python) | n/a | n/a | own `netplay.py` (LAN lockstep) | needs python build lane |
| clawbound | `games/clawbound` (23f) | single copy, canonical | ✗ | ✗ | **already Fifo** (not a stutter source) | `net.rs` present | gpu-kit = robustness only |
| soldier | `games/soldier` (107f) | single copy, canonical | ✗ | ✗ | ✗ (dynamic present-mode) | minimal net | needs inspection |
| gyroball | `games/gyroball` v15 (360f) | v15 canonical; v2–v14 empty | ✗ (its own libiio daemon) | ✗ | ✗ (not wgpu) | none | **best sensor-kit dogfood** |
| bsgames | `games/bsgames` (7f) | the fleet **hub**, not a game | ✗ | ✓ | ✗ (egui) | **✓ handoff wired + tested** | consumes netcode |

The consistent story: **no game yet uses sensor-kit or gpu-kit, and only bsgames uses touchkit.** The
kits exist and are tested in isolation, but the games predate their extraction. That's the whole
integration backlog.

---

## unendlich — voxel GPU engine (Rust, wgpu)
**Content.** `games/unendlich` (2096 files, 40M) is the canonical copy and a strict **superset** of the
now-removed `apps/unendlich` — it carries 5 extra net modules (`chunk_pool`, `client_reconcile`,
`lag_compensation`, `player_roster`, `snapshot_interp`) and nothing lived only on the apps side.
**Open item:** the `unendlich-r637-sway-water` drop is **slightly newer** than the tree — 2118 vs 2096
files, 26 vs 25 net modules. The delta is the "sway-water" revision (rev 637) plus one extra net
module. Recommend a focused rebase of just that delta onto `games/unendlich` rather than a wholesale
swap (the tree has its own history the drop lacks).

**Kit integration (pending).** Uses wgpu but sets present-mode dynamically (no literal `PresentMode::`),
so it needs individual inspection to route the surface through `gpu_kit::Policy` (intent `Vsync` →
`pick_present_mode` over the surface's supported modes). This is the primary stutter-fix candidate.
Sensor-kit: fold `TiltControl::tick(&mut vel)` into the player-move vector for phone tilt-steer.
Touchkit: `GestureRecognizer` for drag-look / pinch-zoom on touch.

**Networking.** Rich own data plane (`sim/net`: entity replication, chunk streaming, lag comp). It
should *announce/discover* via the shared control plane — call `host_game_session` / `join_friend_session`
(now in bsgames' seam, backed by `netcode::session`) and keep its `UdpTransport` as the data plane.

## blackstar — space RTS on the exengine C engine (C)
**Content.** `games/blackstar` (736 files, 94M) is canonical and **ahead of everything**: its
`netcode.c`/`.h` are byte-identical to `apps/cybertron`'s and it *adds* `net_transport.{c,h}`; and it's
larger than the `cybertron-cmake` drop (524 files) too, which also has `net_transport`. Confirmed: the
24f blackstar is the later, better version.

**Kit integration.** **n/a** — the Rust kits (sensor-kit, touchkit, gpu-kit) can't bind to a C engine
directly. If blackstar should share tilt/touch input, the path is a thin C FFI shim over the kits, or
duplicating the small policy in C. Its networking is native C (`netcode.c` + `net_transport`), so the
Rust `netcode` crate's handoff would also need an FFI bridge to participate in fleet discovery.

## paperbound — origami lane-battler (Python, pygame-ce)
**Content.** Was a **2-file stub** in `apps/`; now `games/paperbound` carries the **real 40-file game**
from the drop (game.py 453K, `netplay`/`ai_director`/`diffusion`/`cards`, 30 cards / 9 arenas / LAN
lockstep). This both moved it into `games/` and did the paperbound rebase.

**Kit integration.** **n/a** — pure Python. It has its own `netplay.py` (UDP lockstep). Manifest now
declares it `kind = "python"`, which the build orchestrator doesn't yet have a lane for (flagged in the
manifest note). Fleet discovery would need a small Python client speaking the same session-record wire.

## clawbound — claw-machine action (Rust, wgpu)
**Content.** Single copy, canonical (23 files). No duplicate, no drop.
**Kit integration.** Already configures the surface as `wgpu::PresentMode::Fifo` **directly** — so it is
**not** a stutter source, and gpu-kit here is a robustness/consistency change (let the policy pick Fifo
from supported modes) rather than a fix. Sensor-kit/touchkit not wired; small enough to be the cleanest
**reference integration** for the other wgpu games.

## soldier — third-person shooter (Rust, wgpu)
**Content.** Single copy, canonical (107 files). No duplicate, no drop.
**Kit integration.** Uses wgpu with a dynamic present-mode — needs inspection to route through gpu-kit.
Sensor-kit/touchkit not wired. Minimal networking today.

## gyroball — tilt-maze (Python + a Rust/C sensor daemon)
**Content.** **v15** is canonical (360 files); `v2`–`v14` are empty placeholders and can be pruned.
Top-level `ball_maze.py` + `run.sh` drive it.
**Kit integration.** This is the **origin** of sensor-kit's tilt logic but predates the extraction — it
still ships its own `sensor_daemon/libiio`. It's the ideal **dogfood**: replace the bespoke tilt read
with `sensor_kit::TiltControl` (`feed_line` the monitor-sensor stream → `tick`), proving the extracted
kit against the code it came from. Not wgpu, so gpu-kit is n/a.

## bsgames — the fleet games hub (Rust, egui)
**Content.** Not a game — the **centre** that shows who's online and launches games (7 files). Consumes
the shared `netcode` crate; chat rides share-kit's `Mailbox` over snapcore; presence bridges to
coord-core `Activity`.
**Kit integration.** Uses touchkit already. gpu-kit is n/a (egui owns its surface).
**Networking — DONE.** The "▶ Host / ▶ Join" seam is wired to the session handoff and **build-verified**
(see below).

---

## Shared networking — implemented and verified ✓
The control-plane **handoff** from `NETCODE_DHT_ANALYSIS.md` is now real code, not a plan:

- `netcode::session::SessionRecord{game, host, transport, addr}` — versioned, length-prefixed wire
  (same discipline as `profile`); keys namespaced `session:` so a token can't collide with a player id.
- `ProfileService::put_raw / get_raw` for non-profile stored values.
- `SessionExt` on `ProfileService`: `announce_session` (publish record + flip presence to
  `InGame{session}`), `resolve_session`, `join_via_friend` (lookup → token → record), `end_session`.
- bsgames seam: `host_game_session` (announce + activity → InGame) and `join_friend_session` (resolve a
  friend's live session → `(transport, addr)` for the game's *own* transport).

**Verified:** `cargo test` — **netcode 12/12** (6 new session tests: announce→join over the snapcore
floor, latest-wins on a register primary, presence flip, round-trip, bad-version/truncation, key
namespacing) and **bsgames 6/6** (incl. `host_then_join_over_the_fleet_dht` against real
snapcore + share-kit). Each game keeps its own realtime transport; this is discovery only.

**Known limitation surfaced.** snapcore is a **multi-value** DHT (put appends distinct values, dedups by
bytes with in-place TTL refresh; get returns all). So `SnapcoreStore::get` taking `.next()` is **not
latest-wins** — a republished profile does not supersede stale values on the micro-DHT floor, and
`end_session` can't *instantly* invalidate an invite everywhere (it flips local presence; the stale
record ages out by TTL). This affects all presence, not just sessions. Proper fix: put a monotonic
stamp in stored values so lookups pick the newest. Offered as a follow-up.

---

## What's done vs pending
**Done + verified:** all games consolidated under `games/` (paperbound moved with full drop content;
apps/ duplicates retired); manifest + suites repointed; the shared-networking session handoff
implemented, wired into bsgames, and green under `cargo test`.

**Pending (per-game kit integration — needs a graphics build I can't run in this sandbox):**
1. **gpu-kit present-mode** into unendlich + soldier (real stutter-fix candidates; clawbound already
   Fifo). Small edit at each surface-config site → `Policy::default().pick_present_mode(supported)`.
2. **sensor-kit** tilt-steer into gyroball (dogfood), unendlich, soldier, clawbound.
3. **touchkit** drag/pinch into the wgpu games.
4. **unendlich r637** delta rebase ("sway-water" + the extra net module).
5. **FFI bridges** if blackstar (C) / paperbound (Python) should join fleet discovery.

Because each of these touches a game's render/input loop and can only be built with wgpu + system
graphics present, the responsible path is one game at a time as reviewable patches you build-check —
which is exactly the "1 at a time" cadence. Say which game to start with.
