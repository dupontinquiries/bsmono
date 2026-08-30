//! The snapshot-cam shell. The core (`lib.rs`) owns what a photo IS and where
//! it may go; this owns the window.
//!
//! Deliberately thin, and behind the `app` feature, so `--no-default-features`
//! tests the rules without a camera or a display attached.

use snapshot_cam::{Roll, ShareTarget, Shot, capture_filename};

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "snapshot-cam",
        // Glow SAID, not merely implied by the feature list.
        //
        // This crate's Cargo.toml documents the crash it learned first:
        // eframe 0.34 defaults include wgpu but not glow, so
        // Renderer::default() picks wgpu, whose default device
        // descriptor asks for 8 colour attachments and dies on a phone
        // GPU that allows 5.
        //
        // The manifest fix works only while nothing else in the build
        // pulls egui-wgpu in. gitmind-egui declared the same glow-only
        // features and still got wgpu through feature unification — so
        // relying on the dependency graph to hold this steady is the
        // part that already failed once, elsewhere.
        eframe::NativeOptions {
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        },
        Box::new(|_cc| Ok(Box::new(Cam::default()))),
    )
}

#[derive(Default)]
struct Cam {
    roll: Roll,
    /// The viewfinder's geometry. 16:9 to match what the capture stub
    /// records, so the preview and the file agree about shape.
    preview: snapshot_cam::preview::Preview,
    /// The contact to send to. Free text until snapclone's contact list is
    /// wired in — a picker over a list this app cannot yet read would be a
    /// dropdown with nothing in it.
    contact: String,
    note: String,
}

impl eframe::App for Cam {
    // `ui`, not `update`. eframe 0.34 makes `ui` the required method and
    // deprecates calling CentralPanel::show with a Context from inside a
    // panel — the compiler asked for exactly this signature, and only this app
    // still had the old shape, which is why it was the one that did not build.
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui.ctx(), |ui| {
            ui.heading("snapshot-cam");
            ui.label(egui::RichText::new(
                "photos stay on this device until you send them").weak().small());

            // THE VIEWFINDER, above the shutter where a camera app puts
            // it. 16:9 because that is what the capture stub records
            // (1920x1080) — the preview and the photo agreeing about
            // shape is the whole point, and a preview at a different
            // aspect from the file would mis-teach framing.
            //
            // A tap sets focus in normalised sensor coordinates, which
            // is the geometry a real frame source will need; nothing
            // consumes it yet, so it is shown rather than stored.
            let shown = snapshot_cam::preview::show(
                ui, &self.preview,
                "no camera attached — this is where the photo will be");
            if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                if ui.input(|i| i.pointer.primary_clicked()) {
                    if let Some(s) = self.preview.to_sensor(pos, shown) {
                        self.note = format!("focus {:.2}, {:.2}", s[0], s[1]);
                    }
                }
            }
            ui.horizontal(|ui| {
                if ui.selectable_label(
                    self.preview.facing == snapshot_cam::preview::Facing::Front,
                    "front").clicked()
                {
                    self.preview.facing = snapshot_cam::preview::Facing::Front;
                }
                if ui.selectable_label(
                    self.preview.facing == snapshot_cam::preview::Facing::Rear,
                    "rear").clicked()
                {
                    self.preview.facing = snapshot_cam::preview::Facing::Rear;
                }
                ui.checkbox(&mut self.preview.full_bleed, "fill");
            });

            if ui.button("take photo").clicked() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64).unwrap_or(0);
                // The capture itself is a stub: wiring a real frame means
                // snapclone's camera_select + frame_pump, which belong to that
                // app's shell. The ROLL and the sharing rules are what this
                // crate exists to get right, and they are real.
                let name = capture_filename(now, "front", "jpg");
                self.roll.push(Shot::new(&name, 1920, 1080, now, "front"));
                self.note = format!("captured {name}");
            }

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label("send to:");
                ui.text_edit_singleline(&mut self.contact);
            });

            if !self.note.is_empty() {
                ui.label(egui::RichText::new(&self.note).weak().small());
            }

            ui.add_space(8.0);
            ui.separator();
            // Counts first: "what have I shared" answered without scrolling.
            ui.label(format!("{} photo(s) · {} local · {} shared with someone",
                             self.roll.len(),
                             self.roll.local_only().len(),
                             self.roll.disclosed().len()));

            let contact = self.contact.trim().to_string();
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut acted: Option<(usize, ShareTarget)> = None;
                for (i, s) in self.roll.shots().iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&s.file).monospace().small());
                        // The DISPOSITION is always visible, not on hover: a
                        // photo somebody else can see should say so from across
                        // the room.
                        ui.label(egui::RichText::new(if s.disclosed() { "shared" } else { "local" })
                            .small().weak());
                        if s.is_local() {
                            if ui.small_button("back up").clicked() {
                                acted = Some((i, ShareTarget::PhotoSync));
                            }
                            if !contact.is_empty()
                                && ui.small_button(format!("send to {contact}")).clicked()
                            {
                                acted = Some((i, ShareTarget::Contact(contact.clone())));
                            }
                        }
                    });
                }
                if let Some((i, target)) = acted {
                    let label = target.label();
                    match self.roll.shots_mut()[i].share_to(target) {
                        // Pending, not sent: the transport is not wired, and
                        // claiming success here would be the exact lie the core
                        // is written to avoid.
                        Ok(()) => self.note = format!("queued for {label}"),
                        Err(e) => self.note = e,
                    }
                }
            });
        });
    }
}
