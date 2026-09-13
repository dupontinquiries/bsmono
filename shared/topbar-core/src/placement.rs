//! placement — where the bar lives and what it shows, per form factor.
//!
//! The three shapes named so far:
//! * laptop — a top bar, sharing the strip a camera notch sits in.
//! * mobile — a right-hand thumb sidebar.
//! * desktop (multi-monitor, no thumb/notch constraint) — a full bottom
//!   bar with room for system monitoring alongside launcher/taskbar.
//!
//! This module only maps a `Placement` to (a) the `systemcompkit` layer
//! intent that shapes it and (b) which `TileCategory`s it shows. It does
//! NOT decide the primary size (bar height/width) — that's a caller/config
//! concern, same as `LayerSurfaceIntent::top_panel` already takes it as a
//! parameter rather than hardcoding one.

use systemcompkit::LayerSurfaceIntent;

use crate::category::TileCategory;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// A top bar. NOTE: "notch-sharing" is the actual target shape here —
    /// reserving a cutout region so the bar's content flows around a
    /// camera notch rather than under it — and that is NOT modeled
    /// anywhere in this crate yet. `LayerSurfaceIntent::top_panel` only
    /// knows a rectangular exclusive zone; a real notch needs per-output
    /// geometry (`wl_output` physical size + a way to learn the cutout
    /// itself, which no protocol here currently surfaces) that this crate
    /// has no source for. Treating this as "just a top bar" until that
    /// geometry question has an actual answer, not before.
    LaptopTopBar,
    /// A right-hand thumb-reachable sidebar.
    MobileThumbSidebar,
    /// A full bottom bar with system-monitoring tiles alongside
    /// launcher/taskbar — the desktop shape with no thumb/notch
    /// constraint driving it.
    DesktopBottomBar,
}

impl Placement {
    /// Which tile categories this form factor shows. A caller filters
    /// `tile::unify`'s output through this before handing it to `Bar` —
    /// `Bar` itself stays placement-agnostic, same as `Board` stays
    /// tile-content-agnostic.
    pub fn visible_categories(&self) -> &'static [TileCategory] {
        match self {
            Placement::LaptopTopBar => &[TileCategory::Launcher, TileCategory::Taskbar],
            Placement::MobileThumbSidebar => &[TileCategory::Launcher, TileCategory::Taskbar],
            Placement::DesktopBottomBar => {
                &[TileCategory::Launcher, TileCategory::Taskbar, TileCategory::SystemMonitor]
            }
        }
    }

    pub fn shows(&self, category: TileCategory) -> bool {
        self.visible_categories().contains(&category)
    }

    /// The `systemcompkit` layer-shell intent for this placement at a
    /// given primary size (height for top/bottom, width for the
    /// sidebar) — see each constructor's own doc for what "primary size"
    /// reserves. Static, same caveat as `LayerSurfaceIntent::top_panel`:
    /// no live-resize path exists yet to actually apply a NEW size to an
    /// already-open surface.
    pub fn layer_intent(&self, primary_size: u32) -> LayerSurfaceIntent {
        match self {
            Placement::LaptopTopBar => LayerSurfaceIntent::top_panel(primary_size),
            Placement::MobileThumbSidebar => LayerSurfaceIntent::side_panel(primary_size),
            Placement::DesktopBottomBar => LayerSurfaceIntent::bottom_panel(primary_size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_is_the_only_placement_showing_system_monitor() {
        assert!(!Placement::LaptopTopBar.shows(TileCategory::SystemMonitor));
        assert!(!Placement::MobileThumbSidebar.shows(TileCategory::SystemMonitor));
        assert!(Placement::DesktopBottomBar.shows(TileCategory::SystemMonitor));
    }

    #[test]
    fn every_placement_shows_launcher_and_taskbar() {
        for p in [Placement::LaptopTopBar, Placement::MobileThumbSidebar, Placement::DesktopBottomBar] {
            assert!(p.shows(TileCategory::Launcher));
            assert!(p.shows(TileCategory::Taskbar));
        }
    }

    #[test]
    fn each_placement_maps_to_the_matching_layer_shape() {
        let top = Placement::LaptopTopBar.layer_intent(32);
        assert!(top.anchor.top && !top.anchor.bottom);

        let side = Placement::MobileThumbSidebar.layer_intent(72);
        assert!(side.anchor.right && !side.anchor.left);

        let bottom = Placement::DesktopBottomBar.layer_intent(48);
        assert!(bottom.anchor.bottom && !bottom.anchor.top);
    }
}
