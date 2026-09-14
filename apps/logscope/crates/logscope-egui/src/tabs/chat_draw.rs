//! chat_draw — `Chat::ui`, the tab's top-level draw.
//!
//! Eighth cut, same treatment as `chat_spatial`: 2,236 lines moved whole rather than broken
//! up. `ui` is the entry point the tab host calls each frame — the chrome, the transcript, the
//! composer and the right page — and having it in the same file as the state it draws meant
//! any change to either began with a scroll past the other.
//!
//! Whole, not decomposed, for the reason the previous cut gives: a move is something the
//! compiler can prove, and lifting a block out of the middle of a 2,000-line function is not.
//!
//! A CHILD module with `impl Chat`, because it reads `Chat`'s private fields.

use super::*;

impl Chat {

    pub fn ui(&mut self, ui: &mut egui::Ui, model: &mut ModelState, doc: &crate::analysis::Doc, infer: &Inference, model_loaded: bool) {
        // Refresh the stamp from the picker each frame: cheap, and it means a model switched
        // between two turns is recorded on the turn that follows it rather than on the whole
        // conversation. Never applied retroactively — history keeps the model that produced it.
        // Only on CHANGE. model.current() allocates, and a panel body runs every frame — writing an
        // unchanged String 60 times a second is the same mistake as the banner cloning the feed.
        {
            let c = model.current();
            if !c.is_empty() && self.stamp_model.as_deref() != Some(c.as_str()) {
                self.stamp_model = Some(c);
            }
        }
        self.sync_worker(infer);
        self.local_model_path = model.selected_local_path(); // for :lanes (run_command has no ModelState)
        self.pump_lanes(); // lane-pool events → fan (a second, parallel event source next to the worker)
        self.check_restore(); // first frame: offer to recover a crashed/closed conversation
        if self.pending_cancel { infer.cancel(); self.pending_cancel = false; }
        let streaming = self.stream.is_some();
        if streaming || self.agent_run.is_some() || self.agent_fan.is_some() || !self.lane_jobs.is_empty() { ui.ctx().request_repaint(); }
        self.pump_voice(infer, doc, ui, streaming); // mic → whisper → route utterances / barge-in (no-op when mic off)
        self.handle_modal(ui, infer, doc); // modal (vim) keys — both views; default Insert keeps typing free

        // ── top bar: status · threading toggle · context toggle · clear ──
        ui.horizontal(|ui| {
            let (badge, bc) = if model_loaded { ("model loaded", theme::GREEN()) } else { ("no model — Load a GGUF up top", theme::DIM()) };
            theme::chip(ui, badge, bc);
            ui.label(RichText::new(self.backend.label()).color(theme::FAINT()).small());
            if let Some(s) = &self.stream {
                let secs = s.started.elapsed().as_secs_f32().max(0.001);
                let toks = s.tokens;
                theme::pulse_dot(ui, if toks == 0 { theme::CYAN() } else { theme::AMBER() });
                let debug = std::env::var("LOGSCOPE_DEBUG").is_ok();
                if toks == 0 {
                    // prefill: the model is reading the prompt, no tokens yet (this is the slow part on big prompts)
                    ui.label(RichText::new(format!("thinking… ({:.0}s)", secs)).color(theme::CYAN()).small())
                        .on_hover_text("prefilling the prompt — the reply starts once this finishes");
                } else if debug {
                    // FEED REVEALPACE. Same numbers the debug readout uses, so the pace tracks the rate the
                    // operator can see rather than a separate estimate that could disagree with it.
                    if secs > 0.5 { self.last_tok_per_s = Some(toks as f32 / secs); }
                    ui.label(RichText::new(format!("{toks} tok · {:.1} tok/s", toks as f32 / secs)).color(theme::AMBER()).small());
                } else if let Some(pred) = crate::estimate::predict(&self.gen_samples, self.last_prompt_tokens, 0.35) {
                    let pct = (crate::estimate::progress(toks, &pred) * 100.0).round() as u32;
                    ui.label(RichText::new(format!("decoding… ~{pct}%")).color(theme::AMBER()).small())
                        .on_hover_text(format!("estimated from {} prior runs{}", self.gen_samples.len(), if pred.from_similar { " with similar prompts" } else { "" }));
                } else {
                    ui.label(RichText::new(format!("decoding… {toks} tokens")).color(theme::AMBER()).small());
                }
                // Display chunk boundary: with auto-continue the reply decodes continuously past the
                // per-chunk size, so mark which chunk it's flowing through (no decode stop).
                let chunk = self.ctx.max_tokens.max(1) as usize;
                if self.auto_continue && toks >= chunk {
                    ui.label(RichText::new(format!("· chunk {}/{}", toks / chunk + 1, 1 + MAX_AUTO_CONTINUE)).color(theme::FAINT()).small());
                }
                // LIVE PROGRESS NOTIFIER — the one-line "what it's doing right now" summary, refreshed
                // ~5s from the streaming tail, shown right next to the decode status so it's actually seen
                // (it was previously tucked in the fan HUD). Amber arrow + text so it reads as live intent.
                if let Some((sum, _)) = &s.progress {
                    if !sum.is_empty() {
                        ui.label(RichText::new(format!("→ {sum}")).color(theme::CYAN()).small().italics())
                            .on_hover_text("live summary of the latest activity in this reply (refreshes every ~5s)");
                    }
                }
            } else if let Some((t, secs)) = self.last {
                if std::env::var("LOGSCOPE_DEBUG").is_ok() {
                    ui.label(RichText::new(format!("· {t} tok · {:.1} tok/s", t as f32 / secs.max(0.001))).color(theme::FAINT()).small());
                } else {
                    ui.label(RichText::new(format!("· {t} tokens")).color(theme::FAINT()).small());
                    // THE ENGINE'S OWN VERDICT, beside the token count. Every number behind this has been
                    // going to stderr, which is why it has been read out of logs by hand rather than seen.
                    //
                    // The worst concern only, and only when there IS one: a row that always carries a
                    // diagnostic is a row nobody reads, and a healthy engine has nothing to say here that
                    // the token count does not already say.
                    // Gated: gpu_kernels is only linked with the engine, and the gui-only build has no
                    // counters to report — a status line that cannot be computed must not be compiled in.
                    #[cfg(feature = "wgpu-engine")]
                    {
                        let (h, f, _b, _r, _) = gpu_kernels::moe_stream_stats();
                        let (pin, tot) = gpu_kernels::moe_pin_counts();
                        let rate = t as f32 / secs.max(0.001);
                        // The ratio the phase-balance line has been spending on stderr since
                        // c4-750. 0 = healthy or unmeasured, which concerns() reads as "say
                        // nothing" rather than as a small ratio.
                        let dr = f32::from_bits(crate::backend::LAST_DECODE_RATIO
                            .load(std::sync::atomic::Ordering::Relaxed));
                        let cs = crate::enginestrip::concerns(0, rate, h, f, pin, tot, None,
                                                              (dr > 0.0).then_some(dr));
                        if let Some(c) = cs.first() {
                            ui.label(RichText::new(c.line()).color(theme::FAINT()).small());
                        }
                    }
                }
                // auto-verify verdict of the just-generated Rust, surfaced HERE (not just the buried Files
                // panel) so it's obvious the model's code was actually build-checked.
                if let Some(job) = &self.verify_job {
                    match &job.result {
                        None => { theme::chip(ui, "⌗ checking build…", theme::AMBER()); }
                        Some((passed, output, _)) => match crate::sandbox::classify(*passed, output) {
                            crate::sandbox::Verdict::Pass => theme::chip(ui, "✓ build ok", theme::GREEN()),
                            crate::sandbox::Verdict::CompileError => theme::chip(ui, "✗ build failed", theme::RED()),
                            crate::sandbox::Verdict::TestFailure => theme::chip(ui, "✗ tests failed", theme::RED()),
                            crate::sandbox::Verdict::DepsUnavailable => theme::chip(ui, "⚠ deps not cached", theme::AMBER()),
                            crate::sandbox::Verdict::ToolError => theme::chip(ui, "⚠ verify error", theme::AMBER()),
                            // Distinct from a failure ON PURPOSE: red would read as "your code is
                            // broken" when the truth is that nothing was checked at all.
                            crate::sandbox::Verdict::Refused => theme::chip(ui, "⊘ not verified — sandbox refused", theme::AMBER()),
                        },
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear").clicked() { self.clear_conversation(); }
                ui.add_space(6.0);
                // PEARL: toggle the in-scene refractive composer pearl + pick its optical variant
                if self.pearl_on {
                    let names = ["balanced", "fine-crackle", "milky", "clear-glass"];
                    let cur = names[(self.pearl_variant as usize).min(3)];
                    crate::ui::combo(ui, "pearl_variant", cur, 116.0, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            if ui.selectable_label(self.pearl_variant as usize == i, *n).clicked() { self.pearl_variant = i as u8; }
                        }
                    });
                }
                theme::switch(ui, &mut self.pearl_on, theme::CYAN());
                ui.label(RichText::new("Pearl").size(10.5).color(if self.pearl_on { theme::CYAN() } else { theme::DIM() }))
                    .on_hover_text("Cast the refractive composer pearl into the scene (physically-shaded: refraction · fresnel · thin-film).");
                ui.add_space(6.0);
                theme::switch(ui, &mut self.threading, theme::CYAN());
                ui.label(RichText::new("Thread").size(10.5).color(if self.threading { theme::CYAN() } else { theme::DIM() }))
                    .on_hover_text("Thread prior turns into the prompt (multi-turn). Off: each message is sent alone with the context.");
                ui.add_space(6.0);
                theme::switch(ui, &mut self.attach_log, theme::GREEN());
                ui.label(RichText::new("Log").size(10.5).color(if self.attach_log { theme::GREEN() } else { theme::DIM() }))
                    .on_hover_text("Include a bounded summary of the current log (line count, top repeated patterns, last lines) in the system prompt.");
                ui.add_space(6.0);
                theme::switch(ui, &mut self.fix_mode, theme::AMBER());
                ui.label(RichText::new("Fix").size(10.5).color(if self.fix_mode { theme::AMBER() } else { theme::DIM() }))
                    .on_hover_text("Step-up fix submode: the assistant names the fault and emits a concrete fix (file or diff) you can save, instead of just discussing it. Pairs with Log.");
                ui.add_space(6.0);
                if ui.selectable_label(self.show_context, RichText::new("☰ Panels").color(theme::DIM())).on_hover_text("Files · Graph · Context · Prompting").clicked() {
                    self.show_context = !self.show_context;
                }
                ui.add_space(6.0);
                let vlabel = if self.spatial_view { "◫ Spatial" } else { "▤ Linear" };
                if ui.selectable_label(self.spatial_view, RichText::new(vlabel).color(if self.spatial_view { theme::CYAN() } else { theme::DIM() }))
                    .on_hover_text("Switch the main area between the linear chat and the 3D message graph").clicked() {
                    let next = !self.spatial_view;
                    self.set_view(next, ui);
                }
                // Only meaningful in Spatial mode: the composer/files/suggestions strip otherwise
                // cramps or cuts off the physical UI's scene against the bottom of the window, with
                // no way to reclaim that space — this hides the WHOLE bottom panel, not just parts
                // of it, and moves nothing (no repositioning to "fit better") since a toggle is a
                // much more predictable fix than trying to auto-shrink composer sub-elements.
                if self.spatial_view {
                    ui.add_space(6.0);
                    let label = if self.hide_bottom_bar { "▲ Show bar" } else { "▼ Hide bar" };
                    if ui.selectable_label(self.hide_bottom_bar, RichText::new(label).color(theme::DIM()))
                        .on_hover_text("Hide the composer/files/suggestions strip entirely, so the physical UI gets the full window height instead of being cramped behind it.")
                        .clicked() {
                        self.hide_bottom_bar = !self.hide_bottom_bar;
                    }
                }
            });
        });

        // prominent, always-visible modal indicator (mode pill + live :/​/ input or key hint)
        self.modal_banner(ui);
        if self.request_smaller_model {
            self.request_smaller_model = false;
            if let Some(i) = crate::model::smallest_local(&model.items) {
                model.selected = i;
                let name = model.current();
                self.note = format!("🚩 diagnosing the bad jump with {name} (smallest local model) — send to run.");
            } else {
                self.note = "🚩 bad jump staged, but no local model is available for the diagnosis — load a small GGUF.".into();
            }
        }
        self.help_overlay(ui);

        // artifacts from the latest reply (consumed by the bottom panel AND the Files sidebar page)
        let last_text = self.msgs.iter().rev().find(|m| m.role == Role::Assistant).map(|m| m.text().to_string());
        let arts = last_text.as_deref().map(crate::workspace::parse_artifacts).unwrap_or_default();

        // poll a running sandbox verify (cargo check) job
        if let Some(job) = &mut self.verify_job {
            if job.result.is_none() {
                match job.rx.try_recv() {
                    Ok(r) => job.result = Some(r),
                    Err(std::sync::mpsc::TryRecvError::Empty) => { ui.ctx().request_repaint(); }
                    Err(_) => job.result = Some((false, "verify worker disconnected".into(), false)),
                }
            }
        }

        // ── right sidebar: per-conversation subpages behind a button rail (Files · Context · Prompting) ──
        // ONE LINE, ABOVE EVERYTHING, when the engine had to do less than asked. The Engine page is
        // somewhere you go; this is something you see. The case it exists for is silent data loss: a
        // 13412-token paste truncated to 3840 produces a confident answer about code the model never
        // received, and until now the only place that was said was stderr.
        //
        // Severity-then-recency, matching the Engine page's own ranking so the two never disagree,
        // and absent entirely when nothing interrupts — a banner that is usually there is furniture.
        if let Some(msg) = crate::diagfeed::banner() {
            egui::Panel::top("chat_diag_banner").show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("⚠").color(egui::Color32::from_rgb(230, 170, 80)));
                    ui.label(RichText::new(msg).small().color(egui::Color32::from_rgb(230, 170, 80)));
                    if ui.small_button("details").clicked() {
                        self.show_context = true;
                        self.right_page = RightPage::Engine;
                    }
                    // Hide, do not delete: the button beside this one opens the page that shows
                    // these lines, and wiping them on the way there is the worst possible pairing.
                    if ui.small_button("✕").on_hover_text("dismiss until the next engine notice").clicked() {
                        crate::diagfeed::dismiss_banner();
                    }
                });
            });
        }

        // ADAPTIVE, not a fixed 330 px. `chat_layout` has existed and been
        // tested since the port and nothing called it — grep src/tabs for
        // DeviceClass and there were no hits at all, so every tab
        // hard-coded a desktop shape. On a phone a 330 px side panel plus
        // a transcript leaves neither usable.
        //
        // `chat_layout_for_width` rather than the DeviceClass form: the
        // width egui reports here is what actually constrains this panel,
        // and it already accounts for a narrow window on a desktop —
        // which a device class does not.
        //
        // CAUTION for anyone reading more fields off `cl`: `any_touches()`
        // is true only WHILE a finger is down, so it flickers between
        // frames on a touch device. `split` does not depend on it (see
        // `chat_layout`), which is why it is safe here — but
        // `input_docked_bottom` and `touch_target` do, and reading those
        // from this call would make the input bar jump as you tap. Those
        // want a sticky "this is a touch device" flag, which the app does
        // not have yet.
        let cl = crate::chat_layout::chat_layout_for_width(
            ui.available_width(),
            ui.ctx().input(|i| i.any_touches()),
        );
        // `split` false means the pane does not fit beside the transcript.
        // The context is then reachable through the existing toggle
        // rather than shown, which is the same information one tap away
        // instead of two panes fighting for the same 400 px.
        if self.show_context && cl.split {
            let w = (ui.available_width() * 0.38).clamp(240.0, 330.0);
            egui::Panel::right("chat_right").resizable(true).default_size(w).show_inside(ui, |ui| {
                ui.add_space(theme::PAD_XS as f32);
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(self.right_page == RightPage::Files, RichText::new("📄 Files").small()).clicked() { self.right_page = RightPage::Files; }
                    if ui.selectable_label(self.right_page == RightPage::Graph, RichText::new("⌗ Graph").small()).clicked() { self.right_page = RightPage::Graph; }
                    if ui.selectable_label(self.right_page == RightPage::Context, RichText::new("⚙ Context").small()).clicked() { self.right_page = RightPage::Context; }
                    if ui.selectable_label(self.right_page == RightPage::Prompting, RichText::new("✦ Prompting").small()).clicked() { self.right_page = RightPage::Prompting; }
                    // ENGINE. The count comes from diagfeed::attention(), which counts only
                    // warnings and failures — the engine emits dozens of provenance and measurement
                    // lines a load, and a badge that always shows a number is a badge people learn
                    // to ignore.
                    {
                        let n = crate::diagfeed::attention();
                        let lbl = crate::diagfeed::tab_label(n);
                        // A NEW WARNING FLASHES. The count changing from 2 to 3 is silent
                        // otherwise, and this badge exists precisely to be noticed — while a
                        // badge that is permanently loud is one people learn to ignore, which
                        // is why the CHANGE is highlighted rather than the badge. Keyed on the
                        // count, so first sight does not flash: lighting up because the panel
                        // was opened would be crying wolf on frame one.
                        let f = uikit_egui::motion::flash_on_change(
                            ui.ctx(), egui::Id::new("logscope-engine-badge"), &n, 0.6);
                        let warn = egui::Color32::from_rgb(230, 170, 90);
                        let base = if n > 0 { warn } else { theme::DIM() };
                        // Toward white at the peak, back to the resting colour as it decays.
                        let col = egui::Color32::from_rgb(
                            base.r().max((255.0 * f) as u8),
                            base.g().max((255.0 * f) as u8),
                            base.b().max((255.0 * f) as u8),
                        );
                        let txt = RichText::new(lbl).small().color(col);
                        if ui.selectable_label(self.right_page == RightPage::Engine, txt).clicked() {
                            self.right_page = RightPage::Engine;
                        }
                    }
                });
                ui.separator();
                // floating context cards — the freshest code block, the loaded file, and salient
                // keywords, kept next to the chat regardless of which subpage is open.
                {
                    // CACHED. This block ran EVERY FRAME: scanning back
                    // through the transcript calling `extract_code` on
                    // each message until one had a code block, then
                    // joining the last four messages into a String and
                    // running `keywords` over it — which lowercases a
                    // String per word and clones each unique one. On four
                    // turns of chat that is thousands of allocations,
                    // sixty times a second, to produce the same two cards
                    // until the transcript changes.
                    //
                    // Keyed on message COUNT plus the length of the last
                    // message: the count catches a new turn, and the
                    // length catches a streaming reply growing in place,
                    // which is the case a count alone would miss and is
                    // exactly when this panel most needs to update.
                    let ctx_key = (
                        self.msgs.len(),
                        self.msgs.last().map(|m| m.text().len()).unwrap_or(0),
                    );
                    if self.ctx_card_key != Some(ctx_key) {
                        self.ctx_card_code = self.msgs.iter().rev()
                            .map(|m| crate::verdict::extract_code(m.text()))
                            .find(|c| !c.trim().is_empty());
                        let kw_src: String = self.msgs.iter().rev().take(4)
                            .map(|m| m.text()).collect::<Vec<_>>().join(" ");
                        self.ctx_card_kws = crate::convograph::keywords(&kw_src)
                            .into_iter().take(6).collect();
                        self.ctx_card_key = Some(ctx_key);
                    }
                    let code = self.ctx_card_code.clone();
                    let kws: Vec<String> = self.ctx_card_kws.clone();
                    let has_file = !doc.name.is_empty();
                    if code.is_some() || has_file || !kws.is_empty() {
                        // Carved-frame treatment (bevel lip + material grain + breathing corner
                        // runes) instead of the flat hairline box: `theme::panel()` already builds
                        // exactly this — a titled, textured card matching the reference frames —
                        // but had no callers, so every sidebar card was still a plain rectangle.
                        // This is the card the reference images show most prominently, so it goes
                        // first.
                        theme::panel(ui, theme::CYAN(), "Context cards", None, |ui| {
                            if has_file {
                                ui.add_space(3.0);
                                // NAME, SIZE, AND WHY IT IS HERE — the three facts the reference
                                // cards carry and ours did not. The figure on the right is a COUNT
                                // rendered as a ratio, always shown with its denominator: a bare
                                // 0.67 in the panel whose job is explaining the context would read
                                // as a model's confidence, and this is something a reader can check
                                // by opening the file.
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(format!("📄 {}", doc.name)).color(theme::CYAN()).small());
                                    if let Some((_, hit, total)) = crate::ctxcard::topic_match(&doc.lines, &kws) {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(RichText::new(crate::ctxcard::match_label(hit, total))
                                                .color(if hit * 2 >= total { theme::AMBER() } else { theme::DIM() })
                                                .small().monospace());
                                        });
                                    }
                                });
                                ui.label(RichText::new(crate::ctxcard::size_line(&doc.lines, doc.placeholder))
                                    .color(theme::FAINT()).small());
                            }
                            if let Some(code) = &code {
                                ui.add_space(4.0);
                                let shown: String = code.lines().take(6).collect::<Vec<_>>().join("\n");
                                let more = code.lines().count() > 6;
                                egui::Frame { fill: theme::TRACK_BG(), corner_radius: theme::r(6), inner_margin: egui::Margin::same(6), ..Default::default() }
                                    .show(ui, |ui| {
                                        ui.label(RichText::new(shown).monospace().size(10.5).color(theme::INK()));
                                        if more { ui.label(RichText::new("…").color(theme::FAINT()).small()); }
                                    });
                                if ui.small_button(format!("{} copy code", uikit_egui::icons::copy_label(ui.ctx()))).clicked() { ui.ctx().copy_text(code.clone()); }
                            }
                            if !kws.is_empty() {
                                ui.add_space(5.0);
                                ui.horizontal_wrapped(|ui| { for k in &kws { theme::chip(ui, k, theme::AMBER()); } });
                            }
                        });
                    }
                }
                // PAGE TRANSITION. Switching the right page snapped — the old content gone and
                // the new one present in the same frame, which reads as a redraw glitch rather
                // than as navigation: there is no moment that says "this replaced that".
                //
                // A cross-fade over OPACITY, not a slide: these pages are dense text, a sliding
                // wall of it is harder to read mid-move, and an opacity ramp leaves everything
                // where the eye expects it. `motion::cross_fade` also survives a mid-flight
                // switch, so Files→Graph→Context reads as one movement instead of snapping
                // back through Graph, and it is zero-duration under `UIKIT_REDUCE_MOTION`.
                //
                // The INCOMING page only. Drawing both would need each to lay out in the same
                // rect, and two dense text pages overlapping at half opacity is less readable
                // than either — fading the arrival is the honest half.
                let (_leaving, page_t) = uikit_egui::motion::cross_fade(
                    ui.ctx(),
                    egui::Id::new("logscope-right-page"),
                    &self.right_page,
                    0.16,
                );
                ui.set_opacity(page_t.max(0.05));
                egui::ScrollArea::vertical().id_salt("right_page").auto_shrink([false, false]).show(ui, |ui| {
                    match self.right_page {
                        RightPage::Engine => {
                            // THE TUNABLES, above the log they explain. `engine_knobs` has
                            // described these since it was written and nothing drew them, so
                            // the only way to try `LOGSCOPE_MATVEC_SG=1` was to know the
                            // variable existed, know its spelling, and restart from a
                            // terminal.
                            //
                            // Folded by default: this is a panel someone opens to read a
                            // failure, and a wall of toggles above the failure is in the way
                            // of the thing they came for.
                            let knobs = crate::engine_knobs::all();
                            let resolved: Vec<(&crate::engine_knobs::Knob, crate::engine_knobs::Resolved)> =
                                knobs
                                    .iter()
                                    .map(|k| {
                                        let ui_val = self.engine_knob_values.get(k.env);
                                        (k, crate::engine_knobs::resolve(k, ui_val, |n| std::env::var(n).ok()))
                                    })
                                    .collect();
                            let header = format!("tuning — {}", crate::engine_knobs::summary(&resolved));
                            egui::CollapsingHeader::new(header)
                                .id_salt("engine-knobs")
                                .default_open(false)
                                .show(ui, |ui| {
                                    for (k, r) in &resolved {
                                        let editable = crate::engine_knobs::editable(r);
                                        ui.horizontal(|ui| {
                                            match &r.value {
                                                crate::engine_knobs::Value::Flag(on) => {
                                                    let mut v = *on;
                                                    // DISABLED, not hidden, when the
                                                    // environment owns it: a control that
                                                    // vanished would look like the setting
                                                    // did, and the operator needs to see
                                                    // that their shell is winning.
                                                    if ui
                                                        .add_enabled(
                                                            editable,
                                                            egui::Checkbox::new(&mut v, k.label),
                                                        )
                                                        .changed()
                                                    {
                                                        self.engine_knob_values.insert(
                                                            k.env.to_string(),
                                                            crate::engine_knobs::Value::Flag(v),
                                                        );
                                                    }
                                                }
                                                crate::engine_knobs::Value::Count(n) => {
                                                    let mut v = *n;
                                                    ui.label(k.label);
                                                    if ui
                                                        .add_enabled(
                                                            editable,
                                                            egui::DragValue::new(&mut v)
                                                                .range(k.range.0..=k.range.1),
                                                        )
                                                        .changed()
                                                    {
                                                        // Clamped by the KNOB, not just by
                                                        // the widget: a value can also arrive
                                                        // from a hand-edited settings file.
                                                        self.engine_knob_values.insert(
                                                            k.env.to_string(),
                                                            k.clamp(crate::engine_knobs::Value::Count(v)),
                                                        );
                                                    }
                                                }
                                            }
                                            match r.source {
                                                crate::engine_knobs::Source::Env => {
                                                    ui.colored_label(
                                                        theme::active().amber,
                                                        format!("{} set in the environment", k.env),
                                                    );
                                                }
                                                crate::engine_knobs::Source::EnvUnparsed => {
                                                    // A typo is otherwise indistinguishable
                                                    // from not setting it at all.
                                                    ui.colored_label(
                                                        theme::active().amber,
                                                        format!(
                                                            "{}={:?} unreadable, ignored",
                                                            k.env,
                                                            r.raw.clone().unwrap_or_default()
                                                        ),
                                                    );
                                                }
                                                _ => {}
                                            }
                                            if k.risk == crate::engine_knobs::Risk::Fragile {
                                                ui.colored_label(theme::active().amber, "fragile");
                                            }
                                        });
                                        ui.label(egui::RichText::new(k.help).small().weak());
                                    }
                                    // AND THE VALUES REACH THE ENGINE. It reads these from
                                    // the ENVIRONMENT at load (`std::env::var
                                    // ("LOGSCOPE_PREFILL_CHUNK")` and friends), so the panel
                                    // writes them there — anything else would be a second
                                    // source of truth for the same setting.
                                    //
                                    // Written on every frame the section is open rather than
                                    // on change: it is a handful of setenv calls on a folded
                                    // panel, and a "changed" edge that missed once would
                                    // leave a knob showing one value while the engine used
                                    // another, which is the failure this whole module exists
                                    // to avoid.
                                    //
                                    // `env_to_apply` refuses to write over anything the
                                    // environment already set, so a shell override stays the
                                    // winner.
                                    let ui_vals: Vec<(&'static str, crate::engine_knobs::Value)> =
                                        knobs
                                            .iter()
                                            .filter_map(|k| {
                                                self.engine_knob_values
                                                    .get(k.env)
                                                    .map(|v| (k.env, v.clone()))
                                            })
                                            .collect();
                                    for (name, value) in
                                        crate::engine_knobs::env_to_apply(&ui_vals, |n| std::env::var(n).ok())
                                    {
                                        // The engine reads these only while loading a model,
                                        // and the panel says changes apply at the next load,
                                        // so the window where a worker could observe a
                                        // half-written set is one the operator is told about.
                                        std::env::set_var(name, value);
                                    }
                                    // Applies at the next LOAD, said once rather than per
                                    // knob: a change that appears to do nothing is how
                                    // someone concludes the toggles are broken.
                                    ui.label(
                                        egui::RichText::new(
                                            "changes apply when a model is next loaded",
                                        )
                                        .small()
                                        .weak(),
                                    );
                                });
                            ui.separator();
                            // The engine's own diagnostics, ranked and folded (diagfeed). Until now
                            // these existed only on stderr, which is why diagnosing a refusal meant
                            // running the app from a terminal.
                            let snap = crate::diagfeed::snapshot();
                            if snap.is_empty() {
                                ui.weak("No engine output yet — load a model.");
                            } else {
                                // GROUPED BY SUBJECT, foldable, copyable. A load emits a run
                                // of preflight notes, a run of tile overrides and a run of
                                // dispatch levers — three things said in eighteen lines, with
                                // the one that matters somewhere in the middle. Failures and
                                // warnings are never folded: see `loggroup`.
                                let groups = crate::loggroup::group(&snap, 2);
                                ui.horizontal(|ui| {
                                    ui.weak(format!("{} line(s)", snap.len()));
                                    if ui.small_button("clear").clicked() { crate::diagfeed::clear(); }
                                    // The reason anyone reads this panel is to paste it
                                    // somewhere; the alternative today is a screenshot.
                                    if ui.small_button("copy all").on_hover_text("every line, with its group headers").clicked() {
                                        ui.ctx().copy_text(crate::loggroup::all_to_clipboard(&groups));
                                    }
                                });
                                ui.separator();
                                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                                  for g in &groups {
                                    // A group of one is drawn loose — a header hiding a
                                    // single line costs a click and saves nothing.
                                    let folded = !g.open && g.entries.len() > 1;
                                    if folded {
                                        let head = format!("{}  ({} lines)", g.label, g.line_count());
                                        let id = ui.make_persistent_id(("engine-group", &g.label));
                                        egui::CollapsingHeader::new(head).id_salt(id).default_open(false).show(ui, |ui| {
                                            if ui.small_button("copy").clicked() {
                                                ui.ctx().copy_text(g.to_clipboard());
                                            }
                                            draw_engine_lines(ui, &g.entries);
                                        });
                                    } else {
                                        draw_engine_lines(ui, &g.entries);
                                    }
                                  }
                                });
                            }
                        }
                        RightPage::Context => {
                            // SYSTEM STATUS, from real signals. The reference corner carries a dot
                            // and "All systems operational"; hardcoding that would be a lie on
                            // every fresh start — no model, the bundled sample standing in for the
                            // operator's data, no project mounted — and a permanently-green light
                            // asserts the things beside it are fine until the reader stops
                            // checking them. See `sysstatus`; every signal there can veto.
                            {
                                let sig = crate::sysstatus::Signals {
                                    model_loaded,
                                    sample_doc: doc.placeholder,
                                    project_mounted: self.gitmind_session.is_some(),
                                    run_blocked: self.agent_run.as_ref()
                                        .map(|r| r.phase() == Some("blocked")).unwrap_or(false),
                                    // The backend is compiled in or it is not; `preferred()` already
                                    // falls back, so reaching here means something is usable.
                                    backend_ready: true,
                                };
                                let (level, msg) = crate::sysstatus::assess(&sig);
                                let col = match level {
                                    crate::sysstatus::Readiness::Ready => theme::GREEN(),
                                    crate::sysstatus::Readiness::Degraded => theme::AMBER(),
                                    crate::sysstatus::Readiness::NotReady => theme::RED(),
                                };
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("●").color(col).small());
                                    ui.label(RichText::new(level.label()).color(col).small().strong());
                                    ui.label(RichText::new(msg).color(theme::DIM()).small());
                                });
                                ui.add_space(theme::PAD_XS as f32);
                            }
                            // ACTIVE THREAD, pinned above everything else on this page. The status
                            // line already says what the last turn did, and is overwritten by the
                            // next one — for an unattended run left going for an hour, "what is it
                            // doing and how long has it been" needs a fixed place to be answered.
                            // Rows rather than a sentence: see `runcard`.
                            if let Some(card) = self.agent_run.as_ref().map(|r| r.run_state()) {
                                theme::panel(ui, theme::CYAN(), "Active thread", None, |ui| {
                                    for (label, value) in crate::runcard::rows(&card) {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(label).color(theme::FAINT()).small());
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.label(RichText::new(value).color(theme::INK()).small());
                                            });
                                        });
                                    }
                                });
                                // The elapsed row ticks; without this it only advances when
                                // something else asks for a frame.
                                ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                            }
                            // ACTIVITY — one row per fan branch, WHILE it runs. The fan reported
                            // itself only in `finish_agent_fan`, so a fan mid-flight was a single
                            // status sentence and nothing else; which branch held the one worker,
                            // and which were queued behind it, was not visible anywhere.
                            let fan_rows: Vec<(String, crate::runcard::Doing, usize, bool, Option<bool>)> =
                                self.agent_fan.as_ref().map(|f| f.summaries().into_iter()
                                    .map(|b| (b.label, b.doing, b.files, b.verified, b.approved))
                                    .collect()).unwrap_or_default();
                            if !fan_rows.is_empty() {
                                theme::panel(ui, theme::AMBER(), "Activity", None, |ui| {
                                    for (label, doing, detail) in crate::runcard::branch_rows(&fan_rows) {
                                        ui.horizontal(|ui| {
                                            // The branch holding the single worker is the one thing
                                            // worth picking out of the list.
                                            let hot = doing == "generating" || doing == "reviewing";
                                            ui.label(RichText::new(label)
                                                .color(if hot { theme::CYAN() } else { theme::INK() }).small());
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.label(RichText::new(doing)
                                                    .color(if hot { theme::AMBER() } else { theme::FAINT() }).small());
                                            });
                                        });
                                        ui.label(RichText::new(detail).color(theme::FAINT()).small());
                                        ui.add_space(2.0);
                                    }
                                });
                            }
                            // The physical PARAMS panel (dials + backend/persona/model selectors, in the
                            // spatial scene) is now the primary place for these controls. When it's active
                            // don't duplicate them here — a slider changed in two places at once is a bug
                            // waiting to happen. LOGSCOPE_PHYS_PARAMS=0 (or non-spatial view) falls back to
                            // this page as before.
                            let phys_params_on = self.spatial_view
                                && std::env::var("LOGSCOPE_PHYS_PARAMS").map(|v| v != "0").unwrap_or(true);
                            ui.label(RichText::new("CONTEXT (system prompt)").color(theme::DIM()).small().strong());
                            if phys_params_on {
                                ui.label(RichText::new("Sampling/backend/model moved into the scene — drag the PARAMS dials on the left, click BACKEND/PERSONA/MODEL to cycle them. Persona is still editable here.").color(theme::FAINT()).small());
                                ui.add_space(6.0);
                            }
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("persona").color(theme::DIM()).small());
                                for (key, title, body) in PERSONAS {
                                    let active = self.context.trim() == body.trim();
                                    if ui.selectable_label(active, RichText::new(*title).small()).on_hover_text(*key).clicked() {
                                        self.context = (*body).to_string();
                                    }
                                }
                            });
                            ui.add(egui::TextEdit::multiline(&mut self.context).id_salt("chat-self_context").desired_rows(4).desired_width(f32::INFINITY)
                                .hint_text("System prompt / persistent context for every turn…"));
                            if !phys_params_on {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("backend").color(theme::DIM()).small());
                                egui::ComboBox::from_id_salt("backend").width(180.0).selected_text(self.backend.label())
                                    .show_ui(ui, |ui| { for k in BackendKind::ALL { ui.selectable_value(&mut self.backend, k, k.label()); } });
                            });
                            ui.add(egui::Slider::new(&mut self.ctx.n_ctx, 512..=32768).step_by(256.0).text("ctx window"))
                                .on_hover_text("Total context (prompt + reply). Bigger = more memory; a long paste is trimmed to fit this.");
                            ui.add(egui::Slider::new(&mut self.ctx.max_tokens, 64..=4096).step_by(64.0).text("reply budget"));
                            ui.add(egui::Slider::new(&mut self.ctx.temperature, 0.0..=1.5).text("temperature"));
                            ui.add(egui::Slider::new(&mut self.ctx.repeat_penalty, 1.0..=1.5).text("repeat penalty"));
                            ui.add(egui::Slider::new(&mut self.ctx.min_p, 0.0..=0.3).step_by(0.01).text("min-p"));
                            ui.add_enabled(self.threading, egui::Slider::new(&mut self.ctx.history_turns, 1..=32).text("history turns"));
                            }
                            let used: usize = est(&self.context) + self.msgs.iter().map(|m| est(m.text()) + 4).sum::<usize>();
                            let budget = self.ctx.n_ctx.saturating_sub(self.ctx.max_tokens).max(1) as usize;
                            let frac = (used as f32 / budget as f32).min(1.0);
                            let col = if used > budget { theme::RED() } else if frac > 0.8 { theme::YELLOW() } else { theme::CYAN() };
                            theme::usage_bar_anim(ui, "ctx_usage", 280.0, frac, col);
                            ui.label(RichText::new(format!("~{used} / {budget} tok · model: {}", model.current())).color(theme::DIM()).small());
                        }
                        RightPage::Prompting => {
                            // Its own carved card — FREECAM KEYS / WORKING DISCIPLINE below stay
                            // as plain sections, so only the thing the reference frames box off
                            // (the mode picker) gets boxed off here too.
                            theme::panel(ui, theme::AMBER(), "Prompting mode", None, |ui| {
                            // GROUP BY AXIS. Six entries answering two different questions — three vary how WIDE the
                            // fan-out is, three vary WHO REASONS — and an undifferentiated list makes an operator infer
                            // that from the names before they can choose. A heading per group costs two lines and
                            // removes the inference.
                            let mut last_axis = "";
                            for m in crate::coordinator::Mode::all() {
                                if m.axis() != last_axis {
                                    if !last_axis.is_empty() { ui.add_space(6.0); }
                                    ui.label(RichText::new(m.axis()).color(theme::FAINT()).small());
                                    last_axis = m.axis();
                                }
                                let on = self.coord_mode == m;
                                if ui.selectable_label(on, RichText::new(m.label()).color(if on { theme::CYAN() } else { theme::INK() }).strong()).clicked() {
                                    self.coord_mode = m;
                                    // aggressive pulls in the spatial view; barebones returns to the linear chat
                                    match m { crate::coordinator::Mode::Aggressive => self.set_view(true, ui),
                                              crate::coordinator::Mode::Barebones => { self.replicator = None; self.set_view(false, ui) }
                                              // Selecting Replicator OPENS it. Every other entry
                                              // in this list records an intent the next run reads;
                                              // this one has a live UI, and a menu entry that only
                                              // sets a flag would leave the operator exactly where
                                              // they were — looking for the mode they just chose.
                                              crate::coordinator::Mode::Replicator =>
                                                  self.run_command(crate::vimmode::Command::Replicate(None), ui),
                                              _ => {} }
                                }
                                ui.label(RichText::new(m.blurb()).color(theme::DIM()).small());
                                // WHAT IT ACTUALLY DOES, per mode, under the
                                // description of what it is for.
                                //
                                // The single footnote this replaces said every
                                // mode merely "records the intent", which was
                                // wrong in both directions: Replicator opens a
                                // live UI, Aggressive switches the view,
                                // LongHorizon already suppresses interrupts,
                                // and StatefulGuide's phase machine is real.
                                // Meanwhile the thing genuinely missing —
                                // nothing consumes `coordinator::plan()` — was
                                // not stated at all.
                                //
                                // Dimmer than the blurb and brighter than the
                                // axis heading: it is the line that decides
                                // whether to pick this, so it must be readable
                                // without competing with the name.
                                let live = m.has_effect();
                                ui.label(RichText::new(m.wiring())
                                    .color(if live { theme::DIM() } else { theme::FAINT() })
                                    .small().italics());
                                ui.add_space(6.0);
                            }
                            }); // close "Prompting mode" carved panel
                            ui.separator();
                            // ── MOVEMENT KEYS ────────────────────────────────────────────────
                            ui.label(RichText::new("FREECAM KEYS").color(theme::FAINT()).small());
                            for l in crate::navkeys::NavLayout::all() {
                                let on = self.nav_layout == l;
                                if ui.selectable_label(on, RichText::new(l.label())
                                    .color(if on { theme::CYAN() } else { theme::INK() })).clicked() {
                                    self.nav_layout = l;
                                    self.persist_prefs();
                                }
                                ui.label(RichText::new(l.hint()).color(theme::DIM()).small());
                                ui.add_space(3.0);
                            }
                            ui.add_space(4.0);
                            ui.separator();
                            // ── ENGINEERING PROFILES ─────────────────────────────────────────
                            ui.label(RichText::new("WORKING DISCIPLINE").color(theme::FAINT()).small());
                            let auto = self.profiles_manual.is_empty();
                            if ui.selectable_label(auto, RichText::new("Autopilot")
                                .color(if auto { theme::CYAN() } else { theme::INK() }).strong()).clicked() {
                                self.profiles_manual.clear();
                                self.persist_prefs();
                            }
                            ui.label(RichText::new("Chooses the posture from the task: a shader bug gets the GPU role's rule for verification, a slow path gets the optimizer's. Anything unrecognised gets the native-fullstack default, which is what a third of your turns — \"keep working\" — actually ask for.")
                                .color(theme::DIM()).small());
                            ui.add_space(4.0);
                            for id in crate::engprofile::ProfileId::all() {
                                let p = id.profile();
                                let on = self.profiles_manual.contains(&id);
                                if ui.selectable_label(on, RichText::new(p.name)
                                    .color(if on { theme::CYAN() } else { theme::INK() })).clicked() {
                                    // Order of selection is order of PRECEDENCE — the first picked
                                    // wins a shared rule, which is what `compose` resolves by.
                                    if on { self.profiles_manual.retain(|x| *x != id); }
                                    else { self.profiles_manual.push(id); }
                                    self.persist_prefs();
                                }
                                ui.label(RichText::new(p.when).color(theme::DIM()).small());
                                ui.label(RichText::new(format!("source: {}", p.source))
                                    .color(theme::FAINT()).small().italics());
                                ui.add_space(4.0);
                            }
                            if !self.profiles_manual.is_empty() {
                                // What the merge costs, stated before a run rather than discovered:
                                // two profiles with a rule for the same thing means one is dropped.
                                let b = crate::engprofile::compose(&self.profiles_manual, 6000);
                                let mut note = format!("{} profile(s), {} bytes of prompt",
                                    self.profiles_manual.len(), b.text.len());
                                if !b.dropped.is_empty() {
                                    note.push_str(&format!("; {} overlapping rule(s) resolved in favour of the earlier pick", b.dropped.len()));
                                }
                                if b.trimmed > 0 { note.push_str(&format!("; {} trimmed to fit", b.trimmed)); }
                                ui.label(RichText::new(note).color(theme::CYAN()).small());
                            }
                            ui.separator();
                            ui.label(RichText::new("Coordinator modes fan one task out to parallel workers over the same loaded model — no extra models needed. The per-mode line above says what each one changes today. `coordinator::plan()` now drives the fan: the steps of the plan your model wrote become the units it splits the work into, so a fan is only as good as the plan above it — and the same steps decide afterwards which ones were actually done.").color(theme::FAINT()).small());
                            // Replicator is the exception to that sentence and says so where the
                            // sentence is, rather than leaving the operator to discover that one
                            // entry behaves differently from the other seven.
                            if self.coord_mode == crate::coordinator::Mode::Replicator {
                                ui.label(RichText::new(match self.replicator.as_ref() {
                                    Some(m) => format!("Replicator is OPEN with {} column(s). \
Type into the composer to message all of them, click a column header to aim at one, and 'links' \
sets how often each worker sees another's latest. Run :lanes {} so they generate in parallel.",
                                        m.workers.len(), m.workers.len().max(2)),
                                    None => "Replicator did not open — no host reported room for a worker.".into(),
                                }).color(theme::CYAN()).small());
                            }
                        }
                        RightPage::Graph => {
                            theme::panel(ui, theme::CYAN(), "Conversation graph", None, |ui| {
                            ui.label(RichText::new("Click a node to jump the chat there; ◀ ▶ dolly the camera through it.").color(theme::FAINT()).small());
                            let mut g = self.convo_graph(self.msgs.len());
                            let budget = self.ctx.n_ctx.saturating_sub(self.ctx.max_tokens).max(1) as usize;
                            let max_turns = if self.threading { self.ctx.history_turns * 2 + 1 } else { 1 };
                            let (in_ctx, used) = g.mark_window_ex(budget, est(self.sys()) + 8, 8, max_turns);
                            let n = g.nodes.len();
                            if n == 0 {
                                ui.label(RichText::new("No turns yet — start a conversation to see the spine form.").color(theme::DIM()).small());
                            } else {
                                ui.horizontal(|ui| {
                                    let cur = self.walk_focus.unwrap_or(0);
                                    if ui.add_enabled(cur > 0, egui::Button::new(RichText::new("◀").small())).on_hover_text("Previous turn").clicked() {
                                        let t = cur.saturating_sub(1); self.walk_focus = Some(t); self.scroll_chat_to = Some(t);
                                    }
                                    ui.label(RichText::new(self.walk_focus.map(|i| format!("{}/{n}", i + 1)).unwrap_or_else(|| format!("–/{n}"))).color(theme::DIM()).small());
                                    if ui.add_enabled(cur + 1 < n, egui::Button::new(RichText::new("▶").small())).on_hover_text("Next turn").clicked() {
                                        let t = (cur + 1).min(n - 1); self.walk_focus = Some(t); self.scroll_chat_to = Some(t);
                                    }
                                    if ui.button(RichText::new("⤓ latest").small()).clicked() { let t = n - 1; self.walk_focus = Some(t); self.scroll_chat_to = Some(t); }
                                });
                                ui.add_space(4.0);
                                let row_h = 30.0;
                                let content_h = n as f32 * row_h + 12.0;
                                let viewport_h = 240.0_f32;
                                // advance the camera toward the focused node — critically damped, cuts on a big jump
                                let dt = ui.input(|i| i.stable_dt).clamp(0.001, 0.05);
                                if let Some(idx) = self.walk_focus {
                                    self.walk.focus(idx, row_h, viewport_h, content_h, 3.0, viewport_h * 0.9);
                                    self.walk.tick(dt);
                                    if !self.walk.settled() { ui.ctx().request_repaint(); }
                                }
                                let mut clicked_node: Option<usize> = None;
                                egui::ScrollArea::vertical().id_salt("graph_view").max_height(viewport_h)
                                    .vertical_scroll_offset(self.walk.top)
                                    .show(ui, |ui| {
                                        let (resp, p) = ui.allocate_painter(egui::vec2(ui.available_width().max(140.0), content_h), egui::Sense::click());
                                        let rect = resp.rect;
                                        let cx = rect.left() + 16.0;
                                        let top = rect.top() + 8.0;
                                        for nd in &g.nodes {
                                            let y = top + nd.id as f32 * row_h;
                                            if let Some(pid) = nd.parent {
                                                let py = top + pid as f32 * row_h;
                                                let edge = if nd.in_context && g.nodes[pid].in_context { theme::CYAN().linear_multiply(0.5) } else { theme::LINE() };
                                                p.line_segment([egui::pos2(cx, py), egui::pos2(cx, y)], egui::Stroke::new(1.5_f32, edge));
                                            }
                                            let base = if nd.turn == crate::convograph::Turn::User { theme::CYAN() } else { theme::GREEN() };
                                            let dot = if nd.in_context { base } else { base.linear_multiply(0.30) };
                                            let focused = self.walk_focus == Some(nd.id);
                                            if focused { p.circle_stroke(egui::pos2(cx, y), 8.0, egui::Stroke::new(2.0_f32, theme::AMBER())); }
                                            if nd.pruned { p.circle_stroke(egui::pos2(cx, y), 5.0, egui::Stroke::new(1.0_f32, theme::DIM())); }
                                            else { p.circle_filled(egui::pos2(cx, y), 5.0, dot); }
                                            let tcol = if nd.pruned { theme::FAINT() } else if focused { theme::AMBER() } else if nd.in_context { theme::INK() } else { theme::DIM() };
                                            let who = if nd.turn == crate::convograph::Turn::User { "you" } else { "asst" };
                                            p.text(egui::pos2(cx + 14.0, y), egui::Align2::LEFT_CENTER,
                                                format!("{who} · {}t  {}{}", nd.tokens, if nd.pruned { "⌫ " } else { "" }, nd.preview),
                                                egui::FontId::proportional(11.0), tcol);
                                        }
                                        if resp.clicked() {
                                            if let Some(pos) = resp.interact_pointer_pos() {
                                                let i = (((pos.y - top) / row_h).round() as i64).clamp(0, n as i64 - 1) as usize;
                                                clicked_node = Some(i);
                                            }
                                        }
                                    });
                                if let Some(i) = clicked_node { self.walk_focus = Some(i); self.scroll_chat_to = Some(i); }
                                ui.add_space(8.0);
                                ui.separator();
                                ui.label(RichText::new(format!("{n} turns stored · {in_ctx} in window · ~{used}/{budget} tok")).color(theme::DIM()).small());
                                ui.label(RichText::new("The amber ring is the walkthrough focus; click or step jumps the chat there. Branch forks appear once multiple-solution mapping is wired.").color(theme::FAINT()).small());
                            }
                            }); // close "Conversation graph" carved panel
                        }
                        RightPage::Files => {
                            // WHY THESE FILES. The planner ranks every candidate
                            // and shows the model a few; without this the choice
                            // is invisible, and an invisible choice is one nobody
                            // can disagree with. The field case: "add a bright
                            // pink theme" put two Cargo.tomls in front of the
                            // model at 0.90 and dropped palette.rs at 0.17.
                            {
                                let why = self.planned_why.borrow();
                                if !why.is_empty() {
                                    theme::panel(ui, theme::BLUE(), "Workspace files in context", None, |ui| {
                                        let shown = why.iter().filter(|e| e.verdict != "skip").count();
                                        ui.label(RichText::new(format!(
                                            "{} of {} candidates, ranked by relevance to your last message",
                                            shown, why.len())).color(theme::DIM()).small());
                                        egui::ScrollArea::vertical().max_height(220.0)
                                            .id_salt("ctxplan-why").show(ui, |ui| {
                                            // Ranked order, so the cut line is visible: the
                                            // first thing that did NOT make it sits directly
                                            // under the last thing that did.
                                            let mut rows: Vec<_> = why.iter().collect();
                                            rows.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance)
                                                .unwrap_or(std::cmp::Ordering::Equal));
                                            for e in rows {
                                                let inc = e.verdict.starts_with("whole") || e.verdict.starts_with("peek");
                                                let col = if inc { theme::INK() } else { theme::DIM() };
                                                ui.horizontal(|ui| {
                                                    ui.label(RichText::new(format!("{:.2}", e.relevance))
                                                        .monospace().small().color(
                                                            if inc { theme::GREEN() } else { theme::DIM() }));
                                                    ui.label(RichText::new(&e.path).small().color(col));
                                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                        ui.label(RichText::new(&e.verdict).small().color(theme::DIM()));
                                                    });
                                                });
                                            }
                                        });
                                    });
                                    ui.add_space(6.0);
                                }
                            }
                            theme::panel(ui, theme::GREEN(), "Generated files", None, |ui| {
                            if arts.is_empty() {
                                ui.label(RichText::new("No files in the latest reply yet.").color(theme::DIM()).small());
                            } else {
                                ui.label(RichText::new(format!("{} file(s) from the latest reply:", arts.len())).color(theme::DIM()).small());
                                for a in &arts { ui.label(RichText::new(format!("• {}", a.path)).color(theme::FAINT()).small()); }
                                ui.add_space(6.0);
                                // ── sandbox oracle: cargo check the generated files in an isolated copy ──
                                let running = self.verify_job.as_ref().map(|j| j.result.is_none()).unwrap_or(false);
                                let rust = arts.iter().any(|a| a.path.ends_with(".rs"));
                                ui.horizontal(|ui| {
                                    if ui.add_enabled(!running && rust, egui::Button::new(RichText::new("⌗ Verify build").small()))
                                        .on_hover_text("Offline-first: scaffold the files into a throwaway cargo project and run `cargo check --offline` in an isolated copy.")
                                        .clicked()
                                    {
                                        let files: Vec<(String, String)> = arts.iter().map(|a| (a.path.clone(), a.content.clone())).collect();
                                        let allow_net = self.verify_net;
                                        let (tx, rx) = std::sync::mpsc::channel();
                                        let root = std::env::temp_dir().join("logscope-verify-ui");
                                        std::thread::spawn(move || {
                                            let r = crate::sandbox::verify_files("generated", &files, &root, allow_net)
                                                .map(|r| (r.passed, r.output, r.sandboxed))
                                                .unwrap_or_else(|e| (false, format!("verify error: {e}"), false));
                                            let _ = tx.send(r);
                                        });
                                        self.verify_job = Some(VerifyJob { rx, started: Instant::now(), result: None });
                                    }
                                    ui.checkbox(&mut self.verify_net, RichText::new("fetch deps").small())
                                        .on_hover_text("Off = hermetic (offline only). On = if a dep isn't cached, fetch it from crates.io once (then cached).");
                                });
                                if !rust { ui.label(RichText::new("(no .rs files to check)").color(theme::FAINT()).small()); }
                                if let Some(job) = &self.verify_job {
                                    match &job.result {
                                        None => { ui.label(RichText::new(format!("checking… {:.0}s", job.started.elapsed().as_secs_f32())).color(theme::AMBER()).small()); }
                                        Some((passed, output, sandboxed)) => {
                                            let (txt, col, note) = match crate::sandbox::classify(*passed, output) {
                                                crate::sandbox::Verdict::Pass => ("✓ compiles", theme::GREEN(), ""),
                                                crate::sandbox::Verdict::CompileError => ("✗ build failed", theme::RED(), ""),
                                                crate::sandbox::Verdict::TestFailure => ("✗ tests failed", theme::RED(), "it compiled, but a test assertion failed — see the output below"),
                                                crate::sandbox::Verdict::DepsUnavailable => ("⚠ dependencies not cached", theme::AMBER(), "ran offline; tick 'fetch deps' to resolve from crates.io once (the code itself wasn't judged)"),
                                                crate::sandbox::Verdict::ToolError => ("⚠ verify error", theme::AMBER(), ""),
                                                // The note carries the fix, because the operator
                                                // cannot act on "refused" alone — and the code was
                                                // NOT judged, which the wording has to say or this
                                                // reads as a verdict on the model's work.
                                                crate::sandbox::Verdict::Refused => ("⊘ not verified", theme::AMBER(),
                                                    "the sandbox refused to run this — nothing was checked. Install bubblewrap, or mark the code Trust::Own if you wrote it"),
                                            };
                                            ui.horizontal(|ui| {
                                                ui.label(RichText::new(txt).color(col).strong());
                                                if !*sandboxed { ui.label(RichText::new("· unsandboxed").color(theme::FAINT()).small()); }
                                            });
                                            if !note.is_empty() { ui.label(RichText::new(note).color(theme::FAINT()).small()); }
                                            if !output.trim().is_empty() {
                                                egui::ScrollArea::both().id_salt("verify_out").max_height(220.0).show(ui, |ui| {
                                                    ui.add(egui::Label::new(RichText::new(output).monospace().small()));
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                            ui.add_space(6.0);
                            ui.label(RichText::new("Files also appear in the drawer at the bottom (preview, save, history). Verify is the objective grader behind the coordinator loop — the compiler, not an LLM, decides if generated code is real.").color(theme::FAINT()).small());
                            }); // close "Generated files" carved panel
                        }
                    }
                });
            });
        }

        // ── the engine's compaction request, picked up once per frame ──
        // ctxwin::relief publishes this when the long-horizon policy decides the reply budget has
        // been squeezed below a usable answer. The ACTION is `:compact`, which already exists —
        // student/teacher self-compaction with a pinnable KV checkpoint — so this offers that
        // rather than growing a second folding path beside it.
        if let Some(req) = infer.compact_request() {
            // CLEAR ONLY WHAT WE TOOK. The engine publishes this from inside
            // generate() — i.e. always while the stream is live — so a poll
            // that skipped the offer because `stream.is_some()` and cleared
            // anyway threw the request away before the stream could end. The
            // offer then never appeared, which is "the summarize toggle does
            // nothing" exactly: the policy fired, the request was published,
            // and one frame later it was gone. Declines still clear (the
            // seq check below handles that), so a request is dropped only
            // when it has actually been seen.
            match crate::backend::compact_poll(req.seq, self.compact_declined, self.stream.is_some()) {
                crate::backend::CompactPoll::Take => {
                    self.compact_offer = Some(req);
                    infer.clear_compact_request();
                }
                crate::backend::CompactPoll::Clear => infer.clear_compact_request(),
                crate::backend::CompactPoll::Leave => {}
            }
            // else: leave it pending — the next frame after the stream ends takes it
        }
        let mut do_compact = false;
        let mut do_send = false;
        let mut do_continue = false;
        let mut do_checkpoint = false;
        let mut do_export = false;
        let mut do_restore = false;
        let mut rollback: Option<usize> = None;
        let mut raise_ctx: Option<u32> = None;
        let mut action: Option<MsgAction> = None;

        // ── bottom: generated files + composer, PINNED so they never scroll off-screen even
        //    when the Context panel is open or a reply produces a tall file list ──
        // Gated behind hide_bottom_bar: when hidden, this whole panel doesn't exist, so the spatial
        // view's CentralPanel gets the full remaining height instead of being cramped behind it.
        if !self.hide_bottom_bar {
        egui::Panel::bottom("chat_bottom").resizable(false).show_inside(ui, |ui| {
            ui.add_space(theme::PAD_XS as f32);
            if !arts.is_empty() && self.show_files {
                // Own CollapsingHeader (dynamic count/stitched text) rather than a fixed title, so
                // this can't go through theme::panel() directly — carve the same frame by hand
                // instead of leaving it flat.
                let gen_files_inner = egui::Frame { fill: theme::PANEL(), corner_radius: theme::r(10), inner_margin: egui::Margin::same(theme::PAD_MD),
                    outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() }, stroke: egui::Stroke::new(1.0_f32, theme::LINE()), ..Default::default() }
                .show(ui, |ui| {
                    let stitched = arts.iter().filter(|a| a.fragments > 1).count();
                    let hdr = if stitched > 0 {
                        format!("⎙ Generated files ({}) — {stitched} stitched from stop/start · click a row to preview", arts.len())
                    } else {
                        format!("⎙ Generated files ({}) — click a row to preview", arts.len())
                    };
                    // Collapsible drawer (closed by default) so a tall file list never blocks the
                    // chat — the header still shows the count; click to expand. egui skips the body
                    // entirely while collapsed, and remembers the open/closed choice across frames.
                    egui::CollapsingHeader::new(RichText::new(hdr).color(theme::CYAN()).strong().small())
                        .id_salt("gen_files_drawer").default_open(false).show(ui, |ui| {
                    // Provenance view for the expanded file: each prior version linked to the turn (and its
                    // reasoning) that produced it. Precomputed here so the row loop can mutate self freely.
                    let file_hist_view: Vec<(crate::vault::NodeId, String, String)> =
                        match (self.file_hist, self.head, self.vault.as_ref()) {
                            (Some(fi), Some(h), Some(v)) if fi < arts.len() => v.file_history(h, &arts[fi].path).unwrap_or_default()
                                .into_iter().map(|(nid, fe)| {
                                    let why = v.get_text(&fe.produced_by);
                                    let snip = why.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(90).collect::<String>();
                                    (nid, snip, fe.hash.chars().take(8).collect::<String>())
                                }).collect(),
                            _ => Vec::new(),
                        };
                    let mut toggle_hist: Option<usize> = None;
                    egui::ScrollArea::vertical().id_salt("gen_files").auto_shrink([false, true]).max_height(220.0).show(ui, |ui| {
                        for (i, a) in arts.iter().enumerate() {
                            let badge = if a.kind == crate::workspace::Kind::Patch { "patch" } else { "file" };
                            let sz = a.content.len();
                            let szs = if sz >= 1024 { format!("{:.1}K", sz as f32 / 1024.0) } else { format!("{sz} B") };
                            ui.horizontal(|ui| {
                                let open = self.preview.as_deref() == Some(a.path.as_str());
                                let tri = if open { "▾" } else { "▸" };
                                let merged = if a.fragments > 1 { format!("  ·  merged ×{}", a.fragments) } else { String::new() };
                                let mut r = ui.selectable_label(open, RichText::new(format!("{tri} {}  ·  {badge}  ·  {szs}{merged}", a.path)).monospace().small().color(theme::INK()));
                                if a.fragments > 1 { r = r.on_hover_text("Reassembled from pieces emitted across stop/start (warm-resume) — one whole file"); }
                                if r.clicked() { self.preview = if open { None } else { Some(a.path.clone()) }; }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("Copy").on_hover_text("Copy this file's contents to the clipboard").clicked() {
                                        ui.ctx().copy_text(a.content.clone());
                                        self.ws_note = Some((format!("copied {} to clipboard", a.path), Instant::now()));
                                    }
                                    if self.head.is_some() {
                                        let on = self.file_hist == Some(i);
                                        if ui.selectable_label(on, RichText::new("↟").small())
                                            .on_hover_text("Version history — and the reasoning behind each change").clicked() { toggle_hist = Some(i); }
                                    }
                                });
                            });
                            if self.file_hist == Some(i) {
                                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6), inner_margin: egui::Margin::same(theme::PAD_SM),
                                    stroke: egui::Stroke::new(1.0_f32, theme::LINE()), ..Default::default() }
                                .show(ui, |ui| {
                                    if file_hist_view.is_empty() {
                                        ui.label(RichText::new("no recorded history for this file yet").color(theme::FAINT()).small());
                                    } else {
                                        ui.label(RichText::new(format!("history of {} — why it changed", a.path)).color(theme::CYAN()).small());
                                        for (nid, snip, vh) in &file_hist_view {
                                            ui.label(RichText::new(format!("#{nid} · v{vh} · {snip}")).color(theme::DIM()).small().monospace());
                                        }
                                    }
                                });
                            }
                            if self.preview.as_deref() == Some(a.path.as_str()) {
                                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6), inner_margin: egui::Margin::same(theme::PAD_SM), ..Default::default() }
                                .show(ui, |ui| {
                                    // Keyed by path, not by `i`. `arts` is re-parsed from the
                                    // message text every frame, so a reply that emits a new
                                    // artifact renumbers the ones after it and this pane would
                                    // inherit another artifact's scroll offset.
                                    //
                                    // `self.preview` is keyed by path too, for the same reason —
                                    // it was an index into this vector and previewed the wrong
                                    // artifact after a renumber.
                                    egui::ScrollArea::vertical().id_salt(("prev", a.path.as_str())).max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
                                        ui.label(RichText::new(&a.content).monospace().small().color(theme::DIM()));
                                    });
                                });
                            }
                        }
                    });
                    if let Some(i) = toggle_hist { self.file_hist = if self.file_hist == Some(i) { None } else { Some(i) }; }
                    ui.add_space(theme::PAD_XS as f32);
                    ui.horizontal_wrapped(|ui| {
                        if theme::action_button(ui, "Write workspace", theme::GREEN(), true).clicked() {
                            self.ws_note = Some((match crate::workspace::write_workspace(&arts) {
                                Ok(dir) => { self.last_saved = Some(dir.clone()); format!("wrote {} files → {}", arts.len(), dir.display()) }
                                Err(e) => format!("error: {e}"),
                            }, Instant::now()));
                        }
                        if theme::action_button(ui, "Save .zip", theme::AMBER(), true).clicked() {
                            self.ws_note = Some((match crate::workspace::write_workspace(&arts).and_then(|d| crate::workspace::zip_artifacts(&arts, &d)) {
                                Ok(zip) => { self.last_saved = zip.parent().map(|p| p.to_path_buf()); format!("bundled → {}", zip.display()) }
                                Err(e) => format!("error: {e}"),
                            }, Instant::now()));
                        }
                        if ui.button("📁 Save to folder…").clicked() {
                            let start = self.save_dir.clone()
                                .or_else(|| std::env::var("HOME").ok().map(std::path::PathBuf::from))
                                .unwrap_or_else(|| std::path::PathBuf::from("/"));
                            self.save_picker.open_dir(&start, "Save generated files to…");
                        }
                        if let Some(dir) = self.save_dir.clone() {
                            let name = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| dir.display().to_string());
                            if ui.button(format!("💾 Save to {name}")).on_hover_text(format!("Re-save into {}", dir.display())).clicked() {
                                self.save_to(&arts, dir);
                            }
                        }
                        if let Some(dir) = self.last_saved.clone() {
                            if ui.button("📂 Open folder").on_hover_text(format!("Reveal {}", dir.display())).clicked() { Self::reveal(&dir); }
                        }
                    });
                    }); // close gen_files_drawer
                });
                let t = ui.input(|i| i.time) as f32;
                theme::carved_frame(ui.painter(), gen_files_inner.response.rect, t, theme::GREEN());
            }
            if let Some((msg, at)) = &self.ws_note {
                let a = 1.0 - theme::appear(*at, 8.0);
                if a > 0.0 { ui.ctx().request_repaint(); theme::fade(ui, a, |ui| { ui.label(RichText::new(msg).color(theme::GREEN()).small()); }); }
            }
            // WHAT ACTUALLY HAPPENED, above what might. `ctx_suggestion` below is an ESTIMATE of
            // the next prompt (est() over the text); `context_fit` is the engine's MEASUREMENT of
            // the last one, and when it reports tokens elided from the middle that is not a
            // prediction to hedge — it is a fact about the reply already on screen. It had no
            // callers at all until now, which is why a session could quietly lose the middle of
            // every prompt with nothing in the UI saying so.
            if let Some(fit) = infer.context_fit().filter(|f| f.elided > 0) {
                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(8), inner_margin: egui::Margin::same(theme::PAD_SM),
                    outer_margin: egui::Margin { bottom: theme::PAD_XS, ..Default::default() }, stroke: egui::Stroke::new(1.0_f32, theme::AMBER()), ..Default::default() }
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("✂").color(theme::AMBER()).small());
                        ui.label(RichText::new(format!(
                            "Last turn: {} tokens of prompt into a {}-token window — {} elided from the middle \
(head and tail kept, the cut marked in the prompt).",
                            fit.prompt, fit.window, fit.elided)).color(theme::DIM()).small());
                    });
                });
            }
            // context-overflow suggestion: the next prompt won't fit the window, so it'd be trimmed
            if let Some(want) = self.ctx_suggestion(doc) {
                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(8), inner_margin: egui::Margin::same(theme::PAD_SM),
                    outer_margin: egui::Margin { bottom: theme::PAD_XS, ..Default::default() }, stroke: egui::Stroke::new(1.0_f32, theme::AMBER()), ..Default::default() }
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("⚠").color(theme::AMBER()).small());
                        ui.label(RichText::new(format!("This is longer than the {}-token window — it'll be trimmed.", self.ctx.n_ctx)).color(theme::DIM()).small());
                        if theme::action_button(ui, &format!("Raise to {want}"), theme::AMBER(), true).clicked() { raise_ctx = Some(want); }
                    });
                });
            }
            if self.show_metrics {
                theme::panel(ui, theme::CYAN(), "GPU phase breakdown — last generation", None, |ui| {
                    let now = crate::metricspanel::snapshot(self.metrics_tokens);
                    let lines = now.since(&self.metrics_base).report(self.metrics_wall_ns);
                    if lines.is_empty() {
                        ui.label(RichText::new("nothing measured yet — send a message").color(theme::DIM()).small());
                    } else {
                        for l in &lines {
                            // the dominant/remedy line is the actionable one, so it gets the accent
                            let c = if l.starts_with("dominant:") { theme::AMBER() } else { theme::DIM() };
                            ui.label(RichText::new(l).color(c).monospace().small());
                        }
                    }
                    if let Some(why) = now.heatmap_unavailable() {
                        ui.add_space(theme::PAD_XS as f32);
                        ui.label(RichText::new(format!("heatmap: {why}")).color(theme::FAINT()).small());
                    }
                });
            }
            if self.show_checkpoints && !self.checkpoints.is_empty() {
                theme::panel(ui, theme::CYAN(), "Checkpoints", None, |ui| {
                    egui::ScrollArea::vertical().id_salt("ckpts").max_height(140.0).show(ui, |ui| {
                        for (i, cp) in self.checkpoints.iter().enumerate().rev() {
                            theme::list_item(ui, false, |ui| {
                                ui.horizontal(|ui| {
                                    if theme::action_button(ui, "Restore", theme::CYAN(), !streaming).clicked() { rollback = Some(i); }
                                    ui.label(RichText::new(format!("{} · {} msgs", cp.label, cp.msgs.len())).color(theme::DIM()).small());
                                });
                            });
                        }
                    });
                });
            }
            // history tree: every committed turn is a node; "Return" loads it and the next turn forks.
            if self.show_history {
                let head_now = self.head;
                let rows: Vec<(crate::vault::NodeId, Option<crate::vault::NodeId>, String, usize, usize)> =
                    self.vault.as_ref().map(|v| v.all_ids().iter().rev()
                        .filter_map(|&id| v.node(id).ok().map(|n| (n.id, n.parent, n.label.clone(), n.files.len(), n.cum_len)))
                        .collect()).unwrap_or_default();
                let mut return_to: Option<crate::vault::NodeId> = None;
                if rows.is_empty() {
                    ui.label(RichText::new("no history yet — finish a turn to create the first node").color(theme::FAINT()).small());
                } else {
                    theme::panel(ui, theme::CYAN(), "History", None, |ui| {
                        egui::ScrollArea::vertical().id_salt("tree").max_height(170.0).show(ui, |ui| {
                            for (id, parent, label, nfiles, msgs) in &rows {
                                let here = head_now == Some(*id);
                                theme::list_item(ui, here, |ui| {
                                    ui.horizontal(|ui| {
                                        if theme::action_button(ui, if here { "● here" } else { "Return" }, theme::CYAN(), !streaming && !here).clicked() { return_to = Some(*id); }
                                        let from = parent.map(|p| format!(" ←#{p}")).unwrap_or_else(|| " (root)".into());
                                        ui.label(RichText::new(format!("#{id}{from}  {}  · {} msgs · {} files", label, msgs, nfiles)).color(if here { theme::CYAN() } else { theme::DIM() }).small());
                                    });
                                });
                            }
                        });
                    });
                }
                if let Some(id) = return_to { self.return_to_node(id); }
            }
            // suggestion chips (starters / follow-ups) — click to fill the composer
            if self.input.trim().is_empty() {
                let sugg = self.suggestions(doc);
                if !sugg.is_empty() {
                    // KEYS ON THE CHIPS, as the reference panels print them beside every quick
                    // action. Positional rather than per-action: these chips are generated from the
                    // conversation, so binding a key to "Summarize" would bind it to nothing
                    // whenever the last reply contained code. See `chords`.
                    let picked = ui.input_mut(|i| {
                        let mods = i.modifiers;
                        (0..crate::chords::MAX_INDEXED.min(sugg.len())).find(|&n| {
                            crate::chords::digit_for(n)
                                .map(|k| crate::chords::fires(n, mods, k) && i.consume_key(egui::Modifiers::COMMAND, k))
                                .unwrap_or(false)
                        })
                    });
                    if let Some(n) = picked {
                        if let Some(s) = sugg.get(n) { self.input = s.clone(); }
                    }
                    ui.horizontal_wrapped(|ui| {
                        for (n, s) in sugg.iter().enumerate() {
                            let face = match crate::chords::chord_text(n) {
                                Some(c) if n < crate::chords::MAX_INDEXED => format!("{s}  {c}"),
                                _ => s.clone(),
                            };
                            if theme::carved_button_tinted(ui, &face, theme::accent()).clicked() {
                                self.input = s.clone();
                            }
                        }
                    });
                    ui.add_space(theme::PAD_XS as f32);
                }
            }
            // id autocomplete — typing a node-id prefix in :mergectx / :goto offers matching ids; click to complete
            else {
                // command palette: a hint line for the command being typed (persists through its arguments),
                // and a zsh-style grid of name completions while the command itself is still being typed.
                if let Some(h) = crate::vimmode::command_hint(&self.input) {
                    let args = if h.args.is_empty() { String::new() } else { format!(" {}", h.args) };
                    ui.label(RichText::new(format!(":{}{}  —  {}", h.name, args, h.help)).color(theme::AMBER_INK()).small());
                }
                let cmds = crate::vimmode::command_completions(&self.input);
                if !cmds.is_empty() {
                    egui::Grid::new("cmd_palette").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                        for (k, c) in cmds.iter().enumerate() {
                            let hover = if c.aliases.is_empty() { format!(":{} {}\n{}", c.name, c.args, c.help) }
                                        else { format!(":{} {}\naliases: {}\n{}", c.name, c.args, c.aliases.join(", "), c.help) };
                            if ui.add(egui::Button::new(RichText::new(format!(":{}", c.name)).monospace().color(theme::CYAN()).small()).frame(true))
                                .on_hover_text(hover).clicked() {
                                self.input = format!(":{} ", c.name);  // complete the command, ready for its arguments
                            }
                            if k % 4 == 3 { ui.end_row(); }
                        }
                    });
                    ui.add_space(theme::PAD_XS as f32);
                } else {
                    let comps = self.id_completions();
                    if !comps.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new("ids").color(theme::DIM()).small());
                            for (id, completed) in &comps {
                                if ui.add(egui::Button::new(RichText::new(id).monospace().color(theme::VIOLET()).small()).frame(true)).clicked() {
                                    self.input = completed.clone();
                                }
                            }
                        });
                        ui.add_space(theme::PAD_XS as f32);
                    }
                }
            }
            let composer_frame = egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(10), inner_margin: egui::Margin::same(theme::PAD_MD),
                outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() }, stroke: egui::Stroke::new(1.0_f32, theme::LINE()), ..Default::default() }
            .show(ui, |ui| {
                if let Some(bf) = self.branch_from {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("⑂ branching from turn {} — your next message forks a side-thread", bf + 1)).color(egui::Color32::from_rgb(0xc9, 0x8f, 0xff)).small().strong());
                        if ui.small_button("✕ cancel").clicked() { self.branch_from = None; }
                    });
                    ui.add_space(2.0);
                }
                // Stable, explicit id: the suggestion chips above are rendered only while the input
                // is empty (see above), so typing the first character removes those widgets and would
                // shift this TextEdit's AUTO id — which makes egui drop focus after one keystroke.
                // Pinning the id keeps focus across that empty→non-empty layout change.
                // The composer is a PEARL: the typed text is drawn as temporally-stable resonance cells (same
                // language as the 3D pearls), with a real TextEdit overlaid transparently on the same band so it
                // still owns input, caret, selection and IME — and the mobile focus fix below still applies.
                let pearl_t = ui.input(|i| i.time) as f32;
                // live-transcription preview: while the mic is on, the forming utterance shows on the pearl
                // (dimmer, ahead of the committed text) so dictation has real-time feedback before it commits.
                let vpartial = self.voice.as_ref().map(|v| v.partial().to_string()).unwrap_or_default();
                let band = self.pearl_input.show(ui, &self.input, &vpartial, pearl_t, 132.0, theme::CYAN());
                // the pearl draws its OWN caret shimmer aligned to the cells; hide egui's caret (it sits at the
                // transparent TextEdit's content origin — up/left of the cells) and its selection fill.
                ui.visuals_mut().text_cursor.stroke = egui::Stroke::NONE;
                ui.visuals_mut().selection.bg_fill = egui::Color32::from_rgba_unmultiplied(0x4d, 0xd6, 0xe6, 36);
                let resp = ui.put(band, egui::TextEdit::multiline(&mut self.input)
                            // SAME salt at both call sites on purpose. This composer is
                            // drawn from two places (the wide and the narrow layout), and
                            // an auto-id would differ between them — so crossing the
                            // responsive breakpoint mid-sentence silently dropped focus
                            // and everything typed after it went nowhere. One stable id
                            // makes the two branches the same widget as far as focus is
                            // concerned.
                            .id_salt("logscope-chat-composer")
                    .id(egui::Id::new("logscope_chat_composer"))
                    .desired_width(f32::INFINITY)
                    .frame(egui::Frame::NONE)
                    .text_color(egui::Color32::TRANSPARENT)   // invisible text — the pearl cells are the visual
                    .font(egui::FontId::new(14.0, egui::FontFamily::Monospace)));
                // ENTER IS ROUTED, not hardcoded. With a fan open the composer is addressing one
                // column or all of them, and the keystroke has to say which — the rule (and the
                // reason Shift stays a newline and Ctrl only means something when aimed) is in
                // replicatorlayout::enter_action, tested headlessly.
                let fan_open = self.replicator.is_some();
                let fan_aim = self.replicator.as_ref().and_then(|m| m.aim);
                let pressed = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                let mods = ui.input(|i| crate::replicatorlayout::Mods {
                    shift: i.modifiers.shift, ctrl: i.modifiers.command || i.modifiers.ctrl });
                let mut enter = false;
                if pressed {
                    match crate::replicatorlayout::enter_action(
                        fan_open, fan_aim, crate::replicatorlayout::Focus::Composer, mods) {
                        crate::replicatorlayout::EnterAction::Newline => {}
                        crate::replicatorlayout::EnterAction::SendAimed => enter = true,
                        crate::replicatorlayout::EnterAction::SendBroadcast => {
                            // one message to everyone WITHOUT dropping the aim: the operator is
                            // interrupting a focused exchange, not ending it
                            enter = true;
                            self.fan_broadcast_once = true;
                        }
                        _ => {}
                    }
                }
                if enter && self.input.ends_with('\n') { self.input.pop(); }
                // Plasma Mobile / IME robustness: on some mobile stacks (Wayland + Maliit) the soft-keyboard
                // commit drops widget focus right after the FIRST character — the empty→non-empty layout change
                // re-targets the IME, so only one char lands. Signature: the text changed THIS frame but focus
                // was lost in the SAME frame. When we see that, re-assert focus so typing continues. This is
                // inert on desktop (focus is retained there) and when the user navigates away (no text change).
                if resp.changed() {
                    self.mode = crate::vimmode::Mode::Insert;
                    if !resp.has_focus() { resp.request_focus(); }
                }
                // Composer command hint: when the line is a ':' command (and not a '::' escaped message),
                // show its usage + argument suggestions inline — the same palette the vim cmdline uses — so
                // commands typed straight into the message box are discoverable and self-documenting, and it's
                // visually obvious the line will RUN a command instead of being sent to the model.
                let ci = self.input.trim();
                if ci.starts_with(':') && !ci.starts_with("::") {
                    let raw = ci.trim_start_matches(':');
                    let mut parts = raw.splitn(2, char::is_whitespace);
                    let head = parts.next().unwrap_or("");
                    let arg_partial = parts.next().unwrap_or("");
                    if let Some(s) = crate::cmdpalette::spec(head) {
                        ui.label(RichText::new(format!("⌘ {}  ·  {}", s.usage, s.help)).color(theme::CYAN()).small());
                        let ctx = self.arg_ctx();
                        let sugg = crate::cmdpalette::suggest_args(s, arg_partial, &ctx);
                        if !sugg.is_empty() {
                            let line = sugg.iter().take(4).map(|a| format!("{} — {}", a.value, a.hint)).collect::<Vec<_>>().join("    ");
                            ui.label(RichText::new(line).color(theme::FAINT()).small());
                        }
                    } else if !head.is_empty() {
                        let names: Vec<&str> = crate::cmdpalette::matches(head).iter().take(8).map(|s| s.name).collect();
                        ui.label(RichText::new(if names.is_empty() { "no match · :help to list commands".to_string() } else { format!("↹ {}", names.join("  ")) })
                            .color(theme::DIM()).small());
                    } else {
                        ui.label(RichText::new("command mode · :help to list · Enter runs it (not sent to the model)").color(theme::DIM()).small());
                    }
                }
                ui.add_space(theme::PAD_XS as f32);
                // NOT the replicator fan. It used to be drawn here, inside the composer's vertical
                // flow — a second chat area stacked under the first, growing downward as workers
                // replied and pushing the composer toward the bottom edge. A fan is a parallel
                // view of the conversation, not a continuation of it, so it now SPLITS the chat
                // area (side panel below) and both halves scroll under one composer.
                self.poll_image(ui);
                // MOBILE OVERLAP. egui's horizontal() does not wrap: controls past the right edge are drawn
                // anyway, on top of each other. On a desktop this bar never reaches the edge, so the failure is
                // invisible in development and immediate on a phone. horizontal_wrapped stops the overlap;
                // mobilelayout::row_mode decides when a row should instead SHED controls, because a wrapped
                // four-line bar of tiny buttons is not usable either.
                ui.horizontal_wrapped(|ui| {
                    // SHED, DO NOT STACK. Wrapping stopped the overlap and left a phone with a four-line bar of
                    // tiny buttons, which is not usable either — I fixed half the problem in _367 and wired the
                    // decision that fixes the other half without ever calling it.
                    //
                    // Two controls must stay reachable (send state, and the mode the run is in); the rest go
                    // behind a menu when the width cannot hold them at a touch-sized target.
                    let touch = ui.input(|i| i.pointer.has_pointer()) == false || cfg!(target_os = "android");
                    let overflow = crate::mobilelayout::row_mode(ui.available_width(), 2, 5, touch)
                        == crate::mobilelayout::RowMode::Overflow;
                    if theme::carved_button(ui, "📎 Attach").on_hover_text("Attach a text file into your message (bounded)").clicked() {
                        let start = self.save_dir.clone()
                            .or_else(|| std::env::var("HOME").ok().map(std::path::PathBuf::from))
                            .unwrap_or_else(|| std::path::PathBuf::from("/"));
                        self.attach_picker.open_file(&start, "Attach a file", &[]);
                    }
                    // Mic toggle: start/stop the live voice session. Dictates finalized speech into the composer
                    // (the routing seam in on_voice_utterance grows into graph commands next). Errors surface
                    // inline (no model/mic, or a build without the `voice` feature).
                    let miced = self.voice.is_some();
                    if theme::carved_button(ui, if miced { "🔴 Stop mic" } else { "🎙 Mic" })
                        .on_hover_text(
                            "Voice — speak; each utterance runs as a command or dictates.\n\
                             Nav: \"go to 12\" · \"focus on three\" · \"node seven\"\n\
                             Graph: \"branch here\" · \"prune\" · \"unprune\" · \"best\"\n\
                             View: \"overview\" · \"show the palace\" · \"back to chat\"\n\
                             Camera: \"start/stop tour\" · \"free cam\" / \"exit freecam\"\n\
                             Compose: \"send it\" · \"stop\" (cancel) · anything else dictates")
                        .clicked() {
                        if miced { self.voice = None; }
                        else {
                            match crate::voice_session::VoiceSession::start() {
                                Ok(s) => { self.voice = Some(s); self.voice_err = None; ui.ctx().request_repaint(); }
                                Err(e) => { self.voice_err = Some(format!("voice: {e}")); }
                            }
                        }
                    }
                    if let Some(v) = &self.voice {
                        let p = v.partial();
                        let shown = if p.is_empty() { "listening…".to_string() } else { p.chars().take(60).collect::<String>() };
                        ui.label(RichText::new(shown).color(theme::CYAN()).small().italics());
                        ui.ctx().request_repaint(); // keep transcription live while the mic is on
                    } else if let Some(e) = &self.voice_err {
                        ui.label(RichText::new(e).color(egui::Color32::from_rgb(0xe0, 0x6c, 0x6c)).small());
                    }
                    ui.label(RichText::new(if self.threading { "threaded" } else { "single-shot" }).color(theme::FAINT()).small());
                    // PANEL MODE. Built in _373/_375 and never given a control, which is why it could not be
                    // found in the menu. Its advantage is not answer quality — it is that several short turns
                    // can each stop and ask, where one long generation cannot interrupt itself usefully.
                    // The four mode toggles, rendered inline when the bar can hold them and inside the ⋯ menu
                    // when it cannot. Hoisted into locals so ONE definition serves both placements — two copies
                    // would be a policy expressed twice, which is how the watchdog tables drifted.
                    let (mut panel_mode, mut stepwise) = (self.panel_mode, self.stepwise);
                    let (mut auto_continue, mut patch_mode) = (self.auto_continue, self.patch_mode);
                    let mut relief_policy = self.relief_policy;
                    {
                        // Split the borrow: the closure below already
                        // holds `&mut` locals, so reaching through `self`
                        // inside it would conflict. Same pattern the
                        // bot-arena and snapclone adoptions needed.
                        let a11y = &mut self.a11y;
                        let mut modes = |ui: &mut egui::Ui| {
                        uikit_egui::a11y::note(a11y, egui::Id::new("logscope.chat.panel_mode").value(),
                            "debate panel", panel_mode,
                            ui.checkbox(&mut panel_mode, RichText::new("debate panel").small()))
                            .on_hover_text("Several small models debate instead of one thinker. They can pause to \
    ask you a question between messages, and one member is held BLIND to the answer — so the next \
    round can tell whether your hint drove the conclusion or the question did.");
                        uikit_egui::a11y::note(a11y, egui::Id::new("logscope.chat.stepwise").value(),
                            "stepwise", stepwise,
                            ui.checkbox(&mut stepwise, RichText::new("stepwise").small()))
                            .on_hover_text("checkpoint mode: the model works in numbered steps, pausing at ⏸ STEP k/n markers; the harness continues for you (warm-KV). Each pause is a fan-out injection point.");
                        uikit_egui::a11y::note(a11y, egui::Id::new("logscope.chat.auto_continue").value(),
                            "auto-continue", auto_continue,
                            ui.checkbox(&mut auto_continue, RichText::new("auto-continue").small()))
                            .on_hover_text("When a reply hits the length cap, keep generating to finish it");
                        // The relief policy, next to the mode it modifies. Enabled only with
                        // auto-continue on: that is the mode whose replies are long enough for the
                        // budget squeeze to cost an answer, and a checkbox that silently does
                        // nothing is worse than one that is visibly unavailable.
                        // A DROPDOWN, not a checkbox. Two states could not hold "ask",
                        // and the code around the compaction path keeps saying a human
                        // should be in the loop — so the case those comments describe
                        // was the one setting that could not be selected.
                        ui.add_enabled_ui(auto_continue, |ui| {
                            let before = relief_policy;
                            egui::ComboBox::from_id_salt("logscope.chat.relief_policy")
                                .selected_text(RichText::new(relief_policy.label()).small())
                                .show_ui(ui, |ui| {
                                    for p in crate::relief_policy::ReliefPolicy::all() {
                                        ui.selectable_value(
                                            &mut relief_policy, p,
                                            RichText::new(p.label()).small())
                                            .on_hover_text(p.hover());
                                    }
                                });
                            // `a11y::note` takes a bool, which is the same shape
                            // problem the checkbox had. The dropdown announces
                            // itself as three entries — each `selectable_value`
                            // carries its own label and hover — and the combo's
                            // response below carries the current one, so a reader
                            // hears which policy is active rather than "on/off".
                            let _ = before;
                        }).response.on_hover_text(relief_policy.hover());
                        uikit_egui::a11y::note(a11y, egui::Id::new("logscope.chat.patch_mode").value(),
                            "patch mode", patch_mode,
                            ui.checkbox(&mut patch_mode, RichText::new("patch mode").small()))
                            .on_hover_text("edit mode: the model emits unified-diff patches (only changed lines) instead of rewriting whole files — fewer tokens per edit, applied context-anchored and committed via gitmind.");
                        };
                        if overflow { ui.menu_button("⋯ modes", |ui| modes(ui)); } else { modes(ui); }
                    }
                    self.panel_mode = panel_mode; self.stepwise = stepwise;
                    // THE PANEL, at last drawn. `panel_mode` was read
                    // only to draw its own checkbox and written straight
                    // back — a toggle a person could flip that changed
                    // nothing, which is the same class as voiceterm's
                    // microphone setting.
                    //
                    // Shown only with a fan running: `debatepanel::ui`
                    // returns early on empty slots, but drawing an empty
                    // strip would still take vertical space from the
                    // transcript and suggest a debate is happening.
                    if self.panel_mode {
                        if let Some(fan) = self.agent_fan.as_ref() {
                            let slots = fan.debate_slots();
                            if !slots.is_empty() {
                                crate::debatepanel::ui(ui, &slots);
                            }
                        }
                    }
                    self.auto_continue = auto_continue; self.patch_mode = patch_mode;
                    self.relief_policy = relief_policy;
                    if self.hit_cap && !streaming {
                        if theme::action_button(ui, "⏭ Continue", theme::CYAN(), true)
                            .on_hover_text("The last reply hit the length cap — resume it").clicked() { do_continue = true; }
                    }
                    if !arts.is_empty() {
                        let n = arts.len();
                        if ui.selectable_label(self.show_files, RichText::new(format!("⎙ {n}")).small())
                            .on_hover_text("Show/hide the generated files from the latest reply").clicked() { self.show_files = !self.show_files; }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if streaming {
                            // PHASE on the button: prefill (reading the prompt — the long silent gap before
                            // any token) reads as "thinking"; once tokens flow it's "drafting" the reply.
                            let (phase, pcol) = match self.stream.as_ref() {
                                Some(s) if s.tokens == 0 => ("✦ thinking", theme::CYAN()),
                                _ => ("✍ drafting", theme::AMBER()),
                            };
                            theme::spinner(ui, 16.0, pcol);
                            if theme::action_button(ui, "⏹ Stop", theme::RED(), true).clicked() { infer.cancel(); }
                            ui.label(RichText::new(phase).color(pcol).small())
                                .on_hover_text(if pcol == theme::CYAN() { "prefilling the prompt (no tokens yet)" } else { "decoding the reply" });
                            let can = !self.input.trim().is_empty();
                            if theme::action_button(ui, "↪ Queue", theme::CYAN(), can)
                                .on_hover_text("Fire this as a branch when the current reply finishes — no need to stop the model").clicked() { do_send = true; }
                            if !self.queue.is_empty() { ui.label(RichText::new(format!("{} queued", self.queue.len())).color(theme::CYAN()).small()); }
                        } else {
                            let can = !self.input.trim().is_empty();
                            // The label is the same decision the key makes, from the same
                            // function: a composer aimed at one worker while the button reads
                            // "Send" is how a message meant for w2 gets broadcast to four.
                            let label = crate::replicatorlayout::enter_hint(
                                self.replicator.is_some(),
                                self.replicator.as_ref().and_then(|m| m.aim),
                                self.replicator.as_ref().map(|m| m.workers.len()).unwrap_or(0));
                            if theme::action_button(ui, &label, theme::AMBER(), can).clicked() { do_send = true; }
                            if !self.queue.is_empty() { ui.label(RichText::new(format!("{} queued", self.queue.len())).color(theme::CYAN()).small()); }
                        }
                        // collapsed in from the old top bar: checkpoint · history · tree · export
                        ui.add_space(10.0);
                        if ui.add_enabled(!self.msgs.is_empty(), egui::Button::new(RichText::new(uikit_egui::icons::copy_label(ui.ctx())).small()))
                            .on_hover_text("Export the chat as Markdown").clicked() { do_export = true; }
                        if self.vault.is_some() {
                            let n = self.vault.as_ref().map(|v| v.all_ids().len()).unwrap_or(0);
                            if ui.button(RichText::new(format!("🌲 {n}")).small())
                                .on_hover_text("Project history tree — return to any point to branch a new line").clicked() { self.show_history = !self.show_history; }
                        }
                        if !self.checkpoints.is_empty() {
                            if ui.button(RichText::new(format!("🕘 {}", self.checkpoints.len())).small())
                                .on_hover_text("Checkpoint history (rollback)").clicked() { self.show_checkpoints = !self.show_checkpoints; }
                        }
                        // THINKER WATCH, LEFT of the metrics toggle: what the big model is writing now, condensed,
                        // plus a forecast inferred from the text's STRUCTURE. Shown only while generating — a strip
                        // left beside a finished reply reads as current.
                        if self.stream.is_some() {
                            let w = logscope::thinkerwatch::Watch {
                                gist: self.watch_gist.clone(),
                                forecast: logscope::thinkerwatch::forecast(&self.watch_full),
                                words_so_far: self.watch.total_words(),
                            };
                            ui.label(RichText::new(w.line()).color(theme::DIM()).small())
                                .on_hover_text("What the Thinker is writing now. The tilde clause is INFERRED from the \
text's structure — an unclosed fence, an open brace — not predicted by a model.");
                            ui.add_space(6.0);
                        }
                        if ui.button(RichText::new("📊").small())
                            .on_hover_text("GPU phase breakdown: where the last generation's time went")
                            .clicked() { self.show_metrics = !self.show_metrics; }
                        if ui.add_enabled(!self.msgs.is_empty(), egui::Button::new(RichText::new("📌").small()))
                            .on_hover_text("Save a restore point for this conversation").clicked() { do_checkpoint = true; }
                    });
                });
                if enter { do_send = true; self.pearl_input.pulse(); } // send() queues it if the model is busy
            });
            theme::carved_trough(ui.painter(), composer_frame.response.rect, ui.input(|i| i.time) as f32);
        });
        }

        let mut close_fan = false;
        // ── the replicator fan SPLITS the chat area, when one is open ──
        // Panel before CentralPanel: the fan claims a column beside the log, both stopping above
        // the composer, so the operator reads the main thread and the workers side by side and
        // types into one box. `replicatorlayout::split` decides whether that is honest at this
        // width — on a phone it is not (two 190pt ribbons of prose are worse than either alone),
        // and Stacked shows the fan in place of the log with a way back.
        if self.replicator.is_some() {
            let window_w = ui.available_width();
            let n = self.replicator.as_ref().map(|m| m.workers.len()).unwrap_or(1);
            match crate::replicatorlayout::split(window_w, n) {
                crate::replicatorlayout::Split::Side { fan_pt } => {
                    egui::Panel::right("replicator_fan")
                        .resizable(true)
                        .default_size(fan_pt)
                        // The clamp is the container's, not the content's: dragging the splitter
                        // can starve the log but never past its floor, and cannot widen the fan
                        // past the window.
                        .size_range(crate::replicatorlayout::MIN_COL_PT
                            ..=(window_w - crate::replicatorlayout::MIN_LOG_PT)
                                .max(crate::replicatorlayout::MIN_COL_PT))
                        .show_inside(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("⑃ fan").color(theme::accent()).strong());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.small_button("✕").on_hover_text("Close replicator mode").clicked() {
                                        close_fan = true;
                                    }
                                });
                            });
                            self.draw_replicator(ui);
                        });
                }
                crate::replicatorlayout::Split::Stacked => {
                    // Too narrow to split. One at a time, with the toggle stating which is showing
                    // — a fan drawn at 190pt per column is not a view, it is a stack of ribbons.
                    ui.horizontal_wrapped(|ui| {
                        if ui.selectable_label(self.fan_fullscreen, RichText::new("⑃ fan").small()).clicked() {
                            self.fan_fullscreen = true;
                        }
                        if ui.selectable_label(!self.fan_fullscreen, RichText::new("chat").small()).clicked() {
                            self.fan_fullscreen = false;
                        }
                        if ui.small_button("✕").on_hover_text("Close replicator mode").clicked() { close_fan = true; }
                    });
                    if self.fan_fullscreen {
                        egui::CentralPanel::default().show_inside(ui, |ui| { self.draw_replicator(ui); });
                    }
                }
            }
        }
        if close_fan { self.replicator = None; self.note = "replicator: closed".into(); }

        // ── center: the message log fills whatever height remains above the pinned bottom ──
        if !(self.replicator.is_some() && self.fan_fullscreen
             && matches!(crate::replicatorlayout::split(ui.available_width(),
                         self.replicator.as_ref().map(|m| m.workers.len()).unwrap_or(1)),
                         crate::replicatorlayout::Split::Stacked)) {
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if self.spatial_view {
                let want_send = self.spatial_ui(ui, Some(model));
                if want_send { self.send(infer, doc, ui); }
                return;
            } // aggressive: spatial graph replaces the linear log
            egui::ScrollArea::vertical().id_salt("chat_log").auto_shrink([false, false]).stick_to_bottom(true)
                .show(ui, |ui| {
                    // CONTEXT IS FULL. The alternative is the default: shrink this reply, and the
                    // next, and the one after that, without saying so. One session's budget walked
                    // 2470 → 814 tokens over seventeen turns that way.
                    if let Some(req) = self.compact_offer.clone() {
                        egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(10), inner_margin: egui::Margin::same(theme::PAD_MD),
                            outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() }, stroke: egui::Stroke::new(1.0_f32, theme::AMBER()), ..Default::default() }
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("🗜").color(theme::AMBER()).strong());
                                ui.label(RichText::new(format!(
                                    "The context is full: replies from here would be cut short. Fold the older \
turns into a briefing (~{} tokens of history) and keep the most recent {} verbatim?",
                                    req.summarize_upto, req.keep_recent)).color(theme::INK()));
                            });
                            ui.horizontal_wrapped(|ui| {
                                if theme::action_button(ui, "Summarize & continue", theme::AMBER(), true)
                                    .on_hover_text("Stages the :compact teacher turn — fold the older turns into a \
dense briefing that replaces them as the working context, then carry on with room to answer").clicked() {
                                    do_compact = true;
                                }
                                if ui.button("Keep squeezing").on_hover_text(
                                    "Shrink this reply instead. The offer returns if it gets worse.").clicked() {
                                    self.compact_declined = req.seq;
                                    self.compact_offer = None;
                                }
                            });
                        });
                    }
                    if let Some(n) = self.recover {
                        egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(10), inner_margin: egui::Margin::same(theme::PAD_MD),
                            outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() }, stroke: egui::Stroke::new(1.0_f32, theme::CYAN()), ..Default::default() }
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("⟲").color(theme::CYAN()).strong());
                                ui.label(RichText::new(format!("A previous conversation ({n} messages) was found.")).color(theme::INK()));
                                if theme::action_button(ui, "Recover", theme::CYAN(), true).clicked() { do_restore = true; }
                                if ui.button("Dismiss").clicked() { self.recover = None; }
                            });
                        });
                    }
                    if self.msgs.is_empty() {
                        ui.add_space(16.0);
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("No messages yet").color(theme::DIM()));
                            ui.label(RichText::new("Type below to start. Toggle Thread for multi-turn, or attach a file with 📎.").color(theme::FAINT()).small());
                        });
                    }
                    let cur = self.stream.as_ref().map(|s| s.idx);
                    let streaming_any = cur.is_some();
                    let nmsgs = self.msgs.len();
                    let scroll_to = self.scroll_chat_to.take(); // jump-scroll target from the graph
                    let msg_count = self.msgs.len();
                    // ROW DESCRIPTORS, built once per frame from a borrow of the messages that ends
                    // here. The paint reads these instead of deciding per-message, so the collapse
                    // decision and its label are computed in one place and the renderer stops needing
                    // to know the rules. Summaries are computed only for rows that actually collapse.
                    let rows = logscope::msgview::build_rows(
                        &MsgSlice(&self.msgs), 0..msg_count, &self.expanded_msgs,
                        logscope::liaison_relay::COLLAPSE_ABOVE_LINES);
                    let expanded_now = self.expanded_msgs.clone();
                    let step_open = self.step_open.clone();
                    let mut toggle_step: Option<(usize, usize)> = None;
                    let mut swipe_variant: Option<(usize, i32)> = None;
                    let mut over_any_message = false;
                    let mut expand_msg: Option<usize> = None;
                    let mut collapse_msg: Option<usize> = None;
                    for (i, m) in self.msgs.iter_mut().enumerate() {
                        if Some(i) == scroll_to { ui.scroll_to_cursor(Some(egui::Align::Center)); }
                        let (who, c, fill) = match m.role {
                            Role::User => ("you", theme::ensure_contrast(theme::AMBER(), theme::PANEL2(), 4.5), theme::PANEL2()),
                            Role::Assistant => ("assistant", theme::ensure_contrast(theme::GREEN(), theme::PANEL(), 4.5), theme::PANEL()),
                        };
                        let a = theme::appear(m.born, 0.18);
                        if a < 1.0 { ui.ctx().request_repaint(); }
                        let streaming_here = cur == Some(i);
                        // SWIPE BETWEEN REGENERATIONS. A horizontal drag on a message steps its
                        // variants, the way a photo gallery does — which takes the arrow buttons out of
                        // a bar that was already overlapping rather than adding to it. The axis lock and
                        // the mid-stream refusal live in chatgesture; this only supplies the drag.
                        //
                        // CUMULATIVE distance since the press, not the per-frame delta. A per-frame
                        // delta is typically a couple of points even for a fast drag — comparing THAT
                        // against a 12pt threshold means the gesture only ever fired on a hard flick
                        // that happened to move >12pt within one frame, not an ordinary slow swipe.
                        // `total_drag_delta()` is the whole drag from press_origin to now, recomputed
                        // fresh each frame, so a slow deliberate swipe crosses the threshold exactly
                        // when it visually should. `swipe_consumed` stops it firing again on every
                        // later frame of the SAME drag once past that threshold.
                        if m.variants.len() > 1 && !streaming_here {
                            if ui.rect_contains_pointer(ui.min_rect())
                                && ui.input(|i| i.pointer.primary_down()) {
                                over_any_message = true;
                                // Same widget-ownership check as the background
                                // swipe below. A message bubble can contain a
                                // selectable label or a code block being
                                // scrolled; neither is a variant swipe.
                                let widget_owns = uikit_egui::gesture_layer::drag_owner(
                                    ui.ctx(), ui.min_rect()) == uikit_egui::gesture_layer::Owner::Widget;
                                if !self.swipe_consumed && !widget_owns {
                                    let d = ui.input(|i| i.pointer.total_drag_delta()).unwrap_or_default();
                                    let ax = crate::chatgesture::lock_axis(d.x, d.y);
                                    let delta = if d.x < -crate::chatgesture::AXIS_LOCK_PT { 1 }
                                                else if d.x > crate::chatgesture::AXIS_LOCK_PT { -1 }
                                                else { 0 };
                                    if let crate::chatgesture::Action::ShowVariant { delta, .. } =
                                        crate::chatgesture::action(
                                            crate::chatgesture::Target::Message(i), delta, ax,
                                            streaming_here, m.variants.len(), m.cur) {
                                        swipe_variant = Some((i, delta));
                                        self.swipe_consumed = true;
                                    }
                                }
                            }
                        } else if ui.input(|i| i.pointer.primary_down()) && ui.rect_contains_pointer(ui.min_rect()) {
                            // A single-variant (or streaming) message still counts as "on a message"
                            // for the purposes of the background swipe below — chatgesture's own
                            // ambiguous-target failure mode (#4) is exactly a drag over a message
                            // being read as a background gesture because nothing claimed it.
                            over_any_message = true;
                        }
                        theme::fade(ui, a, |ui| {
                            egui::Frame { fill, corner_radius: theme::r(10),
                                inner_margin: egui::Margin::symmetric(theme::PAD_MD, theme::PAD_SM + 2),
                                outer_margin: egui::Margin { bottom: theme::PAD_SM, ..Default::default() },
                                stroke: egui::Stroke::new(1.0_f32, theme::LINE()), ..Default::default() }
                            .show(ui, |ui| {
                                // header: role · branch pager · controls
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(who).color(c).small().strong());
                                    // WHEN, beside WHO. Both clocks were already on `Msg` — `born`
                                    // with a doc comment saying it exists to measure "3s ago" — and
                                    // neither reached the screen: the only timestamps in the app
                                    // were in the exported transcript. See `turn_age` for which
                                    // clock is trusted and why a reopened session gets nothing
                                    // rather than a fabricated one.
                                    let now_unix = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map(|d| d.as_secs()).unwrap_or(0);
                                    if let Some(age) = turn_age(m.at, m.born, now_unix) {
                                        ui.label(RichText::new(age).color(theme::FAINT()).small());
                                    }
                                    // node id — highlighted so it can be referenced in :mergectx (git-style prefix)
                                    ui.label(RichText::new(crate::vimmode::short_id(i)).monospace().small().color(theme::md_run_violet()))
                                        .on_hover_text("node id — reference it in :mergectx (a unique prefix works, e.g. :mergectx a9j,3df)");
                                    if m.variants.len() > 1 {
                                        if ui.small_button("‹").on_hover_text("Previous branch").clicked() { action = Some(MsgAction::PrevVar(i)); }
                                        ui.label(RichText::new(format!("{}/{}", m.cur + 1, m.variants.len())).color(theme::FAINT()).small());
                                        if ui.small_button("›").on_hover_text("Next branch").clicked() { action = Some(MsgAction::NextVar(i)); }
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if !streaming_any && !m.editing {
                                            if ui.small_button("🗑").on_hover_text("Delete from here down").clicked() { action = Some(MsgAction::Delete(i)); }
                                            if !m.text().is_empty() && ui.small_button(uikit_egui::icons::copy_label(ui.ctx())).on_hover_text("Copy message").clicked() { action = Some(MsgAction::Copy(i)); }
                                            if m.role == Role::User && ui.small_button("✎").on_hover_text("Edit & re-run from here").clicked() { action = Some(MsgAction::EditStart(i)); }
                                            if m.role == Role::Assistant && i + 1 >= nmsgs && ui.small_button("↻").on_hover_text("Reroll (keeps the old as a branch)").clicked() { action = Some(MsgAction::Regenerate(i)); }
                                        }
                                    });
                                });
                                ui.add_space(2.0);
                                if m.editing {
                                    ui.add(egui::TextEdit::multiline(&mut m.edit_buf).id(egui::Id::new(("logscope_chat_edit", i))).desired_rows(2).desired_width(f32::INFINITY));
                                    ui.horizontal_wrapped(|ui| {
                                        if theme::action_button(ui, "Save & re-run", theme::GREEN(), true).clicked() { action = Some(MsgAction::EditSave(i)); }
                                        if ui.button("Cancel").clicked() { action = Some(MsgAction::EditCancel(i)); }
                                    });
                                } else {
                                    // CLIFFSNOTES COLLAPSE. A long assistant reply is relevant for one turn and is scenery
                                    // afterwards, so it folds once it is no longer the latest message. Only when BOTH long and
                                    // not-latest: collapsing what someone is currently reading is hostile, and folding a
                                    // three-line answer saves nothing.
                                    let row = rows.get(i);
                                    let body_lines = row.map(|r| r.lines).unwrap_or(0);
                                    let collapsible = m.role == Role::Assistant
                                        && body_lines > logscope::liaison_relay::COLLAPSE_ABOVE_LINES
                                        && i + 1 != msg_count;
                                    let expanded = expanded_now.contains(&i);
                                    if let Some(r) = row.filter(|r| r.collapsed) {
                                        // The label was computed when the row was built; the paint just
                                        // draws it. No parse, no summary, no clone at draw time.
                                        if ui.button(RichText::new(r.label.clone()).color(theme::DIM()).small()).clicked() {
                                            expand_msg = Some(i);
                                        }
                                        return;
                                    }
                                    if collapsible && expanded {
                                        if ui.button(RichText::new("\u{25be} collapse").color(theme::FAINT()).small()).clicked() {
                                            collapse_msg = Some(i);
                                        }
                                    }
                                    // STEPWISE OUTLINE. A coordinator reply carries "STEP k/n — summary" markers, which are a table
                                    // of contents the reader was scrolling past. Rendered as headers with the long middles folded,
                                    // the verbosity stops being a cost — which matters because these replies arrive FAST, so the
                                    // problem was never the tokens, only having to scroll through them to find the one that counts.
                                    let steps = logscope::stepview::split_steps(m.text());
                                    if steps.len() > 1 {
                                        for (si2, st) in steps.iter().enumerate() {
                                            let folded = logscope::stepview::fold_by_default(&steps, si2)
                                                && !step_open.contains(&(i, si2));
                                            if !st.summary.is_empty() {
                                                let mark = if folded { "\u{25b8}" } else { "\u{25be}" };
                                                if ui.button(RichText::new(format!("{mark} {}", st.header()))
                                                    .color(theme::DIM()).small()).clicked() { toggle_step = Some((i, si2)); }
                                            }
                                            if !folded { render_markdown(ui, &st.body, ""); }
                                        }
                                        return;
                                    }
                                    // BOUND THE MEASURE. Nothing capped prose width, so on a wide window a line ran past 200
                                    // characters against a comfortable maximum of 80. The failure is not illegibility — it is
                                    // the RETURN SWEEP: the eye travels back across the whole width and lands on the wrong line
                                    // often enough to be exhausting over an hour. Nothing looks broken at any width, so it never
                                    // presents as a bug; it presents as the reader being tired.
                                    //
                                    // Code is deliberately NOT bounded — a wrapped line of code loses its indentation as
                                    // structure, so it takes the full width and scrolls instead.
                                    // Measured from a laid-out glyph rather than assumed: the same point
                                // size gives very different measures in a condensed face and a wide one.
                                let fid = egui::TextStyle::Body.resolve(ui.style());
                                let char_px = ui.painter()
                                    .layout_no_wrap("0".into(), fid, theme::INK()).size().x;
                                    let prose_w = crate::readability::prose_width(ui.available_width(), char_px);
                                    ui.set_max_width(prose_w.max(120.0));
                                    // SECTION-PACED REVEAL. Token-by-token was proof of life when generation was slow; above
                                    // reading speed it is a jittering edge and half-formed words. revealpace holds the tail back
                                    // to the last semantic boundary — but only when the model is FAST, because on a slow one a
                                    // still screen reads as a hang, which is exactly when someone is deciding if anything works.
                                    //
                                    // The flush timeout bounds a boundary that never arrives (a long code block), and completion
                                    // always reveals everything, so a held tail can never become text that never appears.
                                    let shown = if streaming_here {
                                        let pace = crate::revealpace::pace(self.last_tok_per_s);
                                        let held = self.reveal_held_ms;
                                        let n = crate::revealpace::reveal_upto(m.text(), pace, held, false);
                    // Reset the clock whenever the boundary MOVED: the tail has only been withheld
                    // since the last thing the reader actually saw, not since the turn began.
                    if n != self.reveal_shown_len { self.reveal_held_ms = 0; self.reveal_shown_len = n; }
                                        &m.text()[..n.min(m.text().len())]
                                    } else { m.text() };
                                    let segs = segment_message(shown);
                                    let last = segs.len().saturating_sub(1);
                                    let think_open = m.think_open;
                                    let mut toggle_think = false;
                                    for (si, seg) in segs.iter().enumerate() {
                                        let tail = if streaming_here && si == last { "▌" } else { "" };
                                        match seg {
                                            Seg::Prose(p) => { render_markdown(ui, p, tail); }
                                            Seg::Think(t) => {
                                                let open = ui.ctx().animate_bool(egui::Id::new((i, si, "think")), think_open);
                                                let arrow = if think_open { "▾" } else { "▸" };
                                                if ui.add(egui::Button::new(RichText::new(format!("{arrow} thinking")).color(theme::FAINT()).small().italics()).frame(false)).clicked() { toggle_think = true; }
                                                if open > 0.0 {
                                                    theme::fade(ui, open, |ui| {
                                                        egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6), inner_margin: egui::Margin::same(theme::PAD_SM),
                                                            stroke: egui::Stroke::new(1.0_f32, theme::LINE()), ..Default::default() }
                                                        .show(ui, |ui| { ui.label(RichText::new(format!("{t}{tail}")).color(theme::DIM()).italics()); });
                                                    });
                                                }
                                            }
                                            Seg::Code { lang, body } => {
                                                // ── file summary toggle (gitmind integration) ──
                                                // Blocks naming a file get a [✨ Summary | Raw] switch. Files inside the
                                                // loaded .gitmind session's /tmp workspace DEFAULT to summary — the
                                                // session's own files are the ones you want described, not re-scrolled.
                                                // Summary = the instant structural digest, upgraded in place by the
                                                // small-model summary once the ollama sidecar delivers it (cached by
                                                // content hash; see filesummary.rs).
                                                let fpath = crate::filesummary::code_block_path(lang);
                                                let in_session = fpath.as_deref().map(|p| {
                                                    self.gitmind_session.as_ref().is_some_and(|s| crate::filesummary::is_session_file(p, s.work_dir()))
                                                }).unwrap_or(false);
                                                let sum_id = egui::Id::new(("codesum", i, si));
                                                let mut show_summary = fpath.is_some()
                                                    && ui.ctx().data_mut(|d| *d.get_temp_mut_or(sum_id, in_session));
                                                egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6),
                                                    inner_margin: egui::Margin::same(theme::PAD_SM), stroke: egui::Stroke::new(1.0_f32, theme::LINE()), ..Default::default() }
                                                .show(ui, |ui| {
                                                    ui.horizontal(|ui| {
                                                        let l = lang.split_whitespace().next().unwrap_or("");
                                                        if !l.is_empty() { ui.label(RichText::new(l).color(theme::md_accent()).small().monospace()); }
                                                        if let Some(p) = &fpath {
                                                            let mut sw = show_summary;
                                                            if ui.selectable_label(sw, RichText::new("\u{2728} Summary").small()).on_hover_text("Describe this file instead of showing the code").clicked() { sw = true; }
                                                            if ui.selectable_label(!sw, RichText::new("Raw").small()).clicked() { sw = false; }
                                                            if sw != show_summary {
                                                                show_summary = sw;
                                                                ui.ctx().data_mut(|d| d.insert_temp(sum_id, sw));
                                                            }
                                                            if in_session { ui.label(RichText::new("session").color(theme::md_dim()).small()).on_hover_text(format!("{p} is in the gitmind /tmp workspace")); }
                                                        }
                                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                            if ui.small_button("Copy").on_hover_text("Copy this block").clicked() { ui.ctx().copy_text(body.clone()); }
                                                        });
                                                    });
                                                    if show_summary {
                                                        let p = fpath.as_deref().unwrap_or("");
                                                        let key = crate::filesummary::content_key(p, body);
                                                        crate::filesummary::request_model_summary(key, p, body);
                                                        match crate::filesummary::model_summary(key) {
                                                            Some(text) => { render_markdown(ui, &text, ""); }
                                                            None => {
                                                                render_markdown(ui, &crate::filesummary::structural_digest(p, body), "");
                                                                let note = if std::env::var("LOGSCOPE_OLLAMA").is_ok() && crate::filesummary::in_flight(key) {
                                                                    "\u{2728} model summary generating…"
                                                                } else {
                                                                    "structural digest (set LOGSCOPE_OLLAMA for model summaries)"
                                                                };
                                                                ui.label(RichText::new(note).color(theme::FAINT()).small().italics());
                                                                if crate::filesummary::in_flight(key) { ui.ctx().request_repaint_after(std::time::Duration::from_millis(500)); }
                                                            }
                                                        }
                                                    } else {
                                                        ui.label(RichText::new(format!("{body}{tail}")).monospace().color(theme::md_ink()));
                                                    }
                                                });
                                            }
                                        }
                                    }
                                    if toggle_think { m.think_open = !m.think_open; }
                                    if m.text().is_empty() && streaming_here { ui.label(RichText::new("▌").color(theme::INK())); }
                                }
                            });
                        });
                    }
                    // Apply the chevron clicks here: the render closure holds `self.msgs` mutably, so it
                    // records the intent and the mutation happens once the borrow ends.
                    // Applied through the view model, so the expansion set has one mutation path and
                    // the caller learns whether anything actually moved.
                    let mut acts = Vec::new();
                    if let Some(i) = expand_msg { acts.push(logscope::msgview::RowAction::Expand(i)); }
                    if let Some(i) = collapse_msg { acts.push(logscope::msgview::RowAction::Collapse(i)); }
                    if logscope::msgview::apply_actions(&mut self.expanded_msgs, &acts) {
                        ui.ctx().request_repaint();
                    }
                    // ── LIAISON HUD: the running backend account, under the newest message ──
                    //
                    // Only while generating: after a reply lands, a status strip is stale and competes
                    // with the answer for attention. Every line originates in an engine observation and
                    // carries its source; when there is nothing observed the strip is absent entirely,
                    // because a fabricated status looks mechanical and is therefore believed.
                    if cur.is_some() {
                        if let Some((line, narrated)) = self.hud.display(Some(&self.hud_narration)) {
                            ui.add_space(theme::PAD_XS as f32);
                            egui::Frame { fill: theme::PANEL2(), corner_radius: theme::r(6),
                                inner_margin: egui::Margin::symmetric(theme::PAD_SM, theme::PAD_XS),
                                stroke: egui::Stroke::new(1.0_f32, theme::LINE()), ..Default::default() }
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    // the dot distinguishes a liaison SENTENCE from a raw engine line, so
                                    // a reader always knows whether a model phrased it
                                    // shape AND colour: the glyph already distinguishes narrated from
                                    // raw, so a reader who cannot separate the hues still can
                                    ui.label(RichText::new(if narrated { "◈" } else { "·" })
                                        .color(if narrated { theme::CYAN() } else { theme::FAINT() }).small());
                                    ui.label(RichText::new(&line).color(theme::DIM()).small());
                                });
                                if let Some(src) = self.hud.latest().filter(|s| s.is_grounded()) {
                                    ui.label(RichText::new(format!("via {}", src.grounds.join(", ")))
                                        .color(theme::FAINT()).small());
                                }
                            });
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
                        }
                    }
                    // APPLY THE SWIPE. This was computed and discarded, so the gesture has done nothing since
                    // _368 — the model layer was correct, tested, and never consulted.
                    //
                    // chatgesture::action already refused mid-stream swipes and single-variant messages, so
                    // reaching here means the change is allowed. The clamp is the second half of that contract:
                    // CLAMP, not wrap, because wrapping from the last regeneration back to the first reads as
                    // having lost the newest one.
                    if let Some((i, d)) = swipe_variant {
                        let n = self.msgs[i].variants.len();
                        self.msgs[i].cur = (self.msgs[i].cur as i32 + d).clamp(0, n as i32 - 1) as usize;
                    }
                    // SWIPE ON THE BACKGROUND — chatgesture's other target. A drag that landed on no
                    // message at all (over_any_message tracked per-row above, so this can't fire from
                    // a pixel inside a message bubble that merely had one variant) steps between
                    // sibling branches instead, the same "hjkl branch" the header hints at, now
                    // reachable without the keyboard. `switch_sibling` already exists and is wired to
                    // h/l and the graph tab; chatgesture::action's Target::Background case had no
                    // caller until this. Same cumulative-distance + one-shot-latch reasoning as the
                    // message swipe above.
                    // A drag a WIDGET owns is not a page gesture. Without this,
                    // a finger selecting text in the composer, dragging a
                    // slider, or touch-scrolling a list reads as a background
                    // swipe and switches branch under the user — and on a touch
                    // device it competes for the press that would give a text
                    // field focus, which is the leading suspect for the
                    // "fields take one character on the Fairphone" report.
                    //
                    // `drag_owner` is the shared answer (uikit-egui), added with
                    // the snapclone merge and until now used only by `pager` and
                    // `touch`, neither of which logscope goes through. Calling it
                    // here rather than re-deriving the check is the point: a
                    // second copy is how these diverge.
                    let widget_owns = uikit_egui::gesture_layer::drag_owner(
                        ui.ctx(), ui.min_rect()) == uikit_egui::gesture_layer::Owner::Widget;
                    if !over_any_message && !self.swipe_consumed && !widget_owns
                        && ui.rect_contains_pointer(ui.min_rect()) && ui.input(|i| i.pointer.primary_down()) {
                        let d = ui.input(|i| i.pointer.total_drag_delta()).unwrap_or_default();
                        let ax = crate::chatgesture::lock_axis(d.x, d.y);
                        let delta = if d.x < -crate::chatgesture::AXIS_LOCK_PT { 1 }
                                    else if d.x > crate::chatgesture::AXIS_LOCK_PT { -1 } else { 0 };
                        if let crate::chatgesture::Action::SwitchBranch { delta } =
                            crate::chatgesture::action(crate::chatgesture::Target::Background, delta, ax, false, 0, 0) {
                            self.switch_sibling(delta);
                            self.swipe_consumed = true;
                        }
                    }
                    // Latch resets the instant the button lifts, whether or not this drag fired
                    // anything — the next press starts a fresh gesture with its own drag distance.
                    if !ui.input(|i| i.pointer.primary_down()) { self.swipe_consumed = false; }
                });
        });
        } // narrow + fan-fullscreen: the fan took the central area instead of the log

        // ── picker modals + their outcomes ──
        if let Some(crate::filepicker::Outcome::Picked(dir)) = self.save_picker.ui(ui.ctx()) {
            self.save_to(&arts, dir);
        }
        if let Some(crate::filepicker::Outcome::Picked(file)) = self.attach_picker.ui(ui.ctx()) {
            self.attach(&file);
        }
        if let Some(act) = action {
            let ctx = ui.ctx().clone();
            self.apply_msg_action(&ctx, act, infer, doc);
        }
        if do_send { self.send(infer, doc, ui); }
        // Drain one queued (branch) turn as soon as the model is free — serial, but non-blocking from
        // the user's side: they fired it mid-generation and it runs the moment the current reply ends.
        if self.stream.is_none() && self.agent_run.is_none() && self.agent_fan.is_none() {
            // Between rounds of a depth fan: once this round's generations have all finished and drained,
            // fire the next round on the NEW frontier (this is what makes :Nca / :Nfa go N deep).
            if self.queue.is_empty() && self.fan.is_some() { self.fan_step(); }
            if let Some((qt, parent)) = self.queue.pop_front() {
                if qt == CONTINUE_ALL_TOK {
                    if let Some(l) = parent { self.expand_leaf(l, infer, doc); }
                } else {
                    self.start_generation(qt, parent, infer, doc);
                }
                ui.ctx().request_repaint();
            }
        }
        if do_continue { self.continue_generation(infer, doc); }
        // Stepwise auto-continue: resume the paused reply on the user's behalf (warm-KV, one per pause).
        // If a cargo-test run is in flight for this pause, wait for its result and splice it in first.
        if self.step_arm_continue && self.stream.is_none() {
            if let Some(rx) = &self.step_shell {
                match rx.try_recv() {
                    Ok(msg) => { self.step_inject = Some(msg); self.step_shell = None; }
                    Err(std::sync::mpsc::TryRecvError::Empty) => { ui.ctx().request_repaint(); return; } // keep waiting
                    Err(_) => { self.step_shell = None; }
                }
            }
            self.step_arm_continue = false;
            if let Some(msg) = self.step_inject.take() {
                if let Some(m) = self.msgs.last_mut() {
                    m.cur_mut().push_str(&format!("\n\n{msg}\n"));
                }
            }
            self.continue_generation(infer, doc);
        }
        // Accepting the offer runs the SAME `:compact` the operator could type, with the fold sized
        // from the engine's request instead of the command's default ratio — one folding path, two
        // ways in. A checkpoint first, because a compaction rewrites the working context and the
        // operator's next thought may be that it dropped something.
        if do_compact {
            let req = self.compact_offer.take();
            self.checkpoint("before compaction");
            let mut chain = Vec::new();
            let mut cur = self.active_leaf;
            while let Some(i) = cur { chain.push(i); cur = self.msgs.get(i).and_then(|m| m.parent); }
            chain.reverse();
            let turns: Vec<(String, String)> = chain.iter().filter_map(|&i| self.msgs.get(i))
                .map(|m| ((if m.role == Role::Assistant { "assistant" } else { "user" }).to_string(),
                          m.text().to_string()))
                .collect();
            if turns.len() < 2 {
                self.note = "compaction: need at least two turns of history to fold".into();
            } else {
                let keep_tokens = req.map(|r| r.keep_recent).unwrap_or(1024);
                let (keep, ratio) = crate::refresh::plan_for_request(
                    &turns, keep_tokens, self.ctx.n_ctx as usize,
                    crate::refresh::REPLY_FLOOR);
                let plan = crate::refresh::plan(&turns, keep, ratio);
                // Same guard as the :compact path. The engine asking for room
                // does not make a pointless fold worth doing — if there is too
                // little history to recover, say so rather than staging a
                // generation that cannot help.
                if let Some(why) = plan.refusal() { self.note = why; }
                else {
                let folded: Vec<(String, String)> = turns[..plan.fold].to_vec();
                self.input = crate::refresh::teacher_turn(&folded, plan.target_tokens);
                self.note = format!(
                    "🗜 compaction staged from the engine's request: fold {} turns → ~{} tok, keep {} \
recent → context {}→{} tok (−{:.0}%). Send it; the briefing becomes the working context.",
                    plan.fold, plan.target_tokens, plan.keep, plan.before_tokens, plan.after_tokens,
                    plan.ratio_saved() * 100.0);
                }
            }
        }
        if do_checkpoint { self.checkpoint(format!("manual · {} msgs", self.msgs.len())); }
        if do_export {
            let note = match self.export_chat(ui.ctx()) {
                Some(p) => format!("Copied to clipboard · saved {}", p.display()),
                None => "Copied chat to clipboard".to_string(),
            };
            self.ws_note = Some((note, Instant::now()));
        }
        if do_restore { self.restore_autosave(); }
        if let Some(i) = rollback { self.rollback_to(i); }
        if let Some(want) = raise_ctx { self.ctx.n_ctx = want.min(32768); }
    }
}
