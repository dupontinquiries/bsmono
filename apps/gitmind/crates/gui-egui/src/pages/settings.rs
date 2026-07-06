//! Settings page — edits the app's `Prefs` live. Because the shell calls `uikit_egui::apply_prefs`
//! every frame, changing these re-themes the whole window immediately: the animation picker (now
//! actually wired through the kit), palette, text size, and surface transparency.
use crate::app::App;
use settings_kit::AnimationStyle;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Settings");
    ui.separator();

    ui.label(egui::RichText::new("animation").strong());
    ui.horizontal_wrapped(|ui| {
        for a in AnimationStyle::all() {
            if ui
                .selectable_label(app.prefs.gesture.animation == a, a.label())
                .clicked()
            {
                app.prefs.gesture.animation = a;
            }
        }
    });
    ui.add_space(8.0);

    ui.label(egui::RichText::new("palette").strong());
    ui.horizontal_wrapped(|ui| {
        for p in snapclone_theme::PaletteId::all() {
            let sel = app.prefs.palette == p.as_str();
            if ui.selectable_label(sel, p.as_str()).clicked() {
                app.prefs.palette = p.as_str().to_string();
            }
        }
    });
    ui.add_space(8.0);

    ui.add(egui::Slider::new(&mut app.prefs.touch.text_scale, 0.7..=1.6).text("text size"));
    ui.add(egui::Slider::new(&mut app.prefs.display.opacity, 0.4..=1.0).text("surface opacity"));

    ui.add_space(10.0);
    ui.label(
        egui::RichText::new("changes apply live — the window re-themes each frame from prefs")
            .weak()
            .small(),
    );
}
