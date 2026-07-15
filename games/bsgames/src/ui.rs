//! ui — the egui game centre: a grid of launchable games, a friends roster (presence over the
//! profile service), and a snapclone chat panel. Edits are local; launching spawns the game, and
//! chat/join route out to snapclone + the game's netcode (the I/O seams are marked).
use crate::centre::{self, GameEntry};
use crate::identity::LocalIdentity;
use crate::wizard::{self, WizardEvent, WizardPhase};
use netcode::profile::Profile;
use netcode::store::{ProfileService, SnapcoreStore};

/// LAN-only service for now; the OpenDHT primary drops into the first slot once its FFI backend is
/// built (feature `opendht`), no UI change.
pub type Service = ProfileService<SnapcoreStore, SnapcoreStore>;

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

    /// Persisted local profile (id + display name); `me` above is the *published* wire copy kept
    /// in sync with it. Onboarding wizard walks Welcome → Name → ShareId → AddFriend → Done.
    pub identity: LocalIdentity,
    pub wizard_phase: WizardPhase,
    pub name_edit: String,
    pub add_friend_input: String,
    pub qr_texture: Option<egui::TextureHandle>,
    pub show_qr: bool,
}

impl GameCentre {
    pub fn new(me: Profile, svc: Service) -> Self {
        let first_run = LocalIdentity::is_first_run();
        let identity = LocalIdentity::load_or_create();
        let mut me = me;
        me.id = identity.id.clone();
        me.name = identity.name.clone();
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
            wizard_phase: wizard::initial_phase(first_run),
            name_edit: identity.name.clone(),
            identity,
            add_friend_input: String::new(),
            qr_texture: None,
            show_qr: false,
        }
    }

    #[allow(deprecated)] // eframe hands app code only a Context; top-level Panel::show(ctx) is the only path
    pub fn ui(&mut self, ctx: &egui::Context) {
        uikit_egui::apply_prefs(ctx, &self.prefs);
        if self.wizard_phase != WizardPhase::Done {
            egui::CentralPanel::default().show(ctx, |ui| self.wizard_panel(ui, ctx));
            return;
        }
        egui::SidePanel::right("friends").min_width(240.0).show(ctx, |ui| self.friends_panel(ui, ctx));
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
                        ui.horizontal(|ui| {
                            if ui.button("▶ launch").clicked() {
                                self.launch(g);
                            }
                            // Quick-match hook: same launch, plus --quick-match (see
                            // centre::quick_match_argv's doc comment — no game reads the flag yet,
                            // this is the seam future per-game wiring lands on one at a time).
                            if ui.button("⚡ quick match").clicked() {
                                self.quick_match(g);
                            }
                        });
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
        self.spawn(centre::launch_argv(g), g);
    }

    fn quick_match(&mut self, g: &GameEntry) {
        self.spawn(centre::quick_match_argv(g), g);
    }

    fn spawn(&mut self, argv: Vec<String>, g: &GameEntry) {
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

    /// The Profile section: editable display name, read-only id + Copy, and a QR toggle — reused
    /// as-is inside the wizard's ShareId step and standing permanently in the friends panel.
    fn profile_section(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Name:").weak().small());
            if ui.text_edit_singleline(&mut self.name_edit).lost_focus() && !self.name_edit.trim().is_empty() {
                let name = self.name_edit.trim().to_string();
                self.identity.name = name.clone();
                self.identity.save();
                self.me.name = name;
                self.svc.publish(&self.me);
            }
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Your ID:").weak().small());
            ui.label(egui::RichText::new(&self.identity.id).monospace().strong());
            if ui.small_button("copy").clicked() {
                ctx.copy_text(self.identity.id.clone());
                self.status = "copied your id".into();
            }
        });
        if ui.small_button(if self.show_qr { "hide QR" } else { "show QR" }).clicked() {
            self.show_qr = !self.show_qr;
        }
        if self.show_qr {
            self.qr_image(ui, ctx);
        }
    }

    fn qr_image(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.qr_texture.is_none() {
            if let Some(img) = crate::qr::render(&format!("bsgames:{}", self.identity.id), 6) {
                let color = egui::ColorImage::from_gray([img.side, img.side], &img.pixels);
                self.qr_texture = Some(ctx.load_texture("my-qr", color, egui::TextureOptions::NEAREST));
            }
        }
        if let Some(tex) = &self.qr_texture {
            ui.add(egui::Image::new(tex).max_width(200.0));
        }
    }

    /// The friend-adding section: paste a friend's id, "Add" validates via `centre::add_friend`.
    /// Reused as-is inside the wizard's AddFriend step and permanently in the friends panel.
    fn add_friend_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Add a friend by id:").weak().small());
            ui.text_edit_singleline(&mut self.add_friend_input);
            if ui.button("add").clicked() {
                match centre::add_friend(&mut self.me, &mut self.svc, &self.add_friend_input) {
                    Ok(name) => {
                        self.status = format!("added {name} as a friend");
                        self.add_friend_input.clear();
                        if self.wizard_phase == WizardPhase::AddFriend {
                            self.wizard_phase = wizard::next_phase(self.wizard_phase, &WizardEvent::FriendAdded);
                        }
                    }
                    Err(e) => self.status = e,
                }
            }
        });
    }

    fn wizard_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            match self.wizard_phase {
                WizardPhase::Welcome => {
                    ui.heading("Welcome to BigSilicon Games");
                    ui.label("Let's get you set up with a name and a couple of friends.");
                    ui.add_space(16.0);
                    if ui.button("Get Started").clicked() {
                        self.wizard_phase = wizard::next_phase(self.wizard_phase, &WizardEvent::Next);
                    }
                    if ui.small_button("skip setup").clicked() {
                        self.wizard_phase = wizard::next_phase(self.wizard_phase, &WizardEvent::Skip);
                    }
                }
                WizardPhase::Name => {
                    ui.heading("What should friends call you?");
                    ui.add_space(8.0);
                    ui.text_edit_singleline(&mut self.name_edit);
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui.button("Back").clicked() {
                            self.wizard_phase = wizard::next_phase(self.wizard_phase, &WizardEvent::Back);
                        }
                        let can_continue = !self.name_edit.trim().is_empty();
                        if ui.add_enabled(can_continue, egui::Button::new("Continue")).clicked() {
                            let name = self.name_edit.trim().to_string();
                            self.identity.name = name.clone();
                            self.identity.save();
                            self.me.name = name;
                            self.svc.publish(&self.me);
                            self.wizard_phase = wizard::next_phase(self.wizard_phase, &WizardEvent::Next);
                        }
                    });
                }
                WizardPhase::ShareId => {
                    ui.heading("Here's your id");
                    ui.label("Share it (or the QR) so friends can add you.");
                    ui.add_space(8.0);
                    self.profile_section(ui, ctx);
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui.button("Back").clicked() {
                            self.wizard_phase = wizard::next_phase(self.wizard_phase, &WizardEvent::Back);
                        }
                        if ui.button("Continue").clicked() {
                            self.wizard_phase = wizard::next_phase(self.wizard_phase, &WizardEvent::Next);
                        }
                    });
                }
                WizardPhase::AddFriend => {
                    ui.heading("Add a friend");
                    ui.label("Paste a friend's id (they can copy it from their own Profile).");
                    ui.add_space(8.0);
                    self.add_friend_section(ui);
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui.button("Back").clicked() {
                            self.wizard_phase = wizard::next_phase(self.wizard_phase, &WizardEvent::Back);
                        }
                        if ui.small_button("I'll do this later").clicked() {
                            self.wizard_phase = wizard::next_phase(self.wizard_phase, &WizardEvent::Skip);
                        }
                    });
                    if !self.status.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(&self.status).monospace().small());
                    }
                }
                WizardPhase::Done => {} // unreachable: `ui()` doesn't route here
            }
        });
    }

    fn friends_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Profile");
        ui.separator();
        self.profile_section(ui, ctx);
        ui.add_space(10.0);
        self.add_friend_section(ui);
        ui.add_space(14.0);
        ui.separator();
        ui.heading("Friends");
        ui.separator();
        let rows = centre::friend_rows(&self.me, &self.svc);
        if rows.is_empty() {
            ui.label(egui::RichText::new("no friends yet — add one above").weak());
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
        egui::ScrollArea::vertical().id_salt("chat").max_height(160.0).stick_to_bottom(true).show(ui, |ui| {
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
