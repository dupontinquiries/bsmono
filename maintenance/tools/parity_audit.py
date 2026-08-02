#!/usr/bin/env python3
"""parity_audit.py — one command, every egui port, no hand-maintained ledger.

The parity docs are written by hand per app, so they drift the moment anyone
lands a widget without editing the table. This derives the state instead:

  * what SURFACES the original has (Qt widgets, QML pages, GTK views, Rust UI
    modules), found by naming convention per toolkit;
  * which of them the port names anywhere in its source;
  * whether the port builds, has a deployable binary, and is catalogued in the
    manifest — the three ways a port can be "done" and still not reach anyone.

The output is a table you can regenerate after any change, so parity stops being
a claim and becomes a measurement.
"""
import os, re, subprocess, sys, json

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))

# Ports live in egui-frontend/ BY CONVENTION, not by rule. krapper had no egui
# build at all and this list never said so, because a missing directory reads
# the same as an app that was never meant to have one. The companion check
# below walks every app and reports which have egui ANYWHERE, so an app with no
# port cannot hide by not having the directory the audit looks in.
PORTS = [
    ("bo-suite",    "apps/bo-suite"),
    ("bot-arena",   "apps/bot-arena"),
    ("fao-codec",   "apps/fao-codec"),
    ("rusty-board", "apps/rusty-board"),
    ("snapclone",   "apps/snapclone"),
    ("voiceterm",   "apps/voiceterm"),
    ("krapper",     "apps/krapper"),
]

def surfaces(app_dir):
    """Every UI surface the original exposes, by toolkit convention.

    Qt/C++: a class that INHERITS a widget base, plus the older
      Widget/Dialog/Page/View/Panel/Bar/Overlay suffix rule as a fallback.
    QML: one surface per .qml file.
    GTK/Rust: a pub struct or a `mod` under src/ui.

    The suffix rule alone was undercounting badly. snapclone has 94 Qt classes
    and the suffix list could see 38 of them, so ContactCard, ShutterButton,
    ExportReminderCard, SegmentedTabStrip, ModalSurface, SmartRibbon and
    SnapTile were not merely uncovered -- they were not in the DENOMINATOR, and
    every percentage in the table was inflated by their absence. A port cannot
    be measured against surfaces the measurer cannot see.

    Inheritance is the better signal because it separates UI from services
    without a vocabulary of suffixes: ContactCard : public QWidget is a surface,
    NetworkManager : public QObject is not. The pass is transitive, since the
    app defines its own bases -- most dialogs here inherit MobileDialog rather
    than QDialog, and stopping at the Qt classes would miss all of them."""
    found = set()
    inherits = {}          # base class -> {subclasses}
    for root, dirs, files in os.walk(app_dir):
        dirs[:] = [d for d in dirs if d not in ('target', 'build', '.git', 'egui-frontend')]
        for f in files:
            p = os.path.join(root, f)
            # QML only — .html files under a docs or test tree are fixtures,
            # not UI surfaces, and voiceterm's showed up as "missing surfaces"
            # which is nonsense.
            if f.endswith('.qml'):
                found.add(f[:-4])
            elif f.endswith(('.h', '.hpp')):
                try: txt = open(p, encoding='utf8', errors='replace').read()
                except Exception: continue
                for m in re.finditer(r'class\s+(\w+(?:Widget|Dialog|Page|View|Panel|Bar|Overlay))\b', txt):
                    found.add(m.group(1))
                # Inheritance, which catches the families the suffix list
                # cannot name. Recorded as edges and resolved below, because a
                # base may be declared in a header we have not read yet.
                for m in re.finditer(r'class\s+(\w+)\s*:\s*public\s+(\w+)', txt):
                    inherits.setdefault(m.group(2), set()).add(m.group(1))
            elif f.endswith('.rs'):
                # Not just src/ui/: bot-arena keeps its views in src/*.rs and
                # fao-codec under fao_app/, so restricting to a ui/ directory
                # found ZERO surfaces in both and reported them as 0% — a
                # detector that silently sees nothing is worse than one that
                # over-counts, because it looks like a finished port.
                rp = p.replace(os.sep, '/')
                if '/tests/' in rp or '/fixtures/' in rp or rp.endswith('_test.rs'):
                    continue
                try: txt = open(p, encoding='utf8', errors='replace').read()
                except Exception: continue
                # a UI surface, not every struct in the codebase: something
                # that draws or holds view state
                for m in re.finditer(r'pub struct\s+(\w+(?:View|Page|Panel|Widget|Dialog|Screen|Tab|Bar|Overlay|App))\b', txt):
                    found.add(m.group(1))
                if re.search(r'fn\s+\w*(?:ui|draw|render|show)\s*\(', txt):
                    found.add(''.join(w.capitalize() for w in f[:-3].split('_')))

    # Everything reachable from a Qt widget base is a surface. Transitive, so
    # ContactEditorDialog : public MobileDialog : public QDialog counts.
    QT_UI_BASES = {
        'QWidget', 'QDialog', 'QMainWindow', 'QFrame', 'QLabel', 'QPushButton',
        'QAbstractButton', 'QScrollArea', 'QToolButton', 'QGroupBox',
        'QStackedWidget', 'QListWidget', 'QGraphicsView', 'QOpenGLWidget',
        'QQuickItem', 'QQuickPaintedItem',
    }
    # Qt's own classes are not surfaces of this app. They arrive via the suffix
    # rule (QWidget, QDialog, QProgressBar, QStackedWidget all end in one of the
    # magic words) and pad the denominator with seven things no port can ever
    # cover. The convention is Q followed by an uppercase letter, which
    # correctly spares snapclone's own QrScanDialog.
    found = {n for n in found if not re.match(r'^Q[A-Z]', n)}

    frontier = [b for b in QT_UI_BASES if b in inherits]
    seen_bases = set()
    while frontier:
        base = frontier.pop()
        if base in seen_bases:
            continue
        seen_bases.add(base)
        for sub in inherits.get(base, ()):
            found.add(sub)
            frontier.append(sub)
    return found

def port_text(app_dir):
    out = []
    src = os.path.join(app_dir, 'egui-frontend')
    for root, dirs, files in os.walk(src):
        dirs[:] = [d for d in dirs if d != 'target']
        for f in files:
            if f.endswith(('.rs', '.md', '.toml')):
                try: out.append(open(os.path.join(root, f), encoding='utf8', errors='replace').read())
                except Exception: pass
    return '\n'.join(out)

def snake(name):
    return re.sub(r'(?<!^)(?=[A-Z])', '_', name).lower()

def covered(name, text):
    """Does the port mention this surface? CamelCase or snake_case, since a
    ported widget usually keeps its name in one form or the other.

    Both error directions are real and neither is fixable by name matching:

      * FALSE NEGATIVE. snapclone's contacts.rs had ContactPickerDialog and
        ContactEditorDialog fully ported, with tests, under Rust names
        (`Picker`, `Editor`). The audit called both pending for months. A port
        that renames things idiomatically -- which is the right thing to do --
        scores worse than one that transliterates.
      * FALSE POSITIVE. A doc comment saying "unlike CameraWidget" is a match.
        The number is therefore a floor on what has been NAMED, not a measure
        of what works.

    So read the percentage as a worklist, not a score, and read the tests to
    find out whether a surface actually behaves. The next revision should look
    for a surface's name in a `fn *_ui`/`draw` signature or a test name rather
    than anywhere in the file, which would kill most false positives without
    reintroducing the transliteration bias."""
    return name in text or snake(name) in text

def manifest_state(app_id):
    p = os.path.join(ROOT, 'manifest', 'projects.toml')
    try: t = open(p, encoding='utf8').read()
    except Exception: return (False, False, '')
    m = re.search(r'^\[apps\.' + re.escape(app_id) + r'\]\n(.*?)(?=^\[|\Z)', t, re.S | re.M)
    if not m: return (False, False, '')
    body = m.group(1)
    role = (re.search(r'role\s*=\s*"([^"]*)"', body) or [None, ''])[1]
    return (True, 'primary = true' in body, role)

def cargo_state(app_dir):
    tm = os.path.join(app_dir, 'egui-frontend', 'Cargo.toml')
    try: t = open(tm, encoding='utf8').read()
    except Exception: return (False, False)
    return ('[[bin]]' in t, 'default = ["app"]' in t)

def main():
    rows = []
    for name, rel in PORTS:
        d = os.path.join(ROOT, rel)
        if not os.path.isdir(os.path.join(d, 'egui-frontend')): continue
        surf = surfaces(d)
        text = port_text(d)
        hit = {s for s in surf if covered(s, text)}
        has_bin, app_default = cargo_state(d)
        in_man, primary, role = manifest_state(name + '-egui')
        rows.append({
            "app": name, "surfaces": len(surf), "covered": len(hit),
            "pct": round(100.0 * len(hit) / max(len(surf), 1)),
            "bin": has_bin, "app_default": app_default,
            "manifest": in_man, "primary": primary, "role": role,
            "missing": sorted(surf - hit)[:8],
        })
    w = max(len(r["app"]) for r in rows) if rows else 10
    print(f"{'app':<{w}}  {'surfaces':>8} {'covered':>7} {'%':>4}  bin  app-def  manifest  primary")
    for r in rows:
        print(f"{r['app']:<{w}}  {r['surfaces']:>8} {r['covered']:>7} {r['pct']:>3}%  "
              f"{'yes' if r['bin'] else ' no':>3}  {'yes' if r['app_default'] else ' no':>6}  "
              f"{'yes' if r['manifest'] else ' NO':>7}  {'yes' if r['primary'] else '  -':>6}")
    print()
    for r in rows:
        if r["missing"]:
            print(f"{r['app']}: next surfaces — {', '.join(r['missing'][:6])}")
    # Apps with no port at all — the gap the table above cannot show.
    apps = os.path.join(ROOT, 'apps')
    missing = []
    for name in sorted(os.listdir(apps)):
        d = os.path.join(apps, name)
        if not os.path.isdir(d): continue
        if any(p[0] == name for p in PORTS): continue
        hit = False
        for root, dirs, files in os.walk(d):
            dirs[:] = [x for x in dirs if x != 'target']
            if 'Cargo.toml' in files:
                try:
                    if 'egui' in open(os.path.join(root, 'Cargo.toml'), encoding='utf8').read():
                        hit = True; break
                except Exception: pass
        if not hit: missing.append(name)
    if missing:
        print("\nNO egui anywhere (daemons are expected here):")
        for m in missing: print(f"  {m}")

    if '--json' in sys.argv:
        json.dump(rows, open(os.path.join(ROOT, 'maintenance', 'parity.json'), 'w'), indent=1)
        print("\n-> maintenance/parity.json")

def why(surface):
    """Show WHERE a surface was counted as covered, file and line.

    covered() matches the name anywhere in the port, so a doc comment reading
    "unlike CameraWidget" scores the same as a ported widget. That false
    positive cannot be regexed away without also punishing ports that rename
    things idiomatically -- which is the right thing to do and which this tool
    already penalised for months.

    So instead of a cleverer guess, make the claim checkable: print every match
    with its location and let a human spend ten seconds on it. A number nobody
    can audit is how a port ends up reported as done.
    """
    needles = (surface, snake(surface))
    hits = 0
    for app, d in PORTS:
        src = os.path.join(ROOT, d, 'egui-frontend')
        for root, dirs, files in os.walk(src):
            dirs[:] = [x for x in dirs if x != 'target']
            for f in sorted(files):
                if not f.endswith(('.rs', '.md', '.toml')):
                    continue
                fp = os.path.join(root, f)
                try: lines = open(fp, encoding='utf8', errors='replace').readlines()
                except Exception: continue
                for n, line in enumerate(lines, 1):
                    if any(x in line for x in needles):
                        rel = os.path.relpath(fp, ROOT)
                        # Flag the weak matches rather than hiding them.
                        kind = 'comment' if line.lstrip().startswith(('//', '#', '*')) else 'code'
                        print(f"  [{kind:7}] {rel}:{n}: {line.strip()[:100]}")
                        hits += 1
    if not hits:
        print(f"  no match for {surface} in any port")

if __name__ == '__main__':
    if '--why' in sys.argv:
        i = sys.argv.index('--why')
        if i + 1 < len(sys.argv):
            why(sys.argv[i + 1])
        else:
            print("usage: parity_audit.py --why SurfaceName")
    else:
        main()
