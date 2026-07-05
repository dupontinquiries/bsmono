//! chat_layout — device-adaptive layout decisions for the chat tab.
//!
//! The chat tab grew up desktop-only with functional-but-placeholder controls. This computes how
//! it should arrange itself for the current device from one code path, so the render code (the
//! big `tabs::chat` module) just reads these flags instead of hard-coding a desktop layout — and
//! it's ready for the physical-UI panel (which reports as a compact, touch class). Built on
//! `uikit_egui::adaptive::DeviceClass` so it matches the rest of the kit. Pure + testable; no egui
//! needed, so it builds in the lean (headless) crate too.

use uikit_egui::adaptive::DeviceClass;

/// How the chat tab should arrange itself for the current device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChatLayout {
    /// Scene + transcript side-by-side (desktop/tablet) vs stacked/overlaid (phone).
    pub split: bool,
    /// Phone: render the 3D scene as a translucent backdrop *behind* the transcript to save space
    /// (pairs with uikit-egui's surface transparency).
    pub scene_as_backdrop: bool,
    /// Dock the input bar to the bottom edge (thumb reach) instead of inline.
    pub input_docked_bottom: bool,
    /// Minimum touch target for buttons/controls (points) — the placeholder button rows reflow to
    /// this on touch devices.
    pub touch_target: f32,
    /// Cap transcript width so lines stay readable on wide screens (full width on phone).
    pub transcript_max_width: f32,
    /// Spacing between panes/rows.
    pub gutter: f32,
    /// Collapse the control cluster into an overflow menu (phone) vs show inline (desktop).
    pub controls_overflow: bool,
}

/// Decide the chat layout from device class, available width, and whether input is touch.
pub fn chat_layout(class: DeviceClass, avail_width: f32, touch: bool) -> ChatLayout {
    let compact = class.is_compact(); // phone
    ChatLayout {
        split: !compact && avail_width >= 720.0,
        scene_as_backdrop: compact,
        input_docked_bottom: compact || touch,
        touch_target: if touch { 44.0 } else { 28.0 },
        transcript_max_width: if compact { avail_width } else { 760.0 },
        gutter: if compact { 8.0 } else { 12.0 },
        controls_overflow: compact,
    }
}

/// Convenience: decide straight from the available width using the kit's default breakpoints.
pub fn chat_layout_for_width(avail_width: f32, touch: bool) -> ChatLayout {
    chat_layout(DeviceClass::from_width(avail_width), avail_width, touch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_stacks_scene_and_docks_input() {
        let l = chat_layout(DeviceClass::Phone, 400.0, true);
        assert!(!l.split, "phone is single-column");
        assert!(l.scene_as_backdrop, "scene goes behind the transcript");
        assert!(l.input_docked_bottom, "input docks to thumb reach");
        assert!(l.controls_overflow, "controls collapse to an overflow menu");
        assert_eq!(l.touch_target, 44.0);
        assert_eq!(l.transcript_max_width, 400.0, "full width on phone");
    }

    #[test]
    fn desktop_splits_and_caps_width() {
        let l = chat_layout(DeviceClass::Desktop, 1600.0, false);
        assert!(l.split, "wide desktop shows scene + transcript side by side");
        assert!(!l.scene_as_backdrop);
        assert!(!l.controls_overflow, "controls inline on desktop");
        assert_eq!(l.touch_target, 28.0);
        assert_eq!(l.transcript_max_width, 760.0, "capped for readability");
    }

    #[test]
    fn narrow_desktop_window_does_not_split() {
        // a desktop-class but narrowed window falls back to single column
        let l = chat_layout(DeviceClass::Desktop, 680.0, false);
        assert!(!l.split);
    }

    #[test]
    fn from_width_picks_the_class() {
        assert!(chat_layout_for_width(400.0, true).scene_as_backdrop, "narrow -> phone");
        assert!(chat_layout_for_width(1400.0, false).split, "wide -> desktop split");
    }
}
