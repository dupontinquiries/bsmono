# ambient-learner egui port — where it stands

## Done
`src/egui_lib.rs` + `src/ui/egui/mod.rs` build and test with
`--no-default-features --features egui`, no GTK. 9 tests. Live page only.

## Blockers found and handled
* **240 errors on `--features egui`**: `main.rs` is 7,048 lines with nine cfg
  guards, so `gtk::`/`adw::`/`glib::` paths leak everywhere. Worked around with a
  separate library root rather than cfg-ing working GTK code.
* **eframe cannot be added to this crate**: iced 0.12 caps `web-sys` at 0.3.67,
  eframe 0.34 needs ≥0.3.77, and cargo resolves the whole manifest regardless of
  features. Both frontends are real, so neither was deleted.

## Next, in order
1. A windowed binary — either upgrade iced past 0.12, or put the shell in a thin
   separate crate that depends on this one.
2. History, Locations, Model, Sync, Map pages. The Live page is the pattern.
