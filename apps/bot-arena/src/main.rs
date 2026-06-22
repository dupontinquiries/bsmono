//! main.rs — GTK4 + libadwaita front-end for the population-based agent arena.
//!
//! Three pages in an adw::ViewStack:
//!   Setup — sliders (population, workers/container, keep %, generations), backend
//!           selector, container image + image import, tool list, workspace picker.
//!   Arena — a FlowBox of one cell per bot, colored by lifecycle status, with fitness.
//!   Logs  — a scrolling monospace transcript of everything the engine reports.
//!
//! The engine runs on a worker thread (see engine.rs) and streams EngineMsg back
//! over an async-channel; the GUI drains it with glib::spawn_future_local. Widgets
//! never leave the main thread; the engine only ever sees the plain Config struct.

mod engine;
#[path = "fsm.rs"]
mod fsm;
mod heroes;
mod inspect;
mod models;
mod llama_backend;
mod registry;
mod replays;
use engine::{BackendKind, BotStatus, Config, EngineMsg};

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use gtk::prelude::*;
use adw::prelude::*;
use gtk::{gio, glib};

const APP_ID: &str = "org.example.BotArena";

/// A compact unix-seconds stamp for naming cached models.
fn chrono_stamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

const CSS: &str = "
.bot-cell { border-radius: 6px; margin: 2px; padding: 4px; min-width: 52px;
            transition: background-color 220ms ease, box-shadow 260ms ease, opacity 200ms ease; }
.bot-idle { background-color: #4b4b4b; }
.bot-prov { background-color: #5a4b00; }
.bot-run  { background-color: #1c5fa8; animation: bot-pulse 1.15s ease-in-out infinite; }
.bot-pass { background-color: #1f7a3d; box-shadow: 0 0 7px 1px rgba(46,204,113,0.55); }
.bot-fail { background-color: #7a2f2f; }
.bot-cull { background-color: #262626; opacity: 0.55; }
.bot-spawn{ background-color: #8a7a1f; animation: bot-spawn-in 360ms ease-out; }
@keyframes bot-pulse { 0% { opacity: 0.66; } 50% { opacity: 1.0; } 100% { opacity: 0.66; } }
@keyframes bot-spawn-in { 0% { opacity: 0.2; background-color: #b8a52f; } 100% { opacity: 1.0; background-color: #8a7a1f; } }
.bot-id { font-size: 9px; color: #cfcfcf; }
.bot-fit { font-size: 11px; color: #ffffff; font-weight: bold; }
.arena-status { font-size: 13px; padding: 8px; transition: color 200ms ease; }
.logview { font-family: monospace; font-size: 12px; }
.activity-view { font-family: monospace; font-size: 11px; }
";

/// A single bot's widgets, held on the main thread for in-place updates.
struct BotCell {
    root: gtk::Box,
    fit: gtk::Label,
}

fn main() -> glib::ExitCode {
    // Headless mode: `bot-arena --headless --workspace PATH [opts]` runs the swarm
    // from the command line with no GUI (for scripts, CI, or driving the engine from
    // another program). Falls through to the GTK app when --headless is absent.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--headless" || a == "-H") {
        return run_headless(&args);
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_cli_help();
        return glib::ExitCode::SUCCESS;
    }
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_ui);
    app.connect_shutdown(|_| engine::cleanup_workspace_temp());
    app.run()
}

fn print_cli_help() {
    eprintln!(
"bot-arena — population-based code-repair swarm

GUI:       bot-arena
Headless:  bot-arena --headless --workspace PATH [options]

Options (headless):
  --workspace PATH     repo/crate to evolve (repeatable; not needed for --backend sim)
  --backend NAME       sim | local | docker | conda   (default: local)
  --test-cmd CMD       test command, or 'auto' to detect cargo/pytest/etc (default: auto)
  --generations N      number of generations            (default: 6)
  --population N        bots per generation              (default: 24)
  --niches N           latent purposes; >1 enables the coordinator (default: 1)
  --recurrent N        hill-climb N steps per bot (RNN)  (default: 40)
  --batch N            best-of-k attempts per bot, anneals to 1 (default: 1)
  --keep FRAC          survivor fraction 0..1            (default: 0.5)
  --novelty LAMBDA     quality-diversity bonus weight    (default: 0.0)
  --drift RATE         Sim only: moving-target rate 0..1 (default: 0.0)
  --aggression FRAC    edit magnitude 0..1               (default: 0.3)
  --differential       score by fixed−2×broke vs a pristine baseline
  --proposer-cmd CMD   model seam: a command that reads JSON context on stdin and
                       writes a unified diff on stdout (replaces random mutation)
  --escalate-cmd CMD   escalation tier (repeatable): tried in order when the primary
                       proposer doesn't fully solve a case — fast model first, then
                       the strong/slow model only where needed (a cost cascade)
  --context-cmd CMD    context enrichment stage (repeatable): a context→context filter
                       run once before the proposer — e.g. RAG retrieval of prior fixes,
                       or a planner that injects a high-level plan. Stages run in order.
  --seed N             fixed RNG seed (reproducible)
  --threads N          parallel workers                  (default: 4)
  --min-fitness F      exit nonzero unless best fitness ≥ F (for CI/gating)
  --json               emit one JSON object per line (log/generation/done) for piping
  --no-cache           disable the shared build cache
  --no-crossover       disable genome crossover
  --allow-test-edits   let bots modify tests (off by default; reward-hack tripwire)
  --features LIST      cargo --features list
  -h, --help           this help

Examples:
  bot-arena --headless --backend sim --generations 200 --drift 0.5 --niches 5 --json
  bot-arena --headless -w ./mycrate --generations 20 --differential --min-fitness 1.0");
}

/// Drive run_engine to completion with no GUI, printing progress to stdout.
fn run_headless(args: &[String]) -> glib::ExitCode {
    let val = |flag: &str| -> Option<String> {
        args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
    };
    let has = |flag: &str| args.iter().any(|a| a == flag);

    // workspaces: every --workspace occurrence
    let mut workspaces: Vec<PathBuf> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--workspace" || args[i] == "-w" {
            if let Some(p) = args.get(i + 1) { workspaces.push(PathBuf::from(p)); }
            i += 2;
        } else { i += 1; }
    }
    let backend = match val("--backend").as_deref() {
        Some("sim") => BackendKind::Sim,
        Some("docker") => BackendKind::Docker,
        Some("conda") => BackendKind::Conda,
        _ => BackendKind::Local,
    };
    // Sim is a synthetic landscape and needs no repo; real backends do.
    if workspaces.is_empty() && backend != BackendKind::Sim {
        eprintln!("error: --backend {:?} needs at least one --workspace PATH (Sim needs none; see --help)",
            backend);
        return glib::ExitCode::FAILURE;
    }
    let json = has("--json");
    // escalation chain: every --escalate-cmd, in order (tried after the primary proposer)
    let mut fallbacks: Vec<String> = Vec::new();
    let mut ctx_cmds: Vec<String> = Vec::new();
    {
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--escalate-cmd" {
                if let Some(c) = args.get(i + 1) { fallbacks.push(c.clone()); }
                i += 2;
            } else if args[i] == "--context-cmd" {
                if let Some(c) = args.get(i + 1) { ctx_cmds.push(c.clone()); }
                i += 2;
            } else { i += 1; }
        }
    }
    let num = |flag: &str, default: usize| -> usize {
        val(flag).and_then(|s| s.parse().ok()).unwrap_or(default)
    };
    let fnum = |flag: &str, default: f64| -> f64 {
        val(flag).and_then(|s| s.parse().ok()).unwrap_or(default)
    };

    let cfg = Config {
        workspaces,
        backend,
        test_cmd: val("--test-cmd").unwrap_or_else(|| "auto".into()),
        generations: num("--generations", 6),
        population: num("--population", 24),
        recurrent_steps: num("--recurrent", 40),
        keep_fraction: fnum("--keep", 0.5),
        seed: val("--seed").and_then(|s| s.parse().ok()),
        threads: num("--threads", 4),
        niches: num("--niches", 1).max(1),
        coordinator: num("--niches", 1) > 1,
        batch_start: num("--batch", 1),
        novelty_lambda: fnum("--novelty", 0.0),
        drift: fnum("--drift", 0.0),
        aggression: fnum("--aggression", 0.3),
        differential: has("--differential"),
        proposer_cmd: val("--proposer-cmd").unwrap_or_default(),
        proposer_fallbacks: fallbacks,
        context_cmds: ctx_cmds,
        crossover: !has("--no-crossover"),
        cache: !has("--no-cache"),
        allow_test_edits: has("--allow-test-edits"),
        cargo_features: val("--features").unwrap_or_default(),
        turbo: true,
        memoize: true,
        ..Config::default()
    };
    let min_fitness = val("--min-fitness").and_then(|s| s.parse::<f64>().ok());

    if !json {
        eprintln!("[arena] backend={:?} targets={} gens={} pop={} niches={} drift={:.2} novelty={:.2} seed={:?}",
            cfg.backend, cfg.workspaces.len(), cfg.generations, cfg.population, cfg.niches, cfg.drift, cfg.novelty_lambda, cfg.seed);
    }

    let (tx, rx) = async_channel::unbounded::<EngineMsg>();
    let stop = Arc::new(AtomicBool::new(false));
    let axis = Arc::new(AtomicU32::new(0));
    let cfg_summary = (cfg.backend, cfg.generations, cfg.population);
    let handle = std::thread::spawn(move || engine::run_engine(cfg, tx, stop, axis));

    // Drain messages synchronously until Done. `--json` emits one JSON object per line
    // (log/generation/done) for a program to consume; otherwise human-readable text.
    let je = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ");
    let mut best_overall = 0.0f64;
    let mut last_mean = 0.0f64;
    let mut gens_done = 0usize;
    loop {
        match rx.recv_blocking() {
            Ok(EngineMsg::Log(s)) => {
                if json { println!("{{\"type\":\"log\",\"msg\":\"{}\"}}", je(&s)); }
                else { println!("{s}"); }
            }
            Ok(EngineMsg::Generation { n, best, mean, alive, .. }) => {
                best_overall = best_overall.max(best);
                last_mean = mean; gens_done = n;
                if json { println!("{{\"type\":\"generation\",\"gen\":{n},\"best\":{best:.4},\"mean\":{mean:.4},\"alive\":{alive}}}"); }
                else { println!("[gen {n}] best={best:.3} mean={mean:.3} alive={alive}"); }
            }
            Ok(EngineMsg::Done) => break,
            Ok(_) => {} // Bot/Activity/Replay/Coordinator: not surfaced in headless
            Err(_) => break,
        }
    }
    let _ = handle.join();
    let (bk, _g, _p) = cfg_summary;
    let met = min_fitness.map(|m| best_overall + 1e-9 >= m).unwrap_or(true);
    if json {
        println!("{{\"type\":\"done\",\"backend\":\"{:?}\",\"generations\":{gens_done},\"best\":{best_overall:.4},\"mean\":{last_mean:.4},\"met_min_fitness\":{met}}}", bk);
    } else {
        println!("[arena] done — {gens_done} generations, best fitness {best_overall:.3}, mean {last_mean:.3}");
        if let Some(m) = min_fitness {
            println!("[arena] --min-fitness {m:.3}: {}", if met { "met ✓" } else { "NOT met ✗" });
        }
    }
    engine::cleanup_workspace_temp();
    // Exit 0 on clean completion (and on meeting --min-fitness if set); nonzero only
    // when an explicit fitness gate was requested and missed.
    if met { glib::ExitCode::SUCCESS } else { glib::ExitCode::FAILURE }
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Build a labeled horizontal slider inside an ActionRow.
/// Exponential mapping for the population slider: 1..3000 (the grid gets heavy
/// well before the top, but the engine handles large populations fine).
fn pop_from_slider(s: f64) -> usize {
    (50000f64.powf(s.clamp(0.0, 1.0))).round().max(1.0) as usize
}
fn slider_from_pop(p: usize) -> f64 {
    ((p.max(1) as f64).ln() / 50000f64.ln()).clamp(0.0, 1.0)
}

/// Exponential mapping for the generations slider: a smooth 0..1 slider maps to
/// 1..1,000,000 generations (decimal slider steps, integer generation readout).
fn gens_from_slider(s: f64) -> usize {
    (1_000_000f64.powf(s.clamp(0.0, 1.0))).round().max(1.0) as usize
}
fn slider_from_gens(g: usize) -> f64 {
    ((g.max(1) as f64).ln() / 1_000_000f64.ln()).clamp(0.0, 1.0)
}

fn slider_row(title: &str, min: f64, max: f64, step: f64, val: f64, digits: i32)
    -> (adw::ActionRow, gtk::Scale)
{
    let row = adw::ActionRow::builder().title(title).build();
    // Scale and SpinButton share one Adjustment, so dragging the slider updates the
    // number field and typing an exact value moves the slider — surgical accuracy.
    let adj = gtk::Adjustment::new(val, min, max, step, step, 0.0);
    let scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(&adj));
    scale.set_hexpand(true);
    scale.set_size_request(220, -1);
    scale.set_draw_value(false); // the number lives in the spin button now
    scale.set_digits(digits);
    scale.set_valign(gtk::Align::Center);
    let spin = gtk::SpinButton::new(Some(&adj), step, digits.max(0) as u32);
    spin.set_valign(gtk::Align::Center);
    spin.set_width_chars(7);
    spin.set_numeric(true);
    row.add_suffix(&scale);
    row.add_suffix(&spin);
    (row, scale)
}

fn append_log(view: &gtk::TextView, line: &str) {
    let buf = view.buffer();
    let mut end = buf.end_iter();
    buf.insert(&mut end, line);
    let mut end = buf.end_iter();
    buf.insert(&mut end, "\n");
    let mut end = buf.end_iter();
    view.scroll_to_iter(&mut end, 0.0, false, 0.0, 0.0);
}

fn read_tools(rows: &Rc<RefCell<Vec<adw::EntryRow>>>) -> Vec<String> {
    rows.borrow()
        .iter()
        .map(|r| r.text().trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn rebuild_grid(flow: &gtk::FlowBox, cells: &Rc<RefCell<Vec<BotCell>>>, n: usize) {
    while let Some(child) = flow.first_child() {
        flow.remove(&child);
    }
    let mut v = cells.borrow_mut();
    v.clear();
    for id in 0..n {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_css_classes(&["bot-cell", "bot-idle"]);
        root.set_halign(gtk::Align::Center);
        let id_l = gtk::Label::new(Some(&format!("{id}")));
        id_l.add_css_class("bot-id");
        let fit = gtk::Label::new(Some("·"));
        fit.add_css_class("bot-fit");
        root.append(&id_l);
        root.append(&fit);
        flow.insert(&root, -1);
        v.push(BotCell { root, fit });
    }
}

/// Append one editable tool row (a binary name) with its own delete button.
/// Takes its dependencies explicitly so it needs no captured environment.
fn add_tool_row(
    group: &adw::PreferencesGroup,
    tool_rows: &Rc<RefCell<Vec<adw::EntryRow>>>,
    initial: &str,
) {
    let row = adw::EntryRow::builder().title("binary").build();
    row.set_text(initial);
    let del = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .valign(gtk::Align::Center)
        .build();
    del.add_css_class("flat");
    row.add_suffix(&del);
    group.add(&row);
    tool_rows.borrow_mut().push(row.clone());

    let group2 = group.clone();
    let tool_rows2 = tool_rows.clone();
    let row2 = row.clone();
    del.connect_clicked(move |_| {
        group2.remove(&row2);
        tool_rows2.borrow_mut().retain(|r| r != &row2);
    });
}

fn build_ui(app: &adw::Application) {
    // ---- shared state --------------------------------------------------------
    let cells: Rc<RefCell<Vec<BotCell>>> = Rc::new(RefCell::new(Vec::new()));
    let tool_rows: Rc<RefCell<Vec<adw::EntryRow>>> = Rc::new(RefCell::new(Vec::new()));
    let workspace_path: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let corpus_path: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    // Latent-axis knob, shared with the engine; stores an f32 in [0,1] as bits.
    let axis = Arc::new(AtomicU32::new(0.5f32.to_bits()));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Bot Arena")
        .default_width(1120)
        .default_height(760)
        .width_request(720)
        .height_request(520)
        .build();

    // ---- view stack ----------------------------------------------------------
    let stack = adw::ViewStack::new();

    // ===== SETUP PAGE =========================================================
    let setup = adw::PreferencesPage::new();

    let pop_group = adw::PreferencesGroup::builder().title("Population").build();
    // Population: exponential slider (1..3000) with a live computed readout.
    let pop_row = adw::ActionRow::builder().title("Population count").build();
    let pop_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.001);
    pop_scale.set_value(slider_from_pop(100));
    pop_scale.set_hexpand(true);
    pop_scale.set_size_request(260, -1);
    pop_scale.set_draw_value(false);
    pop_scale.set_valign(gtk::Align::Center);
    let pop_value = gtk::Entry::new();
    pop_value.set_text("100");
    pop_value.set_width_chars(8);
    pop_value.set_max_width_chars(8);
    pop_value.set_valign(gtk::Align::Center);
    {
        // slider → entry
        let pop_value = pop_value.clone();
        pop_scale.connect_value_changed(move |s| {
            let v = pop_from_slider(s.value()).to_string();
            if pop_value.text() != v.as_str() { pop_value.set_text(&v); }
        });
    }
    {
        // entry → slider (on Enter)
        let pop_scale = pop_scale.clone();
        pop_value.connect_activate(move |e| {
            if let Ok(v) = e.text().trim().parse::<usize>() {
                pop_scale.set_value(slider_from_pop(v.clamp(1, 50000)));
            }
        });
    }
    pop_row.add_suffix(&pop_scale);
    pop_row.add_suffix(&pop_value);
    let (wpc_row, wpc_scale) = slider_row("Workers per container", 1.0, 64.0, 1.0, 10.0, 0);
    let (keep_row, keep_scale) = slider_row("Keep fraction (survivors)", 0.05, 0.9, 0.05, 0.30, 2);
    // Generations: exponential slider (1..50000) with a live computed readout.
    let gen_row = adw::ActionRow::builder().title("Generations").build();
    let gen_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.001);
    gen_scale.set_value(slider_from_gens(20));
    gen_scale.set_hexpand(true);
    gen_scale.set_size_request(260, -1);
    gen_scale.set_draw_value(false);
    gen_scale.set_valign(gtk::Align::Center);
    let gen_value = gtk::Entry::new();
    gen_value.set_text("20");
    gen_value.set_width_chars(8);
    gen_value.set_max_width_chars(8);
    gen_value.set_valign(gtk::Align::Center);
    {
        let gen_value = gen_value.clone();
        gen_scale.connect_value_changed(move |s| {
            let v = gens_from_slider(s.value()).to_string();
            if gen_value.text() != v.as_str() { gen_value.set_text(&v); }
        });
    }
    {
        let gen_scale = gen_scale.clone();
        gen_value.connect_activate(move |e| {
            if let Ok(v) = e.text().trim().parse::<usize>() {
                gen_scale.set_value(slider_from_gens(v.clamp(1, 1_000_000)));
            }
        });
    }
    gen_row.add_suffix(&gen_scale);
    gen_row.add_suffix(&gen_value);
    pop_group.add(&pop_row);
    pop_group.add(&wpc_row);
    pop_group.add(&keep_row);
    pop_group.add(&gen_row);
    setup.add(&pop_group);

    let env_group = adw::PreferencesGroup::builder().title("Compute backend").build();
    let backend_row = adw::ComboRow::builder().title("Backend").build();
    let backend_model = gtk::StringList::new(&["Simulation (no deps)", "Docker", "Conda", "Local (host)"]);
    backend_row.set_model(Some(&backend_model));
    backend_row.set_selected(0);
    env_group.add(&backend_row);

    let image_row = adw::EntryRow::builder().title("Container image").build();
    image_row.set_text("python:3.12-slim");
    env_group.add(&image_row);

    let testcmd_row = adw::EntryRow::builder().title("Test command ('auto' detects cargo/pytest/ctest/meson/npm)").build();
    testcmd_row.set_text("auto");
    env_group.add(&testcmd_row);

    let features_row = adw::EntryRow::builder().title("Cargo features (comma-separated, blank = none)").build();
    env_group.add(&features_row);

    let buildflags_row = adw::EntryRow::builder().title("Extra test args (appended to auto cargo/pytest/npm/go)").build();
    env_group.add(&buildflags_row);

    let extratools_row = adw::EntryRow::builder().title("Extra shelter tools (comma-sep, e.g. qmake,moc,rcc for Qt)").build();
    env_group.add(&extratools_row);

    let cache_row = adw::SwitchRow::builder()
        .title("Build cache (ccache / shared cargo target)")
        .subtitle("Reuse identical compilations across the swarm")
        .build();
    cache_row.set_active(true);
    env_group.add(&cache_row);

    let fastclone_row = adw::SwitchRow::builder()
        .title("Fast clone (git worktrees)")
        .subtitle("Per-bot worktrees for git repos — no full copy (HEAD state)")
        .build();
    fastclone_row.set_active(true);
    env_group.add(&fastclone_row);

    let (timeout_row, timeout_scale) = slider_row("Per-command timeout (s)", 5.0, 600.0, 5.0, 60.0, 0);
    timeout_row.set_subtitle("Each test command's wall-clock limit. Cargo needs 15-60s even on small projects; \
        too low and every bot will silently score 0 (tests killed before finishing).");
    env_group.add(&timeout_row);

    let (maxfile_row, maxfile_scale) = slider_row("Max output file (MB)", 10.0, 2000.0, 10.0, 100.0, 0);
    env_group.add(&maxfile_row);

    let memoize_row = adw::SwitchRow::builder()
        .title("Reuse scores for repeated repo states")
        .subtitle("Hash the worktree; skip re-testing states already scored")
        .build();
    memoize_row.set_active(true);
    env_group.add(&memoize_row);

    let crossover_row = adw::SwitchRow::builder()
        .title("Recombine survivors (hybrids)")
        .subtitle("Sometimes blend two survivors instead of mutating one")
        .build();
    crossover_row.set_active(true);
    env_group.add(&crossover_row);

    let sandbox_row = adw::SwitchRow::builder()
        .title("Shelter mode")
        .subtitle("No network; writes confined to the worktree; only allowed binaries on PATH")
        .build();
    sandbox_row.set_active(true);
    env_group.add(&sandbox_row);

    let (threads_row, threads_scale) = slider_row("Parallel workers (real backends)", 1.0, 256.0, 1.0, 1.0, 0);
    threads_row.set_subtitle("Concurrent worktrees, each with its own target dir");
    env_group.add(&threads_row);

    let (aggr_row, aggr_scale) = slider_row("Edit aggression", 0.0, 1.0, 0.01, 0.3, 2);
    aggr_row.set_subtitle("Timid (tiny edits) → aggressive (big blocks, more random chars)");
    env_group.add(&aggr_row);

    let (batch_row, batch_scale) = slider_row("Batch size (best-of-k, anneals to 1)", 1.0, 10000.0, 1.0, 1.0, 0);
    batch_row.set_subtitle("Attempts per bot per gen, with in-place worktree restore + workspace dedup");
    env_group.add(&batch_row);
    let (recur_row, recur_scale) = slider_row("Recurrent steps (0=off → RNN hill-climb per bot)", 0.0, 200.0, 1.0, 0.0, 0);
    recur_row.set_subtitle("Each bot iterates: mutate → score → keep-if-improves, accumulating edits");
    env_group.add(&recur_row);

    let turbo_row = adw::SwitchRow::builder()
        .title("Turbo (skip animation delays)")
        .subtitle("Removes per-generation pacing for fast / headless iteration")
        .build();
    turbo_row.set_active(false);
    env_group.add(&turbo_row);

    let (novelty_row, novelty_scale) = slider_row("Novelty bonus (quality-diversity)", 0.0, 0.3, 0.01, 0.0, 2);
    novelty_row.set_subtitle("Selection = fitness + λ·distance to recent survivors. Display fitness unchanged.");
    env_group.add(&novelty_row);

    let (drift_row, drift_scale) = slider_row("Sim target drift", 0.0, 1.0, 0.01, 0.0, 2);
    drift_row.set_subtitle("Simulation only: 0 = static optimum (converges then plateaus); >0 = optimum keeps moving so the swarm must keep adapting.");
    env_group.add(&drift_row);

    let resume_row = adw::SwitchRow::builder()
        .title("Resume from latest checkpoint")
        .subtitle("Load the previous run's final population if a checkpoint exists")
        .build();
    resume_row.set_active(false);
    env_group.add(&resume_row);

    let differential_row = adw::SwitchRow::builder()
        .title("Differential scoring (fixed − 2×broke)")
        .subtitle("Reward fixing baseline-failing tests; penalize regressions harder. Real backends.")
        .build();
    differential_row.set_active(false);
    env_group.add(&differential_row);

    let seed_row = adw::EntryRow::builder().title("Seed (blank = random)").build();
    env_group.add(&seed_row);

    // Image import: pick a .tar and (under Docker) `docker load` it.
    let import_row = adw::ActionRow::builder()
        .title("Import image")
        .subtitle("Load a .tar to define available tools (docker load)")
        .build();
    let import_btn = gtk::Button::builder().label("Import…").valign(gtk::Align::Center).build();
    import_row.add_suffix(&import_btn);
    env_group.add(&import_row);

    // Workspace importer.
    let ws_row = adw::ActionRow::builder()
        .title("Workspace")
        .subtitle("No git repo selected")
        .build();
    let ws_btn = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    // One-click restore of the most recently used workspace from a prior session.
    let last_ws = registry::load_last_workspace();
    if let Some(ref lw) = last_ws {
        let last_btn = gtk::Button::builder()
            .label("↺ Last")
            .valign(gtk::Align::Center)
            .tooltip_text(lw.display().to_string())
            .build();
        last_btn.add_css_class("flat");
        {
            let ws_row = ws_row.clone();
            let workspace_path = workspace_path.clone();
            let lw = lw.clone();
            last_btn.connect_clicked(move |_| {
                ws_row.set_subtitle(&lw.display().to_string());
                *workspace_path.borrow_mut() = Some(lw.clone());
            });
        }
        ws_row.add_suffix(&last_btn);
    }
    ws_row.add_suffix(&ws_btn);
    env_group.add(&ws_row);
    setup.add(&env_group);

    // Tool list (editable list of binaries).
    let tools_group = adw::PreferencesGroup::builder()
        .title("Tools (allowed binaries)")
        .description("Binaries the agent may call inside the container")
        .build();
    let add_tool_btn = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .valign(gtk::Align::Center)
        .build();
    add_tool_btn.add_css_class("flat");
    tools_group.set_header_suffix(Some(&add_tool_btn));
    setup.add(&tools_group);

    // Seed with sensible defaults, including the bundled helper binaries.
    for t in ["cargo", "pytest", "git", "grep", "sed", "cat", "head", "tail",
              "bracecheck", "astsketch", "funcscan", "treelite", "loc", "todos",
              "jsoncheck", "findtext", "lexscan", "symfind"] {
        add_tool_row(&tools_group, &tool_rows, t);
    }
    {
        let tools_group = tools_group.clone();
        let tool_rows = tool_rows.clone();
        add_tool_btn.connect_clicked(move |_| add_tool_row(&tools_group, &tool_rows, ""));
    }

    // ----- Edit operators (the "edit commands") ------------------------------
    let edit_group = adw::PreferencesGroup::builder()
        .title("Edit operators")
        .description("Mutations a worker may apply — toggle to shape the search space")
        .build();
    let mut op_switches: Vec<(String, adw::SwitchRow)> = Vec::new();
    for name in engine::EDIT_OPS {
        let row = adw::SwitchRow::builder().title(*name).build();
        row.set_active(true);
        edit_group.add(&row);
        op_switches.push((name.to_string(), row));
    }
    setup.add(&edit_group);

    // ----- Latent space / purpose --------------------------------------------
    let latent_group = adw::PreferencesGroup::builder()
        .title("Latent space (purpose)")
        .description("Ground the swarm in a corpus; each worker gets a purpose coordinate")
        .build();

    // Pick a corpus from the shared registry (timestamped list), or browse manually.
    let reg_entries = registry::list_corpora();
    let reg_row = adw::ComboRow::builder()
        .title("Corpus (registry)")
        .subtitle("Pick a prepared corpus, or browse below")
        .build();
    let mut reg_labels: Vec<String> = vec!["(none)".to_string()];
    reg_labels.extend(reg_entries.iter().map(|e| format!("{}  —  {}", e.name, e.artifact_summary())));
    let reg_label_refs: Vec<&str> = reg_labels.iter().map(|s| s.as_str()).collect();
    let reg_model = gtk::StringList::new(&reg_label_refs);
    reg_row.set_model(Some(&reg_model));
    latent_group.add(&reg_row);

    let corpus_row = adw::ActionRow::builder()
        .title("Corpus")
        .subtitle("None — purposes will be generic")
        .build();
    let corpus_btn = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    corpus_row.add_suffix(&corpus_btn);
    latent_group.add(&corpus_row);

    // Selecting a registry corpus sets the corpus path to that entry's corpus dir.
    {
        let corpus_path = corpus_path.clone();
        let corpus_row = corpus_row.clone();
        let entries = reg_entries.clone();
        reg_row.connect_selected_notify(move |r| {
            let idx = r.selected();
            if idx == 0 {
                *corpus_path.borrow_mut() = None;
                corpus_row.set_subtitle("None — purposes will be generic");
            } else if let Some(entry) = entries.get((idx - 1) as usize) {
                let dir = entry.corpus_dir();
                corpus_row.set_subtitle(&dir.display().to_string());
                *corpus_path.borrow_mut() = Some(dir);
            }
        });
    }

    let (niche_row, niche_scale) = slider_row("Behavior niches", 2.0, 32.0, 1.0, 12.0, 0);
    latent_group.add(&niche_row);

    let coord_row = adw::SwitchRow::builder()
        .title("Coordinator learns the axis")
        .subtitle("Hill-climb the latent axis toward the best-rewarding purpose")
        .build();
    latent_group.add(&coord_row);

    let (axis_row, axis_scale) = slider_row("Latent axis (purpose focus)", 0.0, 1.0, 0.01, 0.5, 2);
    latent_group.add(&axis_row);
    setup.add(&latent_group);

    // Live knob: the scrollbar drives the shared atomic the engine reads each gen.
    {
        let axis = axis.clone();
        axis_scale.connect_value_changed(move |s| {
            axis.store((s.value() as f32).to_bits(), Ordering::Relaxed);
        });
    }
    // Corpus picker.
    {
        let window = window.clone();
        let corpus_row = corpus_row.clone();
        let corpus_path = corpus_path.clone();
        corpus_btn.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Select text corpus (folder)").build();
            let corpus_row = corpus_row.clone();
            let corpus_path = corpus_path.clone();
            dialog.select_folder(Some(&window), gio::Cancellable::NONE, move |res| {
                if let Ok(folder) = res {
                    if let Some(path) = folder.path() {
                        corpus_row.set_subtitle(&path.display().to_string());
                        *corpus_path.borrow_mut() = Some(path);
                    }
                }
            });
        });
    }

    stack.add_titled_with_icon(&setup, Some("setup"), "Setup", "preferences-system-symbolic");

    // ---- Models page: wire the proposer cascade (the model seam) from the GUI ------
    // This connects the GUI to everything the headless `--proposer-cmd`/`--escalate-cmd`/
    // `--context-cmd` flags drive. Each field is a command that turns context (JSON on
    // stdin) into a unified diff (stdout); the engine gates and scores it. Leave the
    // fields blank to run the random-mutation baseline.
    let models = adw::PreferencesPage::new();
    let intro_group = adw::PreferencesGroup::builder()
        .title("Patch proposer")
        .description("A coding ecosystem replaces random mutation with model-proposed patches. \
                      The fast/cheap model runs first; the engine escalates to the strong model \
                      only on cases the cheap one can't solve, and harvests every win for \
                      distillation. Each box is a shell command following the context→diff protocol.")
        .build();
    models.add(&intro_group);

    let prop_group = adw::PreferencesGroup::builder()
        .title("Cascade tiers")
        .description("Each box holds a shell command (context JSON in → unified diff out). \
                      You usually don't type here — use the Model registry below to Train or pick \
                      a model and it fills these in. Tier 0 runs first; Tier 1 runs only when Tier 0 \
                      falls short. Leave all blank for the random-mutation baseline.")
        .build();
    let fast_row = adw::EntryRow::builder()
        .title("Tier 0 — fast proposer (e.g. local tinygpt)")
        .build();
    fast_row.set_tooltip_text(Some(
        "Filled by 'Train now' or the Tier-0 dropdown below. \
         Manual form: TINYGPT_TOK=tok.bin TINYGPT_MODEL=model.bin python3 .../proposer_tinygpt.py"));
    let escalate_row = adw::EntryRow::builder()
        .title("Tier 1 — escalate to (e.g. llama.cpp / API)")
        .build();
    escalate_row.set_tooltip_text(Some(
        "Filled by the Tier-1 dropdown after you import a GGUF. \
         Manual form: LLAMA_MODEL=model.gguf python3 .../proposer_llamacpp.py"));
    let context_row = adw::EntryRow::builder()
        .title("Context enrichment (optional RAG/planner stage)")
        .build();
    context_row.set_tooltip_text(Some(
        "Optional extra stage that enriches context before the tiers. \
         e.g. python3 .../retriever_experience.py"));
    let allow_test_edits_row = adw::SwitchRow::builder()
        .title("Allow test edits")
        .subtitle("Off by default: a proposer that edits tests trips the reward-hack tripwire and scores 0")
        .build();
    allow_test_edits_row.set_active(false);
    prop_group.add(&fast_row);
    prop_group.add(&escalate_row);
    prop_group.add(&context_row);
    prop_group.add(&allow_test_edits_row);
    models.add(&prop_group);

    // ---- Graphical model registry: select / import / train, all managed --------
    let reg_group = adw::PreferencesGroup::builder()
        .title("Model registry")
        .description("Pick managed models for each tier. GGUF models live in the registry's \
                      gguf/ folder; local tinygpt models are trained from your harvested wins and \
                      cached automatically. Selecting a model fills its tier command below.")
        .build();

    let entries: Rc<RefCell<Vec<models::ModelEntry>>> = Rc::new(RefCell::new(models::scan()));
    let make_list = |entries: &[models::ModelEntry]| {
        let labels: Vec<String> = std::iter::once("(none / custom command)".to_string())
            .chain(entries.iter().map(|e| e.label.clone())).collect();
        let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        gtk::StringList::new(&refs)
    };
    let tier0_combo = adw::ComboRow::builder().title("Tier 0 — fast proposer").build();
    let tier1_combo = adw::ComboRow::builder().title("Tier 1 — escalation").build();
    tier0_combo.set_model(Some(&make_list(&entries.borrow())));
    tier1_combo.set_model(Some(&make_list(&entries.borrow())));
    reg_group.add(&tier0_combo);
    reg_group.add(&tier1_combo);

    // selecting a model fills the corresponding command field (the Config source of truth)
    {
        let entries = entries.clone(); let fast_row = fast_row.clone();
        tier0_combo.connect_selected_notify(move |c| {
            let i = c.selected() as usize;
            if i >= 1 { if let Some(e) = entries.borrow().get(i - 1) { fast_row.set_text(&e.command); } }
        });
    }
    {
        let entries = entries.clone(); let escalate_row = escalate_row.clone();
        tier1_combo.connect_selected_notify(move |c| {
            let i = c.selected() as usize;
            if i >= 1 { if let Some(e) = entries.borrow().get(i - 1) { escalate_row.set_text(&e.command); } }
        });
    }

    // refresh helper: rescan the registry and repopulate both selectors
    let refresh = {
        let entries = entries.clone();
        let tier0_combo = tier0_combo.clone(); let tier1_combo = tier1_combo.clone();
        let make_list = make_list;
        Rc::new(move || {
            let scanned = models::scan();
            tier0_combo.set_model(Some(&make_list(&scanned)));
            tier1_combo.set_model(Some(&make_list(&scanned)));
            *entries.borrow_mut() = scanned;
        })
    };

    // ---- manage: import GGUF, train a local model ------------------------------
    let manage_group = adw::PreferencesGroup::builder()
        .title("Manage models")
        .description("Import a GGUF (copied into the registry) or train a local tinygpt from \
                      your harvested fixes. Training runs in the background and the result is \
                      cached and added to the selectors automatically.")
        .build();

    let import_row = adw::ActionRow::builder()
        .title("Import GGUF")
        .subtitle("Copy a .gguf into the registry so it appears in the tier selectors")
        .build();
    let import_btn = gtk::Button::with_label("Choose .gguf…");
    import_btn.add_css_class("flat"); import_btn.set_valign(gtk::Align::Center);
    import_row.add_suffix(&import_btn);

    let train_row = adw::ActionRow::builder()
        .title("Train local model from experience")
        .subtitle("Distill harvested wins (experience.jsonl) into a fast tinygpt tier-0 model")
        .build();
    let train_spinner = gtk::Spinner::new();
    train_spinner.set_valign(gtk::Align::Center);
    let train_btn = gtk::Button::with_label("Train now");
    train_btn.add_css_class("suggested-action"); train_btn.set_valign(gtk::Align::Center);
    train_row.add_suffix(&train_spinner);
    train_row.add_suffix(&train_btn);

    let train_status = gtk::Label::builder().label("").wrap(true).xalign(0.0).build();
    train_status.add_css_class("dim-label"); train_status.add_css_class("caption");
    let train_status_row = adw::ActionRow::new();
    train_status_row.set_child(Some(&train_status));

    manage_group.add(&import_row);
    manage_group.add(&train_row);
    manage_group.add(&train_status_row);

    // Verify a configured model actually works before a full run.
    let test_row = adw::ActionRow::builder()
        .title("Test configured models")
        .subtitle("Dry-run each tier's command on a sample bug and check it emits a diff")
        .build();
    let test_btn = gtk::Button::with_label("Test now");
    test_btn.add_css_class("flat"); test_btn.set_valign(gtk::Align::Center);
    test_row.add_suffix(&test_btn);
    manage_group.add(&test_row);

    // Remove the cached local model currently selected for tier 0.
    let remove_row = adw::ActionRow::builder()
        .title("Remove selected local model")
        .subtitle("Deletes the tier-0 local model from the registry (GGUF imports are left alone)")
        .build();
    let remove_btn = gtk::Button::with_label("Remove");
    remove_btn.add_css_class("destructive-action"); remove_btn.set_valign(gtk::Align::Center);
    remove_row.add_suffix(&remove_btn);
    manage_group.add(&remove_row);

    models.add(&reg_group);
    models.add(&manage_group);

    // Test handler: dry-run tier-0 and tier-1 commands off the main thread.
    {
        let fast_row = fast_row.clone();
        let escalate_row = escalate_row.clone();
        let train_status = train_status.clone();
        let train_spinner = train_spinner.clone();
        test_btn.connect_clicked(move |btn| {
            let t0 = fast_row.text().trim().to_string();
            let t1 = escalate_row.text().trim().to_string();
            if t0.is_empty() && t1.is_empty() {
                train_status.set_text("nothing to test — select or enter a model command first");
                return;
            }
            btn.set_sensitive(false);
            train_spinner.start();
            train_status.set_text("testing …");
            let (tx, rx) = async_channel::unbounded::<String>();
            std::thread::spawn(move || {
                let report = |label: &str, cmd: &str| -> String {
                    if cmd.is_empty() { return format!("{label}: (none)"); }
                    match models::test_proposer(cmd) {
                        Ok(n) => format!("{label}: ok — produced a {n}-line diff"),
                        Err(e) => format!("{label}: FAILED — {e}"),
                    }
                };
                let lines = vec![report("tier 0", &t0), report("tier 1", &t1)];
                let _ = tx.send_blocking(format!("__DONE__{}", lines.join("   ·   ")));
            });
            let train_status = train_status.clone();
            let train_spinner = train_spinner.clone();
            let test_btn = btn.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
                let mut done = false;
                while let Ok(msg) = rx.try_recv() {
                    if let Some(r) = msg.strip_prefix("__DONE__") { train_status.set_text(r); done = true; }
                    else { train_status.set_text(&msg); }
                }
                if done { train_spinner.stop(); test_btn.set_sensitive(true); glib::ControlFlow::Break }
                else { glib::ControlFlow::Continue }
            });
        });
    }

    // Remove handler: delete the tier-0-selected local model.
    {
        let entries = entries.clone();
        let tier0_combo = tier0_combo.clone();
        let train_status = train_status.clone();
        let refresh = refresh.clone();
        let fast_row = fast_row.clone();
        remove_btn.connect_clicked(move |_| {
            let i = tier0_combo.selected() as usize;
            if i < 1 { train_status.set_text("select a local model in the tier-0 list first"); return; }
            let entry = entries.borrow().get(i - 1).cloned();
            match entry {
                Some(e) if e.kind == "local" => match models::delete_local(&e.label) {
                    Ok(()) => { train_status.set_text(&format!("removed {}", e.label));
                                fast_row.set_text(""); (refresh)(); }
                    Err(err) => train_status.set_text(&format!("remove failed: {err}")),
                },
                Some(_) => train_status.set_text("that's a GGUF import — remove it from the registry folder manually"),
                None => train_status.set_text("nothing selected"),
            }
        });
    }

    // ---- Download a model: curated common sources so you don't have to learn HF ----
    let dl_group = adw::PreferencesGroup::builder()
        .title("Download a model")
        .description("Pick a known coder GGUF and fetch it straight into the registry — no \
                      Hugging Face or Ollama account needed. Or choose ‘Custom URL’ and paste any \
                      direct .gguf link. Downloads can be several GB.")
        .build();
    let dl_combo = adw::ComboRow::builder().title("Source").build();
    let cat = models::catalog();
    let mut dl_labels: Vec<String> = cat.iter().map(|e| format!("{} · {}", e.label, e.note)).collect();
    dl_labels.push("Custom URL…".to_string());
    let dl_refs: Vec<&str> = dl_labels.iter().map(|s| s.as_str()).collect();
    dl_combo.set_model(Some(&gtk::StringList::new(&dl_refs)));
    let dl_url_row = adw::EntryRow::builder().title("Custom .gguf URL (used when ‘Custom URL’ is selected)").build();
    let dl_row = adw::ActionRow::builder().title("Fetch into registry").build();
    let dl_btn = gtk::Button::with_label("Download");
    dl_btn.add_css_class("suggested-action"); dl_btn.set_valign(gtk::Align::Center);
    dl_row.add_suffix(&dl_btn);
    dl_group.add(&dl_combo);
    dl_group.add(&dl_url_row);
    dl_group.add(&dl_row);
    models.add(&dl_group);

    {
        let refresh = refresh.clone();
        let train_status = train_status.clone();
        let train_spinner = train_spinner.clone();
        let tier1_combo = tier1_combo.clone();
        let entries = entries.clone();
        let dl_combo = dl_combo.clone();
        let dl_url_row = dl_url_row.clone();
        dl_btn.connect_clicked(move |btn| {
            let cat = models::catalog();
            let sel = dl_combo.selected() as usize;
            let (url, filename) = if sel < cat.len() {
                (cat[sel].url(), cat[sel].file.clone())
            } else {
                let u = dl_url_row.text().trim().to_string();
                if u.is_empty() { train_status.set_text("enter a .gguf URL, or pick a source above"); return; }
                let f = models::filename_from_url(&u);
                (u, f)
            };
            btn.set_sensitive(false);
            train_spinner.start();
            train_status.set_text(&format!("downloading {filename} …"));
            let (tx, rx) = async_channel::unbounded::<String>();
            {
                let url = url.clone(); let filename = filename.clone();
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(match models::download_gguf(&url, &filename) {
                        Ok(p) => format!("__DONE__{}", p.display()),
                        Err(e) => format!("__ERR__{e}"),
                    });
                });
            }
            let refresh = refresh.clone();
            let train_status = train_status.clone();
            let train_spinner = train_spinner.clone();
            let dl_btn = btn.clone();
            let tier1_combo = tier1_combo.clone();
            let entries = entries.clone();
            let part = models::download_part_path(&filename);
            let filename2 = filename.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(400), move || {
                let mut done = false;
                while let Ok(msg) = rx.try_recv() {
                    if let Some(p) = msg.strip_prefix("__DONE__") {
                        train_status.set_text(&format!("downloaded · {p}"));
                        (refresh)();
                        if let Some(i) = entries.borrow().iter().position(|e| e.command.contains(&filename2)) {
                            tier1_combo.set_selected((i + 1) as u32);
                        }
                        done = true;
                    } else if let Some(e) = msg.strip_prefix("__ERR__") {
                        train_status.set_text(&format!("download failed: {e}"));
                        done = true;
                    }
                }
                if !done {
                    // progress: poll the in-flight .part size
                    if let Ok(m) = std::fs::metadata(&part) {
                        train_status.set_text(&format!("downloading {filename2} … {:.0} MB", m.len() as f64 / 1.0e6));
                    }
                    glib::ControlFlow::Continue
                } else {
                    train_spinner.stop(); dl_btn.set_sensitive(true);
                    glib::ControlFlow::Break
                }
            });
        });
    }

    let help_group = adw::PreferencesGroup::builder().title("How the loop works").build();
    let help = gtk::Label::builder()
        .label("Run the cascade with a strong tier-1 model; every winning patch is harvested to \
                experience.jsonl. Click ‘Train now’ to distill those wins into a local tier-0 model — \
                it’s cached in the registry and selectable above. Re-train as the harvest grows and \
                the cheap local tier handles more of your recurring work for free.")
        .wrap(true).xalign(0.0).build();
    help.add_css_class("dim-label"); help.add_css_class("caption");
    help_group.add(&help);
    models.add(&help_group);

    // import GGUF → copy into registry, refresh
    {
        let window = window.clone();
        let refresh = refresh.clone();
        let train_status = train_status.clone();
        import_btn.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder().title("Select a GGUF model").build();
            let refresh = refresh.clone();
            let train_status = train_status.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        match models::import_gguf(&path) {
                            Ok(dst) => { train_status.set_text(&format!("imported {}", dst.display())); (refresh)(); }
                            Err(e) => train_status.set_text(&format!("import failed: {e}")),
                        }
                    }
                }
            });
        });
    }

    // train local model → background thread, progress streamed to the looking-glass timer
    {
        let refresh = refresh.clone();
        let train_status = train_status.clone();
        let train_spinner = train_spinner.clone();
        let train_btn = train_btn.clone();
        let tier0_combo = tier0_combo.clone();
        let entries = entries.clone();
        train_btn.connect_clicked(move |btn| {
            let exp = registry::data_root().join("experience.jsonl");
            if !exp.exists() {
                train_status.set_text("no experience.jsonl yet — run the arena with a proposer first to harvest fixes");
                return;
            }
            btn.set_sensitive(false);
            train_spinner.start();
            train_status.set_text("starting training …");
            let name = format!("tinygpt-{}", chrono_stamp());
            let (tx, rx) = async_channel::unbounded::<String>();
            {
                let exp = exp.clone(); let name = name.clone();
                std::thread::spawn(move || {
                    let tx2 = tx.clone();
                    let res = models::train_local(&exp, &name, 1000, move |line| { let _ = tx2.send_blocking(line); });
                    let _ = tx.send_blocking(match res {
                        Ok(p) => format!("__DONE__{}", p.display()),
                        Err(e) => format!("__ERR__{e}"),
                    });
                });
            }
            // poll the channel from the main loop (off the engine's path; never blocks UI)
            let refresh = refresh.clone();
            let train_status = train_status.clone();
            let train_spinner = train_spinner.clone();
            let train_btn = btn.clone();
            let tier0_combo = tier0_combo.clone();
            let entries = entries.clone();
            glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
                let mut done = false;
                while let Ok(msg) = rx.try_recv() {
                    if let Some(p) = msg.strip_prefix("__DONE__") {
                        train_status.set_text(&format!("done · cached {p}"));
                        (refresh)();
                        // auto-select the freshly trained model for tier 0
                        let want = std::path::Path::new(p).file_name()
                            .map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        if let Some(idx) = entries.borrow().iter().position(|e| e.label.contains(&want)) {
                            tier0_combo.set_selected((idx + 1) as u32);
                        }
                        done = true;
                    } else if let Some(e) = msg.strip_prefix("__ERR__") {
                        train_status.set_text(&format!("training failed: {e}"));
                        done = true;
                    } else {
                        train_status.set_text(&msg);
                    }
                }
                if done {
                    train_spinner.stop();
                    train_btn.set_sensitive(true);
                    glib::ControlFlow::Break
                } else {
                    glib::ControlFlow::Continue
                }
            });
        });
    }

    stack.add_titled_with_icon(&models, Some("models"), "Models", "applications-science-symbolic");

    // ===== ARENA PAGE =========================================================
    let arena_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let spinner = gtk::Spinner::new();
    spinner.set_margin_start(8);
    status_row.append(&spinner);
    let status_label = gtk::Label::builder()
        .label("idle — configure on the Setup page, then press Start")
        .xalign(0.0)
        .build();
    status_label.add_css_class("arena-status");
    status_row.append(&status_label);
    arena_box.append(&status_row);

    // Legend: what the tile colours mean (small, dim, self-documenting).
    let legend = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    legend.set_margin_start(10); legend.set_margin_end(10); legend.set_margin_bottom(4);
    for (cls, text) in [("bot-run", "running"), ("bot-pass", "passed"),
                        ("bot-fail", "failed"), ("bot-cull", "culled"),
                        ("bot-spawn", "spawned"), ("bot-idle", "idle")] {
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let swatch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        swatch.set_size_request(12, 12);
        swatch.add_css_class("bot-cell");
        swatch.add_css_class(cls);
        let lbl = gtk::Label::new(Some(text));
        lbl.add_css_class("dim-label");
        lbl.add_css_class("caption");
        chip.append(&swatch);
        chip.append(&lbl);
        legend.append(&chip);
    }
    arena_box.append(&legend);

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(8)
        .max_children_per_line(24)
        .row_spacing(2)
        .column_spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&flow)
        .build();
    arena_box.append(&scroller);
    rebuild_grid(&flow, &cells, 100);

    // Live worker activity — what each worker is doing / its last command.
    // Updated from Activity messages but only *rendered* on a 3s heartbeat.
    let activity_view = gtk::TextView::builder()
        .editable(false).cursor_visible(false).monospace(true)
        .left_margin(8).right_margin(8).top_margin(4).bottom_margin(4)
        .build();
    activity_view.add_css_class("activity-view");
    let activity_scroller = gtk::ScrolledWindow::builder()
        .min_content_height(132).max_content_height(132)
        .hexpand(true).child(&activity_view).build();
    let activity_frame = gtk::Frame::builder()
        .label("Live worker activity (refreshes every 3s)")
        .margin_start(6).margin_end(6).margin_bottom(6)
        .child(&activity_scroller).build();
    arena_box.append(&activity_frame);

    // newest-first (id, text), deduped by id, capped.
    let activity: Rc<RefCell<Vec<(usize, String)>>> = Rc::new(RefCell::new(Vec::new()));

    // 3-second render heartbeat.
    {
        let activity = activity.clone();
        let activity_view = activity_view.clone();
        glib::timeout_add_seconds_local(3, move || {
            let a = activity.borrow();
            if a.is_empty() {
                activity_view.buffer().set_text("(idle)");
            } else {
                let mut s = String::new();
                for (id, text) in a.iter().take(16) {
                    s.push_str(&format!("worker {id:>3}  ·  {text}\n"));
                }
                activity_view.buffer().set_text(s.trim_end());
            }
            glib::ControlFlow::Continue
        });
    }

    stack.add_titled_with_icon(&arena_box, Some("arena"), "Arena", "view-grid-symbolic");

    // ===== LOGS PAGE ==========================================================
    let logview = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    logview.add_css_class("logview");
    let log_scroller = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hexpand(true)
        .child(&logview)
        .build();
    stack.add_titled_with_icon(&log_scroller, Some("logs"), "Logs", "document-edit-symbolic");
    let replays_page = replays::Replays::new();
    stack.add_titled_with_icon(&replays_page.root, Some("replays"), "Replays", "media-playback-start-symbolic");
    stack.add_titled_with_icon(&heroes::page(), Some("heroes"), "Heroes", "starred-symbolic");
    stack.add_titled_with_icon(&inspect::page(), Some("inspect"), "Population", "view-paged-symbolic");
    stack.add_titled_with_icon(&inspect::audit_page(), Some("audit"), "Audit", "dialog-information-symbolic");

    // ---- header with view switcher + Start/Stop ------------------------------
    let header = adw::HeaderBar::new();
    let switcher = adw::ViewSwitcher::builder()
        .stack(&stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();
    header.set_title_widget(Some(&switcher));

    let start_btn = gtk::Button::builder().label("Start").build();
    start_btn.add_css_class("suggested-action");
    let stop_btn = gtk::Button::builder().label("Stop").sensitive(false).build();
    stop_btn.add_css_class("destructive-action");
    header.pack_start(&start_btn);
    header.pack_start(&stop_btn);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));

    // Responsive nav: the header's Wide switcher needs a lot of room (4 tabs +
    // Start/Stop). Below ~1200px it can clip a tab off the edge with no overflow
    // affordance — which is exactly why the Heroes tab could go missing. So add a
    // bottom ViewSwitcherBar and a breakpoint: when the window is narrower than
    // 1200sp, hide the header switcher and reveal the bottom bar, which always
    // shows every tab. Wide windows keep the header switcher.
    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();
    toolbar.add_bottom_bar(&switcher_bar);
    window.set_content(Some(&toolbar));
    {
        use gtk::prelude::Cast;
        let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MaxWidth, 1200.0, adw::LengthUnit::Sp));
        let sw_obj = switcher.clone().upcast::<glib::Object>();
        let bar_obj = switcher_bar.clone().upcast::<glib::Object>();
        bp.add_setters(&[
            (&sw_obj, "visible", false),
            (&bar_obj, "reveal", true),
        ]);
        window.add_breakpoint(bp);
    }

    // ---- workspace picker wiring ---------------------------------------------
    {
        let window = window.clone();
        let ws_row = ws_row.clone();
        let workspace_path = workspace_path.clone();
        ws_btn.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Select workspace (git repo)")
                .build();
            let ws_row = ws_row.clone();
            let workspace_path = workspace_path.clone();
            dialog.select_folder(Some(&window), gio::Cancellable::NONE, move |res| {
                if let Ok(folder) = res {
                    if let Some(path) = folder.path() {
                        ws_row.set_subtitle(&path.display().to_string());
                        *workspace_path.borrow_mut() = Some(path);
                    }
                }
            });
        });
    }

    // ---- image import wiring -------------------------------------------------
    {
        let window = window.clone();
        let logview = logview.clone();
        import_btn.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Import container image (.tar)")
                .build();
            let logview = logview.clone();
            dialog.open(Some(&window), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        append_log(&logview, &format!("[ui] image import requested: {}", path.display()));
                        // Under the Docker backend this is where `docker load -i <path>` runs.
                        match std::process::Command::new("docker")
                            .args(["load", "-i", &path.display().to_string()])
                            .output()
                        {
                            Ok(o) if o.status.success() =>
                                append_log(&logview, &format!("[docker] {}", String::from_utf8_lossy(&o.stdout).trim())),
                            Ok(o) =>
                                append_log(&logview, &format!("[docker] load failed: {}", String::from_utf8_lossy(&o.stderr).trim())),
                            Err(e) =>
                                append_log(&logview, &format!("[ui] docker not available: {e}")),
                        }
                    }
                }
            });
        });
    }

    // ---- Stop wiring ---------------------------------------------------------
    {
        let stop = stop.clone();
        stop_btn.connect_clicked(move |_| stop.store(true, Ordering::Relaxed));
    }

    // ---- Start wiring --------------------------------------------------------
    {
        let pop_scale = pop_scale.clone();
        let wpc_scale = wpc_scale.clone();
        let keep_scale = keep_scale.clone();
        let gen_scale = gen_scale.clone();
        let backend_row = backend_row.clone();
        let image_row = image_row.clone();
        let testcmd_row = testcmd_row.clone();
        let features_row = features_row.clone();
        let buildflags_row = buildflags_row.clone();
        let extratools_row = extratools_row.clone();
        let cache_row = cache_row.clone();
        let fastclone_row = fastclone_row.clone();
        let timeout_scale = timeout_scale.clone();
        let maxfile_scale = maxfile_scale.clone();
        let memoize_row = memoize_row.clone();
        let crossover_row = crossover_row.clone();
        let sandbox_row = sandbox_row.clone();
        let threads_scale = threads_scale.clone();
        let aggr_scale = aggr_scale.clone();
        let batch_scale = batch_scale.clone();
        let recur_scale = recur_scale.clone();
        let turbo_row = turbo_row.clone();
        let novelty_scale = novelty_scale.clone();
        let drift_scale = drift_scale.clone();
        let resume_row = resume_row.clone();
        let fast_row = fast_row.clone();
        let escalate_row = escalate_row.clone();
        let context_row = context_row.clone();
        let allow_test_edits_row = allow_test_edits_row.clone();
        let spinner = spinner.clone();
        let differential_row = differential_row.clone();
        let seed_row = seed_row.clone();
        let op_switches = op_switches.clone();
        let tool_rows = tool_rows.clone();
        let workspace_path = workspace_path.clone();
        let corpus_path = corpus_path.clone();
        let niche_scale = niche_scale.clone();
        let coord_row = coord_row.clone();
        let axis = axis.clone();
        let cells = cells.clone();
        let flow = flow.clone();
        let logview = logview.clone();
        let status_label = status_label.clone();
        let stack = stack.clone();
        let replays_page = replays_page.clone();
        let start_btn_inner = start_btn.clone();
        let stop_btn_inner = stop_btn.clone();
        let stop = stop.clone();

        start_btn.connect_clicked(move |_| {
            let cfg = Config {
                population: pop_from_slider(pop_scale.value()),
                workers_per_container: wpc_scale.value() as usize,
                keep_fraction: keep_scale.value(),
                generations: gens_from_slider(gen_scale.value()),
                tools: read_tools(&tool_rows),
                image: image_row.text().to_string(),
                workspace: workspace_path.borrow().clone(),
                workspaces: workspace_path.borrow().clone().into_iter().collect(),
                backend: BackendKind::from_index(backend_row.selected()),
                corpus: corpus_path.borrow().clone(),
                niches: niche_scale.value() as usize,
                coordinator: coord_row.is_active(),
                test_cmd: testcmd_row.text().to_string(),
                cache: cache_row.is_active(),
                fast_clone: fastclone_row.is_active(),
                timeout_secs: timeout_scale.value() as u64,
                max_file_mb: maxfile_scale.value() as u64,
                memoize: memoize_row.is_active(),
                crossover: crossover_row.is_active(),
                sandbox: sandbox_row.is_active(),
                threads: threads_scale.value() as usize,
                ops: {
                    let on: Vec<String> = op_switches.iter()
                        .filter(|(_, r)| r.is_active()).map(|(n, _)| n.clone()).collect();
                    if on.is_empty() { vec!["no-op".to_string()] } else { on }
                },
                aggression: aggr_scale.value(),
                seed: { let t = seed_row.text(); let s = t.trim(); if s.is_empty() { None } else { s.parse::<u64>().ok() } },
                batch_start: batch_scale.value() as usize,
                turbo: turbo_row.is_active(),
                allow_test_edits: allow_test_edits_row.is_active(),
                novelty_lambda: novelty_scale.value(),
                drift: drift_scale.value(),
                proposer_cmd: fast_row.text().trim().to_string(),
                proposer_fallbacks: {
                    let e = escalate_row.text().trim().to_string();
                    if e.is_empty() { Vec::new() } else { vec![e] }
                },
                context_cmds: {
                    let c = context_row.text().trim().to_string();
                    if c.is_empty() { Vec::new() } else { vec![c] }
                },
                resume: resume_row.is_active(),
                differential: differential_row.is_active(),
                cargo_features: features_row.text().to_string(),
                build_flags: buildflags_row.text().to_string(),
                extra_tools: extratools_row.text().split(',')
                    .map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
                recurrent_steps: recur_scale.value() as usize,
            };
            // Remember this workspace so next launch can offer a one-click restore.
            if let Some(ws) = workspace_path.borrow().as_ref() {
                registry::save_last_workspace(ws);
            }

            // Reset and rebuild the grid to the chosen population.
            stop.store(false, Ordering::Relaxed);
            rebuild_grid(&flow, &cells, cfg.population);
            stack.set_visible_child_name("arena");
            status_label.set_text(&format!(
                "running — {} bots across {} container(s)",
                cfg.population,
                engine::container_count(cfg.population, cfg.workers_per_container)
            ));
            start_btn_inner.set_sensitive(false);
            stop_btn_inner.set_sensitive(true);

            // Spawn the engine and wire its message stream into the GUI.
            let (tx, rx) = async_channel::unbounded::<EngineMsg>();
            let cfg_thread = cfg.clone();
            let stop_thread = stop.clone();
            let axis_thread = axis.clone();
            replays_page.reset();
            spinner.start();
            std::thread::spawn(move || engine::run_engine(cfg_thread, tx, stop_thread, axis_thread));

            // The GUI is a *looking glass*: it samples engine state at a fixed cadence
            // instead of reacting to every message. The engine emits freely — a Bot/
            // Activity update per worker, many times a second at turbo — and we bulk-drain
            // whatever queued each frame, coalesce to the latest state per bot, and render
            // once. This bounds all UI work to ~30 Hz regardless of engine speed, so the
            // view can never stall and is never in the engine's critical path. (The old
            // design awaited one message per main-loop turn, which the message flood
            // saturated — that was the freeze.)
            #[derive(Clone)]
            struct CellState { status: BotStatus, fitness: f64, z: f64, purpose: String, tag: Option<String> }
            let pending: Rc<RefCell<std::collections::HashMap<usize, CellState>>> =
                Rc::new(RefCell::new(std::collections::HashMap::new()));
            const LOG_CAP: i32 = 800; // keep the on-screen log bounded; oldest lines drop

            {
                let pending = pending.clone();
                let cells = cells.clone();
                let logview = logview.clone();
                let status_label = status_label.clone();
                let activity = activity.clone();
                let replays_page = replays_page.clone();
                let start_btn_done = start_btn_inner.clone();
                let stop_btn_done = stop_btn_inner.clone();
                let spinner = spinner.clone();
                glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
                    // 1) Bulk-drain the channel, coalescing. A per-frame budget guarantees a
                    //    huge backlog can never stall a single frame; the rest waits a frame.
                    let mut new_logs: Vec<String> = Vec::new();
                    let mut latest_status: Option<String> = None;
                    let mut done = false;
                    let mut budget = 50_000u32;
                    loop {
                        if budget == 0 { break; }
                        budget -= 1;
                        match rx.try_recv() {
                            Ok(EngineMsg::Bot { id, status, fitness, z, purpose }) => {
                                let mut p = pending.borrow_mut();
                                let prev_tag = p.get(&id).and_then(|c| c.tag.clone());
                                p.insert(id, CellState { status, fitness, z, purpose,
                                    tag: if status == BotStatus::Running { prev_tag } else { None } });
                            }
                            Ok(EngineMsg::Activity { id, text }) => {
                                let tag: String = text.split_whitespace().next().unwrap_or("")
                                    .trim_start_matches("edit:").chars().take(6).collect();
                                {
                                    let mut p = pending.borrow_mut();
                                    let e = p.entry(id).or_insert(CellState { status: BotStatus::Running,
                                        fitness: 0.0, z: 0.0, purpose: String::new(), tag: None });
                                    e.tag = Some(tag);
                                }
                                let mut a = activity.borrow_mut();
                                a.retain(|(i, _)| *i != id);
                                a.insert(0, (id, text));
                                a.truncate(60);
                            }
                            Ok(EngineMsg::Log(s)) => new_logs.push(s),
                            Ok(EngineMsg::Generation { n, best, mean, alive, focus }) => {
                                latest_status = Some(format!(
                                    "generation {n} · alive {alive} · best {best:.3} · mean {mean:.3} · focus z={focus:.2}"));
                            }
                            Ok(EngineMsg::Replay { gen, best, mean, top }) => {
                                replays_page.push(replays::Frame { gen, best, mean, top });
                            }
                            Ok(EngineMsg::Coordinator { focus, label, niche_reward }) => {
                                new_logs.push(format!(
                                    "[coordinator] deploying purpose '{label}' (z={focus:.2}), niche reward {niche_reward:.3}"));
                            }
                            Ok(EngineMsg::Done) => done = true,
                            Err(async_channel::TryRecvError::Empty) => break,
                            Err(async_channel::TryRecvError::Closed) => break,
                        }
                    }
                    // 2) Render tiles once (latest state per bot).
                    if !pending.borrow().is_empty() {
                        let drained: Vec<(usize, CellState)> = pending.borrow_mut().drain().collect();
                        let cells = cells.borrow();
                        for (id, s) in drained {
                            if let Some(cell) = cells.get(id) {
                                cell.root.set_css_classes(&["bot-cell", s.status.css_class()]);
                                let txt = match &s.tag {
                                    Some(t) => t.clone(),
                                    None => if s.fitness > 0.0 { format!("{:.2}", s.fitness) } else { "·".to_string() },
                                };
                                cell.fit.set_text(&txt);
                                cell.root.set_tooltip_text(Some(&format!(
                                    "worker {id}\npurpose: {}  (z={:.2})\nfitness: {:.2}",
                                    s.purpose, s.z, s.fitness)));
                            }
                        }
                    }
                    // 3) Status line: only the latest matters.
                    if let Some(st) = latest_status { status_label.set_text(&st); }
                    // 4) Log: append the whole batch in one insert, scroll once, then trim
                    //    to LOG_CAP lines so the buffer stays bounded over long runs.
                    if !new_logs.is_empty() {
                        let buf = logview.buffer();
                        let mut end = buf.end_iter();
                        buf.insert(&mut end, &(new_logs.join("\n") + "\n"));
                        let lines = buf.line_count();
                        if lines > LOG_CAP {
                            let mut s0 = buf.start_iter();
                            if let Some(mut cut) = buf.iter_at_line(lines - LOG_CAP) {
                                buf.delete(&mut s0, &mut cut);
                            }
                        }
                        let mut end = buf.end_iter();
                        logview.scroll_to_iter(&mut end, 0.0, false, 0.0, 0.0);
                    }
                    if done {
                        start_btn_done.set_sensitive(true);
                        stop_btn_done.set_sensitive(false);
                        spinner.stop();
                        let buf = logview.buffer();
                        let mut end = buf.end_iter();
                        buf.insert(&mut end, "[ui] engine finished.\n");
                    }
                    glib::ControlFlow::Continue
                });
            }
        });
    }

    window.present();
}
