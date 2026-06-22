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
qrc!(register_qml,
    "/" {
        "qml/main.qml",
        "qml/GestureHost.qml",
        "qml/FileRow.qml",
        "qml/Timeline.qml",
        "qml/Branches.qml",
        "qml/PeekViewer.qml",
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

    session: WatchSession,
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
                    items.push(json!({"oid": oid, "subject": subject}));
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
        let store = match gstore() { Ok(s) => s, Err(e) => return err_json(&e) };
        let (name, email) = identity();
        match native::merge(&store, &branch.to_string(), &name, &email) {
            Ok(MergeOutcome::UpToDate) =>
                QString::from(json!({"ok": true, "outcome": "uptodate"}).to_string()),
            Ok(MergeOutcome::FastForward { new_tip }) => {
                sync_worktree(&store);
                QString::from(json!({"ok": true, "outcome": "fastforward", "new_tip": new_tip}).to_string())
            }
            Ok(MergeOutcome::Merged { commit }) => {
                sync_worktree(&store);
                QString::from(json!({"ok": true, "outcome": "merged", "commit": commit}).to_string())
            }
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
}

fn main() {
    register_qml();
    let mut engine = QmlEngine::new();
    let backend = QObjectBox::new(Backend::default());
    engine.set_object_property("backend".into(), backend.pinned());
    engine.load_file("qrc:/qml/main.qml".into());
    engine.exec();
}
