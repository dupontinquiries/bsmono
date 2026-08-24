# C11_TODO — open threads from the c11 session

Same convention as `C9_TODO.md` / `C10_TODO.md`: every item says how far it
actually got. Those two files are NOT superseded — both were edited in place
this session where status changed (C10-0a), and C9 remains the larger backlog.

The c11 session installed the offline 1.96.0 toolchain from the pack-rust
bundle and did the C10-0a state model (`2e9a9695`). What follows is what it
found and did not get to.

---

## 1. C10-0a's remainder — tuning, on hardware

REVISED mid-session on the user's call: appearance work gets DEMOED in the
codebase and tuned on device, not deferred to it. So the forms now exist:
every transient surface renders through egui's Modal (scrim, click-outside,
Esc consumed) in its QML form, and the PTY key drain is gated while a surface
is up — see the second c11 commit. What remains on-device is TUNING, not
implementation: every knob is in `overlay.rs`'s `form()` table (QML line
cited per row) and the two constants under it. Known open judgments for the
device pass: settings is centered rather than QML's nav-anchored position;
the cmd sheet's radius-20 top corners were not ported; workspaces' right-
drawer form has no QML counterpart and was chosen. Appearance is UNVERIFIED
here — no display.

## 2. Checker findings, all pre-existing, none fixed

Run with the repo root as the argument — the default path does not exist in
this sandbox, and a checker pointed at nothing reports success
(`scanned 0 crates`), which is exactly the vacuous-pass shape CLAUDE.md §5b
warns about. The runs below scanned 254 crates.

- **C11-2a — `lock_check`: `games/unendlich/crates/unendlich-engine` declares
  but has not locked** audiopus, egui, linkme, netcode, ogg, rayon, rodio,
  unendlich-ui (and its dash sibling serde_json). `verified` (the checker ran;
  the cause is not diagnosed). NOTE the same crate is C10-1a's one "looks
  real" feature-sweep FAIL — a stale lock resolves online and fails offline,
  which is consistent with an 82 s failure in an offline sandbox. Check the
  lock BEFORE debugging the crate.
- **C11-2b — `orphan_check`: 7 orphaned .rs files, 0 tests.**
  `games/blackstar-rs/src/rig.rs` (446 lines),
  `games/unendlich-engine`'s `render/bloom.rs`, `render/postfx.rs`,
  `render_meshref_shim.rs`, `system/snapclone/camera-bridge/src/shared_frames.rs`,
  plus two more past the listing cut. Each wants the §7b decision: archive
  with provenance, or declare and compile. Nobody has looked at why they
  exist.
- **C11-2c — `rustcheck` flags 10 `missing-mod-file` WARNs in
  voiceterm/egui-frontend** (`mod recall;` in cmd_sheet.rs etc.) that the
  green compile contradicts — the mods resolve, so the checker's file-layout
  expectation is wrong for however these are declared. Either teach rustcheck
  the pattern or record it as known noise; a checker that cries wolf on a
  green crate gets ignored (same failure mode as C10-1b's skip list).
  Also `ambient-learner` (1), `krapper` (2), `rusty-board` (1), undiagnosed.

## 3. Not reached

The rest of C10_TODO §0 (C10-0b snapclone-egui, C10-0c fao-codec inventory,
C10-0d rusty-board keyboard parity inventory) and §1 (the feature sweep's
games/ tail and the whole apps/ slice) were not touched this session.

## 4. The eframe backend thread (snapshot-cam's on-device crash)

- **C11-4a — `camera-client` does not compile.** `verified` pre-existing (by
  stash): `PinningVerifier` lacks `Debug` where something formats it. Nothing
  to do with the limits fix in the same crate; undiagnosed beyond that.
- **C11-4b — road-warrior's wgpu limits are unclamped.** `read`. It passes
  `eframe_backend_check.py` via an explicit `Renderer::Wgpu`, so the choice
  is deliberate — but it inherits egui-wgpu 0.27's default device request,
  the same over-ask that killed snapshot-cam. Old eframe, different API;
  wants the camera-bridge treatment when someone is in there.
- **C11-4c — beatspace's app feature and unendlich-dash are unverified by
  compilation.** Graphs resolve with the new explicit features; cpal blocks
  beatspace's GUI here (§3) and dash is the known sweep-timeout unit.
- gstreamer dev headers ARE installable here (`apt-get update` first — the
  baked lists are stale and 404). That is what made camera-bridge verifiable;
  noted in SANDBOX_BUILD_LIMITS.md.
