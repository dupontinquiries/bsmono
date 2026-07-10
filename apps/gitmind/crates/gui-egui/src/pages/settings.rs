//! Settings page — edits the app's `Prefs` live. Because the shell calls `uikit_egui::apply_prefs`
//! every frame, changing these re-themes the whole window immediately: the animation picker (now
//! actually wired through the kit), palette, text size, and surface transparency.
use crate::app::App;
use settings_kit::AnimationStyle;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Settings");
    ui.separator();
    ui.label(
        egui::RichText::new("this window's local prefs — the fleet's central settings app is bsettings (bigsilicon-settings), which publishes changes to every app live via settingsd")
            .weak()
            .small(),
    );
    ui.add_space(6.0);

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

    // Text size re-scales the whole window, so applying it live makes the slider jump under the
    // cursor. Draft it in memory during the drag and only commit to prefs on release. Reduced range
    // with quarter-step detents.
    {
        let id = egui::Id::new("gm_text_scale_draft");
        let mut draft: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(app.prefs.touch.text_scale);
        let resp = ui.add(egui::Slider::new(&mut draft, 0.75..=1.5).step_by(0.25).text("text size"));
        if resp.dragged() {
            // hold the in-progress value; do NOT apply yet (that's what bugs out)
            ui.data_mut(|d| d.insert_temp(id, draft));
        } else {
            if resp.drag_stopped() || resp.changed() {
                app.prefs.touch.text_scale = draft; // commit on release / click / keyboard
            }
            // keep the draft synced to the live pref while idle
            ui.data_mut(|d| d.insert_temp(id, app.prefs.touch.text_scale));
        }
    }
    ui.add(egui::Slider::new(&mut app.prefs.display.opacity, 0.4..=1.0).text("surface opacity"));

    ui.add_space(10.0);
    ui.label(
        egui::RichText::new("changes apply live — the window re-themes each frame from prefs")
            .weak()
            .small(),
    );
}
