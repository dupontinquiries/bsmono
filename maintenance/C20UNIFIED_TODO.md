# C20UNIFIED_TODO — open threads from the c20unified session

Same convention as `C9`–`C19`: every item says how far it actually got. Named
`C20UNIFIED`, not `C20`: `C20_TODO.md` (and `MERGE_GUIDE_C20.md` at the repo root) came
with the blackstar-rs drop from a DIFFERENT session that also called itself C20.

Session: `0260c59f7..f849dad57` (79 commits by `c20unified agent`, 4 of them merges), exported
as `c20unified-001` … `-023`; sections 10-12 were added after the first write (`b0135f593`).
The merge guide for this line is `MERGE_GUIDE_c20unified.md` at the repo root. Per-commit detail is in the commit messages; this file is the
index and the "verified how", plus what is open. Nothing here was run on a phone by this
session; phone results came from the operator and are quoted as such.

---

## 1. Recovered work (lost with the c19 sandbox)

The 042 bundle was cut before c19's last commits. Rebuilt verbatim from the transcript:
tideshell ctl socket / install-keys / logind drop-in (`8fb7cb3df`, was 85f295473), depot
pairing (`dc9b26321`…`f8240aedc`, was never committed), the hatch Wayland quit fix
(`0260c59f7`, from a stash). The tideshell session later dropped its own copies of the first
two when rebasing onto our export, so nothing is duplicated.

## 2. Merges

| merge | what | notes |
|---|---|---|
| `6ec8ee781` | c19unified-036-remarkable (eink-kit, the tablet fleet, Alpine roots) | 1 conflict (two CLI subcommands) |
| `e74b33b9c` | c19unified-039-hive: the sibling integration line (fao streaming/compact/Media, voiceterm, snapclone, clawbound, studio-film, blackstar-rs, logscope-hive) | 8 conflicts; each resolution is in the merge message. Backlight writer merged by hand; tests `429330919` |
| `6d5b0a182` | the tideshell session, rebased onto our 007 (camera wired, warmth chain, speaker pose, …) | pack-src.sh was missing shared/speaker-pose: the phone build would have been refused |
| `0d5c2a36f` | the tideshell session, continued (idle spin, mimalloc, chaingraph, fixed exposure) | no conflicts |

## 3. tideshell

- Power key: the popup was cosmic-comp's default `System(PowerOff)`; COSMIC reloads
  `custom` shortcuts live (**seen on the FP5**). tideshell now writes its bindings at every
  start (`aee3331fb`; merge by entry, not line: `1ae93fd22`). `tideshell doctor power-key`
  (`84497dea0`) names every consumer. **Open:** a real press through tideshell's own Spawn
  entry, on the phone (DEPLOY-FP5 row 11a).
- Fractional-scale edge (`558a5c642`): **open** on the phone.
- tideshell-settings + cosmic-levers (`40573e518`, `4021ed782`): **open** -- never run in a
  window or on the phone; **not registered** in manifest/projects.toml or the gitmind store.
- Camera: pick_rear takes the first BACK camera, which on the FP5 is the ultra-wide
  (camera@29), not the main one. Whether that is the intended default is a question for the
  tideshell session. DEPLOY-FP5 16a says how to override.
- `tests/frame_cost.rs` (`7c394307d`): building + tessellating a shell frame costs
  microseconds; the phone's frame cost is painting. The CPU blur and tile cache are not called
  from anywhere yet.
- **Open:** ext-session-lock-v1 (a real lock); DEPLOY-FP5 row 22 (the idle fixes, measured
  after); the podman image has no explicit C compiler and mimalloc needs one (Alpine's `rust`
  should pull in gcc).

## 4. gitmind

- join server (`6b783011e`, `b5e0dcfca`): first contact from a browser; docs in
  `apps/gitmind/DEPLOY_TO_A_PHONE.md`. **Open:** a phone; busybox `sha256sum -c -`.
- depot client + `gitmind depot` CLI: live-tested against a local `depot serve`. **Open:**
  approval over ssh; the Depot and Patchsets screens in gitmind-egui.
- patchsets (`46e57133a`): package() now installs what cosmic-comp's make install does, and
  rollback reinstalls the kept package. **Open:** `apk fetch NAME=VERSION` on a real device;
  the rollout has never run on one.

## 5. fao-codec

- Encode page (default tab) and Archive page; after the hive merge the Encode page runs the
  streaming compact/fast transcoder. **Lost in the merge:** its live per-frame stats and
  decoded preview (the new transcoder's progress callback reports counts only).
- Library fixes: T-frame chroma (`3e2a9fa33`), archive manifest panics (`290ee0c3b`), damaged
  .fao abort + off-by-one (`54ae9491c`, test `a74155cab`: 46,330 damaged files, 0 panics).
- **Open:** the web view; codec versions v2.0–v8 / v9; the compact encoder's speed (0.4–0.6x
  fast). Hypothesis NOT tested: motion.rs `sad()` clamps and bounds-checks every pixel, which
  cannot vectorise; an interior fast path would. Profile first -- the early exit may mean it
  rarely runs in full. A profiling target test was written and not committed.

## 6. depot, crypto

- Pairing documented in MANUAL section 7 (`4a092d4fd`). One CSPRNG for nonces and tokens
  (`48dfc99f4`, `4a13080a3`); two write helpers instead of four (`9294e5dce`).
- **Open:** `stored_ingest_paths_match_file_ingest_and_reject_cleanly` failed once under load,
  then passed 28 times; no message captured. `write_atomic`'s temp name is unique per process,
  not per thread (no caller writes one path from two threads today).

## 7. The repo's own checkers (bugs found in them)

- textedit_id_check was not run on the hive merge: 16 Hive-tab fields pinned (`a8d74770f`).
- orphan_check: include!("x.rs") and examples/ mounts were false positives (`b2baf41b8`).
- lock_check never read workspace MEMBERS (`8d097623b`); it then found bo-suite and cadscope
  locks that `cargo --locked` refused (`25fad33e5`).
- fleet_build: whisper-rs-sys / audiopus_sys build-script failures are missing tools, SKIP
  (`18573e5ba`).

**Still flagged, left for a decision:** orphan_check's five real orphans (gpu-gemm
kernels.rs by design; unendlich bloom.rs/postfx.rs parked; snapclone camera-bridge
shared_frames.rs in both trees, see SNAPCLONE_TWO_TREES.md); cfg_feature_check: rusty-board's
`llm-swipe-ffi` (9 arms no manifest declares, since June); about a dozen hand-written atomic /
private file writes across apps (only depot's were consolidated); uikit-egui
`callout::body_text_stays_readable_on_every_tinted_surface_in_every_palette` fails
(slate/Error 2.93:1), pre-existing.

## 8. The fleet sweep (incomplete)

`fleet_build.sh` app by app. Done: ambient-learner (160 tests), av1tuner (167), bot-arena
(39), cadscope, bo-suite (Qt, skipped); aperture and beatspace were the missing-tool false
failures. **bscale needs a re-run** (its script was edited mid-run). The rest of `apps/`, and
all of `shared/`, `games/`, `system/`, `tools/`, were not reached. The sandbox only runs while
a session is active, so an "overnight" run advances only during tool calls.

## 9. Worth doing next, in the method that found things this session

Point the damaged-file harness (fao `decode.rs`) at the other parsers of user-chosen files:
depot catalog and packs, gitmind bundle and manifest readers, eink-kit's tablet surveys.
Both parser bugs found this session were found that way, and neither was reachable by any
existing test.

## 10. unendlich (after the first write)

- **Its tests run again.** The engine's test crate builds in a 3 GB sandbox with ONE build
  job (rustc peaks ~2.6 GB, ~3.0 GB with default features) -- the likely reason sessions saw
  it "die". Found and fixed on first run: an uncompiled proposal shader in `src/shaders/`
  (`08ffc680f`, now `shaders_proposed/`); the GPU generator's Dome canopy still a cone after
  the Rust side became a dome (`ad9f3d4e6`, shader + `.spv` regenerated); a test pinning the
  stuck-button latch (`2bbc77224`); the eink height cache's counter ordering (`206b737f2`).
- **The default build compiles** (broken since 2026-09-12: ui-overlay-only items ungated,
  `f19420473`), and so does every target with defaults AND with `ui-overlay` (`0de339fe2`,
  `99dd6cac0`, `2748218d3`, `a686d7972`, `c656615f7`). bloom/postfx compile again with the
  three render passes they needed (`262fa8dc2`) -- declared, not wired, not run.
- **Stale render tests** read the packed mesh now (`d7bc95360`).
- **First person**: the camera sits in the hero's head (`841d2a4f7`, `b26053a33`); a hero's
  height is its body, not its props or pose (`dec75fb73`); items in the anatomical right hand
  and a carry pose that keeps a held item in view (`61d26091c`, switched on in `044f4147e`).
- **Cloth and waves**: `sim::cloth`, `sim::ripples` (`746a0d3e1`, `708e8b8a0`), `sim::garment`
  and the heroes' capes / cloak / scarf as cloth in the game (`5bcf10406`, `4cd79cbb1`,
  `044f4147e`). `044f4147e` also FIXED a break: `61d26091c` added a HeroPose field the game
  sets by name, and unendlich-fullgame did not compile from `61d26091c` until `044f4147e`
  (exports `-019`, `-020`).
- Verified: engine lib (no-default) 8850 passed; with defaults 9232 passed and exactly the 2
  expected failures (below), measured at `f849dad57`; all
  targets compile both ways. NOTHING seen on screen: no GPU here.
- **Open:**
  - `mesh_tri_regression::sample_grid_worst_case_stays_under_budget` fails ON PURPOSE: one
    chunk is 443,922 triangles, ~410K of them foliage (~800 per stamped leaf block, 512
    stamps per chunk by default). Options in `79ab94223`: the cap, culling faces hidden
    inside a template, skipping enclosed leaves. A decision, not a fix.
  - `mesher_binary::cross_tufts_are_eight_vertices_and_double_sided` (binary mesher, off by
    default): hypothesis in KNOWN-TEST-FAILURES -- the stamp budget is charged before the
    surface checks. Unconfirmed.
  - Gravitybender's carry is out of frame (named in the test); the landing crouch is shallow
    and its chest fold may lean back; `sim::ripples` is not wired to the water yet; the
    wind-to-cloth scale (x3) is a guess; whether the ULTRA-WIDE should be tideshell's
    default camera (pick_rear takes the first back camera).

## 11. snapclone-egui voice

- Calls send Opus, not raw S16LE (`623f42518`, `53195b3b3`; C14_TODO section 14 closed):
  `snapclone-audio` got a safe Rust `VoiceEncoder`/`VoiceDecoder`; the app stopped telling the
  host it had an encoder while it had none. Feature `opus`, default ON: needs libopus
  (`opus-dev` / `libopus-dev`); `--no-default-features --features app` is the raw fallback.
- Arriving voice is played in sequence order, gaps concealed (`f849dad57`): the Opus packet
  carries `voice_send`'s seq and spurt flag (wire kind 4: flags, u32 seq, payload); `voice_rx`
  feeds the ported jitter scheduler (`voice_playout`, which nothing had fed) and decodes /
  conceals per speaker per frame interval. Raw frames from older builds keep their path;
  older builds DROP kind 4 (silence from a newer peer, not noise).
- Verified: 1746 passed with opus; the no-opus build compiles and its voice tests pass.
  Recovery after a loss is gradual (0.60 / 0.71 / 0.80 of the true level over three frames).
- **Open:** a real call between devices, and over a lossy network; in-band FEC is sent but
  not used on receive (one packet of lookahead would rebuild a lost frame).

## 12. Environment notes for the next session here

- 3 GB RAM: build unendlich with `CARGO_BUILD_JOBS=1`. Disk runs short: build caches in
  `/tmp/bd*` are disposable (voxel-tools' 62 examples alone are 3.2 GB in debug).
- Installed this session: cmake, X11 dev headers (xrandr/xinerama/xcursor/xi), libopus-dev,
  glslang-tools, valgrind. With them GLFW, audiopus and the shader build all work here.
- The sandbox stops background jobs between turns: anything that temporarily edits source (a
  mutation check) must start AND restore within one turn. A killed job left `sim/cloth.rs`
  mutated once; it was caught and restored from backup.

## 13. Merged after the merge-ready export (c20unified-024)

Four incremental bundles on `0d5c2a36f`, each merge message recording what it
brought and what was run: clawbound-battleground (`9ece7f372`), vidkit
(`8a5574fcc`), unendlich dragons (`a0f907fc0`), tideshell c20unified-011
(`3d620722f`, one conflict in the main loop, both sides kept). The merges were
first done on a scratch branch and tested there; the final branch was rebuilt
with real messages and its tree checked identical to the tested one.
Found and fixed: `shared/ambient-palette` missing from tideshell's pack list
(`162acb640`, a merge interaction) and 31 lockfiles stale on vidkit's branch
alone (`beb59b21b`; the settings app's lock was also a merge interaction).
- **Open from those bundles** (their own notes list more): vidkit's whisper
  path was not built here (no libclang); vidkit-transcribe ran 24 tests
  without `whisper` (its note: 27 with it); clawbound's three pre-existing
  failures; dragons audio / ARM / real-GPU items per their handoff.

