#!/usr/bin/env python3
"""wiring_radar.py — the wiring system:
  (1) RADAR: mechanizes every sweep technique from WIRING_STATUS into one
      generated, file:line-anchored scratchpad (docs/RADAR.md). I read the
      pad instead of re-grepping the tree every turn.
  (2) FSM CHECK: a real Rust-aware lexer state machine for brace/paren/
      bracket balance — strings, raw strings, chars, line+nested block
      comments handled — reporting the LINE of the first mismatch and the
      innermost unclosed bracket at EOF. Replaces the regex strip that
      missed placement bugs (r175b) and string walls (r176).
  (3) ANCHOR: uniqueness check for an edit anchor BEFORE replacing
      (the r175b lesson, mechanized).

Usage:
  python3 tools/wiring_radar.py                 # write docs/RADAR.md
  python3 tools/wiring_radar.py check FILE...   # FSM balance verify
  python3 tools/wiring_radar.py anchor FILE STR # count + lines of STR
"""
import os, re, sys, collections

SRC = "crates/unendlich-engine/src"
OUT = "docs/RADAR.md"

# ───────────────────────── FSM lexer (Rust-aware) ──────────────────────
def fsm_check(path):
    """Returns (ok, msg). State machine over the raw bytes:
    states: CODE, LINE_COMMENT, BLOCK_COMMENT(depth), STR, RAW_STR(hashes),
    CHAR. Tracks {,(,[ with line numbers."""
    t = open(path, encoding="utf-8", errors="replace").read()
    n = len(t); i = 0; line = 1
    stack = []  # (char, line)
    PAIR = {'}': '{', ')': '(', ']': '['}
    state = "CODE"; depth = 0; raw_hashes = 0
    while i < n:
        c = t[i]
        if c == '\n': line += 1
        if state == "CODE":
            if c == '/' and i+1 < n and t[i+1] == '/':
                state = "LINE_COMMENT"; i += 2; continue
            if c == '/' and i+1 < n and t[i+1] == '*':
                state = "BLOCK_COMMENT"; depth = 1; i += 2; continue
            if c == 'r' and i+1 < n and (t[i+1] == '"' or t[i+1] == '#'):
                j = i+1; h = 0
                while j < n and t[j] == '#': h += 1; j += 1
                if j < n and t[j] == '"':
                    state = "RAW_STR"; raw_hashes = h; i = j+1; continue
            if c == '"':
                state = "STR"; i += 1; continue
            if c == "'":
                # char literal vs lifetime: 'x' or '\x' is a char; 'ident is a lifetime
                if i+1 < n and t[i+1] == '\\':
                    state = "CHAR"; i += 2; continue
                if i+2 < n and t[i+2] == "'":
                    i += 3; continue
                i += 1; continue  # lifetime — skip the quote
            if c in "{([":
                stack.append((c, line)); i += 1; continue
            if c in "})]":
                if not stack:
                    return False, f"{path}:{line}: unmatched closing '{c}' (stack empty)"
                oc, ol = stack.pop()
                if oc != PAIR[c]:
                    return False, f"{path}:{line}: closing '{c}' but innermost open is '{oc}' from line {ol}"
                i += 1; continue
            i += 1; continue
        if state == "LINE_COMMENT":
            if c == '\n': state = "CODE"
            i += 1; continue
        if state == "BLOCK_COMMENT":
            if c == '/' and i+1 < n and t[i+1] == '*': depth += 1; i += 2; continue
            if c == '*' and i+1 < n and t[i+1] == '/':
                depth -= 1; i += 2
                if depth == 0: state = "CODE"
                continue
            i += 1; continue
        if state == "STR":
            if c == '\\': i += 2; continue
            if c == '"': state = "CODE"
            i += 1; continue
        if state == "RAW_STR":
            if c == '"':
                j = i+1; h = 0
                while j < n and t[j] == '#' and h < raw_hashes: h += 1; j += 1
                if h == raw_hashes: state = "CODE"; i = j; continue
            i += 1; continue
        if state == "CHAR":
            if c == "'": state = "CODE"
            i += 1; continue
    if stack:
        oc, ol = stack[-1]
        return False, f"{path}: EOF with {len(stack)} unclosed; innermost '{oc}' opened line {ol}"
    if state != "CODE":
        return False, f"{path}: EOF inside {state}"
    return True, f"{path}: OK"

def cmd_check(paths):
    bad = 0
    for p in paths:
        ok, msg = fsm_check(p)
        print(("PASS " if ok else "FAIL ") + msg)
        bad += 0 if ok else 1
    sys.exit(1 if bad else 0)

def cmd_anchor(path, needle):
    t = open(path, encoding="utf-8", errors="replace").read()
    idxs = [m.start() for m in re.finditer(re.escape(needle), t)]
    print(f"{len(idxs)} occurrence(s) of anchor in {path}")
    for ix in idxs[:10]:
        print(f"  line {t.count(chr(10),0,ix)+1}")
    sys.exit(0 if len(idxs) == 1 else 1)

# ───────────────────────────── analyzers ───────────────────────────────
def files():
    for dp, dn, fns in os.walk(SRC):
        if "target" in dp: continue
        for f in fns:
            if f.endswith(".rs"):
                yield os.path.join(dp, f)

def read(p): return open(p, encoding="utf-8", errors="replace").read()

def lineno(text, idx): return text.count("\n", 0, idx) + 1

def run_radar():
    ALL = {p: read(p) for p in files()}
    BIN = SRC + "/bin/unendlich_fullgame.rs"
    def code_only(t):
        t = re.sub(r"#\[cfg\(test\)\]\s*mod tests.*", "", t, flags=re.S)
        return re.sub(r"//[^\n]*", "", t)
    CODE = {p: code_only(t) for p, t in ALL.items()}
    JOINED = "\n".join(CODE.values())
    findings = collections.OrderedDict()

    # one pass: count every `ident(` in the tree, then O(1) lookups.
    CALLS = collections.Counter(re.findall(r"\b(\w+)\s*\(", JOINED))
    A = []
    SKIP_FN = {"new","default","main","fmt","clone","eq","hash","drop","deref","from","into","next","get","set","len","is_empty"}
    for p, t in ALL.items():
        if "registry" in p: continue
        for m in re.finditer(r"^\s*pub fn (\w+)", t, re.M):
            name = m.group(1)
            if name in SKIP_FN or name.startswith("test"): continue
            if CALLS.get(name, 0) <= 1:
                A.append(f"{p}:{lineno(t, m.start())} pub fn {name}")
    findings["A zero-callsite pub fns"] = A
    # A2: cluster A by module family — batches live where counts spike.
    fam = collections.Counter()
    for line in A:
        m = re.match(r"crates/unendlich-engine/src/([\w/]+?)/", line)
        fam[m.group(1) if m else "(root)"] += 1
    findings["A2 zero-callsite clusters (by family)"] = [
        f"{k}: {v} fns" for k, v in fam.most_common()
    ]

    B = []
    tb = ALL[BIN]
    for m in re.finditer(r"^\s*let _\w* =.*$", tb, re.M):
        line = m.group(0).strip()
        if "create_dir_all" in line: continue
        B.append(f"{BIN}:{lineno(tb, m.start())} {line[:90]}")
    findings["B discarded values (bin)"] = B

    C = []
    for p, t in ALL.items():
        for m in re.finditer(r"(TODO|FIXME|\bstub\b|placeholder|unimplemented)", t):
            C.append(f"{p}:{lineno(t, m.start())} ...{t[max(0,m.start()-30):m.start()+60].strip()[:80]}...".replace("\n"," "))
    findings["C TODO/stub markers"] = C[:80]

    D = []
    PROMISE = re.compile(r"^//!?.*\b(when gameplay|not yet|future|will be|once the|until a|awaiting|hook for|eventually)\b.*$", re.M | re.I)
    for p, t in ALL.items():
        for m in PROMISE.finditer(t):
            D.append(f"{p}:{lineno(t, m.start())} {m.group(0).strip()[:100]}")
    findings["D doc promises"] = D[:60]

    E = []
    for p, t in ALL.items():
        for m in re.finditer(r"^\s*//\s*(tick_reg\.register|\w+\.register\(|registry\.add)", t, re.M):
            E.append(f"{p}:{lineno(t, m.start())} {t[m.start():m.start()+90].strip()}")
    findings["E commented registrations"] = E

    F = []
    for dp, dn, fns in os.walk(SRC):
        if "bin" in dp: continue
        for f in fns:
            if not f.endswith(".rs") or f in ("mod.rs","lib.rs","main.rs"): continue
            name = f[:-3]
            parent = os.path.join(dp, "mod.rs")
            if not os.path.exists(parent):
                parent = os.path.join(SRC, "lib.rs") if dp == SRC else None
            if not parent or not os.path.exists(parent): continue
            if not re.search(rf"\bmod\s+{re.escape(name)}\b", ALL.get(parent, "")):
                F.append(f"{os.path.join(dp,f)}  (no `mod {name}` in {os.path.basename(parent)})")
    findings["F undeclared files"] = F

    G = []
    read_vars = set(re.findall(r'env::var\("([A-Z_0-9]+)"\)', JOINED))
    script = read("tools/run-latest.sh") if os.path.exists("tools/run-latest.sh") else ""
    script_vars = set(re.findall(r"\b(UNENDLICH_[A-Z_0-9]+)\b", script))
    for v in sorted(script_vars - read_vars):
        G.append(f"script sets {v}; no env::var read (dead/renamed knob)")
    for v in sorted(v for v in read_vars - script_vars if v.startswith("UNENDLICH_")):
        G.append(f"code reads {v}; script doesn't set it (undocumented knob)")
    findings["G env-var asymmetry"] = G

    H = []
    ds = ALL.get(SRC + "/debug_settings.rs", "")
    for m in re.finditer(r"pub (\w+): Atomic", ds):
        field = m.group(1)
        uses = JOINED.count("." + field) - code_only(ds).count("." + field)
        if uses <= 0:
            H.append(f"{SRC}/debug_settings.rs:{lineno(ds, m.start())} {field}  (zero consumers)")
    findings["H dead debug toggles"] = H

    I = []
    app = ALL.get(SRC + "/app/mod.rs", "")
    em = re.search(r"pub enum AppInput \{(.*?)\n\}", app, re.S)
    if em:
        for vm in re.finditer(r"^\s*(\w+)", em.group(1), re.M):
            v = vm.group(1)
            if v in ("None",): continue
            if len(re.findall(rf"AppInput::{v}\b", ALL[BIN])) == 0:
                I.append(f"AppInput::{v} — handled by App, never produced by the bin")
    findings["I unproduced AppInputs"] = I

    J = []
    mods = {os.path.basename(p)[:-3] for p in ALL}
    if os.path.isdir("docs/design"):
        for f in sorted(os.listdir("docs/design")):
            if not f.endswith(".extracted.md"): continue
            words = set(re.findall(r"[a-z_]{5,}", read(os.path.join("docs/design", f)).lower()))
            hits = sorted(words & mods)
            if hits:
                J.append(f"{f}: {', '.join(hits[:14])}")
    findings["J doc->module map"] = J

    # K: sibling-scope risk — names referenced in the bin BEFORE (above)
    # their only `let` binding line. Heuristic for the r180b error class.
    K = []
    tb2 = CODE[BIN]
    lets = {}
    for m in re.finditer(r"^\s*let (?:mut )?(\w+)\s*[=:]", tb2, re.M):
        lets.setdefault(m.group(1), []).append(lineno(tb2, m.start()))
    for name, lns in lets.items():
        if len(lns) != 1 or len(name) < 6: continue
        first_use = re.search(rf"\b{name}\b", tb2)
        if first_use and lineno(tb2, first_use.start()) < lns[0]:
            K.append(f"{BIN}:{lineno(tb2, first_use.start())} `{name}` used above its only let ({lns[0]})")
    findings["K sibling-scope risks (bin)"] = K[:40]

    lines = ["# WIRING RADAR — generated scratchpad (file:line anchors)",
             "# regen: python3 tools/wiring_radar.py   |   verify: ... check FILE   |   ... anchor FILE STR", ""]
    for k, v in findings.items():
        lines.append(f"## {k}  ({len(v)})")
        lines.extend(v if v else ["(clean)"])
        lines.append("")
    open(OUT, "w").write("\n".join(lines))
    print(f"radar -> {OUT}")
    for k, v in findings.items():
        print(f"  {k}: {len(v)}")

if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "check":
        cmd_check(sys.argv[2:])
    elif len(sys.argv) > 1 and sys.argv[1] == "anchor":
        cmd_anchor(sys.argv[2], sys.argv[3])
    else:
        run_radar()
