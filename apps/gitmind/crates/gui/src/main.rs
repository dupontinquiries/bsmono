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

qrc!(register_qml,
    "/" {
        "qml/main.qml",
        "qml/GestureHost.qml",
        "qml/FileRow.qml",
        "qml/Timeline.qml",
        "qml/Branches.qml",
        "qml/PeekViewer.qml",
        "qml/Apps.qml",
        "qml/Deploy.qml",
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

    // ── deploy: discover SSH devices, browse + push the repo, remote build + .desktop ──
    // The slow ops run off-thread (see impls); the QML drives them via poll_job().
    start_discover: qt_method!(fn(&self, subnet: QString) -> QString),
    list_remote_dirs: qt_method!(fn(&self, user: QString, host: QString, path: QString) -> QString),
    start_push: qt_method!(fn(&self, user: QString, host: QString, dir: QString) -> QString),
    start_deploy: qt_method!(fn(&self, user: QString, host: QString, dir: QString, app: QString) -> QString),
    poll_job: qt_method!(fn(&self) -> QString),

    session: WatchSession,
    // Shared state for the one in-flight background deploy job.
    job: std::sync::Arc<deploy::Job>,
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
}

fn main() {
    register_qml();
    let mut engine = QmlEngine::new();
    let backend = QObjectBox::new(Backend::default());
    engine.set_object_property("backend".into(), backend.pinned());
    engine.load_file("qrc:/qml/main.qml".into());
    engine.exec();
}
