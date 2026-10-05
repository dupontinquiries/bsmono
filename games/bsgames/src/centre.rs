//! centre — the pure game-centre backend.
use netcode::profile::{Presence, Profile};
use netcode::store::{FileStore, ProfileBackend, ProfileService};

/// The hub's profile service: the SAME shared store every game publishes to.
///
/// `netcode::multiplayer::Multiplayer::new` (soldier, skydribble, blackstar-rs via shellkit) puts
/// a game's profile, presence and session record in `FileStore::dev_default_path()`. Until c21 the
/// hub built its service over two `SnapcoreStore`s -- a private in-process map -- so a friend
/// hosting a match in a game process was invisible to the centre that was supposed to show the
/// "Join" row, and the hub's own presence never reached any game. The OpenDHT primary still drops
/// into the first slot behind its feature; this is the floor both sides agree on.
pub type HubService = ProfileService<FileStore, FileStore>;

/// The hub's service over the machine-wide shared store (what the games use).
pub fn hub_service() -> HubService {
    hub_service_at(FileStore::dev_default_path())
}

/// Same, pointed at an explicit store file -- tests give each run its own path so they neither
/// pollute the real dev store nor see a game that happens to be running on the box.
pub fn hub_service_at(path: std::path::PathBuf) -> HubService {
    ProfileService::new(FileStore::at(&path), FileStore::at(&path))
}

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

/// The five fleet games.
pub fn registry() -> Vec<GameEntry> {
    vec![
        GameEntry { id: "clawbound".into(), name: "Clawbound".into(), engine: "wgpu".into(), dir: "games/clawbound".into(), kind: LaunchKind::Cargo },
        // The 5v5 arena for Clawbound's thirteen ancestors. Reads `--quick-match` (straight into a
        // match with a random ancestor) and the BSGAMES_HANDOFF_* env (logs the session, plays solo —
        // online play is not wired in it yet). Default features want libasound2-dev; the game's own
        // README covers `--no-default-features` for a silent build.
        GameEntry { id: "clawbound-battleground".into(), name: "Clawbound: Battleground".into(), engine: "wgpu".into(), dir: "games/clawbound-battleground".into(), kind: LaunchKind::Cargo },
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
    /// The raw game id (`GameEntry::id`, e.g. `"soldier"`) they're in, straight off
    /// `Presence::InGame`'s `game` field — `status` is for display, this is for dispatch (which
    /// game to spawn, and with which handoff env). `None` when offline/online-not-in-game.
    pub raw_game: Option<String>,
}

fn status_of(p: &Presence) -> String {
    match p {
        Presence::Offline => "offline".to_string(),
        Presence::Online => "online".to_string(),
        Presence::InGame { game, .. } => format!("in {game}"),
    }
}

fn raw_game_of(p: &Presence) -> Option<String> {
    match p {
        Presence::InGame { game, .. } => Some(game.clone()),
        _ => None,
    }
}

/// Which role the launched game should take on startup, per the handoff env protocol below.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HandoffRole {
    Host,
    Join,
}

/// The "skip the menu" handoff: environment variables set on the spawned game process so it can
/// jump straight into a match instead of sitting at its main menu — the piece that makes
/// bsgames→game "passing off" actually work across every game's *own* menu system, without each
/// game needing to know anything about bsgames beyond reading three env vars. A game with no
/// handoff env set (launched directly, standalone) behaves exactly as it does today — this is
/// additive, not a replacement for the game's normal menu-driven flow.
///
/// `BSGAMES_HANDOFF_ROLE`   — "host" or "join"
/// `BSGAMES_HANDOFF_PEER`   — the friend's id (join only — who to dial)
/// `BSGAMES_HANDOFF_ME`     — our own id + display name, `"<id>|<name>"` (so the launched game
///                            doesn't have to re-derive an identity separately from bsgames')
pub fn handoff_env(role: HandoffRole, me: &Profile, peer_id: Option<&str>) -> Vec<(String, String)> {
    let mut env = vec![
        ("BSGAMES_HANDOFF_ROLE".to_string(), match role { HandoffRole::Host => "host".to_string(), HandoffRole::Join => "join".to_string() }),
        ("BSGAMES_HANDOFF_ME".to_string(), format!("{}|{}", me.id, me.name)),
    ];
    if let (HandoffRole::Join, Some(id)) = (role, peer_id) {
        env.push(("BSGAMES_HANDOFF_PEER".to_string(), id.to_string()));
    }
    env
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
            raw_game: raw_game_of(&f.presence),
        })
        .collect();
    // online (and especially joinable) first
    rows.sort_by_key(|r| (r.status == "offline", !r.joinable, r.name.clone()));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    
    use netcode::session::{SessionExt, SessionRecord};
    use netcode::store::{ProfileService, SnapcoreStore};

    #[test]
    fn registry_has_the_five_games_with_launchers() {
        let r = registry();
        assert_eq!(r.len(), 5);
        let bg = r.iter().find(|g| g.id == "clawbound-battleground").unwrap();
        assert_eq!(launch_argv(bg), vec!["cargo", "run", "--release", "--manifest-path", "games/clawbound-battleground/Cargo.toml"]);
        assert_eq!(quick_match_argv(bg).last().map(String::as_str), Some("--quick-match"));
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

    /// Env var naming the store file for the child half of the two-process test below.
    const CHILD_STORE: &str = "BSGAMES_TEST_CHILD_STORE";

    /// The GAME half: runs in a separate process (spawned by the test below), exactly as soldier
    /// or skydribble would -- `Multiplayer::new` resolves to the same `at_store` constructor, with
    /// the path coming from the environment instead of `dev_default_path()`.
    #[test]
    #[ignore = "the child half of hub_sees_a_match_hosted_by_a_separate_game_process; its parent runs it"]
    fn child_game_process_hosts_a_match() {
        let path = std::path::PathBuf::from(std::env::var(CHILD_STORE).expect("parent sets the store path"));
        let mut game = netcode::multiplayer::Multiplayer::at_store("soldier", "bob@1", "Bob", path);
        game.host("127.0.0.1:0").expect("bind a loopback socket and announce");
        // the socket dies with this process; the announcement stays in the file, which is what the hub reads
    }

    /// The hub's `Service` and a game's `Multiplayer` must read one store. The game side runs in
    /// another PROCESS (this test binary re-exec'd on the ignored child above), because the bug
    /// this pins -- an in-process `SnapcoreStore` in the hub -- is invisible to any single-process
    /// test: within one process a shared map works perfectly.
    #[test]
    fn hub_sees_a_match_hosted_by_a_separate_game_process() {
        let path = std::env::temp_dir().join(format!("bsgames-two-process-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let exe = std::env::current_exe().expect("test binary path");
        let out = std::process::Command::new(&exe)
            .args(["--exact", "centre::tests::child_game_process_hosts_a_match", "--ignored", "--nocapture"])
            .env(CHILD_STORE, &path)
            .output()
            .expect("re-exec the test binary");
        assert!(out.status.success(), "child failed:\n{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(path.exists(), "the child must have written the shared store at {}", path.display());

        let hub = hub_service_at(path.clone());
        let mut me = Profile::new("me@0", "Me");
        me.friends.push("bob@1".into());
        let rows = friend_rows(&me, &hub);
        assert_eq!(rows.len(), 1, "Bob was published by the game process; the hub must resolve him: {rows:?}");
        assert_eq!(rows[0].name, "Bob");
        assert!(rows[0].joinable, "Bob is hosting; the row must offer Join: {:?}", rows[0]);
        assert_eq!(rows[0].raw_game.as_deref(), Some("soldier"));
        // and the other direction: the hub's own publish is what a game's friends list resolves
        let mut hub = hub;
        assert!(hub.publish(&me));
        let game = netcode::multiplayer::Multiplayer::at_store("soldier", "bob@1", "Bob", path.clone());
        let (found, _) = game.svc().lookup("me@0");
        assert_eq!(found.map(|p| p.name), Some("Me".to_string()), "a game must see the hub's profile in the same store");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn handoff_env_join_carries_role_peer_and_me() {
        let me = Profile::new("me@0", "Kessler");
        let env = handoff_env(HandoffRole::Join, &me, Some("bob@1"));
        assert_eq!(env.iter().find(|(k, _)| k == "BSGAMES_HANDOFF_ROLE").unwrap().1, "join");
        assert_eq!(env.iter().find(|(k, _)| k == "BSGAMES_HANDOFF_PEER").unwrap().1, "bob@1");
        assert_eq!(env.iter().find(|(k, _)| k == "BSGAMES_HANDOFF_ME").unwrap().1, "me@0|Kessler");
    }

    #[test]
    fn handoff_env_host_carries_no_peer() {
        let me = Profile::new("me@0", "Kessler");
        let env = handoff_env(HandoffRole::Host, &me, None);
        assert_eq!(env.iter().find(|(k, _)| k == "BSGAMES_HANDOFF_ROLE").unwrap().1, "host");
        assert!(env.iter().all(|(k, _)| k != "BSGAMES_HANDOFF_PEER"));
    }

    #[test]
    fn friend_rows_carry_the_raw_game_id_for_dispatch() {
        let mut svc = svc();
        let mut bob = Profile::new("bob@1", "Bob");
        let rec = SessionRecord::new("soldier", "bob@1", "udp", "127.0.0.1:1");
        svc.announce_session(&mut bob, "soldier@bob@1", &rec);
        let mut me = Profile::new("me@0", "Me");
        me.friends.push("bob@1".to_string());

        let rows = friend_rows(&me, &svc);
        let bob_row = rows.iter().find(|r| r.id == "bob@1").unwrap();
        assert_eq!(bob_row.raw_game.as_deref(), Some("soldier"));
        assert!(bob_row.status.contains("soldier"));
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
