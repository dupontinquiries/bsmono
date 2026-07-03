//! # vvenc_install — build VVenC from source into a local prefix
//!
//! VVenC isn't in the Fedora default repos (or RPM Fusion), and 
//! Fraunhofer's GitHub releases only ship source. This module handles 
//! the install path: clone + cmake + make + install-to-local-prefix.
//!
//! Default install location: `~/.local/share/av1tuner/bin/vvencapp`. 
//! That's user-writable (no sudo), and the app prepends the bin dir to 
//! the PATH it gives the encoder subprocess. So once installed, the 
//! existing encoder-detection logic finds vvencapp transparently.
//!
//! The install is a streaming subprocess sequence. Each step writes to 
//! a log channel the UI displays; cancellation is cooperative via an 
//! AtomicBool that the worker checks between steps.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use crossbeam_channel::Sender;
use thiserror::Error;

/// Events emitted during install. UI maps these to progress display.
#[derive(Clone, Debug)]
pub enum InstallEvent {
    /// New step starting (one of "clone", "cmake", "build", "install").
    StepStarted(String),
    /// A line of subprocess output (stdout or stderr).
    Output(String),
    /// Step finished successfully.
    StepFinished(String),
    /// Install complete; binary is at the given path.
    Done(PathBuf),
    /// Install failed.
    Failed(String),
    /// Install cancelled by user.
    Cancelled,
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("missing build tool: {0} — install it with your package manager")]
    MissingTool(String),
    #[error("subprocess failed: {0}")]
    Subprocess(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled")]
    Cancelled,
}

/// Where the install lives. `bin_dir` is what gets prepended to PATH.
#[derive(Clone, Debug)]
pub struct InstallPaths {
    /// Root for av1tuner-managed files. `~/.local/share/av1tuner` on Linux.
    pub data_dir: PathBuf,
    /// Where source is cloned + built. Big — keep separate so it can 
    /// be cleaned post-install without losing the installed binary.
    pub build_dir: PathBuf,
    /// Where the installed `vvencapp` lives. This is what we add to PATH.
    pub bin_dir: PathBuf,
    /// Full path to the installed binary.
    pub vvencapp_path: PathBuf,
}

impl InstallPaths {
    /// Default install paths for the current platform.
    pub fn default_paths() -> Option<Self> {
        let data_dir = data_dir()?;
        let build_dir = data_dir.join("build").join("vvenc");
        let bin_dir = data_dir.join("bin");
        let vvencapp_path = bin_dir.join("vvencapp");
        Some(Self { data_dir, build_dir, bin_dir, vvencapp_path })
    }
    
    /// True if the binary exists where we expect it.
    pub fn is_installed(&self) -> bool {
        self.vvencapp_path.exists()
    }
}

/// Linux/BSD: $XDG_DATA_HOME or ~/.local/share. macOS: ~/Library/Application Support.
/// Windows: %APPDATA% (same as config dir; we don't separate on Windows).
fn data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Some(PathBuf::from(appdata).join("av1tuner"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return Some(PathBuf::from(home)
                .join("Library").join("Application Support").join("av1tuner"));
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
            if !xdg.is_empty() {
                return Some(PathBuf::from(xdg).join("av1tuner"));
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return Some(PathBuf::from(home).join(".local").join("share").join("av1tuner"));
        }
    }
    None
}

/// Check that the build toolchain is present before starting. Returns 
/// Ok if everything's there, otherwise the first missing tool's name.
pub fn check_build_tools() -> Result<(), String> {
    for tool in &["git", "cmake", "make"] {
        if which::which(tool).is_err() {
            return Err(tool.to_string());
        }
    }
    // C++ compiler. Prefer g++; clang++ is also fine.
    if which::which("g++").is_err() && which::which("clang++").is_err() {
        return Err("g++ or clang++".to_string());
    }
    Ok(())
}

/// Run the install. Blocking call — run from a background thread. 
/// Streams events through the channel. Cancellable via the AtomicBool.
pub fn install(
    paths: &InstallPaths,
    events: Sender<InstallEvent>,
    cancel: Arc<AtomicBool>,
) -> Result<(), InstallError> {
    // 0. Toolchain check.
    if let Err(missing) = check_build_tools() {
        return Err(InstallError::MissingTool(missing));
    }
    
    std::fs::create_dir_all(&paths.data_dir)?;
    std::fs::create_dir_all(&paths.bin_dir)?;
    
    // 1. Clone (or update existing checkout).
    let _ = events.send(InstallEvent::StepStarted("clone".into()));
    if paths.build_dir.exists() {
        // Reuse the checkout — pull latest. Faster than re-cloning.
        let _ = events.send(InstallEvent::Output(
            format!("(reusing existing checkout at {})", paths.build_dir.display())
        ));
        run_streaming(
            "git", &["pull", "--ff-only"],
            Some(&paths.build_dir), &events, &cancel,
        )?;
    } else {
        std::fs::create_dir_all(paths.build_dir.parent().unwrap_or(&paths.data_dir))?;
        run_streaming(
            "git",
            &[
                "clone", "--depth=1",
                "https://github.com/fraunhoferhhi/vvenc",
                &paths.build_dir.to_string_lossy(),
            ],
            None, &events, &cancel,
        )?;
    }
    let _ = events.send(InstallEvent::StepFinished("clone".into()));
    
    check_cancel(&cancel)?;
    
    // 2. CMake configure.
    let _ = events.send(InstallEvent::StepStarted("cmake configure".into()));
    let build_subdir = paths.build_dir.join("build").join("release-static");
    std::fs::create_dir_all(&build_subdir)?;
    run_streaming(
        "cmake",
        &[
            "-S", &paths.build_dir.to_string_lossy(),
            "-B", &build_subdir.to_string_lossy(),
            "-DCMAKE_BUILD_TYPE=Release",
        ],
        None, &events, &cancel,
    )?;
    let _ = events.send(InstallEvent::StepFinished("cmake configure".into()));
    
    check_cancel(&cancel)?;
    
    // 3. Build. -j without a number = "use everything"; safer to cap at 
    // physical cores so we don't tank the rest of the system.
    let _ = events.send(InstallEvent::StepStarted("build".into()));
    let jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .to_string();
    run_streaming(
        "cmake",
        &[
            "--build", &build_subdir.to_string_lossy(),
            "-j", &jobs,
        ],
        None, &events, &cancel,
    )?;
    let _ = events.send(InstallEvent::StepFinished("build".into()));
    
    check_cancel(&cancel)?;
    
    // 4. Copy binary to our bin dir. VVenC's Makefile builds at 
    // `bin/release-static/vvencapp` relative to the source root.
    let _ = events.send(InstallEvent::StepStarted("install".into()));
    let built_binary = paths.build_dir
        .join("bin").join("release-static").join("vvencapp");
    if !built_binary.exists() {
        // Some build configurations put it elsewhere. Search for it.
        let _ = events.send(InstallEvent::Output(
            format!("expected binary at {} not found; searching build dir...",
                built_binary.display())
        ));
        let found = find_binary(&paths.build_dir, "vvencapp")?;
        std::fs::copy(&found, &paths.vvencapp_path)?;
        let _ = events.send(InstallEvent::Output(
            format!("copied from {}", found.display())
        ));
    } else {
        std::fs::copy(&built_binary, &paths.vvencapp_path)?;
    }
    // Ensure executable bit (cargo copy preserves permissions on Unix 
    // but be defensive for cross-FS copies).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&paths.vvencapp_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&paths.vvencapp_path, perms)?;
    }
    let _ = events.send(InstallEvent::StepFinished("install".into()));
    
    let _ = events.send(InstallEvent::Done(paths.vvencapp_path.clone()));
    Ok(())
}

fn check_cancel(cancel: &Arc<AtomicBool>) -> Result<(), InstallError> {
    if cancel.load(Ordering::SeqCst) {
        Err(InstallError::Cancelled)
    } else {
        Ok(())
    }
}

/// Recursive walk to find a binary by name. Used when the expected path 
/// doesn't match for unusual build configurations.
fn find_binary(root: &Path, name: &str) -> std::io::Result<PathBuf> {
    fn walk(dir: &Path, name: &str) -> std::io::Result<Option<PathBuf>> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                if let Some(p) = walk(&path, name)? {
                    return Ok(Some(p));
                }
            } else if ft.is_file() {
                if path.file_name().map(|s| s == name).unwrap_or(false) {
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }
    walk(root, name)?.ok_or_else(|| std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("binary {} not found under {}", name, root.display()),
    ))
}

/// Run a subprocess, streaming stdout + stderr line-by-line through the 
/// events channel. Polls cancel between line reads so cancel takes 
/// effect within ~1 line of output (a few ms typically).
fn run_streaming(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    events: &Sender<InstallEvent>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), InstallError> {
    let _ = events.send(InstallEvent::Output(
        format!("$ {} {}", program, args.join(" "))
    ));
    
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    if let Some(cwd) = cwd { cmd.current_dir(cwd); }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    
    // Drain both streams in background threads, sending lines to the 
    // events channel as they arrive.
    let events_out = events.clone();
    let stdout_handle = std::thread::Builder::new()
        .name("install-stdout".into())
        .spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                let _ = events_out.send(InstallEvent::Output(line));
            }
        })
        .expect("spawn stdout reader");
    
    let events_err = events.clone();
    let stderr_handle = std::thread::Builder::new()
        .name("install-stderr".into())
        .spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                let _ = events_err.send(InstallEvent::Output(line));
            }
        })
        .expect("spawn stderr reader");
    
    // Poll for cancel while waiting. Cancel kills the subprocess.
    loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            // Drain stream threads (they'll exit shortly after kill).
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return Err(InstallError::Cancelled);
        }
        match child.try_wait()? {
            Some(status) => {
                // Wait for stream threads to finish draining.
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                if status.success() {
                    return Ok(());
                } else {
                    return Err(InstallError::Subprocess(format!(
                        "{} exited with status {:?}", program, status.code()
                    )));
                }
            }
            None => std::thread::sleep(std::time::Duration::from_millis(100)),
        }
    }
}

/// Run the install on a background thread. Returns a receiver the UI 
/// polls for events + the cancel handle. Use this rather than `install` 
/// directly from a UI context.
pub fn install_async(
    paths: InstallPaths,
) -> (crossbeam_channel::Receiver<InstallEvent>, Arc<AtomicBool>) {
    let (tx, rx) = crossbeam_channel::unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = cancel.clone();
    let tx_for_thread = tx.clone();
    
    std::thread::Builder::new()
        .name("vvenc-install".into())
        .spawn(move || {
            match install(&paths, tx_for_thread.clone(), cancel_for_thread) {
                Ok(()) => {}  // Done event already sent
                Err(InstallError::Cancelled) => {
                    let _ = tx_for_thread.send(InstallEvent::Cancelled);
                }
                Err(e) => {
                    let _ = tx_for_thread.send(InstallEvent::Failed(e.to_string()));
                }
            }
        })
        .expect("spawn install thread");
    
    (rx, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn default_paths_have_sensible_structure() {
        // Skip if HOME / XDG aren't set (sandboxed test envs).
        let Some(paths) = InstallPaths::default_paths() else { return; };
        // bin_dir is under data_dir, vvencapp_path is under bin_dir.
        assert!(paths.bin_dir.starts_with(&paths.data_dir));
        assert!(paths.vvencapp_path.starts_with(&paths.bin_dir));
        assert_eq!(
            paths.vvencapp_path.file_name().and_then(|s| s.to_str()),
            Some("vvencapp")
        );
    }
    
    #[test]
    fn is_installed_false_when_missing() {
        let paths = InstallPaths {
            data_dir: PathBuf::from("/tmp/av1tuner_test_nonexistent"),
            build_dir: PathBuf::from("/tmp/av1tuner_test_nonexistent/build"),
            bin_dir: PathBuf::from("/tmp/av1tuner_test_nonexistent/bin"),
            vvencapp_path: PathBuf::from("/tmp/av1tuner_test_nonexistent/bin/vvencapp"),
        };
        assert!(!paths.is_installed());
    }
}
