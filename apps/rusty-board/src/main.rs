//! RustyBoard - A modern Rust-based on-screen keyboard
//!
//! Built with GTK4, libadwaita, and whisper-rs for voice input.
//! Designed as a successor to Maliit with a focus on modern UX.

use anyhow::Result;
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use tracing::{info, warn};

mod compositor;
mod config;
mod gestures;
mod ipc;
mod keyboard;
mod layouts;
mod llm;
mod steno;
mod ui;
mod util;
mod voice;

const APP_ID: &str = "io.rustyboard.Keyboard";

/// Print a one-screen help message describing supported flags. Exits
/// after printing.
fn print_help() {
    println!("RustyBoard — modern Rust on-screen keyboard");
    println!();
    println!("Usage: rusty-board [OPTIONS]");
    println!();
    println!("Options:");
    println!("  --debug-layout     Dump per-combo per-row layout geometry");
    println!("                     to stdout and exit. Useful for diagnosing");
    println!("                     overflow at a specific screen width.");
    println!("  -h, --help         Show this help and exit.");
    println!();
    println!("Without flags, rusty-board starts as a Wayland virtual keyboard.");
    println!("KWin launches this binary automatically when configured via");
    println!("`./scripts/plasma-setup.sh`; users typically don't invoke it");
    println!("by hand.");
}

/// Run the `--debug-layout` audit. Prints the geometry table from
/// `ui::keyboard_view::debug_audit_layouts` and exits. No GTK init,
/// no logging, no Wayland connection — purely the math.
///
/// Capture for later inspection with:
///   `rusty-board --debug-layout > /tmp/layout.txt 2>&1`
fn run_debug_layout() -> Result<()> {
    println!("RustyBoard v{} — layout audit", env!("CARGO_PKG_VERSION"));
    println!();
    ui::keyboard_view::debug_audit_layouts();
    Ok(())
}

/// Initialize logging. We always write to BOTH stderr (for normal interactive
/// runs) and a logfile (for when KWin launches us as `--inputmethod`, where
/// stderr ends up nowhere visible). The logfile path is
/// `$XDG_RUNTIME_DIR/rusty-board.log` if set, else `/tmp/rusty-board.log`.
fn init_logging() {
    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,rusty_board=debug"));

    let log_path = std::env::var("XDG_RUNTIME_DIR")
        .map(|d| format!("{}/rusty-board.log", d))
        .unwrap_or_else(|_| "/tmp/rusty-board.log".into());

    // Truncate on each run so the file represents the current session only.
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path);

    let stderr_layer = fmt::layer()
        .with_target(true)
        .with_line_number(true)
        .with_writer(std::io::stderr);

    match file {
        Ok(f) => {
            let file_layer = fmt::layer()
                .with_target(true)
                .with_line_number(true)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(f));
            tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .with(file_layer)
                .init();
            eprintln!("[rusty-board] logging to {}", log_path);
        }
        Err(e) => {
            tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .init();
            eprintln!("[rusty-board] could not open logfile {}: {}", log_path, e);
        }
    }
}

#[cfg(unix)]
unsafe fn libc_getppid() -> i32 {
    extern "C" {
        fn getppid() -> i32;
    }
    getppid()
}

#[cfg(not(unix))]
unsafe fn libc_getppid() -> i32 {
    0
}

/// Ensure `libgtk4-layer-shell.so` is loaded BEFORE libwayland-client so
/// the shim's symbol overrides actually win the dynamic linker's
/// resolution.
///
/// ## Why this is hard
///
/// gtk4-layer-shell works by *shimming* libwayland: it exports symbols
/// (like `wl_proxy_marshal_flags`) that normally come from
/// libwayland-client, intercepts the role-related calls, and forwards
/// everything else. For the shim to win, it must appear before
/// libwayland-client in the linker's resolution order.
///
/// `dlopen("libgtk4-layer-shell.so", RTLD_NOW | RTLD_GLOBAL)` from
/// `main()` does NOT achieve this. By the time main() runs, the dynamic
/// linker has already resolved all symbol references in our binary and
/// its transitive dependencies — they're pointing at libwayland's
/// originals. RTLD_GLOBAL adds the shim to the global namespace for
/// *future* lookups, but symbols already resolved stay resolved. So
/// `is_supported()` correctly returns false even after dlopen.
///
/// The only reliable way is to make the dynamic linker see the shim
/// BEFORE it processes our binary's dependencies. Two ways:
///
///  (a) Link order at build time. Brittle and toolchain-dependent.
///  (b) `LD_PRELOAD` set before execve. Forces the loader to inject the
///      shim into the resolution table FIRST, so when libwayland's
///      symbols come up they bind to the shim's exports.
///
/// We do (b) self-hostedly: if LD_PRELOAD doesn't already include our
/// shim, we set it and re-exec ourselves. The second instance inherits
/// LD_PRELOAD, the loader honors it before anything else loads, and the
/// shim becomes active.
///
/// A wrapper script could do the same thing externally, but doing it
/// in-binary keeps deployment self-contained.
#[cfg(unix)]
fn ensure_layer_shell_active() {
    use std::env;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    // 1. Find a library file that exists on this system. We can't dlopen
    //    or even ldconfig-search without invoking shell; just stat a few
    //    likely paths.
    let candidates = [
        "/usr/lib/libgtk4-layer-shell.so",
        "/usr/lib/libgtk4-layer-shell.so.0",
        "/usr/lib64/libgtk4-layer-shell.so",
        "/usr/lib/x86_64-linux-gnu/libgtk4-layer-shell.so",
        "/usr/lib/aarch64-linux-gnu/libgtk4-layer-shell.so",
    ];
    let lib_path = candidates
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .copied();
    let Some(lib_path) = lib_path else {
        eprintln!(
            "[rusty-board:pre-exec] Could not find libgtk4-layer-shell.so on \
             disk. Layer-shell anchoring will not work. \
             Install with: apk add gtk4-layer-shell"
        );
        return;
    };

    // 2. Check if our shim is already in LD_PRELOAD. If so, we've been
    //    re-exec'd by ourselves; just return.
    let preload = env::var("LD_PRELOAD").unwrap_or_default();
    if preload.contains("libgtk4-layer-shell") {
        eprintln!(
            "[rusty-board:pre-exec] LD_PRELOAD already includes \
             gtk4-layer-shell ({}): proceeding normally.",
            preload
        );
        return;
    }

    // 3. Build a new LD_PRELOAD that prepends our shim.
    let mut new_preload = OsString::from(lib_path);
    if !preload.is_empty() {
        new_preload.push(":");
        new_preload.push(&preload);
    }
    env::set_var("LD_PRELOAD", &new_preload);

    eprintln!(
        "[rusty-board:pre-exec] Setting LD_PRELOAD={:?} and re-execing \
         self so gtk4-layer-shell shim loads before libwayland-client.",
        new_preload
    );

    // Collect argv as null-terminated C strings.
    let argv: Vec<std::ffi::CString> = env::args_os()
        .map(|s| {
            let bytes = s.into_vec();
            std::ffi::CString::new(bytes).unwrap_or_else(|_| {
                std::ffi::CString::new("rusty-board").unwrap()
            })
        })
        .collect();
    let argv_ptrs: Vec<*const std::os::raw::c_char> =
        argv.iter().map(|s| s.as_ptr()).chain(std::iter::once(std::ptr::null())).collect();

    extern "C" {
        fn execv(
            path: *const std::os::raw::c_char,
            argv: *const *const std::os::raw::c_char,
        ) -> std::os::raw::c_int;
    }

    let self_exe = std::ffi::CString::new("/proc/self/exe").unwrap();
    unsafe {
        execv(self_exe.as_ptr(), argv_ptrs.as_ptr());
    }
    // If we got here, execv failed. Continue without layer-shell.
    eprintln!(
        "[rusty-board:pre-exec] execv(/proc/self/exe) failed. \
         Continuing without LD_PRELOAD. Layer-shell anchoring will \
         not work in this run."
    );
}

#[cfg(not(unix))]
fn ensure_layer_shell_active() {
    // No-op on non-unix; layer-shell isn't relevant anyway.
}

fn main() -> Result<()> {
    // Handle diagnostic flags before any GTK init. These early-exit
    // paths don't need a Wayland connection.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--debug-layout") {
        return run_debug_layout();
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }

    // CRITICAL: must happen BEFORE init_logging (which truncates the
    // log file). See ensure_layer_shell_active doc comment for the full
    // story; tl;dr is that it may re-exec ourselves, and we don't want
    // to truncate the log file twice.
    ensure_layer_shell_active();

    init_logging();

    info!("Starting RustyBoard v{}", env!("CARGO_PKG_VERSION"));
    info!(
        "Process info: pid={}, ppid={}, cwd={:?}",
        std::process::id(),
        unsafe { libc_getppid() },
        std::env::current_dir().ok()
    );

    // CRITICAL FOR KWin INPUT-METHOD LAUNCHING
    //
    // When KWin launches us as `--inputmethod`, it provides TWO Wayland
    // sockets to choose from:
    //
    //   * the public socket via `WAYLAND_DISPLAY=wayland-0` — exposes
    //     `wl_compositor`, `zwlr_layer_shell_v1`, `org_kde_plasma_shell`,
    //     and all the rendering / placement protocols
    //   * the privileged socket via `WAYLAND_SOCKET=<fd>` — exposes
    //     `zwp_input_method_v2` and `zwp_virtual_keyboard_v1`, which are
    //     gated to the input-method client and NOT on the public socket
    //
    // GTK/GDK needs the PUBLIC socket for rendering and layer-shell.
    // Our input-method code needs the PRIVILEGED socket. We can't pick
    // just one — we need both.
    //
    // libwayland's behavior: `wl_display_connect(name)` checks
    // `WAYLAND_SOCKET` only when `name == NULL`. GDK passes a non-NULL
    // name (read from `WAYLAND_DISPLAY`), so it naturally connects to
    // the public socket. Good — that's what we want.
    //
    // The catch: `wl_display_connect` *unsets* `WAYLAND_SOCKET` after
    // checking it, even when it didn't use it. So by the time our
    // input-method code runs, the env var would be gone. We capture the
    // fd ourselves here and stash it for `wayland_backend` to open a
    // SECOND Wayland connection with.
    //
    // If `WAYLAND_SOCKET` isn't set (running standalone, not as a
    // KWin/Weston input-method child), we just skip this — input-method
    // will try GDK's shared connection and fail gracefully.
    if let Ok(sock) = std::env::var("WAYLAND_SOCKET") {
        info!(
            "Detected WAYLAND_SOCKET={} (KWin input-method fd). \
             Stashing for input-method backend; GDK will connect to the \
             public socket for rendering/layer-shell.",
            sock
        );
        // Parse the fd and capture it before something else closes/uses it.
        if let Ok(fd) = sock.parse::<i32>() {
            compositor::im_socket::set_inherited_fd(fd);
        } else {
            warn!("WAYLAND_SOCKET={} is not a valid fd number", sock);
        }
        // Remove it from env so GDK / other libs don't accidentally try
        // to use it on their connections. We keep WAYLAND_DISPLAY so GDK
        // connects to the public socket.
        std::env::remove_var("WAYLAND_SOCKET");
    }

    // Load configuration
    let cfg = config::Config::load_or_default()?;
    info!("Configuration loaded from: {:?}", config::Config::config_path());

    // Initialize libadwaita
    adw::init()?;

    // Create the application
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::NON_UNIQUE)
        .build();

    // Wire up the activate signal
    let cfg_clone = cfg.clone();
    app.connect_activate(move |app| {
        if let Err(e) = ui::window::build_window(app, &cfg_clone) {
            warn!("Failed to build window: {}", e);
        }
    });

    // Handle command line args (passes through to GTK). The signal
    // callback expects a `glib::ExitCode` return — SUCCESS means
    // "consumed successfully." Earlier gtk4 versions accepted a bare
    // i32; current versions require ExitCode.
    app.connect_command_line(|app, _cmd_line| {
        app.activate();
        glib::ExitCode::SUCCESS
    });

    // Optionally start the D-Bus service for IPC
    #[cfg(feature = "dbus")]
    {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;

        std::thread::spawn(move || {
            rt.block_on(async {
                if let Err(e) = ipc::dbus::run_service().await {
                    warn!("D-Bus service failed: {}", e);
                }
            });
        });
    }

    // Run the GTK app
    let exit_code = app.run();
    info!("RustyBoard exited with code {:?}", exit_code);

    Ok(())
}
