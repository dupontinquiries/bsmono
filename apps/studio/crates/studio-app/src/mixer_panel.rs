//! mixer_panel — per-track balance for the arrangement.
//!
//! Three controls a track needs before an export is a mix and not a sum:
//! a fader (`Op::SetTrackGain`), mute (`Op::SetTrackFlags` — persisted, on
//! the wire, in undo), and solo — which is deliberately NOT an op. Solo is
//! monitoring state, the same species as `studio-engine`'s cue level: it
//! answers "what am I listening to right now", never "what is this
//! project", so it lives on the local `Studio` and only dirties the render.
//! A solo that replayed into a collaborator's project — or into your own
//! undo history — is the classic DAW-clone mistake this codebase's command
//! docs already warn about.
//!
//! ## Faders commit on release
//!
//! A slider emits a value every frame of a drag; an op per frame would be
//! sixty undo entries and sixty packets per gesture — the exact flood the
//! grid's `Paint` exists to prevent, one layer up. So the drag paints a
//! LOCAL preview (`Studio::mixer_drag`), the render follows it live via the
//! dirty flag, and one `SetTrackGain` lands when the finger lifts: one
//! gesture, one op, one undo step.

use uikit_egui::PaletteColors;

use crate::Studio;

/// The effects a "+ fx" click can add, with sane defaults — the registry in
/// `studio_fx::instantiate` is string-keyed, so this list is the UI's
/// curated door into it, not a second source of truth: an unknown kind
/// still instantiates to None and is skipped, exactly as before.
pub const ADDABLE_FX: &[(&str, &[(&str, f32)])] = &[
    ("saturation", &[("profile", 1.0), ("drive", 0.35)]), // 1 = tape
    ("eq", &[("bands", 1.0), ("freq0", 1000.0), ("q0", 0.707), ("gain0", 0.0)]),
    ("compressor", &[("threshold", -18.0), ("ratio", 4.0)]),
    ("limiter", &[("ceiling", -0.3)]),
    ("color", &[]),
    // Live note-locking pitch correction. Defaults to A-minor key-lock;
    // an explicit note set is params notes/note0.. (see the registry).
    ("autotune", &[("root", 9.0), ("mode", 0.0), ("speed", 30.0)]),
    // Saturation that follows the material: more drive down low, less on
    // sibilance. `tied` 1.0 makes the de-essing register-dependent.
    ("adaptivesat", &[("profile", 1.0), ("drive", 0.4),
                      ("note_amount", 0.5), ("sib_amount", 0.5), ("tied", 0.0)]),
    // The playrate pair — a tape stop on the master and halftime on a drum
    // bus are the two most-asked trap moves, and both are now real DSP
    // behind a chain chip rather than a plugin you must go find.
    ("tapestop", &[("start", 0.0), ("length", 1.0), ("channels", 2.0)]),
    ("halftime", &[("window", 0.5), ("channels", 2.0)]),
];

pub fn effect(kind: &str, params: &[(&str, f32)]) -> studio_core::Effect {
    studio_core::Effect {
        kind: kind.into(),
        params: params.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
        enabled: true,
    }
}

pub fn show(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| show_inner(ui, s, c));
}

fn show_inner(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors) {
    let tracks: Vec<studio_core::Track> = s.session.doc.project.tracks.clone();
    if tracks.is_empty() {
        ui.colored_label(c.text_weak, "No tracks.");
        return;
    }
    for t in &tracks {
        ui.horizontal(|ui| {
            let kind = match t.kind {
                studio_core::TrackKind::Audio => "♪",
                studio_core::TrackKind::Video => "▣",
            };
            ui.label(format!("{kind} {}", t.name));

            // Mute is a project fact: an op, so it undoes and travels.
            let mut audible = !t.muted;
            if ui.checkbox(&mut audible, "M").on_hover_text("muted (an edit)").changed() {
                s.edit(studio_core::Op::SetTrackFlags {
                    id: t.id, muted: !audible, locked: t.locked,
                }, if audible { "unmuted" } else { "muted" });
            }

            // Solo is a listening fact: local, dirty-only.
            let soloed = s.solo.contains(&t.id);
            if ui.add(egui::Button::selectable(soloed, "S"))
                .on_hover_text("solo (monitoring only — not an edit)").clicked()
            {
                if soloed { s.solo.retain(|id| *id != t.id); }
                else { s.solo.push(t.id); }
                s.arrangement_dirty = true;
            }

            // The fader: preview locally while dragging, one op on release.
            let mut shown = match s.mixer_drag {
                Some((id, v)) if id == t.id => v,
                _ => t.gain,
            };
            let resp = ui.add(egui::Slider::new(&mut shown, 0.0..=1.5)
                .text("gain").custom_formatter(|v, _| {
                    if v <= 0.0 { "-inf dB".into() }
                    else { format!("{:+.1} dB", 20.0 * v.log10()) }
                }));
            if resp.dragged() {
                // Visual preview only: the AUDIO follows on release, because
                // "audible while dragging" here would mean a full arrangement
                // re-render per frame (this shell's whole-buffer trade), and
                // the project wouldn't even hold the preview value to render
                // from. A live fader ride is the streaming DeviceEngine's
                // job, named as future work — not something to fake badly.
                s.mixer_drag = Some((t.id, shown));
            }
            if resp.drag_stopped() {
                s.mixer_drag = None;
                s.edit(studio_core::Op::SetTrackGain { id: t.id, gain: shown },
                       "gain");
            } else if resp.changed() && !resp.dragged() {
                // Keyboard / click-set: no drag lifecycle, commit at once.
                s.edit(studio_core::Op::SetTrackGain { id: t.id, gain: shown },
                       "gain");
            }
        });
    }
    // ── busses ──────────────────────────────────────────────────────────
    // The routing half the model always had and this panel never showed:
    // tracks feed busses, busses feed busses or master, every hop with its
    // own gain, mute, and effect chain — the drum-bus-into-saturation shape.
    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("busses");
        if ui.button("+ bus").clicked() {
            let id = studio_core::BusId(
                s.session.doc.project.busses.keys().map(|b| b.0).max().unwrap_or(0) + 1);
            s.edit(studio_core::Op::AddBus { bus: studio_core::Bus {
                id, name: format!("Bus {}", id.0), parent: None,
                gain: 1.0, chain: Vec::new(), sends: Vec::new(), muted: false,
            } }, "bus added");
        }
    });
    let busses: Vec<studio_core::Bus> = s.session.doc.project.busses.values().cloned().collect();
    for b in &busses {
        ui.horizontal(|ui| {
            ui.label(format!("◆ {}", b.name));
            let mut audible = !b.muted;
            if ui.checkbox(&mut audible, "M").changed() {
                s.edit(studio_core::Op::SetBusFlags { id: b.id, muted: !audible }, "bus mute");
            }
            // Same commit-on-release discipline as the track faders.
            let mut shown = match s.bus_drag {
                Some((id, v)) if id == b.id => v,
                _ => b.gain,
            };
            let resp = ui.add(egui::Slider::new(&mut shown, 0.0..=1.5).text("gain")
                .custom_formatter(|v, _| if v <= 0.0 { "-inf dB".into() }
                                  else { format!("{:+.1} dB", 20.0 * v.log10()) }));
            if resp.dragged() { s.bus_drag = Some((b.id, shown)); }
            if resp.drag_stopped() {
                s.bus_drag = None;
                s.edit(studio_core::Op::SetBusGain { id: b.id, gain: shown }, "bus gain");
            } else if resp.changed() && !resp.dragged() {
                s.edit(studio_core::Op::SetBusGain { id: b.id, gain: shown }, "bus gain");
            }
            ui.label("→");
            let parent_label = b.parent
                .and_then(|p| busses.iter().find(|x| x.id == p))
                .map(|x| x.name.clone()).unwrap_or_else(|| "master".into());
            ui.label(parent_label);
        });
        // Sends: the parallel taps. A reverb bus is not something drums
        // ROUTE into (that would replace the dry signal) — it is something
        // they send a copy to, which is why the model has both and why the
        // UI has to as well, or "reverb bus" is unbuildable here.
        ui.horizontal_wrapped(|ui| {
            ui.add_space(18.0);
            ui.label("sends:");
            let mut sends = b.sends.clone();
            let mut changed = false;
            let mut remove: Option<usize> = None;
            for (i, send) in sends.iter_mut().enumerate() {
                let to = busses.iter().find(|x| x.id == send.to)
                    .map(|x| x.name.clone()).unwrap_or_else(|| format!("bus {}", send.to.0));
                ui.label(format!("→ {to}"));
                let mut lvl = send.level;
                let resp = ui.add(egui::Slider::new(&mut lvl, 0.0..=1.0)
                    .show_value(false).text(""));
                // Sends commit on release like every other fader here; a
                // value per drag frame would be an op per frame.
                if resp.drag_stopped() || (resp.changed() && !resp.dragged()) {
                    send.level = lvl;
                    changed = true;
                }
                let mut pre = send.pre_fader;
                if ui.checkbox(&mut pre, "pre").on_hover_text(
                    "tap before this bus's own fader").changed() {
                    send.pre_fader = pre;
                    changed = true;
                }
                if ui.small_button("✕").clicked() { remove = Some(i); }
            }
            if let Some(i) = remove { sends.remove(i); changed = true; }
            // Only busses that are not this one and do not already receive:
            // a bus sending to itself is a feedback loop the mixdown would
            // have to defend against, and a duplicate send is two faders
            // fighting over one path.
            let candidates: Vec<_> = busses.iter()
                .filter(|x| x.id != b.id && !sends.iter().any(|s| s.to == x.id))
                .collect();
            if !candidates.is_empty() {
                ui.menu_button("+ send", |ui| {
                    for target in candidates {
                        if ui.button(&target.name).clicked() {
                            sends.push(studio_core::Send {
                                to: target.id, level: 0.3, pre_fader: false });
                            changed = true;
                            ui.close();
                        }
                    }
                });
            }
            if changed {
                s.edit(studio_core::Op::SetBusSends { id: b.id, sends }, "bus sends");
            }
        });

        // The chain: chips in order, each removable; whole-chain SetBusChain
        // is the op shape the model chose, so one click is one op.
        ui.horizontal_wrapped(|ui| {
            ui.add_space(18.0);
            let mut chain = b.chain.clone();
            let mut changed = false;
            let mut remove: Option<usize> = None;
            for (i, fx) in chain.iter().enumerate() {
                if ui.small_button(format!("{} ✕", fx.kind))
                    .on_hover_text("remove").clicked() {
                    remove = Some(i);
                }
            }
            if let Some(i) = remove { chain.remove(i); changed = true; }
            ui.menu_button("+ fx", |ui| {
                for (kind, params) in ADDABLE_FX {
                    if ui.button(*kind).clicked() {
                        chain.push(effect(kind, params));
                        changed = true;
                        ui.close();
                    }
                }
            });
            if changed {
                s.edit(studio_core::Op::SetBusChain { id: b.id, chain }, "bus chain");
            }
        });
    }
    if !busses.is_empty() {
        ui.add_space(4.0);
        // Track routing lives with the busses it points at.
        for t in &tracks {
            if t.kind != studio_core::TrackKind::Audio { continue; }
            ui.horizontal(|ui| {
                ui.label(format!("{} →", t.name));
                let current = t.route
                    .and_then(|p| busses.iter().find(|x| x.id == p))
                    .map(|x| x.name.clone()).unwrap_or_else(|| "master".into());
                egui::ComboBox::from_id_salt(("route", t.id))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(t.route.is_none(), "master").clicked()
                            && t.route.is_some() {
                            s.edit(studio_core::Op::SetRoute { track: t.id, bus: None }, "routed");
                        }
                        for b in &busses {
                            if ui.selectable_label(t.route == Some(b.id), &b.name).clicked()
                                && t.route != Some(b.id) {
                                s.edit(studio_core::Op::SetRoute {
                                    track: t.id, bus: Some(b.id) }, "routed");
                            }
                        }
                    });
            });
        }
    }

    if !s.solo.is_empty() {
        ui.add_space(4.0);
        ui.colored_label(c.warning,
            format!("solo: {} track(s) — monitoring only, exports follow it",
                    s.solo.len()));
    }
}
