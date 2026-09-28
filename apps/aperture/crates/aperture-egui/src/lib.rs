//! aperture-egui — the shell. See `state` for the brain (headless, tested),
//! `view` for the paint, `camera` for the motion, `ghost` for the suggestion
//! machine, `loop_pane` for the steerable internals, `palette_ui` for Ctrl+P.
//! The `aperture` binary (feature `app`, default-on) is a thin eframe boot.

pub mod camera;
pub mod file_finder;
pub mod ghost;
pub mod loop_pane;
pub mod lsp_worker;
pub mod page;
pub mod palette_ui;
pub mod touch_input;
pub mod state;
pub mod view;
