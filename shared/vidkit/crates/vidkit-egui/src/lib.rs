//! vidkit-egui — the egui front end for vidkit, on shared/uikit.
//!
//! Two pieces:
//!
//! 1. [`EguiPresenter`] implements `vidkit_core::Presenter` by converting on
//!    the CPU and uploading an egui texture. Portable to the glow backend every
//!    egui app in this tree already uses.
//! 2. [`VideoView`] paints it, letterboxed, with `uikit-egui`'s chrome and
//!    gesture layer, so a video surface looks like the rest of the app and a
//!    restyle of the kit restyles this too.
//!
//! ## What this is not
//!
//! The zero-copy path. `vidkit-vk` imports the decoder's DMA-BUF and converts
//! in a compute shader; that needs the wgpu backend and `wgpu_hal` access, and
//! it is a different presenter. On a phone it is the one worth having. This one
//! works everywhere.
//!
//! ## Theme
//!
//! Call [`install`] once at startup. It reaches `uikit_egui::fonts::install`,
//! which `maintenance/tools/font_install_check.py` requires of every egui
//! surface in this tree — switcheroo shipped with `★`, `☆` and `↻` rendering as
//! boxes because it was the one app that skipped it.

pub mod bar;
pub mod chrome;
/// Declares the video picture to an e-ink runtime (eink-kit); inert elsewhere.
pub mod eink_hint;
pub mod fit;
pub mod glass;
pub mod palette;
pub mod settings;
pub mod tips;
pub mod voice;
/// GPU upload path: NV12 planes plus a conversion shader, 2.7x less bus
/// traffic than the RGBA8 upload in [`presenter`]. Needs a glow context.
#[cfg(feature = "glow-upload")]
pub mod glow_present;
#[cfg(feature = "glow-upload")]
pub mod dmabuf_gl;
pub mod gestures;
pub mod presenter;
pub mod pip;
pub mod sections;
pub mod view;

pub use gestures::{
    apply_rotation, apply_speed_step, apply_volume_step, apply_zoom_step, DragAxis, GestureConfig,
    GestureRouter, Intent, ViewLock, Zone,
};
pub use bar::{bottom_bar, fit_label, next_fit, BarAction};
pub use settings::{FillMode, Settings};
pub use tips::{Tip, TipPolicy, Tips, GESTURE_TIPS};
pub use palette::{available, commands, icon_for, Action, ClipboardState, Command};
pub use chrome::{Chrome, ChromePolicy};
pub use glass::{contrast_ratio, GlassStyle};
pub use fit::{fit_rect, surface_to_frame, Fit, PixelAspect};
pub use presenter::EguiPresenter;
pub use pip::{Pip, PipAction};
pub use sections::{Scope, SectionAction, SectionsModel, SectionsPanel};
pub use view::{format_time, progress, scrub_fraction, scrub_target, Transport, VideoView, ViewAction};

use egui::Context;
use snapclone_theme::Palette;
use uikit_egui::PaletteColors;

/// Install fonts and apply a palette. Call once, at startup, before the first
/// frame.
///
/// Split from `VideoView` so the same call covers an app whose video surface is
/// one tab among many: the fonts and palette are process-wide, the widget is
/// not.
pub fn install(ctx: &Context, palette: &Palette) -> PaletteColors {
    uikit_egui::fonts::install(ctx);
    uikit_egui::theme_bridge::touch_targets(ctx);
    uikit_egui::theme_bridge::apply_palette(ctx, palette)
}
