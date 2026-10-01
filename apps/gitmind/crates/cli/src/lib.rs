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
        "migrate" => cmd_migrate(rest),
        "var" => cmd_var(rest),
        "app" => cmd_app(rest),
        "check" => cmd_check(rest),
        "patch" => cmd_patch(rest),
        "apply" => cmd_apply(rest),
        "zip" => cmd_zip(rest),
        "fleet" => cmd_fleet(rest),
        "patchset" => cmd_patchset(rest),
        "cosmic" => cmd_cosmic(rest),
        "fp5" => cmd_fp5(rest),
        "depot" => cmd_depot(rest),
        "eink" => cmd_eink(rest),
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

// arg helpers now come from cli-kit (lifted from here) — one implementation, shared.
use cli_kit::{flag, opt};

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

/// `gin migrate` — move delta-hostile files out of git's object store and
/// into the chunk store. Plan by default; `--apply` executes; `--untrack`
/// additionally `git rm --cached`s each file and shields it in .gitignore.
/// `--restore <path>` writes a migrated file's exact bytes back; `--list`
/// prints the migration record. History is never rewritten here.
fn cmd_migrate(rest: &[String]) -> i32 {
    use gitmind_core::migrate;
    let repo = match open_repo() { Ok(r) => r, Err(e) => return err(&e) };

    if flag(rest, "--list") {
        return match migrate::migrated(repo.root()) {
            Ok(entries) => {
                for e in entries {
                    println!("{}  {:>12}  {}", &e.blob_oid[..12], e.size, e.path);
                }
                0
            }
            Err(e) => err(&e),
        };
    }
    if let Some(path) = opt(rest, "--restore") {
        let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
        return match migrate::restore_file(repo.root(), &store, path, None) {
            Ok(n) => { println!("restored {path} ({n} bytes)"); 0 }
            Err(e) => err(&e),
        };
    }

    let mut cfg = migrate::MigrateConfig::defaults();
    cfg.untrack = flag(rest, "--untrack");
    if let Some(ms) = opt(rest, "--min-size") {
        match ms.parse::<u64>() {
            Ok(v) => cfg.min_size = v,
            Err(_) => return err("--min-size takes bytes"),
        }
    }
    let plan = match migrate::scan(&repo, &cfg) { Ok(p) => p, Err(e) => return err(&e) };
    if plan.candidates.is_empty() {
        println!("no delta-hostile tracked files ≥ {} bytes — nothing to migrate", cfg.min_size);
        return 0;
    }
    for c in &plan.candidates {
        let why = match &c.reason {
            migrate::Reason::CompressedFormat(e) => format!(".{e} (compressed format)"),
            migrate::Reason::HighEntropy(b) => format!("entropy {b:.2} b/B"),
        };
        println!("{:>12}  {:<26} {}", c.size, why, c.path);
    }
    println!("{} file(s), {} bytes total", plan.candidates.len(), plan.total_bytes);

    if !flag(rest, "--apply") {
        println!("plan only — re-run with --apply (add --untrack to stop git tracking them)");
        return 0;
    }
    let store = match open_store(&repo) { Ok(s) => s, Err(e) => return err(&e) };
    match migrate::apply(&repo, &store, &plan, &cfg) {
        Ok(r) => {
            let pct = if r.bytes_ingested > 0 {
                100.0 * r.bytes_stored as f64 / r.bytes_ingested as f64
            } else { 0.0 };
            println!(
                "migrated {} file(s): {} bytes in, {} new bytes stored ({pct:.0}% — dedup + raw/zstd), {} untracked",
                r.files, r.bytes_ingested, r.bytes_stored, r.untracked
            );
            println!("record: .gitmind/migrated.jsonl  (restore any file with `gin migrate --restore <path>`)");
            0
        }
        Err(e) => err(&e),
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
    println!("  eink <enroll|probe|deploy|stop|status> <host>   e-ink tablets (reMarkable)");
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
    println!("  migrate [--apply] [--untrack] [--min-size N] [--restore <path>] [--list]");
    println!("                              move delta-hostile files (media/archives/high-entropy) into the chunk store");
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

/// `gitmind depot ...`: this machine as a paired depot client. Each command
/// is one step of the pairing loop (cockpit::depot_client), printed as it
/// stands, so the loop is visible and can be driven by hand.
fn cmd_depot(rest: &[String]) -> i32 {
    use cockpit::depot_client::{default_name, Client, Conf, PairState, DEFAULT_SCOPES};
    let usage = "usage: gitmind depot [status]\n       gitmind depot config <host:port> [ssh=USER@HOST] [root=DIR]\n       gitmind depot pair [name] [scopes]      (default name gitmind-<host>, scopes ingest,packs)\n       gitmind depot approve [--token TOKEN]   (default: over ssh to the depot host)\n       gitmind depot put-crate <file.crate>    (a signed, hash-checked ingest)\n       gitmind depot forget";
    let Some(dir) = Client::default_dir() else { return err("no $XDG_CONFIG_HOME or $HOME") };
    let c = Client::new(&dir);
    let show = |st: &PairState| match st {
        PairState::NoServer => println!("no depot configured: gitmind depot config <host:port> [ssh=USER@HOST]"),
        PairState::Unpaired => println!("not paired: gitmind depot pair"),
        PairState::Pending { id, code } => println!("pending: request {id}, code {code}\n  approve on the depot host: depot pair approve {id} --code {code}\n  or from here:              gitmind depot approve [--token TOKEN]"),
        PairState::Paired { name } => println!("paired as {name}"),
        PairState::Denied => println!("the depot denied the request; gitmind depot pair to ask again"),
        PairState::Lost => println!("the request expired or vanished; gitmind depot pair to ask again"),
    };
    match rest.first().map(String::as_str).unwrap_or("status") {
        "status" => {
            let conf = c.conf();
            println!("config  {}  (server={} ssh={} root={})", dir.join("depot.conf").display(), conf.server, conf.ssh, conf.root);
            show(&c.poll());
            0
        }
        "config" => {
            let Some(server) = rest.get(1) else { return err(usage) };
            let mut conf = Conf { server: cockpit::depot_client::normalize_server(server), ..c.conf() };
            for kv in &rest[2..] {
                match kv.split_once('=') {
                    Some(("ssh", v)) => conf.ssh = v.to_string(),
                    Some(("root", v)) => conf.root = v.to_string(),
                    _ => return err(usage),
                }
            }
            match c.save_conf(&conf) {
                Ok(()) => { println!("server={} ssh={} root={}", conf.server, conf.ssh, conf.root); 0 }
                Err(e) => err(&e),
            }
        }
        "pair" => {
            let name = rest.get(1).cloned().unwrap_or_else(default_name);
            let scopes = rest.get(2).map(String::as_str).unwrap_or(DEFAULT_SCOPES);
            match c.start_pairing(&name, scopes) {
                Ok(st) => { show(&st); 0 }
                Err(e) => err(&e),
            }
        }
        "approve" => {
            let token = match (rest.get(1).map(String::as_str), rest.get(2)) {
                (Some("--token"), Some(t)) => Some(t.clone()),
                (None, _) => None,
                _ => return err(usage),
            };
            let r = match token {
                Some(t) => c.approve_with(&t, None),
                None => c.approve_via_ssh(),
            };
            match r {
                Ok(out) => { println!("{out}"); show(&c.poll()); 0 }
                Err(e) => err(&e),
            }
        }
        "put-crate" => {
            let Some(f) = rest.get(1) else { return err(usage) };
            let body = match std::fs::read(f) {
                Ok(b) => b,
                Err(e) => return err(&format!("{f}: {e}")),
            };
            match c.put_checked("/api/crates", &body) {
                Ok(reply) => { println!("{reply}"); 0 }
                Err(e) => err(&e),
            }
        }
        "forget" => {
            c.forget();
            println!("forgot this client's key (the depot still lists it until `depot pair revoke`)");
            0
        }
        _ => err(usage),
    }
}

/// `gitmind patchset <show|apkbuild|freeze|rollout|bump> <patchset.txt> [args]`:
/// the frozen package base, its patches, and the fleet rollout as text.
/// `gitmind cosmic plan | install USER@HOST | remove USER@HOST`: the COSMIC
/// session onto a postmarketOS phone over ssh, beside its current UI
/// (cockpit::cosmic_setup). `plan` prints the steps and changes nothing.
fn cmd_cosmic(rest: &[String]) -> i32 {
    use cockpit::cosmic_setup::{install_plan, remove_plan, run_over_ssh, Root};
    // The bootstrap account's password (root work goes through `sudo -S`; the
    // key-only fleet account has no root): $GITMIND_SUDO_PASSWORD, or one line
    // on stdin with --password-stdin. Never an argument.
    let password: Option<String> = if rest.iter().any(|a| a == "--password-stdin") {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        Some(line.trim_end_matches(['\n', '\r']).to_string())
    } else {
        std::env::var("GITMIND_SUDO_PASSWORD").ok()
    };
    let root = if password.is_some() { Root::PasswordOnStdin } else { Root::NoPrompt };
    let print = |steps: &[cockpit::patchset::Step]| {
        for (i, s) in steps.iter().enumerate() {
            println!("{:>2}. {}\n    $ {}\n    gate: {:?}{}", i + 1, s.what, s.cmd, s.gate,
                s.rollback.as_ref().map(|r| format!("\n    on failure: {r}")).unwrap_or_default());
        }
    };
    let target = |a: Option<&String>| a.and_then(|t| t.split_once('@')).map(|(u, h)| (u.to_string(), h.to_string()));
    match rest.first().map(String::as_str) {
        Some("plan") => { print(&install_plan(root)); 0 }
        Some(verb @ ("install" | "remove")) => {
            let Some((user, host)) = target(rest.get(1)) else {
                eprintln!("usage: gitmind cosmic {verb} USER@HOST");
                return 2;
            };
            let steps = if verb == "install" { install_plan(root) } else { remove_plan(root) };
            match run_over_ssh(&steps, &user, &host, password.as_deref(), &mut |l| println!("{l}")) {
                Ok(()) => {
                    if verb == "install" {
                        println!("COSMIC is installed beside the current UI. Choose the COSMIC session at the login screen; \
                                  `gitmind cosmic remove {user}@{host}` undoes it.");
                    }
                    0
                }
                Err(e) => { eprintln!("gitmind cosmic {verb}: {e}"); 1 }
            }
        }
        _ => { eprintln!("usage: gitmind cosmic plan | install USER@HOST | remove USER@HOST [--password-stdin]\n  root: the bootstrap account (user@phone) with its password via --password-stdin or $GITMIND_SUDO_PASSWORD;\n  or an account with passwordless doas/sudo, or root"); 2 }
    }
}

/// The parts of `fp5_setup` whose name contains `want` (all of them when `want` is `None`).
fn fp5_select(parts: Vec<(&'static str, Vec<cockpit::patchset::Step>)>, want: Option<&str>) -> Vec<(&'static str, Vec<cockpit::patchset::Step>)> {
    parts.into_iter().filter(|(n, _)| want.map_or(true, |w| n.to_lowercase().contains(&w.to_lowercase()))).collect()
}

/// `gitmind fp5 list | plan [PART] | apply USER@HOST [PART] [--password-stdin]`: replay the
/// Fairphone 5 customisations (cockpit::fp5_setup) over ssh. `list` and `plan` change nothing.
/// PART is a substring of a part's name ("watchdog", "video"); no PART means every part.
fn cmd_fp5(rest: &[String]) -> i32 {
    use cockpit::cosmic_setup::{run_over_ssh, Root};
    let password: Option<String> = if rest.iter().any(|a| a == "--password-stdin") {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        Some(line.trim_end_matches(['\n', '\r']).to_string())
    } else {
        std::env::var("GITMIND_SUDO_PASSWORD").ok()
    };
    let root = if password.is_some() { Root::PasswordOnStdin } else { Root::NoPrompt };
    let args: Vec<&String> = rest.iter().filter(|a| !a.starts_with("--")).collect();
    let usage = "usage: gitmind fp5 list | plan [PART] | apply USER@HOST [PART] [--password-stdin]";
    match args.first().map(|s| s.as_str()) {
        Some("list") => {
            for (n, steps) in cockpit::fp5_setup::all_parts(root) {
                println!("{n}  ({} steps)", steps.len());
            }
            0
        }
        Some("plan") => {
            let parts = fp5_select(cockpit::fp5_setup::all_parts(root), args.get(1).map(|s| s.as_str()));
            if parts.is_empty() { eprintln!("no part matches\n{usage}"); return 2; }
            for (n, steps) in parts {
                println!("== {n}");
                for (i, s) in steps.iter().enumerate() {
                    println!("{:>2}. {}\n    $ {}\n    gate: {:?}{}", i + 1, s.what, s.cmd, s.gate,
                        s.rollback.as_ref().map(|r| format!("\n    on failure: {r}")).unwrap_or_default());
                }
            }
            0
        }
        Some("apply") => {
            let Some((user, host)) = args.get(1).and_then(|t| t.split_once('@')) else { eprintln!("{usage}"); return 2 };
            let parts = fp5_select(cockpit::fp5_setup::all_parts(root), args.get(2).map(|s| s.as_str()));
            if parts.is_empty() { eprintln!("no part matches\n{usage}"); return 2; }
            for (n, steps) in parts {
                println!("== {n}");
                if let Err(e) = run_over_ssh(&steps, user, host, password.as_deref(), &mut |l| println!("{l}")) {
                    eprintln!("gitmind fp5 apply: {n}: {e}");
                    return 1;
                }
            }
            0
        }
        _ => { eprintln!("{usage}"); 2 }
    }
}

fn cmd_patchset(rest: &[String]) -> i32 {
    use cockpit::patchset::{apkbuild_source, rollout, Freeze, Patchset, Where};
    let usage = "usage: gitmind patchset show <patchset.txt>\n       gitmind patchset apkbuild <patchset.txt> [arch]\n       gitmind patchset freeze <patchset.txt> <repo-url> [more pkgs...]\n       gitmind patchset rollout <patchset.txt> <repo-url> <builder-aports-dir> [--untested]\n       gitmind patchset bump <patchset.txt> <new-version> [base-pkgrel]";
    let (sub, path) = match (rest.first(), rest.get(1)) {
        (Some(s), Some(p)) => (s.as_str(), p),
        _ => return err(usage),
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return err(&format!("{path}: {e}")),
    };
    let ps = match Patchset::from_text(&text) {
        Ok(p) => p,
        Err(e) => return err(&format!("{path}: {e}")),
    };
    let freeze = |url: &str, extra: &[String]| Freeze { repo_url: url.to_string(), tag: ps.tag.clone(), pinned: std::iter::once(ps.pkg.clone()).chain(extra.iter().cloned()).collect() };
    match sub {
        "show" => {
            println!("{} {} (upstream {}-r{} + {} patch(es)) tag @{}", ps.pkg, ps.version_string(), ps.base_version, ps.base_pkgrel, ps.patches.len(), ps.tag);
            for p in &ps.patches {
                println!("  {}  {}", p.file, p.subject);
            }
            println!("  packages as: {} ({}), builddir {}, package(): {}", ps.recipe.url, if ps.recipe.license.is_empty() { "NO LICENSE SET: the APKBUILD will say unknown" } else { &ps.recipe.license }, ps.builddir(), ps.recipe.package.join("; "));
            if ps.tested_on.is_empty() {
                println!("  UNTESTED: a rollout to the fleet is refused until one device is recorded as tested");
            }
            for t in &ps.tested_on {
                println!("  tested on {} ({}): {}", t.device, t.date, t.note);
            }
            if !ps.notes.is_empty() {
                println!("  notes: {}", ps.notes);
            }
            0
        }
        "apkbuild" => {
            let arch = rest.get(2).map(|s| s.as_str()).unwrap_or("aarch64");
            print!("{}", apkbuild_source(&ps, arch, &["cargo", "rust", "libinput-dev", "libseat-dev", "libdisplay-info-dev", "libxkbcommon-dev", "mesa-dev", "pixman-dev", "udev-dev", "pkgconf"]));
            0
        }
        "freeze" => {
            let Some(url) = rest.get(2) else { return err(usage) };
            let f = freeze(url, &rest[3..]);
            println!("# /etc/apk/repositories line:\n{}\n# /etc/apk/world entries:\n{}\n# one-shot install:\n{}", f.repositories_line(), f.world_entries().join("\n"), f.install_cmd());
            0
        }
        "rollout" => {
            let (Some(url), Some(dir)) = (rest.get(2), rest.get(3)) else { return err(usage) };
            let untested_ok = rest.iter().any(|a| a == "--untested");
            let f = freeze(url, &[]);
            match rollout(&ps, &f, dir, !untested_ok) {
                Ok(steps) => {
                    for (i, s) in steps.iter().enumerate() {
                        println!("{:>2}. [{}] {}\n    $ {}\n    gate: {:?}{}", i + 1, match s.on { Where::Builder => "builder", Where::Device => "device " }, s.what, s.cmd, s.gate, s.rollback.as_ref().map(|r| format!("\n    rollback: {r}")).unwrap_or_default());
                    }
                    0
                }
                Err(e) => err(&e),
            }
        }
        "bump" => {
            let Some(v) = rest.get(2) else { return err(usage) };
            let rel = rest.get(3).and_then(|r| r.parse().ok()).unwrap_or(0);
            let b = ps.bump(v, rel);
            match std::fs::write(path, b.to_text()) {
                Ok(()) => {
                    println!("{path}: now {} (tests cleared; test on one device with `rollout --untested`, then record `tested = ...`)", b.version_string());
                    0
                }
                Err(e) => err(&format!("{path}: {e}")),
            }
        }
        _ => err(usage),
    }
}

/// e-ink tablets (reMarkable): enrol, probe, deploy/launch, stop, status.
///
/// Every subcommand re-probes the device rather than trusting saved state:
/// it is one SSH round-trip, and tablets change under you (OS updates wipe
/// the root filesystem; Oxide/AppLoad get installed or removed).
fn cmd_eink(rest: &[String]) -> i32 {
    use cockpit::eink_fleet as ef;
    const USAGE: &str = "usage: gitmind eink <enroll|probe|survey|build|deploy|list|remove|stop|status|activate|wifi-ssh|pkg|retrust> <host> ...\n  \
        build <host> <bin> [--engine auto|host|container]   build an eink-kit app for that tablet (container = podman/docker rust:alpine)\n  \
        list <host>                         apps gitmind put in the tablet's AppLoad sidebar\n  \
        remove <host> <app>                 take one out again (never touches other apps)\n  \
        activate <host>                     after a reboot: start xovi or the launcher, whichever is needed\n  \
        wifi-ssh <host>                     turn on SSH over Wi-Fi (Paper Pro family; run over USB)\n  \
        pkg <host> runner|bootstrap|add|del|list|update|audio   packages like a laptop (Alpine under /home)\n  \
        survey <host>                       what the device has (run at enrol); builds pick features from it\n  \
        restart-ui <host>                   restart xochitl so AppLoad re-reads changed app settings\n  \
        (every command: --user U, or GITMIND_RM_USER; default root)\n  \
        retrust <host>                      accept the tablet's NEW host key (after an OS update)\n  \
        enroll <host>                       first contact: password (GITMIND_RM_PASSWORD or stdin) → key\n  \
        probe <host>                        model, display servers, route\n  \
        deploy <host> <bin> [--name N] [--title T] [--icon PNG] [--windowed] [--profile P] [--userland alpine] [-- args...]\n  \
        (--userland alpine on build/deploy: dynamic, inside the Alpine root; needs `pkg bootstrap` and the runner)\n  \
        stop <host> <app>\n  \
        status <host> <app>";
    let sub = match rest.first() { Some(s) => s.as_str(), None => return err(USAGE) };
    let Some(host) = rest.get(1).cloned() else { return err(USAGE) };
    let (flags, app_args) = match rest.iter().position(|a| a == "--") {
        Some(i) => (&rest[2..i], rest[i + 1..].to_vec()),
        None => (&rest[2..], vec![]),
    };
    let pos: Vec<&String> = eink_positionals(flags);
    // `--user U`, else GITMIND_RM_USER, else root (reMarkable: "root at the
    // time of writing", so a default, not a constant).
    let user = opt(flags, "--user").map(str::to_string)
        .or_else(|| std::env::var("GITMIND_RM_USER").ok().filter(|u| !u.trim().is_empty()))
        .unwrap_or_else(|| ef::DEFAULT_USER.to_string());
    // `--userland alpine`: build dynamic and run inside the Alpine root.
    let userland = match opt(flags, "--userland") {
        Some(u) => match ef::Userland::parse(u) { Some(u) => u, None => { eprintln!("--userland: static|alpine"); return 2; } },
        None => ef::Userland::Static,
    };
    let mut remote = ef::SshRemote { user: user.clone(), host: host.clone() };
    let need = |a: Option<ef::Activation>| match a {
        Some(a) => format!("\n          needs, once per boot: {} → `gitmind eink activate {host}`", a.label()),
        None => String::new(),
    };
    let describe = |p: &ef::Probe, d: &ef::Displays| {
        println!("  model   {}", p.model.map(|m| m.label()).unwrap_or("unknown (set it in the roster)"));
        println!("          identified by: {}", p.how);
        if let Some(c) = &p.unknown_codename {
            println!("          motd names \"{c}\", which gitmind does not know yet");
        }
        println!("  target  {}{}", p.triple.unwrap_or("unknown"), d.glibc.as_ref().map(|g| format!(" (glibc {g}: build with zigbuild .{g})")).unwrap_or_default());
        println!("  displays qtfb={} appload={} blight_lib={} blight_running={}",
            d.qtfb, d.appload_dir.as_deref().unwrap_or("-"), d.blight_lib.as_deref().unwrap_or("-"), d.blight_running);
        match d.readiness() {
            Ok((ef::Route::Blight, a)) => println!("  route   Blight: gitmind can launch apps remotely{}", need(a)),
            Ok((ef::Route::AppLoad { dir }, a)) => println!("  route   AppLoad ({dir}): gitmind installs, you tap the icon{}", need(a)),
            Err(e) => println!("  route   none: {e}"),
        }
        if d.rootfs_ro || d.etc_overlay {
            println!("  system  read-only root{}; gitmind writes only under /home/root",
                if d.etc_overlay { ", /etc resets at boot" } else { "" });
        }

    };
    match sub {
        "enroll" => {
            let password = std::env::var("GITMIND_RM_PASSWORD").ok().or_else(|| {
                use std::io::BufRead;
                eprint!("SSH password for {user}@{host} (Settings → General → Help → About → Copyrights and licenses; blank if a key is installed): ");
                let mut line = String::new();
                std::io::stdin().lock().read_line(&mut line).ok()?;
                Some(line.trim().to_string()).filter(|s| !s.is_empty())
            });
            let mut e = ef::SshEnroller { user: user.clone(), host: host.clone() };
            match ef::enroll(&mut e, password.as_deref(), &mut |ev| eprintln!("  · {ev:?}")) {
                Ok(r) => {
                    println!("enrolled {user}@{host}{}", if r.used_password { " (keys installed; no more passwords)" } else { "" });
                    // Which tablet this is (cookie, SoC serial; USB identity if on a cable).
                    if let Some(l) = cockpit::tablet_identity::usb_link_for(&host) { println!("  on {}", l.describe()); }
                    if let Ok(out) = ef::Remote::run(&mut remote, &cockpit::tablet_identity::identity_cmd()) {
                        let id = cockpit::tablet_identity::parse_identity(&out);
                        println!("  device id {} · soc {} · {}", id.device_id, id.soc_serial.as_deref().unwrap_or("-"), id.dt_model.as_deref().unwrap_or("-"));
                    }
                    // What the device HAS, stored for builds to use.
                    match ef::Remote::run(&mut remote, &cockpit::capabilities::survey_cmd()) {
                        Ok(out) => {
                            let _ = cockpit::capabilities::save(&cockpit::tablet_identity::device_key(&host), &out);
                            for l in cockpit::capabilities::parse(&out).summary() { println!("  {l}"); }
                        }
                        Err(e) => eprintln!("  (hardware survey failed: {e})"),
                    }
                    describe(&r.probe, &r.displays);
                    0
                }
                Err(e) if e == ef::HOSTKEY_CHANGED => eink_err(&format!("{e}\n  → `gitmind eink retrust {host}`, then enroll again")),
                Err(e) => eink_err(&e),
            }
        }
        "probe" => {
            let p = match remote_probe(&mut remote) { Ok(p) => p, Err(e) => return eink_err(&e) };
            println!("{host}:");
            describe(&p.0, &p.1);
            0
        }
        "deploy" => {
            let Some(bin) = pos.first() else { return err(USAGE) };
            let name = opt(flags, "--name").map(str::to_string).unwrap_or_else(|| {
                std::path::Path::new(bin.as_str()).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()
            });
            let (probe, displays) = match remote_probe(&mut remote) { Ok(p) => p, Err(e) => return eink_err(&e) };
            let (route, activation) = match displays.readiness() { Ok(r) => r, Err(e) => return eink_err(&e) };
            if let (ef::Route::Blight, Some(a)) = (&route, activation) {
                return eink_err(&format!("the tablet needs `{}` since its last boot: run `gitmind eink activate {host}`", a.label()));
            }
            if let Err(e) = ef::preflight_binary_for(bin, &route, probe.model, userland) {
                return eink_err(&format!("not deployed: {e}"));
            }
            if userland == ef::Userland::AlpineRoot {
                match ef::Remote::run(&mut remote, &cockpit::alpine_root::probe_cmd()) {
                    Ok(o) => if let Some(why) = cockpit::alpine_root::parse_probe(&o).not_ready() { return eink_err(&format!("not deployed: {why}")); },
                    Err(e) => return eink_err(&e),
                }
            }
            let profile = match opt(flags, "--profile") {
                Some(p) => match ef::RefreshProfile::parse(p) { Some(p) => p, None => return eink_err("--profile: fast-color|fast-mono|quality") },
                None => ef::RefreshProfile::fastest(probe.model.map(|m| m.panel()).unwrap_or(ef::Panel::Mono)),
            };
            let args: Vec<&str> = app_args.iter().map(String::as_str).collect();
            let env = ef::LaunchEnv { profile, model: probe.model, blight_lib: displays.blight_lib.clone() };
            let opts = ef::AppLoadOptions {
                display_name: opt(flags, "--title").map(str::to_string).unwrap_or_else(|| name.clone()),
                fullscreen: !flags.iter().any(|f| f == "--windowed"),
                args: app_args.clone(),
                userland,
            };
            let icon = opt(flags, "--icon");
            match ef::deliver(&mut remote, &route, &name, bin, &env, &args, Some((&opts, icon)), &mut |ev| eprintln!("  · {ev:?}")) {
                Ok(ef::Delivered::Launched(ef::Outcome::Running { pid })) => {
                    println!("{name} running on {host} (pid {pid}, {})", profile.as_env());
                    0
                }
                Ok(ef::Delivered::Launched(ef::Outcome::ExitedEarly { log_tail })) => {
                    eprintln!("{name} exited right after launch. Its last output:\n{log_tail}");
                    for f in ef::diagnose(&log_tail) { eprint!("\n{}", f.render()); }
                    1
                }
                Ok(ef::Delivered::Installed { path }) => {
                    println!("installed tool {path} (not an app: no AppLoad entry)");
                    0
                }
                Ok(ef::Delivered::Registered { manifest_dir, change }) => {
                    if let Some(n) = change.note() { eprintln!("  {n}"); }
                    println!("{name} installed for AppLoad ({manifest_dir}); on the tablet open AppLoad from the sidebar and tap ↻ once");
                    if activation.is_some() {
                        println!("  (xovi is not running since the last boot: `gitmind eink activate {host}` first)");
                    }
                    0
                }
                Err(e) => eink_err(&e),
            }
        }
        "build" => {
            let Some(bin) = pos.first() else { return err(USAGE) };
            let (probe, d) = match remote_probe(&mut remote) { Ok(p) => p, Err(e) => return eink_err(&e) };
            let (route, _) = match d.readiness() { Ok(r) => r, Err(e) => return eink_err(&e) };
            let root = cockpit::ops::repo_root();
            let Some(app) = ef::discover_tablet_apps(&root).into_iter().find(|a| a.bin == **bin) else {
                return eink_err(&format!("{bin}: not an eink-kit app in {}", root.display()));
            };
            let mut t = ef::build_target_for(&route, probe.model, userland);
            if userland == ef::Userland::AlpineRoot {
                // Built against the SAME Alpine release as the root it runs in.
                let st = match ef::Remote::run(&mut remote, &cockpit::alpine_root::probe_cmd()) {
                    Ok(o) => cockpit::alpine_root::parse_probe(&o), Err(e) => return eink_err(&e) };
                if !st.root { return eink_err(&st.not_ready().unwrap_or_default()); }
                t.container_image = st.release.as_deref().and_then(cockpit::alpine_root::image_for_release);
                eprintln!("  Alpine root {}: building with {}", st.release.as_deref().unwrap_or("?"), t.container_image.as_deref().unwrap_or("the default image"));
            }
            let engine = match opt(flags, "--engine") {
                Some(e) => match ef::Engine::parse(e) { Some(e) => e, None => return eink_err("--engine: auto|host|container") },
                None => ef::Engine::Auto,
            };
            let caps = cockpit::capabilities::load(&cockpit::tablet_identity::device_key(&host));
            if caps.is_none() { eprintln!("  (no hardware survey for {host}: default features; `gitmind eink survey {host}`)"); }
            let plan = match ef::plan_build_for(engine, &root, &app, &t, &std::env::var("PATH").unwrap_or_default(),
                std::env::var("HOME").ok().as_deref(), std::env::var("CARGO").ok().as_deref(), caps.as_ref()) {
                Ok(p) => p,
                Err(e) => return eink_err(&e),
            };
            eprintln!("  {}", plan.note);
            eprintln!("  $ {}  (in {}; {})", plan.argv.join(" "), plan.cwd.display(), t.why);
            // The shim patch rewrites Cargo.lock: saved and restored around the build.
            let st = ef::with_lock_preserved(&plan, || std::process::Command::new(&plan.argv[0]).args(&plan.argv[1..]).current_dir(&plan.cwd).envs(plan.env.clone()).status());
            match st {
                Ok(s) if s.success() => {}
                Ok(s) => return eink_err(&format!("build failed ({s})")),
                Err(e) => return eink_err(&format!("could not run {}: {e}", plan.argv[0])),
            }
            let native = ef::native_for(&t);
            let rel = ef::tablet_build_output(bin, &t, native);
            let Some(path) = [plan.output.clone()].into_iter().chain(app.dir.ancestors().map(|d| d.join(&rel))).find(|p| p.exists()) else {
                return eink_err(&format!("built, but {} not found", plan.output.display()));
            };
            let path = path.to_string_lossy().to_string();
            match ef::preflight_binary_for(&path, &route, probe.model, userland) {
                Ok(_) => { println!("{path}"); 0 }
                Err(e) => eink_err(&format!("built, but it will not run on the tablet: {e}")),
            }
        }
        "list" | "remove" => {
            let (_, d) = match remote_probe(&mut remote) { Ok(p) => p, Err(e) => return eink_err(&e) };
            let Some(dir) = d.appload_dir else { return eink_err("AppLoad is not installed on this tablet") };
            if sub == "list" {
                match ef::appload_list_cmd(&dir).and_then(|c| ef::Remote::run(&mut remote, &c)) {
                    Ok(out) => {
                        for (n, t) in ef::parse_appload_list(&out) { println!("{n}\t{t}"); }
                        0
                    }
                    Err(e) => eink_err(&e),
                }
            } else {
                let Some(app) = pos.first() else { return err(USAGE) };
                match ef::appload_remove_cmd(app, &dir).and_then(|c| ef::Remote::run(&mut remote, &c)) {
                    Ok(out) => { println!("{}", out.trim()); 0 }
                    Err(e) => eink_err(&e),
                }
            }
        }
        "activate" => {
            let (_, d) = match remote_probe(&mut remote) { Ok(p) => p, Err(e) => return eink_err(&e) };
            match d.readiness() {
                Ok((_, None)) if d.screen_ready() => { println!("{host}: already active, apps can reach the screen"); 0 }
                Ok((_, None)) => eink_err("no display server is running and none can be started from here \
                    (AppLoad without xovi's start script, or no launcher configured)"),
                Ok((_, Some(a))) => {
                    eprintln!("  · {}", a.label());
                    match ef::Remote::run(&mut remote, &ef::activation_cmd(a)) {
                        Ok(out) => { println!("{}", out.trim()); 0 }
                        Err(e) => eink_err(&e),
                    }
                }
                Err(e) => eink_err(&e),
            }
        }
        "restart-ui" => match ef::Remote::run(&mut remote, ef::RESTART_UI_CMD) {
            Ok(out) => { println!("tablet UI restarted ({}): AppLoad has re-read every manifest", out.trim()); 0 }
            Err(e) => eink_err(&e),
        },
        "survey" => match ef::Remote::run(&mut remote, &cockpit::capabilities::survey_cmd()) {
            Ok(out) => {
                let _ = cockpit::capabilities::save(&cockpit::tablet_identity::device_key(&host), &out);
                for l in cockpit::capabilities::parse(&out).summary() { println!("{l}"); }
                println!("(saved; builds for {host} use it)");
                0
            }
            Err(e) => eink_err(&e),
        },
        "wifi-ssh" => {
            let (_, d) = match remote_probe(&mut remote) { Ok(p) => p, Err(e) => return eink_err(&e) };
            if !d.wlan_ssh_tool {
                return eink_err("this tablet has no rm-ssh-over-wlan (not a Paper Pro-family OS?); SSH over Wi-Fi may already be on");
            }
            match ef::Remote::run(&mut remote, ef::WLAN_SSH_ON) {
                Ok(out) => { println!("{}\nSSH over Wi-Fi is on; the Wi-Fi address is under the password in Copyrights and licenses", out.trim()); 0 }
                Err(e) => eink_err(&e),
            }
        }
        // Packages on the tablet, like a laptop: an Alpine userland under /home.
        //   pkg <host> runner <alpine-run binary>   install the runner (build it with `build <host> alpine-run`)
        //   pkg <host> bootstrap [--mirror URL] [--branch B]
        //   pkg <host> add|del <pkg>...   pkg <host> list|update
        //   pkg <host> audio               install bluez-alsa and start it for this boot
        "pkg" => {
            use cockpit::alpine_root as al;
            let action = pos.first().map(|s| s.as_str()).unwrap_or("");
            let names: Vec<&str> = pos.iter().skip(1).map(|s| s.as_str()).collect();
            let mirror = al::Mirror {
                base: opt(flags, "--mirror").unwrap_or(al::DEFAULT_MIRROR).to_string(),
                branch: opt(flags, "--branch").unwrap_or(al::DEFAULT_BRANCH).to_string(),
            };
            // A depot mirror is asked first whether it can satisfy the whole
            // install (dependency closure vs what was ingested); a partial
            // mirror otherwise fails halfway with a bare 404.
            let wanted: Vec<&str> = match action {
                "bootstrap" => ["apk-tools-static", "alpine-keys"].into_iter().chain(al::BASE_PACKAGES.iter().copied()).collect(),
                "add" | "check" => names.clone(),
                "audio" => vec!["bluez-alsa", "bluez-alsa-utils", "alsa-utils"],
                _ => vec![],
            };
            if !wanted.is_empty() && !cli_flag(flags, "--force") {
                let arch = match ef::Remote::run(&mut remote, "uname -m") { Ok(a) => a.trim().to_string(), Err(e) => return eink_err(&e) };
                match al::check_mirror(&mirror, &arch, &wanted) {
                    Some(Ok(c)) if c.ok() => eprintln!("depot has everything: {} package(s) for {arch}", c.needed),
                    Some(Ok(c)) => {
                        eprintln!("the depot cannot satisfy this install for {arch}:");
                        for k in &c.absent_repos { eprintln!("  no repository {k}"); }
                        for (k, f) in &c.missing { eprintln!("  missing {k}/{f}"); }
                        for u in &c.unresolved { eprintln!("  nothing provides {u}"); }
                        eprintln!("\n{}", c.fetch_script());
                        eprintln!("(--force to try anyway)");
                        return 1;
                    }
                    Some(Err(e)) => return eink_err(&format!("asking the depot: {e}")),
                    None => {} // not a depot: apk reports as usual
                }
                if action == "check" { return 0; }
            }
            let cmd = match action {
                "check" => return eink_err("pkg check <host> <package>... --mirror <depot>/alpine (only a depot can be asked)"),
                "runner" => {
                    let Some(bin) = names.first() else { return eink_err("pkg runner <path to the built alpine-run>") };
                    let (probe, d) = match remote_probe(&mut remote) { Ok(p) => p, Err(e) => return eink_err(&e) };
                    let route = d.readiness().map(|r| r.0).unwrap_or(ef::Route::AppLoad { dir: String::new() });
                    if let Err(e) = ef::preflight_binary(bin, &route, probe.model) { return eink_err(&e); }
                    if let Err(e) = ef::Remote::run(&mut remote, &ef::prepare_cmd()) { return eink_err(&e); }
                    if let Err(e) = ef::Remote::push(&mut remote, bin, &ef::staging_path("alpine-run")) { return eink_err(&e); }
                    match ef::install_cmd("alpine-run") { Ok(c) => c, Err(e) => return eink_err(&e) }
                }
                "bootstrap" => al::bootstrap_cmd(&mirror),
                "add" => match al::add_cmd(&names) { Ok(c) => c, Err(e) => return eink_err(&e) },
                "del" => match al::del_cmd(&names) { Ok(c) => c, Err(e) => return eink_err(&e) },
                "list" => al::list_cmd(),
                "update" => al::update_cmd(),
                "audio" => format!("{} && {}", al::add_cmd(&["bluez-alsa", "bluez-alsa-utils", "alsa-utils"]).unwrap(), al::bluealsa_start_cmd()),
                _ => return eink_err("pkg <host> runner|bootstrap|add|del|list|update|audio|check ..."),
            };
            match ef::Remote::run(&mut remote, &cmd) {
                Ok(out) => { println!("{}", out.trim_end()); 0 }
                Err(e) => eink_err(&e),
            }
        }
        "retrust" if cockpit::tablet_identity::usb_link_for(&host).is_some() => {
            let l = cockpit::tablet_identity::usb_link_for(&host).unwrap();
            match cockpit::tablet_identity::untrust(&l) { Ok(m) => { println!("{m}; enrol again to trust its new key"); 0 } Err(e) => eink_err(&e) }
        }
        "retrust" => match cockpit::deploy::retrust_host(&host) {
            Ok(m) => { println!("{m}"); 0 }
            Err(e) => eink_err(&e),
        },
        "stop" | "status" => {
            let Some(app) = pos.first() else { return err(USAGE) };
            let cmd = if sub == "stop" { ef::stop_cmd(app) } else { ef::status_cmd(app, 0) };
            let cmd = match cmd { Ok(c) => c, Err(e) => return eink_err(&e) };
            match cockpit::eink_fleet::Remote::run(&mut remote, &cmd) {
                Ok(out) if sub == "status" => {
                    let st = ef::parse_status(&out);
                    match st.pid { Some(p) => println!("{app}: running (pid {p})"), None => println!("{app}: stopped") }
                    if !st.log_tail.is_empty() { println!("{}", st.log_tail); }
                    for f in ef::diagnose(&st.log_tail) { print!("\n{}", f.render()); }
                    0
                }
                Ok(_) => { println!("{app}: stopped"); 0 }
                Err(e) => eink_err(&e),
            }
        }
        _ => err(USAGE),
    }
}

/// `gitmind eink` flags that take a value (the next argument).
const EINK_VALUE_FLAGS: &[&str] = &["--user", "--name", "--title", "--icon", "--profile", "--engine", "--mirror", "--branch", "--userland"];

/// Positional arguments: everything that is neither a flag nor a value
/// flag's value. (Dropping only the `--x` tokens kept their values, so
/// `pkg add bluez-alsa --mirror URL` tried to install a package named URL.)
fn eink_positionals(flags: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in flags {
        if skip {
            skip = false;
        } else if a.starts_with("--") {
            skip = EINK_VALUE_FLAGS.contains(&a.as_str());
        } else {
            out.push(a);
        }
    }
    out
}

#[cfg(test)]
mod eink_args_tests {
    #[test]
    fn value_flags_do_not_leak_into_positionals() {
        let v: Vec<String> = ["add", "bluez-alsa", "--mirror", "http://d/alpine", "--force", "alsa-utils", "--user", "root"].iter().map(|s| s.to_string()).collect();
        let p: Vec<&str> = super::eink_positionals(&v).iter().map(|s| s.as_str()).collect();
        assert_eq!(p, vec!["add", "bluez-alsa", "alsa-utils"]);
        let d: Vec<String> = ["--name", "X", "./bin/app"].iter().map(|s| s.to_string()).collect();
        assert_eq!(super::eink_positionals(&d), vec!["./bin/app"], "deploy <host> --name X <bin>");
    }
}

fn cli_flag(flags: &[String], name: &str) -> bool {
    flags.iter().any(|f| f == name)
}

/// An eink error, followed by what gitmind can say about it (stoat's lesson:
/// a failure is explained, not just reported): rustc-style findings with
/// notes, help and any fix to copy.
fn eink_err(msg: &str) -> i32 {
    let code = err(msg);
    for f in cockpit::eink_fleet::diagnose(msg) {
        eprint!("\n{}", f.render());
    }
    code
}

fn remote_probe(remote: &mut cockpit::eink_fleet::SshRemote) -> Result<(cockpit::eink_fleet::Probe, cockpit::eink_fleet::Displays), String> {
    use cockpit::eink_fleet::{self as ef, Remote};
    let p = ef::parse_probe(&remote.run(ef::probe_cmd())?);
    let d = ef::parse_displays(&remote.run(ef::displays_probe_cmd())?);
    Ok((p, d))
}

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
                    let name = match pos.get(1) { Some(n) => n, None => return err("usage: gitmind fleet group build <name> [--to user@host] [--target T]") };
                    let g = match flow::group_by_name(name) { Some(g) => g, None => return err("no such group (try: gitmind fleet group list)") };
                    // --to user@host → build here, push+install each app on the phone
                    if let Some(dest) = opt(rest, "--to") {
                        let (user, host) = match dest.split_once('@') {
                            Some((u, h)) if !u.is_empty() && !h.is_empty() => (u.to_string(), h.to_string()),
                            _ => return err("--to expects user@host"),
                        };
                        // target: explicit, else detect the device's triple over ssh, else native
                        let target = match opt(rest, "--target") {
                            Some(t) => t.to_string(),
                            None => cockpit::deploy::remote_capture(&user, &host, "export PATH=\"$HOME/.cargo/bin:$PATH\" && rustc -vV")
                                .ok()
                                .and_then(|o| cockpit::buildflags::host_triple_from_rustc(&o))
                                .map(|h| cockpit::buildflags::normalize_native(&h))
                                .unwrap_or_default(),
                        };
                        println!("building {} app(s) here for '{}' and installing on {host} …", g.apps.len(), if target.is_empty() { "native" } else { &target });
                        let results = flow::install_group_remote(&g, &target, &user, &host);
                        let ok = results.iter().filter(|r| r.ok).count();
                        for r in &results { println!("  {} {:<24} {}", if r.ok { "✓" } else { "✗" }, r.name, r.message); }
                        println!("{ok}/{} installed on {host}", results.len());
                        return if ok == results.len() { 0 } else { 1 };
                    }
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
        "package" => {
            let dir = match pos.first() {
                Some(d) => std::path::PathBuf::from(d),
                None => return err("usage: gitmind fleet package <dir> [--rpm|--apk] [--out <dir>]"),
            };
            let format = if flag(rest, "--apk") { flow::RepoFormat::Apk }
                else if flag(rest, "--rpm") { flow::RepoFormat::Rpm }
                else { flow::host_repo_format() };
            let out = opt(rest, "--out").map(std::path::PathBuf::from).unwrap_or_else(|| dir.join("gitmind-pkg"));
            println!("building {} package from {} …", format.as_str(), dir.display());
            match flow::package_project(&dir, format, &out) {
                Ok(o) => {
                    match &o.artifact {
                        Some(a) => println!("✓ {}\n  package: {}", o.message, a.display()),
                        None => println!("• {}\n  spec/APKBUILD: {}", o.message, o.staged.manifest.display()),
                    }
                    0
                }
                Err(e) => err(&e),
            }
        }
        "repo" => {
            match pos.first().map(|s| s.as_str()) {
                Some("add") => {
                    let (Some(pkg), Some(to)) = (pos.get(1), opt(rest, "--to")) else {
                        return err("usage: gitmind fleet repo add <pkg> --to <repo-dir> [--rpm|--apk]");
                    };
                    let format = if flag(rest, "--apk") { flow::RepoFormat::Apk } else { flow::RepoFormat::Rpm };
                    match flow::repo_add(std::path::Path::new(pkg), std::path::Path::new(to), format) {
                        Ok(msg) => { println!("{msg}"); 0 }
                        Err(e) => err(&e),
                    }
                }
                Some("serve") => {
                    let dir = match pos.get(1) {
                        Some(d) => std::path::PathBuf::from(d),
                        None => return err("usage: gitmind fleet repo serve <repo-dir> [--port N] [--rpm|--apk]"),
                    };
                    let port: u16 = opt(rest, "--port").and_then(|p| p.parse().ok()).unwrap_or(8080);
                    let format = if flag(rest, "--apk") { flow::RepoFormat::Apk } else { flow::RepoFormat::Rpm };
                    let ip = cockpit::netshare::local_ip().unwrap_or_else(|| "127.0.0.1".to_string());
                    let baseurl = format!("http://{ip}:{port}/");
                    println!("{}", flow::repo_client_config(format, &baseurl, "gitmind"));
                    println!("serving {} at {baseurl} (Ctrl-C to stop) …", dir.display());
                    match flow::serve_repo(&dir, port) { Ok(()) => 0, Err(e) => err(&e) }
                }
                _ => err("usage: gitmind fleet repo <add|serve> …"),
            }
        }
        "account" => {
            match flow::self_account() {
                Some(a) => { println!("this device's fleet account: {a}"); 0 }
                None => err("no ssh identity key found (~/.ssh/id_ed25519) to derive an account"),
            }
        }
        "advertise" => {
            let port: u16 = opt(rest, "--port").and_then(|p| p.parse().ok()).unwrap_or(22);
            let parse_hp = |s: &str| -> Option<(String, u16)> {
                let (h, p) = s.rsplit_once(':')?;
                Some((h.to_string(), p.parse().ok()?))
            };
            let public = opt(rest, "--public").and_then(parse_hp);
            let relay = opt(rest, "--relay").and_then(parse_hp);
            let ttl: u64 = opt(rest, "--ttl").and_then(|t| t.parse().ok()).unwrap_or(3600);
            let advert = match flow::build_self_advert(port, public, relay, ttl) {
                Some(a) => a,
                None => return err("could not build advert (no ssh identity key?)"),
            };
            let mut store = flow::default_advert_store();
            match flow::advertise(&mut store, &advert) {
                Ok(()) => {
                    println!("advertised account {} ({} endpoint(s), ttl {}s)", advert.account, advert.endpoints.len(), advert.ttl_secs);
                    for e in &advert.endpoints { println!("  {}  {}:{}", e.kind.as_str(), e.host, e.port); }
                    if advert.endpoints.iter().all(|e| e.kind == cockpit::rendezvous::EndpointKind::Lan) {
                        eprintln!("note: only a LAN endpoint — pass --public <host:port> so off-network peers can reach you (or run snapcore DHT)");
                    }
                    0
                }
                Err(e) => err(&e),
            }
        }
        "resolve" => {
            let account = match pos.first() { Some(a) => a, None => return err("usage: gitmind fleet resolve <account>") };
            let store = flow::default_advert_store();
            match flow::resolve_peer(&store, account) {
                Some(a) => {
                    println!("{} @ {} — {} endpoint(s):", a.account, a.hostname, a.endpoints.len());
                    for e in &a.endpoints { println!("  {}  {}:{}", e.kind.as_str(), e.host, e.port); }
                    0
                }
                None => err("account not found in the rendezvous store (not advertised, or DHT not reachable)"),
            }
        }
        "reach" => {
            let account = match pos.first() { Some(a) => a, None => return err("usage: gitmind fleet reach <account> [-- <remote cmd>]") };
            let store = flow::default_advert_store();
            match flow::reach(&store, account) {
                Some((user, host, port)) => {
                    println!("best route to {account}: {user}@{host}:{port}");
                    println!("  ssh -p {port} {user}@{host}");
                    0
                }
                None => err("could not resolve a reachable endpoint for that account"),
            }
        }
        "deps" => {
            let pe = cockpit::ops::tool_path_env();
            // ensure a cross target if asked
            if let Some(t) = opt(rest, "--target") {
                match cockpit::sysdeps::ensure_rust_target(t, &pe) {
                    Ok(true) => println!("✓ installed rust target {t}"),
                    Ok(false) => println!("✓ rust target {t} already present"),
                    Err(e) => return err(&e),
                }
            }
            if flag(rest, "--build-essentials") || pos.is_empty() && opt(rest, "--target").is_none() {
                let extra: Vec<String> = pos.iter().map(|s| s.to_string()).collect();
                match cockpit::sysdeps::ensure_build_essentials(&extra) {
                    Ok(msg) => { println!("✓ {msg}"); 0 }
                    Err(e) => err(&e),
                }
            } else {
                0
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

#[cfg(test)]
mod fp5_cli_tests {
    use super::*;

    #[test]
    fn a_part_is_picked_by_a_substring_of_its_name() {
        let root = cockpit::cosmic_setup::Root::NoPrompt;
        let all = cockpit::fp5_setup::all_parts(root);
        let n = all.len();
        assert!(n >= 6, "the scan found the parts: {n}");
        assert_eq!(fp5_select(cockpit::fp5_setup::all_parts(root), None).len(), n);
        let w = fp5_select(cockpit::fp5_setup::all_parts(root), Some("WATCHDOG"));
        assert_eq!(w.len(), 1, "case-insensitive, one match");
        assert!(w[0].0.contains("watchdog"));
        assert!(fp5_select(cockpit::fp5_setup::all_parts(root), Some("no such part")).is_empty());
    }
}
