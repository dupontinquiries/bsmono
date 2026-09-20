//! Build driving. A build is planned first (so the shell can show and edit the
//! exact command) and run second, streaming lines out through a callback. For
//! cargo, the JSON message stream tells us which executables were produced, which
//! feeds straight back into discovery as the strongest ranking signal.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildSystem {
    Cargo,
    Meson,
    CMake,
    Make,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub system: BuildSystem,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

impl BuildPlan {
    pub fn command_line(&self) -> String {
        std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildLine {
    Out(String),
    Err(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildOutcome {
    pub success: bool,
    pub executables: Vec<PathBuf>,
}

pub fn plan(root: &Path) -> Option<BuildPlan> {
    let has = |f: &str| root.join(f).exists();
    let p = |system, program: &str, args: &[&str]| {
        Some(BuildPlan { system, program: program.into(), args: args.iter().map(|s| s.to_string()).collect(), cwd: root.to_path_buf() })
    };
    if has("Cargo.toml") {
        // json-render-diagnostics: artifacts as JSON on stdout, human-readable
        // diagnostics still on stderr. Plain `json` would make us re-render them.
        return p(BuildSystem::Cargo, "cargo", &["build", "--message-format=json-render-diagnostics"]);
    }
    if has("meson.build") {
        let dir = ["build", "_build", "builddir"].into_iter().find(|d| root.join(d).join("build.ninja").exists())?;
        return p(BuildSystem::Meson, "meson", &["compile", "-C", dir]);
    }
    if has("CMakeLists.txt") {
        let dir = ["build", "cmake-build-debug"].into_iter().find(|d| root.join(d).join("CMakeCache.txt").exists())?;
        return p(BuildSystem::CMake, "cmake", &["--build", dir]);
    }
    if has("Makefile") || has("makefile") || has("GNUmakefile") {
        return p(BuildSystem::Make, "make", &[]);
    }
    None
}

/// Blocks until the build ends. Call from a worker thread; `on_line` is invoked
/// on that thread (stdout) and on one helper thread (stderr).
pub fn run(plan: &BuildPlan, on_line: impl Fn(BuildLine) + Send + Sync) -> std::io::Result<BuildOutcome> {
    let mut child = Command::new(&plan.program)
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let mut executables = Vec::new();
    std::thread::scope(|s| {
        s.spawn(|| {
            for l in BufReader::new(stderr).lines().map_while(Result::ok) {
                on_line(BuildLine::Err(l));
            }
        });
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            match (plan.system, cargo_executable(&l)) {
                (BuildSystem::Cargo, Some(CargoLine::Executable(p))) => executables.push(p),
                (BuildSystem::Cargo, Some(CargoLine::Other)) => {}
                _ => on_line(BuildLine::Out(l)),
            }
        }
    });
    Ok(BuildOutcome { success: child.wait()?.success(), executables })
}

enum CargoLine {
    Executable(PathBuf),
    Other,
}

fn cargo_executable(line: &str) -> Option<CargoLine> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let reason = v.get("reason")?.as_str()?;
    if reason == "compiler-artifact" {
        if let Some(exe) = v.get("executable").and_then(|e| e.as_str()) {
            return Some(CargoLine::Executable(exe.into()));
        }
    }
    Some(CargoLine::Other)
}
