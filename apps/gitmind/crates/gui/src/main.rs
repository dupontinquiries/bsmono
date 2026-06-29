// gitmind-gui/src/main.rs
//
// The FFI bridge between the shared gitmind-core (the exact same code the
// CLI runs) and the QML touch UI. Following the web2qml reference: a single
// `Backend` QObject is registered as the `backend` context property, QML is
// embedded with qrc! for a self-contained binary, and the engine loads
// qrc:/qml/main.qml.
//
// Methods return JSON strings (QString). Passing a JSON blob across the
// Rust↔QML boundary and parsing it in QML (JSON.parse) is far simpler than
// marshalling custom list models, and it keeps the bridge surface tiny:
// every screen is "call a *_json method, render the result".
//
// Because every method goes through gitmind-core, the GUI and CLI can never
// drift — a fix in the core fixes both frontends at once.

use cstr::cstr;
use qmetaobject::prelude::*;
use serde_json::json;
use std::path::PathBuf;

use gitmind_core::backend::{GitCli, RepoBackend};
use gitmind_core::checkpoint::CheckpointEngine;
use gitmind_core::native;
use gitmind_core::query;
use gitmind_core::store::ChunkStore;
use gitmind_core::watch::WatchPolicy;
use gitmind_core::watch_session::WatchSession;

// Embed the QML into the binary so the shipped GUI has no external files.
mod deploy;
mod browse;
mod netshare;
mod bttransfer;
mod appcatalog;
mod buildflags;
mod fleet;

qrc!(register_qml,
    "/" {
        "qml/main.qml",
        "qml/GestureHost.qml",
        "qml/FileRow.qml",
        "qml/Timeline.qml",
        "qml/Branches.qml",
        "qml/PeekViewer.qml",
        "qml/Apps.qml",
        "qml/Fleet.qml",
        "qml/Files.qml",
        "qml/Network.qml",
    },
);

#[derive(QObject, Default)]
struct Backend {
    base: qt_base_class!(trait QObject),

    // Repo / status.
    repo_root: qt_method!(fn(&self) -> QString),
    status_json: qt_method!(fn(&self) -> QString),

    // Timeline / checkpoints.
    timeline_json: qt_method!(fn(&self, n: u32) -> QString),
    snapshot: qt_method!(fn(&self, label: QString) -> QString),
    checkpoint_diff_json: qt_method!(fn(&self, from: QString, to: QString) -> QString),
    checkpoint_extract: qt_method!(fn(&self, oid: QString, dir: QString) -> QString),

    // Search by author/language.
    search_json: qt_method!(fn(&self, author: QString, lang: QString) -> QString),

    // Touch staging (swipe gestures call these).
    stage: qt_method!(fn(&self, path: QString) -> QString),
    unstage: qt_method!(fn(&self, path: QString) -> QString),

    // File viewing via the peek engine.
    peek_file: qt_method!(fn(&self, path: QString, spec: QString, context: u32) -> QString),

    // Ingest into the .gitmind chunk store.
    ingest: qt_method!(fn(&self, path: QString) -> QString),

    // Realtime time-machine controls. The watcher runs on a background
    // thread; QML drains new checkpoints on a timer.
    watch_start: qt_method!(fn(&self, fanotify: bool, poll: bool) -> QString),
    watch_stop: qt_method!(fn(&self) -> QString),
    watch_drain: qt_method!(fn(&self) -> QString),
    watch_running: qt_method!(fn(&self) -> bool),

    // Native VCS (git-free) for the touch UI.
    branches_json: qt_method!(fn(&self) -> QString),
    branch_create: qt_method!(fn(&self, name: QString) -> QString),
    branch_delete: qt_method!(fn(&self, name: QString) -> QString),
    branch_switch: qt_method!(fn(&self, name: QString) -> QString),
    cherry_pick: qt_method!(fn(&self, oid: QString) -> QString),
    rebase: qt_method!(fn(&self, branch: QString, onto: QString) -> QString),
    merge: qt_method!(fn(&self, branch: QString) -> QString),
    commit_patch: qt_method!(fn(&self, oid: QString) -> QString),
    apply_patch: qt_method!(fn(&self, text: QString) -> QString),
    native_commit: qt_method!(fn(&self, message: QString) -> QString),
    native_log_json: qt_method!(fn(&self, n: u32) -> QString),
    native_status_json: qt_method!(fn(&self) -> QString),

    run_check: qt_method!(fn(&self, commit: QString, cmd: QString) -> QString),
    checks_json: qt_method!(fn(&self) -> QString),

    apps_json: qt_method!(fn(&self) -> QString),
    app_use: qt_method!(fn(&self, name: QString, label: QString) -> QString),
    app_desktop: qt_method!(fn(&self, name: QString, label: QString, terminal: bool) -> QString),
    app_remove: qt_method!(fn(&self, name: QString, label: QString) -> QString),
    app_install: qt_method!(fn(&self, name: QString, path: QString, label: QString, commit: QString) -> QString),

    // ── files: browse the repo working tree ──
    list_dir: qt_method!(fn(&self, subpath: QString) -> QString),

    // ── deploy: discover SSH devices, browse + push the repo, remote build + .desktop ──
    // The slow ops run off-thread (see impls); the QML drives them via poll_job().
    start_discover: qt_method!(fn(&self, subnet: QString) -> QString),
    list_remote_dirs: qt_method!(fn(&self, user: QString, host: QString, path: QString) -> QString),
    start_push: qt_method!(fn(&self, user: QString, host: QString, dir: QString) -> QString),
    start_deploy: qt_method!(fn(&self, user: QString, host: QString, dir: QString, app: QString) -> QString),
    poll_job: qt_method!(fn(&self) -> QString),
    // SSH identity + host-key trust, surfaced so the GUI never assumes a username
    // and never trusts a key in the terminal behind the GUI's back.
    current_user: qt_method!(fn(&self) -> QString),
    // managed fleet user (a dedicated service account, not your login)
    fleet_user: qt_method!(fn(&self) -> QString),
    set_fleet_user: qt_method!(fn(&self, name: QString) -> QString),
    start_provision_fleet_user: qt_method!(fn(&self, admin: QString, host: QString, password: QString) -> QString),
    host_trusted: qt_method!(fn(&self, host: QString) -> QString),
    start_trust: qt_method!(fn(&self, host: QString) -> QString),
    start_retrust: qt_method!(fn(&self, host: QString) -> QString),
    start_browse: qt_method!(fn(&self, user: QString, host: QString, path: QString) -> QString),
    app_details: qt_method!(fn(&self, user: QString, host: QString, path: QString) -> QString),
    // build-flag customization + the cross-build → ship pipeline
    app_build_options: qt_method!(fn(&self, user: QString, host: QString, path: QString) -> QString),
    // app-store catalog + this device's native target
    store_catalog: qt_method!(fn(&self, user: QString, host: QString, root: QString) -> QString),
    local_target: qt_method!(fn(&self) -> QString),
    // app pages over gitmind's managed repos: list local buildable crates, scan a
    // crate's flags, build it (with status), send only the built binary to a device.
    local_apps: qt_method!(fn(&self) -> QString),
    local_build_options: qt_method!(fn(&self, path: QString) -> QString),
    build_app: qt_method!(fn(&self, app: QString, target: QString, features: QString, no_default: bool) -> QString),
    send_binary: qt_method!(fn(&self, binary: QString, user: QString, host: QString, dest: QString) -> QString),
    install_app: qt_method!(fn(&self, user: QString, host: QString, path: QString, app: QString, triple: QString, features: QString, no_default: bool, stage_tmp: bool) -> QString),
    start_setup_key: qt_method!(fn(&self, user: QString, host: QString, password: QString) -> QString),
    // two-sided fingerprint matching: show this device's key, preview a peer's key,
    // and trust exactly the key the user compared on screen.
    local_fingerprint: qt_method!(fn(&self) -> QString),
    start_preview: qt_method!(fn(&self, host: QString) -> QString),
    trust_previewed_key: qt_method!(fn(&self, host: QString, keys: QString) -> QString),
    // bluetooth file send (OBEX) + a bundle helper to give it something to send.
    bluetooth_devices: qt_method!(fn(&self) -> QString),
    start_bluetooth_send: qt_method!(fn(&self, addr: QString, file: QString) -> QString),
    make_bundle: qt_method!(fn(&self, dest: QString) -> QString),

    // ── network: advertise this repo's card, look up + install a peer's ──
    advertise_start: qt_method!(fn(&self, port: QString) -> QString),
    advertise_stop: qt_method!(fn(&self) -> QString),
    advertise_status: qt_method!(fn(&self) -> QString),
    lookup_peer: qt_method!(fn(&self, addr: QString) -> QString),
    start_install: qt_method!(fn(&self, name: QString, provider: QString, dest: QString) -> QString),

    session: WatchSession,
    // Shared state for the one in-flight background deploy job.
    job: std::sync::Arc<deploy::Job>,
    // HTTP advertiser state (serving this repo's card to peers).
    advertiser: std::sync::Arc<netshare::Advertiser>,
}

/// Open the repo from the process working directory (the GUI may be launched
/// from a file manager pointing at a project, or chdir'd by a wrapper).
fn repo() -> Result<GitCli, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    GitCli::discover(&cwd).ok_or_else(|| "not inside a git repository".to_string())
}

fn err_json(e: &str) -> QString {
    QString::from(json!({"ok": false, "error": e}).to_string())
}

/// Mark a freshly-installed binary executable (best-effort; Unix perms).
fn set_executable(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}

/// Create the fleet user on a device and install the restricted key into it, over
/// ssh as the sudo-capable `admin` account. On-device (useradd/adduser + sudo).
fn provision_fleet_user(admin: &str, host: &str, password: &str) -> Result<String, String> {
    let user = fleet::fleet_user();
    let pubkey = deploy::ensure_local_pubkey()?;
    let key_line = deploy::restricted_key_line(&deploy::authorized_keys_options(host), &pubkey);
    let script = fleet::provision_user_script(&user, &key_line);
    let remote = fleet::provision_remote_command(&fleet::base64_encode(script.as_bytes()));
    let out = deploy::ssh_sudo_run(admin, host, password, &remote, &format!("{password}\n"))?;
    if out.contains("gitmind-fleet-user-ready") {
        Ok(format!("created fleet user '{user}' on {host} — set the user to '{user}' to connect"))
    } else {
        Err("provisioning didn't confirm — does that account have sudo rights?".into())
    }
}

/// Open (or create) the native store for the current repo.
fn gstore() -> Result<ChunkStore, String> {
    let r = repo()?;
    let dir = gitmind_core::portage::native_store_dir(r.root());
    std::fs::create_dir_all(&dir).map_err(|_| "cannot create .gitmind store dir".to_string())?;
    ChunkStore::open(dir.join("store.redb"))
}

/// Author identity from git config, with gitmind fallbacks.
fn identity() -> (String, String) {
    let r = match repo() { Ok(r) => r, Err(_) => return ("gitmind".into(), "gitmind@local".into()) };
    let name = r.git(&["config", "user.name"]).ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gitmind".to_string());
    let email = r.git(&["config", "user.email"]).ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gitmind@local".to_string());
    (name, email)
}

/// Check out whatever HEAD points at, to sync the worktree after a store-only op.
fn sync_worktree(store: &ChunkStore) {
    if let (Ok(refs), Ok(r)) = (store.refs(), repo()) {
        if let Some((_, target)) = refs.iter().find(|(n, _)| n == "HEAD") {
            let t = target.strip_prefix("refs/heads/").unwrap_or(target);
            let _ = native::checkout(store, t, r.root());
        }
    }
}

impl Backend {
    fn repo_root(&self) -> QString {
        match repo() {
            Ok(r) => QString::from(r.root().display().to_string()),
            Err(_) => QString::from(""),
        }
    }

    fn status_json(&self) -> QString {
        let r = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        let st = match r.status() {
            Ok(s) => s,
            Err(e) => return err_json(&e),
        };
        let files: Vec<_> = st
            .files
            .iter()
            .map(|f| {
                json!({
                    "path": f.path, "xy": f.xy, "staged": f.staged,
                    "untracked": f.untracked, "conflicted": f.conflicted
                })
            })
            .collect();
        QString::from(
            json!({
                "ok": true, "branch": st.branch, "detached": st.detached,
                "ahead": st.ahead, "behind": st.behind, "hasUpstream": st.has_upstream,
                "files": files
            })
            .to_string(),
        )
    }

    fn timeline_json(&self, n: u32) -> QString {
        let r = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        let eng = CheckpointEngine::new(&r, "main");
        match eng.list(n as usize) {
            Ok(list) => {
                let items: Vec<_> = list
                    .iter()
                    .map(|c| json!({"oid": c.oid, "label": c.label, "when": c.when}))
                    .collect();
                QString::from(json!({"ok": true, "items": items}).to_string())
            }
            Err(e) => err_json(&e),
        }
    }

    fn snapshot(&self, label: QString) -> QString {
        let r = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        let eng = CheckpointEngine::new(&r, "main");
        let label = label.to_string();
        let label = if label.is_empty() { "checkpoint".to_string() } else { label };
        match eng.snapshot(&label) {
            Ok(Some(c)) => QString::from(json!({"ok": true, "oid": c.oid, "label": c.label}).to_string()),
            Ok(None) => QString::from(json!({"ok": true, "noChange": true}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn checkpoint_diff_json(&self, from: QString, to: QString) -> QString {
        let r = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        let eng = CheckpointEngine::new(&r, "main");
        let t = to.to_string();
        let to_opt = if t.is_empty() { None } else { Some(t.as_str()) };
        match eng.diff(&from.to_string(), to_opt) {
            Ok(deltas) => {
                let items: Vec<_> = deltas
                    .iter()
                    .map(|d| json!({"path": d.path, "added": d.added, "removed": d.removed}))
                    .collect();
                QString::from(json!({"ok": true, "items": items}).to_string())
            }
            Err(e) => err_json(&e),
        }
    }

    fn checkpoint_extract(&self, oid: QString, dir: QString) -> QString {
        let r = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        let eng = CheckpointEngine::new(&r, "main");
        match eng.extract(&oid.to_string(), std::path::Path::new(&dir.to_string())) {
            Ok(n) => QString::from(json!({"ok": true, "files": n}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn search_json(&self, author: QString, lang: QString) -> QString {
        let r = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        let a = author.to_string();
        let l = lang.to_string();
        let author = if a.is_empty() { None } else { Some(a.as_str()) };
        let lang = if l.is_empty() { None } else { Some(l.as_str()) };
        match query::search(&r, author, lang, 200) {
            Ok(hits) => {
                let items: Vec<_> = hits
                    .iter()
                    .map(|h| json!({"oid": h.oid, "author": h.author, "summary": h.summary, "date": h.date_unix}))
                    .collect();
                QString::from(json!({"ok": true, "items": items}).to_string())
            }
            Err(e) => err_json(&e),
        }
    }

    fn stage(&self, path: QString) -> QString {
        let r = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        match r.stage(&path.to_string()) {
            Ok(()) => QString::from(json!({"ok": true}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn unstage(&self, path: QString) -> QString {
        let r = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        match r.unstage(&path.to_string()) {
            Ok(()) => QString::from(json!({"ok": true}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn peek_file(&self, path: QString, spec: QString, context: u32) -> QString {
        let p = PathBuf::from(path.to_string());
        match peek::view::read_to_string(&p, &spec.to_string(), context as usize, true) {
            Ok(text) => QString::from(json!({"ok": true, "text": text}).to_string()),
            Err(e) => err_json(&e.to_string()),
        }
    }

    fn ingest(&self, path: QString) -> QString {
        let r = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        let dir = gitmind_core::portage::native_store_dir(r.root());
        if std::fs::create_dir_all(&dir).is_err() {
            return err_json("cannot create .gitmind store dir");
        }
        let store = match ChunkStore::open(dir.join("store.redb")) {
            Ok(s) => s,
            Err(e) => return err_json(&e),
        };
        let pstr = path.to_string();
        let file = match std::fs::File::open(&pstr) {
            Ok(f) => f,
            Err(e) => return err_json(&e.to_string()),
        };
        let name = PathBuf::from(&pstr)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or(pstr);
        match store.ingest(&name, file) {
            Ok(rep) => QString::from(
                json!({
                    "ok": true, "oid": rep.blob_oid.to_hex(), "totalLen": rep.total_len,
                    "newChunks": rep.new_chunks, "media": format!("{:?}", rep.media)
                })
                .to_string(),
            ),
            Err(e) => err_json(&e),
        }
    }

    /// Start the realtime watcher on a background thread. `fanotify` selects
    /// the privileged PID-attributing backend (failures surface via drain so
    /// the UI can retry without it); `poll` forces the polling backend.
    fn watch_start(&self, fanotify: bool, poll: bool) -> QString {
        let repo = match repo() {
            Ok(r) => r,
            Err(e) => return err_json(&e),
        };
        if self.session.running() {
            return QString::from(json!({"ok": true, "already": true}).to_string());
        }
        self.session.start(
            repo.root().to_path_buf(),
            WatchPolicy::default(),
            "main".to_string(),
            fanotify,
            poll,
        );
        QString::from(json!({"ok": true, "started": true}).to_string())
    }

    fn watch_stop(&self) -> QString {
        self.session.stop();
        QString::from(json!({"ok": true}).to_string())
    }

    /// Drain checkpoints recorded since the last call (for a QML timer).
    fn watch_drain(&self) -> QString {
        let d = self.session.drain();
        let items: Vec<_> = d
            .items
            .iter()
            .map(|(oid, label)| json!({"oid": oid, "label": label}))
            .collect();
        QString::from(json!({"running": d.running, "items": items}).to_string())
    }

    fn watch_running(&self) -> bool {
        self.session.running()
    }

    // --- Native VCS methods ---

    fn branches_json(&self) -> QString {
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        match native::branch_list(&store) {
            Ok(bs) => {
                let items: Vec<_> = bs.iter()
                    .map(|b| json!({"name": b.name, "oid": b.oid, "head": b.is_head}))
                    .collect();
                QString::from(json!({"ok": true, "items": items}).to_string())
            }
            Err(e) => err_json(&e),
        }
    }

    fn branch_create(&self, name: QString) -> QString {
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        match native::branch_create(&store, &name.to_string(), None) {
            Ok(oid) => QString::from(json!({"ok": true, "oid": oid}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn branch_delete(&self, name: QString) -> QString {
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        match native::branch_delete(&store, &name.to_string()) {
            Ok(()) => QString::from(json!({"ok": true}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn branch_switch(&self, name: QString) -> QString {
        let r = match repo() { Ok(r) => r, Err(e) => return err_json(&e) };
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        match native::checkout(&store, &name.to_string(), r.root()) {
            Ok(rep) => QString::from(
                json!({"ok": true, "commit": rep.commit, "written": rep.written, "removed": rep.removed}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn cherry_pick(&self, oid: QString) -> QString {
        use gitmind_core::native::PickOutcome;
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        let (name, email) = identity();
        match native::cherry_pick(&store, &oid.to_string(), &name, &email) {
            Ok(PickOutcome::Committed(c)) => {
                sync_worktree(&store);
                QString::from(json!({"ok": true, "outcome": "committed", "oid": c.oid}).to_string())
            }
            Ok(PickOutcome::Empty) =>
                QString::from(json!({"ok": true, "outcome": "empty"}).to_string()),
            Ok(PickOutcome::Conflicts(paths)) =>
                QString::from(json!({"ok": true, "outcome": "conflicts", "paths": paths}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn native_commit(&self, message: QString) -> QString {
        let r = match repo() { Ok(r) => r, Err(e) => return err_json(&e) };
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        let (name, email) = identity();
        let branch = store.refs().ok().and_then(|refs| {
            refs.into_iter().find(|(n, _)| n == "HEAD")
                .and_then(|(_, t)| t.strip_prefix("refs/heads/").map(|s| s.to_string()))
        }).unwrap_or_else(|| "main".to_string());
        let msg = message.to_string();
        let msg = if msg.is_empty() { "native commit".to_string() } else { msg };
        match native::commit_worktree(&store, r.root(), &branch, &name, &email, &msg) {
            Ok(c) => QString::from(json!({"ok": true, "oid": c.oid, "tree": c.tree, "branch": branch}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn native_log_json(&self, n: u32) -> QString {
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        let mut cur = match native::resolve_head(&store) {
            Ok(c) => c,
            Err(e) => return err_json(&e),
        };
        let mut items = Vec::new();
        let mut count = 0;
        while let Some(oid) = cur {
            if count >= n { break; }
            match gitmind_core::portage::native_commit(&store, &oid) {
                Ok(meta) => {
                    let subject = meta.message.lines().next().unwrap_or("").to_string();
                    let st = gitmind_core::checks::get(&store, &oid).ok().flatten();
                    let mark = gitmind_core::checks::marker(st.as_ref());
                    items.push(json!({
                        "oid": oid, "subject": subject, "check": mark,
                        "checked": st.is_some(), "ok": st.map(|s| s.ok),
                    }));
                    cur = meta.parents.first().cloned();
                    count += 1;
                }
                Err(e) => return err_json(&e),
            }
        }
        QString::from(json!({"ok": true, "items": items}).to_string())
    }

    fn native_status_json(&self) -> QString {
        let r = match repo() { Ok(r) => r, Err(e) => return err_json(&e) };
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        match native::status(&store, r.root()) {
            Ok(changes) => {
                let items: Vec<_> = changes.iter()
                    .map(|c| json!({"sigil": c.kind.sigil().to_string(), "path": c.path}))
                    .collect();
                QString::from(json!({"ok": true, "items": items}).to_string())
            }
            Err(e) => err_json(&e),
        }
    }

    fn run_check(&self, commit: QString, cmd: QString) -> QString {
        use gitmind_core::checks;
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        let cmd_s = cmd.to_string();
        let parts: Vec<String> = if cmd_s.trim().is_empty() {
            checks::default_cmd()
        } else {
            cmd_s.split_whitespace().map(|s| s.to_string()).collect()
        };
        match checks::run(&store, &commit.to_string(), &parts) {
            Ok(st) => QString::from(json!({"ok": true, "passed": st.ok, "code": st.code, "duration_ms": st.duration_ms}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn checks_json(&self) -> QString {
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        match store.list_checks() {
            Ok(rows) => {
                let items: Vec<_> = rows.iter().filter_map(|(commit, bytes)| {
                    serde_json::from_slice::<gitmind_core::checks::CheckStatus>(bytes).ok().map(|s| json!({
                        "commit": commit, "ok": s.ok, "code": s.code,
                        "duration_ms": s.duration_ms, "ran_at": s.ran_at,
                    }))
                }).collect();
                QString::from(json!({"ok": true, "items": items}).to_string())
            }
            Err(e) => err_json(&e),
        }
    }

    fn apps_json(&self) -> QString {
        use gitmind_core::binpin;
        let apps: Vec<_> = binpin::list_apps().into_iter().map(|name| {
            let m = binpin::load_manifest(&name);
            let versions: Vec<_> = m.versions.iter().map(|v| json!({
                "label": v.label, "commit": v.commit, "target": v.target,
                "installed": v.installed, "path": v.path,
                "current": m.current.as_deref() == Some(&v.label),
            })).collect();
            json!({"name": name, "current": m.current, "versions": versions})
        }).collect();
        QString::from(json!({"ok": true, "apps": apps}).to_string())
    }

    fn app_use(&self, name: QString, label: QString) -> QString {
        use gitmind_core::binpin;
        match binpin::use_version(&name.to_string(), &label.to_string()) {
            Ok(()) => QString::from(json!({"ok": true}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn app_desktop(&self, name: QString, label: QString, terminal: bool) -> QString {
        use gitmind_core::binpin;
        match binpin::write_desktop(&name.to_string(), &label.to_string(), terminal, None) {
            Ok(p) => QString::from(json!({"ok": true, "path": p.to_string_lossy()}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn app_remove(&self, name: QString, label: QString) -> QString {
        use gitmind_core::binpin;
        match binpin::remove(&name.to_string(), &label.to_string()) {
            Ok(()) => QString::from(json!({"ok": true}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn app_install(&self, name: QString, path: QString, label: QString, commit: QString) -> QString {
        use gitmind_core::{binpin, varcache};
        let bytes = match std::fs::read(path.to_string()) { Ok(b) => b, Err(e) => return err_json(&e.to_string()) };
        let store = match varcache::open_var_store() { Ok(s) => s, Err(e) => return err_json(&e) };
        let commit = commit.to_string();
        let commit = if commit.is_empty() { "manual".to_string() } else { commit };
        match binpin::install(&store, &name.to_string(), &label.to_string(), &commit, "host", &bytes, true) {
            Ok(v) => QString::from(json!({"ok": true, "path": v.path, "oid": v.oid}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn rebase(&self, branch: QString, onto: QString) -> QString {
        use gitmind_core::native::RebaseOutcome;
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        let (name, email) = identity();
        match native::rebase(&store, &branch.to_string(), &onto.to_string(), &name, &email) {
            Ok(RebaseOutcome::Rebased { new_tip, commits }) => {
                sync_worktree(&store);
                QString::from(json!({"ok": true, "outcome": "rebased", "new_tip": new_tip, "commits": commits}).to_string())
            }
            Ok(RebaseOutcome::UpToDate) =>
                QString::from(json!({"ok": true, "outcome": "uptodate"}).to_string()),
            Ok(RebaseOutcome::Conflicts { commit, paths }) =>
                QString::from(json!({"ok": true, "outcome": "conflicts", "commit": commit, "paths": paths}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn commit_patch(&self, oid: QString) -> QString {
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        match native::format_commit_patch(&store, &oid.to_string()) {
            Ok(p) => QString::from(json!({"ok": true, "patch": p}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn merge(&self, branch: QString) -> QString {
        use gitmind_core::native::MergeOutcome;
        let r = match repo() { Ok(r) => r, Err(e) => return err_json(&e) };
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        let (name, email) = identity();
        match native::merge_into_worktree(&store, &branch.to_string(), r.root(), &name, &email) {
            Ok(MergeOutcome::UpToDate) =>
                QString::from(json!({"ok": true, "outcome": "uptodate"}).to_string()),
            Ok(MergeOutcome::FastForward { new_tip }) =>
                QString::from(json!({"ok": true, "outcome": "fastforward", "new_tip": new_tip}).to_string()),
            Ok(MergeOutcome::Merged { commit }) =>
                QString::from(json!({"ok": true, "outcome": "merged", "commit": commit}).to_string()),
            Ok(MergeOutcome::Conflicts(paths)) =>
                QString::from(json!({"ok": true, "outcome": "conflicts", "paths": paths}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    fn apply_patch(&self, text: QString) -> QString {
        let r = match repo() { Ok(r) => r, Err(e) => return err_json(&e) };
        match native::apply_patch_to_dir(&text.to_string(), r.root()) {
            Ok(n) => QString::from(json!({"ok": true, "applied": n}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    /// List one directory of the repo working tree for the Files browser.
    /// `subpath` is repo-relative (empty = root). Skips .git/target/node_modules.
    fn list_dir(&self, subpath: QString) -> QString {
        let r = match repo() { Ok(r) => r, Err(e) => return err_json(&e) };
        match browse::list_repo_dir(r.root(), &subpath.to_string()) {
            Ok((rel, dirs, files)) => {
                QString::from(json!({"ok": true, "path": rel, "dirs": dirs, "files": files}).to_string())
            }
            Err(e) => err_json(&e),
        }
    }

    // ── deploy ───────────────────────────────────────────────────────────
    // The slow operations run on a worker thread so the GUI never freezes. Each
    // `start_*` claims the shared Job, spawns the work, and returns immediately;
    // the QML polls `poll_job()` on a timer and renders the result when it lands.

    /// Start an LAN scan for SSH hosts on a worker thread. `subnet` empty =
    /// auto-detect the local /24. Returns immediately; poll for the device list.
    fn start_discover(&self, subnet: QString) -> QString {
        if !self.job.start("discover") {
            return err_json("a deploy job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        let subnet = subnet.to_string();
        std::thread::spawn(move || {
            let prefix = if subnet.trim().is_empty() {
                deploy::local_subnet24().unwrap_or_default()
            } else {
                subnet.trim().to_string()
            };
            let result = if prefix.is_empty() {
                json!({"ok": false, "error": "could not detect the local subnet — type one like 192.168.1"}).to_string()
            } else {
                let devices = deploy::scan_ssh(&prefix, std::time::Duration::from_millis(300), 64);
                let arr: Vec<_> = devices.iter().map(|d| json!({"ip": d.ip, "hostname": d.hostname})).collect();
                json!({"ok": true, "subnet": prefix, "devices": arr}).to_string()
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "discover"}).to_string())
    }

    /// List subdirectories of `path` on the device — drives the folder picker.
    /// Stays synchronous: a single `ssh ls` is quick and keeps navigation simple.
    fn list_remote_dirs(&self, user: QString, host: QString, path: QString) -> QString {
        match deploy::list_remote_dirs(&user.to_string(), &host.to_string(), &path.to_string()) {
            Ok((pwd, dirs)) => QString::from(json!({"ok": true, "path": pwd, "dirs": dirs}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    /// Start an rsync of the monorepo working tree to `dir` on the device, on a
    /// worker thread. Returns immediately; poll for the result.
    fn start_push(&self, user: QString, host: QString, dir: QString) -> QString {
        let root = match repo() { Ok(r) => r.root().to_string_lossy().into_owned(), Err(e) => return err_json(&e) };
        if !self.job.start("push") {
            return err_json("a deploy job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        let (u, h, d) = (user.to_string(), host.to_string(), dir.to_string());
        std::thread::spawn(move || {
            let result = match deploy::push_repo(&root, &u, &h, &d) {
                Ok(msg) => json!({"ok": true, "msg": msg}).to_string(),
                Err(e) => json!({"ok": false, "error": e}).to_string(),
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "push"}).to_string())
    }

    /// Start a remote build + `gin app install --desktop` on a worker thread, so
    /// the (possibly long) build doesn't freeze the UI. Returns immediately.
    fn start_deploy(&self, user: QString, host: QString, dir: QString, app: QString) -> QString {
        if !self.job.start("deploy") {
            return err_json("a deploy job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        let (u, h, d, a) = (user.to_string(), host.to_string(), dir.to_string(), app.to_string());
        std::thread::spawn(move || {
            let result = match deploy::deploy_app(&u, &h, &d, &a) {
                Ok(msg) => json!({"ok": true, "msg": msg}).to_string(),
                Err(e) => json!({"ok": false, "error": e}).to_string(),
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "deploy"}).to_string())
    }

    /// The GUI polls this on a timer → {running, done, kind, result}. The result
    /// JSON is delivered exactly once (the job resets to idle after a done poll).
    fn poll_job(&self) -> QString {
        let (running, done, kind, result) = self.job.poll();
        QString::from(json!({"running": running, "done": done, "kind": kind, "result": result}).to_string())
    }

    /// The local login name, so the deploy/network tabs can default the SSH user
    /// to *you* instead of the literal "user" (which froze the GUI on a password
    /// prompt when it was wrong).
    fn current_user(&self) -> QString {
        QString::from(json!({"user": netshare::current_user()}).to_string())
    }

    /// The GUI-managed fleet user (default `gitmind`) — what the fleet connects as,
    /// instead of your personal login. `admin` for provisioning still uses your own
    /// account once (it needs sudo to create the user).
    fn fleet_user(&self) -> QString {
        QString::from(json!({"user": fleet::fleet_user(), "admin": netshare::current_user()}).to_string())
    }

    fn set_fleet_user(&self, name: QString) -> QString {
        match fleet::set_fleet_user(&name.to_string()) {
            Ok(()) => QString::from(json!({"ok": true, "user": name.to_string()}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    /// One-time: create the fleet user on `host` and install the restricted key into
    /// it, over ssh as your sudo-capable `admin` account (key auth; the sudo password
    /// is fed in). Async — useradd + key install run on the device. Poll for the
    /// result (kind "provision").
    fn start_provision_fleet_user(&self, admin: QString, host: QString, password: QString) -> QString {
        let (a, h, pw) = (admin.to_string(), host.to_string(), password.to_string());
        if a.trim().is_empty() || h.trim().is_empty() {
            return err_json("admin account and host are required");
        }
        if pw.is_empty() {
            return err_json("enter the device's sudo password to create the fleet user");
        }
        if !self.job.start("provision") {
            return err_json("a job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        std::thread::spawn(move || {
            let result = provision_fleet_user(&a, &h, &pw);
            let payload = match result {
                Ok(msg) => json!({"ok": true, "msg": msg}).to_string(),
                Err(e) => json!({"ok": false, "error": e}).to_string(),
            };
            job.finish(payload);
        });
        QString::from(json!({"ok": true, "started": "provision"}).to_string())
    }

    /// Whether `host` is already in `~/.ssh/known_hosts`. Fast + local, so the GUI
    /// can render a trust badge without blocking.
    fn host_trusted(&self, host: QString) -> QString {
        QString::from(json!({"trusted": deploy::host_trusted(&host.to_string())}).to_string())
    }

    /// Trust `host`'s SSH key (fetch + append to known_hosts) on the shared worker
    /// thread — `ssh-keyscan` does a brief network round-trip, so it runs off the
    /// UI thread and reports via poll_job (kind "trust"). Returns immediately.
    fn start_trust(&self, host: QString) -> QString {
        let h = host.to_string();
        if h.trim().is_empty() {
            return err_json("no host to trust");
        }
        if !self.job.start("trust") {
            return err_json("a job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        std::thread::spawn(move || {
            let result = match deploy::trust_host(&h) {
                Ok(msg) => json!({"ok": true, "msg": msg, "host": h}).to_string(),
                Err(e) => json!({"ok": false, "error": e, "host": h}).to_string(),
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "trust"}).to_string())
    }

    /// Re-trust `host`: clear its (possibly stale) known_hosts entry and re-fetch
    /// the current key. For the "host key changed / messed up" case. Async.
    fn start_retrust(&self, host: QString) -> QString {
        let h = host.to_string();
        if h.trim().is_empty() {
            return err_json("no host to re-trust");
        }
        if !self.job.start("retrust") {
            return err_json("a job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        std::thread::spawn(move || {
            let result = match deploy::retrust_host(&h) {
                Ok(msg) => json!({"ok": true, "msg": msg, "host": h}).to_string(),
                Err(e) => json!({"ok": false, "error": e, "host": h}).to_string(),
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "retrust"}).to_string())
    }

    /// Browse a device's filesystem (folder picker) on the worker thread, so a slow
    /// or failing ssh can't freeze the UI. On failure the result carries a `reason`
    /// ("auth"/"hostkey"/"unreachable") the GUI uses to offer the right fix.
    fn start_browse(&self, user: QString, host: QString, path: QString) -> QString {
        if !self.job.start("browse") {
            return err_json("a job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        let (u, h, p) = (user.to_string(), host.to_string(), path.to_string());
        std::thread::spawn(move || {
            let target = if p.trim().is_empty() || p == "~" { "$HOME".to_string() } else { deploy::shell_quote(&p) };
            let cmd = appcatalog::remote_listing_cmd(&target);
            let result = match deploy::remote_capture(&u, &h, &cmd) {
                Ok(text) => {
                    let mut lines = text.lines();
                    let pwd = lines.next().unwrap_or("").trim().to_string();
                    let body = lines.collect::<Vec<_>>().join("\n");
                    let entries: Vec<_> = appcatalog::parse_app_listing(&body)
                        .into_iter()
                        .map(|e| json!({"name": e.name, "app": e.is_app(), "cargo": e.is_cargo, "git": e.is_git}))
                        .collect();
                    json!({"ok": true, "path": pwd, "entries": entries}).to_string()
                }
                Err(e) => {
                    let reason = deploy::ssh_error_reason(&e);
                    json!({"ok": false, "error": e, "reason": reason}).to_string()
                }
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "browse"}).to_string())
    }

    /// Metadata for one recognised app on the device (crate count, size, README),
    /// gathered over SSH. Synchronous but quick (a single remote command).
    fn app_details(&self, user: QString, host: QString, path: QString) -> QString {
        let (u, h, p) = (user.to_string(), host.to_string(), path.to_string());
        if p.trim().is_empty() {
            return err_json("no app path");
        }
        let cmd = appcatalog::remote_details_cmd(&deploy::shell_quote(&p));
        match deploy::remote_capture(&u, &h, &cmd) {
            Ok(text) => {
                let d = appcatalog::parse_app_details(&text);
                QString::from(json!({
                    "ok": true,
                    "crate_count": d.crate_count,
                    "size": d.size_human,
                    "readme": d.readme_excerpt,
                }).to_string())
            }
            Err(e) => err_json(&e),
        }
    }

    /// Scan an app's customizable build flags over SSH — cargo `[features]` and
    /// CMake `option(...)` — and report the repo's default-on set **per target**.
    fn app_build_options(&self, user: QString, host: QString, path: QString) -> QString {
        let (u, h, p) = (user.to_string(), host.to_string(), path.to_string());
        if p.trim().is_empty() {
            return err_json("no app path");
        }
        let q = deploy::shell_quote(&p);
        let cargo = deploy::remote_capture(&u, &h, &format!("cat {q}/Cargo.toml 2>/dev/null")).unwrap_or_default();
        let cmake = deploy::remote_capture(&u, &h, &format!("cat {q}/CMakeLists.txt 2>/dev/null")).unwrap_or_default();
        let feats = buildflags::scan_cargo_features(&cargo);
        let defaults: serde_json::Map<String, serde_json::Value> = buildflags::BUILD_TARGETS
            .iter()
            .map(|t| (t.to_string(), json!(buildflags::arch_default_features(&cargo, t))))
            .collect();
        let cmake_opts: Vec<_> = buildflags::scan_cmake_options(&cmake)
            .into_iter()
            .map(|o| json!({"name": o.name, "on": o.default_on, "help": o.help}))
            .collect();
        QString::from(json!({
            "ok": true,
            "targets": buildflags::BUILD_TARGETS,
            "features": feats.all,
            "defaults_by_target": defaults,
            "cmake_options": cmake_opts,
            "is_cargo": !cargo.trim().is_empty(),
            "is_cmake": !cmake.trim().is_empty(),
        }).to_string())
    }

    /// The app-store catalog: every recognised app under `root` on the source device,
    /// with size + blurb, from a single ssh round-trip. Returns each app's resolved
    /// path so the installer can act on it directly.
    fn store_catalog(&self, user: QString, host: QString, root: QString) -> QString {
        let (u, h, root) = (user.to_string(), host.to_string(), root.to_string());
        let q = deploy::shell_quote(if root.trim().is_empty() { "~" } else { &root });
        let raw = match deploy::remote_capture(&u, &h, &appcatalog::remote_catalog_cmd(&q)) {
            Ok(s) => s,
            Err(e) => return err_json(&e),
        };
        // first line is the resolved root (pwd); the rest are catalog rows
        let mut lines = raw.lines();
        let base = lines.next().unwrap_or("").trim().to_string();
        let body: String = lines.collect::<Vec<_>>().join("\n");
        let apps: Vec<_> = appcatalog::parse_catalog(&body)
            .into_iter()
            .map(|e| json!({
                "name": e.name,
                "size": e.size,
                "description": e.description,
                "path": format!("{}/{}", base.trim_end_matches('/'), e.name),
            }))
            .collect();
        QString::from(json!({"ok": true, "root": base, "apps": apps}).to_string())
    }

    /// The triple that matches *this* device's libc, so a store install defaults to a
    /// binary that actually runs here (musl on Alpine/postmarketOS, gnu on glibc).
    fn local_target(&self) -> QString {
        let musl = std::path::Path::new("/lib/ld-musl-aarch64.so.1").exists()
            || std::path::Path::new("/lib/libc.musl-aarch64.so.1").exists();
        let triple = if musl {
            "aarch64-unknown-linux-musl"
        } else {
            "aarch64-unknown-linux-gnu"
        };
        QString::from(json!({"target": triple}).to_string())
    }

    /// The app pages: every buildable repo gitmind manages here, **named by its
    /// crate** (not its folder). Local — reads Cargo.toml/CMakeLists straight from
    /// the working tree, no ssh, no remote browsing.
    fn local_apps(&self) -> QString {
        let root = match repo() {
            Ok(r) => r.root().to_path_buf(),
            Err(e) => return err_json(&e),
        };
        // candidate dirs: the repo root's children, plus the usual member folders'.
        let mut roots = vec![root.clone()];
        for sub in ["apps", "crates", "shared", "tools"] {
            let p = root.join(sub);
            if p.is_dir() {
                roots.push(p);
            }
        }
        let mut apps: Vec<serde_json::Value> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for base in &roots {
            let rd = match std::fs::read_dir(base) {
                Ok(rd) => rd,
                Err(_) => continue,
            };
            for ent in rd.flatten() {
                let dir = ent.path();
                if !dir.is_dir() || !seen.insert(dir.clone()) {
                    continue;
                }
                let cargo = dir.join("Cargo.toml");
                let cmake = dir.join("CMakeLists.txt");
                if cargo.is_file() {
                    let txt = std::fs::read_to_string(&cargo).unwrap_or_default();
                    if let Some(name) = buildflags::crate_name(&txt) {
                        apps.push(json!({
                            "name": name,
                            "path": dir.display().to_string(),
                            "description": buildflags::package_description(&txt).unwrap_or_default(),
                            "is_cargo": true,
                            "is_cmake": cmake.is_file(),
                        }));
                    }
                } else if cmake.is_file() {
                    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                    apps.push(json!({
                        "name": name, "path": dir.display().to_string(),
                        "description": "", "is_cargo": false, "is_cmake": true,
                    }));
                }
            }
        }
        apps.sort_by(|a, b| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")));
        QString::from(json!({"ok": true, "apps": apps}).to_string())
    }

    /// Scan a local crate's customizable flags (cargo features + CMake options) with
    /// the repo's per-target defaults. Local twin of app_build_options.
    fn local_build_options(&self, path: QString) -> QString {
        let p = std::path::PathBuf::from(path.to_string());
        let cargo = std::fs::read_to_string(p.join("Cargo.toml")).unwrap_or_default();
        let cmake = std::fs::read_to_string(p.join("CMakeLists.txt")).unwrap_or_default();
        let feats = buildflags::scan_cargo_features(&cargo);
        let defaults: serde_json::Map<String, serde_json::Value> = buildflags::BUILD_TARGETS
            .iter()
            .map(|t| (t.to_string(), json!(buildflags::arch_default_features(&cargo, t))))
            .collect();
        let cmake_opts: Vec<_> = buildflags::scan_cmake_options(&cmake)
            .into_iter()
            .map(|o| json!({"name": o.name, "on": o.default_on, "help": o.help}))
            .collect();
        QString::from(json!({
            "ok": true,
            "targets": buildflags::BUILD_TARGETS,
            "features": feats.all,
            "defaults_by_target": defaults,
            "cmake_options": cmake_opts,
            "is_cargo": !cargo.trim().is_empty(),
            "is_cmake": !cmake.trim().is_empty(),
        }).to_string())
    }

    /// Build `app` (a crate) for `target` from the working tree, locally. Async — a
    /// cross build is slow; poll_job for the result, which carries the build status:
    /// the binary path + size on success, or the compiler output on failure.
    fn build_app(&self, app: QString, target: QString, features: QString, no_default: bool) -> QString {
        let (app, target) = (app.to_string(), target.to_string());
        let feats: Vec<String> = features.to_string().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if app.trim().is_empty() || target.trim().is_empty() {
            return err_json("app and target are required");
        }
        let root = match repo() {
            Ok(r) => r.root().to_path_buf(),
            Err(e) => return err_json(&e),
        };
        if !self.job.start("build") {
            return err_json("a job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        std::thread::spawn(move || {
            let argv = buildflags::cargo_build_argv(&app, &target, &feats, no_default);
            let out = std::process::Command::new(&argv[0])
                .args(&argv[1..])
                .current_dir(&root)
                .output();
            let payload = match out {
                Ok(o) if o.status.success() => {
                    let bin = root.join(buildflags::built_binary_path(&app, &target));
                    let size = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
                    json!({
                        "ok": true, "app": app, "target": target,
                        "binary": bin.display().to_string(),
                        "size": appcatalog::human_size(size),
                    }).to_string()
                }
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr);
                    json!({"ok": false, "error": err.trim()}).to_string()
                }
                Err(e) => json!({"ok": false, "error": format!("couldn't run the build: {e}")}).to_string(),
            };
            job.finish(payload);
        });
        QString::from(json!({"ok": true, "started": "build"}).to_string())
    }

    /// Send a freshly-built binary to a device — and only the binary. Drops it into
    /// `dest` (default `~/.local/bin/<name>`) over scp and marks it executable.
    /// Async — poll_job for the result.
    fn send_binary(&self, binary: QString, user: QString, host: QString, dest: QString) -> QString {
        let (b, u, h, d) = (binary.to_string(), user.to_string(), host.to_string(), dest.to_string());
        if b.trim().is_empty() {
            return err_json("build the app first — there's no binary to send yet");
        }
        if u.trim().is_empty() || h.trim().is_empty() {
            return err_json("choose a device to send to");
        }
        if !self.job.start("send") {
            return err_json("a job is already running");
        }
        let name = b.rsplit('/').next().unwrap_or("app").to_string();
        let job = std::sync::Arc::clone(&self.job);
        std::thread::spawn(move || {
            let dest = if d.trim().is_empty() { format!(".local/bin/{name}") } else { d };
            let result = deploy::scp_push(&b, &u, &h, &dest).map(|()| {
                let _ = deploy::remote_capture(&u, &h, &format!("chmod +x {}", deploy::shell_quote(&dest)));
                format!("sent {name} → {h}:{dest}")
            });
            let payload = match result {
                Ok(msg) => json!({"ok": true, "msg": msg}).to_string(),
                Err(e) => json!({"ok": false, "error": e}).to_string(),
            };
            job.finish(payload);
        });
        QString::from(json!({"ok": true, "started": "send"}).to_string())
    }

    /// Cross-build `app` from `path` for `triple` with the chosen feature set, then
    /// deliver the binary to this device. Default route: scp to `/tmp/<app>` then
    /// `cp` into `~/.local/bin` (the /tmp copy stays for inspection if something
    /// goes wrong); `stage_tmp=false` scps straight into `~/.local/bin`. Async — a
    /// cross build is slow; poll_job for the outcome (build errors come back here).
    fn install_app(&self, user: QString, host: QString, path: QString, app: QString, triple: QString, features: QString, no_default: bool, stage_tmp: bool) -> QString {
        let (u, h, p) = (user.to_string(), host.to_string(), path.to_string());
        let (app, triple) = (app.to_string(), triple.to_string());
        let feats: Vec<String> = features.to_string().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        if app.trim().is_empty() || triple.trim().is_empty() || p.trim().is_empty() {
            return err_json("app, target and path are required");
        }
        if !self.job.start("install") {
            return err_json("a job is already running");
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
        let job = std::sync::Arc::clone(&self.job);
        std::thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                // 1 · cross build on the server
                let argv = buildflags::cargo_build_argv(&app, &triple, &feats, no_default);
                let build_cmd = format!("cd {} && {}", deploy::shell_quote(&p), argv.join(" "));
                deploy::remote_capture(&u, &h, &build_cmd).map_err(|e| format!("build failed:\n{e}"))?;
                // 2 · pull the binary
                let remote_bin = format!("{}/{}", p.trim_end_matches('/'), buildflags::built_binary_path(&app, &triple));
                let bindir = buildflags::install_dir(&home);
                let _ = std::fs::create_dir_all(&bindir);
                let dest = format!("{bindir}/{app}");
                if stage_tmp {
                    let staged = format!("/tmp/{app}");
                    deploy::scp_pull(&u, &h, &remote_bin, &staged)?;
                    std::fs::copy(&staged, &dest).map_err(|e| format!("cp {staged} -> {dest}: {e}"))?;
                    set_executable(&dest);
                    Ok(format!("installed {app} ({triple}) → {dest}  (staged at {staged})"))
                } else {
                    deploy::scp_pull(&u, &h, &remote_bin, &dest)?;
                    set_executable(&dest);
                    Ok(format!("installed {app} ({triple}) → {dest}"))
                }
            })();
            let payload = match result {
                Ok(msg) => json!({"ok": true, "msg": msg}).to_string(),
                Err(e) => json!({"ok": false, "error": e}).to_string(),
            };
            job.finish(payload);
        });
        QString::from(json!({"ok": true, "started": "install"}).to_string())
    }

    /// Install our SSH key on `user@host` using the device password (entered once),
    /// so connections become passwordless. Runs on the worker thread (it does a
    /// password-auth ssh round-trip). Returns immediately; poll for the result.
    fn start_setup_key(&self, user: QString, host: QString, password: QString) -> QString {
        let (u, h, pw) = (user.to_string(), host.to_string(), password.to_string());
        if u.trim().is_empty() || h.trim().is_empty() {
            return err_json("pick a device first");
        }
        if pw.is_empty() {
            return err_json("enter the device password");
        }
        if !self.job.start("setupkey") {
            return err_json("a job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        std::thread::spawn(move || {
            let result = match deploy::setup_key(&u, &h, &pw) {
                Ok(msg) => json!({"ok": true, "msg": msg}).to_string(),
                Err(e) => json!({"ok": false, "error": e}).to_string(),
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "setupkey"}).to_string())
    }

    // ── two-sided fingerprint matching ───────────────────────────────────
    /// This device's own key fingerprints (host key for connection identity, plus
    /// the identity key it installs), with randomart — so the *other* device can
    /// compare what it sees against this screen.
    fn local_fingerprint(&self) -> QString {
        let one = |pubkey: Option<String>| -> serde_json::Value {
            match pubkey.and_then(|k| deploy::key_fingerprint_art(&k)) {
                Some((fp, typ, art)) => json!({"present": true, "fingerprint": fp, "type": typ, "art": art}),
                None => json!({"present": false}),
            }
        };
        QString::from(json!({
            "ok": true,
            "host": one(deploy::local_host_pubkey()),
            "identity": one(deploy::local_identity_pubkey()),
        }).to_string())
    }

    /// Fetch a peer's host key (no write) and return its fingerprint + randomart for
    /// on-screen comparison; `keys` is the exact bytes to trust once confirmed.
    /// Async — ssh-keyscan does a network round-trip.
    fn start_preview(&self, host: QString) -> QString {
        let h = host.to_string();
        if h.trim().is_empty() {
            return err_json("no host to preview");
        }
        if !self.job.start("preview") {
            return err_json("a job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        std::thread::spawn(move || {
            let result = match deploy::fetch_peer_key(&h) {
                Ok(keys) => match deploy::key_fingerprint_art(&keys) {
                    Some((fp, typ, art)) => json!({"ok": true, "fingerprint": fp, "type": typ, "art": art, "keys": keys}).to_string(),
                    None => json!({"ok": true, "fingerprint": "(install ssh-keygen to show)", "type": "", "art": "", "keys": keys}).to_string(),
                },
                Err(e) => json!({"ok": false, "error": e, "reason": deploy::ssh_error_reason(&e)}).to_string(),
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "preview"}).to_string())
    }

    /// Trust exactly the key bytes the user just compared on screen (from a preview).
    /// Local write only — no network — so it's synchronous.
    fn trust_previewed_key(&self, host: QString, keys: QString) -> QString {
        match deploy::write_known_host(&host.to_string(), &keys.to_string()) {
            Ok(msg) => QString::from(json!({"ok": true, "msg": msg}).to_string()),
            Err(e) => err_json(&e),
        }
    }

    // ── bluetooth file send (OBEX) ───────────────────────────────────────
    /// Paired / known Bluetooth devices to send a file to.
    fn bluetooth_devices(&self) -> QString {
        match bttransfer::bt_devices() {
            Ok(devs) => {
                let list: Vec<_> = devs.into_iter().map(|d| json!({"addr": d.addr, "name": d.name})).collect();
                QString::from(json!({"ok": true, "devices": list}).to_string())
            }
            Err(e) => err_json(&e),
        }
    }

    /// Send `file` (e.g. the repo bundle) to a Bluetooth device over OBEX push, on
    /// the worker thread (transfers are slow). Poll poll_job for the result.
    fn start_bluetooth_send(&self, addr: QString, file: QString) -> QString {
        let (a, f) = (addr.to_string(), file.to_string());
        if a.trim().is_empty() || f.trim().is_empty() {
            return err_json("pick a device and a file");
        }
        if !self.job.start("btsend") {
            return err_json("a job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        std::thread::spawn(move || {
            let result = match bttransfer::bt_send(&a, &f) {
                Ok(msg) => json!({"ok": true, "msg": msg}).to_string(),
                Err(e) => json!({"ok": false, "error": e}).to_string(),
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "btsend"}).to_string())
    }

    /// Bundle the whole repo to `dest` (`git bundle create --all`) so it can be
    /// handed to a device over Bluetooth (or any sneakernet) and cloned there.
    fn make_bundle(&self, dest: QString) -> QString {
        let r = match repo() { Ok(r) => r, Err(e) => return err_json(&e) };
        let root = r.root().to_string_lossy().into_owned();
        let out = dest.to_string();
        let out = if out.trim().is_empty() {
            format!("{}/monorepo.bundle", std::env::temp_dir().to_string_lossy())
        } else {
            out
        };
        let st = std::process::Command::new("git")
            .args(["-C", &root, "bundle", "create", &out, "--all"])
            .status();
        match st {
            Ok(s) if s.success() => QString::from(json!({"ok": true, "path": out}).to_string()),
            Ok(s) => err_json(&format!("git bundle exited {:?}", s.code())),
            Err(e) => err_json(&format!("git bundle: {e}")),
        }
    }

    // ── network ──────────────────────────────────────────────────────────
    /// Start advertising this repo's card over HTTP on `port` (default 9876), so
    /// peers can look it up and clone it from the ssh provider in the card.
    fn advertise_start(&self, port: QString) -> QString {
        if self.advertiser.is_serving() {
            return err_json("already advertising — stop it first");
        }
        let port: u16 = port.to_string().trim().parse().unwrap_or(9876);
        let r = match repo() { Ok(r) => r, Err(e) => return err_json(&e) };
        let ip = netshare::local_ip().unwrap_or_else(|| "127.0.0.1".into());
        let card = match netshare::card_for_repo(r.root(), &netshare::current_user(), &ip) {
            Ok(c) => c,
            Err(e) => return err_json(&e),
        };
        let card_json = String::from_utf8_lossy(&card.to_bytes()).into_owned();
        let listener = match std::net::TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(e) => return err_json(&format!("bind port {port}: {e}")),
        };
        let addr = format!("{ip}:{port}");
        self.advertiser.begin(addr.clone());
        let adv = std::sync::Arc::clone(&self.advertiser);
        std::thread::spawn(move || {
            let _ = netshare::serve_loop(listener, card_json, adv.clone());
            adv.end();
        });
        QString::from(json!({"ok": true, "addr": addr, "name": card.name}).to_string())
    }

    /// Stop advertising.
    fn advertise_stop(&self) -> QString {
        self.advertiser.end();
        QString::from(json!({"ok": true}).to_string())
    }

    /// Whether we're advertising, and at what address.
    fn advertise_status(&self) -> QString {
        QString::from(json!({"serving": self.advertiser.is_serving(), "addr": self.advertiser.addr()}).to_string())
    }

    /// Look up a peer's advertised card at `addr` ("host:port") over HTTP.
    fn lookup_peer(&self, addr: QString) -> QString {
        match netshare::lookup_peer(&addr.to_string(), std::time::Duration::from_secs(4)) {
            Ok(card) => QString::from(json!({
                "ok": true,
                "name": card.name,
                "description": card.description,
                "providers": card.providers,
                "refs": card.refs.len(),
                "id": card.id().to_hex(),
            }).to_string()),
            Err(e) => err_json(&e),
        }
    }

    /// Install (git clone) a card's provider into `dest`, on the shared worker
    /// thread (clones can be slow). Poll poll_job for the result.
    fn start_install(&self, name: QString, provider: QString, dest: QString) -> QString {
        if !self.job.start("install") {
            return err_json("a job is already running");
        }
        let job = std::sync::Arc::clone(&self.job);
        let (n, p, d) = (name.to_string(), provider.to_string(), dest.to_string());
        std::thread::spawn(move || {
            let result = match netshare::install_card(&n, &p, &d) {
                Ok(msg) => json!({"ok": true, "msg": msg}).to_string(),
                Err(e) => json!({"ok": false, "error": e}).to_string(),
            };
            job.finish(result);
        });
        QString::from(json!({"ok": true, "started": "install"}).to_string())
    }
}

fn main() {
    register_qml();
    let mut engine = QmlEngine::new();
    let backend = QObjectBox::new(Backend::default());
    engine.set_object_property("backend".into(), backend.pinned());
    engine.load_file("qrc:/qml/main.qml".into());
    engine.exec();
}
