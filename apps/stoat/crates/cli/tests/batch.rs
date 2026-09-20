//! `--batch` is what a script or a harness calls, so the one thing it must never
//! do is wait forever. It did: a load that failed (no such pid, a file that is
//! not there) left nothing running and nothing stopped, and the loop waited for a
//! stop that could not come - found when it hung a 300 s harness call.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run the built `stoat` with a deadline. None = it had to be killed.
fn run(args: &[&str], secs: u64) -> Option<(i32, String)> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_stoat")).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            let mut out = String::new();
            use std::io::Read;
            child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
            return Some((status.code().unwrap_or(-1), out));
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn have_gdb() -> bool {
    Command::new("gdb").arg("--version").output().is_ok_and(|o| o.status.success())
}

#[test]
fn a_core_that_is_not_there_is_refused_before_gdb_is_even_started() {
    let me = std::env::current_exe().unwrap();
    let (code, out) = run(&["core", "--batch", me.to_str().unwrap(), "/nonexistent/core"], 20).expect("it must not hang");
    assert_eq!(code, 1);
    assert!(out.contains("no such file: /nonexistent/core"), "{out}");
}

#[test]
fn attaching_to_a_pid_that_does_not_exist_reports_and_exits() {
    if !have_gdb() {
        eprintln!("NOT RUN: attaching_to_a_pid_that_does_not_exist_reports_and_exits needs gdb");
        return;
    }
    // Above any default pid_max, so it cannot be somebody's process.
    let (code, out) = run(&["attach", "--batch", "4194999"], 30).expect("a failed attach under --batch must exit, not wait for a stop");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error["), "the failure is reported as a diagnostic: {out}");
}

#[test]
fn usage_errors_exit_2_without_touching_gdb() {
    for args in [&["attach"][..], &["attach", "notapid"], &["core", "only-one-arg"], &["nonsense"]] {
        let (code, _) = run(args, 10).expect("usage errors return at once");
        assert_eq!(code, 2, "{args:?}");
    }
}
