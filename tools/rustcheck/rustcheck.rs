// rustcheck (Rust port) — AI-centric Rust + GPU-shader checker.
// Compatible with the Python version: same tiers, output format, exit codes.
//   0 clean · 1 problems in YOUR code · 2 inconclusive (env/dep/timeout)

use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SHADER_EXTS: &[&str] = &[
    "vert", "frag", "comp", "geom", "tesc", "tese", "glsl", "mesh", "task",
    "rgen", "rchit", "rmiss", "rahit", "rcall", "rint", "spv",
];
const STAGE_EXTS: &[&str] = &[
    "vert", "frag", "comp", "geom", "tesc", "tese", "mesh", "task",
];

#[derive(Default)]
struct Args {
    paths: Vec<String>,
    changed: Option<Vec<String>>,
    git: bool,
    fast: bool,
    shaders: bool,
    clippy: bool,
    deny_warnings: bool,
    full: bool,
    no_default_features: bool,
    features: Option<String>,
    all_targets: bool,
    tests: bool,
    offline: bool,
    edition: String,
    timeout: u64,
    json: bool,
}

fn parse_args() -> Args {
    let mut a = Args { edition: "2021".into(), timeout: 600, ..Default::default() };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let s = &argv[i];
        match s.as_str() {
            "--git" => a.git = true,
            "--fast" => a.fast = true,
            "--shaders" => a.shaders = true,
            "--clippy" => a.clippy = true,
            "--deny-warnings" => a.deny_warnings = true,
            "--full" => a.full = true,
            "--no-default-features" => a.no_default_features = true,
            "--all-targets" => a.all_targets = true,
            "--tests" => a.tests = true,
            "--offline" => a.offline = true,
            "--json" => a.json = true,
            "--features" => { i += 1; a.features = argv.get(i).cloned(); }
            "--edition" => { i += 1; if let Some(v) = argv.get(i) { a.edition = v.clone(); } }
            "--timeout" => { i += 1; if let Some(v) = argv.get(i) { a.timeout = v.parse().unwrap_or(600); } }
            "--jobs" => { i += 1; }
            "-h" | "--help" => { print_help(); std::process::exit(0); }
            "--changed" => {
                let mut v = vec![];
                while i + 1 < argv.len() && !argv[i + 1].starts_with("--") {
                    i += 1; v.push(argv[i].clone());
                }
                a.changed = Some(v);
            }
            _ => a.paths.push(s.clone()),
        }
        i += 1;
    }
    if a.paths.is_empty() { a.paths.push(".".into()); }
    a
}

fn print_help() {
    println!("rustcheck — AI-centric Rust + shader checker
USAGE: rustcheck [PATHS...] [--changed F...] [--git] [--fast] [--shaders]
                 [--clippy] [--deny-warnings] [--full] [--no-default-features]
                 [--features X] [--all-targets] [--tests] [--offline]
                 [--edition E] [--timeout S] [--json]
EXIT:  0 clean · 1 your-code problems · 2 inconclusive (env/dep/timeout)");
}

// ───────── Tier 0: static Rust lexer ─────────

struct Issue { kind: String, line: usize, col: usize, msg: String }

fn find_conflicts(src: &str) -> Vec<Issue> {
    let mut out = vec![];
    for (n, line) in src.lines().enumerate() {
        if line.starts_with("<<<<<<< ") || line.starts_with(">>>>>>> ")
            || line.starts_with("||||||| ") || line.trim_end() == "======="
        {
            let mut t: String = line.chars().take(40).collect();
            t = format!("conflict marker: {:?}", t);
            out.push(Issue { kind: "merge-conflict".into(), line: n + 1, col: 1, msg: t });
        }
    }
    out
}

// returns (issues, hard_break)
fn scan_source(src: &str) -> (Vec<Issue>, bool) {
    let s: Vec<char> = src.chars().collect();
    let n = s.len();
    let (mut i, mut line, mut col) = (0usize, 1usize, 1usize);
    let mut stack: Vec<(char, usize, usize)> = vec![];
    let mut issues = vec![];
    macro_rules! adv { ($k:expr) => {{ for _ in 0..$k { if i < n { if s[i] == '\n' { line += 1; col = 1; } else { col += 1; } i += 1; } } }}; }

    while i < n {
        let c = s[i];
        if c == '/' && i + 1 < n && s[i + 1] == '/' {
            while i < n && s[i] != '\n' { adv!(1); }
            continue;
        }
        if c == '/' && i + 1 < n && s[i + 1] == '*' {
            let (sl, sc) = (line, col); let mut depth = 1; adv!(2);
            while i < n && depth > 0 {
                if s[i] == '/' && i + 1 < n && s[i + 1] == '*' { depth += 1; adv!(2); }
                else if s[i] == '*' && i + 1 < n && s[i + 1] == '/' { depth -= 1; adv!(2); }
                else { adv!(1); }
            }
            if depth > 0 {
                issues.push(Issue { kind: "unterminated-block-comment".into(), line: sl, col: sc, msg: "block comment never closed".into() });
                return (issues, true);
            }
            continue;
        }
        if (c == 'r' || c == 'b') && is_raw_start(&s, i) {
            let mut j = i;
            if s[j] == 'b' { j += 1; }
            j += 1;
            let mut hashes = 0;
            while j < n && s[j] == '#' { hashes += 1; j += 1; }
            if j < n && s[j] == '"' {
                let (sl, sc) = (line, col);
                adv!(j - i + 1);
                let closer: String = std::iter::once('"').chain(std::iter::repeat('#').take(hashes)).collect();
                let rest: String = s[i..].iter().collect();
                if let Some(pos) = rest.find(&closer) {
                    let chars_to = rest[..pos].chars().count() + closer.chars().count();
                    adv!(chars_to);
                    continue;
                } else {
                    issues.push(Issue { kind: "unterminated-raw-string".into(), line: sl, col: sc, msg: "raw string never closed".into() });
                    return (issues, true);
                }
            }
        }
        if c == '"' || (c == 'b' && i + 1 < n && s[i + 1] == '"') {
            let (sl, sc) = (line, col);
            if c == 'b' { adv!(1); }
            adv!(1);
            let mut ok = false;
            while i < n {
                if s[i] == '\\' { adv!(2); continue; }
                if s[i] == '"' { adv!(1); ok = true; break; }
                adv!(1);
            }
            if !ok {
                issues.push(Issue { kind: "unterminated-string".into(), line: sl, col: sc, msg: "string literal never closed".into() });
                return (issues, true);
            }
            continue;
        }
        if c == '\'' || (c == 'b' && i + 1 < n && s[i + 1] == '\'') {
            let off = if c == 'b' { 1 } else { 0 };
            if looks_like_char(&s, i + off) {
                if c == 'b' { adv!(1); }
                adv!(1);
                while i < n {
                    if s[i] == '\\' { adv!(2); continue; }
                    if s[i] == '\'' { adv!(1); break; }
                    adv!(1);
                }
                continue;
            }
        }
        if c == '(' || c == '[' || c == '{' { stack.push((c, line, col)); adv!(1); continue; }
        if c == ')' || c == ']' || c == '}' {
            let want = match c { ')' => '(', ']' => '[', _ => '{' };
            match stack.pop() {
                None => { issues.push(Issue { kind: "stray-delim".into(), line, col, msg: format!("stray '{}'", c) }); return (issues, true); }
                Some((oc, ol, ocol)) if oc != want => {
                    issues.push(Issue { kind: "mismatched-delim".into(), line, col, msg: format!("'{}' closes '{}' opened at {}:{}", c, oc, ol, ocol) });
                    return (issues, true);
                }
                _ => { adv!(1); continue; }
            }
        }
        adv!(1);
    }
    if let Some((oc, ol, ocol)) = stack.last() {
        issues.push(Issue { kind: "unclosed-delim".into(), line: *ol, col: *ocol, msg: format!("'{}' is never closed", oc) });
        return (issues, true);
    }
    (issues, false)
}

fn is_raw_start(s: &[char], i: usize) -> bool {
    let mut j = i;
    if s[j] == 'b' { j += 1; }
    if j >= s.len() || s[j] != 'r' { return false; }
    j += 1;
    while j < s.len() && s[j] == '#' { j += 1; }
    j < s.len() && s[j] == '"'
}

fn looks_like_char(s: &[char], i: usize) -> bool {
    let n = s.len();
    if i + 1 >= n { return false; }
    if s[i + 1] == '\\' {
        for k in (i + 2)..(i + 14).min(n) { if s[k] == '\'' { return true; } }
        return false;
    }
    i + 2 < n && s[i + 2] == '\''
}

fn resolve_mods(p: &Path, src: &str) -> Vec<Issue> {
    let mut out = vec![];
    let parent = p.parent().unwrap_or(Path::new("."));
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let base: PathBuf = if stem == "main" || stem == "lib" || stem == "mod" {
        parent.to_path_buf()
    } else { parent.join(stem) };
    let lines: Vec<&str> = src.lines().collect();
    for (n, raw) in lines.iter().enumerate() {
        let line = raw.trim();
        if line.starts_with("//") { continue; }
        for kw in ["pub mod ", "mod "] {
            if line.starts_with(kw) && line.ends_with(';') {
                let name = line[kw.len()..line.len() - 1].trim();
                if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') || name.chars().next().map(|c| c.is_numeric()).unwrap_or(true) { break; }
                let prev = if n >= 1 { lines[n - 1].trim() } else { "" };
                if prev.contains("#[path") || prev.contains("#[cfg") { break; }
                let a = base.join(format!("{}.rs", name));
                let b = base.join(name).join("mod.rs");
                if !a.exists() && !b.exists() {
                    out.push(Issue { kind: "missing-mod-file".into(), line: n + 1, col: 1, msg: format!("`mod {};` but no file (expected {})", name, a.display()) });
                }
                break;
            }
        }
    }
    out
}

struct StaticResult { file: String, issues: Vec<Issue>, hard: bool }

fn static_check(p: &Path) -> StaticResult {
    let src = match std::fs::read_to_string(p) {
        Ok(s) => s,
        Err(e) => return StaticResult { file: p.display().to_string(), issues: vec![Issue { kind: "read".into(), line: 0, col: 1, msg: e.to_string() }], hard: true },
    };
    let mut issues = find_conflicts(&src);
    let hard_conflict = !issues.is_empty();
    let (lex, hard) = scan_source(&src);
    issues.extend(lex);
    issues.extend(resolve_mods(p, &src));
    StaticResult { file: p.display().to_string(), issues, hard: hard || hard_conflict }
}

// ───────── Shader checker ─────────

struct ShaderIssue { kind: String, line: usize, msg: String, level: String }
struct ShaderResult { file: String, issues: Vec<ShaderIssue> }

fn strip_glsl_comments(src: &str) -> String {
    let s: Vec<char> = src.chars().collect();
    let n = s.len(); let mut out = String::with_capacity(n); let mut i = 0;
    while i < n {
        let c = s[i];
        if c == '/' && i + 1 < n && s[i + 1] == '/' {
            while i < n && s[i] != '\n' { out.push(' '); i += 1; }
            continue;
        }
        if c == '/' && i + 1 < n && s[i + 1] == '*' {
            out.push(' '); out.push(' '); i += 2;
            while i < n && !(s[i] == '*' && i + 1 < n && s[i + 1] == '/') {
                out.push(if s[i] == '\n' { '\n' } else { ' ' }); i += 1;
            }
            if i < n { out.push(' '); out.push(' '); i += 2; }
            continue;
        }
        out.push(c); i += 1;
    }
    out
}

fn lineno(src: &str, pos: usize) -> usize { src[..pos].matches('\n').count() + 1 }

fn check_glsl(p: &Path, src: &str) -> ShaderResult {
    let mut issues = vec![];
    macro_rules! add { ($k:expr, $l:expr, $m:expr, $lv:expr) => { issues.push(ShaderIssue { kind: $k.into(), line: $l, msg: $m.into(), level: $lv.into() }); }; }

    // structural delimiters + block comments
    let s: Vec<char> = src.chars().collect(); let n = s.len();
    let (mut i, mut line) = (0usize, 1usize);
    let mut stack: Vec<(char, usize)> = vec![]; let mut hard = false;
    while i < n {
        let c = s[i];
        if c == '\n' { line += 1; i += 1; continue; }
        if c == '/' && i + 1 < n && s[i + 1] == '/' { while i < n && s[i] != '\n' { i += 1; } continue; }
        if c == '/' && i + 1 < n && s[i + 1] == '*' {
            let sl = line; i += 2;
            while i + 1 < n && !(s[i] == '*' && s[i + 1] == '/') { if s[i] == '\n' { line += 1; } i += 1; }
            if i + 1 >= n { add!("unterminated-block-comment", sl, "block comment never closed", "error"); hard = true; break; }
            i += 2; continue;
        }
        if c == '(' || c == '[' || c == '{' { stack.push((c, line)); i += 1; continue; }
        if c == ')' || c == ']' || c == '}' {
            let want = match c { ')' => '(', ']' => '[', _ => '{' };
            match stack.pop() {
                None => { add!("stray-delim", line, format!("stray '{}'", c), "error"); hard = true; break; }
                Some((oc, ol)) if oc != want => { add!("mismatched-delim", line, format!("'{}' closes '{}' opened at line {}", c, oc, ol), "error"); hard = true; break; }
                _ => { i += 1; continue; }
            }
        }
        i += 1;
    }
    if !hard { if let Some((oc, ol)) = stack.last() { add!("unclosed-delim", *ol, format!("'{}' never closed", oc), "error"); hard = true; } }

    let code = strip_glsl_comments(src);

    // preprocessor balance
    let pp = Regex::new(r"(?m)^\s*#\s*(ifdef|ifndef|if|endif)\b").unwrap();
    let mut depth = 0i32; let mut first_open = 0usize;
    for cap in pp.captures_iter(&code) {
        let m = cap.get(0).unwrap();
        if &cap[1] == "endif" {
            depth -= 1;
            if depth < 0 { add!("preproc", lineno(&code, m.start()), "#endif without matching #if", "error"); depth = 0; }
        } else {
            if first_open == 0 { first_open = lineno(&code, m.start()); }
            depth += 1;
        }
    }
    if depth > 0 { add!("preproc", first_open.max(1), format!("{} unterminated #if/#ifdef (missing #endif)", depth), "error"); }

    if hard { return ShaderResult { file: p.display().to_string(), issues }; }

    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
    let is_stage = STAGE_EXTS.contains(&ext);
    let stage = ext;

    if is_stage {
        if !Regex::new(r"(?m)^\s*#\s*version\b").unwrap().is_match(&code) {
            add!("no-version", 1, "missing `#version` directive (Vulkan GLSL needs e.g. #version 450)", "error");
        }
        if !Regex::new(r"\bvoid\s+main\s*\(").unwrap().is_match(&code) {
            add!("no-main", 1, "no `void main()` entry point found", "error");
        }
    }

    let layout_re = Regex::new(r"layout\s*\(([^)]*)\)\s*([^;{]*)").unwrap();
    let mut descriptors: HashMap<(String, String), usize> = HashMap::new();
    let mut in_locs: HashMap<String, usize> = HashMap::new();
    let mut out_locs: HashMap<String, usize> = HashMap::new();
    let mut push_constants: Vec<usize> = vec![];
    for cap in layout_re.captures_iter(&code) {
        let argstr = &cap[1]; let rest = &cap[2];
        let ln = lineno(&code, cap.get(0).unwrap().start());
        let mut args: HashMap<String, String> = HashMap::new();
        for part in argstr.split(',') {
            let part = part.trim();
            if part.is_empty() { continue; }
            if let Some((k, v)) = part.split_once('=') { args.insert(k.trim().into(), v.trim().into()); }
            else { args.insert(part.into(), "true".into()); }
        }
        if args.keys().any(|k| k.starts_with("local_size")) { continue; }
        if args.contains_key("constant_id") { continue; }
        let storage = ["uniform", "buffer", "in", "out", "shared"].iter()
            .find(|kw| Regex::new(&format!(r"\b{}\b", kw)).unwrap().is_match(rest)).copied();
        if args.contains_key("push_constant") { push_constants.push(ln); continue; }
        match storage {
            Some("uniform") | Some("buffer") => {
                let storage = storage.unwrap();
                if !args.contains_key("binding") {
                    add!("missing-binding", ln, format!("{} block has no `binding=` (required in Vulkan GLSL)", storage), "error");
                } else {
                    let set = args.get("set").cloned().unwrap_or_else(|| "0".into());
                    let key = (set.clone(), args["binding"].clone());
                    if let Some(prev) = descriptors.get(&key) {
                        add!("binding-collision", ln, format!("set {} binding {} already used at line {}", key.0, key.1, prev), "error");
                    } else { descriptors.insert(key, ln); }
                }
            }
            Some("in") | Some("out") => {
                if let Some(loc) = args.get("location") {
                    let tbl = if storage == Some("in") { &mut in_locs } else { &mut out_locs };
                    if let Some(prev) = tbl.get(loc) {
                        add!("location-collision", ln, format!("{} location {} already used at line {}", storage.unwrap(), loc, prev), "error");
                    } else { tbl.insert(loc.clone(), ln); }
                }
            }
            _ => {}
        }
    }

    // bare uniform/buffer with no layout at all
    let bare = Regex::new(r"(?m)^\s*(uniform|buffer)\s+([A-Za-z_]\w*)").unwrap();
    for cap in bare.captures_iter(&code) {
        let m = cap.get(0).unwrap();
        let lstart = code[..m.start()].rfind('\n').map(|x| x + 1).unwrap_or(0);
        if !code[lstart..m.start()].contains("layout") && !code[m.start()..(m.end()+40).min(code.len())].contains("push_constant") {
            add!("missing-binding", lineno(&code, m.start()), format!("`{}` declaration without a `layout(binding=...)` (Vulkan requires it)", &cap[1]), "error");
        }
    }

    if push_constants.len() > 1 {
        add!("push-constant", push_constants[1], "more than one push_constant block (only one allowed)", "error");
    }

    if stage == "comp" && !Regex::new(r"layout\s*\([^)]*local_size_x").unwrap().is_match(&code) {
        add!("compute-no-localsize", 1, "compute shader missing `layout(local_size_x = ...) in;`", "error");
    }
    if stage == "vert" && !code.contains("gl_Position") {
        add!("vertex-no-position", 1, "vertex shader never writes `gl_Position`", "warning");
    }
    if stage == "frag" && !Regex::new(r"\bout\s+\w").unwrap().is_match(&code) && !code.contains("gl_FragDepth") {
        add!("fragment-no-output", 1, "fragment shader declares no `out` color and no gl_FragDepth", "warning");
    }

    for (pat, msg) in [
        (r"\bgl_FragColor\b", "gl_FragColor is legacy; use a `layout(location=0) out` in Vulkan"),
        (r"\btexture2D\s*\(", "texture2D() is legacy; use texture() in core/Vulkan GLSL"),
        (r"\bvarying\b", "`varying` is legacy; use in/out"),
        (r"\battribute\b", "`attribute` is legacy; use `layout(location=) in`"),
    ] {
        if let Some(m) = Regex::new(pat).unwrap().find(&code) {
            add!("legacy", lineno(&code, m.start()), msg, "warning");
        }
    }

    ShaderResult { file: p.display().to_string(), issues }
}

fn check_spv(p: &Path) -> ShaderResult {
    let b = match std::fs::read(p) {
        Ok(b) => b,
        Err(e) => return ShaderResult { file: p.display().to_string(), issues: vec![ShaderIssue { kind: "read".into(), line: 0, msg: e.to_string(), level: "error".into() }] },
    };
    if b.len() < 20 {
        return ShaderResult { file: p.display().to_string(), issues: vec![ShaderIssue { kind: "spv-truncated".into(), line: 0, msg: "file too small to be SPIR-V".into(), level: "error".into() }] };
    }
    let le = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let be = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let (endian, ver): (&str, [u8; 4]) = if le == 0x0723_0203 {
        ("little", [b[4], b[5], b[6], b[7]])
    } else if be == 0x0723_0203 {
        ("big", [b[7], b[6], b[5], b[4]])
    } else {
        return ShaderResult { file: p.display().to_string(), issues: vec![ShaderIssue { kind: "spv-magic".into(), line: 0, msg: format!("bad magic 0x{:08x} — not a SPIR-V module", le), level: "error".into() }] };
    };
    let (major, minor) = (ver[2], ver[1]);
    ShaderResult { file: p.display().to_string(), issues: vec![ShaderIssue { kind: "spv-ok".into(), line: 0, msg: format!("valid SPIR-V {}.{} ({}-endian, {} bytes)", major, minor, endian, b.len()), level: "info".into() }] }
}

fn check_shader(p: &Path) -> ShaderResult {
    if p.extension().and_then(|e| e.to_str()) == Some("spv") { return check_spv(p); }
    match std::fs::read_to_string(p) {
        Ok(src) => check_glsl(p, &src),
        Err(e) => ShaderResult { file: p.display().to_string(), issues: vec![ShaderIssue { kind: "read".into(), line: 0, msg: e.to_string(), level: "error".into() }] },
    }
}

// ───────── project model + cargo ─────────

fn toml_min(path: &Path) -> (Option<String>, bool) {
    // returns (package_name, is_workspace)
    let txt = std::fs::read_to_string(path).unwrap_or_default();
    let mut name = None; let mut ws = false; let mut sect = "";
    for line in txt.lines() {
        let t = line.trim();
        if t.starts_with("[workspace") { ws = true; sect = "workspace"; continue; }
        if t.starts_with("[package") { sect = "package"; continue; }
        if t.starts_with('[') { sect = ""; continue; }
        if sect == "package" && t.starts_with("name") {
            if let Some((_, v)) = t.split_once('=') { name = Some(v.trim().trim_matches('"').to_string()); }
        }
    }
    (name, ws)
}

fn nearest_manifest(start: &Path) -> Option<PathBuf> {
    let mut d = if start.is_dir() { Some(start.to_path_buf()) } else { start.parent().map(|p| p.to_path_buf()) };
    while let Some(dir) = d {
        let m = dir.join("Cargo.toml");
        if m.exists() { return Some(m); }
        d = dir.parent().map(|p| p.to_path_buf());
    }
    None
}

fn find_project(start: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    let m = match nearest_manifest(start) { Some(m) => m, None => return (None, None) };
    let (name, ws) = toml_min(&m);
    if ws && name.is_none() { return (Some(m), None); }
    let mut wsroot = None;
    let mut d = m.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf());
    while let Some(dir) = d {
        let cand = dir.join("Cargo.toml");
        if cand.exists() { let (_, isws) = toml_min(&cand); if isws { wsroot = Some(cand); break; } }
        d = dir.parent().map(|p| p.to_path_buf());
    }
    (wsroot, Some(m))
}

fn owning_package(file: &Path) -> Option<(PathBuf, String)> {
    let mut d = if file.is_dir() { Some(file.to_path_buf()) } else { file.parent().map(|p| p.to_path_buf()) };
    while let Some(dir) = d {
        let m = dir.join("Cargo.toml");
        if m.exists() { if let (Some(name), _) = toml_min(&m) { return Some((m, name)); } }
        d = dir.parent().map(|p| p.to_path_buf());
    }
    None
}

struct Diag { level: String, code: String, loc: String, msg: String, rendered: Option<String> }

struct CargoResult {
    timed_out: bool, secs: f64, build_failed: bool,
    diagnostics: Vec<Diag>, dep_failures: Vec<Diag>, raw_stderr: String,
    tool: String, clippy_unavailable: bool, packages: Vec<String>,
}

fn which(cmd: &str) -> bool {
    if let Ok(path) = std::env::var("PATH") {
        for d in path.split(':') { if Path::new(d).join(cmd).exists() { return true; } }
    }
    false
}

fn run_cargo(root: &Path, packages: &BTreeSet<String>, a: &Args) -> CargoResult {
    let use_clippy = a.clippy && (which("cargo-clippy") || which("clippy-driver"));
    let sub = if use_clippy { "clippy" } else { "check" };
    let mut cmd = Command::new("cargo");
    cmd.arg(sub).arg("--message-format=json").arg("--color=never");
    if a.no_default_features { cmd.arg("--no-default-features"); }
    if let Some(f) = &a.features { cmd.arg("--features").arg(f); }
    if a.all_targets { cmd.arg("--all-targets"); }
    if a.tests { cmd.arg("--tests"); }
    if a.offline { cmd.arg("--offline"); }
    for p in packages { cmd.arg("-p").arg(p); }
    cmd.current_dir(root).env("CARGO_TERM_COLOR", "never")
        .stdout(Stdio::piped()).stderr(Stdio::piped());

    let t0 = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return CargoResult { timed_out: false, secs: 0.0, build_failed: true, diagnostics: vec![], dep_failures: vec![], raw_stderr: "cargo not found".into(), tool: sub.into(), clippy_unavailable: a.clippy && !use_clippy, packages: packages.iter().cloned().collect() },
    };
    let deadline = Duration::from_secs(a.timeout);
    let timed_out = loop {
        match child.try_wait() {
            Ok(Some(_)) => break false,
            Ok(None) => { if t0.elapsed() > deadline { let _ = child.kill(); break true; } std::thread::sleep(Duration::from_millis(50)); }
            Err(_) => break false,
        }
    };
    let out = child.wait_with_output().unwrap();
    let secs = (t0.elapsed().as_millis() as f64) / 1000.0;
    if timed_out {
        return CargoResult { timed_out: true, secs: a.timeout as f64, build_failed: false, diagnostics: vec![], dep_failures: vec![], raw_stderr: String::new(), tool: sub.into(), clippy_unavailable: a.clippy && !use_clippy, packages: packages.iter().cloned().collect() };
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let (diagnostics, dep_failures) = parse_cargo_json(&stdout, root, a.full);
    let flat: String = stdout.chars().filter(|c| !c.is_whitespace()).collect();
    let build_ok = flat.contains("\"reason\":\"build-finished\",\"success\":true");
    let build_failed = flat.contains("\"reason\":\"build-finished\",\"success\":false")
        || (out.status.code().unwrap_or(1) != 0 && !build_ok);
    CargoResult {
        timed_out: false, secs, build_failed, diagnostics, dep_failures,
        raw_stderr: stderr_signal(&stderr), tool: sub.into(),
        clippy_unavailable: a.clippy && !use_clippy,
        packages: packages.iter().cloned().collect(),
    }
}

fn stderr_signal(stderr: &str) -> String {
    const PRIO: &[&str] = &["system library", "was not found", "could not find", "cannot find",
        "failed to run custom build", "linker", "undefined reference", "is not installed", "no such file", "required by"];
    let mut seen = BTreeSet::new(); let mut out = vec![];
    let lines: Vec<String> = stderr.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    for l in lines.iter().filter(|l| { let lo = l.to_lowercase(); PRIO.iter().any(|k| lo.contains(k)) }) {
        if seen.insert(l.clone()) { out.push(l.clone()); }
    }
    for l in lines.iter().filter(|l| l.to_lowercase().starts_with("error")) {
        if seen.insert(l.clone()) { out.push(l.clone()); }
    }
    out.into_iter().take(8).collect::<Vec<_>>().join("\n")
}

fn is_rollup(text: &str, has_span: bool, code: &str) -> bool {
    if has_span || !code.is_empty() { return false; }
    let t = text.to_lowercase();
    t.starts_with("aborting due to") || t.contains("emitted") || t.starts_with("generated ")
        || t.contains("previous error") || t.contains("for more information about")
}

fn parse_cargo_json(stdout: &str, root: &Path, full: bool) -> (Vec<Diag>, Vec<Diag>) {
    let roots = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let roots = roots.to_string_lossy().to_string();
    let mut seen = BTreeSet::new();
    let mut diags = vec![]; let mut deps = vec![];
    for line in stdout.lines() {
        let line = line.trim();
        if !line.starts_with('{') { continue; }
        let obj: Value = match serde_json::from_str(line) { Ok(v) => v, Err(_) => continue };
        if obj.get("reason").and_then(|v| v.as_str()) != Some("compiler-message") { continue; }
        let msg = match obj.get("message") { Some(m) => m, None => continue };
        let level = msg.get("level").and_then(|v| v.as_str()).unwrap_or("");
        if level != "error" && level != "warning" { continue; }
        let code = msg.get("code").and_then(|c| c.get("code")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let text = msg.get("message").and_then(|v| v.as_str()).unwrap_or("").lines().next().unwrap_or("").to_string();
        let spans = msg.get("spans").and_then(|v| v.as_array());
        let prim = spans.and_then(|arr| arr.iter().find(|s| s.get("is_primary").and_then(|v| v.as_bool()).unwrap_or(false)).or_else(|| arr.first()));
        if is_rollup(&text, prim.is_some(), &code) { continue; }
        let (loc, is_dep) = if let Some(sp) = prim {
            let fname = sp.get("file_name").and_then(|v| v.as_str()).unwrap_or("");
            let absf = if Path::new(fname).is_absolute() { fname.to_string() } else { format!("{}/{}", roots, fname) };
            let rel = pathdiff(&absf, &roots);
            let l = sp.get("line_start").and_then(|v| v.as_u64()).unwrap_or(0);
            let c = sp.get("column_start").and_then(|v| v.as_u64()).unwrap_or(0);
            (format!("{}:{}:{}", rel, l, c), absf.contains("/registry/") || absf.contains("/.cargo/"))
        } else { ("(no span)".to_string(), false) };
        let key = format!("{}|{}|{}|{}", level, code, loc, text);
        if !seen.insert(key) { continue; }
        let rendered = if full { msg.get("rendered").and_then(|v| v.as_str()).map(|s| s.to_string()) } else { None };
        let d = Diag { level: level.into(), code, loc, msg: text, rendered };
        if is_dep { deps.push(d); } else { diags.push(d); }
    }
    (diags, deps)
}

fn pathdiff(abs: &str, root: &str) -> String {
    if let Some(rest) = abs.strip_prefix(root) { rest.trim_start_matches('/').to_string() } else { abs.to_string() }
}

fn rustc_single(p: &Path, edition: &str, full: bool) -> Vec<Diag> {
    let out = Command::new("rustc")
        .args(["--edition", edition, "--emit=metadata", "--crate-type", "lib", "-o", "/tmp/rustcheck.rmeta", "--error-format=json", "--color=never"])
        .arg(p).output();
    let out = match out { Ok(o) => o, Err(_) => return vec![] };
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut seen = BTreeSet::new(); let mut ds = vec![];
    for line in stderr.lines() {
        let line = line.trim();
        if !line.starts_with('{') { continue; }
        let msg: Value = match serde_json::from_str(line) { Ok(v) => v, Err(_) => continue };
        let level = msg.get("level").and_then(|v| v.as_str()).unwrap_or("");
        if level != "error" && level != "warning" { continue; }
        let code = msg.get("code").and_then(|c| c.get("code")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let text = msg.get("message").and_then(|v| v.as_str()).unwrap_or("").lines().next().unwrap_or("").to_string();
        let spans = msg.get("spans").and_then(|v| v.as_array());
        let prim = spans.and_then(|arr| arr.iter().find(|s| s.get("is_primary").and_then(|v| v.as_bool()).unwrap_or(false)).or_else(|| arr.first()));
        if is_rollup(&text, prim.is_some(), &code) { continue; }
        let loc = if let Some(sp) = prim {
            format!("{}:{}:{}", sp.get("file_name").and_then(|v| v.as_str()).unwrap_or(""),
                sp.get("line_start").and_then(|v| v.as_u64()).unwrap_or(0),
                sp.get("column_start").and_then(|v| v.as_u64()).unwrap_or(0))
        } else { "(no span)".into() };
        let key = format!("{}|{}|{}|{}", level, code, loc, text);
        if !seen.insert(key) { continue; }
        let rendered = if full { msg.get("rendered").and_then(|v| v.as_str()).map(|s| s.to_string()) } else { None };
        ds.push(Diag { level: level.into(), code, loc, msg: text, rendered });
    }
    ds
}

fn git_changed(start: &str) -> Vec<String> {
    let root = Command::new("git").args(["-C", start, "rev-parse", "--show-toplevel"]).output();
    let gr = match root { Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(), _ => return vec![] };
    let mut set = BTreeSet::new();
    for a in [vec!["diff", "--name-only", "HEAD"], vec!["diff", "--name-only", "--cached"], vec!["ls-files", "--others", "--exclude-standard"]] {
        let mut args = vec!["-C", &gr]; args.extend(a);
        if let Ok(o) = Command::new("git").args(&args).output() {
            for f in String::from_utf8_lossy(&o.stdout).lines() {
                let f = f.trim(); if !f.is_empty() { set.insert(format!("{}/{}", gr, f)); }
            }
        }
    }
    set.into_iter().collect()
}

// ───────── driver + render ─────────

fn gather(paths: &[String]) -> Vec<PathBuf> {
    let mut out = vec![]; let mut seen = BTreeSet::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() { if !p.to_string_lossy().contains("/target/") { walk(&p, out); } }
                else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    if ext == "rs" || SHADER_EXTS.contains(&ext) { out.push(p); }
                }
            }
        }
    }
    for raw in paths {
        let p = Path::new(raw);
        if p.is_dir() { walk(p, &mut out); }
        else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if ext == "rs" || SHADER_EXTS.contains(&ext) { out.push(p.to_path_buf()); }
        }
    }
    out.into_iter().filter(|p| { let c = p.canonicalize().unwrap_or_else(|_| p.clone()); seen.insert(c) }).collect()
}

fn ensure_toolchain() {
    if which("cargo") { return; }
    let home = std::env::var("HOME").unwrap_or_default();
    for d in [format!("{}/.local/bin", home), format!("{}/.cargo/bin", home), "/usr/local/bin".into(), "/root/.local/bin".into()] {
        if Path::new(&d).join("cargo").exists() {
            let path = std::env::var("PATH").unwrap_or_default();
            std::env::set_var("PATH", format!("{}:{}", d, path));
            return;
        }
    }
}

fn ext_of(p: &Path) -> &str { p.extension().and_then(|e| e.to_str()).unwrap_or("") }

enum Sem { None, Cargo(CargoResult), Single(Vec<(String, Vec<Diag>)>) }

fn main() {
    ensure_toolchain();
    let mut a = parse_args();

    let targets: Vec<PathBuf> = if a.git {
        let changed = git_changed(a.paths.first().map(|s| s.as_str()).unwrap_or("."));
        let t: Vec<PathBuf> = changed.iter().map(PathBuf::from)
            .filter(|p| ext_of(p) == "rs" || SHADER_EXTS.contains(&ext_of(p))).collect();
        let rs: Vec<String> = t.iter().filter(|p| ext_of(p) == "rs").map(|p| p.display().to_string()).collect();
        a.changed = if rs.is_empty() { None } else { Some(rs) };
        t
    } else if let Some(ch) = &a.changed {
        ch.iter().map(PathBuf::from).collect()
    } else {
        gather(&a.paths)
    };

    let rs_files: Vec<PathBuf> = targets.iter().filter(|p| ext_of(p) == "rs" && !a.shaders).cloned().collect();
    let sh_files: Vec<PathBuf> = targets.iter().filter(|p| SHADER_EXTS.contains(&ext_of(p)) || (a.shaders && ext_of(p) == "rs")).cloned().collect();

    let static_results: Vec<StaticResult> = rs_files.iter().map(|p| static_check(p)).filter(|r| !r.issues.is_empty()).collect();
    let static_hard = static_results.iter().any(|r| r.hard);
    let shader_results: Vec<ShaderResult> = sh_files.iter().map(|p| check_shader(p)).collect();

    let mut run_semantic = !a.fast && !rs_files.is_empty();
    let mut skip_reason = if a.fast { "fast" } else { "" };
    if static_hard { run_semantic = false; skip_reason = "hard"; }

    let mut sem = Sem::None;

    if run_semantic {
        let anchor = a.changed.as_ref().and_then(|c| c.first()).cloned()
            .unwrap_or_else(|| rs_files.first().map(|p| p.display().to_string()).unwrap_or_else(|| a.paths[0].clone()));
        let anchor_abs = Path::new(&anchor).canonicalize().unwrap_or_else(|_| PathBuf::from(&anchor));
        let (ws, pkg) = find_project(&anchor_abs);
        if ws.is_none() && pkg.is_none() {
            let mut files = vec![];
            for p in &rs_files { files.push((p.display().to_string(), rustc_single(p, &a.edition, a.full))); }
            sem = Sem::Single(files);
        } else {
            let root = ws.clone().or(pkg.clone()).unwrap().parent().unwrap().to_path_buf();
            let mut packages = BTreeSet::new();
            if let Some(ch) = &a.changed {
                for f in ch { if let Some((_, name)) = owning_package(&Path::new(f).canonicalize().unwrap_or_else(|_| PathBuf::from(f))) { packages.insert(name); } }
            } else if let Some(pk) = &pkg { if let (Some(name), _) = toml_min(pk) { packages.insert(name); } }
            sem = Sem::Cargo(run_cargo(&root, &packages, &a));
        }
    }

    // ── tally ──
    let n_static: usize = static_results.iter().map(|r| r.issues.len()).sum();
    let hard_static = static_results.iter().any(|r| r.issues.iter().any(|i| i.kind != "missing-mod-file"));
    let has_missing_mod = static_results.iter().any(|r| r.issues.iter().any(|i| i.kind == "missing-mod-file"));
    let sh_err: usize = shader_results.iter().flat_map(|r| &r.issues).filter(|i| i.level == "error").count();
    let sh_warn: usize = shader_results.iter().flat_map(|r| &r.issues).filter(|i| i.level == "warning").count();

    let (mut code_errors, mut warnings, mut dep_failures) = (0usize, 0usize, 0usize);
    let (mut timed_out, mut blocked) = (false, false);
    match &sem {
        Sem::Cargo(c) => {
            code_errors = c.diagnostics.iter().filter(|d| d.level == "error").count();
            warnings = c.diagnostics.iter().filter(|d| d.level == "warning").count();
            dep_failures = c.dep_failures.len();
            timed_out = c.timed_out;
            blocked = c.build_failed && code_errors == 0 && !timed_out;
        }
        Sem::Single(files) => {
            for (_, ds) in files { code_errors += ds.iter().filter(|d| d.level == "error").count(); warnings += ds.iter().filter(|d| d.level == "warning").count(); }
        }
        Sem::None => {}
    }

    // ── render ──
    let color = std::io::stdout().is_terminal();
    let cc = |s: &str, code: &str| if color { format!("{}{}\x1b[0m", code, s) } else { s.to_string() };
    let (green, red, yel, dim, bold) = ("\x1b[32m", "\x1b[31m", "\x1b[33m", "\x1b[2m", "\x1b[1m");

    if a.json {
        print_json(&static_results, &shader_results, &sem, skip_reason, rs_files.len(), sh_files.len());
    } else {
        if !rs_files.is_empty() {
            if static_results.is_empty() { println!("{}", cc("● static  OK", green)); }
            else {
                println!("{}", cc("● static", bold));
                for r in &static_results {
                    for it in &r.issues {
                        let (tag, col) = if it.kind == "missing-mod-file" { ("WARN", yel) } else { ("FAIL", red) };
                        println!("  {} {}:{}:{}  {}  {}", cc(tag, col), r.file, it.line, it.col, cc(&it.kind, dim), it.msg);
                    }
                }
            }
        }
        if !sh_files.is_empty() {
            let problem: Vec<&ShaderResult> = shader_results.iter().filter(|r| r.issues.iter().any(|i| i.level == "error" || i.level == "warning")).collect();
            if problem.is_empty() {
                println!("{}", cc(&format!("● shaders  OK ({} file{})", sh_files.len(), if sh_files.len() != 1 { "s" } else { "" }), green));
            } else {
                println!("{}", cc("● shaders", bold));
                for r in &problem {
                    for it in &r.issues {
                        if it.level == "info" { continue; }
                        let (tag, col) = if it.level == "error" { ("FAIL", red) } else { ("WARN", yel) };
                        let where_ = if it.line > 0 { format!(":{}", it.line) } else { String::new() };
                        println!("  {} {}{}  {}  {}", cc(tag, col), r.file, where_, cc(&it.kind, dim), it.msg);
                    }
                }
                let ok = sh_files.len() - problem.len();
                if ok > 0 { println!("{}", cc(&format!("  {} more shader file(s) OK", ok), dim)); }
            }
        }
        match &sem {
            Sem::None => {
                if skip_reason == "hard" { println!("{}", cc("● semantic  SKIPPED — fix the structural break above first", yel)); }
                else if skip_reason == "fast" { println!("{}", cc("● semantic  SKIPPED (--fast)", dim)); }
            }
            Sem::Single(files) => {
                println!("{}", cc("● semantic  (single-file rustc — no Cargo project)", bold));
                for (f, ds) in files {
                    if ds.is_empty() { println!("  {}   {}", cc("OK", green), f); }
                    for d in ds { print_diag(&cc, d, red, yel); }
                }
            }
            Sem::Cargo(c) => {
                if c.clippy_unavailable { println!("{}", cc("  note: clippy not installed — used `cargo check` instead", dim)); }
                let plist: String = c.packages.iter().map(|p| format!("-p {}", p)).collect::<Vec<_>>().join(" ");
                let head = format!("● semantic  cargo {} {}", c.tool, plist);
                println!("{}{}", cc(head.trim_end(), bold), cc(&format!("   ({}s)", round2(c.secs)), dim));
                if c.timed_out { println!("  {} exceeded {}s — narrow scope with --changed or raise --timeout", cc("TIMEOUT", red), c.secs as u64); }
                for d in &c.diagnostics { print_diag(&cc, d, red, yel); }
                if c.diagnostics.is_empty() && c.dep_failures.is_empty() && !c.timed_out {
                    println!("  {}   no errors or warnings", cc("OK", green));
                }
                if !c.dep_failures.is_empty() || (!c.raw_stderr.is_empty() && c.diagnostics.is_empty()) {
                    println!("{}", cc("  ── dependency / environment (NOT your code):", dim));
                    for d in c.dep_failures.iter().take(6) { println!("     {} {}  {}", cc("dep", yel), d.loc, d.msg); }
                    for ln in c.raw_stderr.lines().take(4) { println!("     {} {}", cc("env", yel), ln); }
                }
            }
        }

        let fail = code_errors > 0 || n_static > 0 || sh_err > 0 || (a.deny_warnings && (warnings > 0 || sh_warn > 0));
        let (verdict, vcol) = if fail { ("FAIL", red) } else if timed_out || blocked { ("BLOCKED", yel) } else { ("PASS", green) };
        let mut bits = vec![format!("{} static", n_static), format!("{} errors", code_errors), format!("{} warnings", warnings)];
        if !sh_files.is_empty() { bits.push(format!("{} shader-err", sh_err)); bits.push(format!("{} shader-warn", sh_warn)); }
        if dep_failures > 0 || blocked { bits.push(format!("{} dep/env", dep_failures)); }
        let note = if verdict == "BLOCKED" { cc("  (could not finish — env/deps, not necessarily your code)", dim) } else { String::new() };
        println!("\n{}  {}{}", cc(verdict, vcol), bits.join("  ·  "), note);
    }

    // ── exit code ──
    let _ = (hard_static, has_missing_mod);
    let mut fail = code_errors > 0 || hard_static || sh_err > 0 || (n_static > 0 && matches!(sem, Sem::None));
    if has_missing_mod && (matches!(sem, Sem::None) || blocked || timed_out) { fail = true; }
    if a.deny_warnings && (warnings > 0 || sh_warn > 0) { fail = true; }
    if fail { std::process::exit(1); }
    if timed_out || blocked { std::process::exit(2); }
    std::process::exit(0);
}

fn round2(x: f64) -> f64 { (x * 100.0).round() / 100.0 }

fn print_diag(cc: &dyn Fn(&str, &str) -> String, d: &Diag, red: &str, yel: &str) {
    let col = if d.level == "error" { red } else { yel };
    let code = if d.code.is_empty() { String::new() } else { format!("[{}]", d.code) };
    println!("  {}{} {}  {}", cc(&d.level, col), code, d.loc, d.msg);
    if let Some(r) = &d.rendered { for ln in r.trim_end().lines() { println!("      {}", ln); } }
}

fn diag_json(d: &Diag) -> Value {
    use serde_json::json;
    let mut o = json!({"level": d.level, "code": d.code, "loc": d.loc, "msg": d.msg});
    if let Some(r) = &d.rendered { o["rendered"] = Value::String(r.clone()); }
    o
}

fn sem_json(sem: &Sem) -> Value {
    use serde_json::json;
    match sem {
        Sem::None => Value::Null,
        Sem::Single(files) => json!({
            "mode": "single-file",
            "files": files.iter().map(|(f, ds)| json!({
                "file": f, "diagnostics": ds.iter().map(diag_json).collect::<Vec<_>>()
            })).collect::<Vec<_>>()
        }),
        Sem::Cargo(c) => json!({
            "mode": "cargo", "tool": c.tool, "packages": c.packages,
            "secs": round2(c.secs), "timed_out": c.timed_out, "build_failed": c.build_failed,
            "diagnostics": c.diagnostics.iter().map(diag_json).collect::<Vec<_>>(),
            "dep_failures": c.dep_failures.iter().map(diag_json).collect::<Vec<_>>(),
            "raw_stderr": c.raw_stderr,
        }),
    }
}

fn print_json(static_results: &[StaticResult], shaders: &[ShaderResult], sem: &Sem, skip: &str, n_rs: usize, n_sh: usize) {
    use serde_json::json;
    let st: Vec<Value> = static_results.iter().map(|r| json!({
        "file": r.file, "hard": r.hard,
        "issues": r.issues.iter().map(|i| json!({"kind": i.kind, "line": i.line, "col": i.col, "msg": i.msg})).collect::<Vec<_>>()
    })).collect();
    let sh: Vec<Value> = shaders.iter().map(|r| json!({
        "file": r.file,
        "issues": r.issues.iter().map(|i| json!({"kind": i.kind, "line": i.line, "msg": i.msg, "level": i.level})).collect::<Vec<_>>()
    })).collect();
    let out = json!({"static": st, "shaders": sh, "semantic": sem_json(sem),
                     "skip_reason": skip, "counts": {"rs": n_rs, "sh": n_sh}});
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
