# C19_TODO — open threads from the c19 session

Same convention as `C9`–`C18`: every item says how far it actually got.
None of those files are superseded.

Session: `a9cb8a906..72ac18c58`, 14 commits, no merges. Touched
`games/clawbound-battleground`, `system/photosync` (+ new
`photosync-egui` crate), `apps/gitmind/crates/gui-egui`, and
`apps/snapclone` (both the Rust `snapclone-nat` crate and the CMake
build). Full per-commit detail is in the commit messages themselves;
this file is the cross-project index and the honest "verified how" for
each one, plus what's still open.

---

## 1. clawbound-battleground — water/depth, prop collision, cats, a
##    third control scheme, item-gated leveling

Five commits, `3f70d2d7d..9977eb82b`.

- **Water/alpha depth-test (`3f70d2d7d`).** `depth_state_ro` used
  `CompareFunction::Always` — every alpha-blended draw ignored the depth
  buffer entirely. Fixed to `LessEqual`; also sorted the transparent
  draw list back-to-front (it wasn't sorted at all). **Not fixed:** the
  water surface is a separate non-depth-writing draw outside that
  sorted list, so it still can't correctly interleave with a
  transparent object genuinely behind it.
- **Prop collision (`43531ea40`).** Trees/stones had zero collision
  geometry anywhere. New `solid!` placement macro records `(x, z,
  radius)` as a side effect of the existing visual-mesh placement loop
  — one source of truth. Flows through `Game::set_obstacles` into
  push-out logic mirroring the existing unit-separation code.
- **Cats (`1c89e0965`).** `build_minion` was humanoid even though the
  ancestors are already quadruped felines. Same function signature, so
  no other file needed touching.
- **Dvorak ESDF (`d5331a130`).** `.OEU` added as a *separate* scheme
  (`Scheme::DvorakEsdf`), not merged into the existing `,AOE` one — `o`
  and `e` are already bound to different actions there, so merging
  would make one physical key trigger two actions.
- **Leveling 1–9 (`6e4e399a7`).** Item slots now ramp 1→6 across levels
  1–9 (`items::slots_for_level`); level 9 is also a one-time +10%
  HP/power capstone (`GodUnit.awakened`), layered on the existing
  smooth 1–20 curve, not replacing it.

**Verified:** all four Rust changes compile and their own test suites
pass (`cargo test`, `--no-default-features`), including the 120s full
headless match simulation after the leveling change. **Not verified:**
no GPU in this sandbox, so the water/depth fix and the cat model's
actual on-screen appearance are unconfirmed by eye — only by reasoning
about the pipeline and the mesh-generation code.

## 2. photosync — permissions, connection() reachability, a gallery
##    UI, and a chunk-sync channel on snapclone-group's DAG

Four commits, `af77c7a7b..095f7579d`.

- **`connection()` (`af77c7a7b`).** `album::load` — the function that
  actually builds a gallery `Library` with albums — was unreachable
  from anything holding a `PhotoSyncDB` (the daemon, sync server, CLI,
  any egui shell) because the connection was private. This was
  discovered by first building a *worse*, redundant path
  (`FileRecord`/`Item::from`/`Library::from_records`) before checking
  whether `album::load` already existed — it did, and better (it also
  loads albums). Backed that out in favor of the real fix. Worth
  knowing this almost shipped a duplicate.
- **Permissions, same commit.** `Permission` enum + a `permissions`
  table. Nothing calls `authorize()` yet — `sync_server.rs` still
  accepts writes ungated.
- **`peers.rs`, same commit.** Sync-partner discovery on `dht-kit`
  only — deliberately not `share-kit::mailbox` (pulls in `snapcore` +
  `crypto-kit`, real dependency weight for a crate with no networking
  today; noted as the natural next layer, not built).
- **`photosync-egui` (`223cf0151`).** New crate. Settings is an
  ordinary rail destination, not a GTK-style separate window.
  Thumbnails are three procedural styles (`ThumbStyle`), never
  written to disk. **Verified:** `cargo build --features app` actually
  links a running binary, not just `cargo check`.
- **`chunk_sync.rs` (`b542a1492`).** Reuses `snapclone-group`'s DAG +
  anti-entropy engine + delivery tracker for something it wasn't built
  for — durable photo chunks instead of ephemeral group snaps.
  Chunk granularity, not file. **Found and fixed a real bug in
  `snapclone-group::sync` while testing this**, not just in new code:
  `SyncEngine`'s in-flight request tracking has no timeout, so a
  connection dropped mid-request stalls that chunk (and everything
  causally behind it) forever, even across a fresh reconnect. Confirmed
  by writing an interrupted-then-resumed test that got zero chunks at
  every round count tried before the fix. `ChunkSync::begin_connection`
  is the fix (clone the DAG into a fresh `SyncEngine`); it lives in
  `photosync::chunk_sync`, not in `snapclone-group` itself — anyone else
  calling `snapclone-group::sync::SyncEngine` directly still has this
  gap and doesn't get this fix for free. **Worth upstreaming.**

**Verified:** 28 lib tests pass; also ran `snapclone-group`'s own 99
tests to confirm the new dependency didn't disturb anything there.
**Not verified:** no transport, no crypto sealing — chunk hashes travel
in the clear in this version; that's explicit in the module doc, not an
oversight.

## 3. gitmind-egui (`eabb5b376`) — a README the crate itself was missing

Build/run instructions existed only in the top-level
`GITMIND_EGUI_PORT.md`. **Verified for real**, not just documented:
both `cargo build -p gitmind-egui --lib --no-default-features` and
`cargo build -p gitmind-egui --features app` exit 0 from
`apps/gitmind`, the latter producing a linked 385MB debug binary.

## 4. snapclone — snapclone-nat rewrite + OpenDHT link fix

Three commits, `644b63a02..c4a71c6aa`, plus a docs commit
(`72ac18c58`).

- **`snapclone-nat` (`644b63a02`).** Rewritten around one shared UDP
  socket (the scaffold bound two, on the same port, which breaks
  hole-punching's whole mechanism) and made wire-compatible with the
  real Qt peer (the old punch payload was a bare zero byte). Wrote
  `ffi.rs`, which `lib.rs` had declared behind `feature = "ffi"` since
  the crate was first scaffolded but which never existed as a file —
  the feature was silently unbuildable and nothing in the app could
  reach this crate. Ported the tested constants from
  `core/Timings.hpp` instead of the scaffold's guesses.
  **Verified:** 20 tests pass with `--all-features`, including a real
  two-peer punch exchange over the sandbox's live interface. **Not
  verified:** no real NAT, no live STUN server reached, Qt side not
  wired (no `sc_nat_*` header written yet).
- **OpenDHT link fix (`c4a71c6aa`).** `OPENDHT_C` was forced `OFF` in
  CMake, but `JamiAdapter_dht_impl.cpp` calls the C API exclusively —
  a fresh clone fails to link. **Verified with an actual compiler**,
  which is why this is in "done" rather than "diagnosed": built an
  isolated harness against the vendored `vendor/opendht` submodule,
  reproduced the failure (`undefined reference to 'dht_runner_new'`)
  with the old config, confirmed the fix builds/links/runs. The full
  Qt6 app was not build-tested (no Qt6 in this sandbox) — only the DHT
  link step itself.
- This bug was independently diagnosed once before, in an earlier
  session recovered from an exported chat backup — the fix was
  described in detail there but apparently never landed in what became
  this tree's snapshot. Two other bugs from that same export (a
  sticky, non-decaying LAN-isolation failure counter in
  `NetworkManager::connectToPeer`, and a simultaneous-initiation
  "glare" bug in the PQ handshake producing mismatched session keys)
  were checked against the current tree and **both are already
  fixed** — `m_lanProvenThisSession`/`m_lanFailureLastMs` and the
  `stableId`-ordered glare tie-break in `AppController_connect.cpp`
  are both present. No action needed on those two; recorded here so
  the next session doesn't re-diagnose them from scratch.

---

## What's still open, in the order I'd take it

1. **Wire `photosync::chunk_sync`/`peers.rs` to a real transport.**
   Both are pure logic today — no sockets, no NAT traversal, no
   encryption. `snapclone-nat` (this session) is the natural transport
   to reach for once that's wanted; `snapclone-group::entry.rs`'s
   sealed envelope format is the natural next layer for the crypto.
2. **Wire `snapclone-nat` into the Qt app.** Write the `sc_nat_*`
   header, replace whatever the C++ `NatTraversal` class currently does
   (or confirm it's still doing that job and this crate is additive).
3. **Confirm the OpenDHT fix on a real Qt6 build.** `rm -rf build &&
   cmake -B build -DSNAPCLONE_OPENDHT_BACKEND=ON` from clean, on a
   machine with Qt6 installed.
4. **Upstream `SyncEngine`'s in-flight timeout fix** into
   `snapclone-group::sync` itself, rather than leaving it as a
   photosync-only workaround (`ChunkSync::begin_connection`) that every
   other caller of that engine still lacks.
5. **`sync_server.rs` doesn't call `authorize()` yet** — permissions
   exist and are tested but enforce nothing.
