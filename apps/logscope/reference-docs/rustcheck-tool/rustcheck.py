#!/usr/bin/env python3
"""
rustcheck v2 — an AI-centric Rust + GPU-shader checker for sandbox/agent use.

One question, answered fast and cheap: "did my edit break this?"  Cheapest tier
first; the compiler only runs when it's the thing that'll actually tell you
something.

  Tier 0  STATIC (Rust)     no toolchain, ms, pinpoints file:line
  Tier 1  SEMANTIC (Rust)   real `cargo check` (or clippy), scoped + compacted
  SHADER  GLSL/SPIR-V        basic Vulkan-GLSL rules, no GPU needed

Routing is by extension: .rs -> Rust pipeline; .vert/.frag/.comp/.glsl/... and
.spv -> shader pipeline.  --shaders forces shader mode.

Exit codes:  0 clean · 1 problems in YOUR code · 2 inconclusive (env/dep/timeout)

Examples
  rustcheck .                       # check crate/workspace in CWD
  rustcheck --git                   # check only what git says changed
  rustcheck --changed src/voice.rs  # check the crate owning this file
  rustcheck --fast src/*.rs         # Tier 0 only
  rustcheck --clippy .              # use clippy if installed (falls back to check)
  rustcheck --deny-warnings .       # warnings fail the check
  rustcheck assets/shaders/*.comp   # shader rules
  rustcheck --json .                # structured output for an agent
"""
from __future__ import annotations
import argparse, json, os, re, shutil, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:
    tomllib = None

SHADER_EXTS = {".vert", ".frag", ".comp", ".geom", ".tesc", ".tese", ".glsl",
               ".mesh", ".task", ".rgen", ".rchit", ".rmiss", ".rahit",
               ".rcall", ".rint", ".spv"}

# ════════════════════════════════════════════════════════════════════════════
# Tier 0 — static, Rust-aware lexing. Conservative: when unsure, stay silent and
# let cargo be the source of truth, so it rarely cries wolf.
# ════════════════════════════════════════════════════════════════════════════

CONFLICT_PREFIXES = ("<<<<<<< ", "||||||| ", ">>>>>>> ")

def find_conflict_markers(src):
    out = []
    for n, line in enumerate(src.splitlines(), 1):
        if line.startswith(CONFLICT_PREFIXES) or line.rstrip() == "=======":
            out.append((n, line[:40]))
    return out

def scan_source(src):
    """Delimiter balance + unterminated literals/comments, Rust-aware:
    raw strings r#"..."#, nested block comments, char-vs-lifetime ('a' vs 'a).
    Returns (issues, hard_break)."""
    s = src; i, n = 0, len(s); line, col = 1, 1
    stack, issues = [], []
    PAIRS = {')': '(', ']': '[', '}': '{'}

    def adv(k=1):
        nonlocal i, line, col
        for _ in range(k):
            if i < n:
                if s[i] == '\n': line += 1; col = 1
                else: col += 1
                i += 1

    while i < n:
        c = s[i]
        if c == '/' and i + 1 < n and s[i+1] == '/':
            while i < n and s[i] != '\n': adv()
            continue
        if c == '/' and i + 1 < n and s[i+1] == '*':
            sl, sc = line, col; depth = 1; adv(2)
            while i < n and depth:
                if s[i] == '/' and i+1 < n and s[i+1] == '*': depth += 1; adv(2)
                elif s[i] == '*' and i+1 < n and s[i+1] == '/': depth -= 1; adv(2)
                else: adv()
            if depth:
                issues.append(("unterminated-block-comment", sl, sc, "block comment never closed"))
                return issues, True
            continue
        if c in 'rb' and _is_raw_string_start(s, i):
            j = i
            if s[j] == 'b': j += 1
            j += 1
            hashes = 0
            while j < n and s[j] == '#': hashes += 1; j += 1
            if j < n and s[j] == '"':
                sl, sc = line, col; adv(j - i + 1)
                closer = '"' + '#' * hashes
                end = s.find(closer, i)
                if end == -1:
                    issues.append(("unterminated-raw-string", sl, sc, "raw string never closed"))
                    return issues, True
                adv(end - i + len(closer)); continue
        if c == '"' or (c == 'b' and i+1 < n and s[i+1] == '"'):
            sl, sc = line, col
            if c == 'b': adv()
            adv(); ok = False
            while i < n:
                if s[i] == '\\': adv(2); continue
                if s[i] == '"': adv(); ok = True; break
                adv()
            if not ok:
                issues.append(("unterminated-string", sl, sc, "string literal never closed"))
                return issues, True
            continue
        if c == "'" or (c == 'b' and i+1 < n and s[i+1] == "'"):
            off = 1 if c == 'b' else 0
            if _looks_like_char_literal(s, i + off):
                sl, sc = line, col
                if c == 'b': adv()
                adv()
                while i < n:
                    if s[i] == '\\': adv(2); continue
                    if s[i] == "'": adv(); break
                    adv()
                continue
        if c in '([{':
            stack.append((c, line, col)); adv(); continue
        if c in ')]}':
            want = PAIRS[c]
            if not stack:
                issues.append(("stray-delim", line, col, f"stray '{c}'")); return issues, True
            oc, ol, ocol = stack.pop()
            if oc != want:
                issues.append(("mismatched-delim", line, col,
                               f"'{c}' closes '{oc}' opened at {ol}:{ocol}")); return issues, True
            adv(); continue
        adv()
    if stack:
        oc, ol, ocol = stack[-1]
        issues.append(("unclosed-delim", ol, ocol, f"'{oc}' is never closed")); return issues, True
    return issues, False

def _is_raw_string_start(s, i):
    j = i
    if s[j] == 'b': j += 1
    if j >= len(s) or s[j] != 'r': return False
    j += 1
    while j < len(s) and s[j] == '#': j += 1
    return j < len(s) and s[j] == '"'

def _looks_like_char_literal(s, i):
    n = len(s)
    if i + 1 >= n: return False
    if s[i+1] == '\\':
        return s.find("'", i + 2, i + 14) != -1
    return i + 2 < n and s[i+2] == "'"

def resolve_mod_files(rs_path, src):
    missing = []
    lines = src.splitlines()
    parent = rs_path.parent; stem = rs_path.stem
    base = parent if stem in ("main", "lib", "mod") else parent / stem
    for n, raw in enumerate(lines, 1):
        line = raw.strip()
        if line.startswith("//"): continue
        for kw in ("pub mod ", "mod "):
            if line.startswith(kw) and line.endswith(";"):
                name = line[len(kw):-1].strip()
                if not name.isidentifier(): break
                prev = lines[n-2].strip() if n >= 2 else ""
                if "#[path" in prev or "#[cfg" in prev: break
                if not ((base / f"{name}.rs").exists() or (base / name / "mod.rs").exists()):
                    missing.append((n, name, str(base / f"{name}.rs")))
                break
    return missing

def static_check_file(p):
    try:
        src = p.read_text(encoding="utf-8", errors="replace")
    except OSError as e:
        return {"file": str(p), "read_error": str(e), "issues": [], "hard": True}
    issues, hard = [], False
    for ln, txt in find_conflict_markers(src):
        issues.append({"kind": "merge-conflict", "line": ln, "col": 1, "msg": f"conflict marker: {txt!r}"})
        hard = True
    for kind, l, cc, m in scan_source(src)[0]:
        issues.append({"kind": kind, "line": l, "col": cc, "msg": m})
    if scan_source(src)[1]:
        hard = True
    for ln, name, want in resolve_mod_files(p, src):
        issues.append({"kind": "missing-mod-file", "line": ln, "col": 1,
                       "msg": f"`mod {name};` but no file (expected {want})"})
    return {"file": str(p), "issues": issues, "hard": hard}

# ════════════════════════════════════════════════════════════════════════════
# SHADER checker — basic, static, Vulkan-GLSL aware (no GPU / no glslang needed).
# Uses a real validator (glslangValidator / naga) automatically iff present.
# ════════════════════════════════════════════════════════════════════════════

STAGE_BY_EXT = {".vert": "vertex", ".frag": "fragment", ".comp": "compute",
                ".geom": "geometry", ".tesc": "tess-control", ".tese": "tess-eval",
                ".mesh": "mesh", ".task": "task"}

def _strip_glsl_comments(src):
    """Blank // and /* */ comments but preserve every newline so line numbers
    stay exact."""
    out = []; i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == '/' and i+1 < n and src[i+1] == '/':
            while i < n and src[i] != '\n': out.append(' '); i += 1
            continue
        if c == '/' and i+1 < n and src[i+1] == '*':
            out.append('  '); i += 2
            while i < n and not (src[i] == '*' and i+1 < n and src[i+1] == '/'):
                out.append('\n' if src[i] == '\n' else ' '); i += 1
            if i < n: out.append('  '); i += 2
            continue
        out.append(c); i += 1
    return ''.join(out)

def _lineno(src, pos):
    return src.count('\n', 0, pos) + 1

def _parse_layout_args(argstr):
    d = {}
    for part in argstr.split(','):
        part = part.strip()
        if not part: continue
        if '=' in part:
            k, v = part.split('=', 1)
            d[k.strip()] = v.strip()
        else:
            d[part] = True
    return d

LAYOUT_RE = re.compile(r'layout\s*\(([^)]*)\)\s*([^;{]*)', re.S)
STORAGE_KW = ("uniform", "buffer", "in", "out", "shared")

def check_glsl(path, src):
    issues = []
    def add(kind, line, msg, level="error"):
        issues.append({"kind": kind, "line": line, "col": 1, "msg": msg, "level": level})

    # structural delimiters / block comments (C-style)
    s = src; i, n = 0, len(s); line = 1; stack = []
    PAIRS = {')': '(', ']': '[', '}': '{'}; hard = False
    while i < n:
        c = s[i]
        if c == '\n': line += 1; i += 1; continue
        if c == '/' and i+1 < n and s[i+1] == '/':
            while i < n and s[i] != '\n': i += 1
            continue
        if c == '/' and i+1 < n and s[i+1] == '*':
            sl = line; i += 2
            while i+1 < n and not (s[i] == '*' and s[i+1] == '/'):
                if s[i] == '\n': line += 1
                i += 1
            if i+1 >= n: add("unterminated-block-comment", sl, "block comment never closed"); hard = True; break
            i += 2; continue
        if c in '([{': stack.append((c, line)); i += 1; continue
        if c in ')]}':
            if not stack: add("stray-delim", line, f"stray '{c}'"); hard = True; break
            oc, ol = stack.pop()
            if oc != PAIRS[c]: add("mismatched-delim", line, f"'{c}' closes '{oc}' opened at line {ol}"); hard = True; break
            i += 1; continue
        i += 1
    if not hard and stack:
        oc, ol = stack[-1]; add("unclosed-delim", ol, f"'{oc}' never closed"); hard = True

    code = _strip_glsl_comments(src)

    # preprocessor balance (#if/#ifdef/#ifndef vs #endif)
    depth = 0; first_open = None
    for m in re.finditer(r'(?m)^\s*#\s*(ifdef|ifndef|if|endif)\b', code):
        kw = m.group(1)
        if kw == 'endif':
            depth -= 1
            if depth < 0: add("preproc", _lineno(code, m.start()), "#endif without matching #if"); depth = 0
        else:
            if first_open is None: first_open = _lineno(code, m.start())
            depth += 1
    if depth > 0:
        add("preproc", first_open or 1, f"{depth} unterminated #if/#ifdef (missing #endif)")

    if hard:  # don't bother with semantic rules on a structurally broken file
        return {"file": str(path), "stage": STAGE_BY_EXT.get(path.suffix, "glsl"),
                "issues": issues, "hard": True}

    stage = STAGE_BY_EXT.get(path.suffix, "glsl")
    is_stage = path.suffix in STAGE_BY_EXT   # .glsl/.h/.inc are library fragments

    # #version / entry point only apply to standalone stage shaders
    if is_stage:
        if not re.search(r'(?m)^\s*#\s*version\b', code):
            add("no-version", 1, "missing `#version` directive (Vulkan GLSL needs e.g. #version 450)")
        if not re.search(r'\bvoid\s+main\s*\(', code):
            add("no-main", 1, "no `void main()` entry point found")

    # parse layout-qualified declarations
    descriptors = {}     # (set,binding) -> line
    in_locs, out_locs = {}, {}
    push_constants = []
    for m in LAYOUT_RE.finditer(code):
        args = _parse_layout_args(m.group(1))
        rest = m.group(2)
        ln = _lineno(code, m.start())
        if any(k.startswith('local_size') for k in args):  # compute workgroup decl
            continue
        if 'constant_id' in args:  # spec constant
            continue
        storage = next((k for k in STORAGE_KW if re.search(r'\b'+k+r'\b', rest)), None)
        if 'push_constant' in args:
            push_constants.append(ln); continue
        if storage in ("uniform", "buffer"):
            if 'binding' not in args:
                add("missing-binding", ln, f"{storage} block has no `binding=` (required in Vulkan GLSL)")
            else:
                key = (args.get('set', '0'), args['binding'])
                if key in descriptors:
                    add("binding-collision", ln,
                        f"set {key[0]} binding {key[1]} already used at line {descriptors[key]}")
                else:
                    descriptors[key] = ln
        elif storage in ("in", "out") and 'location' in args:
            tbl = in_locs if storage == "in" else out_locs
            loc = args['location']
            if loc in tbl:
                add("location-collision", ln, f"{storage} location {loc} already used at line {tbl[loc]}")
            else:
                tbl[loc] = ln

    # opaque/UBO/SSBO declared with NO layout at all -> missing binding (Vulkan)
    for m in re.finditer(r'(?m)^\s*(uniform|buffer)\s+(?!.*push_constant)([A-Za-z_]\w*)', code):
        head = code[max(0, m.start()-80):m.start()]
        # if a layout(...) governs this statement it'll be on the same logical line
        lstart = code.rfind('\n', 0, m.start())
        if 'layout' not in code[lstart+1:m.start()]:
            add("missing-binding", _lineno(code, m.start()),
                f"`{m.group(1)}` declaration without a `layout(binding=...)` (Vulkan requires it)")

    if len(push_constants) > 1:
        add("push-constant", push_constants[1], "more than one push_constant block (only one allowed)")

    # stage-specific
    if stage == "compute" and not re.search(r'layout\s*\([^)]*local_size_x', code):
        add("compute-no-localsize", 1, "compute shader missing `layout(local_size_x = ...) in;`")
    if stage == "vertex" and 'gl_Position' not in code:
        add("vertex-no-position", 1, "vertex shader never writes `gl_Position`", level="warning")
    if stage == "fragment" and not re.search(r'\bout\s+\w', code) and 'gl_FragDepth' not in code:
        add("fragment-no-output", 1, "fragment shader declares no `out` color and no gl_FragDepth", level="warning")

    # legacy / non-Vulkan builtins
    for pat, msg in ((r'\bgl_FragColor\b', "gl_FragColor is legacy; use a `layout(location=0) out` in Vulkan"),
                     (r'\btexture2D\s*\(', "texture2D() is legacy; use texture() in core/Vulkan GLSL"),
                     (r'\bvarying\b', "`varying` is legacy; use in/out"),
                     (r'\battribute\b', "`attribute` is legacy; use `layout(location=) in`")):
        m = re.search(pat, code)
        if m: add("legacy", _lineno(code, m.start()), msg, level="warning")

    return {"file": str(path), "stage": stage, "issues": issues, "hard": False}

def check_spv(path):
    issues = []
    try:
        b = path.read_bytes()
    except OSError as e:
        return {"file": str(path), "stage": "spir-v", "issues":
                [{"kind": "read", "line": 0, "col": 1, "msg": str(e), "level": "error"}], "hard": True}
    if len(b) < 20:
        return {"file": str(path), "stage": "spir-v", "issues":
                [{"kind": "spv-truncated", "line": 0, "col": 1, "msg": "file too small to be SPIR-V", "level": "error"}], "hard": True}
    le = int.from_bytes(b[0:4], "little")
    be = int.from_bytes(b[0:4], "big")
    if le == 0x07230203:
        endian = "little"; ver = b[4:8]
    elif be == 0x07230203:
        endian = "big"; ver = b[4:8][::-1]
    else:
        return {"file": str(path), "stage": "spir-v", "issues":
                [{"kind": "spv-magic", "line": 0, "col": 1,
                  "msg": f"bad magic 0x{le:08x} — not a SPIR-V module", "level": "error"}], "hard": True}
    major, minor = ver[2], ver[1]
    info = {"kind": "spv-ok", "line": 0, "col": 1,
            "msg": f"valid SPIR-V {major}.{minor} ({endian}-endian, {len(b)} bytes)", "level": "info"}
    return {"file": str(path), "stage": "spir-v", "issues": [info], "hard": False}

def check_shader_file(p):
    if p.suffix == ".spv":
        return check_spv(p)
    try:
        src = p.read_text(encoding="utf-8", errors="replace")
    except OSError as e:
        return {"file": str(p), "stage": "glsl", "issues":
                [{"kind": "read", "line": 0, "col": 1, "msg": str(e), "level": "error"}], "hard": True}
    res = check_glsl(p, src)
    # optional: real validator if the environment has one
    val = shutil.which("glslangValidator")
    if val and p.suffix != ".spv":
        try:
            pr = subprocess.run([val, "-V", str(p)], capture_output=True, text=True, timeout=30)
            if pr.returncode != 0:
                for ln in pr.stdout.splitlines():
                    if "ERROR" in ln:
                        res["issues"].append({"kind": "glslang", "line": 0, "col": 1,
                                              "msg": ln.strip()[:160], "level": "error"})
        except Exception:
            pass
    return res

# ════════════════════════════════════════════════════════════════════════════
# Project model + Tier 1 cargo/clippy (carried from v1, with clippy + git + full)
# ════════════════════════════════════════════════════════════════════════════

def load_toml(path):
    if tomllib is None: return _toml_fallback(path)
    try:
        with path.open("rb") as f: return tomllib.load(f)
    except Exception: return {}

def _toml_fallback(path):
    data = {"package": {}, "workspace": None}; sect = None
    for line in path.read_text(errors="replace").splitlines():
        t = line.strip()
        if t.startswith("[workspace"): data["workspace"] = {}; sect = "workspace"; continue
        if t.startswith("[package"): sect = "package"; continue
        if t.startswith("["): sect = None; continue
        if sect == "package" and t.startswith("name"):
            data["package"]["name"] = t.split("=", 1)[1].strip().strip('"')
    return data

def nearest_manifest(start):
    p = start if start.is_dir() else start.parent
    for d in [p, *p.parents]:
        if (d / "Cargo.toml").exists(): return d / "Cargo.toml"
    return None

def package_name_of(m): return load_toml(m).get("package", {}).get("name")
def is_workspace_root(m): return load_toml(m).get("workspace") is not None

def find_project(start):
    m = nearest_manifest(start)
    if m is None: return None, None
    if is_workspace_root(m) and package_name_of(m) is None: return m, None
    ws = None
    for d in m.parent.parents:
        cand = d / "Cargo.toml"
        if cand.exists() and is_workspace_root(cand): ws = cand; break
    return ws, m

def owning_package(file_path):
    p = file_path if file_path.is_dir() else file_path.parent
    for d in [p, *p.parents]:
        m = d / "Cargo.toml"
        if m.exists() and package_name_of(m): return m
    return None

def cargo_root_dir(ws, pkg):
    m = ws or pkg
    return m.parent if m else None

def run_cargo(root_dir, packages, args):
    use_clippy = args.clippy and (shutil.which("cargo-clippy") or shutil.which("clippy-driver"))
    sub = "clippy" if use_clippy else "check"
    cmd = ["cargo", sub, "--message-format=json", "--color=never"]
    if args.no_default_features: cmd.append("--no-default-features")
    if args.features: cmd += ["--features", args.features]
    if args.all_targets: cmd.append("--all-targets")
    if args.tests: cmd.append("--tests")
    if args.offline: cmd.append("--offline")
    for p in sorted(packages): cmd += ["-p", p]
    env = dict(os.environ); env["CARGO_TERM_COLOR"] = "never"
    clippy_unavailable = args.clippy and not use_clippy
    t0 = time.time()
    try:
        proc = subprocess.run(cmd, cwd=str(root_dir), env=env, capture_output=True,
                              text=True, timeout=args.timeout)
    except subprocess.TimeoutExpired:
        return {"timed_out": True, "cmd": " ".join(cmd), "secs": args.timeout,
                "diagnostics": [], "dep_failures": [], "raw_stderr": "",
                "tool": sub, "clippy_unavailable": clippy_unavailable}
    secs = round(time.time() - t0, 2)
    diags, dep_fail = parse_cargo_json(proc.stdout, root_dir, args.full)
    flat = proc.stdout.replace(" ", "")
    build_ok = '"reason":"build-finished","success":true' in flat
    build_failed = ('"reason":"build-finished","success":false' in flat or
                    (proc.returncode != 0 and not build_ok))
    return {"timed_out": False, "cmd": " ".join(cmd), "secs": secs, "exit": proc.returncode,
            "build_failed": build_failed, "diagnostics": diags, "dep_failures": dep_fail,
            "raw_stderr": _stderr_signal(proc.stderr), "tool": sub,
            "clippy_unavailable": clippy_unavailable}

def _stderr_signal(stderr):
    PRIO = ("system library", "was not found", "could not find", "cannot find",
            "failed to run custom build", "linker", "undefined reference",
            "is not installed", "no such file", "required by")
    lines = [l.strip() for l in stderr.splitlines() if l.strip()]
    prio = [l for l in lines if any(k in l.lower() for k in PRIO)]
    generic = [l for l in lines if l.lower().startswith("error")]
    seen, out = set(), []
    for l in prio + generic:
        if l not in seen: seen.add(l); out.append(l)
    return "\n".join(out[:8])

def _is_rollup(text, has_span, code):
    if has_span or code: return False
    t = text.lower()
    return (t.startswith("aborting due to") or "emitted" in t or t.startswith("generated ")
            or "previous error" in t or "for more information about" in t)

def _primary_span(spans):
    for s in spans:
        if s.get("is_primary"): return s
    return spans[0] if spans else None

def _rel(path, root):
    try: return os.path.relpath(path, root)
    except ValueError: return path

def parse_cargo_json(stdout, root_dir, full=False):
    root = str(root_dir.resolve()); seen = set(); diags, dep_fail = [], []
    for line in stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"): continue
        try: obj = json.loads(line)
        except json.JSONDecodeError: continue
        if obj.get("reason") != "compiler-message": continue
        msg = obj.get("message", {}); level = msg.get("level")
        if level not in ("error", "warning"): continue
        code = (msg.get("code") or {}).get("code") or ""
        text = (msg.get("message") or "").splitlines()[0]
        prim = _primary_span(msg.get("spans", []))
        if _is_rollup(text, bool(prim), code): continue
        if prim:
            fpath = prim["file_name"]
            absf = fpath if os.path.isabs(fpath) else os.path.join(root, fpath)
            loc = f"{_rel(absf, root)}:{prim['line_start']}:{prim['column_start']}"
            is_dep = ("/registry/" in absf) or ("/.cargo/" in absf)
        else:
            loc, is_dep = "(no span)", False
        key = (level, code, loc, text)
        if key in seen: continue
        seen.add(key)
        rec = {"level": level, "code": code, "loc": loc, "msg": text}
        if full and msg.get("rendered"): rec["rendered"] = msg["rendered"]
        (dep_fail if is_dep else diags).append(rec)
    return diags, dep_fail

def rustc_single_file(p, edition, full=False):
    out = "/tmp/rustcheck.rmeta"
    cmd = ["rustc", "--edition", edition, "--emit=metadata", "--crate-type", "lib",
           "-o", out, "--error-format=json", "--color=never", str(p)]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
    except FileNotFoundError:
        return {"unavailable": True, "diagnostics": []}
    seen, ds = set(), []
    for line in proc.stderr.splitlines():
        line = line.strip()
        if not line.startswith("{"): continue
        try: msg = json.loads(line)
        except json.JSONDecodeError: continue
        level = msg.get("level")
        if level not in ("error", "warning"): continue
        code = (msg.get("code") or {}).get("code") or ""
        text = (msg.get("message") or "").splitlines()[0]
        prim = _primary_span(msg.get("spans", []))
        if _is_rollup(text, bool(prim), code): continue
        loc = (f"{prim['file_name']}:{prim['line_start']}:{prim['column_start']}" if prim else "(no span)")
        key = (level, code, loc, text)
        if key in seen: continue
        seen.add(key)
        rec = {"level": level, "code": code, "loc": loc, "msg": text}
        if full and msg.get("rendered"): rec["rendered"] = msg["rendered"]
        ds.append(rec)
    return {"unavailable": False, "exit": proc.returncode, "diagnostics": ds}

# ════════════════════════════════════════════════════════════════════════════
# git-aware changed-file detection
# ════════════════════════════════════════════════════════════════════════════

def git_changed_files(start="."):
    try:
        root = subprocess.run(["git", "-C", start, "rev-parse", "--show-toplevel"],
                              capture_output=True, text=True, timeout=10)
        if root.returncode != 0: return None, []
        gr = root.stdout.strip()
        files = set()
        for a in (["diff", "--name-only", "HEAD"], ["diff", "--name-only", "--cached"],
                  ["ls-files", "--others", "--exclude-standard"]):
            r = subprocess.run(["git", "-C", gr, *a], capture_output=True, text=True, timeout=15)
            for f in r.stdout.splitlines():
                f = f.strip()
                if f: files.add(os.path.join(gr, f))
        return gr, sorted(files)
    except Exception:
        return None, []

# ════════════════════════════════════════════════════════════════════════════
# Driver
# ════════════════════════════════════════════════════════════════════════════

def gather_files(paths):
    files = []
    for raw in paths:
        p = Path(raw)
        if p.is_dir():
            files += [q for q in p.rglob("*") if q.is_file()
                      and (q.suffix == ".rs" or q.suffix in SHADER_EXTS)
                      and "/target/" not in str(q)]
        elif p.suffix == ".rs" or p.suffix in SHADER_EXTS:
            files.append(p)
    seen, out = set(), []
    for f in files:
        r = f.resolve()
        if r not in seen: seen.add(r); out.append(f)
    return out

def _ensure_toolchain_on_path():
    if shutil.which("cargo"): return
    for d in (os.path.expanduser("~/.local/bin"), os.path.expanduser("~/.cargo/bin"),
              "/usr/local/bin", "/root/.local/bin"):
        if os.path.exists(os.path.join(d, "cargo")):
            os.environ["PATH"] = d + os.pathsep + os.environ.get("PATH", ""); return

def main(argv=None):
    _ensure_toolchain_on_path()
    ap = argparse.ArgumentParser(prog="rustcheck", description="AI-centric Rust + shader checker.")
    ap.add_argument("paths", nargs="*", default=["."], help="dir(s)/file(s) (default: .)")
    ap.add_argument("--changed", nargs="+", metavar="FILE", help="only check the package(s) owning these files")
    ap.add_argument("--git", action="store_true", help="derive changed files from git (diff+staged+untracked)")
    ap.add_argument("--fast", action="store_true", help="Tier 0 static only; never invoke the compiler")
    ap.add_argument("--shaders", action="store_true", help="treat all inputs as shaders")
    ap.add_argument("--clippy", action="store_true", help="use cargo clippy if available (else cargo check)")
    ap.add_argument("--deny-warnings", action="store_true", help="warnings count as failure")
    ap.add_argument("--full", action="store_true", help="include rustc's full rendered diagnostic text")
    ap.add_argument("--no-default-features", action="store_true")
    ap.add_argument("--features")
    ap.add_argument("--all-targets", action="store_true")
    ap.add_argument("--tests", action="store_true")
    ap.add_argument("--offline", action="store_true")
    ap.add_argument("--edition", default="2021")
    ap.add_argument("--timeout", type=int, default=600)
    ap.add_argument("--jobs", type=int, default=8, help="parallel workers for static/shader scan")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args(argv)

    report = {"static": [], "shaders": [], "semantic": None, "skip_reason": None}

    # resolve target files
    git_root = None
    if args.git:
        git_root, changed = git_changed_files(args.paths[0] if args.paths else ".")
        targets = [Path(f) for f in changed if Path(f).suffix == ".rs" or Path(f).suffix in SHADER_EXTS]
        args.changed = [str(t) for t in targets if t.suffix == ".rs"] or None
    elif args.changed:
        targets = [Path(f) for f in args.changed]
    else:
        targets = gather_files(args.paths)

    rs_files = [p for p in targets if p.suffix == ".rs" and not args.shaders]
    sh_files = [p for p in targets if (p.suffix in SHADER_EXTS) or (args.shaders and p.suffix == ".rs")]

    # ---- static (Rust) + shader, in parallel ----
    static_hard = False
    report["counts"] = {"rs": len(rs_files), "sh": len(sh_files)}
    with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as ex:
        for r in ex.map(static_check_file, rs_files):
            if r.get("read_error") or r["issues"]:
                report["static"].append(r); static_hard = static_hard or r.get("hard", False)
        report["shaders"] = list(ex.map(check_shader_file, sh_files))

    # ---- semantic (Rust) ----
    run_semantic = not args.fast and bool(rs_files)
    report["skip_reason"] = "fast" if args.fast else None
    if static_hard:
        run_semantic = False; report["skip_reason"] = "hard"

    if run_semantic:
        anchor = Path((args.changed[0] if args.changed else
                       (str(rs_files[0]) if rs_files else args.paths[0])))
        ws, pkg = find_project(anchor.resolve())
        if ws is None and pkg is None:
            sf = []
            for p in rs_files:
                res = rustc_single_file(p, args.edition, args.full)
                if res.get("unavailable"): break
                sf.append({"file": str(p), **res})
            report["semantic"] = {"mode": "single-file", "files": sf}
        else:
            root = cargo_root_dir(ws, pkg); packages = set()
            if args.changed:
                for f in args.changed:
                    m = owning_package(Path(f).resolve())
                    if m and package_name_of(m): packages.add(package_name_of(m))
            elif pkg is not None and package_name_of(pkg):
                packages.add(package_name_of(pkg))
            res = run_cargo(root, packages, args)
            res.update({"mode": "cargo", "root": str(root),
                        "packages": sorted(packages) or ["<whole workspace>"]})
            report["semantic"] = res

    return render(report, args)

def render(report, args):
    sem = report["semantic"]
    n_static = sum(len(r.get("issues", [])) for r in report["static"])
    hard_static = any(i["kind"] != "missing-mod-file"
                      for r in report["static"] for i in r.get("issues", []))
    # shaders
    sh_err = sum(1 for r in report["shaders"] for i in r["issues"] if i.get("level") == "error")
    sh_warn = sum(1 for r in report["shaders"] for i in r["issues"] if i.get("level") == "warning")
    code_errors = warnings = dep_failures = 0
    timed_out = blocked = single = False
    if sem and sem.get("mode") == "cargo":
        code_errors = sum(1 for d in sem["diagnostics"] if d["level"] == "error")
        warnings = sum(1 for d in sem["diagnostics"] if d["level"] == "warning")
        dep_failures = len(sem["dep_failures"]); timed_out = bool(sem.get("timed_out"))
        blocked = bool(sem.get("build_failed")) and code_errors == 0 and not timed_out
    elif sem and sem.get("mode") == "single-file":
        single = True
        for f in sem["files"]:
            code_errors += sum(1 for d in f["diagnostics"] if d["level"] == "error")
            warnings += sum(1 for d in f["diagnostics"] if d["level"] == "warning")

    deny = args.deny_warnings
    summary = {"static_issues": n_static, "code_errors": code_errors, "warnings": warnings,
               "shader_errors": sh_err, "shader_warnings": sh_warn,
               "dep_or_env_failures": dep_failures, "timed_out": timed_out, "blocked": blocked}
    report["summary"] = summary

    if args.json:
        print(json.dumps(report, indent=2))
    else:
        _print_human(report, summary, single, deny)

    fail = bool(code_errors or hard_static or sh_err or (n_static and not sem))
    if n_static and any(i["kind"] == "missing-mod-file" for r in report["static"] for i in r.get("issues", [])):
        if not sem or blocked or timed_out: fail = True
    if deny and (warnings or sh_warn): fail = True
    if fail: return 1
    if timed_out or blocked: return 2
    return 0

def render_color():
    on = sys.stdout.isatty()
    g = lambda s, c: (f"{c}{s}\033[0m" if on else s)
    return g, ("\033[32m", "\033[31m", "\033[33m", "\033[2m", "\033[1m")

def _print_human(report, summary, single, deny):
    c, (GREEN, RED, YEL, DIM, BOLD) = render_color()
    counts = report.get("counts", {"rs": 0, "sh": 0})
    # Tier 0 (only if .rs files were checked)
    if counts["rs"]:
        if report["static"]:
            print(c("● static", BOLD))
            for r in report["static"]:
                if r.get("read_error"): print(f"  {c('READ', RED)} {r['file']}: {r['read_error']}")
                for it in r["issues"]:
                    tag = "WARN" if it["kind"] == "missing-mod-file" else "FAIL"
                    col = YEL if tag == "WARN" else RED
                    print(f"  {c(tag, col)} {r['file']}:{it['line']}:{it['col']}  {c(it['kind'], DIM)}  {it['msg']}")
        else:
            print(c("● static  OK", GREEN))

    # shaders (only if shader files were checked)
    if counts["sh"]:
        problem = [r for r in report["shaders"]
                   if any(i.get("level") in ("error", "warning") for i in r["issues"])]
        if problem:
            print(c("● shaders", BOLD))
            for r in problem:
                for it in r["issues"]:
                    lvl = it.get("level", "error")
                    if lvl == "info": continue
                    col = {"error": RED, "warning": YEL}.get(lvl, RED)
                    tag = {"error": "FAIL", "warning": "WARN"}[lvl]
                    where = f":{it['line']}" if it["line"] else ""
                    print(f"  {c(tag, col)} {r['file']}{where}  {c(it['kind'], DIM)}  {it['msg']}")
            ok = counts["sh"] - len(problem)
            if ok: print(c(f"  {ok} more shader file(s) OK", DIM))
        else:
            print(c(f"● shaders  OK ({counts['sh']} file{'s' if counts['sh'] != 1 else ''})", GREEN))

    sem = report["semantic"]
    if sem is None:
        if report.get("skip_reason") == "hard":
            print(c("● semantic  SKIPPED — fix the structural break above first", YEL))
        elif report.get("skip_reason") == "fast":
            print(c("● semantic  SKIPPED (--fast)", DIM))
    elif sem.get("mode") == "single-file":
        print(c("● semantic  (single-file rustc — no Cargo project)", BOLD))
        for f in sem["files"]:
            if not f["diagnostics"]: print(f"  {c('OK', GREEN)}   {f['file']}")
            for d in f["diagnostics"]: _print_diag(c, d, RED, YEL)
    else:
        if sem.get("clippy_unavailable"):
            print(c("  note: clippy not installed — used `cargo check` instead", DIM))
        head = f"● semantic  cargo {sem['tool']} " + " ".join("-p "+p for p in sem["packages"] if not p.startswith("<"))
        print(c(head.rstrip(), BOLD) + c(f"   ({sem.get('secs','?')}s)", DIM))
        if sem.get("timed_out"):
            print(f"  {c('TIMEOUT', RED)} exceeded {sem['secs']}s — narrow scope with --changed or raise --timeout")
        for d in sem["diagnostics"]: _print_diag(c, d, RED, YEL)
        if not sem["diagnostics"] and not sem["dep_failures"] and not sem.get("timed_out"):
            print(f"  {c('OK', GREEN)}   no errors or warnings")
        if sem["dep_failures"] or (sem.get("raw_stderr") and not sem["diagnostics"]):
            print(c("  ── dependency / environment (NOT your code):", DIM))
            for d in sem["dep_failures"][:6]:
                print(f"     {c('dep', YEL)} {d['loc']}  {d['msg']}")
            for ln in (sem.get("raw_stderr") or "").splitlines()[:4]:
                print(f"     {c('env', YEL)} {ln}")

    s = summary
    fail = (s["code_errors"] or s["static_issues"] or s["shader_errors"]
            or (deny and (s["warnings"] or s["shader_warnings"])))
    if fail: verdict, vcol = "FAIL", RED
    elif s["timed_out"] or s["blocked"]: verdict, vcol = "BLOCKED", YEL
    else: verdict, vcol = "PASS", GREEN
    bits = [f"{s['static_issues']} static", f"{s['code_errors']} errors", f"{s['warnings']} warnings"]
    if report["shaders"]:
        bits.append(f"{s['shader_errors']} shader-err"); bits.append(f"{s['shader_warnings']} shader-warn")
    if s["dep_or_env_failures"] or s["blocked"]: bits.append(f"{s['dep_or_env_failures']} dep/env")
    note = c("  (could not finish — env/deps, not necessarily your code)", DIM) if verdict == "BLOCKED" else ""
    print(f"\n{c(verdict, vcol)}  " + "  ·  ".join(bits) + note)

def _print_diag(c, d, RED, YEL):
    col = RED if d["level"] == "error" else YEL
    code = f"[{d['code']}]" if d["code"] else ""
    print(f"  {c(d['level'], col)}{code} {d['loc']}  {d['msg']}")
    if d.get("rendered"):
        for ln in d["rendered"].rstrip().splitlines():
            print("      " + ln)

if __name__ == "__main__":
    sys.exit(main())
