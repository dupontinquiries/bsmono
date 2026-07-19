# gitmind → egui port (main GUI on the kits; Qt/QML as fallback)

## Why + shape
The `cockpit` crate is already the GUI-agnostic backend (24 modules, 117 tests). The Qt frontend
just wraps `cockpit::` calls in JSON over qmetaobject. egui can call cockpit **directly** (both
Rust), *and* egui builds in CI/sandbox (Qt6 doesn't) — so the port is faster to write and actually
verifiable. New crate `crates/gui-egui` (`gitmind-egui`) is the main GUI; `crates/gui` (Qt) stays
as a fallback.

## Architecture (built + compiling on egui 0.34)
- **`app.rs`** — `App { tab, prefs, task, <page states> }`; `ui(ctx)` applies uikit-egui theme +
  `Adaptive::detect` and lays out **adaptive nav** (SidePanel on desktop, bottom TopBottomPanel on
  phone), then dispatches to the current page. Tabs: Fleet / Network / Timeline / Settings.
- **`task.rs`** — `Task`: runs a blocking cockpit call (ssh/build) on a worker thread and polls the
  result, so the frame never freezes. The egui analogue of the Qt shared `Job`. `run(closure)` /
  `is_running()` / `take_result()`.
- **`pages/network.rs`** (reference page, done) — renders the daemon registry from
  `cockpit::daemons::all()`; status + install run via `Task` over `cockpit::flow::*`; spinner +
  `request_repaint` while running; monospace result panel.
- **`main.rs`** — eframe entry behind the `app` feature; the lib builds headless (egui needs no
  display), the window needs `--features app`.

## Pattern for each remaining page (direct cockpit calls)
1. A `State` struct in the page module (text fields, selections, cached results).
2. `pub fn ui(app: &mut App, ui: &mut egui::Ui)` reads/writes `app.<page>` + calls `cockpit::`.
3. Blocking calls → `app.task.run(move || …)`, poll with `take_result()`.
4. Layout via uikit-egui `Adaptive` (columns/pick), touch via `uikit_egui::touch` where useful.

## Ported so far
Network (daemon registry + **per-daemon start/stop/restart, one-shot + live streaming logs, fleet
health rollup**), Settings (animation/palette/text/opacity, live),
**Fleet** (app catalog from store_groups → build+install locally), **Push** (app picker + target +
device + deploy_strategy hint + build & push). Fleet + Push now **stream build/deploy logs live**
(Task::run_logged over the *_logged cockpit variants, stick-to-bottom panel). All on the Task pattern; lib builds on egui 0.34.

## Ported (all six main tabs done)
Fleet, Push, Network, **Apps** (installed apps: run/remove via binpin + run_local), **Timeline**
(CheckpointEngine over git history), Settings. Shell = adaptive nav + theme + Task.

## Ported (8 tabs)
Fleet, Push, Network, Apps, **Import** (bundle/zip via cockpit::flow), **Branches** (native-store
branch list), Timeline, Settings.

## Ported (9 tabs — every standalone Qt page)
Fleet, Push, Network, Apps, Import, Branches, **Files** (browse tree + peek file contents),
Timeline, Settings.

## Done
Warning-clean on egui 0.34 (uikit-egui Context renames + gitmind-egui Panel::* with one documented
allow(deprecated) for eframe's top-level show(ctx)). **DevicePicker** embedded in Push (scan the LAN
via deploy::scan_ssh, pick a device to fill the host).

## Ported (the last Qt flow)
**Duplicates & roll-up** (`pages/dupes.rs`, a card at the bottom of Settings — where QML kept it):
scan roots for duplicate checkouts (`flow::duplicates_all`), pick an (older, newer) pair, preview /
apply the ff-only roll-up, run a merge **exploration** (`flow::explore` → per-strategy `gm-<id>`
trial branches), preview a chosen strategy (`flow::merge_preview` — the action previously listed as
remaining), and land it (`flow::land_merge`). Blocking calls ride the shared Task; structured
results cross Task's single-String channel on a tiny `\x1E/\x1F` wire with unit-tested
encode/decode pairs (8 lib tests total).

## Remaining (minor)
Stale-Bluetooth/advertise items from the old Network list if ever needed; everything else on the
original list has landed. The Qt `gui` crate remains the untouched fallback.

## Notes
- Build headless: `cargo build -p gitmind-egui --lib` (RUSTFLAGS='' if the mold pin bites).
  Runnable: `--features app` (eframe; needs display libs).
- Cleanup: panels currently use `.show(ctx, …)` which 0.34 deprecates in favour of `show_inside`
  for nested cases — revisit for a warning-clean build.
- The Qt `gui` crate is untouched and remains the fallback.
