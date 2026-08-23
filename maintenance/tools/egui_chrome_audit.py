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
    """(id, side, guarded, nested) per panel."""
    out, depth, if_depths, panel_depths = [], 0, [], []
    for line in path.read_text(errors="replace").splitlines():
        stripped = line.strip()
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
