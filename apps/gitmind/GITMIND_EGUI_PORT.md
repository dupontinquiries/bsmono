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

## Remaining pages (specialised, not top-level tabs)
Files/PeekViewer (browse + peek), Import (bundle/zip), Branches + merge_preview, DevicePicker
(discovery component used by Push). Network extras (fleet health card, advertise, bluetooth, daemon control/logs stream),
Timeline, Files/PeekViewer, Import, Branches, DevicePicker, Settings. Each is a mechanical
port of an existing QML page onto the pattern above; the cockpit functions all exist already.

## Notes
- Build headless: `cargo build -p gitmind-egui --lib` (RUSTFLAGS='' if the mold pin bites).
  Runnable: `--features app` (eframe; needs display libs).
- Cleanup: panels currently use `.show(ctx, …)` which 0.34 deprecates in favour of `show_inside`
  for nested cases — revisit for a warning-clean build.
- The Qt `gui` crate is untouched and remains the fallback.
