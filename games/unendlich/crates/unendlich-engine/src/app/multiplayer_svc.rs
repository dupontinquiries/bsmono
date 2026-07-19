//! `app::multiplayer_svc` — service #N: fleet online play for unendlich, on the shared
//! `netcode::multiplayer` control plane (game id `"unendlich"`, the same id
//! `bsgames::centre::registry` launches). Host announces a session on the fleet DHT; a friend's
//! bsgames "▶ join" spawns a second unendlich with the `BSGAMES_HANDOFF_*` env set, and this
//! service dials the friend before the title screen ever shows. The engine's own transport is
//! the data plane — this service never touches gameplay bytes.
//!
//! Following the app-core doctrine ("is the menu actually wired?" is a unit test): the
//! env-independent constructor takes the parsed handoff + an explicit store path, so the whole
//! startup behavior — identity override, host/join, status, the skip-title signal — is asserted
//! headless. What is deliberately NOT here yet (marked seam): auto-advancing the GameFlow into a
//! loaded world on `handoff_started` — entering `InGame` requires a world selection, which is
//! menu-flow work; the shell reads [`MultiplayerSvc::handoff_started`] and drives that.

use netcode::handoff::Handoff;
use netcode::multiplayer::Multiplayer;

pub struct MultiplayerSvc {
    pub mp: Option<Multiplayer>,
    /// Human status line ("hosting: unendlich@…", "connected to …", or the failure).
    pub status: String,
    /// True when a startup handoff succeeded — the shell/menu should skip the title screen
    /// toward the match instead of waiting on the main menu.
    pub handoff_started: bool,
}

impl MultiplayerSvc {
    /// The real startup path: parse `BSGAMES_HANDOFF_*` and act on it. Inert (no store touched)
    /// when no handoff env is present — standalone launches are unaffected.
    pub fn from_env() -> Self {
        Self::with_handoff(netcode::handoff::from_env().as_ref(), None)
    }

    /// Env-independent core, testable: `store` overrides the machine-wide default store path.
    pub fn with_handoff(h: Option<&Handoff>, store: Option<std::path::PathBuf>) -> Self {
        let Some(h) = h else {
            return MultiplayerSvc { mp: None, status: String::new(), handoff_started: false };
        };
        let (me_id, me_name) = match h {
            Handoff::Host { me_id, me_name } | Handoff::Join { me_id, me_name, .. } => (me_id.as_str(), me_name.as_str()),
        };
        let mut m = match store {
            Some(p) => Multiplayer::at_store("unendlich", me_id, me_name, p),
            None => Multiplayer::new("unendlich", me_id, me_name),
        };
        let (status, started) = match m.apply_handoff(h) {
            Ok(s) => (s, true),
            Err(e) => (e, false),
        };
        MultiplayerSvc { mp: Some(m), status, handoff_started: started }
    }

    /// The menu's Multiplayer action: toggle hosting (presence flips `InGame`, so friends'
    /// bsgames rosters show us joinable). Returns the new status line.
    pub fn toggle_host(&mut self) -> &str {
        self.toggle_host_at(None)
    }

    /// Testable variant with an explicit store path.
    pub fn toggle_host_at(&mut self, store: Option<std::path::PathBuf>) -> &str {
        let mp = self.mp.get_or_insert_with(|| match store {
            Some(p) => Multiplayer::at_store("unendlich", "wanderer", "Wanderer", p),
            None => Multiplayer::new("unendlich", "wanderer", "Wanderer"),
        });
        self.status = if mp.token.is_some() {
            mp.stop_hosting();
            "stopped hosting".to_string()
        } else {
            match mp.host("0.0.0.0:0") {
                Ok(t) => format!("hosting: {t}"),
                Err(e) => format!("couldn't host: {e}"),
            }
        };
        &self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("unendlich-mp-{name}-{}.jsonl", std::process::id()))
    }

    #[test]
    fn no_handoff_is_inert() {
        let s = MultiplayerSvc::with_handoff(None, None);
        assert!(s.mp.is_none());
        assert!(!s.handoff_started);
        assert!(s.status.is_empty());
    }

    #[test]
    fn host_handoff_binds_announces_and_signals_skip_title() {
        let path = tmp("host");
        let h = Handoff::Host { me_id: "me@0".into(), me_name: "Kessler".into() };
        let s = MultiplayerSvc::with_handoff(Some(&h), Some(path.clone()));
        assert!(s.handoff_started, "successful host handoff must signal the shell: {}", s.status);
        assert_eq!(s.status, "hosting: unendlich@me@0");
        // the launched process presents as the SAME player bsgames published
        assert_eq!(s.mp.as_ref().unwrap().me.id, "me@0");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn join_handoff_resolves_the_host_through_the_shared_store() {
        let path = tmp("join");
        let host = MultiplayerSvc::with_handoff(
            Some(&Handoff::Host { me_id: "h@0".into(), me_name: "H".into() }),
            Some(path.clone()),
        );
        assert!(host.handoff_started);
        let j = MultiplayerSvc::with_handoff(
            Some(&Handoff::Join { me_id: "j@1".into(), me_name: "J".into(), peer_id: "h@0".into() }),
            Some(path.clone()),
        );
        assert!(j.handoff_started, "{}", j.status);
        assert!(j.status.starts_with("connected to h@0 at "), "{}", j.status);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_failed_join_surfaces_the_error_and_keeps_the_menu() {
        let path = tmp("fail");
        let j = MultiplayerSvc::with_handoff(
            Some(&Handoff::Join { me_id: "j@1".into(), me_name: "J".into(), peer_id: "nobody".into() }),
            Some(path.clone()),
        );
        assert!(!j.handoff_started);
        assert!(j.status.contains("nobody"), "{}", j.status);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn menu_toggle_hosts_then_stops() {
        let path = tmp("toggle");
        let mut s = MultiplayerSvc::with_handoff(None, None);
        let st = s.toggle_host_at(Some(path.clone())).to_string();
        assert!(st.starts_with("hosting: unendlich@"), "{st}");
        let st = s.toggle_host_at(Some(path.clone())).to_string();
        assert_eq!(st, "stopped hosting");
        let _ = std::fs::remove_file(&path);
    }
}
