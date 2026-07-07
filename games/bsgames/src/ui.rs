//! ui — the egui game centre: a grid of launchable games, a friends roster (presence over the
//! profile service), and a snapclone chat panel. Edits are local; launching spawns the game, and
//! chat/join route out to snapclone + the game's netcode (the I/O seams are marked).
use crate::centre::{self, GameEntry};
use netcode::profile::Profile;
use netcode::store::{MicroDhtStore, ProfileService};

/// LAN-only service for now; the OpenDHT primary drops into the first slot once its FFI backend is
/// built (feature `opendht`), no UI change.
pub type Service = ProfileService<MicroDhtStore, MicroDhtStore>;

pub struct GameCentre {
    pub prefs: settings_kit::Prefs,
    pub me: Profile,
    pub svc: Service,
    pub games: Vec<GameEntry>,
    pub status: String,
    pub chat_to: Option<String>,
    pub chat_input: String,
    /// The fleet DHT store the chat Mailbox rides on (real share-kit/snapcore path).
    pub store: snapcore::store::Store,
    pub now: u64,
}

impl GameCentre {
    pub fn new(me: Profile, svc: Service) -> Self {
        GameCentre {
            prefs: settings_kit::Prefs::default(),
            me,
            svc,
            games: centre::registry(),
            status: String::new(),
            chat_to: None,
            chat_input: String::new(),
            store: snapcore::store::Store::new(),
            now: 1,
        }
    }

    #[allow(deprecated)] // eframe hands app code only a Context; top-level Panel::show(ctx) is the only path
    pub fn ui(&mut self, ctx: &egui::Context) {
        uikit_egui::apply_prefs(ctx, &self.prefs);
        egui::SidePanel::right("friends").min_width(220.0).show(ctx, |ui| self.friends_panel(ui));
        egui::CentralPanel::default().show(ctx, |ui| self.games_grid(ui));
    }

    fn games_grid(&mut self, ui: &mut egui::Ui) {
        ui.heading("BigSilicon Games");
        ui.label(egui::RichText::new("launch a game — friends see you join over the netcode DHT").weak().small());
        ui.separator();
        let games = self.games.clone();
        egui::Grid::new("games").num_columns(2).spacing([12.0, 12.0]).show(ui, |ui| {
            for (i, g) in games.iter().enumerate() {
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(&g.name).strong());
                        ui.label(egui::RichText::new(format!("engine: {}", g.engine)).weak().small());
                        if ui.button("▶ launch").clicked() {
                            self.launch(g);
                        }
                    });
                });
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        if !self.status.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(&self.status).monospace().small());
        }
    }

    fn launch(&mut self, g: &GameEntry) {
        let argv = centre::launch_argv(g);
        // I/O seam: spawn the game process, fire-and-forget.
        let spawned = std::process::Command::new(&argv[0]).args(&argv[1..]).spawn();
        match spawned {
            Ok(_) => {
                self.status = format!("launched {}", g.name);
                // presence: mark in-game + republish to the DHT for remote friends…
                self.me.presence = netcode::profile::Presence::InGame { game: g.name.clone(), session: None };
                self.svc.publish(&self.me);
                // …and map to the fleet-wide coord-core Activity (published via the Coord client on
                // device, so voiced/media/etc. also see we're in a game).
                let _activity = crate::wiring::activity_of(&self.me.presence);
            }
            Err(e) => self.status = format!("couldn't launch {}: {e}", g.name),
        }
    }

    fn friends_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Friends");
        ui.separator();
        let rows = centre::friend_rows(&self.me, &self.svc);
        if rows.is_empty() {
            ui.label(egui::RichText::new("no friends online").weak());
        }
        for r in &rows {
            ui.horizontal(|ui| {
                let dot = if r.status == "offline" { "○" } else { "●" };
                ui.label(dot);
                ui.label(egui::RichText::new(&r.name).strong());
                ui.label(egui::RichText::new(&r.status).weak().small());
            });
            ui.horizontal(|ui| {
                if r.joinable && ui.small_button("join").clicked() {
                    // netcode I/O seam: join their session via the game's transport.
                    self.status = format!("joining {}…", r.name);
                }
                if ui.small_button("chat").clicked() {
                    self.chat_to = Some(r.id.clone());
                }
            });
            ui.separator();
        }
        self.chat_panel(ui);
    }

    fn chat_panel(&mut self, ui: &mut egui::Ui) {
        let Some(to) = self.chat_to.clone() else {
            return;
        };
        ui.add_space(8.0);
        ui.label(egui::RichText::new(format!("chat · {to}")).strong());
        let mb = crate::wiring::dm_mailbox(&self.me.id, &to);
        let key = crate::wiring::dm_key(&self.me.id, &to);
        let log = crate::wiring::recv_chat(&self.store, &mb, &key, 0, self.now);
        egui::ScrollArea::vertical().max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
            for (from, text) in &log {
                ui.label(egui::RichText::new(format!("{from}: {text}")).small());
            }
        });
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.chat_input);
            if ui.button("send").clicked() && !self.chat_input.trim().is_empty() {
                let msg = std::mem::take(&mut self.chat_input);
                // real path: sealed posting into the DM feed on the fleet DHT store
                crate::wiring::send_chat(&mut self.store, &mb, &self.me.id, &key, &msg, 0, self.now);
                self.now += 1;
            }
        });
    }
}
