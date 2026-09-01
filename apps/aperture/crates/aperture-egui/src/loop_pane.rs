//! loop_pane — F2. The completion loop with its case off.
//!
//! Everything the engine did is a row (generation, provider, latency, first
//! line of the answer or the error), and everything it *will* do is a field
//! you can edit — host, model, token budget, or a whole different provider —
//! applied live via `Engine::swap_provider`, taking effect on the next
//! request. No config file round-trip, no restart, no mystery: the pane IS
//! the contract that this editor's AI is a visible, steerable machine.

use crate::state::{AppState, ProviderKind};
use egui::{RichText, Ui};
use uikit_egui::theme_bridge::PaletteColors;

pub fn show(ui: &mut Ui, state: &mut AppState, colors: &PaletteColors, _now_ms: u64) {
    ui.add_space(6.0);
    ui.label(RichText::new("completion loop").color(colors.text).strong());
    ui.label(RichText::new("newest request wins; stale outcomes are logged, never shown").color(colors.text_weak).small());
    ui.add_space(6.0);

    let mut changed = false;
    egui::ComboBox::from_label("provider")
        .selected_text(match state.provider_kind {
            ProviderKind::Ollama => "ollama",
            ProviderKind::Logscope => "logscope",
            ProviderKind::Engine => "engine (in-process)",
            ProviderKind::Command => "command",
            ProviderKind::Demo => "demo (offline)",
        })
        .show_ui(ui, |ui| {
            changed |= ui.selectable_value(&mut state.provider_kind, ProviderKind::Ollama, "ollama").changed();
            changed |= ui.selectable_value(&mut state.provider_kind, ProviderKind::Logscope, "logscope").changed();
            changed |= ui.selectable_value(&mut state.provider_kind, ProviderKind::Engine, "engine (in-process)").changed();
            changed |= ui.selectable_value(&mut state.provider_kind, ProviderKind::Command, "command").changed();
            changed |= ui.selectable_value(&mut state.provider_kind, ProviderKind::Demo, "demo (offline)").changed();
        });

    match state.provider_kind {
        ProviderKind::Engine => {
            // THE MODEL SELECTOR. The in-process engine's whole
            // configuration starts with "which checkpoint", so that is the
            // first control, and it lists what is actually on disk rather
            // than asking someone to type a path from memory.
            if state.engine_models.is_empty() {
                state.rescan_models();
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("model").color(colors.text_weak));
                let selected = if state.engine_cfg.model_path.is_empty() {
                    "— pick a model —".to_string()
                } else {
                    std::path::Path::new(&state.engine_cfg.model_path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| state.engine_cfg.model_path.clone())
                };
                egui::ComboBox::from_id_salt("engine_model")
                    .selected_text(selected)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        if state.engine_models.is_empty() {
                            ui.label(
                                RichText::new("no .gguf found — set APERTURE_MODELS")
                                    .color(colors.text_weak)
                                    .small(),
                            );
                        }
                        for m in &state.engine_models {
                            let p = m.path.to_string_lossy().into_owned();
                            let label = format!("{}  ({})", m.label, m.size_label());
                            if ui
                                .selectable_label(state.engine_cfg.model_path == p, label)
                                .clicked()
                            {
                                state.engine_cfg.model_path = p;
                                changed = true;
                            }
                        }
                    });
                if ui.button("rescan").on_hover_text("re-read the model directories").clicked() {
                    state.rescan_models();
                }
            });
            ui.horizontal(|ui| {
                let busy = matches!(state.engine_status, crate::state::EngineStatus::Loading(_));
                let can_load = !state.engine_cfg.model_path.is_empty() && !busy;
                if ui
                    .add_enabled(can_load, egui::Button::new("load model"))
                    .on_hover_text("resident in THIS process — no server, no round-trip")
                    .clicked()
                {
                    state.load_engine_model();
                    changed = true;
                }
                ui.label(RichText::new(state.engine_status.line()).color(colors.text_weak).small());
            });
            let cfg = &mut state.engine_cfg;
            ui.horizontal(|ui| {
                ui.label(RichText::new("max tokens").color(colors.text_weak));
                changed |= ui.add(egui::DragValue::new(&mut cfg.max_tokens).range(1..=4096)).changed();
                ui.label(RichText::new("temp").color(colors.text_weak));
                changed |= ui
                    .add(egui::DragValue::new(&mut cfg.temperature).range(0.0..=2.0).speed(0.05))
                    .on_hover_text("0 = greedy, which is what completions want")
                    .changed();
                ui.label(RichText::new("repeat").color(colors.text_weak));
                changed |= ui
                    .add(egui::DragValue::new(&mut cfg.repeat_penalty).range(1.0..=2.0).speed(0.01))
                    .on_hover_text("discourages the model reciting your buffer back")
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("instruction").color(colors.text_weak));
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut cfg.instruction).id_salt("loop_pane-cfg_instruction").desired_width(f32::INFINITY))
                    .lost_focus();
            });
        }
        ProviderKind::Logscope => {
            // logscope-serve (apps/logscope/SERVING.md): POST /generate,
            // prompt as the raw body, key in the Bearer header. Every
            // knob that shapes the prompt lives HERE, and the preview
            // below is assembled by the SAME function the provider calls
            // — the pane shows exactly what the model will see.
            let cfg = &mut state.logscope_cfg;
            ui.horizontal(|ui| {
                ui.label(RichText::new("host").color(colors.text_weak));
                changed |= ui.text_edit_singleline(&mut cfg.host).lost_focus();
                ui.label(RichText::new("key").color(colors.text_weak));
                changed |= ui.add(egui::TextEdit::singleline(&mut cfg.key).id_salt("loop_pane-cfg_key").password(true).desired_width(120.0)).lost_focus();
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("max tokens").color(colors.text_weak));
                changed |= ui.add(egui::DragValue::new(&mut cfg.max_tokens).range(1..=8192)).changed();
                ui.label(RichText::new("fanout").color(colors.text_weak));
                changed |= ui
                    .add(egui::DragValue::new(&mut cfg.fanout).range(1..=8))
                    .on_hover_text("sample up to N times; first non-empty wins")
                    .changed();
            });
            ui.horizontal(|ui| {
                changed |= ui
                    .checkbox(&mut cfg.read_siblings, "read sibling files")
                    .on_hover_text("prepend the first 40 lines of neighboring files")
                    .changed();
                if cfg.read_siblings {
                    changed |= ui.add(egui::DragValue::new(&mut cfg.sibling_files).range(1..=8)).changed();
                }
                changed |= ui
                    .checkbox(&mut cfg.tool_calls, "tool calls")
                    .on_hover_text("one round: a reply starting `@read <path>` fetches that file and asks again")
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("instruction").color(colors.text_weak));
                changed |= ui.add(egui::TextEdit::singleline(&mut cfg.instruction).id_salt("loop_pane-cfg_instruction").desired_width(f32::INFINITY)).lost_focus();
            });
        }
        ProviderKind::Ollama => {
            ui.horizontal(|ui| {
                ui.label(RichText::new("host").color(colors.text_weak));
                changed |= ui.text_edit_singleline(&mut state.ollama_cfg.host).lost_focus();
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("model").color(colors.text_weak));
                changed |= ui.text_edit_singleline(&mut state.ollama_cfg.model).lost_focus();
            });
            let mut np = state.ollama_cfg.num_predict as u32;
            if ui.add(egui::Slider::new(&mut np, 16..=256).text("max tokens")).changed() {
                state.ollama_cfg.num_predict = np;
                changed = true;
            }
            let mut fim = state.ollama_cfg.use_suffix;
            if ui.checkbox(&mut fim, "fill-in-middle (suffix)").changed() {
                state.ollama_cfg.use_suffix = fim;
                changed = true;
            }
        }
        ProviderKind::Command => {
            ui.label(RichText::new("argv — stdin gets `prefix NUL suffix`, stdout is the completion").color(colors.text_weak).small());
            changed |= ui.text_edit_singleline(&mut state.command_line).lost_focus();
        }
        ProviderKind::Demo => {
            ui.label(
                RichText::new("built-in table: try typing `fn main` or `// TODO`")
                    .color(colors.text_weak)
                    .small(),
            );
        }
    }
    if changed {
        state.swap_provider();
    }

    ui.add_space(8.0);
    ui.separator();
    let armed = state.debounce.armed();
    let pending = state.ghost.is_pending();
    let phase = if pending {
        ("in flight", colors.warning)
    } else if armed {
        ("debouncing", colors.accent_soft)
    } else {
        ("idle", colors.text_weak)
    };
    ui.label(RichText::new(format!("state: {}", phase.0)).color(phase.1));
    ui.add_space(4.0);

    uikit_egui::scroll::area(ui, |ui| {
        for row in &state.loop_log {
            let tint = if row.ok { colors.success } else { colors.danger };
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("#{}", row.generation)).color(colors.text_weak).monospace());
                ui.label(RichText::new(format!("{}ms", row.latency_ms)).color(tint).monospace());
                ui.label(RichText::new(&row.provider).color(colors.text_weak).small());
            });
            ui.label(RichText::new(&row.brief).color(colors.text).monospace().small());
            ui.add_space(4.0);
        }
        if state.loop_log.is_empty() {
            ui.label(RichText::new("no traffic yet — pause after typing, or Ctrl+Space").color(colors.text_weak).small());
        }
    });
}
