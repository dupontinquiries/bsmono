//! Spacing, typography, radius, and animation tokens — shared across all palettes.
//!
//! SOURCE OF TRUTH: these mirror the device-tuned constants in the C++
//! `include/ui/Theme.hpp`, which is authoritative (it's what the live FP5 app
//! renders from and has been hand-tuned down over several versions for the
//! small screen). V32.34+: re-synced after drift was found — the font/space
//! scales here still held the OLD pre-tightening values (e.g. FONT_BASE 15
//! vs the current 13, SPACE_XL 48 vs 43), so any backend that wired this
//! crate would have rendered noticeably looser/larger than the Qt UI. When
//! Theme.hpp changes, update these to match.

/// Spacing scale in pixels. Matches Theme:: SPACE_* in Theme.hpp.
pub struct Spacing;
impl Spacing {
    pub const XS:  i32 = 6;
    pub const SM:  i32 = 10;
    pub const MD:  i32 = 18;
    pub const LG:  i32 = 28;
    pub const XL:  i32 = 43;
}

/// Type scale in pixels. Matches Theme:: FONT_* in Theme.hpp.
/// (Theme.hpp defines XS/SM/BASE/LG; the larger steps below extend the
/// scale proportionally for backends that need them.)
pub struct Typography;
impl Typography {
    pub const XS:   i32 = 10;
    pub const SM:   i32 = 11;
    pub const BASE: i32 = 13;
    pub const LG:   i32 = 16;
    pub const XL:   i32 = 20;
    pub const XXL:  i32 = 25;
}

pub struct Radius;
impl Radius {
    pub const SM:   i32 = 8;
    pub const MD:   i32 = 14;
    pub const LG:   i32 = 20;
    pub const XL:   i32 = 28;
    pub const FULL: i32 = 999;
}

pub struct Touch;
impl Touch {
    pub const MIN:   i32 = 40;
    pub const ICON:  i32 = 52;
    pub const LARGE: i32 = 64;
}

pub struct Animation;
impl Animation {
    pub const FAST:   i32 = 110;
    pub const NORMAL: i32 = 220;
    pub const SLOW:   i32 = 340;
    /// Spring: OutBack easing overshoot factor for page switches.
    pub const SPRING_OVERSHOOT: f64 = 1.08;
}
