#!/usr/bin/env python3
"""
QML pre-flight check for web2qml.

WHY: `qmllint` (and the old qml_lint gate) does NOT catch load-fatal errors like
"Property value set multiple times" (a property/handler bound twice on the same
object) — the engine rejects the whole component at load with no UI. This harness
closes that gap.

TWO LAYERS:

1) AUTHORITATIVE — qmlcachegen.
   qmlcachegen is the real Qt QML compiler. It reports the exact errors the engine
   would hit at load (duplicate bindings, assignments to non-existent properties on
   known types, syntax errors) WITHOUT needing the live `backend` context object.
   Any `file:line:col: error:` it prints (line > 0) is a hard failure. The benign
   ":0: Library import requires a version" note is ignored (the running engine is
   fine with version-less imports; qmlcachegen is just stricter).

2) AUTHORITATIVE — duplicate id (document-wide).
   QML ids must be unique across the whole file. qmlcachegen does NOT catch a
   collision ("id is not unique" is an instantiation-time error), so layer 3 scans
   for it directly — tracking QML-vs-JS scope so JS object-literal `id:` keys and
   `property <type> id:` declarations don't false-positive.

3) DIAGNOSTICS — duplicate-property scope parser.
   qmlcachegen points only at the SECOND occurrence. This parser reports the
   property name and BOTH line numbers per object, so the fix is obvious. It is a
   best-effort lexer (handles //, /* */, "…", '…', `…`, and brace-classified
   QML-object vs JS scopes); qmlcachegen above remains the source of truth.

Exit status: 0 if every file compiles, 1 otherwise.

Usage: qml_check.py [file.qml ...]   (default: all qml/*.qml)
"""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
QML_DIR = os.path.join(ROOT, "qml")

RED, GRN, YEL, CYN, RST = "\033[1;31m", "\033[1;32m", "\033[1;33m", "\033[1;36m", "\033[0m"


# ─────────────────── layer 3: duplicate id (document-wide) ───────────────
# QML ids must be unique across the WHOLE file (document-scoped). This is a
# hard load-time error ("id is not unique") — but qmlcachegen does NOT catch it
# (id uniqueness is enforced at QQmlComponent instantiation, not at cache-gen
# compile time), and layer-2's line-start scan misses inline `Type { id: x }`.
# So this is its own authoritative pass.
_ID_RE = re.compile(r"\bid\s*:\s*([A-Za-z_]\w*)")


def find_dup_ids(path):
    """Return (value, first_line, dup_line) for any id declared more than once
    WITHIN THE SAME component id-scope. QML ids are scoped per component: the root
    document is one scope, and every `Component { }` and `delegate:` object opens a
    fresh scope (so the same id may legally appear in two different delegates).
    Only counts `id:` bindings in a QML-object scope (not JS object literals) and
    skips `property <type> id:` declarations."""
    with open(path, encoding="utf-8", errors="replace") as f:
        src = f.read()
    clean = _strip_noise(src)
    n = len(clean)

    # Single scan: per position, record (is-qml-scope, id-scope number). A new
    # id-scope is pushed when a '{' opens a `Component { }` or a `delegate: Type {`
    # object; everything else inherits the enclosing id-scope.
    qml_at = bytearray(n)
    scope_at = [0] * n
    next_scope = 1
    stack = [("qml", 0)]          # (kind, idscope)
    _DELEG_RE = re.compile(r"\bdelegate\s*:\s*\w+\s*$")
    _COMP_RE = re.compile(r"\bComponent\s*$")
    for i, c in enumerate(clean):
        kk, sc = stack[-1]
        qml_at[i] = 1 if kk == "qml" else 0
        scope_at[i] = sc
        if c == "{":
            j = i - 1
            while j >= 0 and clean[j] in " \t\n":
                j -= 1
            prev = clean[j] if j >= 0 else ""
            kind = "js" if prev in ":)>=,&|;([" else "qml"
            idscope = sc
            if kind == "qml":
                pre = clean[max(0, i - 80):i]
                if _COMP_RE.search(pre) or _DELEG_RE.search(pre):
                    idscope = next_scope
                    next_scope += 1
            stack.append((kind, idscope))
        elif c == "}":
            if len(stack) > 1:
                stack.pop()

    line_starts = [0]
    for i, c in enumerate(clean):
        if c == "\n":
            line_starts.append(i + 1)
    import bisect
    def line_of(pos):
        return bisect.bisect_right(line_starts, pos)

    seen = {}                     # (idscope, value) -> first line
    dups = []
    for m in _ID_RE.finditer(clean):
        start = m.start()
        if not qml_at[start]:
            continue                       # JS scope → object-literal key
        j = start - 1
        while j >= 0 and clean[j] in " \t":
            j -= 1
        if j >= 0 and (clean[j].isalnum() or clean[j] == "_"):
            continue                       # `property <type> id:` declaration
        key = (scope_at[start], m.group(1))
        ln = line_of(start)
        if key in seen:
            dups.append((m.group(1), seen[key], ln))
        else:
            seen[key] = ln
    return dups


# ───────────────────────── layer 1: qmlcachegen ─────────────────────────
def qmlcachegen_errors(path):
    """Return list of (line, col, msg) real errors, or None if tool missing."""
    env = dict(os.environ, QT_SELECT="qt5")
    try:
        r = subprocess.run(
            ["qmlcachegen", "-o", "/tmp/_qmlcheck.qmlc", path],
            capture_output=True, text=True, env=env, timeout=180,
        )
    except FileNotFoundError:
        return None
    errs = []
    for line in (r.stdout + "\n" + r.stderr).splitlines():
        m = re.search(r":(\d+):(\d+): error: (.+)", line)
        if not m:
            continue
        ln = int(m.group(1))
        if ln <= 0:  # the ":0: Library import requires a version" note — benign
            continue
        errs.append((ln, int(m.group(2)), m.group(3).strip()))
    return errs


# ───────────────────────── layer 2: dup parser ──────────────────────────
def _strip_noise(src):
    """Blank out comment + string contents (keep newlines & length) so brace and
    colon scanning isn't fooled by them."""
    out = []
    i, n = 0, len(src)
    while i < n:
        c = src[i]
        nx = src[i + 1] if i + 1 < n else ""
        if c == "/" and nx == "/":
            while i < n and src[i] != "\n":
                out.append(" "); i += 1
            continue
        if c == "/" and nx == "*":
            out.append("  "); i += 2
            while i < n and not (src[i] == "*" and i + 1 < n and src[i + 1] == "/"):
                out.append("\n" if src[i] == "\n" else " "); i += 1
            out.append("  "); i += 2
            continue
        if c in "\"'`":
            q = c
            out.append(" "); i += 1
            while i < n and src[i] != q:
                if src[i] == "\\" and i + 1 < n:
                    out.append("  "); i += 2; continue
                out.append("\n" if src[i] == "\n" else " "); i += 1
            out.append(" "); i += 1
            continue
        out.append(c); i += 1
    return "".join(out)


_JS_KEYWORDS = {"else", "try", "do", "finally", "in", "of", "return", "case",
                "default", "new", "typeof", "delete", "void", "instanceof"}
_PROP_RE = re.compile(r"^\s*(on[A-Z]\w*|[A-Za-z_]\w*(?:\.\w+)*)\s*:")
_PROPDECL_RE = re.compile(
    r"^\s*(?:readonly\s+|default\s+|required\s+)*property\s+[\w<>.]+\s+(\w+)\s*[:{]?")


def find_dupes(path):
    """Best-effort: list (name, first_line, dup_line) for properties bound twice
    on the same QML object."""
    with open(path, encoding="utf-8", errors="replace") as f:
        src = f.read()
    clean = _strip_noise(src)
    lines = clean.split("\n")

    # Pass A: per-line scope id (the object a line's first token belongs to).
    # scope stack of ids; props recorded per id. kind tells qml vs js.
    next_id = 1
    stack = [0]
    kind = {0: "qml"}
    line_scope = [0] * (len(lines) + 1)

    def classify_open(prefix):
        # prefix = cleaned text just before this '{'. JS block if it follows ':',
        # ')', '=>', or a JS keyword; otherwise a QML object ('Type {' / 'Behavior
        # on x {').
        p = prefix.rstrip()
        if not p:
            return "qml"
        last = p[-1]
        if last in ":)>=,&|;([":
            return "js"
        m = re.search(r"([A-Za-z_]\w*)$", p)
        if m and m.group(1) in _JS_KEYWORDS:
            return "js"
        # 'Behavior on scale' ends in identifier 'scale' but IS a qml object — the
        # token two back is 'on'. Treat any identifier-terminated prefix as qml.
        return "qml"

    pos = 0
    for li, raw in enumerate(lines, start=1):
        line_scope[li] = stack[-1]
        for ci, c in enumerate(raw):
            if c == "{":
                nonlocal_kind = classify_open(raw[:ci] if ci else
                                              ("".join(lines[max(0, li - 2):li - 1])))
                if nonlocal_kind == "qml":
                    kind[next_id] = "qml"
                else:
                    kind[next_id] = "js"
                stack.append(next_id)
                next_id += 1
            elif c == "}":
                if len(stack) > 1:
                    stack.pop()
        pos += len(raw) + 1

    # Pass B: record property bindings at qml-object line starts.
    scope_props = {}
    dupes = []
    for li, raw in enumerate(lines, start=1):
        sid = line_scope[li]
        if kind.get(sid) != "qml":
            continue
        md = _PROPDECL_RE.match(raw)
        if md:
            name = md.group(1)
        else:
            mb = _PROP_RE.match(raw)
            if not mb:
                continue
            name = mb.group(1)
            # The special object `id:` attribute lives in its own namespace — it
            # does NOT collide with a user property declared `property … id`
            # (see SlidePopup _emitTimer). Skipping it avoids a false positive.
            if name == "id":
                continue
        d = scope_props.setdefault(sid, {})
        if name in d:
            dupes.append((name, d[name], li))
        else:
            d[name] = li
    return dupes


# ───────────────────────────── driver ───────────────────────────────────
def main(argv):
    files = argv[1:] or sorted(
        os.path.join(QML_DIR, f) for f in os.listdir(QML_DIR) if f.endswith(".qml"))
    tool_missing = False
    total_err = 0
    for path in files:
        rel = os.path.relpath(path, ROOT)
        cg = qmlcachegen_errors(path)
        dupes = find_dupes(path)
        dup_ids = find_dup_ids(path)
        cg_present = cg is not None
        if not cg_present:
            tool_missing = True
            cg = []
        cg_lines = {ln for ln, _, _ in cg}

        # Authoritative failures: qmlcachegen errors AND duplicate ids (which
        # qmlcachegen does NOT catch). If the compiler is missing, the heuristic
        # property parser also becomes a gate.
        fail = len(cg) + len(dup_ids) + (0 if cg_present else len(dupes))
        # Heuristic-only finds while the compiler is HAPPY are shown as notes, not
        # failures (the real compiler is the source of truth).
        notes = [d for d in dupes if d[2] not in cg_lines and cg_present]

        if fail == 0:
            if notes:
                print(f"{GRN}✓{RST} {rel}  {YEL}({len(notes)} heuristic note(s)){RST}")
                for name, first, dup in notes:
                    print(f"    {YEL}note{RST} possible duplicate '{name}' "
                          f"(lines {first}, {dup}) — compiler is clean, likely fine")
            else:
                print(f"{GRN}✓{RST} {rel}")
            continue

        total_err += fail
        print(f"{RED}✗ {rel}{RST}")
        for ln, col, msg in cg:
            print(f"    {RED}{ln}:{col}{RST}  {msg}")
        for val, first, dup in dup_ids:
            print(f"    {RED}duplicate id{RST} '{val}': first at line {first}, "
                  f"again at line {dup}  {YEL}(id must be unique per file){RST}")
        for name, first, dup in dupes:
            if dup in cg_lines:
                print(f"    {CYN}↳ duplicate{RST} '{name}': first at line {first}, "
                      f"again at line {dup}")
            elif not cg_present:
                print(f"    {RED}duplicate{RST} '{name}': first at line {first}, "
                      f"again at line {dup}  {YEL}(parser; compiler unavailable){RST}")

    if tool_missing:
        print(f"{YEL}! qmlcachegen not found — install qtdeclarative5-dev-tools "
              f"for authoritative checks; used the heuristic parser only.{RST}")
    if total_err:
        print(f"{RED}═══ QML CHECK FAILED — {total_err} issue(s) — DO NOT SHIP ═══{RST}")
        return 1
    print(f"{GRN}═══ QML check passed ({len(files)} file(s)) ═══{RST}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
