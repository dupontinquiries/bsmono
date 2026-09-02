#!/usr/bin/env python3
"""Standing chrome per egui port — how many panels are on screen at rest?

## Why

voiceterm had three left panels where the Qt original has one, and two of them
rendered the same `workspace.tabs` in the same frame with nothing switching
between them. An egui port grows panels one feature at a time and nothing counts
them, so "the port feels heavier than the original" is a thing you notice by eye
long after it is cheap to fix.

## The heuristic, and its correction

A panel is STANDING if no enclosing `if` guards it. The first version of this
looked six lines back for an `if`, which flagged all five of voiceterm's
remaining panels as unguarded — every one a false positive:

  * `workspaces` is inside `if self.workspaces_open {` twenty lines up;
  * `keycaps` is inside `if show_chrome {`, further still;
  * `tabbar` appears twice because it is built by an if/else and only shown once;
  * `tab-sessions` is nested INSIDE the tabbar closure, so it cannot outlive it.

A fixed window cannot see any of those. This tracks brace DEPTH instead: a panel
is guarded if any `if` is open at a shallower depth than the panel's own. Nested
panels are attributed to their parent rather than counted separately.

Not exact — it does not evaluate conditions, so a panel guarded by `if true` is
counted as guarded. It is a counter for eyeballing, and the number it reports is
"how many panels could be on screen at once", which is the question.
"""
import pathlib, re, collections, sys

ROOT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/home/claude/repo/apps")
PANEL = re.compile(r'egui::(SidePanel|TopBottomPanel)::(left|right|top|bottom)\("([^"]+)"\)')
IF = re.compile(r'^\s*(\}\s*)?(else\s+)?if\b|^\s*match\b')


def scan(path):
    """(id, side, guarded, nested) per panel.

    SKIPS `#[cfg(test)]` modules. A test that builds a panel to measure it is
    not chrome anybody sees, and counting it produced the tool's first false
    positive: voiceterm's headless layout test constructs an unguarded
    `SidePanel::left("tabbar")` to check its width, and the audit reported the
    shipped column — correctly gated twenty lines of source away — as standing.

    That is the failure mode this tool most needed to avoid. A counter that
    cries wolf on the one crate it was built for is a counter people stop
    reading, and the gate it flagged was intact.
    """
    out, depth, if_depths, panel_depths = [], 0, [], []
    test_depth = None          # brace depth at which a #[cfg(test)] block opened
    pending_test = False       # saw the attribute, waiting for its `{`
    for line in path.read_text(errors="replace").splitlines():
        stripped = line.strip()
        if "#[cfg(test)]" in stripped:
            pending_test = True
        if pending_test and "{" in stripped:
            test_depth = depth
            pending_test = False
        if test_depth is not None:
            # Inside a test module: advance the depth and skip everything else.
            depth += stripped.count("{") - stripped.count("}")
            if depth <= test_depth:
                test_depth = None
            continue
        if IF.match(line):
            if_depths.append(depth)
        m = PANEL.search(line)
        if m:
            # STRICTLY shallower: an `if` at depth d opens a block whose body
            # is depth d+1, so it guards this panel only if we are still inside
            # it. `<=` kept every `if` ever seen at this level and made the
            # check vacuous — removing a real guard changed nothing.
            guarded = any(d < depth for d in if_depths)
            nested = any(d < depth for d in panel_depths)
            out.append((m.group(3), m.group(2)[0], guarded, nested))
            panel_depths.append(depth)
        depth += stripped.count("{") - stripped.count("}")
        if_depths = [d for d in if_depths if d < depth]
        panel_depths = [d for d in panel_depths if d <= depth]
    return out


by_app = collections.defaultdict(list)
seen = set()
for pat in ("*/egui-frontend/src/**/*.rs", "*/crates/*-egui/src/**/*.rs"):
    for f in sorted(ROOT.glob(pat)):
        if f in seen:
            continue
        seen.add(f)
        ps = scan(f)
        if ps:
            by_app[f.relative_to(ROOT).parts[0]].extend(ps)

print(f"{'app':<14} {'panels':>6} {'standing':>9}   ids on screen at rest")
worst = []
for app in sorted(by_app):
    ps = by_app[app]
    # De-duplicate by id: an if/else builds one panel in two branches.
    standing = sorted({n for n, _, g, nest in ps if not g and not nest})
    worst.append((len(standing), app))
    print(f"{app:<14} {len({p[0] for p in ps}):>6} {len(standing):>9}   {', '.join(standing) or '—'}")

worst.sort(reverse=True)
if worst and worst[0][0] > 2:
    print(f"\n{worst[0][1]} carries the most standing chrome. Compare against its "
          f"original before adding another.")


# ── unowned global drag reads ────────────────────────────────────────────────
#
# A gesture built from `i.pointer.total_drag_delta()` sees EVERY drag on the
# surface, including one that a widget is already using. In logscope that made
# a finger selecting text in the composer read as a page swipe, which switched
# branch under the user and competed for the press that gives a text field
# focus — the "fields take one character on the Fairphone" report.
#
# `gesture_layer::drag_owner` is the shared answer and the fix was to call it.
# This finds the shape so the next one is found by reading rather than by a
# field report.
#
# NOT flagged: `response.drag_delta()`. That is a drag on a specific widget's
# own response, owned by construction — egui only reports it for the widget
# actually being dragged. Confusing the two would make this audit noise: four
# apps use `response.drag_delta()` correctly and none of them has the bug.

GLOBAL_DRAG = re.compile(r'i\.pointer\.(total_)?drag_delta\(\)')
OWNER_CHECK = re.compile(r'drag_owner\s*\(|GestureLayer|pager::|touch::')


def unowned_drags(root):
    """(file, line) for a global drag read with no ownership check nearby."""
    out = []
    for p in pathlib.Path(root).rglob("*.rs"):
        if "/target/" in str(p):
            continue
        try:
            lines = p.read_text(errors="ignore").split("\n")
        except OSError:
            continue
        for i, l in enumerate(lines):
            if l.lstrip().startswith("//") or not GLOBAL_DRAG.search(l):
                continue
            # Look for the guard in the surrounding block, not just this line —
            # the check is usually a few lines above where the delta is read.
            window = "\n".join(lines[max(0, i - 25):i + 5])
            if not OWNER_CHECK.search(window):
                out.append((str(p), i + 1, l.strip()[:90]))
    return out


if __name__ == "__main__" and "--drags" in sys.argv:
    found = unowned_drags(sys.argv[1] if len(sys.argv) > 2 else ".")
    for f, n, src in found:
        print(f"{f}:{n}\n    {src}\n    global drag read with no drag_owner/GestureLayer check nearby")
    print(f"\nunowned global drag reads: {len(found)}")
    print("`response.drag_delta()` is NOT flagged — that drag belongs to a widget by construction.")
    sys.exit(1 if found else 0)
