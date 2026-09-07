//! polish — the finish snapclone has, for any app, in one line a frame.
//!
//! What made snapclone read as glossy next to the fleet's other egui fronts was
//! never its palette. It was two things layered: a **live backdrop** (`livebg`:
//! feathered light, a drifting sheen, a vignette under the controls) and **glass
//! panels** over it — high-alpha tints the backdrop shows through as a glow,
//! rather than opaque slabs. The second half now lives in `theme_bridge`, so
//! every app that calls `apply_palette` / `apply_prefs` has glass already. This
//! module is the first half:
//!
//! ```ignore
//! struct App { backdrop: uikit_egui::polish::Backdrop, /* … */ }
//! // once:
//! let backdrop = uikit_egui::polish::Backdrop::for_app("drivemind");
//! // each frame, before any panel:
//! let colors = uikit_egui::apply_palette(ctx, palette);
//! self.backdrop.paint(ctx, &colors);
//! ```
//!
//! `paint` ticks the animation from egui's own frame time, draws on the
//! background layer over the whole screen (so it sits behind the glass
//! `CentralPanel` and every window), and asks for a repaint at a background's
//! pace — thirty a second while it moves, none when `speed` is zero — so an
//! idle app does not burn a core to drift a curtain.
//!
//! `for_app` picks the style from the app's name, so each app has a look of
//! its own and the same look every launch; `set_style` is for a settings
//! page.

use crate::livebg::{BgStyle, LiveBackground};
use crate::PaletteColors;
use egui::{Context, LayerId};

/// The backdrop: a live background plus the bookkeeping to paint it once a frame.
#[derive(Debug, Clone)]
pub struct Backdrop {
    pub bg: LiveBackground,
    /// Frames per second asked for while the background moves.
    pub fps: f32,
}

impl Backdrop {
    pub fn new(style: BgStyle) -> Backdrop {
        Backdrop { bg: LiveBackground::new(style), fps: 30.0 }
    }

    /// A style chosen from the app's name — stable across launches, different across apps.
    pub fn for_app(name: &str) -> Backdrop {
        Backdrop::new(style_for_app(name))
    }

    pub fn set_style(&mut self, style: BgStyle) {
        self.bg.style = style;
    }
    pub fn style(&self) -> BgStyle {
        self.bg.style
    }

    /// Paint behind everything for this frame. Call before any panel is shown.
    pub fn paint(&mut self, ctx: &Context, colors: &PaletteColors) {
        let dt = ctx.input(|i| i.stable_dt).min(0.1);
        self.bg.tick(dt);
        // The FULL viewport, not the usable area: this paints behind everything, and a
        // background that stops at the status bar leaves a bare strip along the top of a
        // phone. The one place where `viewport_rect` is the right half of the split.
        let rect = ctx.viewport_rect();
        let painter = ctx.layer_painter(LayerId::background());
        self.bg.paint(&painter, rect, colors);
        if self.bg.speed > 0.0 && self.fps > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs_f32(1.0 / self.fps));
        }
    }
}

/// The style for an app, from its name. FNV-1a over the name, modulo the styles — so
/// "drivemind" is always bokeh and "dialer" is always aurora, whatever else changes.
pub fn style_for_app(name: &str) -> BgStyle {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let all = BgStyle::all();
    all[(h % all.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_apps_style_is_stable_and_apps_do_not_all_get_the_same_one() {
        assert_eq!(style_for_app("drivemind"), style_for_app("drivemind"));
        let names = ["drivemind", "dialer", "bot-arena", "bo-suite", "switcheroo", "voiceterm", "aperture", "rusty-board"];
        let mut seen: Vec<BgStyle> = Vec::new();
        for n in names {
            let s = style_for_app(n);
            if !seen.contains(&s) {
                seen.push(s);
            }
        }
        assert!(seen.len() >= 3, "eight apps, {} styles — the hash is not spreading", seen.len());
    }

    #[test]
    fn paint_draws_behind_and_asks_for_a_repaint_only_while_moving() {
        let colors = crate::default_palette();
        let mut b = Backdrop::for_app("test");
        let ctx = Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 700.0)));
        let out = ctx.run(input.clone(), |ctx| {
            b.paint(ctx, &colors);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("over the glass");
            });
        });
        assert!(!out.shapes.is_empty());
        assert!(b.bg.phase > 0.0, "ticked from the frame time");
        // a still backdrop asks for nothing
        let mut still = Backdrop::new(BgStyle::Gradient);
        still.bg.speed = 0.0;
        let out2 = ctx.run(input, |ctx| {
            still.paint(ctx, &colors);
        });
        assert!(!out2.shapes.is_empty());
        assert_eq!(still.bg.phase, 0.0);
    }
}
