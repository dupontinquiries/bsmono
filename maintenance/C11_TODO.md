# C11_TODO — open threads from the c11 session

Same convention as `C9_TODO.md` / `C10_TODO.md`: every item says how far it
actually got. Those two files are NOT superseded — both were edited in place
this session where status changed (C10-0a), and C9 remains the larger backlog.

The c11 session installed the offline 1.96.0 toolchain from the pack-rust
bundle and did the C10-0a state model (`2e9a9695`). What follows is what it
found and did not get to.

---

## 1. C10-0a's remainder — appearance, on hardware

The unified-surface STATE model is done and verified; see the C10-0a entry in
`C10_TODO.md` for the split. The open questions all need a display: true
modality (scrim + input capture, which is what would make Esc-to-close safe),
and settings-as-popup vs settings-as-right-panel. `surface.rs`'s module docs
carry the reasoning so the on-device session does not re-derive it.

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
