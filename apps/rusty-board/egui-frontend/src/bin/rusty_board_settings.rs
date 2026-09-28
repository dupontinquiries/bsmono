//! rusty-board-settings — the keyboard's companion app.
//!
//! No arguments: the settings + custom layouts window (`companion.rs`).
//! With arguments: the same settings from a terminal, through `settings_cli`
//! (the grammar was written for this binary name):
//!
//!   rusty-board-settings list [group]
//!   rusty-board-settings get|reset <key>
//!   rusty-board-settings set <key> <value>
//!   rusty-board-settings layouts          user layouts, `*` marks the one in use
//!   rusty-board-settings use <slug>|""    show a user layout ("" = built-in)
//!
//! Every write goes to `settings.conf` and then `reload-*` to a running keyboard.

use rusty_board_egui::companion::{Companion, FsBackend};
use rusty_board_egui::{ctl, settings_cli, settings_store};

/// Distinct from the keyboard's `org.rustyboard.RustyBoard`: KWin's
/// keep-above/no-focus window rule matches that id, and would pin this
/// ordinary app window above everything and make it unfocusable.
const APP_ID: &str = "org.rustyboard.RustyBoard.Settings";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        std::process::exit(cli(&args.iter().map(String::as_str).collect::<Vec<_>>()));
    }
    rusty_board_egui::platform::capture_inherited_socket();
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
    let opts = eframe::NativeOptions {
        // Glow, not eframe's default wgpu: see demo_app.rs (a phone GPU with
        // 5 colour attachments dies at wgpu's startup).
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "Keyboard settings", [432.0, 900.0]).with_resizable(true),
        ..Default::default()
    };
    let r = eframe::run_native(
        APP_ID,
        opts,
        Box::new(|cc| {
            rusty_board_egui::companion::touch_style(&cc.egui_ctx);
            Ok(Box::new(Window(Companion::new(FsBackend))))
        }),
    );
    if let Err(e) = r {
        eprintln!("rusty-board-settings: {e}");
        std::process::exit(1);
    }
}

struct Window(Companion<FsBackend>);

impl eframe::App for Window {
    // Same shape as demo_app: this eframe requires ui(); the panels are shown from update().
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.ui(ctx);
    }
}

fn cli(args: &[&str]) -> i32 {
    let tell = |cmd: &str| {
        if ctl::send(cmd).is_err() {
            eprintln!("(keyboard not running; it loads this when it starts)");
        }
    };
    match args {
        ["layouts"] => {
            let in_use = settings_store::load().get(settings_store::USER_LAYOUT_KEY).cloned().unwrap_or_default();
            println!("{} built-in letters board", if in_use.is_empty() { "*" } else { " " });
            for e in board_layouts::io::list_user_layouts() {
                let slug = e.path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                let mark = if slug == in_use { "*" } else { " " };
                match &e.error {
                    None => println!("{mark} {slug:<24} {}", e.display_name),
                    Some(err) => println!("{mark} {slug:<24} BROKEN: {err}"),
                }
            }
            0
        }
        ["use", slug] => {
            let known = slug.is_empty()
                || board_layouts::io::list_user_layouts().iter().any(|e| e.path.file_stem().and_then(|s| s.to_str()) == Some(*slug));
            if !known {
                eprintln!("no user layout '{slug}' — `rusty-board-settings layouts` lists them");
                return 2;
            }
            match settings_store::set(settings_store::USER_LAYOUT_KEY, slug) {
                Ok(_) => {
                    tell("reload-layouts");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        _ => {
            let cmd = match settings_cli::parse(args) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{}", e.explain());
                    return 2;
                }
            };
            let stored = settings_store::valid_values(&settings_store::load());
            let current = |key: &str| {
                stored.get(key).cloned().unwrap_or_else(|| {
                    rusty_board_egui::settings_model::SETTINGS.iter().find(|s| s.key == key).map(|s| s.values[0].to_string()).unwrap_or_default()
                })
            };
            match &cmd {
                settings_cli::Cmd::List(g) => {
                    for l in settings_cli::render_list(*g, current) {
                        println!("{l}");
                    }
                }
                settings_cli::Cmd::Get(key) => {
                    for l in settings_cli::render_list(None, current).into_iter().filter(|l| l.split_whitespace().next() == Some(*key)) {
                        println!("{l}");
                    }
                }
                _ => {}
            }
            if let Some(ch) = settings_cli::change_for(&cmd) {
                // A font is checked before it is saved: fontconfig answers every
                // name with SOME face, so an unknown one must be refused here.
                if ch.key == rusty_board_egui::user_font::KEY {
                    if let Err(e) = rusty_board_egui::user_font::load(&ch.value) {
                        eprintln!("{e}");
                        return 2;
                    }
                }
                if let Err(e) = settings_store::set(ch.key, &ch.value) {
                    eprintln!("{e}");
                    return 1;
                }
                tell("reload-settings");
            }
            0
        }
    }
}
