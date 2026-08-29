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
}

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


def parse(text: str):
    """Structural read. Brace depth is the tree; indentation is not trusted."""
    roots = []
    stack = []
    pending = None

    for lineno, raw in enumerate(text.splitlines(), 1):
        line = raw.split("//")[0]
        m = ELEMENT_RE.match(line)
        if m:
            kind = m.group(2)
            node = Node(kind=kind, line=lineno)
            visual = kind in VISUAL
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
            pm = PROP_RE.match(line)
            if pm:
                key, val = pm.group(1), pm.group(2).rstrip(";").strip()
                if key in KEEP:
                    pending.props[key] = val
                elif key == "id":
                    pending.ident = val

        for _ in range(line.count("}") - line.count("{")):
            if stack:
                stack.pop()
            pending = None


    return roots


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


def outline_file(path: Path, max_depth=None) -> str:
    text = path.read_text(errors="replace")
    roots = parse(text)
    counts = summarise(roots)
    total = sum(counts.values())
    top = sorted(counts.items(), key=lambda kv: -kv[1])[:8]
    lines = [
        "# " + path.name + " — " + str(len(text.splitlines())) + " lines of QML",
        "# " + str(total) + " visual elements",
        "# most used: " + ", ".join(k + "x" + str(v) for k, v in top),
        "",
    ]
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
    files = sorted(target.glob("*.qml")) if target.is_dir() else [target]
    for f in files:
        print(outline_file(f, depth))
        print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
