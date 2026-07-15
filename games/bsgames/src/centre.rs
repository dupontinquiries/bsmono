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
        GameEntry { id: "soldier".into(), name: "Soldier".into(), engine: "wgpu".into(), dir: "games/soldier".into(), kind: LaunchKind::Cargo },
        GameEntry { id: "unendlich".into(), name: "Unendlich".into(), engine: "custom".into(), dir: "games/unendlich".into(), kind: LaunchKind::Cargo },
        GameEntry { id: "blackstar".into(), name: "Blackstar".into(), engine: "vulkan/opengl".into(), dir: "games/blackstar".into(), kind: LaunchKind::CmakeBin("blackstar".into()) },
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

/// The quick-match hook: same launch, plus a `--quick-match` flag each game can read to skip
/// straight into a simple match instead of the main menu. Today every game just ignores the flag
/// (none read it yet — this is the seam the per-game menu work builds on, one game at a time; see
/// `games/soldier/src/multiplayer.rs` for the first real online-play wiring it'll eventually hook
/// into). Separate from `launch_argv` so a game can diverge further later (its own binary flag
/// spelling, a distinct quick-match entry point, etc.) without disturbing the plain launch path.
pub fn quick_match_argv(g: &GameEntry) -> Vec<String> {
    let mut argv = launch_argv(g);
    match &g.kind {
        LaunchKind::Cargo => argv.push("--quick-match".into()),
        LaunchKind::CmakeBin(_) => argv.push("--quick-match".into()),
    }
    argv
}

/// Add a friend by id: looks them up first (so a typo'd/unknown id gives a clear error instead of
/// silently growing the friends list with a dead entry), appends if not already a friend, and
/// republishes `me`. Returns the friend's display name on success.
pub fn add_friend<P: ProfileBackend, F: ProfileBackend>(
    me: &mut Profile,
    svc: &mut netcode::store::ProfileService<P, F>,
    friend_id: &str,
) -> Result<String, String> {
    let friend_id = friend_id.trim();
    if friend_id.is_empty() {
        return Err("enter a friend's id first".into());
    }
    if friend_id == me.id {
        return Err("that's your own id".into());
    }
    let (found, _reach) = svc.lookup(friend_id);
    let Some(friend) = found else {
        return Err(format!("no one is published under \"{friend_id}\" yet"));
    };
    if me.friends.iter().any(|f| f == friend_id) {
        return Err(format!("{} is already a friend", friend.name));
    }
    me.friends.push(friend_id.to_string());
    svc.publish(me);
    Ok(friend.name)
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
    
    use netcode::store::{ProfileService, SnapcoreStore};

    #[test]
    fn registry_has_the_four_games_with_launchers() {
        let r = registry();
        assert_eq!(r.len(), 4);
        // Rust games → cargo; cybertron → cmake bin
        let claw = r.iter().find(|g| g.id == "clawbound").unwrap();
        assert_eq!(launch_argv(claw), vec!["cargo", "run", "--release", "--manifest-path", "games/clawbound/Cargo.toml"]);
        let cyber = r.iter().find(|g| g.id == "blackstar").unwrap();
        assert_eq!(launch_argv(cyber), vec!["games/blackstar/build/blackstar"]);
    }

    #[test]
    fn quick_match_argv_appends_the_flag_to_the_plain_launch() {
        let r = registry();
        let claw = r.iter().find(|g| g.id == "clawbound").unwrap();
        let mut want = launch_argv(claw);
        want.push("--quick-match".into());
        assert_eq!(quick_match_argv(claw), want);
    }

    #[test]
    fn add_friend_requires_the_id_to_be_published() {
        let mut svc = svc();
        let mut me = Profile::new("me@0", "Me");
        let err = add_friend(&mut me, &mut svc, "nobody@1").unwrap_err();
        assert!(err.contains("nobody@1"));
        assert!(me.friends.is_empty());
    }

    #[test]
    fn add_friend_rejects_your_own_id_and_blank_input() {
        let mut svc = svc();
        let mut me = Profile::new("me@0", "Me");
        assert!(add_friend(&mut me, &mut svc, "me@0").is_err());
        assert!(add_friend(&mut me, &mut svc, "  ").is_err());
    }

    #[test]
    fn add_friend_succeeds_once_and_rejects_the_duplicate() {
        let mut svc = svc();
        let bob = Profile::new("bob@1", "Bob");
        svc.publish(&bob);
        let mut me = Profile::new("me@0", "Me");

        let name = add_friend(&mut me, &mut svc, "bob@1").expect("bob is published");
        assert_eq!(name, "Bob");
        assert_eq!(me.friends, vec!["bob@1".to_string()]);

        let err = add_friend(&mut me, &mut svc, "bob@1").unwrap_err();
        assert!(err.contains("already a friend"));
        assert_eq!(me.friends.len(), 1, "must not add bob twice");
    }

    fn svc() -> ProfileService<SnapcoreStore, SnapcoreStore> {
        ProfileService::new(SnapcoreStore::new(), SnapcoreStore::new())
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
