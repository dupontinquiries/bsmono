//! runtime — ties a [`crate::bar::Bar`] to a live layer-shell surface and a
//! [`crate::placement::Placement`], and nothing else. Deliberately does
//! NOT touch rendering: painting `Bar::poses()` into pixels is a real,
//! still-open question (egui has no CPU rasterizer of its own — a
//! layer-shell `WlSurface` isn't something `eframe`/winit will hand a GL
//! context for the way it does for a normal toplevel window, so either a
//! `softbuffer`-backed software path or a `wayland-egl` + `glow` GPU path
//! needs picking, and neither has been started or tested here). That
//! choice is exactly the kind of thing this crate's own design already
//! seams off behind a trait rather than guessing at: [`BarSurface`] is the
//! transport seam, a `BarPainter` trait for rendering is the next one and
//! isn't defined yet because there's nothing concrete to seam against.
//!
//! What IS here: the orchestration logic — poll the surface, tick the
//! board, resize on mode change — tested against a fake surface so it
//! doesn't need a real compositor to verify.

use hatch_core::grid::GridSpec;

use crate::bar::{Bar, BarMode};
use crate::placement::Placement;
use crate::tile::TileItem;

/// The transport seam. `systemcompkit::wayland_io::LayerSurface` implements
/// this (behind this crate's `wayland_io` feature, in the `wayland` module
/// below); tests use a fake. Mirrors exactly the three things `Runtime`
/// needs and no more — not a general Wayland abstraction.
pub trait BarSurface {
    /// Non-blocking poll, same contract as the real `LayerSurface::poll`.
    /// Returns the current configured size if the surface has been
    /// configured at all (still `None` before the compositor's first
    /// `configure` arrives).
    fn poll(&mut self) -> Option<(u32, u32)>;
    fn is_closed(&self) -> bool;
    fn request_resize(&mut self, size: (u32, u32), exclusive_zone: i32) -> Result<(), String>;
}

pub struct Runtime<S: BarSurface> {
    surface: S,
    bar: Bar,
    placement: Placement,
    collapsed_size: u32,
    expanded_size: u32,
}

impl<S: BarSurface> Runtime<S> {
    pub fn new(
        surface: S,
        placement: Placement,
        board_config: hatch_core::anim::BoardConfig,
        collapsed_size: u32,
        expanded_size: u32,
    ) -> Runtime<S> {
        Runtime { surface, bar: Bar::new(board_config), placement, collapsed_size, expanded_size }
    }

    pub fn placement(&self) -> Placement {
        self.placement
    }

    pub fn mode(&self) -> BarMode {
        self.bar.mode()
    }

    /// Filters `items` through `Placement::shows` before handing them to
    /// `Bar` — the enforcement point noted as missing when `Placement` was
    /// first written: a caller going through `Runtime` can no longer skip
    /// the filter by forgetting to apply it themselves. A caller that
    /// bypasses `Runtime` and drives `Bar` directly still can, same as
    /// before; that door isn't closed, just no longer the only door.
    pub fn set_items(&mut self, items: Vec<TileItem>, spec: &GridSpec, now_ms: f64) {
        let filtered: Vec<TileItem> = items.into_iter().filter(|t| self.placement.shows(t.category)).collect();
        self.bar.set_items(filtered, spec, now_ms);
    }

    /// Flips collapsed/expanded and requests the matching resize in the
    /// same call — the two are never meant to happen independently, so
    /// there's deliberately no separate `set_expanded` exposed here (that
    /// still exists on `Bar` directly for a caller that wants the split).
    pub fn toggle_expanded(&mut self) -> Result<(), String> {
        let expanding = self.bar.mode() != BarMode::Expanded;
        self.bar.set_expanded(expanding);
        let size = if expanding { self.expanded_size } else { self.collapsed_size };
        let (wire_size, zone) = self.placement.resize_params(size);
        self.surface.request_resize(wire_size, zone)
    }

    /// Poll the surface and advance the board. Returns true while
    /// something is still animating — the caller's cue to keep repainting
    /// even with no new input.
    pub fn tick(&mut self, now_ms: f64) -> bool {
        self.surface.poll();
        self.bar.tick(now_ms)
    }

    pub fn is_closed(&self) -> bool {
        self.surface.is_closed()
    }

    pub fn poses(&self) -> impl Iterator<Item = (&TileItem, hatch_core::anim::TilePose)> {
        self.bar.poses()
    }
}

/// The real transport, feature-gated exactly like `systemcompkit`'s own
/// `wayland_io` — most consumers of `Runtime`'s orchestration logic (e.g.
/// its own tests) have no reason to link wayland-client.
#[cfg(feature = "wayland_io")]
pub mod wayland {
    use super::BarSurface;
    use systemcompkit::wayland_io::LayerSurface;

    impl BarSurface for LayerSurface {
        fn poll(&mut self) -> Option<(u32, u32)> {
            LayerSurface::poll(self);
            self.configured_size()
        }
        fn is_closed(&self) -> bool {
            LayerSurface::is_closed(self)
        }
        fn request_resize(&mut self, size: (u32, u32), exclusive_zone: i32) -> Result<(), String> {
            LayerSurface::request_resize(self, size, exclusive_zone)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::category::TileCategory;
    use crate::tile::TileSource;
    use hatch_core::anim::BoardConfig;

    struct FakeSurface {
        configured: Option<(u32, u32)>,
        closed: bool,
        resize_calls: Vec<((u32, u32), i32)>,
    }

    impl FakeSurface {
        fn new() -> FakeSurface {
            FakeSurface { configured: Some((800, 32)), closed: false, resize_calls: Vec::new() }
        }
    }

    impl BarSurface for FakeSurface {
        fn poll(&mut self) -> Option<(u32, u32)> {
            self.configured
        }
        fn is_closed(&self) -> bool {
            self.closed
        }
        fn request_resize(&mut self, size: (u32, u32), exclusive_zone: i32) -> Result<(), String> {
            self.resize_calls.push((size, exclusive_zone));
            self.configured = Some(size);
            Ok(())
        }
    }

    fn spec() -> GridSpec {
        GridSpec::fit(800.0, 96.0, 64.0, 0.0)
    }

    fn tile(key: &str, category: TileCategory) -> TileItem {
        TileItem { key: key.into(), label: key.into(), icon: String::new(), running: true, source: TileSource::Pinned, category }
    }

    #[test]
    fn starts_collapsed_and_toggling_resizes_to_the_expanded_size() {
        let mut rt = Runtime::new(FakeSurface::new(), Placement::LaptopTopBar, BoardConfig::default(), 32, 320);
        assert_eq!(rt.mode(), BarMode::Collapsed);

        rt.toggle_expanded().unwrap();
        assert_eq!(rt.mode(), BarMode::Expanded);
        assert_eq!(rt.surface.resize_calls, vec![((0, 320), 320)]);

        rt.toggle_expanded().unwrap();
        assert_eq!(rt.mode(), BarMode::Collapsed);
        assert_eq!(rt.surface.resize_calls[1], ((0, 32), 32));
    }

    #[test]
    fn set_items_drops_categories_the_placement_does_not_show() {
        let mut rt = Runtime::new(FakeSurface::new(), Placement::LaptopTopBar, BoardConfig::default(), 32, 320);
        // LaptopTopBar doesn't show SystemMonitor (see placement.rs).
        let items = vec![tile("firefox", TileCategory::Launcher), tile("cpu", TileCategory::SystemMonitor)];
        rt.set_items(items, &spec(), 0.0);
        let keys: Vec<&str> = rt.poses().map(|(t, _)| t.key.as_str()).collect();
        assert_eq!(keys, vec!["firefox"]);
    }

    #[test]
    fn same_items_on_a_desktop_bottom_bar_keep_the_system_monitor_tile() {
        let mut rt = Runtime::new(FakeSurface::new(), Placement::DesktopBottomBar, BoardConfig::default(), 48, 48);
        let items = vec![tile("firefox", TileCategory::Launcher), tile("cpu", TileCategory::SystemMonitor)];
        rt.set_items(items, &spec(), 0.0);
        assert_eq!(rt.poses().count(), 2);
    }

    #[test]
    fn is_closed_and_tick_pass_through_to_the_surface() {
        let mut surface = FakeSurface::new();
        surface.closed = true;
        let mut rt = Runtime::new(surface, Placement::MobileThumbSidebar, BoardConfig::default(), 72, 72);
        assert!(rt.is_closed());
        rt.tick(0.0); // must not panic with no items ever set
    }
}
