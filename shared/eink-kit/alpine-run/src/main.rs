//! See lib.rs for the plan. This file only carries it out.
use std::ffi::CString;

fn die(what: &str) -> ! {
    eprintln!("alpine-run: {what}: {}", std::io::Error::last_os_error());
    std::process::exit(127);
}

fn c(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| die("path contains NUL"))
}

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    let p = match alpine_run::plan(&argv) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("alpine-run: {e}");
            std::process::exit(2);
        }
    };
    unsafe {
        if libc::unshare(libc::CLONE_NEWNS) != 0 {
            die("unshare(CLONE_NEWNS) (needs root)");
        }
        // Nothing we mount may propagate back to the host.
        if libc::mount(std::ptr::null(), c("/").as_ptr(), std::ptr::null(), libc::MS_REC | libc::MS_PRIVATE, std::ptr::null()) != 0 {
            die("make / private");
        }
        for (src, dst) in &p.binds {
            if !std::path::Path::new(src).exists() {
                continue;
            }
            let _ = std::fs::create_dir_all(dst);
            if libc::mount(c(src).as_ptr(), c(dst).as_ptr(), std::ptr::null(), libc::MS_BIND | libc::MS_REC, std::ptr::null()) != 0 {
                die(&format!("bind {src}"));
            }
        }
        if let Ok(r) = std::fs::read("/etc/resolv.conf") {
            let _ = std::fs::write(format!("{}/etc/resolv.conf", p.root), r);
        }
        if libc::chroot(c(&p.root).as_ptr()) != 0 {
            die("chroot");
        }
        if libc::chdir(c("/").as_ptr()) != 0 {
            die("chdir /");
        }
    }
    let prog = c(&p.program);
    let mut args: Vec<CString> = vec![prog.clone()];
    args.extend(p.args.iter().map(|a| c(a)));
    let mut ptrs: Vec<*const libc::c_char> = args.iter().map(|a| a.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    unsafe { libc::execvp(prog.as_ptr(), ptrs.as_ptr()) };
    die(&format!("exec {}", p.program));
}
