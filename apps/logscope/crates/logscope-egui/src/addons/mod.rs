//! addons — three small panels over `logscope-hive`, mountable in any tab:
//!   * `claude_code::Panel`  — talks to Claude Code the only permitted way (drives the real `claude`
//!                             binary in headless `-p` mode), keeps the session id for follow-ups, and
//!                             files the reply as a ledger artifact / a status line.
//!   * `mini::Panel`         — sends `hive::directive::Directive`s to a `logscope-shim` on another box
//!                             (load/unload models from its NVMe library, run a plan there, read its
//!                             summary), no monitor or KVM involved.
//!   * `status::Panel`       — the liaison's outbound: status lines to the ledger, an HTTP sink (the
//!                             shim) and, with the `dht` feature, the DHT channel hatch shows on the phone.
//! Each panel is a struct with `ui(&mut self, &mut egui::Ui)`; network and process work runs on
//! threads and comes back over channels, so nothing here blocks a frame.
pub mod claude_code;
pub mod mini;
pub mod plan_run;
pub mod status;

/// One instance of each, for a host that wants all three in one tab.
pub struct Panels { pub claude: claude_code::Panel, pub mini: mini::Panel, pub status: status::Panel, pub plan: plan_run::Panel }

impl Default for Panels {
    fn default() -> Self { Panels { claude: claude_code::Panel::default(), mini: mini::Panel::default(), status: status::Panel::default(), plan: plan_run::Panel::default() } }
}

impl Panels {
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |cols| {
            self.claude.ui(&mut cols[0]);
            self.mini.ui(&mut cols[1]);
        });
        ui.separator();
        self.plan.ui(ui);
        ui.separator();
        self.status.ui(ui);
        // outbound hand-offs: a finished Claude reply can be filed or announced from the other panels
        if let Some(line) = self.claude.take_announce() { self.status.say(line.0, &line.1); }
    }
}
