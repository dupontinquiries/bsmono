use std::sync::mpsc::{channel, Receiver};

use hive::directive::{self, Directive, Reply};
use hive::plan::Plan;

pub struct Panel {
    pub base: String,
    pub token: String,
    pub reply: Option<Reply>,
    pub error: Option<String>,
    pub models: Vec<(String, f64, bool)>,   // name, GiB, cached
    pub profile: String,
    pub plan_path: String,
    pub matrix_name: String,
    // Integrate: land a workspace bundle where the code lives on that box
    pub bundle_path: String,
    pub repo: String,
    pub branch: String,
    pub verify: String,
    rx: Option<Receiver<Result<Reply, String>>>,
    last_op: String,
}

impl Default for Panel {
    fn default() -> Self {
        Panel { base: std::env::var("LOGSCOPE_MINI_URL").unwrap_or("http://mini:8790".into()), token: std::env::var("LOGSCOPE_MINI_TOKEN").unwrap_or_default(),
            reply: None, error: None, models: Vec::new(), profile: "mtp".into(), plan_path: String::new(), matrix_name: "mini".into(),
            bundle_path: String::new(), repo: String::new(), branch: "logscope-work".into(), verify: "cargo check".into(), rx: None, last_op: String::new() }
    }
}

impl Panel {
    fn send(&mut self, d: Directive) {
        let (tx, rx) = channel();
        let (base, token) = (self.base.clone(), Some(self.token.clone()).filter(|t| !t.is_empty()));
        self.last_op = match &d { Directive::Status => "status", Directive::Models => "models", Directive::Load { .. } => "load", Directive::Unload => "unload", Directive::Run { .. } => "run", Directive::Summary { .. } => "summary", Directive::Say { .. } => "say", Directive::Integrate { .. } => "integrate" }.into();
        self.error = None;
        std::thread::spawn(move || { let _ = tx.send(directive::send(&base, token.as_deref(), &d)); });
        self.rx = Some(rx);
    }

    fn pump(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        match rx.try_recv() {
            Ok(Ok(r)) => {
                if self.last_op == "models" {
                    self.models = r.data.get("library").and_then(|l| l.as_array()).map(|a| a.iter().filter_map(|m| Some((m.get("name")?.as_str()?.to_string(), m.get("gib")?.as_f64()?, m.get("cached")?.as_bool()?))).collect()).unwrap_or_default();
                }
                self.reply = Some(r); self.rx = None; false
            }
            Ok(Err(e)) => { self.error = Some(e); self.rx = None; false }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => { self.rx = None; false }
        }
    }

    /// Put the shim's llama-server into this machine's matrix as a large local box, so plans here
    /// can route to the mini and `LOGSCOPE_REMOTE=matrix:<name>` can select it.
    fn register(&self) -> Result<(), String> {
        use hive::matrix::{BoxSpec, Locality, Matrix, Role, Tier};
        let port = self.reply.as_ref().and_then(|r| r.data.get("loaded")).and_then(|l| l.get("port")).and_then(|p| p.as_u64()).unwrap_or(8080);
        let host = self.base.trim_start_matches("http://").trim_start_matches("https://").split(':').next().unwrap_or("mini").to_string();
        let mut m = Matrix::load();
        m.upsert(BoxSpec { name: self.matrix_name.clone(), endpoint: hive::endpoint::Endpoint::OpenAi { base: format!("http://{host}:{port}"), model: "default".into(), key: None },
            roles: vec![Role::Worker, Role::Verifier, Role::Integrator], concurrency: 2, tier: Tier::Large, locality: Locality::Local, cost_per_mtok: None, capability: Default::default(), healthy: true });
        m.save()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let busy = self.pump();
        if busy { ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)); }
        ui.heading("Mini (logscope-shim)");
        ui.horizontal(|ui| { ui.label("url"); ui.add(egui::TextEdit::singleline(&mut self.base).desired_width(180.0)); ui.label("token"); ui.add(egui::TextEdit::singleline(&mut self.token).password(true).desired_width(90.0)); });
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Status")).clicked() { self.send(Directive::Status); }
            if ui.add_enabled(!busy, egui::Button::new("Models")).clicked() { self.send(Directive::Models); }
            if ui.add_enabled(!busy, egui::Button::new("Unload")).clicked() { self.send(Directive::Unload); }
            if ui.add_enabled(!busy, egui::Button::new("Summary")).clicked() { self.send(Directive::Summary { n: 20 }); }
            if ui.add_enabled(self.reply.is_some(), egui::Button::new("Add to matrix")).clicked() { if let Err(e) = self.register() { self.error = Some(e); } }
            if busy { ui.spinner(); }
        });
        ui.horizontal(|ui| { ui.label("profile"); for p in ["plain", "mtp", "ngram"] { ui.selectable_value(&mut self.profile, p.into(), p); } });
        if !self.models.is_empty() {
            egui::ScrollArea::vertical().id_salt("mini-models").max_height(140.0).show(ui, |ui| {
                let mut load: Option<String> = None;
                for (name, gib, cached) in &self.models {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{}{name}", if *cached { "● " } else { "○ " })).monospace());
                        ui.label(egui::RichText::new(format!("{gib:.1} GiB")).small());
                        if ui.add_enabled(!busy, egui::Button::new("Load")).clicked() { load = Some(name.clone()); }
                    });
                }
                if let Some(n) = load { self.send(Directive::Load { model: n, profile: Some(self.profile.clone()) }); }
            });
        }
        ui.horizontal(|ui| {
            ui.label("plan"); ui.add(egui::TextEdit::singleline(&mut self.plan_path).desired_width(200.0).hint_text("plan.json on this machine"));
            if ui.add_enabled(!busy && !self.plan_path.is_empty(), egui::Button::new("Run there")).clicked() {
                match Plan::load(&self.plan_path) { Ok(p) => self.send(Directive::Run { plan: p }), Err(e) => self.error = Some(e) }
            }
        });
        // Integrate: the bubble's exit. The bundle (`mount::bundle`, or any `git bundle`) goes to the
        // box that holds the real checkout; it comes back as a ref, a verifier verdict and the merge
        // command, never as a merge.
        ui.separator();
        ui.label(egui::RichText::new("Integrate a bundle where the code lives on the mini").small());
        ui.horizontal(|ui| { ui.label("bundle"); ui.add(egui::TextEdit::singleline(&mut self.bundle_path).desired_width(200.0).hint_text("exports/logscope-<id>.bundle")); ui.label("branch"); ui.add(egui::TextEdit::singleline(&mut self.branch).desired_width(110.0)); });
        ui.horizontal(|ui| { ui.label("repo on mini"); ui.add(egui::TextEdit::singleline(&mut self.repo).desired_width(200.0).hint_text("/Users/you/src/c19unified")); ui.label("verify"); ui.add(egui::TextEdit::singleline(&mut self.verify).desired_width(110.0)); });
        if ui.add_enabled(!busy && !self.bundle_path.is_empty() && !self.repo.is_empty(), egui::Button::new("Integrate there")).clicked() {
            match std::fs::read(&self.bundle_path) {
                Ok(bytes) => {
                    let name = std::path::Path::new(&self.bundle_path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or("bundle".into());
                    let verify: Vec<String> = self.verify.split_whitespace().map(|s| s.to_string()).collect();
                    self.send(Directive::Integrate { name, bundle_b64: hive::directive::b64::encode(&bytes), repo: self.repo.clone(), branch: self.branch.clone(), verify: if verify.is_empty() { None } else { Some(verify) } });
                }
                Err(e) => self.error = Some(format!("{}: {e}", self.bundle_path)),
            }
        }
        if let Some(e) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED, e); }
        if let Some(r) = &self.reply {
            egui::ScrollArea::vertical().id_salt("mini-reply").max_height(200.0).show(ui, |ui| {
                ui.colored_label(if r.ok { egui::Color32::LIGHT_GREEN } else { egui::Color32::LIGHT_RED }, if r.ok { "ok" } else { "failed" });
                ui.label(egui::RichText::new(&r.text).monospace());
            });
        }
    }
}
