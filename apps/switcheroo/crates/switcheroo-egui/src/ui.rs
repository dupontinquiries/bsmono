//! The drawing layer.
//!
//! Layout discipline here is deliberate and differs from several other
//! apps in this fleet, which currently let settings content flow over the
//! bottom navigation (see C13_TODO §24). The rule followed throughout:
//! **reserve chrome first, then give the remainder to exactly one
//! `ScrollArea`.** A `TopBottomPanel` claims its height before the central
//! panel is laid out, so the scroll region can never overlap it no matter
//! how much content there is.

use crate::{App, Tab, STALE_AFTER};
use std::time::Instant;
use switcheroo_core::{Device, DeviceId, Power};

/// Minimum row height. A thumb is roughly 9mm; at the 300% scale these
/// compositors commonly use, 56 logical px stays a comfortable target
/// without the row growing so tall that only three fit on screen.
const ROW_H: f32 = 56.0;

pub fn draw(app: &mut App, ctx: &egui::Context) {
    // `Panel::top`/`Panel::bottom` with `.show(ctx, ..)` is this fleet's egui
    // 0.34 form — `TopBottomPanel` is a deprecated alias. Verified against
    // dialer-egui rather than guessed; writing this crate from remembered API
    // is what produced the missing `eframe::App::ui` that stopped it building.

    app.tick(Instant::now());
    // BLE state changes arrive on a background thread, so the UI cannot
    // wait for input to repaint or a toggle would appear to hang.
    ctx.request_repaint_after(std::time::Duration::from_millis(250));

    top_bar(app, ctx);
    bottom_bar(app, ctx);

    egui::CentralPanel::default().show(ctx, |ui| match app.tab {
        Tab::Devices => devices_pane(app, ui),
        Tab::Activity => activity_pane(app, ui),
        Tab::About => about_pane(app, ui),
    });
}

fn top_bar(app: &mut App, ctx: &egui::Context) {
    egui::Panel::top("switcheroo-top").show(ctx, |ui| {
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.heading("switcheroo");
            ui.separator();
            if app.hub.scanning {
                ui.spinner();
                if ui.button("stop").clicked() {
                    app.stop_scan();
                }
            } else if ui.button("scan").clicked() {
                app.start_scan();
            }
            ui.separator();
            // horizontal_wrapped so a long adapter name on a narrow phone
            // wraps instead of pushing the buttons off the right edge.
            ui.label(egui::RichText::new(app.backend.describe()).weak().small());
        });
        if let Some(note) = &app.fallback_note {
            ui.label(
                egui::RichText::new(note)
                    .small()
                    .color(egui::Color32::from_rgb(230, 170, 90)),
            );
        }
        ui.add_space(4.0);
    });
}

fn bottom_bar(app: &mut App, ctx: &egui::Context) {
    egui::Panel::bottom("switcheroo-bottom").show(ctx, |ui| {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            for (tab, label) in [
                (Tab::Devices, "outlets"),
                (Tab::Activity, "activity"),
                (Tab::About, "about"),
            ] {
                if ui
                    .add(egui::Button::selectable(app.tab == tab, label))
                    .clicked()
                {
                    app.tab = tab;
                }
            }
        });
        ui.add_space(2.0);
    });
}

fn devices_pane(app: &mut App, ui: &mut egui::Ui) {
    if app.hub.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.label(if app.hub.scanning {
                "Scanning…"
            } else {
                "No outlets yet — press scan."
            });
        });
        return;
    }

    // Collect the actions first: the row closures borrow `app.hub`
    // immutably, and applying a toggle needs `&mut app`. Deferring one
    // frame would make the button feel laggy, so instead the ids are
    // gathered and applied immediately after the scroll area closes.
    let mut to_toggle: Option<DeviceId> = None;
    let mut to_refresh: Option<DeviceId> = None;
    let mut to_pin: Option<DeviceId> = None;
    let now = Instant::now();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for d in app.hub.sorted() {
                let (tog, refr, pin) = device_row(ui, d, now);
                if tog {
                    to_toggle = Some(d.id.clone());
                }
                if refr {
                    to_refresh = Some(d.id.clone());
                }
                if pin {
                    to_pin = Some(d.id.clone());
                }
            }
            ui.add_space(8.0);
        });

    if let Some(id) = to_toggle {
        app.toggle(&id);
    }
    if let Some(id) = to_refresh {
        app.refresh(&id);
    }
    if let Some(id) = to_pin {
        app.hub.toggle_favourite(&id);
    }
}

/// One outlet. Returns (toggle_clicked, refresh_clicked, pin_clicked).
fn device_row(ui: &mut egui::Ui, d: &Device, now: Instant) -> (bool, bool, bool) {
    let stale = d.is_stale(now, STALE_AFTER);
    let mut toggled = false;
    let mut refreshed = false;
    let mut pinned = false;

    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_min_height(ROW_H);
            ui.horizontal(|ui| {
                // pin
                let star = if d.favourite { "★" } else { "☆" };
                if ui.small_button(star).on_hover_text("pin to top").clicked() {
                    pinned = true;
                }

                ui.vertical(|ui| {
                    let mut title = egui::RichText::new(&d.name).strong();
                    if stale {
                        title = title.weak();
                    }
                    ui.label(title);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            egui::RichText::new(d.kind.label())
                                .small()
                                .color(egui::Color32::from_rgb(122, 162, 247)),
                        );
                        if let Some(r) = d.rssi {
                            ui.label(egui::RichText::new(format!("{r} dBm")).small().weak());
                        }
                        if stale {
                            ui.label(
                                egui::RichText::new("out of range")
                                    .small()
                                    .color(egui::Color32::from_rgb(200, 140, 90)),
                            );
                        }
                    });
                    if let Some(err) = &d.last_error {
                        ui.label(
                            egui::RichText::new(err)
                                .small()
                                .color(egui::Color32::from_rgb(230, 110, 110)),
                        );
                    } else if let Some(why) = d.kind.unsupported_reason() {
                        // Say WHY a plug has no button. An inert row with no
                        // explanation is the worst outcome here — the user
                        // concludes the app is broken.
                        ui.label(egui::RichText::new(why).small().weak());
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if d.pending {
                        ui.spinner();
                    } else if d.kind.is_controllable() {
                        let (label, tint) = match d.power {
                            Power::On => ("ON", egui::Color32::from_rgb(120, 200, 130)),
                            Power::Off => ("OFF", egui::Color32::from_rgb(150, 150, 160)),
                            Power::Unknown => ("—", egui::Color32::from_rgb(180, 170, 110)),
                        };
                        let btn = egui::Button::new(
                            egui::RichText::new(label).strong().color(tint),
                        )
                        .min_size(egui::vec2(72.0, 40.0));
                        if ui.add(btn).clicked() {
                            toggled = true;
                        }
                        if ui.small_button("↻").on_hover_text("re-read state").clicked() {
                            refreshed = true;
                        }
                    } else {
                        ui.label(egui::RichText::new("no control").small().weak());
                    }
                });
            });
        });
    (toggled, refreshed, pinned)
}

fn activity_pane(app: &mut App, ui: &mut egui::Ui) {
    ui.label(
        egui::RichText::new(
            "Everything the radio was asked to do and everything it said back.",
        )
        .small()
        .weak(),
    );
    ui.separator();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if app.hub.journal().is_empty() {
                ui.label("nothing yet");
            }
            for line in app.hub.journal() {
                ui.label(egui::RichText::new(line).monospace().small());
            }
        });
}

fn about_pane(_app: &mut App, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.heading("switcheroo");
            ui.label("Tap a Bluetooth outlet to turn it on or off.");
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new("What works")
                    .strong(),
            );
            ui.label(
                "• Discovery of any BLE device in range, with signal strength.\n\
                 • On/off for plain GATT switches that expose the Automation IO \
                   digital-output characteristic (0x2A57).",
            );
            ui.add_space(8.0);
            ui.label(egui::RichText::new("What does not, yet").strong());
            ui.label(
                "• HomeKit accessories — including the Wemo WSP100 sold in Apple \
                 stores — are found and correctly identified, but cannot be \
                 switched. HomeKit requires a pairing handshake (SRP-6a against \
                 the 8-digit setup code, an Ed25519 key exchange, then an \
                 encrypted session) before an accessory honours any write. That \
                 is a real protocol implementation, not a missing UUID. See \
                 HOMEKIT.md.",
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(
                    "None of the radio code has been run against physical \
                     hardware yet.",
                )
                .small()
                .color(egui::Color32::from_rgb(230, 170, 90)),
            );
        });
}
