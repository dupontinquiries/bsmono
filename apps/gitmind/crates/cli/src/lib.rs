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
        "spread" => cmd_spread(rest),
        "repack" => cmd_repack(rest),
        "var" => cmd_var(rest),
        "app" => cmd_app(rest),
        "check" => cmd_check(rest),
        "patch" => cmd_patch(rest),
        "apply" => cmd_apply(rest),
        "zip" => cmd_zip(rest),
        "fleet" => cmd_fleet(rest),
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

fn cmd_spread(rest: &[String]) -> i32 {
    use gitmind_core::{native, portage};
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let tree_mode = flag(rest, "--tree");
    let into = opt(rest, "--into").map(|s| s.to_string())
        .unwrap_or_else(|| "spread".to_string());
    let label_kind = opt(rest, "--label").unwrap_or("index"); // index|oid|branch

    // Build the (label, rev) list from a selection flag.
    let revs: Vec<(String, String)> = if let Some(list) = opt(rest, "--revs") {
        list.split(',').filter(|s| !s.is_empty()).enumerate()
            .map(|(i, r)| (label_for(label_kind, i, r, r), r.to_string()))
            .collect()
    } else if flag(rest, "--branches") {
        match native::branch_list(&store) {
            Ok(bs) => bs.into_iter().map(|b| (b.name.clone(), b.name)).collect(),
            Err(e) => return err(&e),
        }
    } else {
        // default / --last N: walk native history from HEAD
        let n = opt(rest, "--last").and_then(|s| s.parse::<usize>().ok()).unwrap_or(5);
        let mut cur = match native::resolve_head(&store) {
            Ok(Some(o)) => Some(o),
            Ok(None) => return err("no native commits — run `gin native-commit` first"),
            Err(e) => return err(&e),
        };
        let mut out = Vec::new();
        let mut i = 0;
        while let Some(oid) = cur {
            if i >= n { break; }
            out.push((label_for(label_kind, i, &oid, &oid), oid.clone()));
            cur = portage::native_commit(&store, &oid).ok().and_then(|m| m.parents.first().cloned());
            i += 1;
        }
        out
    };
    if revs.is_empty() {
        return err("no revisions selected");
    }

    let into_path = std::path::Path::new(&into);
    if tree_mode {
        match native::spread_tree(&store, &revs, into_path) {
            Ok(dirs) => { println!("spread {} tree(s) into {}/:", dirs.len(), into);
                          for d in &dirs { println!("  {d}/"); } 0 }
            Err(e) => err(&e),
        }
    } else {
        let path = match rest.iter().find(|a| !a.starts_with('-')) {
            Some(p) => p,
            None => return err("usage: gin spread <path> [--last N|--revs a,b|--branches] [--into dir] [--tree]"),
        };
        match native::spread_file(&store, path, &revs, into_path) {
            Ok(w) if w.is_empty() => { println!("'{path}' not found in any selected revision"); 0 }
            Ok(w) => { println!("spread {} version(s) of {path} into {}/:", w.len(), into);
                       for f in &w { println!("  {f}"); } 0 }
            Err(e) => err(&e),
        }
    }
}

fn label_for(kind: &str, idx: usize, oid: &str, rref: &str) -> String {
    match kind {
        "oid" => oid[..oid.len().min(8)].to_string(),
        "branch" => rref.to_string(),
        _ => format!("head{:02}", idx + 1),
    }
}

fn cmd_repack(rest: &[String]) -> i32 {
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let min_gain = opt(rest, "--min-gain").and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.05);
    match store.repack(min_gain) {
        Ok(r) => {
            let pct = if r.bytes_before > 0 {
                100.0 * (r.bytes_before - r.bytes_after) as f64 / r.bytes_before as f64
            } else { 0.0 };
            println!("repack: {} chunks scanned, {} deltified", r.chunks_scanned, r.chunks_deltified);
            println!("        {} → {} stored bytes ({:.1}% smaller)", r.bytes_before, r.bytes_after, pct);
            0
        }
        Err(e) => err(&e),
    }
}

fn cmd_var(rest: &[String]) -> i32 {
    use gitmind_core::varcache;
    let sub = match rest.first() {
        Some(s) => s.as_str(),
        None => return err("usage: gin var <stash|restore|list|pin|unpin> ..."),
    };
    let args = &rest[1..];
    let var_dir = std::path::Path::new(".var");
    let pointer = |name: &str| var_dir.join(name);

    match sub {
        "stash" => {
            let dir = args.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str()).unwrap_or("target");
            let dpath = std::path::Path::new(dir);
            if !dpath.exists() { return err(&format!("no such directory: {dir}")); }
            let name = opt(args, "--name").map(|s| s.to_string())
                .unwrap_or_else(|| dpath.file_name().and_then(|s| s.to_str()).unwrap_or("cache").to_string());
            let store = match varcache::open_var_store() { Ok(s) => s, Err(e) => return err(&e) };
            let oid = match varcache::stash(&store, dpath) { Ok(o) => o, Err(e) => return err(&e) };
            if let Err(e) = std::fs::create_dir_all(var_dir).and_then(|_| std::fs::write(pointer(&name), &oid)) {
                return err(&e.to_string());
            }
            if flag(args, "--pin") { let _ = varcache::pin(&store, &name, &oid); }
            if flag(args, "--repack") {
                if let Ok(r) = store.repack(0.05) {
                    println!("repacked: {} chunks deltified, {} → {} bytes", r.chunks_deltified, r.bytes_before, r.bytes_after);
                }
            }
            println!("stashed {dir} → {} (pointer .var/{name})", &oid[..oid.len().min(12)]);
            println!("commit .var/{name} (tiny); the bytes live system-wide in {}", varcache::var_store_path().display());
            0
        }
        "restore" => {
            let name = match args.iter().find(|a| !a.starts_with('-')) {
                Some(n) => n, None => return err("usage: gin var restore <name> [--into dir]"),
            };
            let oid = match std::fs::read_to_string(pointer(name)) {
                Ok(s) => s.trim().to_string(),
                Err(_) => return err(&format!("no pointer .var/{name}")),
            };
            let into = opt(args, "--into").unwrap_or(name);
            let store = match varcache::open_var_store() { Ok(s) => s, Err(e) => return err(&e) };
            match varcache::restore(&store, &oid, std::path::Path::new(into)) {
                Ok(()) => { println!("restored .var/{name} → {into}/"); 0 }
                Err(e) => err(&e),
            }
        }
        "list" => {
            println!("pointers (.var/):");
            if let Ok(rd) = std::fs::read_dir(var_dir) {
                for e in rd.flatten() {
                    if let Ok(oid) = std::fs::read_to_string(e.path()) {
                        println!("  {}  {}", e.file_name().to_string_lossy(), &oid.trim()[..oid.trim().len().min(12)]);
                    }
                }
            }
            if let Ok(store) = varcache::open_var_store() {
                if let Ok(ps) = varcache::pins(&store) {
                    if !ps.is_empty() {
                        println!("pinned (system store):");
                        for (n, o) in ps { println!("  {n}  {}", &o[..o.len().min(12)]); }
                    }
                }
            }
            0
        }
        "pin" | "unpin" => {
            let name = match args.iter().find(|a| !a.starts_with('-')) {
                Some(n) => n, None => return err(&format!("usage: gin var {sub} <name>")),
            };
            let store = match varcache::open_var_store() { Ok(s) => s, Err(e) => return err(&e) };
            if sub == "pin" {
                let oid = match std::fs::read_to_string(pointer(name)) {
                    Ok(s) => s.trim().to_string(), Err(_) => return err(&format!("no pointer .var/{name}")),
                };
                match varcache::pin(&store, name, &oid) { Ok(()) => { println!("pinned {name}"); 0 } Err(e) => err(&e) }
            } else {
                match varcache::unpin(&store, name) { Ok(()) => { println!("unpinned {name}"); 0 } Err(e) => err(&e) }
            }
        }
        "gc" => {
            let mut store = match varcache::open_var_store() { Ok(s) => s, Err(e) => return err(&e) };
            let path = varcache::var_store_path();
            let before = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if !flag(args, "--no-repack") {
                if let Ok(r) = store.repack(0.05) {
                    println!("repacked: {} chunks deltified", r.chunks_deltified);
                }
            }
            match store.compact() {
                Ok(moved) => {
                    let after = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                    println!("compacted ({}): {} → {} bytes", if moved { "reclaimed" } else { "no-op" }, before, after);
                    0
                }
                Err(e) => err(&e),
            }
        }
        _ => err("usage: gin var <stash|restore|list|pin|unpin|gc> ..."),
    }
}

fn cmd_check(rest: &[String]) -> i32 {
    use gitmind_core::{checks, native, portage};
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };

    // `--candidates` just lists already-green commits.
    if flag(rest, "--candidates") {
        match checks::candidates(&store) {
            Ok(cs) if cs.is_empty() => { println!("no passing checks recorded yet"); return 0; }
            Ok(cs) => {
                for c in cs {
                    let subj = portage::native_commit(&store, &c.commit).ok()
                        .and_then(|m| m.message.lines().next().map(|s| s.to_string())).unwrap_or_default();
                    println!("✓ {}  {} ({} ms)", &c.commit[..c.commit.len().min(12)], subj, c.duration_ms);
                }
                return 0;
            }
            Err(e) => return err(&e),
        }
    }

    let cmd: Vec<String> = match opt(rest, "--cmd") {
        Some(c) => c.split_whitespace().map(|s| s.to_string()).collect(),
        None => checks::default_cmd(),
    };

    // Which commits to check: explicit target, --last N, or HEAD.
    let targets: Vec<String> = if let Some(n) = opt(rest, "--last").and_then(|s| s.parse::<usize>().ok()) {
        let mut cur = native::resolve_head(&store).ok().flatten();
        let mut out = Vec::new();
        while let (Some(oid), true) = (cur.clone(), out.len() < n) {
            out.push(oid.clone());
            cur = portage::native_commit(&store, &oid).ok().and_then(|m| m.parents.first().cloned());
        }
        out
    } else if let Some(t) = rest.iter().find(|a| !a.starts_with('-')) {
        match native::resolve_ref_or_commit(&store, t) { Ok(o) => vec![o], Err(e) => return err(&e) }
    } else {
        match native::resolve_head(&store) { Ok(Some(o)) => vec![o], _ => return err("no commit to check") }
    };

    let mut failures = 0;
    for oid in &targets {
        println!("checking {} …", &oid[..oid.len().min(12)]);
        match checks::run(&store, oid, &cmd) {
            Ok(st) => {
                println!("  {} {} (code {}, {} ms)", checks::marker(Some(&st)),
                         if st.ok { "ok" } else { "FAILED" }, st.code, st.duration_ms);
                if !st.ok { failures += 1; }
            }
            Err(e) => { eprintln!("  error: {e}"); failures += 1; }
        }
    }
    if failures > 0 { 1 } else { 0 }
}

fn cmd_app(rest: &[String]) -> i32 {
    use gitmind_core::{binpin, varcache};
    let sub = match rest.first() { Some(s) => s.as_str(), None =>
        return err("usage: gin app <install|build|use|list|log|run|desktop|remove> ...") };
    let args = &rest[1..];
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();

    match sub {
        "install" => {
            let (name, binpath) = match pos.as_slice() {
                [n, p, ..] => (n.as_str(), p.as_str()),
                _ => return err("usage: gin app install <name> <binary> [--label L] [--commit C] [--use] [--desktop]"),
            };
            let bytes = match std::fs::read(binpath) { Ok(b) => b, Err(e) => return err(&e.to_string()) };
            let store = match varcache::open_var_store() { Ok(s) => s, Err(e) => return err(&e) };
            let label = opt(args, "--label").map(|s| s.to_string()).unwrap_or_else(|| default_label(args));
            let commit = opt(args, "--commit").unwrap_or("manual").to_string();
            let target = opt(args, "--target").unwrap_or("host").to_string();
            match binpin::install(&store, name, &label, &commit, &target, &bytes, flag(args, "--use")) {
                Ok(v) => {
                    println!("installed {name}@{label} → {}", v.path);
                    if flag(args, "--desktop") {
                        match binpin::write_desktop(name, &label, flag(args, "--terminal"), None) {
                            Ok(p) => println!("pinned: {}", p.display()),
                            Err(e) => return err(&e),
                        }
                    }
                    0
                }
                Err(e) => err(&e),
            }
        }
        "build" => {
            let name = match pos.first() { Some(n) => n.as_str(), None =>
                return err("usage: gin app build <name> [commit] [--cmd \"...\"] [--artifact path] [--target T] [--label L]") };
            let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
            let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
            let commit = pos.get(1).map(|s| s.as_str()).unwrap_or("HEAD").to_string();
            let target = opt(args, "--target").map(|s| s.to_string());
            let cmd: Vec<String> = match opt(args, "--cmd") {
                Some(c) => c.split_whitespace().map(|s| s.to_string()).collect(),
                None => {
                    let mut c = vec!["cargo".to_string(), "build".to_string(), "--release".to_string()];
                    if let Some(t) = &target { c.push("--target".to_string()); c.push(t.clone()); }
                    c
                }
            };
            let artifact = opt(args, "--artifact").map(|s| s.to_string()).unwrap_or_else(|| match &target {
                Some(t) => format!("target/{t}/release/{name}"),
                None => format!("target/release/{name}"),
            });
            let work = std::env::temp_dir().join(format!("gitmind-build-{name}-{}", std::process::id()));
            println!("building {name} from {commit} ({})…", target.as_deref().unwrap_or("host"));
            let bytes = match binpin::build_from_commit(&store, &commit, &work, &cmd, &artifact) {
                Ok(b) => b, Err(e) => return err(&e),
            };
            let vstore = match varcache::open_var_store() { Ok(s) => s, Err(e) => return err(&e) };
            let label = opt(args, "--label").map(|s| s.to_string())
                .unwrap_or_else(|| commit[..commit.len().min(8)].to_string());
            let resolved = gitmind_core::native::resolve_ref_or_commit(&store, &commit).unwrap_or(commit.clone());
            match binpin::install(&vstore, name, &label, &resolved, target.as_deref().unwrap_or("host"), &bytes, flag(args, "--use")) {
                Ok(v) => { println!("installed {name}@{label} → {} ({} bytes)", v.path, bytes.len());
                    if flag(args, "--desktop") { let _ = binpin::write_desktop(name, &label, flag(args, "--terminal"), None); }
                    0 }
                Err(e) => err(&e),
            }
        }
        "use" => match pos.as_slice() {
            [n, l, ..] => match binpin::use_version(n, l) {
                Ok(()) => { println!("now using {n}@{l}"); 0 } Err(e) => err(&e) },
            _ => err("usage: gin app use <name> <label>"),
        },
        "list" => {
            if let Some(n) = pos.first() {
                let m = binpin::load_manifest(n);
                for v in &m.versions {
                    let cur = if m.current.as_deref() == Some(&v.label) { "*" } else { " " };
                    println!("{cur} {}@{}  {}  {}", n, v.label, &v.commit[..v.commit.len().min(8)], v.target);
                }
            } else {
                for a in binpin::list_apps() {
                    let m = binpin::load_manifest(&a);
                    println!("{a}  ({} versions, current {})", m.versions.len(), m.current.as_deref().unwrap_or("-"));
                }
            }
            0
        }
        "log" => {
            let n = match pos.first() { Some(n) => n, None => return err("usage: gin app log <name>") };
            let mut m = binpin::load_manifest(n);
            m.versions.sort_by(|a, b| b.installed.cmp(&a.installed));
            for v in &m.versions {
                println!("{}@{}  commit {}  target {}  installed {}",
                    n, v.label, &v.commit[..v.commit.len().min(12)], v.target, v.installed);
            }
            0
        }
        "run" => {
            use std::os::unix::process::CommandExt;
            let n = match pos.first() { Some(n) => n, None => return err("usage: gin app run <name> [label] [-- args]") };
            let m = binpin::load_manifest(n);
            let label = pos.get(1).map(|s| s.to_string())
                .or_else(|| m.current.clone())
                .unwrap_or_default();
            let path = match binpin::version_path(n, &label) { Some(p) => p, None =>
                return err(&format!("no installed {n}@{label}")) };
            let passthrough: Vec<&String> = match args.iter().position(|a| a == "--") {
                Some(i) => args[i + 1..].iter().collect(), None => Vec::new(),
            };
            let e = std::process::Command::new(&path).args(passthrough).exec();
            err(&format!("exec failed: {e}"))
        }
        "desktop" => match pos.as_slice() {
            [n, l, ..] => match binpin::write_desktop(n, l, flag(args, "--terminal"), opt(args, "--icon")) {
                Ok(p) => { println!("wrote {}", p.display()); 0 } Err(e) => err(&e) },
            _ => err("usage: gin app desktop <name> <label> [--terminal] [--icon NAME]"),
        },
        "remove" => match pos.as_slice() {
            [n, l, ..] => match binpin::remove(n, l) {
                Ok(()) => { println!("removed {n}@{l}"); 0 } Err(e) => err(&e) },
            _ => err("usage: gin app remove <name> <label>"),
        },
        _ => err("usage: gin app <install|build|use|list|log|run|desktop|remove> ..."),
    }
}

/// A default version label from a `--commit`/HEAD-ish hint, else a timestamp.
fn default_label(args: &[String]) -> String {
    if let Some(c) = opt(args, "--commit") {
        return c[..c.len().min(8)].to_string();
    }
    format!("v{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
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
    match native::merge_into_worktree(&store, theirs, repo.root(), &name, &email) {
        Ok(MergeOutcome::UpToDate) => { println!("already up to date"); 0 }
        Ok(MergeOutcome::FastForward { new_tip }) => {
            println!("fast-forwarded to {}", &new_tip[..new_tip.len().min(12)]);
            0
        }
        Ok(MergeOutcome::Merged { commit }) => {
            println!("merged {theirs} → merge commit {}", &commit[..commit.len().min(12)]);
            0
        }
        Ok(MergeOutcome::Conflicts(paths)) => {
            eprintln!("merge conflicts — markers written to:");
            for p in &paths { eprintln!("  {p}"); }
            eprintln!("resolve them, then `gin native-commit -m \"merge\"` to finish.");
            1
        }
        Err(e) => err(&e),
    }
}

fn cmd_native_rebase(rest: &[String]) -> i32 {
    use gitmind_core::native::{self, RebaseOutcome};
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    let (name, email) = identity(&repo);

    if flag(rest, "--abort") {
        return match native::rebase_abort(&store, repo.root()) {
            Ok(()) => { println!("rebase aborted; branch restored"); 0 }
            Err(e) => err(&e),
        };
    }
    let finish = |out: Result<RebaseOutcome, String>| -> i32 {
        match out {
            Ok(RebaseOutcome::Rebased { new_tip, commits }) => {
                println!("rebase complete: {commits} commit(s) → {}", &new_tip[..new_tip.len().min(12)]);
                0
            }
            Ok(RebaseOutcome::UpToDate) => { println!("already up to date"); 0 }
            Ok(RebaseOutcome::Conflicts { commit, paths }) => {
                eprintln!("rebase paused at {} — conflicts, markers written to:", &commit[..commit.len().min(12)]);
                for p in &paths { eprintln!("  {p}"); }
                eprintln!("resolve them, then `gin rebase --continue` (or `gin rebase --abort`).");
                1
            }
            Err(e) => err(&e),
        }
    };
    if flag(rest, "--continue") {
        return finish(native::rebase_continue(&store, repo.root(), &name, &email));
    }

    let positional: Vec<&String> = rest.iter().filter(|a| !a.starts_with('-')).collect();
    let onto_flag = opt(rest, "--onto");
    let (branch, onto) = match (onto_flag, positional.as_slice()) {
        (Some(o), [b, ..]) => (b.to_string(), o.to_string()),
        (Some(o), []) => (current_branch(&store).unwrap_or_else(|| "main".into()), o.to_string()),
        (None, [o]) => (current_branch(&store).unwrap_or_else(|| "main".into()), o.to_string()),
        (None, [b, o, ..]) => (b.to_string(), o.to_string()),
        _ => return err("usage: gin rebase <onto> | <branch> --onto <onto> | --continue | --abort"),
    };
    finish(native::rebase_into_worktree(&store, &branch, &onto, repo.root(), &name, &email))
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
    match native::cherry_pick_into_worktree(&store, target, repo.root(), &name, &email) {
        Ok(PickOutcome::Committed(c)) => {
            println!("cherry-picked → {} (working tree updated)", &c.oid[..c.oid.len().min(12)]);
            0
        }
        Ok(PickOutcome::Empty) => { println!("nothing to do — already present on HEAD"); 0 }
        Ok(PickOutcome::Conflicts(paths)) => {
            eprintln!("cherry-pick conflicts — markers written to:");
            for p in &paths { eprintln!("  {p}"); }
            eprintln!("resolve them, then `gin native-commit -m \"...\"` to finish.");
            1
        }
        Err(e) => err(&e),
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
    if let Ok(Some(mh)) = native::merge_head(&store) {
        println!("merge in progress (MERGE_HEAD {}) — resolve, then `gin native-commit`", &mh[..mh.len().min(12)]);
    }
    if let Ok(Some(cur)) = native::rebase_status(&store) {
        println!("rebase in progress (at {}) — resolve, then `gin rebase --continue`", &cur[..cur.len().min(12)]);
    }
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
    use gitmind_core::{checks, native, portage};
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
                let mark = checks::marker(checks::get(&store, &oid).ok().flatten().as_ref());
                println!("{} {}  {}", mark, &oid[..oid.len().min(12)], subject);
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
    println!("  rebase <onto> | --continue | --abort   replay commits; pauses on conflict with markers");
    println!("  merge <branch>              3-way merge a branch into the current one (no git)");
    println!("  spread <path> [--last N|--revs a,b|--branches] [--into dir] [--tree]");
    println!("                              lay several versions side by side ({{head01}}--file) for native viewers");
    println!("  repack [--min-gain 0.05]    cross-file delta-compress the store (storage only, oids unchanged)");
    println!("  var stash [dir] [--name N] [--pin] | restore <name> [--into dir] | list | pin/unpin <name>");
    println!("                              system-wide dedup cache for build artifacts (target/, node_modules)");
    println!("  app install <name> <bin> | build <name> [commit] | use <name> <label> | list | log | run | desktop | remove");
    println!("                              version-pinned binaries in ~/.local/bin + .desktop pins (GNOME), dedup'd");
    println!("  check [commit] [--last N] [--cmd \"...\"] | --candidates");
    println!("                              cache a build/check verdict per commit; native-log shows ✓/✗/· markers");
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

// ── fleet: cockpit operations shared with the GUI (import / build / apply / merge / reclaim) ──

fn cmd_fleet(rest: &[String]) -> i32 {
    use cockpit::flow;
    let sub = match rest.first() {
        Some(s) => s.as_str(),
        None => return err("usage: gitmind fleet <import|downloads|suite-install|build|clean|prepare|apply|merge|rollup|reclaim|dedup|scope|suite|build-dir|unbreak-ssh> ..."),
    };
    let args = &rest[1..];
    let pos: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();

    match sub {
        "import" => {
            let zip = match pos.first() {
                Some(z) => z.as_str(),
                None => return err("usage: gitmind fleet import <zip> [--run] [--save <dir>] [--init gitmind|git|none] [--base <dir>]"),
            };
            let imp = match flow::import(zip) { Ok(i) => i, Err(e) => return err(&e) };
            println!("crate: {} {}", imp.app, if imp.is_bin { "(binary)" } else { "(library)" });
            if !imp.description.is_empty() { println!("  {}", imp.description); }
            println!("  extracted to {}", imp.dir.display());
            if imp.has_git {
                println!("  carries {} history → `gitmind fleet merge {} <repo>`",
                    if imp.has_gitmind { "gitmind" } else { "git" }, zip);
            }
            let mut run_from = imp.crate_path.clone();
            if let Some(dest) = opt(args, "--save") {
                let init = opt(args, "--init").unwrap_or("gitmind");
                let base = opt(args, "--base").unwrap_or(dest);
                match flow::place(&imp.crate_path, dest, base, init) {
                    Ok(p) => { println!("saved to {}{}", p.display(), if init != "none" { format!(" ({init} initialized)") } else { String::new() }); run_from = p; }
                    Err(e) => return err(&e),
                }
            }
            if flag(args, "--run") {
                if !imp.is_bin { return err("library crate — nothing to run"); }
                match flow::build_and_run(&run_from, &imp.app) {
                    Ok(b) => println!("launched: {}", b.display()),
                    Err(e) => return err(&e),
                }
            }
            if flag(args, "--add") {
                match flow::build_and_add(&run_from, &imp.app, "") {
                    Ok(i) => println!("added {} to your apps @{}", i.name, i.label),
                    Err(e) => return err(&e),
                }
            }
            0
        }
        "unbreak-ssh" | "fix-ssh" => {
            let (user, host) = match (pos.first(), pos.get(1)) {
                (Some(u), Some(h)) => (u.as_str(), h.as_str()),
                _ => {
                    // also accept a single user@host argument
                    match pos.first().and_then(|s| s.split_once('@')) {
                        Some((u, h)) => (u, h),
                        None => return err("usage: gitmind fleet unbreak-ssh <user> <host>   (or user@host)"),
                    }
                }
            };
            match cockpit::deploy::unbreak_ssh(user, host) {
                Ok(m) => { println!("{m}"); 0 }
                Err(e) => err(&e),
            }
        }
        "downloads" => {
            let items = flow::scan_downloads(&flow::downloads_dir());
            if items.is_empty() { println!("nothing importable in {}", flow::downloads_dir().display()); return 0; }
            for it in &items {
                let tag = if it.kind == "bundle" { format!("bundle · {} apps", it.apps) } else if it.installed { "app · installed (update)".into() } else { "app".to_string() };
                println!("  [{tag}]  {}\n           {}", it.name, it.path);
            }
            println!("install: gitmind fleet import <path> --add   |   bundle: gitmind fleet suite-install <path>");
            0
        }
        "suite-install" => {
            let path = match pos.first() { Some(p) => p.as_str(), None => return err("usage: gitmind fleet suite-install <bundle.json|.zip>") };
            let suite = match flow::import_bundle(path) { Ok(s) => s, Err(e) => return err(&e) };
            println!("installing bundle '{}' ({} apps):", suite.name, suite.apps.len());
            let mut ok = 0;
            for (i, a) in suite.apps.iter().enumerate() {
                print!("  [{}/{}] {} … ", i + 1, suite.apps.len(), a.name);
                use std::io::Write; let _ = std::io::stdout().flush();
                match flow::install_suite_app(a) {
                    Ok(inst) => { println!("✓ @{}", inst.label); ok += 1; }
                    Err(e) => println!("✗ {e}"),
                }
            }
            println!("{ok}/{} installed", suite.apps.len());
            if ok == suite.apps.len() { 0 } else { 1 }
        }
        "clean" => {
            let dir = pos.first().map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("."));
            let scan = flow::reclaim_scan(&dir);
            if scan.items.is_empty() { println!("nothing to clean under {}", dir.display()); return 0; }
            for (path, bytes, label) in &scan.items {
                println!("  {:>9}  {:<20} {path}", cockpit::appcatalog::human_size(*bytes), label);
            }
            println!("{} reclaimable in {} item(s)", cockpit::appcatalog::human_size(scan.total_bytes), scan.items.len());
            if flag(args, "--apply") {
                let (n, freed) = flow::reclaim_apply(&dir);
                println!("removed {n} item(s), freed {}", cockpit::appcatalog::human_size(freed));
            } else {
                println!("add --apply to remove");
            }
            0
        }
        "prepare" => {
            let dir = pos.first().map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from("."));
            let p = flow::prepare_ingest(&dir);
            println!("prepared {} ({}): cleaned {} item(s), freed {}, +{} gitignore rule(s)",
                dir.display(), p.system, p.cleaned, cockpit::appcatalog::human_size(p.freed), p.ignored);
            println!("ready to `git add` / ingest — build junk is removed and gitignored");
            0
        }
        "build" => {
            let path = match pos.first() {
                Some(p) => std::path::PathBuf::from(p),
                None => return err("usage: gitmind fleet build <project-path> [--target T]"),
            };
            let proj = match cockpit::buildsystem::detect(&path) {
                Some(p) => p,
                None => return err("couldn't tell what kind of project this is (Rust, Python, Node, Go, CMake, Make)"),
            };
            let target = opt(args, "--target").map(|s| s.to_string()).unwrap_or_default();
            let options: Vec<String> = opt(args, "--features").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()).unwrap_or_default();
            println!("{} — {}", proj.name, proj.kind.label());
            let opts = cockpit::buildsystem::options_for(&proj);
            if !opts.is_empty() {
                let names: Vec<String> = opts.iter().map(|o| format!("{}{}", o.name, if o.default { "*" } else { "" })).collect();
                println!("options: {}  (* = default; pass --features a,b)", names.join(", "));
            }
            match cockpit::buildsystem::build(&proj, &target, &options, &cockpit::tool_path_env()) {
                Ok(b) => {
                    println!("built {} ({})", b.install.display(), cockpit::appcatalog::human_size(b.bytes));
                    if !b.args.is_empty() { println!("run: {} {}", b.run.display(), b.args.join(" ")); }
                    0
                }
                Err(e) => err(&e),
            }
        }
        "apply" => {
            let (zip, repo) = match (pos.first(), pos.get(1)) {
                (Some(z), Some(r)) => (z.as_str(), r.as_str()),
                _ => return err("usage: gitmind fleet apply <zip> <repo> [--mirror]"),
            };
            let imp = match flow::import(zip) { Ok(i) => i, Err(e) => return err(&e) };
            let name = std::path::Path::new(zip).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            match flow::apply_overlay(&imp.crate_path, repo, flag(args, "--mirror"), &name) {
                Ok(r) => {
                    if let Some(m) = r.noop_msg { println!("{m}"); return 0; }
                    println!("applied on branch {} — {}", r.branch, r.shortstat);
                    for (st, p) in &r.files { println!("  {st}  {p}"); }
                    println!("review: git -C {repo} diff {}..{}", r.base, r.branch);
                    0
                }
                Err(e) => err(&e),
            }
        }
        "merge" => {
            let (zip, repo) = match (pos.first(), pos.get(1)) {
                (Some(z), Some(r)) => (z.as_str(), r.as_str()),
                _ => return err("usage: gitmind fleet merge <zip> <repo> [--strategy <id>]"),
            };
            let imp = match flow::import(zip) { Ok(i) => i, Err(e) => return err(&e) };
            let zip_repo = match &imp.zip_repo {
                Some(p) => p.display().to_string(),
                None => return err("the zip has no git history — use `gitmind fleet apply` instead"),
            };
            let ex = match flow::explore(&zip_repo, repo) { Ok(e) => e, Err(e) => return err(&e) };
            println!("into {}{}", ex.target_branch, if ex.related { "" } else { " (unrelated histories)" });
            for t in &ex.trials {
                println!("  {} {:<22} {}{}",
                    if t.ok { "✓" } else { "✗" }, t.label, t.summary,
                    if t.recommended { "  [recommended]" } else { "" });
            }
            if let Some(id) = opt(args, "--strategy") {
                let t = match ex.trials.iter().find(|t| t.id == id && t.ok) {
                    Some(t) => t,
                    None => return err(&format!("no clean strategy '{id}' (try one marked ✓)")),
                };
                if flag(args, "--preview") {
                    match flow::merge_preview(&ex.workspace.display().to_string(), &ex.target_branch, &t.branch) {
                        Ok(p) => {
                            println!("\npreview of '{id}' ({}):", p.shortstat);
                            for (oid, subj) in &p.commits { println!("  {oid}  {subj}"); }
                            for (st, path) in &p.files { println!("  {st}  {path}"); }
                            println!("\nnothing landed — re-run with --strategy {id} (no --preview) to land it");
                            0
                        }
                        Err(e) => err(&e),
                    }
                } else {
                    match flow::land_merge(&ex.workspace.display().to_string(), &t.branch, repo, id) {
                        Ok(b) => { println!("landed on branch {b} — review and merge it"); 0 }
                        Err(e) => err(&e),
                    }
                }
            } else {
                println!("pick one with --strategy <id> (add --preview to see the diff first)");
                0
            }
        }
        "rollup" => {
            let (older, newer) = match (pos.first(), pos.get(1)) {
                (Some(a), Some(b)) => (a.as_str(), b.as_str()),
                _ => return err("usage: gitmind fleet rollup <older-copy> <newer-copy> [--apply]  (or --strategy <id> [--preview|--apply] when diverged)"),
            };
            let pv = flow::rollup_preview(std::path::Path::new(older), std::path::Path::new(newer));
            match pv.relation.as_str() {
                "same" => { println!("already identical — nothing to roll up"); 0 }
                "ahead" => err("the first copy is newer than the second — swap the order"),
                "behind" => {
                    println!("{older}\n  is {} commit(s) behind {newer} — a clean fast-forward:", pv.behind);
                    for (oid, subj) in &pv.incoming { println!("  {oid}  {subj}"); }
                    for (st, path) in &pv.files { println!("  {st}  {path}"); }
                    if flag(args, "--apply") {
                        match flow::rollup_apply(std::path::Path::new(older), std::path::Path::new(newer)) {
                            Ok(m) => { println!("\n{m}"); 0 }
                            Err(e) => err(&e),
                        }
                    } else {
                        println!("\nadd --apply to fast-forward this copy in place (nothing is deleted)");
                        0
                    }
                }
                "diverged" => {
                    println!("{older} and {newer} have diverged (each has unique commits) — a merge is needed.");
                    let ex = match flow::explore(older, newer) { Ok(e) => e, Err(e) => return err(&e) };
                    for t in &ex.trials {
                        println!("  {} {:<22} {}{}", if t.ok { "✓" } else { "✗" }, t.label, t.summary,
                            if t.recommended { "  [recommended]" } else { "" });
                    }
                    if let Some(id) = opt(args, "--strategy") {
                        let t = match ex.trials.iter().find(|t| t.id == id && t.ok) {
                            Some(t) => t, None => return err(&format!("no clean strategy '{id}'")),
                        };
                        if flag(args, "--apply") {
                            match flow::land_merge(&ex.workspace.display().to_string(), &t.branch, newer, id) {
                                Ok(b) => { println!("landed on branch {b} in {newer} — review and merge it"); 0 }
                                Err(e) => err(&e),
                            }
                        } else {
                            match flow::merge_preview(&ex.workspace.display().to_string(), &ex.target_branch, &t.branch) {
                                Ok(p) => {
                                    println!("\npreview of '{id}' ({}):", p.shortstat);
                                    for (st, path) in &p.files { println!("  {st}  {path}"); }
                                    println!("\nadd --apply to land it as a branch in {newer}");
                                    0
                                }
                                Err(e) => err(&e),
                            }
                        }
                    } else {
                        println!("pick a strategy: --strategy <id> [--apply]");
                        0
                    }
                }
                _ => err("these copies don't share history — not the same project"),
            }
        }
        "reclaim" => {
            let root = match pos.first() {
                Some(p) => std::path::PathBuf::from(p),
                None => return err("usage: gitmind fleet reclaim <repo-root> [--apply]"),
            };
            if flag(args, "--apply") {
                let (n, freed) = flow::reclaim_apply(&root);
                println!("reclaimed {} from {n} dir(s)", cockpit::appcatalog::human_size(freed));
            } else {
                let scan = flow::reclaim_scan(&root);
                for (p, b, label) in &scan.items { println!("  {:>10}  {:<18} {p}", cockpit::appcatalog::human_size(*b), label); }
                println!("{} reclaimable in {} dir(s) — add --apply to remove", cockpit::appcatalog::human_size(scan.total_bytes), scan.items.len());
            }
            0
        }
        "dedup" => {
            let groups = match opt(args, "--root") {
                Some(r) => flow::duplicates(std::path::Path::new(r)),
                None => flow::duplicates_all(), // $HOME + any added scan roots
            };
            if groups.is_empty() {
                println!("no duplicate projects found");
                return 0;
            }
            let bases = cockpit::places::repos_bases();
            let path_env = cockpit::tool_path_env();
            println!("{} project(s) exist in more than one place:\n", groups.len());
            for g in &groups {
                let (newest, statuses) = flow::compare_group(&g.paths);
                let canon = cockpit::identity::canonical(&g.paths, &bases);
                println!("{}  ({} copies, key {})", g.name, g.paths.len(), g.key);
                for s in &statuses {
                    let tag = match s.role.as_str() {
                        "newest" => "● newest".to_string(),
                        "older" => format!("↑ {} behind — fast-forward into newest", s.behind),
                        "same" => "= identical".to_string(),
                        "diverged" => format!("⇄ diverged (+{} / -{}) — roll up with: gitmind fleet merge", s.ahead, s.behind),
                        "newer" => "▲ newer than the rest".to_string(),
                        _ => "? unrelated".to_string(),
                    };
                    println!("  {:<8} {}", if *s.path == newest { "[keep]" } else { "" }, s.path);
                    println!("           {tag}");
                }
                if canon != newest {
                    println!("  (canonical by location: {canon})");
                }
                let slug_dir = cockpit::build_dir_for(std::path::Path::new(&newest), &path_env);
                println!("  → all copies build into {}\n", slug_dir.display());
            }
            0
        }
        "scope" => {
            match pos.first().map(|s| s.as_str()) {
                Some("add") => match pos.get(1) {
                    Some(d) => match cockpit::places::add_scan_root(d) {
                        Ok(()) => { println!("scanning {} now includes {d}", "for duplicates"); 0 }
                        Err(e) => err(&e),
                    },
                    None => err("usage: gitmind fleet scope add <dir>"),
                },
                Some("rm") | Some("remove") => match pos.get(1) {
                    Some(d) => match cockpit::places::remove_scan_root(d) { Ok(()) => { println!("removed {d}"); 0 } Err(e) => err(&e) },
                    None => err("usage: gitmind fleet scope rm <dir>"),
                },
                _ => {
                    println!("scan roots (folders gitmind looks in for repos + duplicates):");
                    for r in cockpit::places::scan_roots() { println!("  {r}"); }
                    0
                }
            }
        }
        "suite" => {
            use cockpit::suite;
            match pos.first().map(|s| s.as_str()) {
                Some("list") | None => {
                    let suites = suite::list_suites();
                    if suites.is_empty() { println!("no suites yet — create one with: gitmind fleet suite create"); return 0; }
                    for s in suites { println!("{:<16} {} app(s)   {}", s.name, s.apps.len(), s.description); }
                    0
                }
                Some("show") => match pos.get(1).and_then(|n| suite::load_suite(n)) {
                    Some(s) => {
                        println!("{} — {}", s.name, s.description);
                        for a in &s.apps {
                            let feat = if a.features.is_empty() { String::new() } else { format!("  [{}]", a.features.join(",")) };
                            let tgt = if a.target.is_empty() { "native".into() } else { a.target.clone() };
                            println!("  {:<24} {}  ({}){}", a.name, a.path, tgt, feat);
                        }
                        0
                    }
                    None => err("no such suite (try: gitmind fleet suite list)"),
                },
                Some("create") => {
                    let root = opt(args, "--root").map(std::path::PathBuf::from)
                        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())));
                    match flow::create_default_suite(&root) {
                        Ok(s) => { println!("created suite '{}' with {} app(s) — install with: gitmind fleet suite install {}", s.name, s.apps.len(), s.name); 0 }
                        Err(e) => err(&e),
                    }
                }
                Some("install") => {
                    let s = match pos.get(1).and_then(|n| suite::load_suite(n)) { Some(s) => s, None => return err("no such suite") };
                    let only: Option<Vec<&str>> = opt(args, "--only").map(|s| s.split(',').collect());
                    let apps: Vec<_> = s.apps.iter().filter(|a| only.as_ref().map(|o| o.contains(&a.name.as_str())).unwrap_or(true)).collect();
                    println!("installing {} app(s) from '{}':", apps.len(), s.name);
                    let mut ok = 0;
                    for (i, a) in apps.iter().enumerate() {
                        print!("  [{}/{}] {} … ", i + 1, apps.len(), a.name);
                        use std::io::Write; let _ = std::io::stdout().flush();
                        match flow::install_suite_app(a) {
                            Ok(inst) => { println!("✓ @{}", inst.label); ok += 1; }
                            Err(e) => println!("✗ {e}"),
                        }
                    }
                    println!("{ok}/{} installed — launchers added to your app grid", apps.len());
                    if ok == apps.len() { 0 } else { 1 }
                }
                Some("rm") | Some("remove") => {
                    let name = match pos.get(1) { Some(n) => n, None => return err("usage: gitmind fleet suite rm <name> [<app>]") };
                    match pos.get(2) {
                        Some(app) => {
                            let mut s = match suite::load_suite(name) { Some(s) => s, None => return err("no such suite") };
                            let before = s.apps.len();
                            s.apps.retain(|a| a.name.as_str() != app.as_str());
                            if s.apps.len() == before { return err(&format!("'{app}' isn't in {name}")); }
                            match suite::save_suite(&s) { Ok(()) => { println!("removed {app} from {name}"); 0 } Err(e) => err(&e) }
                        }
                        None => match suite::delete_suite(name) { Ok(()) => { println!("deleted suite {name}"); 0 } Err(e) => err(&e) },
                    }
                }
                Some(other) => err(&format!("unknown suite command: {other} (list|show|create|install|rm)")),
            }
        }
        "group" => {
            match pos.first().map(|s| s.as_str()) {
                Some("list") | None => {
                    println!("BigSilicon app-store groups:");
                    for g in flow::store_groups() {
                        println!("  {:<18} {:>2} app(s)   {}", g.name, g.apps.len(), g.description);
                    }
                    println!("build one with: gitmind fleet group build <name>");
                    0
                }
                Some("build") | Some("install") => {
                    let name = match pos.get(1) { Some(n) => n, None => return err("usage: gitmind fleet group build <name>  (see: gitmind fleet group list)") };
                    let g = match flow::group_by_name(name) { Some(g) => g, None => return err("no such group (try: gitmind fleet group list)") };
                    println!("building {} app(s) from group '{}':", g.apps.len(), g.name);
                    let results = flow::install_suite(&g);
                    let ok = results.iter().filter(|r| r.ok).count();
                    for r in &results { println!("  {} {:<24} {}", if r.ok { "✓" } else { "✗" }, r.name, r.message); }
                    println!("{ok}/{} installed — launchers added to your app grid", results.len());
                    if ok == results.len() { 0 } else { 1 }
                }
                Some(other) => err(&format!("unknown group command: {other} (list|build)")),
            }
        }
        "build-dir" => {
            if let Some(p) = pos.first() {
                match cockpit::set_build_base(p) {
                    Ok(()) => { println!("build base set to {}", cockpit::build_base().display()); 0 }
                    Err(e) => err(&e),
                }
            } else {
                println!("{}", cockpit::build_base().display());
                0
            }
        }
        other => err(&format!("unknown fleet command: {other}")),
    }
}
