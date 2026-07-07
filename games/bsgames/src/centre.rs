//! centre — the pure game-centre backend.
use netcode::profile::{Presence, Profile};
use netcode::store::ProfileBackend;

/// How a game is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchKind {
    /// `cargo run --release` in the game's dir (the Rust games).
    Cargo,
    /// A CMake-built binary at `<dir>/build/<name>` (cybertron).
    CmakeBin(String),
}

/// An installed game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameEntry {
    pub id: String,
    pub name: String,
    /// Graphics engine, for display + the future render-abstraction picker.
    pub engine: String,
    /// Path under `games/`.
    pub dir: String,
    pub kind: LaunchKind,
}

/// The four fleet games.
pub fn registry() -> Vec<GameEntry> {
    vec![
        GameEntry { id: "clawbound".into(), name: "Clawbound".into(), engine: "wgpu".into(), dir: "games/clawbound".into(), kind: LaunchKind::Cargo },
        GameEntry { id: "early-fps".into(), name: "Early FPS".into(), engine: "wgpu".into(), dir: "games/early-fps".into(), kind: LaunchKind::Cargo },
        GameEntry { id: "unendlich".into(), name: "Unendlich".into(), engine: "custom".into(), dir: "games/unendlich".into(), kind: LaunchKind::Cargo },
        GameEntry { id: "cybertron".into(), name: "Cybertron".into(), engine: "vulkan/opengl".into(), dir: "games/cybertron".into(), kind: LaunchKind::CmakeBin("cybertron".into()) },
    ]
}

/// The command line to launch a game (program first). Spawning is the caller's job.
pub fn launch_argv(g: &GameEntry) -> Vec<String> {
    match &g.kind {
        LaunchKind::Cargo => vec![
            "cargo".into(), "run".into(), "--release".into(),
            "--manifest-path".into(), format!("{}/Cargo.toml", g.dir),
        ],
        LaunchKind::CmakeBin(bin) => vec![format!("{}/build/{}", g.dir, bin)],
    }
}

/// A friend row for the centre's roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FriendRow {
    pub id: String,
    pub name: String,
    /// Human status: "offline" / "online" / "in Clawbound".
    pub status: String,
    /// True when they're in a joinable session (presence carries a session token).
    pub joinable: bool,
}

fn status_of(p: &Presence) -> String {
    match p {
        Presence::Offline => "offline".to_string(),
        Presence::Online => "online".to_string(),
        Presence::InGame { game, .. } => format!("in {game}"),
    }
}

/// Resolve a player's friends into roster rows via the profile service, sorted online-first.
pub fn friend_rows<P: ProfileBackend, F: ProfileBackend>(
    me: &Profile,
    svc: &netcode::store::ProfileService<P, F>,
) -> Vec<FriendRow> {
    let mut rows: Vec<FriendRow> = svc
        .friends_of(me)
        .iter()
        .map(|f| FriendRow {
            id: f.id.clone(),
            name: f.name.clone(),
            status: status_of(&f.presence),
            joinable: matches!(&f.presence, Presence::InGame { session: Some(_), .. }),
        })
        .collect();
    // online (and especially joinable) first
    rows.sort_by_key(|r| (r.status == "offline", !r.joinable, r.name.clone()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use netcode::microdht::{MicroDht, NodeId};
    use netcode::store::{MicroDhtStore, ProfileService};

    #[test]
    fn registry_has_the_four_games_with_launchers() {
        let r = registry();
        assert_eq!(r.len(), 4);
        // Rust games → cargo; cybertron → cmake bin
        let claw = r.iter().find(|g| g.id == "clawbound").unwrap();
        assert_eq!(launch_argv(claw), vec!["cargo", "run", "--release", "--manifest-path", "games/clawbound/Cargo.toml"]);
        let cyber = r.iter().find(|g| g.id == "cybertron").unwrap();
        assert_eq!(launch_argv(cyber), vec!["games/cybertron/build/cybertron"]);
    }

    fn svc() -> ProfileService<MicroDhtStore, MicroDhtStore> {
        ProfileService::new(
            MicroDhtStore::new(MicroDht::new(NodeId::from_key("p"), 20)),
            MicroDhtStore::new(MicroDht::new(NodeId::from_key("f"), 20)),
        )
    }

    #[test]
    fn friend_rows_sort_joinable_and_online_first() {
        let mut svc = svc();
        let mut bob = Profile::new("bob@1", "Bob");
        bob.presence = Presence::InGame { game: "Clawbound".into(), session: Some("l7".into()) };
        let mut amy = Profile::new("amy@2", "Amy");
        amy.presence = Presence::Online;
        let zed = Profile::new("zed@3", "Zed"); // Online by default... make offline
        let mut zed = zed;
        zed.presence = Presence::Offline;
        for p in [&bob, &amy, &zed] {
            svc.publish(p);
        }
        let mut me = Profile::new("me@0", "Me");
        me.friends = vec!["zed@3".into(), "amy@2".into(), "bob@1".into()];
        let rows = friend_rows(&me, &svc);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "Bob", "joinable in-game first");
        assert!(rows[0].joinable);
        assert_eq!(rows[0].status, "in Clawbound");
        assert_eq!(rows[2].name, "Zed", "offline last");
        assert_eq!(rows[2].status, "offline");
    }
}
