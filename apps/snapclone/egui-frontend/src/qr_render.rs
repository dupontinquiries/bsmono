//! qr_render — `QrCodeRenderer.cpp`, ported — now lives in the kit as `uikit_egui::qr`, so
//! any app that shows an id draws it the same way: theme colours with the WCAG guard, an
//! integer cell at device pixel ratio, rounded finder eyes and dotted data modules on a
//! glass card, and the expanded full-screen overlay with its open guard and fade. This
//! module is the name snapclone's code and tests used; everything is the kit's.
pub use uikit_egui::qr::*;
