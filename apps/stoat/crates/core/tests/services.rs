use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use stoat_core::{cache, discover, history, vitals};

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("stoat-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn history_formats() {
    let bash = history::parse_bash("#1700000000\n./target/debug/app --config a.toml\nls\n");
    assert_eq!(bash[0].when, Some(1_700_000_000));
    assert_eq!(bash[1].when, None);

    let zsh = history::parse_zsh(": 1700000001:0;RUST_LOG=debug cargo run --bin app -- --port 80 'two words'\n: 1700000002:3;echo a\\\nb\n");
    assert_eq!(zsh.len(), 2);
    assert_eq!(zsh[1].cmd, "echo a\nb");
    let inv = history::invocation(&zsh[0]).unwrap();
    assert_eq!(inv.program, "cargo:app");
    assert_eq!(inv.args, ["--port", "80", "two words"]);
    assert_eq!(inv.env, [("RUST_LOG".to_string(), "debug".to_string())]);

    let fish = history::parse_fish("- cmd: gdb --args ./build/app -v | tee log\n  when: 1700000003\n");
    let inv = history::invocation(&fish[0]).unwrap();
    assert_eq!((inv.program.as_str(), inv.args.as_slice(), inv.when), ("./build/app", &["-v".to_string()][..], Some(1_700_000_003)));

    let all: Vec<_> = bash.into_iter().chain(zsh).chain(fish).collect();
    assert_eq!(history::grep(&all, "CARGO port").len(), 1);
    let of = history::invocations_of(&all, Path::new("/x/target/debug/app"));
    // Matching is by file name, so ./build/app under gdb counts too.
    let progs: Vec<_> = of.iter().map(|i| i.program.as_str()).collect();
    assert_eq!(progs, ["./build/app", "cargo:app", "./target/debug/app"], "newest first");
    assert!(history::invocation(&history::Entry { shell: history::Shell::Bash, when: None, cmd: "ls -la".into() }).is_none());
}

#[test]
fn zsh_metafied_bytes() {
    // An em dash is E2 80 94. 0x80 and 0x94 fall in zsh's metafied range, so the
    // file holds E2, 83 A0, 83 B4.
    let dir = scratch("zsh");
    let f = dir.join("h");
    std::fs::write(&f, [b": 1:0;./a ".as_slice(), &[0xE2, 0x83, 0xA0, 0x83, 0xB4], b"\n"].concat()).unwrap();
    let e = history::load(history::Shell::Zsh, &f);
    assert_eq!(e[0].cmd, "./a \u{2014}");
}

#[test]
fn inspect_own_test_binary() {
    let me = std::env::current_exe().unwrap();
    let facts = discover::inspect(&me).expect("test binary is a runnable ELF");
    assert!(facts.is_rust);
    assert_eq!(facts.arch, std::env::consts::ARCH);
    assert!(discover::inspect(Path::new("/etc/hostname")).is_none());
    // Deliberately no assertion on this binary's debug info: the repo builds
    // tests under `cargo ailite` (debug = 0, strip = "debuginfo"), so it depends
    // on the profile. The classification is tested on binaries we build below.
}

/// The three classes that change what a debug session can show, on real
/// compiler output. Needs cc and strip; without them it says so and the
/// classification is NOT covered by this run.
#[test]
fn debug_info_classes_on_real_binaries() {
    let tool = |t: &str| std::process::Command::new(t).arg("--version").output().is_ok_and(|o| o.status.success());
    if !tool("cc") || !tool("strip") {
        eprintln!("NOT RUN: debug_info_classes_on_real_binaries needs cc and strip");
        return;
    }
    let d = scratch("dbginfo");
    let src = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/crash.c"));
    let build = |name: &str, flags: &[&str]| {
        let out = d.join(name);
        assert!(std::process::Command::new("cc").args(flags).arg("-o").arg(&out).arg(&src).status().unwrap().success());
        out
    };
    let with = build("with-g", &["-g"]);
    let without = build("without-g", &[]);
    let stripped = build("stripped", &[]);
    assert!(std::process::Command::new("strip").arg("-s").arg(&stripped).status().unwrap().success());
    let class = |p: &Path| discover::inspect(p).unwrap().debug_info;
    assert_eq!(class(&with), discover::DebugInfo::Embedded);
    assert_eq!(class(&without), discover::DebugInfo::SymbolsOnly);
    assert_eq!(class(&stripped), discover::DebugInfo::None);
    assert!(!discover::inspect(&with).unwrap().is_rust);
}

#[test]
fn scan_ranks_with_reasons() {
    let root = scratch("scan").join("my-app");
    let dbg = root.join("target/debug");
    std::fs::create_dir_all(&dbg).unwrap();
    std::fs::create_dir_all(dbg.join("deps")).unwrap();
    let me = std::env::current_exe().unwrap();
    std::fs::copy(&me, dbg.join("my-app")).unwrap();
    std::fs::copy(&me, dbg.join("helper")).unwrap();
    std::fs::copy(&me, dbg.join("deps/my_app-0123abcd")).unwrap();
    let hist = history::parse_bash("./target/debug/helper --once\n./target/debug/my-app serve\n./target/debug/my-app serve --port 1\n");
    // scan_with, not scan: this suite runs with CARGO_TARGET_DIR exported (that
    // is how the repo builds), and the result must not depend on the caller's env.
    let c = discover::scan_with(&root, &hist, &[], None);
    let names: Vec<_> = c.iter().map(|c| c.path.file_name().unwrap().to_str().unwrap()).collect();
    assert_eq!(names, ["my-app", "helper"], "deps/ is skipped, project name wins the tie");

    // A shared target dir is searched as well, and does not displace the local one.
    let shared = root.parent().unwrap().join("shared-target");
    std::fs::create_dir_all(shared.join("debug")).unwrap();
    std::fs::copy(&me, shared.join("debug/elsewhere")).unwrap();
    let c2 = discover::scan_with(&root, &hist, &[], Some(&shared));
    let names2: Vec<_> = c2.iter().map(|c| c.path.file_name().unwrap().to_str().unwrap()).collect();
    assert_eq!(names2, ["my-app", "helper", "elsewhere"]);
    assert_eq!(c[0].invocations[0].args, ["serve", "--port", "1"]);
    assert!(c[0].reasons.iter().any(|r| matches!(r, discover::Reason::InHistory { times: 2 })));
    assert!(c[0].reasons.iter().all(|r| !r.describe().is_empty()));
}

#[test]
fn cache_plan_then_apply() {
    let d = scratch("cache");
    std::fs::create_dir_all(d.join("sub")).unwrap();
    std::fs::write(d.join("sub/old"), vec![1u8; 100_000]).unwrap();
    std::fs::write(d.join("new"), vec![1u8; 50_000]).unwrap();
    let past = std::time::SystemTime::now() - Duration::from_secs(40 * 86_400);
    let f = std::fs::File::options().write(true).open(d.join("sub/old")).unwrap();
    f.set_times(std::fs::FileTimes::new().set_accessed(past).set_modified(past)).unwrap();

    let before = cache::measure(&d);
    assert_eq!(before.files, 2);
    let plan = cache::plan_prune(&d, Duration::from_secs(30 * 86_400));
    assert_eq!(plan.remove.len(), 1);
    assert!(plan.remove[0].0.ends_with("sub/old"));
    assert!(d.join("sub/old").exists(), "planning deletes nothing");
    let (freed, failed) = cache::apply(&plan);
    assert!(failed.is_empty() && freed == plan.bytes);
    assert_eq!(cache::measure(&d).files, 1);
}

#[test]
fn proc_sampling_and_cpu() {
    let root = scratch("proc");
    let p = root.join("42");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("status"), "Name:\tx\nVmRSS:\t  20480 kB\nRssAnon:\t 10000 kB\nVmSwap:\t 0 kB\nThreads:\t3\n").unwrap();
    // comm with a space and a paren, the case naive split_whitespace gets wrong.
    std::fs::write(p.join("stat"), "42 (my (app) x) S 1 42 42 0 -1 4194304 100 0 0 0 250 50 0 0 20 0 3 0 100 0 0\n").unwrap();
    let (s, ticks) = vitals::read_sample(&root, 42, None).unwrap();
    assert_eq!((s.rss_kb, s.anon_kb, s.threads, ticks), (20480, 10000, 3, 300));
    let earlier = Instant::now() - Duration::from_secs(1);
    let (s, _) = vitals::read_sample(&root, 42, Some((250, earlier))).unwrap();
    assert!((s.cpu_pct - 50.0).abs() < 5.0, "50 ticks in ~1 s is ~50%: {}", s.cpu_pct);
    assert!(vitals::read_sample(&root, 43, None).is_none());
}

#[test]
fn frame_stats_and_freeze() {
    let mut v = vitals::Vitals::new(8, 64);
    let t0 = Instant::now();
    let mut t = t0;
    for i in 0..40 {
        t += Duration::from_millis(if i == 30 { 50 } else { 16 });
        v.mark_frame(t);
    }
    let st = vitals::frame_stats(&v.frames()).unwrap();
    assert_eq!(st.count, 39);
    assert!((st.p50_ms - 16.0).abs() < 0.5);
    assert_eq!(st.hitches, 1);
    assert!((st.worst_ms - 50.0).abs() < 0.5);
    assert!((st.last_hitch_before_end.unwrap().as_millis() as i64 - 9 * 16).abs() <= 1);

    // A user-visible stop of 10 s must not become a 10 s frame.
    v.set_frozen(true);
    v.set_frozen(false);
    v.mark_frame(t + Duration::from_secs(10));
    v.mark_frame(t + Duration::from_secs(10) + Duration::from_millis(16));
    assert!(vitals::frame_stats(&v.frames()).unwrap().worst_ms < 60.0);

    // Ring capacity, and frozen rings ignore samples.
    let s = |kb| vitals::Sample { at: Instant::now(), rss_kb: kb, anon_kb: 0, swap_kb: 0, threads: 1, cpu_pct: 0.0 };
    for kb in 0..20 {
        v.push_sample(s(kb));
    }
    assert_eq!(v.samples().len(), 8);
    assert_eq!(v.samples()[0].rss_kb, 12);
    v.set_frozen(true);
    v.push_sample(s(999));
    assert_eq!(v.samples().last().unwrap().rss_kb, 19);
}

#[test]
fn leak_slope() {
    let t0 = Instant::now();
    let samples: Vec<_> = (0..20)
        .map(|i| vitals::Sample { at: t0 + Duration::from_millis(100 * i), rss_kb: 10_000 + 1024 * i, anon_kb: 0, swap_kb: 0, threads: 1, cpu_pct: 0.0 })
        .collect();
    let slope = vitals::rss_slope_kb_per_s(&samples).unwrap();
    assert!((slope - 10_240.0).abs() < 1.0, "{slope}");
}

#[test]
fn attachable_processes_come_from_proc_and_exclude_other_users_and_kernel_threads() {
    use std::os::unix::fs::MetadataExt;
    use stoat_core::procs;
    let root = scratch("procs");
    let mk = |pid: &str, cmdline: &[u8], comm: &str| {
        let d = root.join(pid);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("cmdline"), cmdline).unwrap();
        std::fs::write(d.join("comm"), format!("{comm}\n")).unwrap();
    };
    mk("100", b"/usr/bin/tile-viewer\0--zoom\015\0", "tile-viewer");
    mk("200", b"", "kworker/0:1");
    mk("300", b"./app\0", "app");
    std::fs::create_dir_all(root.join("self")).unwrap();
    let me = std::fs::metadata(&root).unwrap().uid();
    let got = procs::list(&root, me, 300);
    assert_eq!(got.len(), 1, "kernel thread, the skipped pid and non-numeric entries are out: {got:?}");
    assert_eq!((got[0].pid, got[0].name.as_str(), got[0].cmdline.as_str()), (100, "tile-viewer", "/usr/bin/tile-viewer --zoom 15"));
    assert!(procs::list(&root, me + 1, 0).is_empty(), "another user's processes are not offered");

    std::fs::create_dir_all(root.join("sys/kernel/yama")).unwrap();
    std::fs::write(root.join("sys/kernel/yama/ptrace_scope"), "1\n").unwrap();
    assert_eq!(procs::ptrace_scope(&root), Some(1));
    assert!(procs::attach_caveat(Some(1)).unwrap().contains("ptrace_scope=1"));
    assert!(procs::attach_caveat(Some(0)).is_none() && procs::attach_caveat(None).is_none());
}
