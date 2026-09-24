//! plan_run — run a `hive::plan::Plan` on this machine's matrix, live. The dispatcher runs on a
//! thread with a channel-backed `Observer`; the panel draws waves, deltas and verdicts as they arrive.
//! Plans come from `:survey` (the Queen), from a file, or from `hive example-plan`.
use std::sync::mpsc::{channel, Receiver};

use hive::dispatch::{self, Observer, Outcome};
use hive::matrix::Matrix;
use hive::plan::Plan;

enum Ev { Wave(usize, Vec<String>), Started(String, String), Delta(String, String), Finished(Outcome), Done(Result<usize, String>) }

struct ChanObserver(std::sync::mpsc::Sender<Ev>);
impl Observer for ChanObserver {
    fn wave(&mut self, n: usize, ids: &[String]) { let _ = self.0.send(Ev::Wave(n, ids.to_vec())); }
    fn started(&mut self, task: &str, on: &str) { let _ = self.0.send(Ev::Started(task.into(), on.into())); }
    fn delta(&mut self, task: &str, text: &str) { let _ = self.0.send(Ev::Delta(task.into(), text.into())); }
    fn finished(&mut self, o: &Outcome) { let _ = self.0.send(Ev::Finished(o.clone())); }
}

#[derive(Default)]
pub struct Panel {
    pub title: String,
    pub plan: Option<Plan>,
    pub plan_path: String,
    pub live: Vec<(String, String)>,      // task → streamed text
    pub done: Vec<Outcome>,
    pub waves: Vec<String>,
    pub error: Option<String>,
    rx: Option<Receiver<Ev>>,
}

impl Panel {
    /// Called by the Chat's `:survey` hand-off (through `Tabs`).
    pub fn set_plan(&mut self, title: String, plan: Plan) { self.title = title; self.plan = Some(plan); self.live.clear(); self.done.clear(); self.waves.clear(); self.error = None; }

    fn start(&mut self) {
        let Some(plan) = self.plan.clone() else { return };
        let (tx, rx) = channel();
        self.live = plan.tasks.iter().map(|t| (t.id.clone(), String::new())).collect();
        self.done.clear(); self.waves.clear(); self.error = None;
        std::thread::spawn(move || {
            let m = Matrix::load();
            let mut obs = ChanObserver(tx.clone());
            let r = dispatch::run(&plan, &m, &mut obs).map(|outs| outs.iter().filter(|o| o.keep).count());
            let _ = tx.send(Ev::Done(r));
        });
        self.rx = Some(rx);
    }

    fn pump(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut running = true;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Ev::Wave(n, ids) => self.waves.push(format!("wave {n}: {}", ids.join(", "))),
                Ev::Started(t, on) => self.waves.push(format!("  {t} → {on}")),
                Ev::Delta(t, s) => { if let Some((_, buf)) = self.live.iter_mut().find(|(id, _)| *id == t) { buf.push_str(&s); } }
                Ev::Finished(o) => self.done.push(o),
                Ev::Done(r) => { if let Err(e) = r { self.error = Some(e); } running = false; }
            }
        }
        if !running { self.rx = None; }
        running
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.pump();
        if busy { ui.ctx().request_repaint_after(std::time::Duration::from_millis(50)); }
        ui.heading(if self.title.is_empty() { "Plan" } else { &self.title });
        ui.horizontal(|ui| {
            ui.label("plan file"); ui.add(egui::TextEdit::singleline(&mut self.plan_path).id_salt("plan-run-plan-path").desired_width(220.0).hint_text("plan.json (or :survey in Chat)"));
            if ui.add_enabled(!busy && !self.plan_path.is_empty(), egui::Button::new("Load")).clicked() {
                match Plan::load(&self.plan_path) { Ok(p) => { let t = self.plan_path.clone(); self.set_plan(t, p); } Err(e) => self.error = Some(e) }
            }
            if ui.add_enabled(!busy && self.plan.is_some(), egui::Button::new("Run on matrix")).clicked() { self.start(); }
            if busy { ui.spinner(); }
        });
        if let Some(p) = &self.plan {
            let m = Matrix::load();
            ui.label(egui::RichText::new(format!("{} tasks in {} wave(s) · matrix: {} box(es), worker width {}", p.tasks.len(), p.waves().map(|w| w.len()).unwrap_or(0), m.boxes.len(), m.width(hive::matrix::Role::Worker))).small());
        }
        if let Some(e) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED, e); }
        egui::ScrollArea::vertical().id_salt("plan-run").max_height(300.0).show(ui, |ui| {
            for w in &self.waves { ui.label(egui::RichText::new(w).small().monospace()); }
            for (id, text) in &self.live {
                let verdict = self.done.iter().find(|o| o.task == *id);
                let head = match verdict { Some(o) if o.keep => format!("{id} · KEEP on {} ({} tok, {:.1}s)", o.r#box, o.out_tokens, o.ms / 1000.0), Some(o) => format!("{id} · DISCARD · {}", o.reason), None => format!("{id} …") };
                ui.label(egui::RichText::new(head).strong().color(match verdict { Some(o) if o.keep => egui::Color32::LIGHT_GREEN, Some(_) => egui::Color32::LIGHT_RED, None => egui::Color32::GRAY }));
                if !text.is_empty() { ui.label(egui::RichText::new(text.chars().rev().take(800).collect::<Vec<_>>().into_iter().rev().collect::<String>()).monospace().small()); }
            }
        });
    }
}
