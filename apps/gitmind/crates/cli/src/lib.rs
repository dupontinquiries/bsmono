// gitmind-cli/src/main.rs
//
// Thin command-line shell over gitmind-core, plus environment-aware
// frontend dispatch: on a phone / touchscreen / no-tty context this binary
// can hand off to the GUI; in a terminal it stays the CLI. Explicit
// --gui/--cli/--tui (or GITMIND_FRONTEND) always win.
//
// Help is forgiving: help, -h, -help, --help all work; so do -v/--version.

use gitmind_core::backend::{GitCli, RepoBackend};
use gitmind_core::launch::{decide, Env, Frontend};
use gitmind_core::store::ChunkStore;
use gitmind_core::{select, CheckpointEngine};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

pub fn run() -> i32 {
    // Restore default SIGPIPE so piping into `head`, `fzf`, etc. terminates
    // quietly instead of panicking on a broken pipe (Rust ignores SIGPIPE by
    // default, turning a closed reader into an EPIPE panic in println!).
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let prog = std::env::args()
        .next()
        .and_then(|a| a.rsplit('/').next().map(|s| s.to_string()))
        .unwrap_or_else(|| "gitmind".to_string());
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // Global frontend override flags, stripped before command parsing.
    let forced = pop_frontend_flag(&mut args);

    let cmd = args.first().cloned().unwrap_or_default();
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };

    // Bare invocation (no command): decide CLI vs GUI from the environment.
    if cmd.is_empty() {
        match decide(&Env::detect(forced)) {
            Frontend::Cli => return cmd_status(),
            Frontend::Gui => {
                eprintln!("gitmind: GUI frontend isn't built into this binary yet — showing status.\n         (build the `gui` crate, or run an explicit command; `{prog} help`.)");
                return cmd_status();
            }
            Frontend::Tui => {
                eprintln!("gitmind: TUI frontend isn't built yet — showing status.");
                return cmd_status();
            }
        }
    }

    match cmd.as_str() {
        "help" | "-h" | "-help" | "--help" => {
            print_help(&prog);
            0
        }
        "version" | "-v" | "-V" | "--version" => {
            println!("gitmind {}", gitmind_core::VERSION);
            0
        }
        "status" | "st" => cmd_status(),
        "snapshot" | "snap" | "ci" => cmd_snapshot(rest.first().cloned()),
        "timeline" | "tl" => cmd_timeline(rest),
        "search" | "find" => cmd_search(rest),
        "review" => cmd_review(rest.first().cloned()),
        "maintain" | "gc" => cmd_maintain(),
        "graft" => cmd_graft(rest),
        "ingest" => match rest.first() {
            Some(f) => cmd_ingest(f),
            None => err("usage: gitmind ingest <file>"),
        },
        "restore" => match (rest.first(), rest.get(1)) {
            (Some(o), Some(out)) => cmd_restore(o, out),
            _ => err("usage: gitmind restore <oid> <out>"),
        },
        "diff" => cmd_diff(rest),
        "restore-checkpoint" | "extract" => cmd_extract(rest),
        "import" => cmd_import(rest),
        "export-native" => cmd_export_native(rest),
        "native-commit" | "ncommit" => cmd_native_commit(rest),
        "native-log" | "nlog" => cmd_native_log(rest),
        "native-status" | "nstatus" | "nst" => cmd_native_status(),
        "native-checkout" | "ncheckout" | "nco" => cmd_native_checkout(rest),
        "native-branch" | "nbranch" | "nb" => cmd_native_branch(rest),
        "cherry-pick" | "pick" => cmd_native_cherry_pick(rest),
        "rebase" => cmd_native_rebase(rest),
        "merge" => cmd_native_merge(rest),
        "patch" => cmd_patch(rest),
        "apply" => cmd_apply(rest),
        "zip" => cmd_zip(rest),
        "bridge" => cmd_bridge(rest),
        "watch" => cmd_watch(rest),
        "bundle" => match rest.first() {
            Some(out) => cmd_bundle(out),
            None => err("usage: gitmind bundle <out.bundle>"),
        },
        "where" | "root" => cmd_where(),
        other => err(&format!("unknown command: {other} (try `{}`)", "gitmind help")),
    }
}

// ── helpers ─────────────────────────────────────────────────────────────

fn pop_frontend_flag(args: &mut Vec<String>) -> Option<Frontend> {
    let mut found = None;
    args.retain(|a| match a.as_str() {
        "--gui" => { found = Some(Frontend::Gui); false }
        "--cli" => { found = Some(Frontend::Cli); false }
        "--tui" => { found = Some(Frontend::Tui); false }
        _ => true,
    });
    found
}

fn open_repo() -> Result<GitCli, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    GitCli::discover(&cwd).ok_or_else(|| "not inside a git repository".to_string())
}

/// Native store lives in `.gitmind/` (separate from `.git`).
fn open_store(repo: &GitCli) -> Result<ChunkStore, String> {
    let dir = gitmind_core::portage::native_store_dir(repo.root());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    ChunkStore::open(dir.join("store.redb"))
}

fn flag(rest: &[String], name: &str) -> bool {
    rest.iter().any(|a| a == name)
}

fn opt<'a>(rest: &'a [String], name: &str) -> Option<&'a str> {
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        if a == name {
            return it.next().map(|s| s.as_str());
        }
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Some(v);
        }
    }
    None
}

fn err(msg: &str) -> i32 {
    eprintln!("error: {msg}");
    1
}

// ── commands ────────────────────────────────────────────────────────────

fn cmd_status() -> i32 {
    let repo = match open_repo() {
        Ok(r) => r,
        Err(e) => return err(&e),
    };
    match repo.status() {
        Ok(st) => {
            let head = if st.detached { format!("HEAD detached at {}", st.branch) } else { st.branch.clone() };
            let mut line = format!("on {head}");
            if st.has_upstream {
                line += &format!("  ↑{} ↓{}", st.ahead, st.behind);
            }
            println!("{line}");
            if st.files.is_empty() {
                println!("clean");
            } else {
                // One clean status column (no redundant tag).
                for f in &st.files {
                    println!("  {}  {}", f.xy, f.path);
                }
            }
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_snapshot(label: Option<String>) -> i32 {
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let mut label = label.unwrap_or_else(|| format!("checkpoint {}", iso_now()));
    // If a process self-reports via GITMIND_AGENT (e.g. an AI tool), record
    // provenance in the checkpoint label so shared-file changes are attributable.
    if std::env::var("GITMIND_AGENT").map(|v| !v.is_empty()).unwrap_or(false) {
        label = gitmind_core::provenance::tag_label(&label);
    }
    let eng = CheckpointEngine::new(&repo, "main");
    match eng.snapshot(&label) {
        Ok(Some(c)) => { println!("checkpoint {} — {}", c.oid, c.label); 0 }
        Ok(None) => { println!("no changes since last checkpoint"); 0 }
        Err(e) => err(&e),
    }
}

fn cmd_timeline(rest: &[String]) -> i32 {
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let limit = rest.iter().find_map(|a| a.parse::<usize>().ok()).unwrap_or(20);
    let eng = CheckpointEngine::new(&repo, "main");
    match eng.list(limit) {
        Ok(list) if list.is_empty() => { println!("no checkpoints yet — run `gitmind snapshot`"); 0 }
        Ok(list) => {
            let rows: Vec<(String, String)> =
                list.iter().map(|c| (c.oid.clone(), c.label.clone())).collect();
            if flag(rest, "-f") {
                fuzzy_list(&rows)
            } else {
                for (oid, label) in &rows {
                    println!("{oid}  {label}");
                }
                0
            }
        }
        Err(e) => err(&e),
    }
}

fn cmd_search(rest: &[String]) -> i32 {
    use gitmind_core::query;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let author = opt(rest, "--author").or_else(|| opt(rest, "-a"));
    let lang = opt(rest, "--lang").or_else(|| opt(rest, "-l"));
    if author.is_none() && lang.is_none() {
        return err("usage: gitmind search [--author NAME] [--lang LANG] [-f]");
    }
    match query::search(&repo, author, lang, 200) {
        Ok(hits) if hits.is_empty() => { println!("no matching commits"); 0 }
        Ok(hits) => {
            let rows: Vec<(String, String)> = hits
                .iter()
                .map(|h| (h.oid.clone(), format!("{:<18} {}", h.author, h.summary)))
                .collect();
            if flag(rest, "-f") {
                fuzzy_list(&rows)
            } else {
                for (oid, desc) in &rows { println!("{oid}  {desc}"); }
                0
            }
        }
        Err(e) => err(&e),
    }
}

fn cmd_review(name: Option<String>) -> i32 {
    use gitmind_core::review::ReviewEngine;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let eng = ReviewEngine::new(&repo);
    let hooks_dir = repo.root().join(".gitmind").join("hooks");
    let hooks = if hooks_dir.is_dir() { Some(hooks_dir.as_path()) } else { None };
    let name = name.unwrap_or_else(|| format!("review-{}", iso_now().replace([' ', ':'], "-")));
    match eng.mark(&name, hooks) {
        Ok((s, hookres)) => {
            println!("review {name}: {} commits, {} files (+{} -{})",
                     s.commits, s.files_changed.len(), s.insertions, s.deletions);
            if !s.authors.is_empty() { println!("  authors: {}", s.authors.join(", ")); }
            if !s.languages.is_empty() {
                let langs: Vec<String> = s.languages.iter().map(|(l, n)| format!("{l}×{n}")).collect();
                println!("  languages: {}", langs.join(", "));
            }
            for h in hookres { println!("  hook {} {}", h.name, if h.ok { "ok" } else { "FAILED" }); }
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_maintain() -> i32 {
    use gitmind_core::maintenance::Maintenance;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    match Maintenance::new(&repo).run_all() {
        Ok(r) => {
            println!("maintenance: commit-graph={} changed-paths={} refs-packed={} repacked={}",
                     r.commit_graph, r.changed_paths, r.refs_packed, r.repacked);
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_graft(rest: &[String]) -> i32 {
    use gitmind_core::compose;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let (src, branch) = match (rest.first(), rest.get(1)) {
        (Some(s), Some(b)) => (s, b),
        _ => return err("usage: gitmind graft <source-repo> <new-branch> [--ref R] [--prefix P]"),
    };
    let src_ref = opt(rest, "--ref").unwrap_or("HEAD");
    let prefix = opt(rest, "--prefix").unwrap_or("vendor");
    match compose::graft(&repo, std::path::Path::new(src), src_ref, prefix, branch) {
        Ok(oid) => { println!("grafted {src} under {prefix}/ on {branch} ({})", &oid[..oid.len().min(12)]); 0 }
        Err(e) => err(&e),
    }
}

fn cmd_ingest(file: &str) -> i32 {
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let f = match std::fs::File::open(file) { Ok(f) => f, Err(e) => return err(&format!("open {file}: {e}")) };
    let name = PathBuf::from(file).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| file.to_string());
    match store.ingest(&name, f) {
        Ok(r) => {
            println!("{}  {} ({:.1} MB, {} chunks, {} new, {:.1} MB stored, {:?})",
                     r.blob_oid.short(), name, r.total_len as f64 / 1.0e6, r.chunk_count,
                     r.new_chunks, r.new_bytes_stored as f64 / 1.0e6, r.media);
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_restore(oid: &str, out: &str) -> i32 {
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let oid = match gitmind_core::Oid::from_hex(oid) { Some(o) => o, None => return err("bad oid (need full 64-char hex)") };
    let f = match std::fs::File::create(out) { Ok(f) => f, Err(e) => return err(&format!("create {out}: {e}")) };
    match store.restore(oid, f) {
        Ok(n) => { println!("restored {n} bytes -> {out}"); 0 }
        Err(e) => err(&e),
    }
}

fn cmd_import(rest: &[String]) -> i32 {
    use gitmind_core::portage;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    if flag(rest, "--blobs-only") {
        println!("importing .git blob data into .gitmind …");
        return match portage::import_git_objects(&repo, &store) {
            Ok(r) => {
                println!("imported {} blobs, {:.1} MB ({} new chunks)",
                         r.blobs, r.total_bytes as f64 / 1.0e6, r.new_chunks);
                0
            }
            Err(e) => err(&e),
        };
    }
    let reachable = !flag(rest, "--all");
    println!(
        "importing {} .git object graph into .gitmind …",
        if reachable { "reachable" } else { "full" }
    );
    match portage::import_graph(&repo, &store, reachable) {
        Ok(r) => { println!("{}", r.summary()); 0 }
        Err(e) => err(&e),
    }
}

fn cmd_export_native(rest: &[String]) -> i32 {
    use gitmind_core::portage;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let dir = match rest.first() {
        Some(d) if !d.starts_with('-') => d,
        _ => return err("usage: gitmind export-native <dir>  (rebuild a git repo from .gitmind alone)"),
    };
    println!("reconstructing a git repository from .gitmind into {dir} …");
    match portage::export_to_git(&store, std::path::Path::new(dir)) {
        Ok(n) => { println!("rebuilt {n} objects — verify with: git -C {dir} log --oneline"); 0 }
        Err(e) => err(&e),
    }
}

fn cmd_bundle(out: &str) -> i32 {
    use gitmind_core::portage;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    match portage::export_bundle(&repo, std::path::Path::new(out)) {
        Ok(()) => { println!("wrote git bundle -> {out} (clone with: git clone {out})"); 0 }
        Err(e) => err(&e),
    }
}

fn cmd_zip(rest: &[String]) -> i32 {
    use gitmind_core::ingest_zip;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let archive = match rest.first() {
        Some(a) if !a.starts_with('-') => a,
        _ => return err("usage: gitmind zip <archive.zip> [--apply]"),
    };
    let target = repo.root();
    let plan = match ingest_zip::plan_unzip(std::path::Path::new(archive), target) {
        Ok(p) => p,
        Err(e) => return err(&e),
    };
    println!("{}", plan.summary());
    for a in &plan.added { println!("  + {a}"); }
    for m in &plan.modified { println!("  ~ {} ({} → {} bytes)", m.path, m.old_size, m.new_size); }
    if !plan.unchanged.is_empty() { println!("  = {} unchanged", plan.unchanged.len()); }
    if flag(rest, "--apply") {
        if plan.is_noop() {
            println!("nothing to extract (all entries unchanged)");
            return 0;
        }
        match ingest_zip::apply_unzip(std::path::Path::new(archive), target) {
            Ok(n) => { println!("extracted {n} files into {}", target.display()); 0 }
            Err(e) => err(&e),
        }
    } else {
        println!("(dry run — pass --apply to extract)");
        0
    }
}

fn cmd_bridge(rest: &[String]) -> i32 {
    use gitmind_core::bridge;
    let repo = open_repo().ok();
    let dir = repo.as_ref().map(|r| r.root().display().to_string())
        .unwrap_or_else(|| ".".to_string());
    let run = flag(rest, "--run");
    let pipeline = match rest.first().map(|s| s.as_str()) {
        Some("send") => {
            let host = match rest.get(1) { Some(h) => h, None => return err("usage: gitmind bridge send <host> <port> [--run]") };
            let port: u16 = match rest.get(2).and_then(|p| p.parse().ok()) { Some(p) => p, None => return err("bad/missing port") };
            bridge::send_pipeline(&dir, host, port, true)
        }
        Some("recv") => {
            let port: u16 = match rest.get(1).and_then(|p| p.parse().ok()) { Some(p) => p, None => return err("usage: gitmind bridge recv <port> [--run]") };
            bridge::recv_pipeline(&dir, port, true)
        }
        _ => return err("usage: gitmind bridge <send|recv> …  (plaintext LAN transport — use for trusted networks only)"),
    };
    println!("{pipeline}");
    if run {
        eprintln!("note: plaintext + unauthenticated; trusted LAN only.");
        let st = std::process::Command::new("sh").arg("-c").arg(&pipeline).status();
        return match st { Ok(s) if s.success() => 0, _ => 1 };
    }
    println!("(printed only — pass --run to execute, or paste into the peer's terminal)");
    0
}

fn print_ck(c: &gitmind_core::checkpoint::Checkpoint) {
    println!("  checkpoint {} — {}", c.oid, c.label);
}

fn cmd_watch(rest: &[String]) -> i32 {
    use gitmind_core::watch::WatchPolicy;
    use gitmind_core::watcher;
    use std::sync::mpsc;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let mut policy = WatchPolicy::default();
    if let Some(ms) = opt(rest, "--debounce").and_then(|s| s.parse().ok()) {
        policy.debounce_ms = ms;
    }
    if let Some(types) = opt(rest, "--types") {
        policy = WatchPolicy::with_types(types.split(','), policy.debounce_ms);
    }

    // Privileged path: real per-write PID attribution via fanotify. Falls
    // back to inotify if unavailable (not built, or no CAP_SYS_ADMIN).
    if flag(rest, "--fanotify") {
        println!(
            "watching {} via fanotify — PID-attributed autosnapshots (Ctrl-C to stop)",
            repo.root().display()
        );
        let (tx, rx) = mpsc::channel();
        std::mem::forget(tx);
        match gitmind_core::fanotify::run(&repo, &policy, "main", rx, print_ck) {
            Ok(_) => return 0,
            Err(e) => eprintln!("fanotify unavailable: {e}\nfalling back to inotify…"),
        }
    }

    let poll = flag(rest, "--poll");
    println!(
        "watching {} — autosnapshots to refs/checkpoints/main{} (Ctrl-C to stop)",
        repo.root().display(),
        if poll { " [poll mode]" } else { "" }
    );
    let (tx, rx) = mpsc::channel();
    std::mem::forget(tx);
    match watcher::run_with_mode(&repo, &policy, "main", poll, rx, print_ck) {
        Ok(_) => 0,
        Err(e) => err(&e),
    }
}

fn cmd_diff(rest: &[String]) -> i32 {
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let eng = CheckpointEngine::new(&repo, "main");
    // 0 args → latest checkpoint vs worktree; 1 → that checkpoint vs worktree;
    // 2 → between two revisions.
    let positional: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect();
    let (from, to): (String, Option<String>) = match (positional.first(), positional.get(1)) {
        (Some(a), Some(b)) => ((*a).clone(), Some((*b).clone())),
        (Some(a), None) => ((*a).clone(), None),
        (None, _) => match eng.list(1) {
            Ok(list) => match list.first() {
                Some(c) => (c.oid.clone(), None),
                None => return err("no checkpoints to diff — run `gitmind snapshot` first"),
            },
            Err(e) => return err(&e),
        },
    };
    match eng.diff(&from, to.as_deref()) {
        Ok(deltas) if deltas.is_empty() => { println!("no changes"); 0 }
        Ok(deltas) => {
            let target = to.as_deref().unwrap_or("working tree");
            println!("{} → {}", &from[..from.len().min(12)], target);
            for d in &deltas {
                println!("  +{:<5} -{:<5} {}", d.added, d.removed, d.path);
            }
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_extract(rest: &[String]) -> i32 {
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let eng = CheckpointEngine::new(&repo, "main");
    let (oid, dir) = match (rest.first(), rest.get(1)) {
        (Some(o), Some(d)) => (o, d),
        _ => return err("usage: gitmind restore-checkpoint <oid> <dir>"),
    };
    match eng.extract(oid, std::path::Path::new(dir)) {
        Ok(n) => {
            println!("extracted {n} files from {} → {dir}", &oid[..oid.len().min(12)]);
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_native_commit(rest: &[String]) -> i32 {
    use gitmind_core::native;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let name = repo.git(&["config", "user.name"]).ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gitmind".to_string());
    let email = repo.git(&["config", "user.email"]).ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gitmind@local".to_string());
    let msg = opt(rest, "-m").or_else(|| opt(rest, "--message"))
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("native commit {}", iso_now()));
    let current = store.refs().ok().and_then(|refs| {
        refs.into_iter().find(|(n, _)| n == "HEAD")
            .and_then(|(_, t)| t.strip_prefix("refs/heads/").map(|s| s.to_string()))
    });
    let branch = opt(rest, "--branch").map(|s| s.to_string())
        .or(current)
        .unwrap_or_else(|| "main".to_string());
    match native::commit_worktree(&store, repo.root(), &branch, &name, &email, &msg) {
        Ok(c) => {
            println!("committed {} to refs/heads/{branch} (tree {})",
                     &c.oid[..c.oid.len().min(12)], &c.tree[..c.tree.len().min(12)]);
            println!("(export with: gin export-native <dir>)");
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_patch(rest: &[String]) -> i32 {
    use gitmind_core::native;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let target = match rest.iter().find(|a| !a.starts_with('-')) {
        Some(t) => t,
        None => return err("usage: gin patch <commit> [-o file.patch]"),
    };
    match native::format_commit_patch(&store, target) {
        Ok(p) => match opt(rest, "-o").or_else(|| opt(rest, "--output")) {
            Some(out) => match std::fs::write(out, &p) {
                Ok(()) => { println!("wrote patch to {out}"); 0 }
                Err(e) => err(&e.to_string()),
            },
            None => { print!("{p}"); 0 }
        },
        Err(e) => err(&e),
    }
}

fn cmd_apply(rest: &[String]) -> i32 {
    use gitmind_core::native;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let file = match rest.iter().find(|a| !a.starts_with('-')) {
        Some(f) => f,
        None => return err("usage: gin apply <file.patch>"),
    };
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => return err(&e.to_string()),
    };
    match native::apply_patch_to_dir(&text, repo.root()) {
        Ok(n) => { println!("applied {n} file section(s) from {file}"); 0 }
        Err(e) => err(&e),
    }
}

fn cmd_native_merge(rest: &[String]) -> i32 {
    use gitmind_core::native::{self, MergeOutcome};
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let theirs = match rest.iter().find(|a| !a.starts_with('-')) {
        Some(t) => t,
        None => return err("usage: gin merge <branch>"),
    };
    let (name, email) = identity(&repo);
    match native::merge(&store, theirs, &name, &email) {
        Ok(MergeOutcome::UpToDate) => { println!("already up to date"); 0 }
        Ok(MergeOutcome::FastForward { new_tip }) => {
            sync_worktree_to_head(&store, repo.root());
            println!("fast-forwarded to {}", &new_tip[..new_tip.len().min(12)]);
            0
        }
        Ok(MergeOutcome::Merged { commit }) => {
            sync_worktree_to_head(&store, repo.root());
            println!("merged {theirs} → merge commit {}", &commit[..commit.len().min(12)]);
            0
        }
        Ok(MergeOutcome::Conflicts(paths)) => {
            eprintln!("merge conflicts (nothing committed):");
            for p in &paths { eprintln!("  {p}"); }
            1
        }
        Err(e) => err(&e),
    }
}

fn cmd_native_rebase(rest: &[String]) -> i32 {
    use gitmind_core::native::{self, RebaseOutcome};
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    // Forms: `rebase <onto>` (rebase current branch onto <onto>)
    //        `rebase <branch> --onto <onto>`
    let positional: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect();
    let onto_flag = opt(rest, "--onto");
    let (branch, onto) = match (onto_flag, positional.as_slice()) {
        (Some(o), [b, ..]) => (b.to_string(), o.to_string()),
        (Some(o), []) => (current_branch(&store).unwrap_or_else(|| "main".into()), o.to_string()),
        (None, [o]) => (current_branch(&store).unwrap_or_else(|| "main".into()), o.to_string()),
        (None, [b, o, ..]) => (b.to_string(), o.to_string()),
        _ => return err("usage: gin rebase <onto>   |   gin rebase <branch> --onto <onto>"),
    };
    let (name, email) = identity(&repo);
    match native::rebase(&store, &branch, &onto, &name, &email) {
        Ok(RebaseOutcome::Rebased { new_tip, commits }) => {
            sync_worktree_to_head(&store, repo.root());
            println!("rebased {branch} onto {onto}: {commits} commit(s) replayed → {}",
                     &new_tip[..new_tip.len().min(12)]);
            0
        }
        Ok(RebaseOutcome::UpToDate) => { println!("{branch} is up to date with {onto}"); 0 }
        Ok(RebaseOutcome::Conflicts { commit, paths }) => {
            eprintln!("rebase stopped at {} — conflicts (nothing moved):", &commit[..commit.len().min(12)]);
            for p in &paths { eprintln!("  {p}"); }
            1
        }
        Err(e) => err(&e),
    }
}

fn current_branch(store: &gitmind_core::store::ChunkStore) -> Option<String> {
    store.refs().ok().and_then(|refs| {
        refs.into_iter().find(|(n, _)| n == "HEAD")
            .and_then(|(_, t)| t.strip_prefix("refs/heads/").map(|s| s.to_string()))
    })
}

fn identity(repo: &gitmind_core::backend::GitCli) -> (String, String) {
    let name = repo.git(&["config", "user.name"]).ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gitmind".to_string());
    let email = repo.git(&["config", "user.email"]).ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gitmind@local".to_string());
    (name, email)
}

fn cmd_native_branch(rest: &[String]) -> i32 {
    use gitmind_core::native;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    match rest.first().map(|s| s.as_str()) {
        None => match native::branch_list(&store) {
            Ok(bs) if bs.is_empty() => { println!("no branches yet — run `gin native-commit`"); 0 }
            Ok(bs) => {
                for b in &bs {
                    println!("{} {}  {}", if b.is_head { '*' } else { ' ' }, b.name, &b.oid[..b.oid.len().min(12)]);
                }
                0
            }
            Err(e) => err(&e),
        },
        Some("-d") | Some("--delete") => match rest.get(1) {
            Some(name) => match native::branch_delete(&store, name) {
                Ok(()) => { println!("deleted branch {name}"); 0 }
                Err(e) => err(&e),
            },
            None => err("usage: gin native-branch -d <name>"),
        },
        Some(name) => {
            let start = rest.get(1).map(|s| s.as_str());
            match native::branch_create(&store, name, start) {
                Ok(oid) => { println!("created branch {name} at {}", &oid[..oid.len().min(12)]); 0 }
                Err(e) => err(&e),
            }
        }
    }
}

fn cmd_native_cherry_pick(rest: &[String]) -> i32 {
    use gitmind_core::native::{self, PickOutcome};
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let target = match rest.first() {
        Some(t) if !t.starts_with('-') => t,
        _ => return err("usage: gin cherry-pick <commit-oid>"),
    };
    let name = repo.git(&["config", "user.name"]).ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gitmind".to_string());
    let email = repo.git(&["config", "user.email"]).ok().filter(|s| !s.is_empty())
        .unwrap_or_else(|| "gitmind@local".to_string());
    match native::cherry_pick(&store, target, &name, &email) {
        Ok(PickOutcome::Committed(c)) => {
            // Sync the working tree to the branch HEAD now points at.
            sync_worktree_to_head(&store, repo.root());
            println!("cherry-picked → {} (working tree updated)", &c.oid[..c.oid.len().min(12)]);
            0
        }
        Ok(PickOutcome::Empty) => { println!("nothing to do — already present on HEAD"); 0 }
        Ok(PickOutcome::Conflicts(paths)) => {
            eprintln!("cherry-pick conflicts (nothing committed):");
            for p in &paths { eprintln!("  {p}"); }
            1
        }
        Err(e) => err(&e),
    }
}

fn sync_worktree_to_head(store: &gitmind_core::store::ChunkStore, root: &std::path::Path) {
    use gitmind_core::native;
    if let Ok(refs) = store.refs() {
        if let Some((_, target)) = refs.iter().find(|(n, _)| n == "HEAD") {
            let r = if let Some(b) = target.strip_prefix("refs/heads/") {
                native::checkout(store, b, root)
            } else {
                native::checkout(store, target, root)
            };
            let _ = r;
        }
    }
}

fn cmd_native_checkout(rest: &[String]) -> i32 {
    use gitmind_core::native;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let target = match rest.first() {
        Some(t) if !t.starts_with('-') => t,
        _ => return err("usage: gin native-checkout <branch|commit-oid>"),
    };
    match native::checkout(&store, target, repo.root()) {
        Ok(r) => {
            println!("checked out {} ({} files written, {} removed)",
                     &r.commit[..r.commit.len().min(12)], r.written, r.removed);
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_native_status() -> i32 {
    use gitmind_core::native;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    match native::status(&store, repo.root()) {
        Ok(changes) if changes.is_empty() => {
            println!("working tree matches native HEAD (clean)");
            0
        }
        Ok(changes) => {
            for c in &changes {
                println!("{} {}", c.kind.sigil(), c.path);
            }
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_native_log(rest: &[String]) -> i32 {
    use gitmind_core::{native, portage};
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let limit = rest.iter().find_map(|a| a.parse::<usize>().ok()).unwrap_or(20);
    let mut cur = match native::resolve_head(&store) {
        Ok(Some(o)) => Some(o),
        Ok(None) => { println!("no native commits yet — run `gin native-commit`"); return 0; }
        Err(e) => return err(&e),
    };
    let mut shown = 0;
    while let Some(oid) = cur {
        if shown >= limit { break; }
        match portage::native_commit(&store, &oid) {
            Ok(meta) => {
                let subject = meta.message.lines().next().unwrap_or("");
                println!("{}  {}", &oid[..oid.len().min(12)], subject);
                cur = meta.parents.first().cloned();
                shown += 1;
            }
            Err(e) => return err(&e),
        }
    }
    0
}

fn cmd_where() -> i32 {
    match open_repo() {
        Ok(r) => { println!("{}", r.root().display()); 0 }
        Err(e) => err(&e),
    }
}

// ── -f fuzzy selection ──────────────────────────────────────────────────
//
// Prints a coded list (a0, a1, …). If stdin+stdout are a terminal and `fzf`
// is installed, hands off to fzf for arrow-key selection; otherwise reads a
// selection string (see select.rs grammar) from stdin. Selected oids are
// printed one per line, so `gitmind tl -f | xargs …` composes.
fn fuzzy_list(rows: &[(String, String)]) -> i32 {
    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    if interactive && which("fzf") {
        return fzf_select(rows);
    }
    // Coded list.
    for (i, (oid, desc)) in rows.iter().enumerate() {
        println!("{}  {oid}  {desc}", select::code_for(i));
    }
    if !interactive {
        return 0; // machine mode: just the list
    }
    eprint!("select (e.g. a0, a3..b1, *): ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return 0;
    }
    match select::parse_selection(line.trim(), rows.len()) {
        Ok(idx) => {
            for i in idx { println!("{}", rows[i].0); }
            0
        }
        Err(e) => err(&e),
    }
}

fn fzf_select(rows: &[(String, String)]) -> i32 {
    use std::process::{Command, Stdio};
    let mut child = match Command::new("fzf")
        .args(["-m", "-d", "\t", "--with-nth", "2,3"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()
    {
        Ok(c) => c,
        Err(_) => return err("fzf launch failed"),
    };
    if let Some(mut sin) = child.stdin.take() {
        for (oid, desc) in rows {
            let _ = writeln!(sin, "{oid}\t{oid}\t{desc}");
        }
    }
    let out = match child.wait_with_output() { Ok(o) => o, Err(e) => return err(&e.to_string()) };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(oid) = line.split('\t').next() {
            println!("{oid}");
        }
    }
    0
}

fn which(bin: &str) -> bool {
    std::env::var("PATH").map(|p| {
        p.split(':').any(|d| std::path::Path::new(d).join(bin).exists())
    }).unwrap_or(false)
}

fn print_help(prog: &str) {
    println!("{prog} {} — touch-first repo manager (CLI frontend)", gitmind_core::VERSION);
    println!("  (\"gin\" is a short alias for \"gitmind\")\n");
    println!("  status | st                 repo state + changed files");
    println!("  snapshot | snap [label]     checkpoint the worktree (shadow ref, index untouched)");
    println!("  timeline | tl [N] [-f]      list checkpoints (-f: fuzzy select)");
    println!("  diff [<a>] [<b>]            changes in a checkpoint vs worktree, or between two");
    println!("  restore-checkpoint <oid> <dir>   extract a snapshot's full tree into <dir>");
    println!("  search [--author N] [--lang L] [-f]   search history");
    println!("  review [name]               summarize work since last review + run hooks");
    println!("  maintain | gc               commit-graph + changed-path bloom + pack-refs");
    println!("  graft <src> <branch> [--ref R] [--prefix P]   field-test another repo on a branch");
    println!("  ingest <file>               stream a file into the .gitmind chunk store");
    println!("  restore <oid> <out>         restore a stored blob");
    println!("  import [--all|--blobs-only] import the git object graph into .gitmind (default: reachable)");
    println!("  export-native <dir>         rebuild a git repo from .gitmind alone (no .git needed)");
    println!("  native-commit | ncommit [-m msg]   commit the worktree directly into .gitmind (no git)");
    println!("  native-log | nlog [N]       walk native commits in .gitmind");
    println!("  native-status | nst         working-tree changes vs native HEAD (no git)");
    println!("  native-checkout | nco <ref> materialize a native commit/branch into the worktree");
    println!("  native-branch | nb [name] [-d name]   list/create/delete native branches");
    println!("  cherry-pick | pick <oid>    apply a commit's change onto the current branch");
    println!("  rebase <onto> | <branch> --onto <onto>   replay a branch's commits onto a new base");
    println!("  merge <branch>              3-way merge a branch into the current one (no git)");
    println!("  patch <commit> [-o file]    export a commit as a unified diff (git-apply compatible)");
    println!("  apply <file.patch>          apply a unified diff to the working tree (no git)");
    println!("  zip <archive> [--apply]     dry-run summary of an overwriting extract, then apply");
    println!("  bridge send|recv … [--run]  ad-hoc folder transfer over netcat (trusted LAN only)");
    println!("  watch [--debounce MS] [--types rs,py] [--fanotify] [--poll]   realtime time-machine: autosnapshot on save");
    println!("  bundle <out.bundle>         export a git bundle for compatibility");
    println!("  where                       print repo root\n");
    println!("  global: --gui | --cli | --tui   force a frontend (also GITMIND_FRONTEND)");
    println!("          -h/--help, -v/--version");
}

// Minimal ISO timestamp without chrono.
fn iso_now() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = (t / 86400) as i64;
    let sod = (t % 86400) as u32;
    let (h, m, s) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mon = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mon <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mon, d, h, m, s)
}
