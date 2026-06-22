#!/usr/bin/env python3
"""qml_check.py — a qmllint-lite static checker for QML, since we can't run Qt
in the sandbox. Partial but systematic. Checks, per file and across the set:

  1. Bracket balance ({}, (), []) — string/comment aware.
  2. Unterminated string literals.
  3. Duplicate `id:` values within a file (QML requires unique ids per scope;
     we treat per-file, which catches the common mistakes).
  4. Element/type resolution: every PascalCase element used as `Name {` must be
     a known builtin (allowlist) OR a local component (a .qml file in the set).
  5. qrc/dependency graph: which local components each file references, and
     whether every referenced local component is embedded in the qrc list in
     the Rust source (so a forgotten qrc entry is caught).
  6. Missing `import` heuristic: a file using Controls types but not importing
     QtQuick.Controls is flagged.

Usage: qml_check.py <qml_dir> [--qrc <rust_file_with_qrc!>]
Exit non-zero if any errors (warnings don't fail).
"""
import os, re, sys

# Builtin QtQuick / QtQuick.Controls / QtQuick.Layouts types we use. Extend as
# needed; unknown PascalCase elements not in here and not local → error.
BUILTINS = {
    # core QtQuick
    "Item", "Rectangle", "Text", "MouseArea", "Image", "Column", "Row", "Grid",
    "Flow", "ListView", "GridView", "PathView", "Repeater", "Loader", "Flickable",
    "Timer", "Component", "Connections", "Behavior", "PropertyAnimation",
    "NumberAnimation", "ColorAnimation", "SequentialAnimation",
    "ParallelAnimation", "State", "Transition", "Gradient", "GradientStop",
    "TextInput", "TextEdit", "FontMetrics", "Translate", "Scale", "Rotation",
    "DropShadow", "OpacityMask",
    # pointer handlers (QtQuick)
    "TapHandler", "DragHandler", "PinchHandler", "HoverHandler", "PointHandler",
    "WheelHandler", "DropArea", "Drag",
    # QtQuick.Controls
    "ApplicationWindow", "Button", "Label", "SwipeView", "TabBar", "TabButton",
    "SwipeDelegate", "ItemDelegate", "TextField", "TextArea", "ScrollView",
    "ScrollBar", "Switch", "CheckBox", "RadioButton", "Slider", "ComboBox",
    "Popup", "Dialog", "Menu", "MenuItem", "ToolBar", "ToolButton", "Page",
    "PageIndicator", "BusyIndicator", "ProgressBar", "Frame", "GroupBox",
    "StackView", "Drawer", "DelayButton", "RoundButton", "Pane", "Control",
    # QtQuick.Layouts
    "RowLayout", "ColumnLayout", "GridLayout", "StackLayout",
}

def strip_strings_comments(src):
    """Return src with string contents and comments blanked (preserving length
    and newlines) so bracket counting isn't fooled. Also returns list of
    unterminated-string line numbers."""
    out = []
    i, n = 0, len(src)
    unterminated = []
    line = 1
    while i < n:
        c = src[i]
        if c == "\n":
            line += 1; out.append(c); i += 1; continue
        # line comment
        if c == "/" and i + 1 < n and src[i+1] == "/":
            while i < n and src[i] != "\n":
                out.append(" "); i += 1
            continue
        # block comment
        if c == "/" and i + 1 < n and src[i+1] == "*":
            out.append("  "); i += 2
            while i < n and not (src[i] == "*" and i+1 < n and src[i+1] == "/"):
                out.append("\n" if src[i] == "\n" else " ")
                if src[i] == "\n": line += 1
                i += 1
            out.append("  "); i += 2
            continue
        # strings
        if c in "\"'":
            quote = c; out.append(" "); i += 1; start_line = line
            closed = False
            while i < n:
                if src[i] == "\\" and i+1 < n:
                    out.append("  "); i += 2; continue
                if src[i] == "\n":
                    break
                if src[i] == quote:
                    out.append(" "); i += 1; closed = True; break
                out.append(" "); i += 1
            if not closed:
                unterminated.append(start_line)
            continue
        out.append(c); i += 1
    return "".join(out), unterminated

def check_balance(clean):
    pairs = {")": "(", "]": "[", "}": "{"}
    opens = set("([{")
    stack = []
    line = 1
    for ch in clean:
        if ch == "\n":
            line += 1
        elif ch in opens:
            stack.append((ch, line))
        elif ch in pairs:
            if not stack or stack[-1][0] != pairs[ch]:
                return f"unbalanced '{ch}' at line {line}"
            stack.pop()
    if stack:
        ch, ln = stack[-1]
        return f"unclosed '{ch}' opened at line {ln}"
    return None

def find_ids(clean):
    ids = {}
    for m in re.finditer(r"\bid\s*:\s*([A-Za-z_]\w*)", clean):
        name = m.group(1)
        ln = clean[:m.start()].count("\n") + 1
        ids.setdefault(name, []).append(ln)
    return ids

def find_elements(clean):
    """PascalCase identifiers used as element headers: `Name {` or `Name.Sub {`
    Return set of root type names used as elements."""
    used = set()
    for m in re.finditer(r"(?<![\w.])([A-Z]\w*)(?:\.[A-Z]\w*)?\s*\{", clean):
        used.add(m.group(1))
    # attached-property usages like `ListView.view` are not element headers; the
    # `{` requirement above avoids most of those.
    return used

def parse_qrc(rust_path):
    try:
        src = open(rust_path).read()
    except OSError:
        return None
    m = re.search(r"qrc!\s*\([^)]*?\{(.*?)\}\s*,?\s*\)", src, re.S)
    if not m:
        return set()
    files = set(re.findall(r'"([^"]+\.qml)"', m.group(1)))
    return {os.path.basename(f) for f in files}

def main():
    if len(sys.argv) < 2:
        print("usage: qml_check.py <qml_dir> [--qrc rust_file]"); return 2
    qml_dir = sys.argv[1]
    qrc_file = None
    if "--qrc" in sys.argv:
        qrc_file = sys.argv[sys.argv.index("--qrc") + 1]

    qml_files = []
    for root, _, files in os.walk(qml_dir):
        for f in files:
            if f.endswith(".qml"):
                qml_files.append(os.path.join(root, f))
    qml_files.sort()
    local_components = {os.path.splitext(os.path.basename(f))[0] for f in qml_files}

    errors, warnings = [], []
    ref_graph = {}  # file basename -> set(local components referenced)

    for path in qml_files:
        base = os.path.basename(path)
        src = open(path).read()
        clean, unterminated = strip_strings_comments(src)
        for ln in unterminated:
            errors.append(f"{base}:{ln}: unterminated string literal")
        bal = check_balance(clean)
        if bal:
            errors.append(f"{base}: {bal}")
        # duplicate ids
        for name, lines in find_ids(clean).items():
            if len(lines) > 1:
                errors.append(f"{base}: duplicate id '{name}' at lines {lines}")
        # element resolution
        used = find_elements(clean)
        refs_local = used & local_components
        ref_graph[base] = refs_local
        unknown = {u for u in used if u not in BUILTINS and u not in local_components}
        for u in sorted(unknown):
            errors.append(f"{base}: unknown element type '{u}' (not a builtin or local component)")
        # import heuristic
        controls = {"Button", "Label", "SwipeView", "TabBar", "TabButton",
                    "SwipeDelegate", "TextField", "TextArea", "ApplicationWindow",
                    "Page", "ScrollView", "Popup", "Dialog"}
        if used & controls and "import QtQuick.Controls" not in src:
            warnings.append(f"{base}: uses Controls types but no 'import QtQuick.Controls'")

    # qrc membership
    if qrc_file:
        embedded = parse_qrc(qrc_file)
        if embedded is not None:
            for base in (os.path.basename(f) for f in qml_files):
                if base not in embedded:
                    warnings.append(f"{base}: not embedded in qrc! ({qrc_file})")
            # referenced-but-missing
            for f, refs in ref_graph.items():
                for r in refs:
                    rfile = r + ".qml"
                    if rfile not in embedded:
                        errors.append(f"{f}: references component '{r}' but {rfile} is not in qrc!")

    # dependency graph print
    print("dependency graph (file → local components used):")
    for f in sorted(ref_graph):
        deps = ", ".join(sorted(ref_graph[f])) or "—"
        print(f"  {f}: {deps}")

    print()
    for w in warnings:
        print(f"  warning: {w}")
    for e in errors:
        print(f"  ERROR: {e}")

    print()
    if errors:
        print(f"✗ {len(errors)} error(s), {len(warnings)} warning(s)")
        return 1
    print(f"● QML OK — {len(qml_files)} file(s), {len(warnings)} warning(s)")
    return 0

if __name__ == "__main__":
    sys.exit(main())
