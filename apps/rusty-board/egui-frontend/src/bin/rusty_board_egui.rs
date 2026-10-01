//! rusty_board_egui.rs — the deployable egui rusty-board app.
//!
//! The screen itself is `rusty_board_egui::demo_app` so this binary and `examples/feel.rs`
//! cannot drift apart (they did, when each carried its own copy).
//!
//! Run:  cargo run --bin rusty-board-egui --features app   (from apps/rusty-board/egui-frontend)

fn main() -> eframe::Result<()> {
    // Without a subscriber every `tracing` line in the keyboard (focus decisions, input-method
    // health) was dropped, and the journal said nothing. RUST_LOG picks the level; warn by default.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")))
        .with_writer(std::io::stderr)
        .try_init();
    // FIRST, before eframe/winit touch Wayland: KWin hands the input-method socket over as
    // WAYLAND_SOCKET, and a toolkit that sees it connects to the wrong socket for rendering.
    rusty_board_egui::platform::capture_inherited_socket();
    // AFTER the capture, before anything opens a surface. The capture moves
    // WAYLAND_SOCKET, so a diagnosis run before it would describe an environment that no
    // longer exists — and this app reaches Wayland twice, through the layer shell and
    // through eframe, so the check belongs above both rather than beside one.
    //
    // `uikit_egui::backend_check`: the usual failure is a static musl build, which cannot
    // dlopen libwayland-client, and wayland-sys's own panic points at a package that is
    // already installed. Silent when the setup is fine.
    if let Some(why) = uikit_egui::backend_check::preflight() {
        eprintln!("{why}");
    }
    // Docked first, where the build and the compositor allow it. An explicit
    // RUSTY_BOARD_PLACEMENT=ruled|floating skips the attempt (the eframe shell is the
    // one with a real window to look at in dev).
    #[cfg(feature = "layer")]
    {
        use rusty_board_egui::platform::{session, Session};
        use rusty_board_egui::shell::layer::{self, LayerError};
        let forced = std::env::var("RUSTY_BOARD_PLACEMENT").ok();
        let skip = matches!(forced.as_deref().map(str::trim), Some("ruled") | Some("floating"));
        if session() == Session::Wayland && !skip {
            match layer::run("rusty-board") {
                Ok(()) => return Ok(()),
                Err(LayerError::Unsupported(why)) => {
                    eprintln!("[rusty-board] {why}; running as a toplevel window instead");
                }
                Err(LayerError::Runtime(e)) => {
                    eprintln!("[rusty-board] layer-shell shell failed: {e:#}");
                    std::process::exit(1);
                }
            }
        }
    }
    // A binary built without `layer` can only ever be a toplevel window. Say so, once, on a
    // Wayland session — otherwise the only symptom is "the keyboard is an app now", with
    // nothing in the journal to say it was a build flag and not the compositor.
    #[cfg(not(feature = "layer"))]
    {
        use rusty_board_egui::platform::{session, Session};
        if session() == Session::Wayland {
            eprintln!(
                "[rusty-board] built without the `layer` feature; running as a toplevel window. \
                 Rebuild with default features (or --features wayland,layer) to dock."
            );
        }
    }
    #[cfg(not(feature = "wayland"))]
    eprintln!(
        "[rusty-board] built without the `wayland` feature; keys type into the preview only, \
         not the focused app."
    );
    rusty_board_egui::demo_app::run("rusty-board")
}
