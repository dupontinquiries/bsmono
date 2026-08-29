#!/usr/bin/env python3
"""qml_outline.py — QML to a plaintext layout description, for porting to egui.

WHY THIS EXISTS
---------------
Porting a Qt UI to egui by reading the QML screen-by-screen was tried and
did not work. The reason is visible the moment you measure: voiceterm's
`main.qml` is 32,111 lines holding 1,282 visual elements. Nobody — person
or model — holds that in their head, and reading it linearly loses the
structure that actually matters, which is the nesting and the sizing.

So this does not translate QML. It extracts the SHAPE: what is inside
what, how each thing gets its size, and the handful of properties a port
must preserve. The output is a few hundred lines per screen, which IS
holdable, and which a finished port can be audited against.

The pathway is deliberately three steps rather than two:

    QML  ->  this outline  ->  egui code

The middle artifact is the point. It is reviewable by someone who knows
what the screen should look like without their reading QML, it diffs
cleanly when the QML changes, and a port can be checked against it
afterwards — none of which a direct QML->Rust jump gives you.

WHAT IT IS NOT
--------------
Not a QML parser. It is a brace-matching structural reader that knows
enough QML to find elements, their ids, and a chosen set of layout
properties. QML's expression grammar (bindings, JS blocks, states,
transitions) is deliberately NOT interpreted: a binding like
`width: parent.width * 0.6` is reported as a proportion rather than
evaluated, because "60% of parent" is what a port needs and evaluating it
would require a running Qt.

SIZE APPROXIMATION
------------------
Explicit sizes are reported as-is. Bindings are CLASSIFIED, not computed:

    fill      anchors.fill, Layout.fillWidth/fillHeight
    frac      an expression over `parent.` with a `*` — a proportion
    fixed     a literal number
    expr      some other binding; shown verbatim, needs a human
    implicit  no size given: the element sizes to its content

That classification is most of a faithful port. egui's
`ui.available_width()` answers `fill`; a `* 0.6` answers `frac`;
`implicit` means let the widget size itself. None of the three need Qt to
be running, which is the whole reason this is tractable offline.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

# Elements that occupy space. Everything else (Connections, Timer, states,
# property declarations) is behaviour, and a layout description that lists
# Timers is noise at exactly the moment you are trying to see the shape.
VISUAL = {
    "Item", "Rectangle", "Text", "Label", "Button", "ToolButton", "TextField",
    "TextArea", "TextEdit", "Image", "Column", "Row", "Grid", "Flow",
    "ColumnLayout", "RowLayout", "GridLayout", "ListView", "GridView",
    "ScrollView", "Flickable", "Repeater", "Slider", "CheckBox", "ComboBox",
    "SpinBox", "ProgressBar", "BusyIndicator", "Switch", "Canvas", "Popup",
    "Dialog", "MenuItem", "Menu", "TabBar", "TabButton", "StackLayout",
    "SwipeView", "Page", "Pane", "Frame", "GroupBox", "Loader",
    # Found by the skip report on voiceterm's own QML — every one of these
    # holds layout and was silently absent from the outline until the
    # report named it. TextInput especially: 14 of them, and an outline
    # that omits the text fields is not a description of the screen.
    "TextInput", "Drawer", "RadioButton", "ToolTip", "FocusScope",
    "ApplicationWindow", "DropArea",
}

# Project-local components: capitalised, hold layout, but are defined in
# the app's own .qml files rather than by Qt. Detected by file name so the
# harness does not need a hardcoded per-app list — a component is anything
# with a matching `<Name>.qml` beside the file being read.
def local_components(path: Path) -> set:
    try:
        return {p.stem for p in path.parent.glob("*.qml")}
    except OSError:
        return set()

# Properties worth carrying into a port. Chosen rather than "all of them":
# an outline that reproduces every binding is just the QML again.
KEEP = [
    "width", "height", "implicitWidth", "implicitHeight",
    "Layout.fillWidth", "Layout.fillHeight", "Layout.preferredWidth",
    "Layout.preferredHeight", "Layout.minimumWidth", "Layout.minimumHeight",
    "anchors.fill", "anchors.margins", "spacing", "padding",
    "text", "visible", "color", "font.pixelSize", "font.bold",
    "orientation", "wrapMode", "elide",
]

ELEMENT_RE = re.compile(r"^(\s*)([A-Z][A-Za-z0-9_.]*)\s*\{")
PROP_RE = re.compile(r"^\s*([a-zA-Z][A-Za-z0-9_.]*)\s*:\s*(.+?)\s*$")


@dataclass
class Node:
    kind: str
    line: int
    ident: str | None = None
    props: dict = field(default_factory=dict)
    children: list = field(default_factory=list)

    def size_class(self) -> str:
        """How this element gets its size — the thing a port must match."""
        p = self.props
        if p.get("anchors.fill") or p.get("Layout.fillWidth") == "true" \
                or p.get("Layout.fillHeight") == "true":
            return "fill"
        for key in ("width", "height", "Layout.preferredWidth",
                    "Layout.preferredHeight"):
            v = p.get(key)
            if v is None:
                continue
            if re.fullmatch(r"[0-9.]+", v):
                return "fixed"
            if "parent." in v and "*" in v:
                return "frac"
            return "expr"
        return "implicit"

    def label(self) -> str:
        bits = [self.kind]
        if self.ident:
            bits.append("#" + self.ident)
        bits.append("[" + self.size_class() + "]")
        for key in ("width", "height", "spacing"):
            if key in self.props:
                bits.append(key + "=" + self.props[key])
        if "text" in self.props:
            t = self.props["text"]
            if len(t) > 42:
                t = t[:39] + "..."
            bits.append("text=" + t)
        if self.props.get("visible") not in (None, "true"):
            bits.append("visible=" + self.props["visible"])
        return " ".join(bits)


# Capitalised names that are QML machinery rather than missing widgets.
# Listed so the skip report stays signal: a Timer in the output would
# train people to ignore it, which is how a genuinely missing widget gets
# ignored too.
NON_VISUAL_KNOWN = {
    "Timer", "Connections", "Component", "QtObject", "Behavior",
    "PropertyAnimation", "NumberAnimation", "ColorAnimation",
    "SequentialAnimation", "ParallelAnimation", "PropertyChanges",
    "State", "Transition", "MouseArea", "TapHandler", "DragHandler",
    "PinchHandler", "HoverHandler", "Shortcut", "FileDialog", "Settings",
    "SoundEffect", "MediaPlayer", "Binding", "Instantiator",
    "SystemPalette", "FontLoader", "Gradient", "GradientStop", "Qt",
    "ListModel", "ListElement", "PauseAnimation", "ScriptAction",
    "SmoothedAnimation", "SpringAnimation", "TextMetrics", "FontMetrics",
    "Translate", "Rotation", "Scale", "Matrix4x4", "WheelHandler",
    "PointHandler", "FolderDialog", "PathView", "Path", "PathLine",
    "Math", "JSON", "Object", "Array", "String", "Number", "Boolean",
    "Date", "RegExp", "Promise", "Error", "Item2",
}


def parse(text: str, extra_visual=frozenset()):
    """Structural read. Brace depth is the tree; indentation is not trusted."""
    roots = []
    stack = []
    pending = None
    skipped = {}

    for lineno, raw in enumerate(text.splitlines(), 1):
        line = raw.split("//")[0]
        m = ELEMENT_RE.match(line)
        if m:
            kind = m.group(2)
            node = Node(kind=kind, line=lineno)
            # NAMESPACED WIDGETS (PlasmaExtras.Menu, Kirigami.Icon,
            # QtCharts.ChartView) are one import prefix away from a name
            # we know. Matching on the last segment covers whole widget
            # libraries without listing them: hatch uses Plasma and
            # Kirigami and would otherwise outline as a handful of bare
            # Items. Reported in the skip list either way, so an unknown
            # namespaced widget still surfaces rather than being guessed
            # at silently.
            bare = kind.rsplit(".", 1)[-1]
            visual = kind in VISUAL or kind in extra_visual \
                or bare in VISUAL or bare in extra_visual
            if not visual and kind not in NON_VISUAL_KNOWN \
                    and bare not in NON_VISUAL_KNOWN:
                skipped[kind] = skipped.get(kind, 0) + 1
            if visual:
                parent = next((n for n in reversed(stack) if n.kind in VISUAL), None)
                if parent is not None:
                    parent.children.append(node)
                else:
                    roots.append(node)
            stack.append(node)
            pending = node if visual else None
            # A ONE-LINE ELEMENT carries its properties on that same line
            # (`Text { text: "hi"; visible: false }`), and QML uses these
            # constantly for small leaves. Scraping them here rather than
            # only in the line-by-line pass below is what stops a whole
            # class of element — every inline Text and Rectangle — from
            # appearing in the outline stripped of the one thing that
            # identifies it.
            if line.count("}") >= line.count("{"):
                if visual:
                    inner = line[line.index("{") + 1:line.rindex("}")]
                    for part in inner.split(";"):
                        pm = PROP_RE.match(part.strip())
                        if not pm:
                            continue
                        key, val = pm.group(1), pm.group(2).strip()
                        if key in KEEP:
                            node.props[key] = val
                        elif key == "id":
                            node.ident = val
                stack.pop()
                pending = None
            continue

        if pending is not None:
            # SEMICOLON-SEPARATED PROPS ON ONE LINE. QML allows
            # `id: termScreen; Layout.fillWidth: true; height: 200` and
            # main.qml uses it constantly. Reading only the first pair
            # lost every id declared that way — which is most of the
            # named regions, so slicing by screen found nothing at all
            # until this was fixed.
            #
            # Split on `;` only when the line has no braces: a `;` inside
            # a binding expression or a JS block is not a separator, and
            # treating it as one would invent properties.
            parts = line.split(";") if "{" not in line and "}" not in line else [line]
            for part in parts:
                pm = PROP_RE.match(part.strip())
                if not pm:
                    continue
                key, val = pm.group(1), pm.group(2).rstrip(";").strip()
                if key in KEEP:
                    pending.props[key] = val
                elif key == "id":
                    pending.ident = val

        for _ in range(line.count("}") - line.count("{")):
            if stack:
                stack.pop()
            pending = None


    return roots, skipped


def find_region(nodes, ident):
    """The subtree rooted at the element whose `id:` is `ident`.

    main.qml is 32k lines and one file, so "audit the terminal screen"
    has no file to point at — the screens are regions inside it, named by
    their `id`. Without this the outline for a screen is the whole app's
    outline and the audit that worked on ChatPanel does not transfer.

    Returns None when the id is absent rather than an empty list, so a
    typo reads as "no such region" instead of "that screen is empty" —
    the second would look like a finding.
    """
    for n in nodes:
        if n.ident == ident:
            return n
        hit = find_region(n.children, ident)
        if hit is not None:
            return hit
    return None


def render(nodes, out, indent=0):
    for n in nodes:
        out.append("  " * indent + n.label() + "  (L" + str(n.line) + ")")
        render(n.children, out, indent + 1)


def summarise(nodes):
    counts = {}

    def walk(ns):
        for n in ns:
            counts[n.kind] = counts.get(n.kind, 0) + 1
            walk(n.children)

    walk(nodes)
    return counts


def outline_file(path: Path, max_depth=None, region=None) -> str:
    text = path.read_text(errors="replace")
    roots, skipped = parse(text, local_components(path))
    if region is not None:
        node = find_region(roots, region)
        if node is None:
            return "# " + path.name + ": no element with id '" + region + "'"
        roots = [node]
        skipped = {}  # counted for the whole file; misleading for a slice
    counts = summarise(roots)
    total = sum(counts.values())
    top = sorted(counts.items(), key=lambda kv: -kv[1])[:8]
    lines = [
        "# " + path.name + " — " + str(len(text.splitlines())) + " lines of QML",
        "# " + str(total) + " visual elements",
        "# most used: " + ", ".join(k + "x" + str(v) for k, v in top),
        "",
    ]
    # WHAT WAS SKIPPED, and why this is not optional. `VISUAL` is a list
    # chosen from what voiceterm happens to use; another Qt app will use
    # elements that are not in it, and those vanish from the outline with
    # no complaint — a port built from a silently incomplete spec is worse
    # than no spec, because it looks finished. Anything capitalised that
    # was neither rendered nor recognised as machinery is reported here.
    if skipped:
        lines.append("# NOT RENDERED (add to VISUAL if these hold layout): "
                     + ", ".join(k + "x" + str(v)
                                 for k, v in sorted(skipped.items(), key=lambda kv: -kv[1])))
        lines.append("")
    body = []
    render(roots, body)
    if max_depth is not None:
        body = [b for b in body if (len(b) - len(b.lstrip())) // 2 <= max_depth]
    lines.extend(body)
    return "\n".join(lines)


def main(argv) -> int:
    if len(argv) < 2:
        print(__doc__)
        print("usage: qml_outline.py <file.qml|dir> [--depth N]", file=sys.stderr)
        return 2
    target = Path(argv[1])
    depth = None
    if "--depth" in argv:
        depth = int(argv[argv.index("--depth") + 1])
    region = None
    if "--region" in argv:
        region = argv[argv.index("--region") + 1]
    files = sorted(target.glob("*.qml")) if target.is_dir() else [target]
    for f in files:
        print(outline_file(f, depth, region))
        print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
