//! Share page — advertise this repo so a peer can fetch it, or fetch a peer's.
//!
//! `cockpit::netshare` was one of the modules with no egui surface. It does two
//! symmetric things — serve a `ProjectCard` over HTTP, and look one up and clone
//! it — and neither was reachable from the GUI.
//!
//! ## The address is the whole interaction
//!
//! Advertising is useless if the person cannot tell the other machine where to
//! look, and `local_ip()` exists precisely because "localhost" is never the
//! answer. The address is shown large and copyable the moment sharing starts;
//! everything else on the panel is secondary to being able to read it out.
//!
//! ## Serving is a state, not an action
//!
//! `Advertiser` has begin / should_stop / end, so it is a thing that stays on.
//! A page that fires a one-shot and forgets leaves a listener bound with nothing
//! showing it, and the user finds out when the port is still busy tomorrow. The
//! panel shows serving-or-not as its primary state and offers the opposite
//! action.

use crate::app::App;
use std::sync::Arc;

pub struct State {
    pub adv: Arc<cockpit::netshare::Advertiser>,
    /// Peer address to fetch from, as typed.
    pub peer_addr: String,
    /// Where a fetched project lands.
    pub dest: String,
    pub note: String,
    /// The card last looked up, so the user sees WHAT they are about to clone
    /// before they clone it.
    pub found: Option<(String, String)>,
}

impl Default for State {
    fn default() -> Self {
        State {
            adv: Arc::new(cockpit::netshare::Advertiser::default()),
            peer_addr: String::new(),
            // The same base imported projects use, so a fetched repo lands
            // where the rest of them are rather than in the current directory.
            dest: cockpit::places::default_repos_base(),
            note: String::new(),
            found: None,
        }
    }
}

/// The address to read out to someone across the room.
///
/// `local_ip()` returns None with no route — a machine with no network, or one
/// behind an interface that cannot reach out. Saying so is the point: "sharing
/// on localhost:9-something" is a wrong answer that wastes both people's time.
pub fn advertise_address(port: u16) -> Option<String> {
    cockpit::netshare::local_ip().map(|ip| format!("{ip}:{port}"))
}

pub const PORT: u16 = 7421;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Share");
    ui.label(egui::RichText::new("advertise this repo, or fetch a peer's").weak().small());
    ui.separator();

    // ── advertising ──────────────────────────────────────────────────
    let serving = app.share.adv.is_serving();
    ui.horizontal(|ui| {
        if serving {
            ui.label(egui::RichText::new("● sharing")
                .color(egui::Color32::from_rgb(120, 200, 140)).strong());
            if ui.button("stop").clicked() {
                app.share.adv.end();
                app.share.note = "stopped sharing".into();
            }
        } else {
            if ui.button("share this repo").clicked() {
                match advertise_address(PORT) {
                    Some(addr) => {
                        app.share.adv.begin(addr.clone());
                        app.share.note = format!("sharing on {addr}");
                    }
                    // Not a silent no-op: the user pressed a button and is
                    // owed a reason.
                    None => app.share.note =
                        "no network route — cannot advertise an address a peer could reach".into(),
                }
            }
        }
    });

    if serving {
        let addr = app.share.adv.addr();
        ui.add_space(4.0);
        // Large and selectable: this is read out loud or typed on another
        // machine, and a small grey label is the wrong size for both.
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&addr).monospace().size(18.0));
            if ui.small_button("copy").clicked() { ui.ctx().copy_text(addr.clone()); }
        });
        ui.label(egui::RichText::new("on the other machine: paste this below and fetch")
            .weak().small());
    }

    ui.add_space(8.0);
    ui.separator();

    // ── fetching ─────────────────────────────────────────────────────
    ui.label("fetch from a peer");
    ui.horizontal(|ui| {
        ui.label("address");
        ui.text_edit_singleline(&mut app.share.peer_addr);
        let can = !app.share.peer_addr.trim().is_empty() && !app.task.is_running();
        if ui.add_enabled(can, egui::Button::new("look up")).clicked() {
            match cockpit::netshare::lookup_peer(app.share.peer_addr.trim(),
                                                 std::time::Duration::from_secs(4)) {
                Ok(card) => {
                    // A card can advertise SEVERAL providers — ssh, rclone, a
                    // dht id — and the first is the one install_card takes.
                    // Showing it rather than the count means the user can see
                    // whether the route is one their machine can actually use.
                    let provider = card.providers.first().cloned().unwrap_or_default();
                    app.share.found = Some((card.name.clone(), provider));
                    app.share.note = format!("found {}", card.name);
                }
                Err(e) => { app.share.found = None; app.share.note = format!("error: {e}"); }
            }
        }
    });

    // Look up before cloning, always. A clone is minutes and a directory; the
    // user should see the project's NAME and where it would come from before
    // committing to either.
    if let Some((name, provider)) = app.share.found.clone() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(format!("{name}")).strong());
        if provider.is_empty() {
            // A card with no provider cannot be fetched from, and saying so
            // beats a fetch button that fails with a blank url.
            ui.label(egui::RichText::new("no provider advertised — nothing to fetch from")
                .weak().small());
        } else {
            ui.label(egui::RichText::new(&provider).monospace().small());
        }
        ui.horizontal(|ui| {
            ui.label("into");
            ui.text_edit_singleline(&mut app.share.dest);
        });
        let can_fetch = !app.task.is_running() && !provider.is_empty();
        if ui.add_enabled(can_fetch, egui::Button::new("fetch")).clicked() {
            let (n, p, d) = (name.clone(), provider.clone(), app.share.dest.clone());
            app.task.run(move || match cockpit::netshare::install_card(&n, &p, &d) {
                Ok(m) => m,
                Err(e) => format!("error: {e}"),
            });
        }
    }

    if let Some(r) = app.task.take_result() { app.share.note = r; }
    if app.task.is_running() { ui.ctx().request_repaint(); }
    if !app.share.note.is_empty() {
        ui.add_space(6.0);
        ui.separator();
        ui.label(egui::RichText::new(&app.share.note).monospace().small());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serving_is_a_state_that_survives_the_frame() {
        // Advertiser has begin/end because it stays on. A page that fired a
        // one-shot would leave a listener bound with nothing showing it, and the
        // user would find out when the port was still busy tomorrow.
        let a = cockpit::netshare::Advertiser::default();
        assert!(!a.is_serving());
        a.begin("192.168.1.5:7421".into());
        assert!(a.is_serving());
        assert_eq!(a.addr(), "192.168.1.5:7421");
        a.end();
        assert!(!a.is_serving());
    }

    #[test]
    fn an_address_carries_the_port_a_peer_must_use() {
        // "192.168.1.5" alone is not something the other machine can fetch
        // from, and the person reading it out has no way to know that.
        if let Some(addr) = advertise_address(PORT) {
            assert!(addr.contains(&PORT.to_string()), "{addr}");
            assert!(addr.contains(':'));
            assert!(!addr.starts_with("127."), "localhost is never the answer: {addr}");
        }
    }

    #[test]
    fn a_fetch_lands_where_imported_projects_live() {
        // Not the current directory: a repo fetched from a peer belongs with
        // the others, and defaulting to cwd scatters them wherever the app
        // happened to be launched.
        let s = State::default();
        assert_eq!(s.dest, cockpit::places::default_repos_base());
        assert!(!s.dest.is_empty());
    }

    #[test]
    fn nothing_is_offered_to_fetch_before_a_lookup() {
        // A clone is minutes and a directory; the name and provider have to be
        // on screen before the button that starts it.
        let s = State::default();
        assert!(s.found.is_none());
    }
}
