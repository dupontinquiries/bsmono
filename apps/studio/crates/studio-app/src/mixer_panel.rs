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
    // The send pair. These defaults are an INSERT's (30% wet); the bus
    // menu turns `mix` up to 1.0 when the bus is a send target — see
    // `send_target_mix`. The delay's 321.4 ms is the dotted eighth at
    // 140 BPM; "sync delay to tempo" in the palette rewrites it.
    ("reverb", &[("size", 0.5), ("damp", 0.4), ("predelay", 10.0), ("width", 1.0),
                 ("mix", 0.3), ("channels", 2.0)]),
    ("delay", &[("time", 321.4), ("feedback", 0.4), ("damp", 5000.0), ("pingpong", 0.0),
                ("mix", 0.3), ("channels", 2.0)]),
];

/// Kinds whose `mix` should be all-wet on a bus other busses send to: a
/// reverb on a send return that also passes the dry would double every
/// track routed through it. Anything else keeps its insert default.
pub const SEND_KINDS: &[&str] = &["reverb", "delay"];

/// The effect to add to bus `to`: the catalog's defaults, with `mix`
/// raised to 1.0 for a send kind when some bus sends into `to` — the
/// one thing a return bus is for. Pure, so the rule is testable without
/// a panel.
pub fn effect_for_bus(kind: &str, params: &[(&str, f32)], to: studio_core::BusId,
                      busses: &[studio_core::Bus]) -> studio_core::Effect {
    let mut e = effect(kind, params);
    let is_return = busses.iter().any(|b| b.sends.iter().any(|sd| sd.to == to));
    if is_return && SEND_KINDS.contains(&kind) {
        e.params.insert("mix".into(), 1.0);
    }
    e
}

pub fn effect(kind: &str, params: &[(&str, f32)]) -> studio_core::Effect {
    studio_core::Effect {
        kind: kind.into(),
        params: params.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
        enabled: true,
    }
}

/// The context menu on a fader: aim the strip at this parameter's lane,
/// or mint a curve on it — gated to the selected clips (a bus fader
/// gated to a vocal clip is the structural sidechain), or bound always
/// when nothing is selected. What Gemini's read of FL called the
/// "context-menu parameter binding": the knob is where automation starts.
pub(crate) fn automate_menu(resp: &egui::Response, s: &mut Studio, target: studio_core::AutoTarget) {
    resp.context_menu(|ui| {
        if ui.button("automate: show lane").clicked() {
            s.auto_target = Some(target);
            s.auto_binding = None;
            s.status = "lane in the strip under the timeline".into();
            ui.close();
        }
        let sel = s.selection.len();
        let label = if sel > 0 {
            format!("automate: new curve gated to {sel} selected clip{}", if sel == 1 { "" } else { "s" })
        } else {
            "automate: new curve (always)".into()
        };
        if ui.button(label).clicked() {
            s.new_curve_on_selection(Some(target));
            ui.close();
        }
    });
}

pub fn show(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors)
    -> crate::scroll_edge::ScrollReport {
    // The map explains what the rows below edit: the whole graph at once,
    // tracks → busses → master, sends dashed. Collapsed state is egui's
    // own; default open, because 'where does my signal go' is the first
    // question this panel gets asked.
    egui::CollapsingHeader::new("routing map").default_open(true).show(ui, |ui| {
        egui::ScrollArea::horizontal().id_salt("routing-map").show(ui, |ui| {
            studio_egui::routing::routing_map(ui, &s.session.doc.project, c);
        });
    });
    ui.separator();
    // The mastering pass lives HERE — on the mixer, beside the busses it
    // would act on — not in a menu: mixing and mastering is what this
    // panel is for, and a tool you have to go looking for is a tool that
    // gets used once. The button only ASKS for the analysis; the window
    // it opens keeps diagnosis, dial and apply in that order.
    ui.horizontal(|ui| {
        let target = s.session.doc.project.busses.iter()
            .find(|(_, b)| b.parent.is_none())
            .map(|(id, b)| (*id, b.name.clone()));
        let enabled = target.is_some();
        if ui.add_enabled(enabled, egui::Button::new("mix & master…"))
            .on_hover_text("measure the mix and propose moves — nothing applies until you say so")
            .on_disabled_hover_text("add a bus feeding master first")
            .clicked() {
            if let Some((id, name)) = target {
                s.mix_pass_request = Some((id, name));
            }
        }
        ui.colored_label(c.text_weak, "analysis reads the current mixdown");
    });
    ui.separator();
    // ── inputs, per bus ─────────────────────────────────────────────────
    // Routing could only ever be edited from the SOURCE end (a track's
    // route combo) and, for busses, only at creation. A mixer strip reads
    // the other way — "what feeds me" — so each bus lists its inputs and
    // offers more: adding one is SetRoute/SetBusParent from the receiving
    // side, removing one sends the feeder back to master. A reparent that
    // would loop is refused by the op and the status line says so.
    let project = s.session.doc.project.clone();
    egui::CollapsingHeader::new("bus inputs").default_open(false).show(ui, |ui| {
        for (bid, bus) in &project.busses {
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!("{} ⇐", bus.name));
                for t in project.tracks.iter().filter(|t| t.route == Some(*bid)) {
                    if ui.small_button(format!("{} ✕", t.name))
                        .on_hover_text("send this track back to master").clicked() {
                        s.edit(studio_core::Op::SetRoute { track: t.id, bus: None },
                               "input removed");
                    }
                }
                for (cid, c) in project.busses.iter().filter(|(_, c)| c.parent == Some(*bid)) {
                    if ui.small_button(format!("bus {} ✕", c.name))
                        .on_hover_text("send this bus back to master").clicked() {
                        s.edit(studio_core::Op::SetBusParent { id: *cid, parent: None },
                               "input removed");
                    }
                }
                ui.menu_button("+ input", |ui| {
                    for t in project.tracks.iter()
                        .filter(|t| t.kind == studio_core::TrackKind::Audio
                                    && t.route != Some(*bid)) {
                        if ui.button(format!("track {}", t.name)).clicked() {
                            s.edit(studio_core::Op::SetRoute {
                                track: t.id, bus: Some(*bid) }, "input added");
                            ui.close();
                        }
                    }
                    for (cid, c) in project.busses.iter()
                        .filter(|(cid, c)| *cid != bid && c.parent != Some(*bid)) {
                        if ui.button(format!("bus {}", c.name)).clicked() {
                            // A cycle is the op's refusal, not this menu's
                            // guess — the status line explains it.
                            s.edit(studio_core::Op::SetBusParent {
                                id: *cid, parent: Some(*bid) }, "input added");
                            ui.close();
                        }
                    }
                });
            });
        }
        if project.busses.is_empty() {
            ui.colored_label(c.text_weak, "no busses yet — add one below");
        }
    });
    ui.separator();
    // The report comes from the FADERS area (the body you actually scroll);
    // the routing map above keeps its own horizontal area out of nav.
    let out = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| show_inner(ui, s, c));
    crate::scroll_edge::report(ui, &out)
}

fn show_inner(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors) {
    let tracks: Vec<studio_core::Track> = s.session.doc.project.tracks.clone();
    if tracks.is_empty() {
        ui.colored_label(c.text_weak, "No tracks.");
        return;
    }
    // FOLDED into families (studio_egui::routing), the same toggles as the
    // routing map above: A Top A Tower's 159 lane rows become ~26 headers,
    // each opening onto its lanes; an all-muted family hides unless "show
    // muted". Single-lane families and video tracks stay plain rows.
    let (fold, show_muted) = ui.data(|d| (d.get_temp::<bool>(egui::Id::new("routing-fold")).unwrap_or(true),
                                          d.get_temp::<bool>(egui::Id::new("routing-show-muted")).unwrap_or(false)));
    if !fold {
        for t in &tracks { track_row(ui, s, c, t); }
    } else {
        for t in tracks.iter().filter(|t| t.kind != studio_core::TrackKind::Audio) { track_row(ui, s, c, t); }
        for f in studio_egui::routing::families(&s.session.doc.project) {
            if f.all_muted() && !show_muted { continue; }
            let lanes: Vec<studio_core::Track> = f.tracks.iter().filter_map(|id| tracks.iter().find(|t| t.id == *id).cloned()).collect();
            if lanes.len() == 1 { track_row(ui, s, c, &lanes[0]); continue; }
            ui.horizontal(|ui| {
                // One mute for the family: ONE grouped edit, one undo. Unmuting
                // brings back only the lanes that were not muted IN FL -- the
                // ideas the artist switched off stay a mute away, each on its
                // own row inside, instead of all coming back at once (the first
                // version unmuted all 19 of Triplit's lanes, 17 of them FL-muted).
                let mut audible = !f.all_muted();
                if ui.checkbox(&mut audible, "").on_hover_text("mute / unmute this family (one edit); unmuting leaves the lanes muted in FL muted").changed() {
                    let ops: Vec<studio_core::Op> = lanes.iter()
                        .filter(|t| !audible || !t.name.contains("(muted in FL)"))
                        .map(|t| studio_core::Op::SetTrackFlags { id: t.id, muted: !audible, locked: t.locked }).collect();
                    s.edit_group(ops, if audible { "family unmuted" } else { "family muted" });
                }
                egui::CollapsingHeader::new(f.label()).id_salt(("mixer-family", f.name.clone(), f.route.map(|b| b.0)))
                    .default_open(false).show(ui, |ui| {
                        ui.vertical(|ui| { for t in &lanes { track_row(ui, s, c, t); } });
                    });
            });
        }
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
                gain: 1.0, chain: Vec::new(), sends: Vec::new(), muted: false, silent: false,
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
            // A silent bus still sums, processes and fades — it just feeds
            // nothing. That is a different control from mute (which drops
            // the bus before its chain), and it is the one that makes a
            // control/dummy channel: send into it, read it, hear nothing.
            let mut silent = b.silent;
            if ui.checkbox(&mut silent, "ctrl")
                .on_hover_text("control channel: renders, feeds nothing. Send into it to drive something.")
                .changed() {
                s.edit(studio_core::Op::SetBusSilent { id: b.id, silent }, "bus control tap");
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
            automate_menu(&resp, s, studio_core::AutoTarget::BusGain(b.id));
            // PAN: a balance (see Project::bus_pans). One edit on release,
            // like the faders; a double-click centres it.
            let cur = s.session.doc.project.bus_pans.get(&b.id).copied().unwrap_or(0.0);
            let key = egui::Id::new(("bus-pan", b.id.0));
            let mut pan = ui.data(|d| d.get_temp::<f32>(key)).unwrap_or(cur);
            let presp = ui.add(egui::Slider::new(&mut pan, -1.0..=1.0).text("pan")
                .custom_formatter(|v, _| if v.abs() < 0.005 { "C".into() }
                                  else if v < 0.0 { format!("L{:.0}", -v * 100.0) } else { format!("R{:.0}", v * 100.0) }));
            if presp.double_clicked() {
                ui.data_mut(|d| d.remove::<f32>(key));
                s.edit(studio_core::Op::SetBusPan { id: b.id, pan: 0.0 }, "bus pan centred");
            } else if presp.dragged() {
                ui.data_mut(|d| d.insert_temp(key, pan));
            } else if presp.drag_stopped() || presp.changed() {
                ui.data_mut(|d| d.remove::<f32>(key));
                s.edit(studio_core::Op::SetBusPan { id: b.id, pan }, "bus pan");
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
            // Chips open the effect editor (parameters, bypass, remove).
            crate::fx_editor::chain_chips(ui, s, crate::fx_editor::Target::Bus(b.id), &chain, c);
            ui.menu_button("+ fx", |ui| {
                for (kind, params) in ADDABLE_FX {
                    if ui.button(*kind).clicked() {
                        chain.push(effect_for_bus(kind, params, b.id, &busses));
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

#[cfg(test)]
mod send_defaults_tests {
    use super::*;
    use studio_core::{Bus, BusId, Send};

    fn bus(id: u64, sends_to: &[u64]) -> Bus {
        Bus {
            id: BusId(id), name: format!("bus {id}"), parent: None, gain: 1.0, chain: vec![],
            sends: sends_to.iter().map(|&t| Send { to: BusId(t), level: 0.5, pre_fader: false }).collect(),
            muted: false, silent: false,
        }
    }

    #[test]
    fn a_reverb_on_a_return_bus_is_all_wet_and_on_an_insert_bus_is_not() {
        let busses = vec![bus(1, &[2]), bus(2, &[]), bus(3, &[])];
        let params = ADDABLE_FX.iter().find(|(k, _)| *k == "reverb").unwrap().1;
        let on_return = effect_for_bus("reverb", params, BusId(2), &busses);
        assert_eq!(on_return.params["mix"], 1.0, "bus 1 sends into bus 2: a return, all wet");
        let on_insert = effect_for_bus("reverb", params, BusId(3), &busses);
        assert_eq!(on_insert.params["mix"], 0.3, "nobody sends to bus 3: the insert default");
        // A non-send kind is never touched, return or not.
        let sat = ADDABLE_FX.iter().find(|(k, _)| *k == "saturation").unwrap().1;
        let e = effect_for_bus("saturation", sat, BusId(2), &busses);
        assert_eq!(e.params.get("mix"), None, "saturation's mix is instantiate's default, untouched");
        // And every send kind is in the catalog, or the rule guards nothing.
        for k in SEND_KINDS {
            assert!(ADDABLE_FX.iter().any(|(kind, _)| kind == k), "{k} is addable");
        }
    }
}

/// One track's row: name, mute, solo, fader -- the unit a family folds.
fn track_row(ui: &mut egui::Ui, s: &mut Studio, c: &PaletteColors, t: &studio_core::Track) {
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
        // Right-click the parameter to automate it — the fader is the
        // ingress point, not a separate routing matrix. The strip lives
        // under the timeline; the status line says so.
        automate_menu(&resp, s, studio_core::AutoTarget::TrackGain(t.id));
    });
}
