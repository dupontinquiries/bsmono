use std::sync::mpsc::{channel, Receiver};

use hive::endpoint::{self, Endpoint, Event, Metrics, Request};
use hive::status::Level;

const TOOLS: [&str; 6] = ["Read", "Grep", "Glob", "Edit", "Write", "Bash"];

pub struct Panel {
    pub prompt: String,
    pub cwd: String,
    pub tools: [bool; 6],
    pub output: String,
    pub session: Option<String>,
    pub last: Option<Metrics>,
    pub error: Option<String>,
    rx: Option<Receiver<Event>>,
    announce: Option<(Level, String)>,
    pub artifact_id: String,
}

impl Default for Panel {
    fn default() -> Self {
        Panel { prompt: String::new(), cwd: std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default(),
            tools: [true, true, true, false, false, false], output: String::new(), session: None, last: None, error: None, rx: None, announce: None, artifact_id: "claude-1".into() }
    }
}

impl Panel {
    fn endpoint(&self) -> Endpoint {
        let allowed: Vec<&str> = TOOLS.iter().zip(self.tools.iter()).filter(|(_, on)| **on).map(|(t, _)| *t).collect();
        let mut extra = Vec::new();
        if !allowed.is_empty() { extra.push("--allowedTools".to_string()); extra.push(allowed.join(",")); }
        Endpoint::ClaudeCode { extra, cwd: Some(self.cwd.clone()).filter(|c| !c.is_empty()) }
    }

    fn start(&mut self, resume: bool) {
        let (tx, rx) = channel();
        let ep = self.endpoint();
        let req = Request { prompt: self.prompt.clone(), system: None, max_tokens: 0, temperature: 0.0, resume: if resume { self.session.clone() } else { None }, grammar: None };
        self.output.clear(); self.error = None; self.last = None;
        std::thread::spawn(move || { let mut on = |e: Event| tx.send(e).is_ok(); let _ = endpoint::generate(&ep, &req, &mut on); });
        self.rx = Some(rx);
    }

    fn pump(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let mut busy = true;
        while let Ok(e) = rx.try_recv() {
            match e {
                Event::Started => {}
                Event::Delta(s) => self.output.push_str(&s),
                Event::Reasoning(_) => {}   // Claude Code carries reasoning too; the panel shows the answer only
                Event::ToolCall { name, .. } => self.output.push_str(&format!("\n[tool: {name}]\n")),
                Event::Done(m) => { if m.session_id.is_some() { self.session = m.session_id.clone(); } self.last = Some(m); busy = false; }
                Event::Error(e) => { self.error = Some(e); busy = false; }
            }
        }
        if !busy { self.rx = None; }
        busy
    }

    /// The status panel picks this up once per frame.
    pub fn take_announce(&mut self) -> Option<(Level, String)> { self.announce.take() }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.pump();
        if busy { ui.ctx().request_repaint_after(std::time::Duration::from_millis(50)); }
        ui.heading("Claude Code (headless `claude -p`)");
        ui.label(egui::RichText::new("Drives the real binary on this machine with its own login; nothing here touches credentials.").small());
        ui.horizontal(|ui| { ui.label("cwd"); ui.add(egui::TextEdit::singleline(&mut self.cwd).desired_width(260.0)); });
        ui.horizontal_wrapped(|ui| { ui.label("tools"); for (i, t) in TOOLS.iter().enumerate() { ui.checkbox(&mut self.tools[i], *t); } });
        ui.add(egui::TextEdit::multiline(&mut self.prompt).desired_rows(3).desired_width(f32::INFINITY).hint_text("What should Claude do in this checkout?"));
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy && !self.prompt.trim().is_empty(), egui::Button::new("Run")).clicked() { self.start(false); }
            if ui.add_enabled(!busy && self.session.is_some() && !self.prompt.trim().is_empty(), egui::Button::new("Continue session")).clicked() { self.start(true); }
            if let Some(s) = &self.session { ui.label(egui::RichText::new(format!("session {}", &s[..s.len().min(8)])).small()); }
            if busy { ui.spinner(); }
        });
        egui::ScrollArea::vertical().id_salt("claude-out").max_height(260.0).stick_to_bottom(true).show(ui, |ui| {
            if let Some(e) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED, e); }
            ui.label(egui::RichText::new(&self.output).monospace());
        });
        if let Some(m) = &self.last {
            ui.label(egui::RichText::new(format!("{} tok · {:.1} s{}", m.out_tokens, m.total_ms / 1000.0, m.cost_usd.map(|c| format!(" · ${c:.4}")).unwrap_or_default())).small());
            ui.horizontal(|ui| {
                ui.label("file as"); ui.add(egui::TextEdit::singleline(&mut self.artifact_id).desired_width(100.0));
                if ui.button("→ ledger artifact").clicked() {
                    if let Ok(p) = hive::ledger::put_artifact(&self.artifact_id, "review", &self.output) {
                        let _ = hive::ledger::append(hive::ledger::Entry::Artifact { task: self.artifact_id.clone(), artifact: "review".into(), r#box: "claude".into(), chars: self.output.chars().count(), path: p.display().to_string() });
                    }
                }
                if ui.button("→ status line").clicked() {
                    let head: String = self.output.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(200).collect();
                    self.announce = Some((Level::Inference, format!("claude: {head}")));
                }
            });
        }
    }
}
