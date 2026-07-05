//! tab_layout — adaptive layout archetypes shared by logscope's non-chat tabs, so each works on
//! phone and desktop from one decision path (the sibling of `chat_layout`). Most tabs are one of
//! two shapes: a **master/detail** list (sources, rotate, cargo) or a **content + controls** view
//! (peek, code, charts). The render code reads these flags instead of hard-coding a desktop
//! layout. Pure + lean-buildable (no egui), built on uikit-egui's `DeviceClass`.

use uikit_egui::adaptive::DeviceClass;

/// Master/detail tabs: a list of items plus a detail pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MasterDetail {
    /// List + detail side-by-side (desktop/tablet) vs drill-in navigation (phone).
    pub split: bool,
    /// Fraction of width the list takes when split.
    pub list_fraction: f32,
    /// Phone: tapping a row pushes a full-screen detail (uikit-egui `nav`), with a back gesture.
    pub drill_navigation: bool,
    /// Row height (bigger on touch so rows are tappable).
    pub row_height: f32,
}

pub fn master_detail(class: DeviceClass, width: f32, touch: bool) -> MasterDetail {
    let compact = class.is_compact();
    MasterDetail {
        split: !compact && width >= 640.0,
        list_fraction: if matches!(class, DeviceClass::Tablet) { 0.40 } else { 0.30 },
        drill_navigation: compact,
        row_height: if touch { 40.0 } else { 24.0 },
    }
}

/// Where a tab's control/toolbar cluster lives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Toolbar {
    /// Alongside the content (wide desktop).
    Side,
    /// Docked to the bottom edge (medium widths / touch reach).
    BottomDocked,
    /// Collapsed into an overflow menu (phone).
    Overflow,
}

/// Content tabs: a main view plus a controls cluster.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContentLayout {
    pub toolbar: Toolbar,
    /// Cap content width for readability (full width on phone).
    pub content_max_width: f32,
    pub gutter: f32,
}

pub fn content_layout(class: DeviceClass, width: f32) -> ContentLayout {
    let compact = class.is_compact();
    let toolbar = if compact {
        Toolbar::Overflow
    } else if width < 900.0 {
        Toolbar::BottomDocked
    } else {
        Toolbar::Side
    };
    ContentLayout {
        toolbar,
        content_max_width: if compact { width } else { (width * 0.9).min(1100.0) },
        gutter: if compact { 8.0 } else { 12.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_master_detail_drills_in() {
        let m = master_detail(DeviceClass::Phone, 400.0, true);
        assert!(!m.split, "phone shows list OR detail, not both");
        assert!(m.drill_navigation, "tap a row -> full-screen detail");
        assert_eq!(m.row_height, 40.0, "tappable rows");
    }

    #[test]
    fn desktop_master_detail_splits() {
        let m = master_detail(DeviceClass::Desktop, 1400.0, false);
        assert!(m.split);
        assert!(!m.drill_navigation);
        assert!((m.list_fraction - 0.30).abs() < 1e-6);
        // tablet gives the list a bit more room
        assert!((master_detail(DeviceClass::Tablet, 900.0, true).list_fraction - 0.40).abs() < 1e-6);
    }

    #[test]
    fn toolbar_moves_with_width() {
        assert_eq!(content_layout(DeviceClass::Phone, 400.0).toolbar, Toolbar::Overflow);
        assert_eq!(content_layout(DeviceClass::Tablet, 820.0).toolbar, Toolbar::BottomDocked);
        assert_eq!(content_layout(DeviceClass::Desktop, 1500.0).toolbar, Toolbar::Side);
    }

    #[test]
    fn content_width_capped_on_desktop_full_on_phone() {
        assert_eq!(content_layout(DeviceClass::Phone, 380.0).content_max_width, 380.0);
        assert_eq!(content_layout(DeviceClass::Desktop, 2000.0).content_max_width, 1100.0);
    }
}
