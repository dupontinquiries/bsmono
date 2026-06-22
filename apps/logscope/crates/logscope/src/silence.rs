/// silence.rs — global output suppression
///
/// Logscope is a log inspector. It must never produce its own logs,
/// diagnostic output, or panic backtraces to stdout/stderr. This module
/// installs two layers of silence:
///
///   1. A no-op tracing subscriber so any `tracing::info!` etc. in
///      dependencies becomes a cheap branch-not-taken with zero output.
///
///   2. On Unix: dup2(/dev/null) over both stdout and stderr so even
///      C libraries, GTK warnings, and glib messages go nowhere.
///      (GTK is very chatty about deprecation warnings otherwise.)
///
/// This runs before any other initialisation. It is unconditional.
/// There is no flag to re-enable output. If you need to debug, use a
/// debugger or compile with `LOGSCOPE_DEBUG=1` which skips step 2 only.

use tracing_subscriber::prelude::*;

pub fn install() {
    // Layer 1: no-op tracing subscriber
    // Uses the minimal "fmt" subscriber with a filter that rejects everything.
    // This satisfies any `tracing` calls in deps without allocating or writing.
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::sink)
                .with_filter(tracing_subscriber::filter::LevelFilter::OFF),
        );
    // Ignore the error if a subscriber is already set (e.g. in tests).
    let _ = tracing::subscriber::set_global_default(subscriber);

    // Layer 2: redirect stdout + stderr to /dev/null at the fd level.
    // Skip if LOGSCOPE_DEBUG is set (dev convenience only).
    #[cfg(unix)]
    if std::env::var_os("LOGSCOPE_DEBUG").is_none() {
        redirect_stdio_to_null();
    }
}

#[cfg(unix)]
fn redirect_stdio_to_null() {
    use std::fs::OpenOptions;
    use std::os::unix::io::IntoRawFd;

    let devnull = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .expect("cannot open /dev/null");
    let null_fd = devnull.into_raw_fd();

    unsafe {
        // dup2(null_fd, STDOUT_FILENO)
        libc_dup2(null_fd, 1);
        // dup2(null_fd, STDERR_FILENO)
        libc_dup2(null_fd, 2);
        // Close the original null_fd (now duplicated)
        libc_close(null_fd);
    }
}

#[cfg(unix)]
unsafe fn libc_dup2(oldfd: i32, newfd: i32) {
    // We call the syscall directly via nix to avoid pulling in libc crate
    // separately. nix re-exports what we need.
    let _ = nix::unistd::dup2(oldfd, newfd);
}

#[cfg(unix)]
unsafe fn libc_close(fd: i32) {
    let _ = nix::unistd::close(fd);
}

#[cfg(not(unix))]
fn redirect_stdio_to_null() {
    // On non-Unix (future Windows port), only the tracing layer applies.
}
