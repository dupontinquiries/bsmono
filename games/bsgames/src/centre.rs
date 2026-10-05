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
    /// `cargo run --release --manifest-path <dir>/Cargo.toml` plus whatever [`CargoRun`] says the
    /// playable window needs (the Rust games).
    Cargo(CargoRun),
    /// A CMake-built binary at `<dir>/build/<name>` (cybertron).
    CmakeBin(String),
}

/// What `cargo run` needs beyond the manifest path to start the PLAYABLE binary. Every field
/// exists because a plain `cargo run --manifest-path` was refused or ran the wrong thing:
///
/// * soldier has four bins and no `default-run` (cargo: "could not determine which binary to
///   run"); unendlich's manifest is a virtual workspace with twenty-six -- so `bin`.
/// * soldier's window is behind `app`, unendlich-fullgame requires `ui-overlay`, neither default
///   (cargo: "requires the features: ..."), and clawbound without `app` is a headless render --
///   so `features`.
/// * clawbound's window is `-- --play`; and cargo rejects any game flag placed before `--`
///   (`--quick-match` was: "unexpected argument '--quick-match' found") -- so `args`, after `--`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CargoRun {
    /// `--bin NAME`, when the manifest has several binaries or is a workspace root.
    pub bin: Option<String>,
    /// `--features A,B` the playable binary requires that are not in the package defaults.
    pub features: Vec<String>,
    /// The game's own arguments, passed after `--`.
    pub args: Vec<String>,
}

impl CargoRun {
    /// `cargo run --release --manifest-path <dir>/Cargo.toml`: one binary, default features.
    pub fn plain() -> LaunchKind {
        LaunchKind::Cargo(CargoRun::default())
    }
    fn with(bin: Option<&str>, features: &[&str], args: &[&str]) -> LaunchKind {
        LaunchKind::Cargo(CargoRun {
            bin: bin.map(str::to_string),
            features: features.iter().map(|f| f.to_string()).collect(),
            args: args.iter().map(|a| a.to_string()).collect(),
        })
    }
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

/// The fleet games. `id` is the registry id a game publishes in its presence (`Multiplayer::new`'s
/// first argument) -- the hub's Join finds the game to spawn by it -- so it must match the game.
/// `registry_launches_resolve_against_the_real_manifests` checks every `Cargo` entry against the
/// manifest it names: the bin exists and its required features are covered.
pub fn registry() -> Vec<GameEntry> {
    vec![
        // The window is behind `app` and starts with `--play`; without both, `cargo run` is the
        // headless render-to-file path.
        GameEntry { id: "clawbound".into(), name: "Clawbound".into(), engine: "wgpu".into(), dir: "games/clawbound".into(), kind: CargoRun::with(None, &["app"], &["--play"]) },
        // The 5v5 arena for Clawbound's thirteen ancestors. Reads `--quick-match` (straight into a
        // match with a random ancestor) and the BSGAMES_HANDOFF_* env (logs the session, plays solo —
        // online play is not wired in it yet). Default features want libasound2-dev; the game's own
        // README covers `--no-default-features` for a silent build.
        GameEntry { id: "clawbound-battleground".into(), name: "Clawbound: Battleground".into(), engine: "wgpu".into(), dir: "games/clawbound-battleground".into(), kind: CargoRun::plain() },
        // Package `early-fps`, four bins; the eframe window is `early-fps` behind `app`.
        GameEntry { id: "soldier".into(), name: "Soldier".into(), engine: "wgpu".into(), dir: "games/soldier".into(), kind: CargoRun::with(Some("early-fps"), &["app"], &[]) },
        // A virtual workspace; the game is unendlich-engine's `unendlich-fullgame`, which
        // requires `ui-overlay` on top of the defaults (`tools/run-latest.sh` is the reference).
        GameEntry { id: "unendlich".into(), name: "Unendlich".into(), engine: "custom".into(), dir: "games/unendlich".into(), kind: CargoRun::with(Some("unendlich-fullgame"), &["ui-overlay"], &[]) },
        // The Rocket-League-style air-dribble playground; speaks the handoff env and `--quick-match`
        // (`games/skydribble/integration/README.md`).
        GameEntry { id: "skydribble".into(), name: "Skydribble".into(), engine: "wgpu".into(), dir: "games/skydribble".into(), kind: CargoRun::plain() },
        // The Rust port of Blackstar (package `blackstar`, one bin); same shell as skydribble.
        GameEntry { id: "blackstar-rs".into(), name: "Blackstar (Rust)".into(), engine: "wgpu".into(), dir: "games/blackstar-rs".into(), kind: CargoRun::plain() },
        // A workspace of `core` + `shell`; the game is the `shell` bin (eframe/wgpu, sound on by
        // default). Single-player: it ignores the handoff env and `--quick-match`.
        GameEntry { id: "road-warrior".into(), name: "Road Warrior".into(), engine: "wgpu".into(), dir: "games/road-warrior".into(), kind: CargoRun::with(Some("shell"), &[], &[]) },
        // The deterministic RTS. The native client is `echelon-deck`'s `play` verb, which
        // needs `window` (minifb) and wants `sound`; it plays a bot, or two decks over the LAN
        // with `--lan-host`/`--lan-join` (not the handoff env).
        GameEntry { id: "echelon".into(), name: "Echelon".into(), engine: "minifb".into(), dir: "games/echelon".into(), kind: CargoRun::with(Some("echelon-deck"), &["window", "sound"], &["play"]) },
        GameEntry { id: "blackstar".into(), name: "Blackstar (C++)".into(), engine: "vulkan/opengl".into(), dir: "games/blackstar".into(), kind: LaunchKind::CmakeBin("blackstar".into()) },
    ]
}

/// The directory a launch must run in: the monorepo root, found by walking up from `start`
/// (the hub's cwd, then its executable) to the first ancestor holding `games/bsgames/Cargo.toml`.
/// Every path in [`launch_argv`] is relative to it, so a hub started from `games/bsgames/`
/// (`cargo run` there) or from a desktop launcher with `$HOME` as cwd could not launch anything.
pub fn fleet_root_from(start: &std::path::Path) -> Option<std::path::PathBuf> {
    start.ancestors().find(|d| d.join("games/bsgames/Cargo.toml").is_file()).map(std::path::Path::to_path_buf)
}

/// [`fleet_root_from`] the cwd, then the executable's directory (a `target/release/bsgames`
/// under the root); `None` when neither is inside the monorepo.
pub fn fleet_root() -> Option<std::path::PathBuf> {
    std::env::current_dir()
        .ok()
        .and_then(|d| fleet_root_from(&d))
        .or_else(|| std::env::current_exe().ok().and_then(|e| fleet_root_from(&e)))
}

/// The command line to launch a game (program first; paths relative to the monorepo root, see
/// [`fleet_root`]). Spawning is the caller's job.
pub fn launch_argv(g: &GameEntry) -> Vec<String> {
    match &g.kind {
        LaunchKind::Cargo(run) => {
            let mut argv: Vec<String> = vec![
                "cargo".into(), "run".into(), "--release".into(),
                "--manifest-path".into(), format!("{}/Cargo.toml", g.dir),
            ];
            if let Some(bin) = &run.bin {
                argv.push("--bin".into());
                argv.push(bin.clone());
            }
            if !run.features.is_empty() {
                argv.push("--features".into());
                argv.push(run.features.join(","));
            }
            if !run.args.is_empty() {
                argv.push("--".into());
                argv.extend(run.args.iter().cloned());
            }
            argv
        }
        LaunchKind::CmakeBin(bin) => vec![format!("{}/build/{}", g.dir, bin)],
    }
}

/// The quick-match hook: same launch, plus a `--quick-match` flag for the game (skydribble,
/// blackstar-rs and clawbound-battleground read it and skip their menus; the others ignore it).
/// For a cargo launch the flag goes after `--`, because cargo itself rejects an unknown flag --
/// until c21 it was appended bare and `cargo run` refused every quick match with "unexpected
/// argument '--quick-match'". Separate from `launch_argv` so a game can diverge further later
/// (its own flag spelling, a distinct entry point) without disturbing the plain launch path.
pub fn quick_match_argv(g: &GameEntry) -> Vec<String> {
    let mut argv = launch_argv(g);
    if matches!(g.kind, LaunchKind::Cargo(_)) && !argv.iter().any(|a| a == "--") {
        argv.push("--".into());
    }
    argv.push("--quick-match".into());
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
    fn registry_has_the_nine_games_with_launchers() {
        let r = registry();
        assert_eq!(r.len(), 9);
        let bg = r.iter().find(|g| g.id == "clawbound-battleground").unwrap();
        assert_eq!(launch_argv(bg), vec!["cargo", "run", "--release", "--manifest-path", "games/clawbound-battleground/Cargo.toml"]);
        assert_eq!(quick_match_argv(bg), vec!["cargo", "run", "--release", "--manifest-path", "games/clawbound-battleground/Cargo.toml", "--", "--quick-match"]);
        // the window needs a feature and a flag of its own
        let claw = r.iter().find(|g| g.id == "clawbound").unwrap();
        assert_eq!(launch_argv(claw), vec!["cargo", "run", "--release", "--manifest-path", "games/clawbound/Cargo.toml", "--features", "app", "--", "--play"]);
        // several bins: name the playable one
        let soldier = r.iter().find(|g| g.id == "soldier").unwrap();
        assert_eq!(launch_argv(soldier), vec!["cargo", "run", "--release", "--manifest-path", "games/soldier/Cargo.toml", "--bin", "early-fps", "--features", "app"]);
        // a workspace: the bin names the member; a feature set and a verb for the deck
        let rw = r.iter().find(|g| g.id == "road-warrior").unwrap();
        assert_eq!(launch_argv(rw), vec!["cargo", "run", "--release", "--manifest-path", "games/road-warrior/Cargo.toml", "--bin", "shell"]);
        let ech = r.iter().find(|g| g.id == "echelon").unwrap();
        assert_eq!(launch_argv(ech), vec!["cargo", "run", "--release", "--manifest-path", "games/echelon/Cargo.toml", "--bin", "echelon-deck", "--features", "window,sound", "--", "play"]);
        // cybertron → cmake bin
        let cyber = r.iter().find(|g| g.id == "blackstar").unwrap();
        assert_eq!(launch_argv(cyber), vec!["games/blackstar/build/blackstar"]);
        assert_eq!(quick_match_argv(cyber), vec!["games/blackstar/build/blackstar", "--quick-match"]);
        // ids are unique: Join dispatches on them
        let mut ids: Vec<&str> = r.iter().map(|g| g.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), r.len());
    }

    #[test]
    fn fleet_root_is_the_ancestor_holding_this_crate() {
        let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let root = here.join("../..").canonicalize().unwrap();
        // from the crate dir, from a deep subdirectory, and from the root itself
        assert_eq!(fleet_root_from(here).unwrap().canonicalize().unwrap(), root);
        assert_eq!(fleet_root_from(&here.join("src/bin")).unwrap().canonicalize().unwrap(), root);
        assert_eq!(fleet_root_from(&root).unwrap().canonicalize().unwrap(), root);
        // a tree without the marker resolves to nothing rather than to `/`
        let tmp = std::env::temp_dir().join(format!("bsgames-root-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("games/other")).unwrap();
        assert_eq!(fleet_root_from(&tmp.join("games/other")), None);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn quick_match_flag_goes_to_the_game_not_to_cargo() {
        for g in registry() {
            let argv = quick_match_argv(&g);
            assert_eq!(argv.last().map(String::as_str), Some("--quick-match"), "{}", g.id);
            if matches!(g.kind, LaunchKind::Cargo(_)) {
                let sep = argv.iter().position(|a| a == "--").expect("a cargo launch separates game args");
                assert!(argv[..sep].iter().all(|a| a != "--quick-match"), "{}: cargo would reject it: {argv:?}", g.id);
                assert_eq!(argv.iter().filter(|a| *a == "--").count(), 1, "{}: one separator: {argv:?}", g.id);
            }
        }
    }

    /// Every `Cargo` entry against the manifest it names, via `cargo metadata --no-deps` (offline,
    /// ~50 ms even for unendlich's workspace): the manifest exists, the chosen bin (or the only
    /// bin, when none is chosen) exists, and its `required-features` are covered by the package
    /// defaults plus the entry's features, transitively. This is the check that `cargo run` does
    /// before building -- the one that refused soldier, unendlich and every quick match until c21.
    #[test]
    fn registry_launches_resolve_against_the_real_manifests() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        let mut checked = 0;
        for g in registry() {
            let LaunchKind::Cargo(run) = &g.kind else { continue };
            let manifest = root.join(&g.dir).join("Cargo.toml");
            assert!(manifest.is_file(), "{}: no manifest at {}", g.id, manifest.display());
            let out = std::process::Command::new("cargo")
                .args(["metadata", "--no-deps", "--format-version", "1", "--offline", "--manifest-path"])
                .arg(&manifest)
                .output()
                .expect("run cargo metadata");
            assert!(out.status.success(), "{}: cargo metadata failed: {}", g.id, String::from_utf8_lossy(&out.stderr));
            let meta: serde_json::Value = serde_json::from_slice(&out.stdout).expect("metadata is json");
            let packages = meta["packages"].as_array().expect("packages");
            // (package, target) pairs for every bin in the manifest's workspace
            let bins: Vec<(&serde_json::Value, &serde_json::Value)> = packages
                .iter()
                .flat_map(|p| p["targets"].as_array().unwrap().iter().map(move |t| (p, t)))
                .filter(|(_, t)| t["kind"].as_array().unwrap().iter().any(|k| k == "bin"))
                .collect();
            let (pkg, target) = match &run.bin {
                Some(name) => bins
                    .iter()
                    .find(|(_, t)| t["name"] == *name)
                    .copied()
                    .unwrap_or_else(|| panic!("{}: no bin `{name}` in {}", g.id, manifest.display())),
                None => {
                    // cargo's own rule: one bin, or the package's `default-run`
                    let default_run = packages.iter().find_map(|p| p["default_run"].as_str().map(str::to_string));
                    match default_run {
                        Some(name) => bins.iter().find(|(_, t)| t["name"] == *name).copied().unwrap_or_else(|| panic!("{}: default-run `{name}` is not a bin", g.id)),
                        None => {
                            assert_eq!(bins.len(), 1, "{}: {} bins, no `bin` chosen and no default-run -- cargo run would refuse: {:?}", g.id, bins.len(), bins.iter().map(|(_, t)| t["name"].to_string()).collect::<Vec<_>>());
                            bins[0]
                        }
                    }
                }
            };
            // feature closure: defaults + the entry's, following feature -> feature edges
            let table = pkg["features"].as_object().expect("features table");
            let mut have: Vec<String> = run.features.clone();
            have.push("default".into());
            let mut i = 0;
            while i < have.len() {
                if let Some(deps) = table.get(&have[i]).and_then(|v| v.as_array()) {
                    for d in deps {
                        let d = d.as_str().unwrap_or("");
                        let d = d.strip_prefix("dep:").unwrap_or(d);
                        let d = d.split('/').next().unwrap_or(d).trim_end_matches('?');
                        if !have.iter().any(|h| h == d) {
                            have.push(d.to_string());
                        }
                    }
                }
                i += 1;
            }
            if let Some(req) = target["required-features"].as_array() {
                for f in req {
                    let f = f.as_str().unwrap();
                    assert!(have.iter().any(|h| h == f), "{}: bin `{}` requires feature `{f}`; the launch enables {:?} (+defaults)", g.id, target["name"], run.features);
                }
            }
            checked += 1;
        }
        assert!(checked >= 6, "the scan must have covered the cargo games, not skipped them: {checked}");
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
