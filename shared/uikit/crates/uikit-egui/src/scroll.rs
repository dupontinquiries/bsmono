//! scroll — scroll-area helpers that make the persistent id automatic, so independent scroll areas
//! can't accidentally share one offset.
//!
//! egui keys a [`egui::ScrollArea`]'s saved offset by an id derived from the parent `ui` plus the
//! area's `id_salt`. Two areas in the same `ui` with no salt resolve to the *same* id and scroll
//! together — a silent bug that's easy to reintroduce on every new page (you only notice when two
//! panes move as one, or scroll position bleeds across tabs).
//!
//! These helpers derive the salt from the **call site** (`#[track_caller]`: file + line + column),
//! so every `scroll::area(..)` / `scroll::bounded(..)` / `scroll::log(..)` is independent by
//! construction with *no* per-page boilerplate and nothing to forget. The salt is stable across
//! frames (same site each frame) and unique across sites.
//!
//! The one case the call-site trick can't cover is several scroll areas created from a *single* call
//! site — e.g. inside a `for` loop. There, pass an explicit per-item key with [`area_keyed`].

use std::hash::Hash;
use std::panic::Location;

/// A vertical scroll region whose id comes from the call site — you never pass, or forget, a salt.
/// Returns the closure's value.
#[track_caller]
pub fn area<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let l = Location::caller();
    egui::ScrollArea::vertical()
        .id_salt((l.file(), l.line(), l.column()))
        .show(ui, add_contents)
        .inner
}

/// Like [`area`], bounded to `max_height` px (the content scrolls within that height).
#[track_caller]
pub fn bounded<R>(ui: &mut egui::Ui, max_height: f32, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let l = Location::caller();
    egui::ScrollArea::vertical()
        .id_salt((l.file(), l.line(), l.column()))
        .max_height(max_height)
        .show(ui, add_contents)
        .inner
}

/// A streaming-log pane: bounded height, stuck to the bottom, each line rendered monospace + small.
/// This is the build / network / push output pattern collapsed into one call, so new pages don't
/// re-derive it (or forget the id). Pass the accumulated lines; it always shows the newest.
#[track_caller]
pub fn log(ui: &mut egui::Ui, lines: &[String], max_height: f32) {
    let l = Location::caller();
    egui::ScrollArea::vertical()
        .id_salt((l.file(), l.line(), l.column()))
        .max_height(max_height)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in lines {
                ui.label(egui::RichText::new(line).monospace().small());
            }
        });
}

/// For when one call site produces several scroll areas (e.g. one per item in a loop): the call-site
/// salt would collide, so pass an explicit per-item `key` instead.
pub fn area_keyed<R>(ui: &mut egui::Ui, key: impl Hash, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::ScrollArea::vertical().id_salt(key).show(ui, add_contents).inner
}
