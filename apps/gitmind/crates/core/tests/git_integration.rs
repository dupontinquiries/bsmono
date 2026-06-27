// gitmind-core/tests/git_integration.rs
//
// "Big tests on real git repos." These spin up actual git repositories
// in temp dirs and exercise the backend, the checkpoint engine, and the
// chunk store against them — no mocks. They assert the properties that
// matter for safety: snapshots never disturb the user's index/HEAD, the
// timeline reconstructs, side worktrees materialize, and dedup holds
// across near-identical versions of a large file.

use gitmind_core::backend::{GitCli, RepoBackend};
use gitmind_core::checkpoint::CheckpointEngine;
use gitmind_core::store::ChunkStore;
use std::io::Cursor;
use std::path::Path;
use std::process::Command;

/// Minimal real-git-repo fixture in a temp dir.
struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        run(p, &["init", "-q"]);
        run(p, &["config", "user.email", "t@example.com"]);
        run(p, &["config", "user.name", "Test"]);
        // Pin the branch name so assertions don't depend on git's default.
        run(p, &["checkout", "-q", "-b", "main"]);
        Repo { dir }
    }
    fn path(&self) -> &Path {
        self.dir.path()
    }
    fn write(&self, name: &str, contents: &str) {
        std::fs::write(self.path().join(name), contents).unwrap();
    }
    fn commit_all(&self, msg: &str) {
        run(self.path(), &["add", "-A"]);
        run(self.path(), &["commit", "-q", "-m", msg]);
    }
    fn porcelain(&self) -> String {
        let out = Command::new("git").arg("-C").arg(self.path()).args(["status", "--porcelain"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).to_string()
    }
    fn head(&self) -> String {
        let out = Command::new("git").arg("-C").arg(self.path()).args(["rev-parse", "HEAD"]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }
    fn git(&self) -> GitCli {
        GitCli::discover(self.path()).expect("discover repo")
    }
}

fn run(dir: &Path, args: &[&str]) {
    let st = Command::new("git").arg("-C").arg(dir).args(args).status().unwrap();
    assert!(st.success(), "git {:?} failed", args);
}

/// Commit a single file authored by a specific person (for query tests).
fn commit_as(r: &Repo, author: &str, file: &str, contents: &str, msg: &str) {
    r.write(file, contents);
    run(r.path(), &["add", "-A"]);
    run(r.path(), &["commit", "-q", &format!("--author={author}"), "-m", msg]);
}

#[test]
fn status_reflects_working_tree() {
    let r = Repo::new();
    r.write("a.txt", "hello\n");
    r.commit_all("init");
    r.write("a.txt", "hello\nworld\n"); // modified
    r.write("b.txt", "new\n"); // untracked

    let st = r.git().status().unwrap();
    assert!(st.in_repo);
    assert_eq!(st.branch, "main");
    assert!(!st.detached);
    assert!(st.dirty());

    let a = st.files.iter().find(|f| f.path == "a.txt").unwrap();
    let b = st.files.iter().find(|f| f.path == "b.txt").unwrap();
    assert!(!a.untracked, "a.txt is tracked");
    assert!(b.untracked, "b.txt is untracked");
}

#[test]
fn snapshot_preserves_index_and_head() {
    let r = Repo::new();
    r.write("a.txt", "one\n");
    r.commit_all("init");
    // Create a mixed dirty state: staged change + unstaged + untracked.
    r.write("a.txt", "one\ntwo\n");
    run(r.path(), &["add", "a.txt"]);
    r.write("a.txt", "one\ntwo\nthree\n");
    r.write("c.txt", "untracked\n");

    let porcelain_before = r.porcelain();
    let head_before = r.head();

    let g = r.git();
    let eng = CheckpointEngine::new(&g, "main");
    let ck = eng.snapshot("snap").unwrap();
    assert!(ck.is_some(), "first snapshot should record something");

    // The user's working state must be byte-for-byte unchanged.
    assert_eq!(r.porcelain(), porcelain_before, "index/working tree changed!");
    assert_eq!(r.head(), head_before, "HEAD moved!");
}

#[test]
fn snapshot_skips_when_nothing_changed() {
    let r = Repo::new();
    r.write("a.txt", "x\n");
    r.commit_all("init");
    r.write("a.txt", "x\ny\n");

    let g = r.git();
    let eng = CheckpointEngine::new(&g, "main");
    assert!(eng.snapshot("first").unwrap().is_some());
    // No edits between snapshots → identical tree → skipped.
    assert!(eng.snapshot("second").unwrap().is_none());
}

#[test]
fn timeline_lists_recent_first() {
    let r = Repo::new();
    r.write("f", "0\n");
    r.commit_all("init");
    let g = r.git();
    let eng = CheckpointEngine::new(&g, "main");
    r.write("f", "1\n");
    eng.snapshot("alpha").unwrap();
    r.write("f", "2\n");
    eng.snapshot("beta").unwrap();

    let tl = eng.list(10).unwrap();
    assert_eq!(tl.len(), 2);
    assert_eq!(tl[0].label, "beta"); // most recent first
    assert_eq!(tl[1].label, "alpha");
}

#[test]
fn materialize_creates_side_worktree() {
    let r = Repo::new();
    r.write("keep.txt", "v1\n");
    r.commit_all("init");
    let g = r.git();
    let eng = CheckpointEngine::new(&g, "main");
    r.write("keep.txt", "v2\n");
    r.write("extra.txt", "side\n");
    let ck = eng.snapshot("snap").unwrap().unwrap();

    let dest = r.path().join("..").join(format!("wt-{}", ck.oid));
    let dest_s = dest.to_string_lossy().to_string();
    eng.materialize(&ck.oid, &dest_s).unwrap();

    // The snapshot captured both files; they exist in the side worktree.
    assert!(dest.join("keep.txt").exists());
    assert!(dest.join("extra.txt").exists());
    assert_eq!(std::fs::read_to_string(dest.join("keep.txt")).unwrap(), "v2\n");

    // Clean up the worktree registration so the temp dir drops cleanly.
    run(r.path(), &["worktree", "remove", "--force", &dest_s]);
}

#[test]
fn graft_inserts_source_under_prefix_on_new_branch() {
    use gitmind_core::compose;

    // Source repo with a library file.
    let src = Repo::new();
    src.write("lib.rs", "pub fn experiment() -> u32 { 42 }\n");
    src.commit_all("source lib");

    // Target repo on its own branch with its own file + a dirty edit we
    // must NOT lose.
    let tgt = Repo::new();
    tgt.write("main.rs", "fn main() {}\n");
    tgt.commit_all("target init");
    tgt.write("main.rs", "fn main() { /* uncommitted work */ }\n");
    let porcelain_before = tgt.porcelain();
    let head_before = tgt.head();

    let g = tgt.git();
    let commit = compose::graft(&g, src.path(), "HEAD", "vendor/explib", "experiment/explib")
        .expect("graft");
    assert!(!commit.is_empty());

    // The user's current branch + working tree are untouched.
    assert_eq!(tgt.porcelain(), porcelain_before, "working tree changed");
    assert_eq!(tgt.head(), head_before, "HEAD moved");

    // The new branch exists and contains the grafted file under the prefix.
    let listed = g
        .git(&["ls-tree", "-r", "--name-only", "experiment/explib"])
        .unwrap();
    assert!(
        listed.lines().any(|l| l == "vendor/explib/lib.rs"),
        "grafted file missing; tree was:\n{listed}"
    );
}

#[test]
fn query_by_author_and_language() {
    use gitmind_core::query;
    let r = Repo::new();
    // Three commits by two authors across two languages.
    commit_as(&r, "Alice <a@x>", "a.rs", "fn a() {}\n", "alice rust");
    commit_as(&r, "Bob <b@x>", "b.py", "def b(): pass\n", "bob python");
    commit_as(&r, "Alice <a@x>", "c.rs", "fn c() {}\n", "alice rust 2");

    let g = r.git();
    let by_alice = query::search(&g, Some("Alice"), None, 50).unwrap();
    assert_eq!(by_alice.len(), 2, "Alice authored two commits");

    let rust = query::search(&g, None, Some("rust"), 50).unwrap();
    assert_eq!(rust.len(), 2, "two commits touched .rs files");

    let alice_rust = query::search(&g, Some("Alice"), Some("rust"), 50).unwrap();
    assert_eq!(alice_rust.len(), 2);

    let bob_python = query::search(&g, Some("Bob"), Some("python"), 50).unwrap();
    assert_eq!(bob_python.len(), 1);
}

#[test]
fn review_summarizes_since_last_and_runs_hooks() {
    use gitmind_core::review::ReviewEngine;
    use std::os::unix::fs::PermissionsExt;

    let r = Repo::new();
    r.write("file1.rs", "fn one() {}\n");
    r.commit_all("init rust");

    let g = r.git();
    let eng = ReviewEngine::new(&g);

    // First review: baseline is the empty tree, so file1.rs counts.
    let (s1, _) = eng.mark("r1", None).unwrap();
    assert!(s1.files_changed.iter().any(|f| f == "file1.rs"));
    assert!(s1.languages.iter().any(|(l, _)| l == "rs"));

    // Do more work, then summarize since the last review only.
    r.write("file2.py", "def two():\n    pass\n");
    r.commit_all("add python");
    let s2 = eng.summarize_since(None).unwrap();
    assert_eq!(s2.commits, 1, "one commit since r1");
    assert_eq!(s2.files_changed, vec!["file2.py".to_string()]);
    assert!(s2.languages.iter().any(|(l, _)| l == "py"));

    // A review hook fires and sees the summary via env.
    let hooks = r.path().join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let script = hooks.join("on_review.sh");
    std::fs::write(&script, "#!/bin/sh\necho \"$GITMIND_COMMITS\" > hook_ran.txt\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (_s3, results) = eng.mark("r2", Some(&hooks)).unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].ok, "hook should succeed");
    assert!(r.path().join("hook_ran.txt").exists(), "hook side effect missing");
}

#[test]
fn maintenance_writes_commit_graph_and_packs_refs() {
    use gitmind_core::checkpoint::CheckpointEngine;
    use gitmind_core::maintenance::Maintenance;

    let r = Repo::new();
    r.write("a", "1\n");
    r.commit_all("c1");
    r.write("a", "2\n");
    r.commit_all("c2");

    // Create several checkpoint refs (the thing that makes packed-refs matter).
    let g = r.git();
    let eng = CheckpointEngine::new(&g, "main");
    r.write("a", "3\n");
    eng.snapshot("s1").unwrap();
    r.write("a", "4\n");
    eng.snapshot("s2").unwrap();

    let m = Maintenance::new(&g);
    let report = m.run_all().unwrap();
    assert!(report.commit_graph, "commit-graph should be written");
    assert!(report.refs_packed, "refs should be packed");

    // Verify on disk: commit-graph file exists and packed-refs holds a checkpoint.
    assert!(r.path().join(".git/objects/info/commit-graph").exists());
    let packed = std::fs::read_to_string(r.path().join(".git/packed-refs")).unwrap();
    assert!(packed.contains("refs/checkpoints/main"), "checkpoint ref not packed:\n{packed}");
}

#[test]
fn patch_series_roundtrips_format_then_apply() {
    use gitmind_core::patch;

    let r = Repo::new();
    r.write("base.txt", "base\n");
    r.commit_all("base");
    let base = r.head();
    r.write("f1.txt", "one\n");
    r.commit_all("add one");
    r.write("f2.txt", "two\n");
    r.commit_all("add two");
    let original_tree = r.git().git(&["rev-parse", "HEAD^{tree}"]).unwrap();

    // Produce a series for "the list".
    let outdir = r.path().join("patches");
    let g = r.git();
    let files = patch::format_series(&g, &format!("{base}..HEAD"), &outdir).unwrap();
    assert_eq!(files.len(), 2, "two commits → two patches");

    // Parse the mbox-style patch files like a UI previewing the series.
    let combined: String = files
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let metas = patch::parse_mbox(&combined);
    assert!(metas.iter().any(|m| m.subject.contains("add one")));

    // Rewind to base and re-apply the series; the tree must come back identical.
    run(r.path(), &["reset", "--hard", &base]);
    let new_head = patch::apply_series(&g, &files, true).unwrap();
    assert!(!new_head.is_empty());
    let new_tree = g.git(&["rev-parse", "HEAD^{tree}"]).unwrap();
    assert_eq!(new_tree, original_tree, "applied series should reproduce the tree");
}

#[test]
fn portage_imports_git_blobs_and_exports_bundle() {
    use gitmind_core::portage;
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write("a.txt", "alpha contents\n");
    r.write("b.txt", "beta contents\n");
    r.commit_all("two files");
    r.write("a.txt", "alpha contents v2\n");
    r.commit_all("edit a");

    let g = r.git();
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("store.redb")).unwrap();

    // Import every reachable blob into the native store.
    let rep = portage::import_git_objects(&g, &store).unwrap();
    assert!(rep.blobs >= 3, "expected >=3 blobs, got {}", rep.blobs);
    assert!(rep.new_chunks >= 1);

    // Re-import is fully deduped.
    let rep2 = portage::import_git_objects(&g, &store).unwrap();
    assert_eq!(rep2.new_chunks, 0, "second import should store nothing new");

    // Export a bundle and let git verify it.
    let bundle = dir.path().join("export.bundle");
    portage::export_bundle(&g, &bundle).unwrap();
    assert!(bundle.exists());
    assert!(portage::verify_bundle(&g, &bundle), "git should accept the bundle");
}

#[test]
fn store_dedup_across_versions_of_a_large_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("store.redb")).unwrap();

    // 8 MiB base with structure so chunk boundaries are stable.
    let base: Vec<u8> = (0..8 * 1024 * 1024u32).map(|i| (i.wrapping_mul(2654435761) >> 16) as u8).collect();
    let v1 = store.ingest("asset.bin", Cursor::new(base.clone())).unwrap();

    // Version 2: flip a few KB in the middle (a small "edit"/re-export).
    let mut v2bytes = base.clone();
    let mid = v2bytes.len() / 2;
    for b in &mut v2bytes[mid..mid + 4096] {
        *b ^= 0xff;
    }
    let v2 = store.ingest("asset.bin", Cursor::new(v2bytes)).unwrap();

    // The second version must re-store only a tiny fraction of chunks —
    // this is the property that makes large-file versioning cheap.
    assert!(
        v2.new_chunks < v1.chunk_count / 4,
        "expected heavy dedup: v1={} chunks, v2 re-stored {}",
        v1.chunk_count,
        v2.new_chunks
    );
    assert!(v2.new_chunks >= 1, "the changed region should re-store at least one chunk");
}

#[test]
fn watcher_snapshots_on_real_write() {
    use gitmind_core::watch::WatchPolicy;
    use gitmind_core::watcher;
    use std::sync::mpsc;
    use std::time::Duration;

    let r = Repo::new();
    r.write("a.rs", "fn main() {}\n");
    r.commit_all("init");
    let root = r.path().to_path_buf();

    let (stop_tx, stop_rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let g = GitCli::discover(&root).unwrap();
        let mut policy = WatchPolicy::default();
        policy.debounce_ms = 250; // speed up the test
        watcher::run(&g, &policy, "main", stop_rx, |_c| {}).unwrap()
    });

    std::thread::sleep(Duration::from_millis(500)); // let the watcher arm
    std::fs::write(r.path().join("a.rs"), "fn main() { /* edit */ }\n").unwrap();
    std::thread::sleep(Duration::from_millis(1400)); // > debounce + settle
    stop_tx.send(()).unwrap();
    let stats = handle.join().unwrap();

    assert!(stats.events >= 1, "watcher saw no relevant events (inotify?)");
    assert!(stats.snapshots >= 1, "watcher took no snapshot");
    let g = r.git();
    let eng = gitmind_core::checkpoint::CheckpointEngine::new(&g, "main");
    assert!(!eng.list(10).unwrap().is_empty(), "no checkpoint recorded");
}

#[test]
fn checkpoint_diff_and_extract_round_trip() {
    use gitmind_core::checkpoint::CheckpointEngine;
    let r = Repo::new();
    r.write("a.rs", "fn a() {}\n");
    r.write("keep.txt", "same\n");
    r.commit_all("init");
    let g = r.git();
    let eng = CheckpointEngine::new(&g, "main");

    // Snapshot v1, then change the worktree.
    r.write("a.rs", "fn a() { /*v1*/ }\n");
    let c1 = eng.snapshot("v1").unwrap().unwrap();
    r.write("a.rs", "fn a() { /*v2*/ }\nfn b() {}\n");

    // diff(checkpoint, worktree) shows a.rs changed.
    let d = eng.diff(&c1.oid, None).unwrap();
    assert!(d.iter().any(|f| f.path == "a.rs"), "diff vs worktree should list a.rs");

    // diff(c1, c2) shows the line delta between two snapshots.
    let c2 = eng.snapshot("v2").unwrap().unwrap();
    let d2 = eng.diff(&c1.oid, Some(&c2.oid)).unwrap();
    assert!(d2.iter().any(|f| f.path == "a.rs" && f.added >= 1), "c1..c2 should show additions to a.rs");

    // extract c1 into a plain dir → recovers the v1 content (time-machine restore).
    let out = r.path().join("restore_c1");
    let n = eng.extract(&c1.oid, &out).unwrap();
    assert!(n >= 2, "expected >=2 files extracted, got {n}");
    let restored = std::fs::read_to_string(out.join("a.rs")).unwrap();
    assert!(restored.contains("v1"), "extracted snapshot should be the v1 content, got: {restored}");
}

#[test]
fn native_graph_import_walk_and_roundtrip() {
    use gitmind_core::portage;
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write("a.rs", "fn a() {}\n");
    std::fs::create_dir_all(r.path().join("sub")).unwrap();
    r.write("sub/b.rs", "fn b() {}\n");
    r.commit_all("c1");
    r.write("a.rs", "fn a() { 1 }\n");
    r.commit_all("c2");

    let g = r.git();
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("store.redb")).unwrap();

    // Import the ENTIRE object graph.
    let report = portage::import_graph(&g, &store, false).unwrap();
    assert!(report.commits >= 2, "commits: {}", report.commits);
    assert!(report.trees >= 2, "trees: {}", report.trees);
    assert!(report.blobs >= 2, "blobs: {}", report.blobs);
    assert!(report.refs >= 1, "refs: {}", report.refs);

    // Walk the graph from the NATIVE store, no .git involved.
    let head = g.git(&["rev-parse", "HEAD"]).unwrap();
    let meta = portage::native_commit(&store, &head).unwrap();
    assert_eq!(meta.parents.len(), 1, "c2 has one parent");
    assert!(meta.message.contains("c2"));
    let parent = portage::native_commit(&store, &meta.parents[0]).unwrap();
    assert!(parent.parents.is_empty(), "c1 is the root");
    assert!(parent.message.contains("c1"));

    // Reconstruct a git repo from the native store ALONE and prove it's identical.
    let out = dir.path().join("rebuilt");
    let n = portage::export_to_git(&store, &out).unwrap();
    assert_eq!(n, report.objects(), "every object written back");

    let rebuilt = gitmind_core::backend::GitCli::discover(&out).unwrap();
    let rebuilt_head = rebuilt.git(&["rev-parse", "HEAD"]).unwrap();
    assert_eq!(head, rebuilt_head, "HEAD oid must match exactly (bit-exact reconstruction)");
    rebuilt.git(&["fsck", "--full"]).unwrap(); // graph integrity
    let orig_count = g.git(&["rev-list", "--all", "--count"]).unwrap();
    let rebuilt_count = rebuilt.git(&["rev-list", "--all", "--count"]).unwrap();
    assert_eq!(orig_count, rebuilt_count, "same number of commits");
}

#[test]
fn import_graph_reachable_excludes_dangling() {
    use gitmind_core::portage;
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write("a.rs", "fn a() {}\n");
    r.commit_all("c1");

    // Create a DANGLING blob: write an object that no commit references.
    let dangling = {
        use std::process::Command;
        let out = Command::new("git").arg("-C").arg(r.path())
            .args(["hash-object", "-w", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn().unwrap();
        use std::io::Write;
        out.stdin.as_ref().unwrap().write_all(b"orphan content not in any tree\n").unwrap();
        let o = out.wait_with_output().unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    };

    let g = r.git();
    let dir = tempfile::tempdir().unwrap();

    // Reachable import must NOT contain the dangling blob.
    let store_reach = ChunkStore::open(dir.path().join("reach.redb")).unwrap();
    portage::import_graph(&g, &store_reach, true).unwrap();
    assert!(store_reach.get_object(&dangling).unwrap().is_none(),
            "reachable import should exclude the dangling blob");

    // Full import (reachable=false) DOES contain it.
    let store_all = ChunkStore::open(dir.path().join("all.redb")).unwrap();
    portage::import_graph(&g, &store_all, false).unwrap();
    assert!(store_all.get_object(&dangling).unwrap().is_some(),
            "full import should include every object");
}

#[test]
fn native_objects_byte_identical_to_git() {
    use gitmind_core::gitobj::{self, TreeEntry};
    use std::io::Write;
    use std::process::{Command, Stdio};

    let r = Repo::new();
    let root = r.path();

    // --- blob ---
    let content = b"hello gitmind\n";
    let native_blob = gitobj::hash_object("blob", content);
    let git_blob = {
        let mut c = Command::new("git").arg("-C").arg(root)
            .args(["hash-object", "-w", "--stdin"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        c.stdin.take().unwrap().write_all(content).unwrap();
        String::from_utf8_lossy(&c.wait_with_output().unwrap().stdout).trim().to_string()
    };
    assert_eq!(native_blob, git_blob, "blob oid must match git");

    // --- tree ---
    let mut entries = vec![TreeEntry { mode: "100644".into(), name: "a.txt".into(), oid: native_blob.clone() }];
    let (native_tree, _) = gitobj::build_tree(&mut entries);
    let git_tree = {
        let mut c = Command::new("git").arg("-C").arg(root)
            .args(["mktree"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        write!(c.stdin.take().unwrap(), "100644 blob {native_blob}\ta.txt\n").unwrap();
        String::from_utf8_lossy(&c.wait_with_output().unwrap().stdout).trim().to_string()
    };
    assert_eq!(native_tree, git_tree, "tree oid must match git");

    // --- commit (fixed identity + date so it's reproducible) ---
    let line = gitobj::ident_line("Ada", "ada@x", 1_700_000_000, "+0000");
    let (native_commit, _) = gitobj::build_commit(&native_tree, &[], &line, &line, "msg");
    let git_commit = {
        let out = Command::new("git").arg("-C").arg(root)
            .args(["commit-tree", &native_tree, "-m", "msg"])
            .env("GIT_AUTHOR_NAME", "Ada").env("GIT_AUTHOR_EMAIL", "ada@x")
            .env("GIT_AUTHOR_DATE", "1700000000 +0000")
            .env("GIT_COMMITTER_NAME", "Ada").env("GIT_COMMITTER_EMAIL", "ada@x")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
            .output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(native_commit, git_commit, "commit oid must match git byte-for-byte");
}

#[test]
fn native_commit_dir_roundtrips_to_real_git() {
    use gitmind_core::{native, portage};
    use gitmind_core::store::ChunkStore;

    // Build a worktree WITHOUT committing it via git.
    let r = Repo::new();
    r.write("app.rs", "fn main() { go(); }\n");
    std::fs::create_dir_all(r.path().join("src")).unwrap();
    r.write("src/lib.rs", "pub fn go() {}\n");

    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("store.redb")).unwrap();

    // Commit the worktree NATIVELY (no git), advancing refs/heads/main.
    let nc = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "native init").unwrap();

    // Reconstruct a real git repo from .gitmind alone and verify it.
    let out = dir.path().join("rebuilt");
    portage::export_to_git(&store, &out).unwrap();
    let g = gitmind_core::backend::GitCli::discover(&out).unwrap();

    // HEAD is the native commit, fsck is clean, and the content matches.
    assert_eq!(g.git(&["rev-parse", "HEAD"]).unwrap(), nc.oid, "git HEAD == native commit oid");
    g.git(&["fsck", "--full"]).unwrap();
    let files = g.git(&["ls-tree", "-r", "--name-only", "HEAD"]).unwrap();
    assert!(files.contains("app.rs") && files.contains("src/lib.rs"), "files: {files}");
    let blob = g.git(&["cat-file", "blob", "HEAD:src/lib.rs"]).unwrap();
    assert_eq!(blob, "pub fn go() {}", "content recovered through native commit + export");
}

#[test]
fn native_commit_honors_gitignore_matches_git_tree() {
    use gitmind_core::native;
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write(".gitignore", "target/\n*.log\n");
    r.write("app.rs", "fn main() {}\n");
    std::fs::create_dir_all(r.path().join("src")).unwrap();
    r.write("src/lib.rs", "pub fn go() {}\n");
    // Files git would ignore — native must skip these too.
    std::fs::create_dir_all(r.path().join("target")).unwrap();
    r.write("target/junk.o", "binary junk\n");
    r.write("debug.log", "noise\n");

    // Native commit (honors .gitignore via load_all).
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    let nc = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "init").unwrap();

    // git's own tree of the same worktree (git honors .gitignore).
    let g = r.git();
    r.commit_all("init");
    let git_tree = g.git(&["rev-parse", "HEAD^{tree}"]).unwrap();

    // Identical tree oid ⇒ identical file set AND content.
    assert_eq!(nc.tree, git_tree, "native tree must equal git's tree exactly");
}

#[test]
fn native_status_detects_add_modify_delete() {
    use gitmind_core::native::{self, ChangeKind};
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write("a.rs", "fn a() {}\n");
    r.write("b.rs", "fn b() {}\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "init").unwrap();

    // Right after committing, the tree is clean.
    assert!(native::status(&store, r.path()).unwrap().is_empty(), "should be clean after commit");

    // Modify a, delete b, add c.
    r.write("a.rs", "fn a() { changed() }\n");
    std::fs::remove_file(r.path().join("b.rs")).unwrap();
    r.write("c.rs", "fn c() {}\n");

    let ch = native::status(&store, r.path()).unwrap();
    let kind = |p: &str| ch.iter().find(|c| c.path == p).map(|c| c.kind.clone());
    assert_eq!(kind("a.rs"), Some(ChangeKind::Modified));
    assert_eq!(kind("b.rs"), Some(ChangeKind::Deleted));
    assert_eq!(kind("c.rs"), Some(ChangeKind::Added));
    assert_eq!(ch.len(), 3, "exactly three changes");
}

#[test]
fn native_checkout_restores_old_commit() {
    use gitmind_core::native;
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write("a.rs", "v1\n");
    r.write("keep.txt", "k\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    let c1 = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "c1").unwrap();

    // Evolve: change a.rs, remove keep.txt, add b.rs; commit c2.
    r.write("a.rs", "v2\n");
    std::fs::remove_file(r.path().join("keep.txt")).unwrap();
    r.write("b.rs", "bee\n");
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "c2").unwrap();

    // Check out c1 → worktree returns to the c1 state.
    let rep = native::checkout(&store, &c1.oid, r.path()).unwrap();
    assert_eq!(rep.commit, c1.oid);
    assert_eq!(std::fs::read_to_string(r.path().join("a.rs")).unwrap(), "v1\n");
    assert!(r.path().join("keep.txt").exists(), "keep.txt restored");
    assert!(!r.path().join("b.rs").exists(), "b.rs (tracked only in c2) removed");

    // HEAD is now at c1 and the worktree matches it (clean).
    assert!(native::status(&store, r.path()).unwrap().is_empty(), "worktree should match c1");
}

#[test]
fn native_branching_and_cherry_pick() {
    use gitmind_core::native::{self, PickOutcome};
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write("a.txt", "base\n");
    r.write("shared.txt", "one\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "base").unwrap();

    // Branch "feature" off main, switch to it, add a file + commit.
    native::branch_create(&store, "feature", None).unwrap();
    native::checkout(&store, "feature", r.path()).unwrap();
    r.write("feature.txt", "hello from feature\n");
    let feat = native::commit_worktree(&store, r.path(), "feature", "Ada", "ada@x", "add feature.txt").unwrap();

    let branches = native::branch_list(&store).unwrap();
    assert_eq!(branches.len(), 2);
    assert!(branches.iter().any(|b| b.name == "feature" && b.is_head));
    assert!(branches.iter().any(|b| b.name == "main" && !b.is_head));

    // Back to main; advance it so the cherry-pick lands on a new base.
    native::checkout(&store, "main", r.path()).unwrap();
    assert!(!r.path().join("feature.txt").exists(), "feature.txt only on feature branch");
    r.write("a.txt", "base+main\n");
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "main moves on").unwrap();

    let outcome = native::cherry_pick(&store, &feat.oid, "Ada", "ada@x").unwrap();
    match outcome {
        PickOutcome::Committed(c) => {
            native::checkout(&store, "main", r.path()).unwrap();
            assert!(r.path().join("feature.txt").exists(), "cherry-pick brought feature.txt to main");
            assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "base+main\n",
                       "main's own change is preserved by the pick");
            assert_ne!(c.oid, feat.oid, "new commit oid (different parent/base)");
        }
        other => panic!("expected Committed, got {other:?}"),
    }

    // Cherry-picking the same commit again is now Empty (already present).
    assert_eq!(native::cherry_pick(&store, &feat.oid, "Ada", "ada@x").unwrap(), PickOutcome::Empty);

    // branch delete works; can't delete current branch.
    assert!(native::branch_delete(&store, "main").is_err(), "refuse deleting current branch");
    native::branch_delete(&store, "feature").unwrap();
    assert_eq!(native::branch_list(&store).unwrap().len(), 1);
}

#[test]
fn native_cherry_pick_detects_conflict() {
    use gitmind_core::native::{self, PickOutcome};
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write("c.txt", "original\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "base").unwrap();

    // feature edits c.txt one way.
    native::branch_create(&store, "feature", None).unwrap();
    native::checkout(&store, "feature", r.path()).unwrap();
    r.write("c.txt", "feature version\n");
    let feat = native::commit_worktree(&store, r.path(), "feature", "Ada", "ada@x", "edit c").unwrap();

    // main edits the SAME file differently.
    native::checkout(&store, "main", r.path()).unwrap();
    r.write("c.txt", "main version\n");
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "edit c on main").unwrap();

    // Cherry-pick must report a conflict on c.txt, committing nothing.
    match native::cherry_pick(&store, &feat.oid, "Ada", "ada@x").unwrap() {
        PickOutcome::Conflicts(paths) => assert_eq!(paths, vec!["c.txt".to_string()]),
        other => panic!("expected Conflicts, got {other:?}"),
    }
}

#[test]
fn native_rebase_replays_commits() {
    use gitmind_core::native::{self, RebaseOutcome};
    use gitmind_core::store::ChunkStore;

    // main: base → m1.  feature (off base): f1 → f2.  Rebase feature onto main.
    let r = Repo::new();
    r.write("a.txt", "base\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "base").unwrap();

    native::branch_create(&store, "feature", None).unwrap();
    native::checkout(&store, "feature", r.path()).unwrap();
    r.write("f1.txt", "one\n");
    native::commit_worktree(&store, r.path(), "feature", "Ada", "ada@x", "f1").unwrap();
    r.write("f2.txt", "two\n");
    native::commit_worktree(&store, r.path(), "feature", "Ada", "ada@x", "f2").unwrap();

    // Advance main independently.
    native::checkout(&store, "main", r.path()).unwrap();
    r.write("m.txt", "on main\n");
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "m1").unwrap();

    // Rebase feature onto main.
    let outcome = native::rebase(&store, "feature", "main", "Ada", "ada@x").unwrap();
    let new_tip = match outcome {
        RebaseOutcome::Rebased { new_tip, commits } => { assert_eq!(commits, 2, "two commits replayed"); new_tip }
        other => panic!("expected Rebased, got {other:?}"),
    };

    // The rebased feature now contains main's file + both feature files.
    native::checkout(&store, "feature", r.path()).unwrap();
    assert!(r.path().join("m.txt").exists(), "feature now includes main's change");
    assert!(r.path().join("f1.txt").exists() && r.path().join("f2.txt").exists());
    assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "base\n");

    // History: feature tip → f2 → f1 → m1 → base (4 ancestors below the tip+...).
    let mut oids = Vec::new();
    let mut cur = Some(new_tip);
    while let Some(o) = cur {
        let m = gitmind_core::portage::native_commit(&store, &o).unwrap();
        oids.push(m.message.lines().next().unwrap_or("").to_string());
        cur = m.parents.first().cloned();
    }
    assert_eq!(oids, vec!["f2", "f1", "m1", "base"], "linear rebased history");
}

#[test]
fn native_patch_exports_and_git_apply_accepts_it() {
    use gitmind_core::native;
    use gitmind_core::store::ChunkStore;
    use std::io::Write;
    use std::process::{Command, Stdio};

    // c1: a.txt + keep.txt.  c2: modify a.txt, add new.txt, delete keep.txt.
    let r = Repo::new();
    r.write("a.txt", "one\ntwo\nthree\n");
    r.write("keep.txt", "bye\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "c1").unwrap();
    r.write("a.txt", "one\nTWO\nthree\nfour\n");
    std::fs::remove_file(r.path().join("keep.txt")).unwrap();
    r.write("new.txt", "fresh\n");
    let c2 = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "c2").unwrap();

    // Export c2 as a unified diff.
    let patch = native::format_commit_patch(&store, &c2.oid).unwrap();
    assert!(patch.contains("--- a/a.txt") && patch.contains("+++ b/a.txt"));
    assert!(patch.contains("+++ b/new.txt") && patch.contains("--- /dev/null"));
    assert!(patch.contains("+++ /dev/null")); // keep.txt deletion

    // ORACLE: a clean git repo at c1's state must accept the patch via git apply.
    let g = tempfile::tempdir().unwrap();
    Command::new("git").arg("-C").arg(g.path()).args(["init", "-q"]).status().unwrap();
    std::fs::write(g.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(g.path().join("keep.txt"), "bye\n").unwrap();
    // git apply --check (validates the patch applies cleanly)
    let mut child = Command::new("git").arg("-C").arg(g.path())
        .args(["apply", "--check", "-"])
        .stdin(Stdio::piped()).spawn().unwrap();
    child.stdin.take().unwrap().write_all(patch.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success(), "git apply --check must accept our patch");

    // And our OWN applier reproduces c2's content from c1's content.
    let base = tempfile::tempdir().unwrap();
    std::fs::write(base.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(base.path().join("keep.txt"), "bye\n").unwrap();
    let n = native::apply_patch_to_dir(&patch, base.path()).unwrap();
    assert_eq!(n, 3, "three file sections applied");
    assert_eq!(std::fs::read_to_string(base.path().join("a.txt")).unwrap(), "one\nTWO\nthree\nfour\n");
    assert!(!base.path().join("keep.txt").exists(), "keep.txt deleted by patch");
    assert_eq!(std::fs::read_to_string(base.path().join("new.txt")).unwrap(), "fresh\n");
}

#[test]
fn native_applier_consumes_git_generated_patch() {
    use gitmind_core::native;
    use std::process::Command;

    // A git repo; produce a real `git diff` patch, then apply it with OUR applier.
    let g = tempfile::tempdir().unwrap();
    let gp = g.path();
    Command::new("git").arg("-C").arg(gp).args(["init", "-q"]).status().unwrap();
    Command::new("git").arg("-C").arg(gp).args(["config", "user.email", "t@t"]).status().unwrap();
    Command::new("git").arg("-C").arg(gp).args(["config", "user.name", "t"]).status().unwrap();
    std::fs::write(gp.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
    Command::new("git").arg("-C").arg(gp).args(["add", "-A"]).status().unwrap();
    Command::new("git").arg("-C").arg(gp).args(["commit", "-qm", "c1"]).status().unwrap();
    std::fs::write(gp.join("f.txt"), "alpha\nBETA\ngamma\ndelta\n").unwrap();
    let patch = Command::new("git").arg("-C").arg(gp).args(["diff"]).output().unwrap();
    let patch = String::from_utf8(patch.stdout).unwrap();

    // Reset the file to c1 state and apply git's patch with our native applier.
    std::fs::write(gp.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
    let n = native::apply_patch_to_dir(&patch, gp).unwrap();
    assert_eq!(n, 1);
    assert_eq!(std::fs::read_to_string(gp.join("f.txt")).unwrap(), "alpha\nBETA\ngamma\ndelta\n");
}

#[test]
fn native_merge_three_way_and_conflict_and_ff() {
    use gitmind_core::native::{self, MergeOutcome};
    use gitmind_core::store::ChunkStore;

    // ---- non-conflicting 3-way merge ----
    let r = Repo::new();
    r.write("a.txt", "A0\n");
    r.write("shared.txt", "S0\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "base").unwrap();

    native::branch_create(&store, "feature", None).unwrap();
    native::checkout(&store, "feature", r.path()).unwrap();
    r.write("shared.txt", "S0\nfeature-add\n");      // feature changes shared.txt
    native::commit_worktree(&store, r.path(), "feature", "Ada", "ada@x", "feat").unwrap();

    native::checkout(&store, "main", r.path()).unwrap();
    r.write("a.txt", "A0\nmain-add\n");               // main changes a.txt
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "mainwork").unwrap();

    let out = native::merge(&store, "feature", "Ada", "ada@x").unwrap();
    let mc = match out { MergeOutcome::Merged { commit } => commit, o => panic!("expected Merged, got {o:?}") };
    // merge commit has two parents
    let meta = gitmind_core::portage::native_commit(&store, &mc).unwrap();
    assert_eq!(meta.parents.len(), 2, "merge commit has two parents");
    // both sides' changes present
    native::checkout(&store, "main", r.path()).unwrap();
    assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "A0\nmain-add\n");
    assert_eq!(std::fs::read_to_string(r.path().join("shared.txt")).unwrap(), "S0\nfeature-add\n");

    // ---- conflict ----
    native::branch_create(&store, "conf", Some("main")).unwrap();
    native::checkout(&store, "conf", r.path()).unwrap();
    r.write("a.txt", "A0\nconf-version\n");
    native::commit_worktree(&store, r.path(), "conf", "Ada", "ada@x", "conf edit").unwrap();
    native::checkout(&store, "main", r.path()).unwrap();
    r.write("a.txt", "A0\nmain-version\n");
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "main edit").unwrap();
    match native::merge(&store, "conf", "Ada", "ada@x").unwrap() {
        MergeOutcome::Conflicts(p) => assert!(p.contains(&"a.txt".to_string())),
        o => panic!("expected Conflicts, got {o:?}"),
    }

    // ---- fast-forward ----
    let r2 = Repo::new();
    let dir2 = tempfile::tempdir().unwrap();
    let store2 = ChunkStore::open(dir2.path().join("s.redb")).unwrap();
    r2.write("x.txt", "1\n");
    native::commit_worktree(&store2, r2.path(), "main", "Ada", "ada@x", "c1").unwrap();
    native::branch_create(&store2, "ahead", None).unwrap();
    native::checkout(&store2, "ahead", r2.path()).unwrap();
    r2.write("y.txt", "2\n");
    native::commit_worktree(&store2, r2.path(), "ahead", "Ada", "ada@x", "c2").unwrap();
    native::checkout(&store2, "main", r2.path()).unwrap();
    match native::merge(&store2, "ahead", "Ada", "ada@x").unwrap() {
        MergeOutcome::FastForward { .. } => {}
        o => panic!("expected FastForward, got {o:?}"),
    }
}

#[test]
fn native_merge_conflict_markers_then_resolve() {
    use gitmind_core::native::{self, MergeOutcome};
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write("a.txt", "base\n");
    r.write("b.txt", "B\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "base").unwrap();

    native::branch_create(&store, "feature", None).unwrap();
    native::checkout(&store, "feature", r.path()).unwrap();
    r.write("a.txt", "feature side\n");
    let feat = native::commit_worktree(&store, r.path(), "feature", "Ada", "ada@x", "feat").unwrap();

    native::checkout(&store, "main", r.path()).unwrap();
    r.write("a.txt", "main side\n");
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "mainwork").unwrap();

    // Merge with worktree materialization → conflict markers written.
    let out = native::merge_into_worktree(&store, "feature", r.path(), "Ada", "ada@x").unwrap();
    match out {
        MergeOutcome::Conflicts(paths) => assert_eq!(paths, vec!["a.txt".to_string()]),
        o => panic!("expected Conflicts, got {o:?}"),
    }
    let conflicted = std::fs::read_to_string(r.path().join("a.txt")).unwrap();
    assert!(conflicted.contains("<<<<<<< ours") && conflicted.contains("main side")
            && conflicted.contains("=======") && conflicted.contains("feature side")
            && conflicted.contains(">>>>>>> theirs"), "markers: {conflicted}");
    assert_eq!(native::merge_head(&store).unwrap(), Some(feat.oid.clone()), "MERGE_HEAD recorded");

    // Resolve and finish: commit_worktree makes a two-parent merge commit.
    r.write("a.txt", "resolved\n");
    let mc = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "Merge feature").unwrap();
    let meta = gitmind_core::portage::native_commit(&store, &mc.oid).unwrap();
    assert_eq!(meta.parents.len(), 2, "merge commit has two parents");
    assert!(native::merge_head(&store).unwrap().is_none(), "MERGE_HEAD cleared after commit");

    native::checkout(&store, "main", r.path()).unwrap();
    assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "resolved\n");
}

#[test]
fn native_cherry_pick_conflict_markers_then_resolve() {
    use gitmind_core::native::{self, PickOutcome};
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    r.write("a.txt", "base\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "base").unwrap();

    native::branch_create(&store, "feature", None).unwrap();
    native::checkout(&store, "feature", r.path()).unwrap();
    r.write("a.txt", "feature\n");
    let feat = native::commit_worktree(&store, r.path(), "feature", "Ada", "ada@x", "feat").unwrap();

    native::checkout(&store, "main", r.path()).unwrap();
    r.write("a.txt", "main\n");
    native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "mainwork").unwrap();

    // Cherry-pick the feature commit → conflict, markers written.
    match native::cherry_pick_into_worktree(&store, &feat.oid, r.path(), "Ada", "ada@x").unwrap() {
        PickOutcome::Conflicts(p) => assert_eq!(p, vec!["a.txt".to_string()]),
        o => panic!("expected Conflicts, got {o:?}"),
    }
    let txt = std::fs::read_to_string(r.path().join("a.txt")).unwrap();
    assert!(txt.contains("<<<<<<< ours") && txt.contains("main") && txt.contains("feature"));

    // Resolve → ordinary single-parent commit (cherry-pick, not a merge).
    r.write("a.txt", "resolved\n");
    let c = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "feat (resolved)").unwrap();
    let meta = gitmind_core::portage::native_commit(&store, &c.oid).unwrap();
    assert_eq!(meta.parents.len(), 1, "cherry-pick resolve is single-parent");
}

#[test]
fn native_rebase_continue_and_abort() {
    use gitmind_core::native::{self, RebaseOutcome};
    use gitmind_core::store::ChunkStore;

    let setup = || {
        let r = Repo::new();
        r.write("a.txt", "base\n");
        let dir = tempfile::tempdir().unwrap();
        let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
        native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "base").unwrap();
        native::branch_create(&store, "feature", None).unwrap();
        native::checkout(&store, "feature", r.path()).unwrap();
        r.write("a.txt", "feature1\n");
        let f1 = native::commit_worktree(&store, r.path(), "feature", "Ada", "ada@x", "f1").unwrap();
        native::checkout(&store, "main", r.path()).unwrap();
        r.write("a.txt", "main1\n");
        native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "m1").unwrap();
        (r, dir, store, f1)
    };

    // ---- continue ----
    {
        let (r, _dir, store, f1) = setup();
        match native::rebase_into_worktree(&store, "feature", "main", r.path(), "Ada", "ada@x").unwrap() {
            RebaseOutcome::Conflicts { paths, .. } => assert_eq!(paths, vec!["a.txt".to_string()]),
            o => panic!("expected Conflicts, got {o:?}"),
        }
        assert!(native::rebase_status(&store).unwrap().is_some(), "rebase paused");
        // resolve and continue
        r.write("a.txt", "rebased-resolved\n");
        match native::rebase_continue(&store, r.path(), "Ada", "ada@x").unwrap() {
            RebaseOutcome::Rebased { commits, .. } => assert_eq!(commits, 0, "remaining after the conflicted one"),
            o => panic!("expected Rebased, got {o:?}"),
        }
        assert!(native::rebase_status(&store).unwrap().is_none(), "rebase state cleared");
        // feature now sits on top of main with resolved content
        native::checkout(&store, "feature", r.path()).unwrap();
        assert_eq!(std::fs::read_to_string(r.path().join("a.txt")).unwrap(), "rebased-resolved\n");
        let _ = f1;
    }

    // ---- abort ----
    {
        let (r, _dir, store, f1) = setup();
        native::rebase_into_worktree(&store, "feature", "main", r.path(), "Ada", "ada@x").unwrap();
        native::rebase_abort(&store, r.path()).unwrap();
        assert!(native::rebase_status(&store).unwrap().is_none(), "state cleared after abort");
        // feature tip restored to original f1
        let tip = native::branch_list(&store).unwrap().into_iter()
            .find(|b| b.name == "feature").unwrap().oid;
        assert_eq!(tip, f1.oid, "branch restored to original tip");
    }
}

#[test]
fn native_spread_file_and_tree() {
    use gitmind_core::native;
    use gitmind_core::store::ChunkStore;

    let r = Repo::new();
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    r.write("song.txt", "v1 lyrics\n");
    let c1 = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "v1").unwrap();
    r.write("song.txt", "v2 lyrics\n");
    let c2 = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "v2").unwrap();
    r.write("song.txt", "v3 lyrics\n");
    let c3 = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "v3").unwrap();

    let out = tempfile::tempdir().unwrap();
    let revs = vec![
        ("head01".to_string(), c3.oid.clone()),
        ("head02".to_string(), c2.oid.clone()),
        ("head03".to_string(), c1.oid.clone()),
    ];
    let written = native::spread_file(&store, "song.txt", &revs, out.path()).unwrap();
    assert_eq!(written, vec!["head01--song.txt", "head02--song.txt", "head03--song.txt"]);
    assert_eq!(std::fs::read_to_string(out.path().join("head01--song.txt")).unwrap(), "v3 lyrics\n");
    assert_eq!(std::fs::read_to_string(out.path().join("head03--song.txt")).unwrap(), "v1 lyrics\n");

    // tree mode: each rev's whole tree under its own subdir.
    let out2 = tempfile::tempdir().unwrap();
    let dirs = native::spread_tree(&store, &revs, out2.path()).unwrap();
    assert_eq!(dirs.len(), 3);
    assert_eq!(std::fs::read_to_string(out2.path().join("head02").join("song.txt")).unwrap(), "v2 lyrics\n");
}

#[test]
fn store_repack_cross_file_delta_roundtrip() {
    use gitmind_core::store::ChunkStore;
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();

    // 40 near-identical ~8KB "files" (single chunk each: under CDC_MIN=16KB),
    // sharing a 12-byte header and length band so they bucket together.
    let mut items = Vec::new();
    for i in 0..40 {
        let mut body = format!("COMMON_HEADER_BLOCK v{i}\n");
        for line in 0..400 {
            body.push_str(&format!("line {line} value {}\n", (line + i) % 7));
        }
        let rep = store.ingest(&format!("f{i}.txt"), body.as_bytes()).unwrap();
        items.push((rep.blob_oid, body));
    }
    // correct before repack
    for (oid, body) in &items {
        assert_eq!(store.restore_bytes(*oid).unwrap(), body.as_bytes());
    }
    let r = store.repack(0.05).unwrap();
    assert!(r.chunks_deltified > 0, "expected some chunks deltified, got {}", r.chunks_deltified);
    assert!(r.bytes_after < r.bytes_before, "expected savings: {} -> {}", r.bytes_before, r.bytes_after);
    // bit-exact after repack
    for (oid, body) in &items {
        assert_eq!(store.restore_bytes(*oid).unwrap(), body.as_bytes(), "restore mismatch after repack");
    }
    // second repack must not corrupt (bases stay plain; deltas left alone)
    store.repack(0.05).unwrap();
    for (oid, body) in &items {
        assert_eq!(store.restore_bytes(*oid).unwrap(), body.as_bytes(), "restore mismatch after 2nd repack");
    }
}

#[test]
fn varcache_stash_restore_and_cross_project_dedup() {
    use gitmind_core::{store::ChunkStore, varcache};
    let sdir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(sdir.path().join("var.redb")).unwrap();

    // "Project A" build cache.
    let a = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(a.path().join("deps")).unwrap();
    std::fs::write(a.path().join("deps/serde.rlib"), vec![7u8; 50_000]).unwrap();
    std::fs::write(a.path().join("deps/app.rlib"), b"project-A-only").unwrap();
    let tree_a = varcache::stash(&store, a.path()).unwrap();
    let after_a = store.list_blobs().unwrap().len();

    // "Project B" shares the same serde build, different app.
    let b = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(b.path().join("deps")).unwrap();
    std::fs::write(b.path().join("deps/serde.rlib"), vec![7u8; 50_000]).unwrap(); // identical
    std::fs::write(b.path().join("deps/app.rlib"), b"project-B-different").unwrap();
    let tree_b = varcache::stash(&store, b.path()).unwrap();
    let after_b = store.list_blobs().unwrap().len();

    assert_ne!(tree_a, tree_b, "different projects, different trees");
    // B added only its unique app blob + 2 tree objects; the 50KB serde
    // dependency dedup'd (no new big blob).
    assert!(after_b - after_a <= 3, "shared dep should dedup (added {} blobs)", after_b - after_a);

    // Re-stashing content identical to A adds nothing at all (pure dedup).
    let a2 = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(a2.path().join("deps")).unwrap();
    std::fs::write(a2.path().join("deps/serde.rlib"), vec![7u8; 50_000]).unwrap();
    std::fs::write(a2.path().join("deps/app.rlib"), b"project-A-only").unwrap();
    let before = store.list_blobs().unwrap().len();
    let tree_a2 = varcache::stash(&store, a2.path()).unwrap();
    assert_eq!(tree_a2, tree_a, "identical content → identical tree oid");
    assert_eq!(store.list_blobs().unwrap().len(), before, "identical re-stash adds zero blobs");

    // restore B elsewhere, bit-exact
    let out = tempfile::tempdir().unwrap();
    varcache::restore(&store, &tree_b, out.path()).unwrap();
    assert_eq!(std::fs::read(out.path().join("deps/serde.rlib")).unwrap(), vec![7u8; 50_000]);
    assert_eq!(std::fs::read(out.path().join("deps/app.rlib")).unwrap(), b"project-B-different");

    // pin/list/unpin
    varcache::pin(&store, "projB", &tree_b).unwrap();
    assert_eq!(varcache::pins(&store).unwrap(), vec![("projB".to_string(), tree_b.clone())]);
    varcache::unpin(&store, "projB").unwrap();
    assert!(varcache::pins(&store).unwrap().is_empty());
}

#[test]
fn binpin_install_switch_desktop_dedup() {
    use gitmind_core::{binpin, store::ChunkStore};
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("GITMIND_BIN", tmp.path().join("bin"));
    std::env::set_var("GITMIND_APPS", tmp.path().join("apps"));
    std::env::set_var("GITMIND_DESKTOP", tmp.path().join("desktop"));
    let store = ChunkStore::open(tmp.path().join("s.redb")).unwrap();

    let v1 = binpin::install(&store, "myapp", "head01", "commitAAAAAA", "host", b"#!/bin/sh\necho v1\n", false).unwrap();
    binpin::install(&store, "myapp", "head02", "commitBBBBBB", "host", b"#!/bin/sh\necho v2\n", false).unwrap();
    // Identical bytes under a new label dedup to the same content oid.
    let v3 = binpin::install(&store, "myapp", "head03", "commitCCCCCC", "host", b"#!/bin/sh\necho v1\n", false).unwrap();
    assert_eq!(v3.oid, v1.oid, "identical binary should dedup to one stored blob");

    // First install became current; switching relinks atomically.
    assert_eq!(binpin::load_manifest("myapp").current.as_deref(), Some("head01"));
    binpin::use_version("myapp", "head02").unwrap();
    let link = tmp.path().join("bin").join("myapp");
    assert_eq!(std::fs::read_link(&link).unwrap(), std::path::PathBuf::from("myapp@head02"));

    // .desktop generation for GNOME pinning.
    let dp = binpin::write_desktop("myapp", "head02", false, None).unwrap();
    let txt = std::fs::read_to_string(&dp).unwrap();
    assert!(txt.contains("Name=myapp @ head02"), "{txt}");
    assert!(txt.contains("X-GitMind-Commit=commitBBBBBB"));
    assert!(txt.contains("Terminal=false"));

    assert_eq!(binpin::list_apps(), vec!["myapp".to_string()]);
    assert!(binpin::version_path("myapp", "head03").is_some());

    // Removing the current version clears the symlink + manifest current.
    binpin::remove("myapp", "head02").unwrap();
    assert!(binpin::version_path("myapp", "head02").is_none());
    assert!(binpin::load_manifest("myapp").current.is_none());

    for k in ["GITMIND_BIN", "GITMIND_APPS", "GITMIND_DESKTOP"] { std::env::remove_var(k); }
}

#[test]
fn binpin_build_from_commit_stub() {
    use gitmind_core::{binpin, native, store::ChunkStore};
    let r = Repo::new();
    r.write("src.txt", "source\n");
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();
    let c = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "src").unwrap();

    // Stand-in for `cargo build`: a shell command that emits an artifact.
    let work = tempfile::tempdir().unwrap();
    let cmd = vec![
        "sh".to_string(), "-c".to_string(),
        "mkdir -p target/release && printf 'BINARY-BYTES' > target/release/app".to_string(),
    ];
    let bytes = binpin::build_from_commit(&store, &c.oid, work.path(), &cmd, "target/release/app").unwrap();
    assert_eq!(bytes, b"BINARY-BYTES");
    // The commit's source was materialized before building.
    assert_eq!(std::fs::read_to_string(work.path().join("src.txt")).unwrap(), "source\n");
}

#[test]
fn store_repack_then_compact() {
    use gitmind_core::store::ChunkStore;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.redb");
    let mut store = ChunkStore::open(&path).unwrap();
    let mut oids = Vec::new();
    for i in 0..30 {
        let mut body = format!("HDR_BLOCK v{i}\n");
        for l in 0..400 { body.push_str(&format!("line {l} value {}\n", (l + i) % 5)); }
        oids.push((store.ingest(&format!("f{i}"), body.as_bytes()).unwrap().blob_oid, body));
    }
    store.repack(0.05).unwrap();
    store.compact().unwrap(); // must succeed and not corrupt
    for (oid, body) in &oids {
        assert_eq!(store.restore_bytes(*oid).unwrap(), body.as_bytes(), "bit-exact after repack+compact");
    }
}

#[test]
fn checks_run_marker_and_candidates() {
    use gitmind_core::{checks, native, store::ChunkStore};
    let r = Repo::new();
    let dir = tempfile::tempdir().unwrap();
    let store = ChunkStore::open(dir.path().join("s.redb")).unwrap();

    r.write("run.sh", "exit 0\n");
    let a = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "passing").unwrap();
    r.write("run.sh", "exit 1\n");
    let b = native::commit_worktree(&store, r.path(), "main", "Ada", "ada@x", "broken").unwrap();

    let cmd = vec!["sh".to_string(), "run.sh".to_string()];
    let sa = checks::run(&store, &a.oid, &cmd).unwrap();
    let sb = checks::run(&store, &b.oid, &cmd).unwrap();
    assert!(sa.ok, "commit A should pass");
    assert!(!sb.ok && sb.code == 1, "commit B should fail with code 1");

    // cached + markers
    assert!(checks::get(&store, &a.oid).unwrap().unwrap().ok);
    assert_eq!(checks::marker(checks::get(&store, &a.oid).unwrap().as_ref()), "✓");
    assert_eq!(checks::marker(checks::get(&store, &b.oid).unwrap().as_ref()), "✗");
    assert_eq!(checks::marker(None), "·");

    // candidates lists only the green commit
    let cands = checks::candidates(&store).unwrap();
    assert!(cands.iter().any(|c| c.commit == a.oid), "A is a candidate");
    assert!(!cands.iter().any(|c| c.commit == b.oid), "B is not a candidate");
}
