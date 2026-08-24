# C10_TODO — open threads from the c10 session

Same convention as `C9_TODO.md`: every item says how far it actually got.
`verified` means it was executed here, `read` means the code was read but not
run, and anything else is a claim nobody has checked. C9_TODO.md is still the
larger backlog and is NOT superseded — it was edited in place this session, so
its status lines are current.

The c10 session did workflow repair, two incoming merges, and one bug fix in
the egui frontends. What follows is what it did not get to.

---

## 0. The four egui/GTK parity gaps — the live request

These came in near the end of c10 and only the fourth was touched. They are the
top of the queue.

- **C10-0a — `voiceterm-egui`: multiple conflicting sidebars → one unified
  sidebar, like the Qt version.** `in-progress` · the state model is DONE and
  `verified` (`2e9a9695`); the appearance pass is not.
  What was actually conflicting: the left edge was already one column
  (`73213ebc`), but every transient surface carried its own bool — settings,
  workspaces and missing_pkg were three right SidePanels that could be open at
  once. The QML gets exclusivity from modality (every such surface is
  `modal: true; dim: true`); egui has none, so `surface.rs` now IS the
  modality as state: zero or one active, all 29 mutation sites routed through
  it, pinned by behavioral tests plus a structural test that bans the old
  flags from the binary. Both checks were broken on purpose and caught it.
  REMAINS (on-device, needs a display): whether these surfaces should be truly
  modal (scrim + input capture, then Esc-to-close becomes safe — see the
  module docs for why it is NOT safe non-modally), and whether settings should
  match the QML's centered popup rather than a right panel.

- **C10-0b — `snapclone-egui`: no camera fullscreen; missing popups/slide-ups
  for voice calls and group snaps.** `not started`.
  Same shape. The headless part is a presentation-state model: framed vs
  fullscreen, which overlay is up, what dismisses it, what happens to an
  incoming call while a group snap is open. That last question is the one worth
  answering in tests, because it is where overlay systems usually break.
  NOTE `apps/snapclone` is the LIVE tree (754 files); `system/snapclone` is the
  stale duplicate — see C9-7a, still an open decision, do not edit both.

- **C10-0c — `fao-codec`: the egui frontend lacks the GTK one's
  functionality.** `not started`. **Suggested first.**
  Before any code: an INVENTORY. Enumerate what `fao_app`/the GTK frontend
  exposes and diff it against `egui-frontend`, feature by feature. That is
  useful output on its own, needs no display, and it is the only way to know
  whether this gap is three missing buttons or a structural difference. "Full
  functionality" is not yet a defined target.

- **C10-0d — `rusty-board-egui`: the cardinal swipe keyboard.**
  `in-progress` · one bug FIXED and `verified` (`084413cd3`).
  Capitalisation was dead 600 ms after launch: drag samples carry
  `ui.input().time * 1000` (ms since the APP started) and the widget stamped its
  drag-start point with a literal `0`, so rebasing did nothing and `duration`
  came out as process uptime. The hairpin arm requires `duration < 600`, and the
  hairpin IS capitalisation in this layout. Rebasing now lives in
  `cardinal_gesture::path_from_samples` and saturates, so a backwards clock step
  cannot wrap `t_ms` and kill every duration-gated arm intermittently.
  **What remains is the rest of the parity gap, which was never assessed.** One
  gesture path in one widget was fixed. Nobody has compared the egui keyboard to
  the GTK one (`src/ui/keyboard_view.rs`, 6176 lines) feature by feature. Do
  that inventory before assuming the remaining defects are small.

---

## 1. The feature sweep — a tool that works, a job half run

- **C10-1a — finish the `games/` slice and run `apps/`.** `in-progress`.
  `maintenance/tools/feature_sweep.sh --only games/` reached **58 of 76 units**
  before being stopped to free the single CPU; the report is resumable and
  `--report` names what was not reached. `apps/` has never been run at all and
  is the largest slice.
  Findings so far, unclassified: `games/unendlich/crates/unendlich-engine` FAIL
  (82 s) — **this one looks real and is unexamined**;
  `games/echelon/client` and `games/unendlich/crates/unendlich-dash` TIMEOUT at
  180 s, which may just need a longer `FEATURE_SWEEP_TIMEOUT`.

- **C10-1b — the skip list is incomplete.** `verified`, trivial fix.
  `games/blackstar-rs[audio]`, `games/skydribble[audio]` and
  `games/soldier[sound]` all FAILed on cpal → alsa-sys, which CLAUDE.md §3
  already documents as an environment block. `audio` and `sound` belong in
  `SKIP_FEATURES` next to `gamepad`. Until they are, the games report contains
  noise, and a checker that reports noise gets ignored.
  `games/skydribble` also FAILs at DEFAULT features for the same reason, so it
  wants a `--no-default-features` row in the `extra_flags` table.

The sweep has already earned its cost: its first complete slice (`shared/`, 56
units) found two configurations that had never been compiled — see C9-2c.

---

## 2. Verified tools whose job has not been run

Three things are in the same state: the tool is proven, the work is not done.
Recorded together so nobody mistakes a tool for an outcome.

- **The full feature sweep.** 420 units, hours. C10-1a above.
- **C9-4a MLA loading.** Blocked on M2 hardware; the forward path is
  unimplemented and this is not a one-commit item.
- **The prefix-cache collision guard** (`f045efd3`). Reasoned-correct but NOT
  test-enforced: deleting it fails nothing, because this fixture's argmax does
  not move on garbage KV. Needs a model sensitive to it, so M2.

---

## 3. Loose ends from c10

- **C10-3a — `probe-bundle.sh` cannot measure a large bundle.** `verified`,
  undiagnosed. On git 2.43.0 here, repacking a scratch that received a lot of
  genuinely new content dies in the `pack-objects` child with
  `bad revision 'pack<TAB><sha>'` (confirmed via `GIT_TRACE`), stdin closed or
  not, with or without `-l`/`-d`/alternates/bitmaps. It exits 3 saying the cost
  was NOT measured rather than reporting a misleading 0 (`815397ac`). Both
  incoming bundles this session are therefore unmeasured.

- **C10-3b — the clawbound bundle was unusable.** `verified`. Incremental,
  prerequisite `e2bdd70c` absent — the `INCOMING_BUNDLE_REBASE.md` case. The
  patch series applied by content instead. Worth telling whoever produces these
  bundles, because the same thing will happen next time: a self-contained
  bundle, or the patch series, both work; an incremental bundle against a
  rewritten history does not.

- **C10-3c — clawbound's balance numbers are close but not reproduced.**
  `verified`. A 784-game pooled run gives clan A exactly 50% and roster spread
  44-56% against their committed 44-55%, but the per-kit outliers differ and
  Windclaw sits 2σ above the figure its own fix claims. The fix is verifiably
  present in the tree, so this reads as sampling — but nobody has matched their
  table, and it should not be written up as "reproduces".

- **C10-3d — two clawbound features exist twice after the merge.** `verified`,
  decision not taken. `mapeval::lane_score` alongside `lane_water_report`, and
  the draft in both `draft.rs` (headless) and `menu.rs` (`Screen::Draft`).
  Both pairs were kept deliberately; collapsing `menu.rs` onto `draft.rs` is the
  obvious follow-up and is nobody's decision yet.

- **C10-3e — the battleground `app.rs` glue is now unblocked and unwritten.**
  `read`. Its seven steps are specified in `games/clawbound/TODO.md` and were
  blocked on rustc >= 1.76; this tree is on 1.96 and the whole winit/wgpu/egui
  stack compiles. `MenuCmd::StartBattleground` and `QuickPlay` currently take an
  honest fallback (say what was picked, reopen the menu) rather than starting a
  match the loop cannot draw.

---

## 4. Things that were true all session and stay true

- **A green default build is not a green crate.** Three separate times something
  was green only because nothing executed it: 28 fixture-gated gpu-gemm tests
  passing silently without `gen-all.sh`, the dupscan ratchet skipped for a whole
  session, and two feature configurations that had never been compiled. Run
  `sh apps/logscope/tools/tiny-variants/gen-all.sh` before believing a gpu-gemm
  pass count, and see CLAUDE.md §3.
- **The capture recorder is fixed** (`bb55960f`) and no longer needs a pinned
  `CAPTURE_LOG`; `--selftest` proves it against a fresh-shell harness. The
  harness shell here is `dash`, so work still needs wrapping in
  `bash -c 'source maintenance/tools/session_capture.sh && …'`.
