//! "Which binary did you mean?" answered with a ranked list where every point of
//! score has a stated reason, so a wrong guess can be read and overruled rather
//! than argued with.

use crate::history::{self, Entry, Invocation};
use object::{Object, ObjectSection};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugInfo {
    /// `.debug_info` is in the file.
    Embedded,
    /// Stripped, but `.gnu_debuglink` / build-id points at a separate file.
    Separate,
    /// Symbol table only: function names in backtraces, no lines or locals.
    SymbolsOnly,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    DebugInfo(DebugInfo),
    Built { ago: Duration },
    InHistory { times: usize },
    NameMatchesProject,
    /// Reported by the build we just ran; the strongest signal there is.
    BuildArtifact,
    OptimizedProfile,
    Example,
    ForeignArch(String),
}

impl Reason {
    pub fn weight(&self) -> i32 {
        match self {
            Reason::DebugInfo(DebugInfo::Embedded) => 30,
            Reason::DebugInfo(DebugInfo::Separate) => 20,
            Reason::DebugInfo(DebugInfo::SymbolsOnly) => 0,
            Reason::DebugInfo(DebugInfo::None) => -30,
            Reason::Built { ago } => match ago.as_secs() {
                0..=600 => 40,
                601..=86_400 => 25,
                86_401..=604_800 => 10,
                _ => 0,
            },
            Reason::InHistory { times } => 15 + (*times as i32).min(10),
            Reason::NameMatchesProject => 15,
            Reason::BuildArtifact => 60,
            Reason::OptimizedProfile => -5,
            Reason::Example => -10,
            Reason::ForeignArch(_) => -100,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Reason::DebugInfo(DebugInfo::Embedded) => "has debug info".into(),
            Reason::DebugInfo(DebugInfo::Separate) => "debug info in a separate file".into(),
            Reason::DebugInfo(DebugInfo::SymbolsOnly) => "symbols only, no line info".into(),
            Reason::DebugInfo(DebugInfo::None) => "stripped".into(),
            Reason::Built { ago } => format!("built {} ago", human_duration(*ago)),
            Reason::InHistory { times } => format!("run {times}x in shell history"),
            Reason::NameMatchesProject => "named after the project".into(),
            Reason::BuildArtifact => "produced by the last build".into(),
            Reason::OptimizedProfile => "release profile".into(),
            Reason::Example => "example target".into(),
            Reason::ForeignArch(a) => format!("built for {a}, not this machine"),
        }
    }
}

pub fn human_duration(d: Duration) -> String {
    match d.as_secs() {
        s @ 0..=119 => format!("{s}s"),
        s @ 120..=7199 => format!("{}m", s / 60),
        s @ 7200..=172_799 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub reasons: Vec<Reason>,
    pub debug_info: DebugInfo,
    pub is_rust: bool,
    /// Newest first.
    pub invocations: Vec<Invocation>,
}

impl Candidate {
    pub fn score(&self) -> i32 {
        self.reasons.iter().map(Reason::weight).sum()
    }
}

pub struct ElfFacts {
    pub debug_info: DebugInfo,
    pub arch: String,
    pub is_rust: bool,
}

/// None if the file is not a runnable ELF (shared libraries are rejected by the
/// absence of PT_INTERP, which is the only thing separating a PIE from a .so).
pub fn inspect(path: &Path) -> Option<ElfFacts> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;
    if &magic != b"\x7fELF" {
        return None;
    }
    // ReadCache pulls in only the headers and the sections asked for. Reading the
    // whole file is not an option: Rust debug binaries run to hundreds of MB and
    // this runs over every executable in the build tree.
    let cache = object::ReadCache::new(file);
    let obj = object::File::parse(&cache).ok()?;
    let runnable = match obj.kind() {
        object::ObjectKind::Executable => true,
        object::ObjectKind::Dynamic => obj.section_by_name(".interp").is_some(),
        _ => false,
    };
    if !runnable {
        return None;
    }
    let has = |n: &str| obj.section_by_name(n).is_some();
    let debug_info = if has(".debug_info") || has(".zdebug_info") {
        DebugInfo::Embedded
    } else if has(".gnu_debuglink") {
        DebugInfo::Separate
    } else if has(".symtab") {
        DebugInfo::SymbolsOnly
    } else {
        DebugInfo::None
    };
    // Cheap and good enough: rustc-built executables carry std's panic machinery
    // as strings even when stripped, and .rodata is far smaller than the symbol
    // table of a debug build.
    let is_rust = obj.section_by_name(".rodata").and_then(|s| s.data().ok()).is_some_and(|d| find(d, b"library/core/src/panicking.rs") || find(d, b"/rustc/"));
    Some(ElfFacts { debug_info, arch: format!("{:?}", obj.architecture()).to_lowercase(), is_rust })
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "arm" => "arm",
        "x86" => "i386",
        "riscv64" => "riscv64",
        other => other,
    }
}

/// Directories worth looking in, relative to a project root. Build trees only:
/// walking the whole checkout finds test fixtures and vendored junk.
fn search_dirs(root: &Path, cargo_target_dir: Option<&Path>) -> Vec<(PathBuf, u32)> {
    let mut v = vec![(root.to_path_buf(), 0)];
    // Both, not either: with CARGO_TARGET_DIR set the project-local target/ is
    // usually absent (costs one failed read_dir), but a stale one from before
    // the variable was set is still where last week's binary is.
    let mut targets = vec![root.join("target")];
    targets.extend(cargo_target_dir.map(Path::to_path_buf));
    for target in targets {
        for profile in ["debug", "release"] {
            v.push((target.join(profile), 0));
            v.push((target.join(profile).join("examples"), 0));
        }
        // target/<triple>/<profile>, which is where cross builds for the phone land.
        if let Ok(rd) = std::fs::read_dir(&target) {
            for e in rd.flatten().filter(|e| e.file_name().to_string_lossy().contains('-')) {
                for profile in ["debug", "release"] {
                    v.push((e.path().join(profile), 0));
                }
            }
        }
    }
    for d in ["build", "_build", "builddir", "bin", "out", "cmake-build-debug"] {
        v.push((root.join(d), 2));
    }
    v
}

fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            let name = e.file_name();
            let skip = matches!(name.to_str(), Some("deps" | "incremental" | ".fingerprint" | "CMakeFiles" | "meson-private" | ".git" | "node_modules"));
            if depth > 0 && !skip {
                walk(&p, depth - 1, out);
            }
        } else if ft.is_file() {
            use std::os::unix::fs::PermissionsExt;
            let exec = e.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0 && m.len() > 4096);
            let noise = p.extension().is_some_and(|x| x == "so" || x == "sh" || x == "py" || x == "d");
            if exec && !noise {
                out.push(p);
            }
        }
    }
}

pub fn scan(root: &Path, history: &[Entry], build_artifacts: &[PathBuf]) -> Vec<Candidate> {
    let target = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from);
    scan_with(root, history, build_artifacts, target.as_deref())
}

/// `scan` with the environment passed in. A shared CARGO_TARGET_DIR holds every
/// project's binaries, so candidates from it are other projects' as often as
/// not; they still rank below a name match, a history hit or a build artifact.
pub fn scan_with(root: &Path, history: &[Entry], build_artifacts: &[PathBuf], cargo_target_dir: Option<&Path>) -> Vec<Candidate> {
    let mut paths = Vec::new();
    for (dir, depth) in search_dirs(root, cargo_target_dir) {
        walk(&dir, depth, &mut paths);
    }
    paths.extend(build_artifacts.iter().cloned());
    paths.sort();
    paths.dedup();

    let project = root.file_name().and_then(|n| n.to_str()).map(|n| n.replace('-', "_"));
    let now = SystemTime::now();
    let mut out: Vec<Candidate> = paths
        .into_iter()
        .filter_map(|path| {
            let facts = inspect(&path)?;
            let mut reasons = vec![Reason::DebugInfo(facts.debug_info)];
            if let Ok(ago) = std::fs::metadata(&path).and_then(|m| m.modified()).map(|m| now.duration_since(m).unwrap_or_default()) {
                reasons.push(Reason::Built { ago });
            }
            let invocations = history::invocations_of(history, &path);
            if !invocations.is_empty() {
                reasons.push(Reason::InHistory { times: invocations.len() });
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).map(|s| s.replace('-', "_"));
            if project.is_some() && stem == project {
                reasons.push(Reason::NameMatchesProject);
            }
            if build_artifacts.contains(&path) {
                reasons.push(Reason::BuildArtifact);
            }
            let comps: Vec<_> = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            if comps.iter().any(|c| c == "release") {
                reasons.push(Reason::OptimizedProfile);
            }
            if comps.iter().any(|c| c == "examples") {
                reasons.push(Reason::Example);
            }
            if facts.arch != host_arch() {
                reasons.push(Reason::ForeignArch(facts.arch.clone()));
            }
            Some(Candidate { path, reasons, debug_info: facts.debug_info, is_rust: facts.is_rust, invocations })
        })
        .collect();
    out.sort_by(|a, b| b.score().cmp(&a.score()).then_with(|| a.path.cmp(&b.path)));
    out
}
