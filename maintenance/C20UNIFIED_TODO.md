# C20UNIFIED_TODO — open threads from the c20unified session

Same convention as `C9`–`C19`: every item says how far it actually got. Named
`C20UNIFIED`, not `C20`: `C20_TODO.md` (and `MERGE_GUIDE_C20.md` at the repo root) came
with the blackstar-rs drop from a DIFFERENT session that also called itself C20.

Session: `0260c59f7..` (50 commits by `c20unified agent`, 4 of them merges), exported as
`c20unified-001` … `-014`. Per-commit detail is in the commit messages; this file is the
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
