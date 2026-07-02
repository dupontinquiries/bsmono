//! A small abstraction over build systems, so importing a zip works for more than just
//! cargo. Each system is detected from the extracted tree and built the way it expects:
//! Rust → a cargo binary; Python (incl. pygame) → a venv with deps installed plus a
//! launcher script; and best-effort Go / Node / CMake / Make. Every system produces a
//! `Built` with something to run now and a single file to add to the apps list.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind { Cargo, Python, Node, Go, CMake, Make }

impl Kind {
    pub fn label(&self) -> &'static str {
        match self {
            Kind::Cargo => "Rust (cargo)",
            Kind::Python => "Python",
            Kind::Node => "Node.js",
            Kind::Go => "Go",
            Kind::CMake => "CMake",
            Kind::Make => "Make",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Detected {
    pub kind: Kind,
    pub root: PathBuf,
    pub name: String,
    pub description: String,
    pub runnable: bool,
    pub entry: Option<String>, // interpreted entrypoint (Python/Node)
}

#[derive(Debug, Clone)]
pub struct Built {
    pub run: PathBuf,      // what to exec to launch now
    pub args: Vec<String>, // args for `run`
    pub install: PathBuf,  // a single file to add to the apps list (binary or launcher)
    pub bytes: u64,
}

fn skip_dir(name: &str) -> bool {
    matches!(name, ".git" | ".gitmind" | "target" | "node_modules" | ".venv" | "venv" | "build" | "vendor" | "__pycache__")
        || name.starts_with('.')
}

/// Shallowest directory under `dir` (depth ≤ 6) that contains any of `names`.
fn shallowest_with(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    fn walk(dir: &Path, names: &[&str], depth: u32) -> Option<PathBuf> {
        if depth > 6 { return None; }
        if names.iter().any(|n| dir.join(n).exists()) {
            return Some(dir.to_path_buf());
        }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| !p.file_name().map(|n| skip_dir(&n.to_string_lossy())).unwrap_or(true))
            .collect();
        subs.sort();
        for s in subs {
            if let Some(f) = walk(&s, names, depth + 1) { return Some(f); }
        }
        None
    }
    walk(dir, names, 0)
}

/// Detect the build system + project root in an extracted tree. Cargo first (Rust
/// monorepos), then Python (the common game/tool case), then Go / Node / CMake / Make.
pub fn detect(dir: &Path) -> Option<Detected> {
    if let Some(root) = shallowest_with(dir, &["Cargo.toml"]) {
        let toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
        let name = crate::buildflags::crate_name(&toml).unwrap_or_else(|| dir_name(&root));
        let runnable = root.join("src/main.rs").is_file() || root.join("src/bin").is_dir() || crate::buildflags::declares_binary(&toml);
        return Some(Detected {
            kind: Kind::Cargo, root, name,
            description: crate::buildflags::package_description(&toml).unwrap_or_default(),
            runnable, entry: None,
        });
    }
    if let Some(d) = detect_python(dir) { return Some(d); }
    if let Some(root) = shallowest_with(dir, &["go.mod"]) {
        let name = dir_name(&root);
        return Some(Detected { kind: Kind::Go, root, name, description: String::new(), runnable: true, entry: None });
    }
    if let Some(root) = shallowest_with(dir, &["package.json"]) {
        let pkg = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
        let name = json_field(&pkg, "name").unwrap_or_else(|| dir_name(&root));
        let entry = json_field(&pkg, "main").or_else(|| ["index.js", "main.js", "app.js"].iter().map(|f| f.to_string()).find(|f| root.join(f).is_file()));
        return Some(Detected { kind: Kind::Node, root, name, description: json_field(&pkg, "description").unwrap_or_default(), runnable: entry.is_some() || pkg.contains("\"start\""), entry });
    }
    if let Some(root) = shallowest_with(dir, &["CMakeLists.txt"]) {
        return Some(Detected { kind: Kind::CMake, root: root.clone(), name: dir_name(&root), description: String::new(), runnable: true, entry: None });
    }
    if let Some(root) = shallowest_with(dir, &["Makefile", "makefile", "GNUmakefile"]) {
        return Some(Detected { kind: Kind::Make, root: root.clone(), name: dir_name(&root), description: String::new(), runnable: true, entry: None });
    }
    None
}

fn dir_name(p: &Path) -> String {
    p.file_name().map(|n| crate::places::sanitize_name(&n.to_string_lossy())).filter(|s| !s.is_empty()).unwrap_or_else(|| "app".into())
}

fn json_field(json: &str, key: &str) -> Option<String> {
    // tiny extractor for "key": "value" (good enough for package.json name/main/description)
    let needle = format!("\"{key}\"");
    let i = json.find(&needle)? + needle.len();
    let rest = &json[i..];
    let colon = rest.find(':')?;
    let after = rest[colon + 1..].trim_start();
    if !after.starts_with('"') { return None; }
    let s = &after[1..];
    let end = s.find('"')?;
    Some(s[..end].to_string())
}

// ── Python ──

const PY_MANIFESTS: &[&str] = &["requirements.txt", "pyproject.toml", "setup.py", "Pipfile"];
const PY_ENTRIES: &[&str] = &["main.py", "app.py", "game.py", "run.py", "__main__.py"];

fn detect_python(dir: &Path) -> Option<Detected> {
    fn looks_python(d: &Path) -> bool {
        PY_MANIFESTS.iter().any(|m| d.join(m).is_file())
            || PY_ENTRIES.iter().any(|e| d.join(e).is_file())
            || dir_has_py_importing_pygame(d)
    }
    fn walk(d: &Path, depth: u32) -> Option<PathBuf> {
        if depth > 6 { return None; }
        if looks_python(d) { return Some(d.to_path_buf()); }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(d).ok()?.flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| !p.file_name().map(|n| skip_dir(&n.to_string_lossy())).unwrap_or(true))
            .collect();
        subs.sort();
        for s in subs { if let Some(f) = walk(&s, depth + 1) { return Some(f); } }
        None
    }
    let root = walk(dir, 0)?;
    let entry = python_entry(&root);
    let name = python_name(&root).unwrap_or_else(|| entry.as_deref().map(|e| crate::places::sanitize_name(e.trim_end_matches(".py"))).unwrap_or_else(|| dir_name(&root)));
    let pygame = dir_has_py_importing_pygame(&root);
    Some(Detected {
        kind: Kind::Python, root, name,
        description: if pygame { "pygame app".into() } else { "Python app".into() },
        runnable: entry.is_some(), entry,
    })
}

fn dir_has_py_importing_pygame(d: &Path) -> bool {
    std::fs::read_dir(d).ok().map(|rd| rd.flatten().any(|e| {
        let p = e.path();
        p.extension().map(|x| x == "py").unwrap_or(false)
            && std::fs::read_to_string(&p).map(|t| t.contains("import pygame")).unwrap_or(false)
    })).unwrap_or(false)
}

fn python_entry(root: &Path) -> Option<String> {
    for e in PY_ENTRIES {
        if root.join(e).is_file() { return Some((*e).to_string()); }
    }
    // a .py with a __main__ guard
    let pys: Vec<PathBuf> = std::fs::read_dir(root).ok()?.flatten()
        .map(|e| e.path()).filter(|p| p.extension().map(|x| x == "py").unwrap_or(false)).collect();
    for p in &pys {
        if std::fs::read_to_string(p).map(|t| t.contains("__name__") && t.contains("__main__")).unwrap_or(false) {
            return p.file_name().map(|n| n.to_string_lossy().into_owned());
        }
    }
    if pys.len() == 1 {
        return pys[0].file_name().map(|n| n.to_string_lossy().into_owned());
    }
    None
}

fn python_name(root: &Path) -> Option<String> {
    if let Ok(toml) = std::fs::read_to_string(root.join("pyproject.toml")) {
        // [project]\nname = "x"
        if let Some(i) = toml.find("name") {
            let rest = &toml[i..];
            if let Some(q) = rest.find('"') {
                let s = &rest[q + 1..];
                if let Some(end) = s.find('"') { return Some(crate::places::sanitize_name(&s[..end])); }
            }
        }
    }
    None
}

/// Third-party top-level modules imported anywhere under `root` (best-effort), for the
/// case where there's no requirements file. Stdlib and local modules are excluded.
pub fn scan_imports(root: &Path) -> Vec<String> {
    let local: std::collections::BTreeSet<String> = std::fs::read_dir(root).ok().map(|rd| rd.flatten().filter_map(|e| {
        let p = e.path();
        if p.is_dir() { p.file_name().map(|n| n.to_string_lossy().into_owned()) }
        else if p.extension().map(|x| x == "py").unwrap_or(false) { p.file_stem().map(|n| n.to_string_lossy().into_owned()) }
        else { None }
    }).collect()).unwrap_or_default();
    let mut mods = std::collections::BTreeSet::new();
    collect_py(root, 0, &mut |text| {
        for line in text.lines() {
            let l = line.trim();
            let module = if let Some(rest) = l.strip_prefix("import ") {
                rest.split([' ', ',', '.']).next().unwrap_or("").to_string()
            } else if let Some(rest) = l.strip_prefix("from ") {
                rest.split([' ', '.']).next().unwrap_or("").to_string()
            } else { continue };
            if !module.is_empty() { mods.insert(module); }
        }
    });
    mods.into_iter()
        .filter(|m| !STDLIB.contains(&m.as_str()) && !local.contains(m))
        .map(|m| pip_name(&m))
        .collect()
}

fn collect_py(dir: &Path, depth: u32, f: &mut dyn FnMut(&str)) {
    if depth > 5 { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if !p.file_name().map(|n| skip_dir(&n.to_string_lossy())).unwrap_or(true) { collect_py(&p, depth + 1, f); }
        } else if p.extension().map(|x| x == "py").unwrap_or(false) {
            if let Ok(t) = std::fs::read_to_string(&p) { f(&t); }
        }
    }
}

fn pip_name(module: &str) -> String {
    match module {
        "cv2" => "opencv-python".into(),
        "PIL" => "Pillow".into(),
        "yaml" => "PyYAML".into(),
        "OpenGL" => "PyOpenGL".into(),
        "sklearn" => "scikit-learn".into(),
        other => other.to_string(),
    }
}

const STDLIB: &[&str] = &[
    "abc","argparse","array","ast","asyncio","base64","bisect","builtins","collections","contextlib",
    "copy","csv","ctypes","dataclasses","datetime","decimal","difflib","dis","enum","errno","functools",
    "gc","getpass","glob","gzip","hashlib","heapq","hmac","html","http","importlib","inspect","io",
    "ipaddress","itertools","json","logging","math","multiprocessing","operator","os","pathlib","pickle",
    "platform","pprint","queue","random","re","secrets","select","shlex","shutil","signal","socket",
    "sqlite3","ssl","stat","string","struct","subprocess","sys","tempfile","textwrap","threading","time",
    "timeit","tkinter","traceback","types","typing","unicodedata","unittest","urllib","uuid","venv",
    "warnings","weakref","xml","zipfile","zlib","__future__",
];

// ── build dispatch ──

fn run(cmd: &mut Command) -> Result<(), String> {
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("the build tool isn't installed on this device".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// Build a detected project for `target` (only meaningful for Cargo) and return how to run
/// it plus a single file to install.
pub fn build(d: &Detected, target: &str, path_env: &str) -> Result<Built, String> {
    match d.kind {
        Kind::Cargo => {
            let (bin, bytes) = crate::ops::build_locally(&d.name, &d.root, target, &[], false)?;
            Ok(Built { run: bin.clone(), args: vec![], install: bin, bytes })
        }
        Kind::Python => build_python(d, path_env),
        Kind::Go => build_go(d, path_env),
        Kind::Node => build_node(d, path_env),
        Kind::CMake => build_cmake(d, path_env),
        Kind::Make => build_make(d, path_env),
    }
}

fn write_launcher(root: &Path, name: &str, exec: &str, args: &[String]) -> Result<PathBuf, String> {
    let quoted: String = args.iter().map(|a| format!(" \"{a}\"")).collect();
    let script = format!(
        "#!/bin/sh\n# gitmind launcher for {name}\ncd \"{root}\"\nexec \"{exec}\"{quoted} \"$@\"\n",
        root = root.display(), exec = exec,
    );
    let path = root.join(".gitmind-run.sh");
    std::fs::write(&path, script).map_err(|e| format!("write launcher: {e}"))?;
    crate::ops::set_executable(&path.display().to_string());
    Ok(path)
}

fn build_python(d: &Detected, path_env: &str) -> Result<Built, String> {
    let root = &d.root;
    let venv = root.join(".venv");
    let py = venv.join("bin/python");
    if !py.exists() {
        run(Command::new("python3").args(["-m", "venv", ".venv"]).current_dir(root).env("PATH", path_env))
            .map_err(|e| format!("couldn't create a virtualenv (need python3-venv): {e}"))?;
    }
    let pip = venv.join("bin/pip").display().to_string();
    // dependencies
    let pip_err = |e: String| format!("installing dependencies failed: {e}\n(on minimal systems pygame needs system libs, e.g. Alpine: apk add python3-dev sdl2-dev sdl2_image-dev sdl2_mixer-dev sdl2_ttf-dev)");
    if root.join("requirements.txt").is_file() {
        run(Command::new(&pip).args(["install", "-r", "requirements.txt"]).current_dir(root).env("PATH", path_env)).map_err(pip_err)?;
    } else if root.join("pyproject.toml").is_file() || root.join("setup.py").is_file() {
        run(Command::new(&pip).args(["install", "."]).current_dir(root).env("PATH", path_env)).map_err(pip_err)?;
    } else {
        let mods = scan_imports(root);
        if !mods.is_empty() {
            let mut args = vec!["install".to_string()];
            args.extend(mods);
            run(Command::new(&pip).args(&args).current_dir(root).env("PATH", path_env)).map_err(pip_err)?;
        }
    }
    let entry = d.entry.clone().ok_or("couldn't find a Python entrypoint (main.py / app.py / game.py)")?;
    let launcher = write_launcher(root, &d.name, &py.display().to_string(), &[entry.clone()])?;
    let bytes = std::fs::metadata(&launcher).map(|m| m.len()).unwrap_or(0);
    Ok(Built { run: py, args: vec![entry], install: launcher, bytes })
}

fn build_go(d: &Detected, path_env: &str) -> Result<Built, String> {
    let root = &d.root;
    let out = root.join(".gitmind-bin").join(&d.name);
    let _ = std::fs::create_dir_all(root.join(".gitmind-bin"));
    run(Command::new("go").args(["build", "-o", &out.display().to_string(), "."]).current_dir(root).env("PATH", path_env))
        .map_err(|e| format!("go build failed: {e}"))?;
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    Ok(Built { run: out.clone(), args: vec![], install: out, bytes })
}

fn build_node(d: &Detected, path_env: &str) -> Result<Built, String> {
    let root = &d.root;
    run(Command::new("npm").args(["install", "--no-audit", "--no-fund"]).current_dir(root).env("PATH", path_env))
        .map_err(|e| format!("npm install failed: {e}"))?;
    let pkg = std::fs::read_to_string(root.join("package.json")).unwrap_or_default();
    let (exec, args) = if pkg.contains("\"start\"") {
        ("npm".to_string(), vec!["start".to_string()])
    } else {
        let entry = d.entry.clone().ok_or("no entry (package.json main / index.js) and no start script")?;
        ("node".to_string(), vec![entry])
    };
    let launcher = write_launcher(root, &d.name, &exec, &args)?;
    let bytes = std::fs::metadata(&launcher).map(|m| m.len()).unwrap_or(0);
    Ok(Built { run: PathBuf::from(exec), args, install: launcher, bytes })
}

fn build_cmake(d: &Detected, path_env: &str) -> Result<Built, String> {
    let root = &d.root;
    let build = root.join("build");
    run(Command::new("cmake").args(["-S", ".", "-B", "build", "-DCMAKE_BUILD_TYPE=Release"]).current_dir(root).env("PATH", path_env))
        .map_err(|e| format!("cmake configure failed: {e}"))?;
    run(Command::new("cmake").args(["--build", "build", "--config", "Release"]).current_dir(root).env("PATH", path_env))
        .map_err(|e| format!("cmake build failed: {e}"))?;
    let bin = find_executable(&build, &d.name).ok_or("built, but couldn't find the produced executable in build/")?;
    let bytes = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
    Ok(Built { run: bin.clone(), args: vec![], install: bin, bytes })
}

fn build_make(d: &Detected, path_env: &str) -> Result<Built, String> {
    let root = &d.root;
    run(Command::new("make").current_dir(root).env("PATH", path_env)).map_err(|e| format!("make failed: {e}"))?;
    let bin = find_executable(root, &d.name).ok_or("built, but couldn't find the produced executable — run it manually")?;
    let bytes = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
    Ok(Built { run: bin.clone(), args: vec![], install: bin, bytes })
}

/// Find an executable file under `dir` (prefer one named `name`). Skips libraries/objects.
fn find_executable(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut best: Option<PathBuf> = None;
    fn is_exec(p: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    fn skip_ext(p: &Path) -> bool {
        matches!(p.extension().and_then(|e| e.to_str()), Some("o") | Some("a") | Some("so") | Some("cmake") | Some("txt"))
    }
    fn walk(dir: &Path, name: &str, depth: u32, best: &mut Option<PathBuf>) {
        if depth > 4 { return; }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() { walk(&p, name, depth + 1, best); }
            else if is_exec(&p) && !skip_ext(&p) {
                if p.file_name().map(|n| n.to_string_lossy() == name).unwrap_or(false) {
                    *best = Some(p); return;
                }
                if best.is_none() { *best = Some(p); }
            }
        }
    }
    walk(dir, name, 0, &mut best);
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_cargo_and_python_and_entry() {
        let base = std::env::temp_dir().join(format!("gm-bs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        // cargo
        let c = base.join("rust/proj");
        fs::create_dir_all(c.join("src")).unwrap();
        fs::write(c.join("Cargo.toml"), "[package]\nname = \"coolgame\"\nversion = \"0.1.0\"\n").unwrap();
        fs::write(c.join("src/main.rs"), "fn main(){}").unwrap();
        let d = detect(&base.join("rust")).unwrap();
        assert_eq!(d.kind, Kind::Cargo);
        assert_eq!(d.name, "coolgame");
        assert!(d.runnable);
        // python pygame
        let p = base.join("py/game");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("main.py"), "import pygame\nimport os\nprint('hi')\n").unwrap();
        let dp = detect(&base.join("py")).unwrap();
        assert_eq!(dp.kind, Kind::Python);
        assert_eq!(dp.entry.as_deref(), Some("main.py"));
        assert_eq!(dp.description, "pygame app");
        // import scan drops stdlib, keeps pygame
        let mods = scan_imports(&p);
        assert!(mods.contains(&"pygame".to_string()) && !mods.iter().any(|m| m == "os"));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn builds_a_python_app_into_a_runnable_launcher() {
        let base = std::env::temp_dir().join(format!("gm-bspy-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let root = base.join("app");
        fs::create_dir_all(&root).unwrap();
        // stdlib-only so no network install is needed for the test
        fs::write(root.join("main.py"), "import sys\nprint('launched-ok')\n").unwrap();
        let d = detect(&base).unwrap();
        assert_eq!(d.kind, Kind::Python);
        let path = std::env::var("PATH").unwrap_or_default();
        let built = build(&d, "", &path).expect("python build");
        assert!(built.install.ends_with(".gitmind-run.sh"), "produces a launcher script");
        assert!(root.join(".venv/bin/python").exists(), "venv was created");
        // running the launcher actually runs the app
        let out = Command::new("sh").arg(&built.install).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("launched-ok"), "launcher runs the entrypoint");
        let _ = fs::remove_dir_all(&base);
    }
}
