//! First-run wizard / splash. Frames what Logscope is for and walks from the basics
//! (load a model, scan logs, chat) to the advanced vision (fleets of small workers
//! clearing many tasks in parallel). Reopen anytime with the `?` button in the header.
use crate::theme;
use egui::{Align2, Color32, RichText};

struct Step {
    title: &'static str,
    accent: Color32,
    lead: &'static str,
    body: &'static [&'static str],
}

fn steps() -> [Step; 5] {
    [
        Step {
            title: "Welcome to Logscope",
            accent: theme::AMBER(),
            lead: "The maintenance half of the workflow.",
            body: &[
                "Claude is for research, planning, and synthesizing a codebase from scratch.",
                "Logscope is the other half: keeping things running once the system is too big",
                "to hand a frontier model every time something goes sideways.",
                "Diagnose and fix from your phone or laptop, fast — even systems with no docs yet.",
            ],
        },
        Step {
            title: "What it's for",
            accent: theme::CYAN(),
            lead: "Two everyday shapes of \"something broke\".",
            body: &[
                "• Sometimes a log file quietly grows to 157 GB by accident and you just need",
                "  to spot it and add logrotate.",
                "• Sometimes you're sifting a bot-storm of synthesized data for the one real signal.",
                "Logscope ingests the logs, finds the pattern, and proposes the fix — locally.",
            ],
        },
        Step {
            title: "The basics",
            accent: theme::GREEN(),
            lead: "Three steps to your first answer.",
            body: &[
                "1. Point \"local gguf\" at a .gguf model and press Load.",
                "2. Add log sources under Sources, then Scan to ingest them.",
                "3. Open Chat and ask about what you're seeing.",
                "Backends: llama.cpp (fast, mature) or the clean-room wgpu engine (experimental).",
            ],
        },
        Step {
            title: "Going advanced — worker fleets",
            accent: theme::AMBER(),
            lead: "One model, many small workers.",
            body: &[
                "The wgpu engine loads the weights once and runs many sessions against them —",
                "spin up the second, tenth, hundredth instance for the cost of a KV cache, not a reload.",
                "Picture a hundred small workers clearing a hundred small tasks in an hour:",
                "triage, patches, log synthesis — in parallel. int8 KV keeps memory low;",
                "continuous batching keeps them all fed.",
            ],
        },
        Step {
            title: "These systems are new",
            accent: theme::CYAN(),
            lead: "No manual yet — so this is the manual.",
            body: &[
                "Tooling like this is emergent: there's little public folklore to lean on.",
                "This wizard is the orientation for now — reopen it anytime with ? in the header.",
                "Start small, get one fix landing cleanly, then scale the fleet out.",
                "Welcome aboard.",
            ],
        },
    ]
}

pub struct Wizard {
    pub open: bool,
    step: usize,
}
impl Default for Wizard {
    fn default() -> Self { Self { open: true, step: 0 } }
}
impl Wizard {
    pub fn reopen(&mut self) { self.open = true; self.step = 0; }

    pub fn ui(&mut self, ctx: &egui::Context) {
        if !self.open { return; }
        let steps = steps();
        let n = steps.len();
        if self.step >= n { self.step = n - 1; }
        let s = &steps[self.step];

        egui::Window::new("logscope_wizard")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .fixed_size([560.0, 0.0])
            .frame(egui::Frame {
                fill: theme::PANEL(),
                corner_radius: theme::r(14),
                inner_margin: egui::Margin::same(22),
                stroke: egui::Stroke::new(1.0, theme::LINE()),
                ..Default::default()
            })
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_width(516.0);
                // brand + step dots
                ui.horizontal(|ui| {
                    ui.label(RichText::new("LOGSCOPE").color(theme::AMBER()).strong().size(13.0));
                    ui.label(RichText::new("first-run guide").color(theme::FAINT()).small());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for i in (0..n).rev() {
                            let c = if i == self.step { s.accent } else { theme::FAINT() };
                            ui.label(RichText::new(if i == self.step { "●" } else { "○" }).color(c).small());
                        }
                    });
                });
                ui.add_space(10.0);
                ui.label(RichText::new(s.title).color(s.accent).strong().size(20.0));
                ui.add_space(2.0);
                ui.label(RichText::new(s.lead).color(theme::INK()).size(14.0));
                ui.add_space(10.0);
                for line in s.body {
                    ui.label(RichText::new(*line).color(theme::DIM()).size(13.0));
                }
                ui.add_space(16.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Skip").clicked() { self.open = false; }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let last = self.step + 1 == n;
                        if ui.button(if last { "Finish" } else { "Next ▶" }).clicked() {
                            if last { self.open = false; } else { self.step += 1; }
                        }
                        if self.step > 0 && ui.button("◀ Back").clicked() { self.step -= 1; }
                        ui.label(RichText::new(format!("{} / {}", self.step + 1, n)).color(theme::FAINT()).small());
                    });
                });
            });
    }
}
