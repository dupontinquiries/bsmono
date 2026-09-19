# Merge guide — c19unified-015 (stoat)

For whoever merges this drop into another copy of the repo, human or Claude.
Same purpose as `MERGE_GUIDE_026.md` / `_028.md`: what changed, what was
verified and how, and where it can collide. The commit messages carry the detail;
this is the map.

Base: `9acc004fe` (main of `c19unified-013-fleet`). Everything here is linear on
top of it, so against an unmoved main it fast-forwards.

```sh
git bundle verify c19unified-015-*.bundle
git fetch /path/to/c19unified-015-*.bundle refs/heads/main:incoming-stoat
git log --oneline main..incoming-stoat          # review first
git fetch /path/to/bundle 'refs/notes/toolcalls:refs/notes/incoming-toolcalls'
git notes --ref=toolcalls merge incoming-toolcalls   # only if you already have notes
```

This bundle came from a single-branch clone: it has `main`, tags and
`refs/notes/toolcalls`. It does NOT have 013's `origin/bundle/main` or
`origin/bundle/vkmain`. Nothing here touched them.

## Where it can collide

Everything new is under `apps/stoat/` and cannot conflict. Outside it, five files:

| file | change | collides if the other side… |
|---|---|---|
| `shared/uikit/crates/uikit-egui/src/segmented_tabs.rs` | `strip_ui` pins `item_spacing.x = 4`, `button_padding.x = 2`; new `tests` module | …edited `strip_ui` or added a tests module to this file (it had none) |
| `shared/uikit/crates/uikit-egui/src/log_view.rs` | `log_row`: `horizontal_top`, message `Label::wrap()`; one new test | …edited `log_row` |
| `shared/uikit/crates/uikit-egui/src/palette_flow.rs` | one test asserts against the constants; diagram says 5000 | …also fixed that red test (take either; keep exactly one) |
| `manifest/projects.toml` | three entries appended: `stoat`, `stoat-egui` (primary), `stoat-gtk` | …appended at the same place — keep both sides |
| `CLAUDE.md` §3 | GTK4/libadwaita are installable in the sandbox | …edited the same paragraph |

The two kit behaviour changes reach every consumer. Not looked at on screen:
snapclone, bo-suite, bot-arena (tab strip: narrower gaps, underline under the
right tab); drivemind, gitmind (log rows holding a long line get taller instead
of running under the edge). If a merged app's layout test goes red on either,
the test was pinning the old overflow.

## What is in it

`apps/stoat` — a debugger front half over gdb/MI. `crates/mi` (wire parser),
`crates/core` (Backend seam, sans-IO Session, Driver, diagnostics, command
vocabulary, discover/history/build/cache/vitals/procs), `crates/cli` (`stoat`),
`crates/gui-egui` (primary), `crates/gui-gtk` (archived tier). Out of
default-members: both GUIs.

## Verified, and how

All under `maintenance/tools/cargo-ailite test`, all targets, one crate at a
time, x86_64, gdb 15.1, rustc 1.96, egui 0.34:

| crate | result |
|---|---|
| stoat-mi | 5 |
| stoat-core | command 5, driver 1, live_gdb 4, replay 4, services 10 |
| stoat-egui | unit 5, palette 6, phone 4 |
| uikit-egui | lib 543 / 0 failed (it was 541 / 1 on arrival), integration 1 + 5 ignored |

`live_gdb` drives a real gdb: C segfault, Rust panic, a core file written by
gdb's own `generate-core-file`, attach to a running process. Those four, and one
services test, print `NOT RUN` and PASS when gdb/cc/rustc/strip are missing — a
green run on a bare image has not exercised them. Look for the string.

Break-tested (the thing broken, the test confirmed red, the file restored):
12 breaks, 12 caught - 1 in `5d18e8a56`, 4 in `92efe5cf4`, 7 in `c75364404`.
Separately, the two kit tests were written first and seen red against the
unfixed code. NOT break-tested: the core-file and attach live tests.

Looked at, under Xvfb + openbox + llvmpipe: stoat-egui at 400x780 and 1300x750
(panic stop, a real xdotool drag walking frames, Ctrl+P palette, tap and hold on
⌘); stoat-gtk at 420 and 360 px under `GSK_RENDERER=cairo`. Several faults in
this drop were found by a screenshot and by nothing else (tab strip overrun,
pacing measured against the UI's repaint rate, a frame view left offset after a
swipe, palette rows swallowing the screen, a tap on ⌘ doing nothing); each now
has a test or a comment at the site.

Checkers: `verify.sh` (stoat clean), `lock_check`, `orphan_check`,
`eframe_backend_check`, `textedit_id_check`, `font_install_check` pass.
`cfg_feature_check` FAILS on `apps/rusty-board` (`llm-swipe-ffi`) — on arrival,
not from this drop.

## Not verified — do not inherit these as claims

* Nothing on aarch64, musl or a phone. Frame-marker overhead (~2.3 ms a frame)
  is this sandbox's number.
* Touch: every gesture was exercised with mouse events, real or synthetic.
* The `voice` feature type-checks (`cargo ailite check --no-default-features
  --features voice`, library only; needs cmake, libasound2-dev, libclang-dev).
  The binary was never linked with it and it has never heard anything; there is
  no microphone here. What an utterance DOES is tested without the feature.
* Attach / core / Build / history search from the egui WINDOW (their core paths
  are live-tested; the widgets were laid out and probed, not clicked).
* Yama `ptrace_scope=1`: this sandbox has no Yama, so the refusal path is untested.
* stoat-gtk has none of the palette / hold / attach / core work.
* The CLI still has its own command parser instead of `stoat_core::command`, and
  the CLI and GTK shells still carry their own vitals glue instead of `Driver`.

## Placeholders someone has to decide

The name `stoat`, the app id `dev.local.Stoat` (must equal the .desktop basename
once there is one), and the licence: MIT OR Apache-2.0 was chosen to match the
tree; no gdb code is copied.
