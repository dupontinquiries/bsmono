#!/usr/bin/env python3
"""
unendlich shader/contract validator — the static layer of the test harness.

PURPOSE
  The assistant (Claude) cannot compile GLSL or Rust in its sandbox, so it has
  historically shipped changes blind. This validator runs the checks that DON'T
  need a compiler, so structural / contract / footgun errors are caught before a
  change is shipped. It is intentionally conservative: ERRORs are things that are
  definitely wrong; WARNs are things to eyeball.

  It is ALSO designed to shell out to the *real* tools when they are present
  (glslc / glslangValidator for GLSL→SPIR-V, cargo for the Rust test suite), so
  on a developer machine it does real compilation too. In the assistant's
  sandbox those are absent and only the static layer runs.

USAGE
  python3 toolkit/harness/validate.py [--repo PATH] [--real] [--quiet]
    --repo PATH   repo root (default: auto-detected from this file's location)
    --real        also invoke glslc/glslangValidator if found (real compile)
    --quiet       only print the summary and failures

EXIT CODE
  0 = no ERRORs (WARNs allowed).  1 = at least one ERROR.  Suitable for CI.

The rules enforced here are documented in toolkit/harness/RULESET.md.
"""
import os, re, sys, subprocess, shutil, argparse

# ─────────────────────────── result plumbing ───────────────────────────
ERRORS = []
WARNS  = []
OKS    = []
def err(rule, msg):  ERRORS.append((rule, msg))
def warn(rule, msg): WARNS.append((rule, msg))
def ok(msg):         OKS.append(msg)

def repo_root_default():
    here = os.path.dirname(os.path.abspath(__file__))
    return os.path.normpath(os.path.join(here, "..", ".."))

# ─────────────────────────── GLSL helpers ──────────────────────────────
def strip_comments(src):
    """Remove // line and /* */ block comments (GLSL has no string literals
    to worry about), preserving newlines so line numbers stay roughly sane."""
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        if c == '/' and i + 1 < n and src[i+1] == '/':
            while i < n and src[i] != '\n': i += 1
        elif c == '/' and i + 1 < n and src[i+1] == '*':
            i += 2
            while i + 1 < n and not (src[i] == '*' and src[i+1] == '/'):
                out.append('\n' if src[i] == '\n' else ' '); i += 1
            i += 2
        else:
            out.append(c); i += 1
    return ''.join(out)

def check_delimiter_balance(name, code):
    pairs = {')': '(', ']': '[', '}': '{'}
    opens = set(pairs.values())
    stack = []
    line = 1
    for ch in code:
        if ch == '\n': line += 1
        elif ch in opens: stack.append((ch, line))
        elif ch in pairs:
            if not stack or stack[-1][0] != pairs[ch]:
                err("glsl.delim", f"{name}: unbalanced '{ch}' near line {line}")
                return
            stack.pop()
    if stack:
        o, ln = stack[-1]
        err("glsl.delim", f"{name}: unclosed '{o}' opened near line {ln}")
    else:
        ok(f"{name}: delimiters balanced")

BINDING_RE = re.compile(r'layout\s*\(([^)]*)\)')
def extract_bindings(code):
    """Return list of (set, binding) ints found in layout(...) qualifiers."""
    found = []
    for m in BINDING_RE.finditer(code):
        body = m.group(1)
        s = re.search(r'\bset\s*=\s*(\d+)', body)
        b = re.search(r'\bbinding\s*=\s*(\d+)', body)
        if b is not None:
            found.append((int(s.group(1)) if s else 0, int(b.group(1))))
    return found

def check_bindings_unique(name, code):
    seen = {}
    for (s, b) in extract_bindings(code):
        if (s, b) in seen:
            err("glsl.binding", f"{name}: duplicate descriptor (set={s}, binding={b})")
        seen[(s, b)] = True
    if seen: ok(f"{name}: {len(seen)} unique descriptor binding(s)")

def check_version(name, code):
    # .glsl files are #include fragments (octree_sample/types) — they have no
    # #version by design. Only standalone stages need one.
    if name.endswith(".glsl"):
        return
    if '#version' not in code:
        warn("glsl.version", f"{name}: no #version directive")

# push-constant size: rough sum of member sizes vs a 128-byte common limit
TYPE_BYTES = {'mat4':64,'mat3':36,'mat2':16,'vec4':16,'vec3':12,'vec2':8,
              'float':4,'int':4,'uint':4,'ivec2':8,'ivec3':12,'ivec4':16,
              'uvec2':8,'uvec3':12,'uvec4':16}
def check_push_constant_size(name, code):
    m = re.search(r'push_constant.*?\{(.*?)\}', code, re.S)
    if not m: return
    total = 0
    for line in m.group(1).split(';'):
        line = line.strip()
        if not line: continue
        t = line.split()[0]
        total += TYPE_BYTES.get(t, 0)
    if total > 128:
        warn("glsl.push", f"{name}: push-constant block ~{total} bytes (>128 may exceed device limit; verify maxPushConstantsSize)")
    elif total:
        ok(f"{name}: push-constant block ~{total} bytes")

# ─────────────────────── chunk.frag contract (mirrors shader_contract.rs) ──
CHUNK_FRAG_EXPECTED_BINDINGS = {0, 1, 2, 3}
FACE_UV_EXPRS = [
    "fract(vec2(v_world_pos.z,v_world_pos.y))",
    "fract(vec2(v_world_pos.x,v_world_pos.z))",
    "fract(vec2(v_world_pos.x,v_world_pos.y))",
]
def check_chunk_frag_contract(code):
    bins = {b for (_, b) in extract_bindings(code)}
    extra = bins - CHUNK_FRAG_EXPECTED_BINDINGS
    if extra:
        err("contract.frag.bindings", f"chunk.frag uses unexpected binding(s) {sorted(extra)} (expected subset of {sorted(CHUNK_FRAG_EXPECTED_BINDINGS)}; update shader_contract.rs if intended)")
    squashed = re.sub(r'\s+', '', code)
    for expr in FACE_UV_EXPRS:
        if expr not in squashed:
            err("contract.frag.faceuv", f"chunk.frag missing face-UV reconstruction `{expr}` (must match world/texture_mapping.rs)")
    ok("chunk.frag: binding + face-UV contract satisfied")

# ─────────────────────── mesher winding footgun (v0.24.90 lesson) ──────────
def check_mesher_winding(repo):
    p = os.path.join(repo, "crates/unendlich-engine/src/render/mesher.rs")
    if not os.path.exists(p): return
    src = open(p, encoding="utf-8", errors="replace").read()
    if "flip_winding" in src:
        err("mesher.winding",
            "render/mesher.rs contains `flip_winding` — REVERTED in v0.24.90. "
            "The CPU greedy mesher is wound INWARD on purpose (inward = visible "
            "under the CLOCKWISE pipeline; the greedy_all_face_normals test asserts "
            "dot<0). Do NOT re-add a global winding flip. See RULESET.md §winding.")
    else:
        ok("render/mesher.rs: no winding-flip footgun (correct)")
    # the contract test that must keep asserting INWARD
    tp = src
    if "greedy_all_face_normals_face_outward_under_pipeline" in tp:
        if re.search(r'assert!\(\s*dot\s*>\s*0', tp):
            err("mesher.winding.test",
                "greedy_all_face_normals_...: assertion is `dot > 0` (outward). It "
                "must be `dot < 0` (inward) — the inward winding is correct here.")
        elif re.search(r'assert!\(\s*dot\s*<\s*0', tp):
            ok("greedy winding test asserts INWARD (dot<0) — correct")

# ─────────────────────── git hygiene: never stage .spv ─────────────────────
def check_no_staged_spv(repo):
    try:
        out = subprocess.run(["git", "-C", repo, "diff", "--cached", "--name-only"],
                             capture_output=True, text=True, timeout=20).stdout
    except Exception:
        return
    spv = [l for l in out.splitlines() if l.endswith(".spv") or l.endswith(".spv.hash")]
    if spv:
        err("git.spv", f"{len(spv)} .spv/.spv.hash file(s) staged — build.rs regenerates these; never commit them: {spv[:4]}")
    else:
        ok("git: no .spv staged")

# ─────────────────────── real compiler layer (if present) ──────────────────
def real_compile(repo, shader_dir):
    glslc = shutil.which("glslc")
    glslang = shutil.which("glslangValidator")
    if not glslc and not glslang:
        warn("real.compiler", "no glslc/glslangValidator found — skipping real GLSL compile (static checks only; run on a dev machine or `cargo test -p vk-harness shaders_compile` for the authoritative compile)")
        return
    stage = {".vert":"vert",".frag":"frag",".comp":"comp"}
    for f in sorted(os.listdir(shader_dir)):
        ext = os.path.splitext(f)[1]
        if ext not in stage: continue
        src = os.path.join(shader_dir, f)
        if glslc:
            cmd = [glslc, "-fshader-stage="+stage[ext], src, "-o", "/dev/null"]
        else:
            cmd = [glslang, "-V", "-S", stage[ext], src, "-o", "/tmp/_v.spv"]
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode != 0:
            err("real.compile", f"{f}: compile failed:\n{(r.stderr or r.stdout).strip()[:800]}")
        else:
            ok(f"{f}: real compile OK")

# ─────────────────────────────── main ──────────────────────────────────
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=repo_root_default())
    ap.add_argument("--real", action="store_true")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args()
    repo = os.path.abspath(args.repo)
    shader_dir = os.path.join(repo, "crates/unendlich-engine/src/shaders")

    if not os.path.isdir(shader_dir):
        print(f"ERROR: shader dir not found: {shader_dir}"); sys.exit(2)

    for f in sorted(os.listdir(shader_dir)):
        if os.path.splitext(f)[1] not in (".comp", ".vert", ".frag", ".glsl"): continue
        raw = open(os.path.join(shader_dir, f), encoding="utf-8", errors="replace").read()
        code = strip_comments(raw)
        check_delimiter_balance(f, code)
        check_bindings_unique(f, code)
        check_version(f, code)
        check_push_constant_size(f, code)
        if f == "chunk.frag":
            check_chunk_frag_contract(code)

    check_mesher_winding(repo)
    check_no_staged_spv(repo)
    if args.real:
        real_compile(repo, shader_dir)

    # ── report ──
    if not args.quiet:
        for m in OKS:   print(f"  ok   {m}")
    for r, m in WARNS:  print(f"  WARN [{r}] {m}")
    for r, m in ERRORS: print(f"  ERR  [{r}] {m}")
    print(f"\n{len(OKS)} ok, {len(WARNS)} warn, {len(ERRORS)} error")
    sys.exit(1 if ERRORS else 0)

if __name__ == "__main__":
    main()
