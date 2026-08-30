//! hatch_egui.rs — the deployable launcher window.
//!
//! Run:  cargo run --features app          (from apps/hatch/egui-frontend)
//!
//! Launcher lifecycle, which is different from a normal app's:
//!
//! * **One instance.** A second `hatch-egui` POKES the first (show/hide) and
//!   exits — that is what a global shortcut binds to. `--toggle` makes the
//!   poke explicit; a bare second launch does the same thing because "the
//!   launcher key" is the only reason a second launch happens.
//! * **Hide, don't quit.** Dismissal (swipe-down / Esc / focus loss) hides the
//!   window; the process stays, so the next summon is instant and icon
//!   textures stay warm. `--quit-on-hide` restores one-shot behaviour.
//! * **Focus loss hides** (the plasmoid's hideOnWindowDeactivate), unless
//!   `--stay` — which is also what you want while poking at it from a
//!   desktop.

use eframe::egui;
use hatch_egui::{toggle, HatchUi, UiEvent};
use uikit_egui::single_instance::{self, Claim};

/// Must match `packaging/org.hatch.Hatch.desktop` exactly — no extension,
/// same case. A mismatch fails SILENTLY: the window runs fine and simply has
/// no icon and no dock identity on Wayland.
const APP_ID: &str = "org.hatch.Hatch";

struct App {
    ui: HatchUi,
    palette: uikit_egui::PaletteColors,
    watch: toggle::ToggleWatch,
    visible: bool,
    stay: bool,
    quit_on_hide: bool,
    opacity: f32,
    /// Focus events arrive the frame they happen; remembering the last state
    /// keeps "hide on focus loss" from firing on the initial unfocused frame
    /// some compositors deliver before the first focus.
    was_focused: bool,
}

impl App {
    fn set_visible(&mut self, ctx: &egui::Context, v: bool) {
        if v == self.visible {
            return;
        }
        self.visible = v;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(v));
        if v {
            self.ui.on_shown();
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        } else if self.quit_on_hide {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // The plasmoid's translucent sheet: let the compositor blend the
        // wallpaper through. Needs a compositor that honours transparent
        // toplevels (KWin does); otherwise this is just dark.
        [0.02, 0.02, 0.03, self.opacity]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The summon channel: poll the poke file. Cheap (one stat), and the
        // repaint request below keeps polling alive while hidden.
        if self.watch.poked(APP_ID) {
            let v = self.visible;
            self.set_visible(ctx, !v);
        }
        if !self.visible {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
            return;
        }

        // Focus-loss hide, launcher-style.
        let mut lost_focus = false;
        ctx.input(|i| {
            for e in &i.raw.events {
                if let egui::Event::WindowFocused(f) = e {
                    if !*f && self.was_focused {
                        lost_focus = true;
                    }
                    self.was_focused = *f;
                }
            }
        });
        if lost_focus && !self.stay {
            self.set_visible(ctx, false);
            return;
        }

        let mut event = UiEvent::None;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().inner_margin(egui::Margin::same(10)))
            .show(ctx, |ui| {
                event = self.ui.ui(ui, &self.palette);
            });

        match event {
            UiEvent::None => {}
            UiEvent::Launched | UiEvent::Dismiss => self.set_visible(ctx, false),
        }
        // Keep the poke poll alive even on idle visible frames.
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    if has("--help") || has("-h") {
        println!(
            "hatch-egui — the hatch launcher (egui shell)\n\
             --toggle        poke a running instance to show/hide (implied by a second launch)\n\
             --stay          don't hide on focus loss (desktop debugging)\n\
             --quit-on-hide  exit instead of hiding (one-shot mode)\n\
             --windowed      smaller floating window instead of a phone-sized one"
        );
        return Ok(());
    }

    match single_instance::claim(APP_ID) {
        Claim::AlreadyRunning(_) => {
            // The launcher-key path: tell the running one and get out of the way.
            let _ = toggle::poke(APP_ID);
            return Ok(());
        }
        Claim::Acquired | Claim::Unavailable => {}
    }
    if has("--toggle") {
        // --toggle with nothing running: fall through and BE the instance.
    }

    let cfg = hatch_core::Config::load();
    let opacity = cfg.opacity.clamp(0.0, 1.0);

    // First run on a phone that already has a curated Plasma dash should show
    // that curation rather than an alphabetical wall. Once hatch has any
    // favorites of its own this is a no-op forever after — see plasma.rs for
    // why it seeds instead of syncing.
    let seeded = hatch_core::plasma::seed_favorites_if_empty();
    if !seeded.is_empty() {
        eprintln!("hatch: adopted {} favorites from Plasma", seeded.len());
    }
    let size = if has("--windowed") { [480.0, 640.0] } else { [420.0, 900.0] };
    let opts = eframe::NativeOptions {
        // Glow SAID, not left to eframe: its 0.34 defaults include
        // wgpu but not glow, and wgpu's default device descriptor
        // asks for 8 colour attachments — which dies at startup on
        // a phone GPU allowing 5. A glow-only feature list is not
        // enough; gitmind-egui declared one and still got wgpu
        // through feature unification.
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "Hatch", size)
            .with_transparent(true)
            .with_decorations(false),
        ..Default::default()
    };

    let stay = has("--stay");
    let quit_on_hide = has("--quit-on-hide");
    let result = eframe::run_native(
        APP_ID,
        opts,
        Box::new(move |cc| {
            // Fleet-wide prefs (palette, touch targets, animation feel) first,
            // then hatch's own opacity rides clear_color above.
            let prefs = settings_kit::Prefs::default();
            uikit_egui::apply_prefs(&cc.egui_ctx, &prefs);
            let palette = uikit_egui::palette_colors(&prefs.palette);
            Ok(Box::new(App {
                ui: HatchUi::new(hatch_core::Launcher::new(cfg)),
                palette,
                watch: toggle::ToggleWatch::default(),
                visible: true,
                stay,
                quit_on_hide,
                opacity,
                was_focused: true,
            }))
        }),
    );
    single_instance::release(APP_ID);
    result
}
