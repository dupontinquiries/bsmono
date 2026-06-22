//! corpus_studio.rs — a GUI that automates the v1.5 corpus-LM pipeline.
//!
//! Pick a dataset, optionally stage it to a temp working dir, optionally run an
//! uploaded preprocessing tool, then run the six pipeline stages (bpe_train ->
//! ngram_train -> word2vec_train -> sentence_pool -> transitions -> tiny_gpt) as a
//! live stepper: each stage shows pending / running / done / failed and streams
//! its output into an expandable per-step log.
//!
//! It shells out to the compiled v1.5 binaries — point "Binaries dir" at your
//! v1.5-complete/bin. The orchestration runs on a worker thread and streams
//! messages to the GUI over an async-channel (same pattern as bot-arena).
//!
//! Build (alongside bot-arena): `cargo build --release` then
//! `cargo run --release --bin corpus_studio`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gtk::prelude::*;
use adw::prelude::*;
use gtk::{gio, glib};

#[path = "../registry.rs"]
mod registry;

const APP_ID: &str = "org.example.CorpusStudio";

const CSS: &str = "
.step-sym { font-size: 16px; min-width: 22px; }
.sym-pending { color: #888; }
.sym-run     { color: #1c9fe0; }
.sym-done    { color: #2ec27e; }
.sym-fail    { color: #e01b24; }
.sym-skip    { color: #555; }
.steptitle { font-weight: bold; }
.steplog { font-family: monospace; font-size: 11px; }
.statusbar { padding: 8px; font-size: 13px; }
";

/// Pipeline stages. Index 0 is the optional preprocessing tool.
const STAGE_NAMES: [&str; 7] = [
    "0 · Preprocess (your tool)",
    "1 · BPE tokenizer",
    "2 · N-gram model",
    "3 · Word2Vec",
    "4 · Sentence pooling",
    "5 · Transitions",
    "6 · tiny_gpt train",
];

#[derive(Clone)]
struct StudioConfig {
    corpus: PathBuf,
    preprocess: bool,
    builtin_pp: bool, // use the bundled Rust corpus_md converter instead of an external tool
    tool: Option<PathBuf>,
    tool_args: String, // template with {in} and {out}
    bin_dir: PathBuf,
    out_dir: PathBuf,
    corpus_ext: String,
    merges: u32,
    max_n: u32,
    w2v_dim: u32,
    w2v_epochs: u32,
    gpt_steps: u32,
    gpt_dim: u32,
    gpt_layers: u32,
    niches: u32,
}

enum Msg {
    Log(String),
    StepStart(usize),
    StepLog(usize, String),
    StepDone(usize, bool),
    Finished,
}

/// Widgets for one stage row in the stepper.
struct StepRow {
    sym: gtk::Label,
    spinner: gtk::Spinner,
    log: gtk::TextView,
    revealer: gtk::Revealer,
    run_from: gtk::Button,
    reset_from: gtk::Button,
}

fn main() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(CSS);
        if let Some(d) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&d, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
    });
    app.connect_activate(build_ui);
    app.connect_shutdown(|_| { let _ = std::fs::remove_dir_all(staging_dir()); });
    app.run()
}

/// One staging dir per process: temp/corpus-studio/staged-<pid>. Reused (cleared)
/// on each "Stage to temp" and removed on shutdown, so /tmp never accumulates.
fn staging_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("corpus-studio").join(format!("staged-{}", std::process::id()))
}

/// Where sticky settings live: <user-config>/corpus-studio/settings.conf.
fn settings_path() -> std::path::PathBuf {
    glib::user_config_dir().join("corpus-studio").join("settings.conf")
}

/// Load settings as a key=value map (empty if no file yet).
fn load_settings_from(path: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                m.insert(k.trim().to_string(), v.to_string());
            }
        }
    }
    m
}
fn load_settings() -> HashMap<String, String> {
    load_settings_from(&settings_path())
}

/// Persist the settings map (creates the parent dir as needed).
fn save_settings_to(path: &Path, m: &HashMap<String, String>) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut out = String::new();
    for (k, v) in m {
        out.push_str(k);
        out.push('=');
        out.push_str(v);
        out.push('\n');
    }
    let _ = std::fs::write(path, out);
}
fn save_settings(m: &HashMap<String, String>) {
    save_settings_to(&settings_path(), m);
}

fn slider_row(title: &str, min: f64, max: f64, step: f64, val: f64, digits: i32)
    -> (adw::ActionRow, gtk::Scale)
{
    let row = adw::ActionRow::builder().title(title).build();
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_value(val);
    scale.set_hexpand(true);
    scale.set_size_request(280, -1);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_digits(digits);
    scale.set_valign(gtk::Align::Center);
    row.add_suffix(&scale);
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

fn pick_folder(window: &adw::ApplicationWindow, title: &str,
               row: &adw::ActionRow, target: &Rc<RefCell<Option<PathBuf>>>) {
    let dialog = gtk::FileDialog::builder().title(title).build();
    let row = row.clone();
    let target = target.clone();
    dialog.select_folder(Some(window), gio::Cancellable::NONE, move |res| {
        if let Ok(f) = res {
            if let Some(p) = f.path() {
                row.set_subtitle(&p.display().to_string());
                *target.borrow_mut() = Some(p);
            }
        }
    });
}

fn pick_file(window: &adw::ApplicationWindow, title: &str,
             row: &adw::ActionRow, target: &Rc<RefCell<Option<PathBuf>>>) {
    let dialog = gtk::FileDialog::builder().title(title).build();
    let row = row.clone();
    let target = target.clone();
    dialog.open(Some(window), gio::Cancellable::NONE, move |res| {
        if let Ok(f) = res {
            if let Some(p) = f.path() {
                row.set_subtitle(&p.display().to_string());
                *target.borrow_mut() = Some(p);
            }
        }
    });
}

/// The bundled Rust HTML→MD converter, expected next to this binary.
fn builtin_converter() -> PathBuf {
    std::env::current_exe().ok()
        .and_then(|p| p.parent().map(|d| d.join("corpus_md")))
        .unwrap_or_else(|| PathBuf::from("corpus_md"))
}

/// Effective corpus the pipeline stages read: the preprocessed dir if enabled.
fn effective_corpus(cfg: &StudioConfig) -> PathBuf {
    if cfg.preprocess { cfg.out_dir.join("preprocessed") } else { cfg.corpus.clone() }
}

/// Scan a corpus directory and return the file extensions present, most-common
/// first (e.g. ".md", ".txt"). The C tools filter by CORPUS_EXT (default ".md"),
/// so a corpus of .txt/.rs files silently matches ZERO files and the first stage
/// produces an empty tokenizer — which only blows up downstream. Detecting the
/// real extensions lets us set CORPUS_EXT correctly and warn on a mismatch.
fn detect_corpus_exts(dir: &Path) -> Vec<(String, usize)> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    fn walk(dir: &Path, counts: &mut std::collections::HashMap<String, usize>, depth: usize) {
        if depth > 6 { return; }
        let Ok(rd) = std::fs::read_dir(dir) else { return; };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().and_then(|s| s.to_str()).map(|s| s.starts_with('.')).unwrap_or(false) { continue; }
                walk(&p, counts, depth + 1);
            } else if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                *counts.entry(format!(".{ext}")).or_insert(0) += 1;
            }
        }
    }
    walk(dir, &mut counts, 0);
    let mut v: Vec<(String, usize)> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}

/// How many files in `dir` match the comma-separated `exts` list (suffix match).
fn count_matching_files(dir: &Path, exts: &str) -> usize {
    let exts: Vec<&str> = exts.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if exts.is_empty() { return 0; }
    let mut n = 0;
    fn walk(dir: &Path, exts: &[&str], n: &mut usize, depth: usize) {
        if depth > 6 { return; }
        let Ok(rd) = std::fs::read_dir(dir) else { return; };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().and_then(|s| s.to_str()).map(|s| s.starts_with('.')).unwrap_or(false) { continue; }
                walk(&p, exts, n, depth + 1);
            } else if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if exts.iter().any(|x| name.ends_with(x)) { *n += 1; }
            }
        }
    }
    walk(dir, &exts, &mut n, 0);
    n
}

/// The output file a stage is expected to produce (for post-run validation). None
/// for stages with no single canonical output (preprocess).
fn stage_output(cfg: &StudioConfig, idx: usize) -> Option<PathBuf> {
    let o = |n: &str| cfg.out_dir.join(n);
    match idx {
        1 => Some(o("tokenizer.bin")),
        2 => Some(o("model.bin")),
        3 => Some(o("vectors.bin")),
        4 => Some(o("paragraph_vectors.bin")),
        5 => Some(o("transitions.bin")),
        6 => Some(o("gpt.bin")),
        _ => None,
    }
}

/// Build the (program, args, set_corpus_ext) for a stage. Returns None to skip.
fn stage_command(cfg: &StudioConfig, idx: usize) -> Option<(PathBuf, Vec<String>, bool)> {
    let bin = |name: &str| cfg.bin_dir.join(name);
    let out = |name: &str| cfg.out_dir.join(name).display().to_string();
    let corpus = effective_corpus(cfg).display().to_string();
    match idx {
        0 => {
            if !cfg.preprocess { return None; }
            let pre = cfg.out_dir.join("preprocessed").display().to_string();
            let subst = cfg
                .tool_args
                .replace("{in}", &cfg.corpus.display().to_string())
                .replace("{out}", &pre);
            let parts: Vec<String> = subst.split_whitespace().map(|s| s.to_string()).collect();
            if cfg.builtin_pp {
                // Bundled Rust converter (sits next to this binary) — no python/html2text.
                Some((builtin_converter(), parts, false))
            } else {
                let tool = cfg.tool.clone()?;
                // Python tools (e.g. create-md-from-web.py) run via the interpreter.
                if tool.extension().and_then(|s| s.to_str()) == Some("py") {
                    let mut args = vec![tool.display().to_string()];
                    args.extend(parts);
                    Some((PathBuf::from("python3"), args, false))
                } else {
                    Some((tool, parts, false))
                }
            }
        }
        1 => Some((bin("bpe_train"),
                   vec![corpus, out("tokenizer.bin"), cfg.merges.to_string()], true)),
        2 => Some((bin("ngram_train"),
                   vec![corpus, out("tokenizer.bin"), out("model.bin"), cfg.max_n.to_string()], true)),
        3 => Some((bin("word2vec_train"),
                   vec![corpus, out("tokenizer.bin"), out("vectors.bin"),
                        cfg.w2v_dim.to_string(), cfg.w2v_epochs.to_string()], true)),
        4 => Some((bin("sentence_pool"),
                   vec![corpus, out("tokenizer.bin"), out("vectors.bin"), out("paragraph_vectors.bin")], true)),
        5 => Some((bin("transitions"),
                   vec![out("paragraph_vectors.bin"), out("transitions.bin"), cfg.niches.to_string()], false)),
        6 => Some((bin("tiny_gpt"),
                   vec!["train".into(), corpus, out("tokenizer.bin"), out("gpt.bin"),
                        cfg.gpt_steps.to_string(), cfg.gpt_dim.to_string(), cfg.gpt_layers.to_string()], true)),
        _ => None,
    }
}

/// Run a sequence of stages on a worker thread, streaming output to the GUI.
fn run_stages(cfg: StudioConfig, stages: Vec<usize>, tx: async_channel::Sender<Msg>, stop: Arc<AtomicBool>) {
    let _ = std::fs::create_dir_all(&cfg.out_dir);
    if cfg.preprocess {
        let _ = std::fs::create_dir_all(cfg.out_dir.join("preprocessed"));
    }
    for idx in stages {
        if stop.load(Ordering::Relaxed) {
            let _ = tx.send_blocking(Msg::Log("stopped.".into()));
            break;
        }
        let cmd = match stage_command(&cfg, idx) {
            Some(c) => c,
            None => {
                let _ = tx.send_blocking(Msg::StepDone(idx, true)); // skipped == ok
                let _ = tx.send_blocking(Msg::Log(format!("[{}] skipped", STAGE_NAMES[idx])));
                continue;
            }
        };
        let (prog, args, set_ext) = cmd;
        let _ = tx.send_blocking(Msg::StepStart(idx));
        let _ = tx.send_blocking(Msg::StepLog(idx, format!("$ {} {}", prog.display(), args.join(" "))));

        let mut command = Command::new(&prog);
        command.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if set_ext && !cfg.corpus_ext.trim().is_empty() {
            command.env("CORPUS_EXT", &cfg.corpus_ext);
        }
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send_blocking(Msg::StepLog(idx, format!("failed to launch: {e}")));
                let _ = tx.send_blocking(Msg::StepDone(idx, false));
                break;
            }
        };
        // Forward stdout on a helper thread; read stderr (where the C tools log) here.
        if let Some(out) = child.stdout.take() {
            let txo = tx.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let _ = txo.send_blocking(Msg::StepLog(idx, line));
                }
            });
        }
        if let Some(err) = child.stderr.take() {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                let _ = tx.send_blocking(Msg::StepLog(idx, line));
                if stop.load(Ordering::Relaxed) { let _ = child.kill(); break; }
            }
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        // Output validation: a stage can exit 0 but produce nothing (e.g. a corpus
        // that matched no files, or a partial write). Verify the expected artifact
        // exists and is non-empty so the failure surfaces HERE, at the real stage,
        // instead of cascading and looking like a later step's fault.
        let ok = if ok {
            match stage_output(&cfg, idx) {
                Some(out) => match std::fs::metadata(&out) {
                    Ok(m) if m.len() > 0 => true,
                    _ => {
                        let _ = tx.send_blocking(Msg::StepLog(idx, format!(
                            "exit 0 but expected output is missing or empty: {}", out.display())));
                        let _ = tx.send_blocking(Msg::StepLog(idx, format!(
                            "→ {} produced no usable output — check the corpus extension and earlier stages.",
                            STAGE_NAMES[idx])));
                        false
                    }
                },
                None => true,
            }
        } else { false };
        let _ = tx.send_blocking(Msg::StepDone(idx, ok));
        if !ok {
            let _ = tx.send_blocking(Msg::Log(format!("[{}] FAILED — halting", STAGE_NAMES[idx])));
            break;
        }
    }
    let _ = tx.send_blocking(Msg::Finished);
}

fn build_ui(app: &adw::Application) {
    let corpus_path: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let tool_path: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let bin_path: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let out_path: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let steps: Rc<RefCell<Vec<StepRow>>> = Rc::new(RefCell::new(Vec::new()));
    let next_stage: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    // Defaults point at the shared registry so binaries are found automatically
    // and outputs land in an organized place.
    *bin_path.borrow_mut() = Some(registry::bin_dir());
    let default_out = registry::data_root().join("out");
    *out_path.borrow_mut() = Some(default_out.clone());

    let window = adw::ApplicationWindow::builder()
        .application(app).title("Corpus Studio")
        .default_width(1040).default_height(780).build();

    let stack = adw::ViewStack::new();

    // ===== SETUP =====
    let setup = adw::PreferencesPage::new();

    let ds = adw::PreferencesGroup::builder().title("Dataset").build();
    let corpus_row = adw::ActionRow::builder().title("Corpus").subtitle("None selected").build();
    let corpus_btn = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    corpus_row.add_suffix(&corpus_btn);
    let stage_btn = gtk::Button::builder().label("Stage to temp").valign(gtk::Align::Center).build();
    corpus_row.add_suffix(&stage_btn);
    ds.add(&corpus_row);
    let ext_row = adw::EntryRow::builder().title("CORPUS_EXT (comma-sep, blank = .md)").build();
    ds.add(&ext_row);
    setup.add(&ds);

    let pp = adw::PreferencesGroup::builder()
        .title("Preprocessing tool (optional)")
        .description("Your corpus tool. {in} = corpus, {out} = preprocessed dir. For create-md-from-web.py (HTML→MD), use: {in} {out}/corpus.md -j 4  (needs the html2text CLI).")
        .build();
    let pp_switch = adw::SwitchRow::builder().title("Run preprocessing first").build();
    pp.add(&pp_switch);
    let builtin_switch = adw::SwitchRow::builder()
        .title("Use built-in HTML→MD converter (Rust)")
        .subtitle("Bundled, multithreaded; no python/html2text needed")
        .build();
    builtin_switch.set_active(true);
    pp.add(&builtin_switch);
    let tool_row = adw::ActionRow::builder().title("Tool").subtitle("None selected").build();
    let tool_btn = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    tool_row.add_suffix(&tool_btn);
    pp.add(&tool_row);
    let args_row = adw::EntryRow::builder().title("Arg template").build();
    args_row.set_text("{in} {out}/corpus.md -j 4");
    pp.add(&args_row);
    setup.add(&pp);

    let paths = adw::PreferencesGroup::builder().title("Pipeline").build();
    let bin_hint = if registry::binaries_present() {
        format!("{} (built)", registry::bin_dir().display())
    } else {
        format!("{} — run `make -C pipeline` to build", registry::bin_dir().display())
    };
    let bin_row = adw::ActionRow::builder().title("Binaries dir").subtitle(&bin_hint).build();
    let bin_btn = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    bin_row.add_suffix(&bin_btn);
    paths.add(&bin_row);
    let out_row = adw::ActionRow::builder().title("Output dir").subtitle(&default_out.display().to_string()).build();
    let out_btn = gtk::Button::builder().label("Choose…").valign(gtk::Align::Center).build();
    out_row.add_suffix(&out_btn);
    paths.add(&out_row);
    setup.add(&paths);

    let params = adw::PreferencesGroup::builder().title("Parameters").build();
    let (merges_row, merges_s) = slider_row("BPE merges", 256.0, 32768.0, 256.0, 16384.0, 0);
    let (maxn_row, maxn_s) = slider_row("N-gram max n", 2.0, 6.0, 1.0, 5.0, 0);
    let (wdim_row, wdim_s) = slider_row("Word2Vec dim", 32.0, 512.0, 16.0, 128.0, 0);
    let (wep_row, wep_s) = slider_row("Word2Vec epochs", 1.0, 20.0, 1.0, 5.0, 0);
    let (nich_row, nich_s) = slider_row("Transition niches", 2.0, 64.0, 1.0, 32.0, 0);
    let (gsteps_row, gsteps_s) = slider_row("tiny_gpt steps", 100.0, 20000.0, 100.0, 2000.0, 0);
    let (gdim_row, gdim_s) = slider_row("tiny_gpt dim", 32.0, 512.0, 16.0, 128.0, 0);
    let (glay_row, glay_s) = slider_row("tiny_gpt layers", 1.0, 12.0, 1.0, 4.0, 0);
    for r in [&merges_row, &maxn_row, &wdim_row, &wep_row, &nich_row, &gsteps_row, &gdim_row, &glay_row] {
        params.add(r);
    }
    setup.add(&params);
    stack.add_titled_with_icon(&setup, Some("setup"), "Setup", "preferences-system-symbolic");

    // ----- restore sticky settings (overriding the defaults set above) -----
    {
        let s = load_settings();
        let set_path = |key: &str, row: &adw::ActionRow, cell: &Rc<RefCell<Option<PathBuf>>>| {
            if let Some(v) = s.get(key) {
                if !v.is_empty() {
                    row.set_subtitle(v);
                    *cell.borrow_mut() = Some(PathBuf::from(v));
                }
            }
        };
        set_path("corpus", &corpus_row, &corpus_path);
        set_path("tool", &tool_row, &tool_path);
        set_path("bin", &bin_row, &bin_path);
        set_path("out", &out_row, &out_path);
        if let Some(v) = s.get("ext") { ext_row.set_text(v); }
        if let Some(v) = s.get("args") { args_row.set_text(v); }
        if let Some(v) = s.get("preprocess") { pp_switch.set_active(v == "true"); }
        for (key, scale) in [
            ("merges", &merges_s), ("max_n", &maxn_s), ("w2v_dim", &wdim_s),
            ("w2v_epochs", &wep_s), ("niches", &nich_s), ("gpt_steps", &gsteps_s),
            ("gpt_dim", &gdim_s), ("gpt_layers", &glay_s),
        ] {
            if let Some(v) = s.get(key) {
                if let Ok(f) = v.parse::<f64>() { scale.set_value(f); }
            }
        }
    }

    // ===== PIPELINE (the stepper) =====
    let pipe_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let status = gtk::Label::builder().label("idle — choose a dataset and binaries, then Run").xalign(0.0).build();
    status.add_css_class("statusbar");
    pipe_box.append(&status);

    let steps_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    steps_box.set_margin_top(8);
    steps_box.set_margin_bottom(8);
    steps_box.set_margin_start(10);
    steps_box.set_margin_end(10);
    {
        let mut v = steps.borrow_mut();
        for name in STAGE_NAMES.iter() {
            let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
            let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let sym = gtk::Label::new(Some("○"));
            sym.set_css_classes(&["step-sym", "sym-pending"]);
            let spinner = gtk::Spinner::new();
            let title = gtk::Label::builder().label(*name).xalign(0.0).hexpand(true).build();
            title.add_css_class("steptitle");
            let toggle = gtk::Button::builder().label("log").valign(gtk::Align::Center).build();
            toggle.add_css_class("flat");
            // Per-step controls: run the pipeline from this stage onward, or roll
            // this stage (and everything after it) back to pending so you can re-run.
            let run_from = gtk::Button::builder().label("▶ from here").valign(gtk::Align::Center).build();
            run_from.add_css_class("flat");
            run_from.set_tooltip_text(Some("Run the pipeline starting at this stage"));
            let reset_from = gtk::Button::builder().label("↺").valign(gtk::Align::Center).build();
            reset_from.add_css_class("flat");
            reset_from.set_tooltip_text(Some("Roll back: mark this stage and all later ones pending"));
            head.append(&sym);
            head.append(&spinner);
            head.append(&title);
            head.append(&run_from);
            head.append(&reset_from);
            head.append(&toggle);

            let log = gtk::TextView::builder().editable(false).monospace(true)
                .cursor_visible(false).wrap_mode(gtk::WrapMode::WordChar).build();
            log.add_css_class("steplog");
            let log_scroll = gtk::ScrolledWindow::builder().min_content_height(120).child(&log).build();
            let revealer = gtk::Revealer::builder().child(&log_scroll).reveal_child(false).build();

            let rev2 = revealer.clone();
            toggle.connect_clicked(move |_| rev2.set_reveal_child(!rev2.reveals_child()));

            row.append(&head);
            row.append(&revealer);
            steps_box.append(&row);
            v.push(StepRow { sym, spinner, log, revealer, run_from, reset_from });
        }
    }
    let steps_scroll = gtk::ScrolledWindow::builder().vexpand(true).child(&steps_box).build();
    pipe_box.append(&steps_scroll);
    stack.add_titled_with_icon(&pipe_box, Some("pipeline"), "Pipeline", "view-list-symbolic");

    // ===== header =====
    let header = adw::HeaderBar::new();
    let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
    header.set_title_widget(Some(&switcher));
    let run_btn = gtk::Button::builder().label("Run all").build();
    run_btn.add_css_class("suggested-action");
    let step_btn = gtk::Button::builder().label("Step").build();
    let stop_btn = gtk::Button::builder().label("Stop").sensitive(false).build();
    stop_btn.add_css_class("destructive-action");
    header.pack_start(&run_btn);
    header.pack_start(&step_btn);
    header.pack_start(&stop_btn);
    let reset_btn = gtk::Button::builder().label("Reset").build();
    header.pack_end(&reset_btn);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&stack));
    window.set_content(Some(&toolbar));

    // ----- pickers -----
    {
        let w = window.clone(); let r = corpus_row.clone(); let t = corpus_path.clone();
        corpus_btn.connect_clicked(move |_| pick_folder(&w, "Select corpus folder", &r, &t));
    }
    {
        let w = window.clone(); let r = tool_row.clone(); let t = tool_path.clone();
        tool_btn.connect_clicked(move |_| pick_file(&w, "Select preprocessing tool", &r, &t));
    }
    {
        let w = window.clone(); let r = bin_row.clone(); let t = bin_path.clone();
        bin_btn.connect_clicked(move |_| pick_folder(&w, "Select binaries dir", &r, &t));
    }
    {
        let w = window.clone(); let r = out_row.clone(); let t = out_path.clone();
        out_btn.connect_clicked(move |_| pick_folder(&w, "Select output dir", &r, &t));
    }

    // ----- stage corpus to a fresh temp dir -----
    {
        let corpus_path = corpus_path.clone();
        let corpus_row = corpus_row.clone();
        let status = status.clone();
        stage_btn.connect_clicked(move |_| {
            let src = match corpus_path.borrow().clone() { Some(p) => p, None => { status.set_text("pick a corpus first"); return; } };
            // One staging dir per process, cleared and reused — never accumulates.
            let dst = staging_dir();
            let (tx, rx) = async_channel::unbounded::<String>();
            let dst2 = dst.clone();
            std::thread::spawn(move || {
                let _ = std::fs::remove_dir_all(&dst2);
                let _ = std::fs::create_dir_all(&dst2);
                let ok = Command::new("cp").arg("-r")
                    .arg(format!("{}/.", src.display())).arg(&dst2)
                    .status().map(|s| s.success()).unwrap_or(false);
                let _ = tx.send_blocking(if ok { format!("staged to {}", dst2.display()) } else { "staging failed".into() });
            });
            let corpus_path = corpus_path.clone();
            let corpus_row = corpus_row.clone();
            let status = status.clone();
            glib::spawn_future_local(async move {
                if let Ok(m) = rx.recv().await {
                    status.set_text(&m);
                    if m.starts_with("staged") {
                        corpus_row.set_subtitle(&dst.display().to_string());
                        *corpus_path.borrow_mut() = Some(dst);
                    }
                }
            });
        });
    }

    // ----- read config helper -----
    let read_cfg = {
        let corpus_path = corpus_path.clone();
        let tool_path = tool_path.clone();
        let bin_path = bin_path.clone();
        let out_path = out_path.clone();
        let pp_switch = pp_switch.clone();
        let builtin_switch = builtin_switch.clone();
        let args_row = args_row.clone();
        let ext_row = ext_row.clone();
        let merges_s = merges_s.clone(); let maxn_s = maxn_s.clone();
        let wdim_s = wdim_s.clone(); let wep_s = wep_s.clone(); let nich_s = nich_s.clone();
        let gsteps_s = gsteps_s.clone(); let gdim_s = gdim_s.clone(); let glay_s = glay_s.clone();
        move || -> Option<StudioConfig> {
            let corpus = corpus_path.borrow().clone()?;
            // Auto-detect the corpus extension when the field is left blank, so the
            // common "corpus is .txt/.rs but CORPUS_EXT defaults to .md → 0 files"
            // trap can't silently produce an empty tokenizer.
            let mut corpus_ext = ext_row.text().to_string();
            if corpus_ext.trim().is_empty() {
                let pp = pp_switch.is_active();
                let scan_dir = if pp { out_path.borrow().clone().map(|o| o.join("preprocessed")).unwrap_or_else(|| corpus.clone()) } else { corpus.clone() };
                let exts = detect_corpus_exts(&scan_dir);
                // take the top 1-3 extensions present
                let picked: Vec<String> = exts.iter().take(3).map(|(e, _)| e.clone()).collect();
                if !picked.is_empty() { corpus_ext = picked.join(","); }
            }
            Some(StudioConfig {
                corpus,
                preprocess: pp_switch.is_active(),
                builtin_pp: builtin_switch.is_active(),
                tool: tool_path.borrow().clone(),
                tool_args: args_row.text().to_string(),
                bin_dir: bin_path.borrow().clone()?,
                out_dir: out_path.borrow().clone()?,
                corpus_ext,
                merges: merges_s.value() as u32,
                max_n: maxn_s.value() as u32,
                w2v_dim: wdim_s.value() as u32,
                w2v_epochs: wep_s.value() as u32,
                gpt_steps: gsteps_s.value() as u32,
                gpt_dim: gdim_s.value() as u32,
                gpt_layers: glay_s.value() as u32,
                niches: nich_s.value() as u32,
            })
        }
    };

    // ----- shared launcher for a set of stages -----
    let launch = {
        let steps = steps.clone();
        let status = status.clone();
        let stack = stack.clone();
        let run_btn = run_btn.clone();
        let step_btn = step_btn.clone();
        let stop_btn = stop_btn.clone();
        let next_stage = next_stage.clone();
        let stop = stop.clone();
        Rc::new(move |cfg: StudioConfig, stages: Vec<usize>| {
            // Pre-flight: if a corpus-reading stage is queued and the configured
            // extension matches zero files, fail NOW with an actionable message
            // rather than letting stage 1 emit an empty tokenizer that blows up at
            // step 3. (Skipped when preprocessing, which generates files at runtime.)
            let reads_corpus = stages.iter().any(|s| matches!(s, 1 | 2 | 3 | 4 | 6));
            if reads_corpus && !cfg.preprocess {
                let n = count_matching_files(&effective_corpus(&cfg), &cfg.corpus_ext);
                if n == 0 {
                    let present = detect_corpus_exts(&effective_corpus(&cfg));
                    let hint = if present.is_empty() {
                        "the corpus folder appears to contain no files".to_string()
                    } else {
                        let list: Vec<String> = present.iter().take(4).map(|(e, c)| format!("{e}×{c}")).collect();
                        format!("corpus contains {} but CORPUS_EXT='{}' matches none — set the extension field to match",
                            list.join(", "), cfg.corpus_ext)
                    };
                    stack.set_visible_child_name("pipeline");
                    status.set_text(&format!("⚠ 0 corpus files match — {hint}"));
                    return;
                }
            }
            stop.store(false, Ordering::Relaxed);
            stack.set_visible_child_name("pipeline");
            run_btn.set_sensitive(false);
            step_btn.set_sensitive(false);
            stop_btn.set_sensitive(true);
            status.set_text("running…");

            let (tx, rx) = async_channel::unbounded::<Msg>();
            let cfg2 = cfg.clone();
            let stop2 = stop.clone();
            std::thread::spawn(move || run_stages(cfg2, stages, tx, stop2));

            let steps = steps.clone();
            let status = status.clone();
            let run_btn = run_btn.clone();
            let step_btn = step_btn.clone();
            let stop_btn = stop_btn.clone();
            let next_stage = next_stage.clone();
            glib::spawn_future_local(async move {
                while let Ok(msg) = rx.recv().await {
                    match msg {
                        Msg::Log(s) => status.set_text(&s),
                        Msg::StepStart(i) => {
                            if let Some(r) = steps.borrow().get(i) {
                                r.sym.set_css_classes(&["step-sym", "sym-run"]);
                                r.sym.set_text("◐");
                                r.spinner.start();
                                r.spinner.set_visible(true);
                                r.revealer.set_reveal_child(true);
                            }
                            status.set_text(&format!("running {}", STAGE_NAMES[i]));
                        }
                        Msg::StepLog(i, line) => {
                            if let Some(r) = steps.borrow().get(i) { append_log(&r.log, &line); }
                        }
                        Msg::StepDone(i, ok) => {
                            if let Some(r) = steps.borrow().get(i) {
                                r.spinner.stop();
                                r.spinner.set_visible(false);
                                if ok {
                                    r.sym.set_css_classes(&["step-sym", "sym-done"]);
                                    r.sym.set_text("✓");
                                } else {
                                    r.sym.set_css_classes(&["step-sym", "sym-fail"]);
                                    r.sym.set_text("✗");
                                }
                            }
                            *next_stage.borrow_mut() = i + 1;
                        }
                        Msg::Finished => {
                            run_btn.set_sensitive(true);
                            step_btn.set_sensitive(true);
                            stop_btn.set_sensitive(false);
                            status.set_text("done");
                        }
                    }
                }
            });
        })
    };

    // ----- Run all -----
    {
        let read_cfg = read_cfg.clone();
        let launch = launch.clone();
        let steps = steps.clone();
        let next_stage = next_stage.clone();
        let status = status.clone();
        let stack = stack.clone();
        run_btn.connect_clicked(move |_| {
            let cfg = match read_cfg() {
                Some(c) => c,
                None => {
                    stack.set_visible_child_name("pipeline");
                    status.set_text("⚠ Pick a corpus on the Setup page first (binaries/output default to the registry).");
                    return;
                }
            };
            if !cfg.bin_dir.join("bpe_train").exists() {
                stack.set_visible_child_name("pipeline");
                status.set_text(&format!(
                    "⚠ Pipeline binaries not found in {} — build them with `make -C pipeline` (or set Binaries dir to your build).",
                    cfg.bin_dir.display()
                ));
                return;
            }
            // reset all step rows
            for (i, r) in steps.borrow().iter().enumerate() {
                let skip = i == 0 && !cfg.preprocess;
                r.sym.set_css_classes(&["step-sym", if skip { "sym-skip" } else { "sym-pending" }]);
                r.sym.set_text(if skip { "–" } else { "○" });
                r.log.buffer().set_text("");
                r.revealer.set_reveal_child(false);
            }
            *next_stage.borrow_mut() = if cfg.preprocess { 0 } else { 1 };
            let stages: Vec<usize> = (if cfg.preprocess { 0 } else { 1 }..=6).collect();
            launch(cfg, stages);
        });
    }

    // ----- Step (run the next pending stage only) -----
    {
        let read_cfg = read_cfg.clone();
        let launch = launch.clone();
        let next_stage = next_stage.clone();
        let status = status.clone();
        let stack = stack.clone();
        step_btn.connect_clicked(move |_| {
            let cfg = match read_cfg() {
                Some(c) => c,
                None => {
                    stack.set_visible_child_name("pipeline");
                    status.set_text("⚠ Pick a corpus on the Setup page first.");
                    return;
                }
            };
            let mut idx = *next_stage.borrow();
            if idx == 0 && !cfg.preprocess { idx = 1; }
            if idx > 6 { status.set_text("pipeline already complete — use Run all to restart"); return; }
            launch(cfg, vec![idx]);
        });
    }

    // ----- Stop -----
    {
        let stop = stop.clone();
        stop_btn.connect_clicked(move |_| stop.store(true, Ordering::Relaxed));
    }

    // ----- per-step controls: "▶ from here" and "↺ rollback" -----
    {
        let n = steps.borrow().len();
        for idx in 0..n {
            // Run from this stage onward.
            {
                let read_cfg = read_cfg.clone();
                let launch = launch.clone();
                let steps = steps.clone();
                let next_stage = next_stage.clone();
                let status = status.clone();
                let stack = stack.clone();
                let btn = steps.borrow()[idx].run_from.clone();
                btn.connect_clicked(move |_| {
                    let cfg = match read_cfg() {
                        Some(c) => c,
                        None => { stack.set_visible_child_name("pipeline"); status.set_text("⚠ Pick a corpus first."); return; }
                    };
                    if idx == 0 && !cfg.preprocess {
                        status.set_text("preprocess is off — stage 0 is skipped; use ▶ from here on stage 1.");
                        return;
                    }
                    // reset this stage and everything after it to pending; keep earlier results
                    for (i, r) in steps.borrow().iter().enumerate() {
                        if i >= idx {
                            let skip = i == 0 && !cfg.preprocess;
                            r.sym.set_css_classes(&["step-sym", if skip { "sym-skip" } else { "sym-pending" }]);
                            r.sym.set_text(if skip { "–" } else { "○" });
                            r.log.buffer().set_text("");
                        }
                    }
                    *next_stage.borrow_mut() = idx;
                    let stages: Vec<usize> = (idx..=6).collect();
                    launch(cfg, stages);
                });
            }
            // Roll back: mark this stage and all later ones pending (no run).
            {
                let steps = steps.clone();
                let next_stage = next_stage.clone();
                let status = status.clone();
                let btn = steps.borrow()[idx].reset_from.clone();
                btn.connect_clicked(move |_| {
                    for (i, r) in steps.borrow().iter().enumerate() {
                        if i >= idx {
                            r.sym.set_css_classes(&["step-sym", "sym-pending"]);
                            r.sym.set_text("○");
                            r.log.buffer().set_text("");
                            r.revealer.set_reveal_child(false);
                        }
                    }
                    *next_stage.borrow_mut() = idx;
                    status.set_text(&format!("rolled back to {} — pending from here", STAGE_NAMES[idx]));
                });
            }
        }
    }

    // ----- sticky settings: gather current widget state into a map -----
    let gather = {
        let corpus_path = corpus_path.clone();
        let tool_path = tool_path.clone();
        let bin_path = bin_path.clone();
        let out_path = out_path.clone();
        let ext_row = ext_row.clone();
        let args_row = args_row.clone();
        let pp_switch = pp_switch.clone();
        let merges_s = merges_s.clone(); let maxn_s = maxn_s.clone();
        let wdim_s = wdim_s.clone(); let wep_s = wep_s.clone(); let nich_s = nich_s.clone();
        let gsteps_s = gsteps_s.clone(); let gdim_s = gdim_s.clone(); let glay_s = glay_s.clone();
        Rc::new(move || -> HashMap<String, String> {
            let path_str = |c: &Rc<RefCell<Option<PathBuf>>>| {
                c.borrow().clone().map(|p| p.display().to_string()).unwrap_or_default()
            };
            let mut m = HashMap::new();
            m.insert("corpus".into(), path_str(&corpus_path));
            m.insert("tool".into(), path_str(&tool_path));
            m.insert("bin".into(), path_str(&bin_path));
            m.insert("out".into(), path_str(&out_path));
            m.insert("ext".into(), ext_row.text().to_string());
            m.insert("args".into(), args_row.text().to_string());
            m.insert("preprocess".into(), if pp_switch.is_active() { "true" } else { "false" }.into());
            m.insert("merges".into(), merges_s.value().to_string());
            m.insert("max_n".into(), maxn_s.value().to_string());
            m.insert("w2v_dim".into(), wdim_s.value().to_string());
            m.insert("w2v_epochs".into(), wep_s.value().to_string());
            m.insert("niches".into(), nich_s.value().to_string());
            m.insert("gpt_steps".into(), gsteps_s.value().to_string());
            m.insert("gpt_dim".into(), gdim_s.value().to_string());
            m.insert("gpt_layers".into(), glay_s.value().to_string());
            m
        })
    };

    // Save on window close so a restart comes back to the same values.
    {
        let gather = gather.clone();
        window.connect_close_request(move |_| {
            save_settings(&gather());
            glib::Propagation::Proceed
        });
    }

    // ----- Reset: defaults + forget the saved file (across restarts) -----
    {
        let corpus_path = corpus_path.clone();
        let tool_path = tool_path.clone();
        let bin_path = bin_path.clone();
        let out_path = out_path.clone();
        let corpus_row = corpus_row.clone();
        let tool_row = tool_row.clone();
        let bin_row = bin_row.clone();
        let out_row = out_row.clone();
        let ext_row = ext_row.clone();
        let args_row = args_row.clone();
        let pp_switch = pp_switch.clone();
        let merges_s = merges_s.clone(); let maxn_s = maxn_s.clone();
        let wdim_s = wdim_s.clone(); let wep_s = wep_s.clone(); let nich_s = nich_s.clone();
        let gsteps_s = gsteps_s.clone(); let gdim_s = gdim_s.clone(); let glay_s = glay_s.clone();
        let default_out = default_out.clone();
        let status = status.clone();
        reset_btn.connect_clicked(move |_| {
            *corpus_path.borrow_mut() = None; corpus_row.set_subtitle("None selected");
            *tool_path.borrow_mut() = None; tool_row.set_subtitle("None selected");
            *bin_path.borrow_mut() = None; bin_row.set_subtitle("point at v1.5-complete/bin");
            *out_path.borrow_mut() = Some(default_out.clone());
            out_row.set_subtitle(&default_out.display().to_string());
            ext_row.set_text("");
            args_row.set_text("{in} {out}/corpus.md -j 4");
            pp_switch.set_active(false);
            merges_s.set_value(16384.0); maxn_s.set_value(5.0);
            wdim_s.set_value(128.0); wep_s.set_value(5.0); nich_s.set_value(32.0);
            gsteps_s.set_value(2000.0); gdim_s.set_value(128.0); glay_s.set_value(4.0);
            let _ = std::fs::remove_file(settings_path());
            status.set_text("settings reset to defaults");
        });
    }

    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_preserves_values_and_spaces() {
        let p = std::env::temp_dir().join(format!("cs-settings-test-{}.conf", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut m = HashMap::new();
        m.insert("merges".to_string(), "8192".to_string());
        m.insert("args".to_string(), "{in} {out}/corpus.md -j 4".to_string());
        m.insert("corpus".to_string(), "/data/my corpus/html".to_string()); // path with spaces
        m.insert("preprocess".to_string(), "true".to_string());
        save_settings_to(&p, &m);

        let back = load_settings_from(&p);
        assert_eq!(back.get("merges").map(String::as_str), Some("8192"));
        assert_eq!(back.get("args").map(String::as_str), Some("{in} {out}/corpus.md -j 4"));
        assert_eq!(back.get("corpus").map(String::as_str), Some("/data/my corpus/html"));
        assert_eq!(back.get("preprocess").map(String::as_str), Some("true"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_file_is_empty_map() {
        let p = std::env::temp_dir().join("cs-definitely-absent-xyz.conf");
        let _ = std::fs::remove_file(&p);
        assert!(load_settings_from(&p).is_empty());
    }
}
