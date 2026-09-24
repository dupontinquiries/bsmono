use hive::status::{self, Level, Sink, StatusLine};

/// The liaison's outbound. Other code calls `say`; the panel shows the feed and lets you type one.
pub struct Panel {
    pub source: String,
    pub sink_http: String,
    pub sink_token: String,
    pub compose: String,
    pub feed: Vec<StatusLine>,
    pub errors: Vec<(String, String)>,
    #[cfg(feature = "dht")]
    pub dht: Option<status::dht::DhtSink>,
    #[cfg(feature = "dht")]
    pub channel: String,
}

impl Default for Panel {
    fn default() -> Self {
        Panel { source: std::env::var("LOGSCOPE_BOX_NAME").unwrap_or("ultra".into()), sink_http: std::env::var("LOGSCOPE_MINI_URL").unwrap_or_default(), sink_token: std::env::var("LOGSCOPE_MINI_TOKEN").unwrap_or_default(),
            compose: String::new(), feed: Vec::new(), errors: Vec::new(),
            #[cfg(feature = "dht")] dht: None,
            #[cfg(feature = "dht")] channel: std::env::var("LOGSCOPE_STATUS_CHANNEL").unwrap_or_default() }
    }
}

impl Panel {
    fn sinks(&self) -> Vec<Box<dyn Sink>> {
        let mut v: Vec<Box<dyn Sink>> = Vec::new();
        if !self.sink_http.is_empty() { v.push(Box::new(status::HttpSink { base: self.sink_http.clone(), token: Some(self.sink_token.clone()).filter(|t| !t.is_empty()) })); }
        v
    }

    /// The one call the rest of the app makes: a line from the liaison (or a worker event) goes out.
    pub fn say(&mut self, level: Level, text: &str) {
        let line = StatusLine::new(&self.source, level, text);
        let mut errs = status::broadcast(&self.sinks(), &line);
        #[cfg(feature = "dht")]
        if let Some(d) = &self.dht { if let Err(e) = d.publish(&line) { errs.push(("dht".into(), e)); } }
        self.errors = errs;
        self.feed.push(line); if self.feed.len() > 200 { self.feed.remove(0); }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Status out (liaison → ledger / shim / phone)");
        ui.horizontal(|ui| {
            ui.label("as"); ui.add(egui::TextEdit::singleline(&mut self.source).desired_width(60.0));
            ui.label("http sink"); ui.add(egui::TextEdit::singleline(&mut self.sink_http).desired_width(160.0).hint_text("http://mini:8790"));
            ui.add(egui::TextEdit::singleline(&mut self.sink_token).password(true).desired_width(80.0).hint_text("token"));
        });
        #[cfg(feature = "dht")]
        ui.horizontal(|ui| {
            ui.label("dht channel"); ui.add(egui::TextEdit::singleline(&mut self.channel).password(true).desired_width(160.0));
            let on = self.dht.is_some();
            if ui.button(if on { "disconnect" } else { "connect" }).clicked() {
                if on { self.dht = None; }
                else if !self.channel.is_empty() {
                    match status::dht::DhtSink::new(&self.channel, "0.0.0.0:0".parse().unwrap(), None) { Ok(s) => self.dht = Some(s), Err(e) => self.errors = vec![("dht".into(), e.to_string())] }
                }
            }
            if on { ui.label(egui::RichText::new("publishing").small()); }
        });
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.compose).desired_width(f32::INFINITY).hint_text("say something to the phone"));
            if ui.button("Say").clicked() && !self.compose.trim().is_empty() { let t = std::mem::take(&mut self.compose); self.say(Level::Inference, &t); }
        });
        for (s, e) in &self.errors { ui.colored_label(egui::Color32::LIGHT_RED, format!("{s}: {e}")); }
        egui::ScrollArea::vertical().id_salt("status-feed").max_height(140.0).stick_to_bottom(true).show(ui, |ui| {
            for l in &self.feed { ui.label(egui::RichText::new(l.render()).monospace().small()); }
        });
    }
}
