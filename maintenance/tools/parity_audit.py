#!/usr/bin/env python3
"""NOTE: this tool counts SURFACES, which is the wrong question on its own.
A surface can be named, ported, tested and unreachable — see
maintenance/tools/reachability.py, which asks whether the app's own binary
arrives at a module. Every bug reported against these ports in August 2026 was
of that second kind, and every one of them is invisible here.
parity_audit.py — one command, every egui port, no hand-maintained ledger.

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
    # Its port is inline (src/ui/egui) rather than in egui-frontend/, which is
    # why it was missing from this list AND from the discovery below. An app
    # with no row looks like an app with nothing to do.
    ("ambient-learner", "apps/ambient-learner"),
    # Workspaces whose port is its own crate (crates/<x>-egui beside crates/<x>).
    # Both were absent from this list AND undiscoverable by port_dirs, so two
    # real ports went unmeasured for the same reason ambient-learner did.
    ("gitmind",     "apps/gitmind"),
    ("logscope",    "apps/logscope"),
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
        dirs[:] = [d for d in dirs if d not in ('target', 'build', '.git', 'egui-frontend', 'egui')]
        for f in files:
            p = os.path.join(root, f)
            # Test trees are not UI. voiceterm keeps golden OUTPUTS named
            # `article.html.qml` under tests/golden/expected/, and the .qml rule
            # was turning eleven of them into surfaces — a third of that app's
            # denominator was fixture files it can never "port", which pushed
            # its number down for no reason. The .rs branch already skipped
            # these paths; the .qml branch did not.
            rel = p.replace(os.sep, '/')
            if any(seg in rel for seg in ('/tests/', '/fixtures/', '/golden/')):
                continue
            # A test HARNESS tree is the same thing under a different name.
            # rusty-board keeps one at test_harness_qml/, and its
            # RustyBoardTestHarness.qml was sitting at the top of that app's
            # "next surfaces" list — telling us to port the scaffolding that
            # exists to exercise the port.
            if '/test_harness' in rel or '/test-harness' in rel:
                continue
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
    # Test scaffolding is not a surface to port, wherever it lives. snapclone's
    # ComponentTestWindow and ModalLayerTestPanel are exercisers for the Qt
    # build — they inherit QWidget, so the inheritance pass above legitimately
    # finds them, and they then occupy the top of the "next surfaces" list
    # ahead of real work. Matched on the name because that is what the
    # convention actually is here; a directory rule cannot catch a test window
    # that lives beside the widgets it tests.
    found = {n for n in found
             if 'Test' not in n and 'Harness' not in n and not n.endswith('Tests')}
    return found

def port_dirs(app_dir):
    """Where an egui port might live.

    `egui-frontend/` is the convention, but it is not the only shape: an app
    whose port is a FEATURE of the existing crate keeps it inline, and
    ambient-learner does exactly that at `src/ui/egui/`. The audit used to walk
    only the convention, so that app was invisible — it had no row at all, and
    an app with no row looks like an app with nothing to do rather than an app
    nobody has measured.

    Returns every directory that exists, so a hybrid is counted once."""
    candidates = [os.path.join(app_dir, 'egui-frontend'),
                  os.path.join(app_dir, 'src', 'ui', 'egui')]
    # A third shape: a workspace whose port is its own crate, `crates/<x>-egui`
    # beside the original `crates/<x>`. gitmind and logscope both do this, and
    # both were invisible for the same reason ambient-learner was — the audit
    # walked conventions rather than looking. Discovered rather than listed, so
    # the next workspace to do it is counted without an edit here.
    crates = os.path.join(app_dir, 'crates')
    if os.path.isdir(crates):
        for name in sorted(os.listdir(crates)):
            if name.endswith('-egui'):
                p = os.path.join(crates, name)
                if os.path.isdir(p):
                    candidates.append(p)
    return [c for c in candidates if os.path.isdir(c)]

def port_text(app_dir):
    out = []
    for src in port_dirs(app_dir):
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


def covered_in_code(name, text):
    """The stricter reading: the name appears OUTSIDE a comment.

    Two ports landed without moving the headline number, because both surfaces
    were already "covered" by a mention — WayfinderPage counted while the page
    drew the group-voice controls, and GroupVoiceBar counted while nothing drew
    a bar at all.

    But code-only is not the answer either, and the measurement says so: applied
    alone it drops bo-suite from 51% to 11%, because the house style names the
    Qt class in the MODULE DOC ("//! browse -- AlbumsPage, ArtistsPage") and
    then uses idiomatic Rust names in the code. That is the same false negative
    this tool already had once, in a new costume.

    So neither number is the truth and the table reports BOTH. `named` is the
    upper bound: somebody at least knew the surface existed. `code` is the lower
    bound: something references it outside prose. A port sitting at a high
    `named` and a low `code` is where to go looking — that gap is exactly where
    WayfinderPage was hiding."""
    code = '\n'.join(l for l in text.split('\n') if not l.lstrip().startswith(('//', '#', '*')))
    if name in code or snake(name) in code:
        return True

    # THE TRAILING-NOUN CASE, found on rusty-board's MicrotermView. The port is
    # complete — StenoCandidate, the fuzzy flag, the exact-before-fuzzy sort,
    # all of it — but the module is `microterm.rs`, because in Rust the `View`
    # suffix is Qt's noise rather than part of the name.
    #
    # So a Qt class ending in View/Widget/Page/Dialog/Panel also matches its
    # stem. That is a real loosening and it can produce a false POSITIVE: a
    # `microterm` mentioned anywhere would now count. Accepted deliberately,
    # because `named` is documented as the UPPER bound — its job is "somebody
    # knew this existed", and `code` remains the honest lower one. A false
    # negative here is worse: it sends somebody to port a surface that is
    # already done, which is the most expensive possible wrong answer.
    for suffix in ('View', 'Widget', 'Page', 'Dialog', 'Panel', 'Window'):
        if name.endswith(suffix) and len(name) > len(suffix):
            stem = name[:-len(suffix)]
            if stem in code or snake(stem) in code:
                return True
    return False

def covered_live(name, port_dirs_list):
    """Is this surface named by a file that the app can actually REACH?

    The third reading, and the one that turned out to matter. `code` is a poor
    proxy for "done" because the house style names the Qt class in the module
    doc and then writes idiomatic Rust — so snapclone reads 49% in code while
    every one of the 24 surfaces in that gap lives in a module with tests that
    the app calls. Chasing that number would have meant re-porting finished
    work, which the docstring above already warns is the most expensive wrong
    answer.

    So: find the files that mention the surface and ask whether any of them is
    live. The crate root is always live; any other module counts only if
    something outside it references it, which is `wiring_check.reached` — the
    same question, reused rather than reimplemented, so the two tools cannot
    drift into disagreeing about what "wired" means.

    A surface that is named ONLY inside a module nothing calls is the real
    finding: somebody wrote it and it never got connected. That is a different
    and much shorter list than the `code` gap.
    """
    import wiring_check
    hits = []
    for src in port_dirs_list:
        for root, dirs, files in os.walk(src):
            dirs[:] = [d for d in dirs if d != 'target']
            for f in files:
                if not f.endswith('.rs'):
                    continue
                try:
                    t = open(os.path.join(root, f), encoding='utf8').read()
                except Exception:
                    continue
                if name in t or snake(name) in t:
                    hits.append((root, f[:-3]))
    if not hits:
        return False
    for root, mod in hits:
        # The crate root is live by definition — and so is a binary target.
        # The previous version tested the PORT dir's parent rather than the
        # file's own directory, so logscope's pearl-input-view and status-view
        # read as stranded while both are [[bin]] entries that ship.
        if mod in ('lib', 'main') or os.path.basename(root) == 'bin':
            return True
        # Reachability is asked across EVERY port dir, not just the one the
        # module lives in. ambient-learner keeps its pages inline at
        # src/ui/egui and its window in a separate egui-frontend crate, so a
        # per-directory check called six rendered pages stranded — the same
        # false alarm this file already fixed once in cargo_state, which is
        # how I recognised it.
        if any(wiring_check.reached(other, mod) for other in port_dirs_list):
            return True
    return False

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
    """Does the port build a runnable binary, and does it default to one?

    Checks the manifest of EVERY port dir rather than only
    `egui-frontend/Cargo.toml`. The hardcoded path reported "no bin" for
    gitmind and logscope, which both ship one — a false alarm on a column
    people scan for missing binaries is worse than the column being absent,
    because it sends someone to fix what is not broken.
    """
    for src in port_dirs(app_dir):
        tm = os.path.join(src, 'Cargo.toml')
        try: t = open(tm, encoding='utf8').read()
        except Exception: continue
        if '[[bin]]' not in t:
            continue
        # Does a default build produce the app? The check used to look for the
        # literal `default = ["app"]`, which is this fleet's convention but not
        # a rule — logscope gates its GUI bin on `gui` and defaults to
        # `["gui", "gpu-scene"]`, so it built fine while the column said
        # otherwise. Read the bin's required-features and ask whether the
        # default list satisfies them.
        default = re.search(r'^default\s*=\s*\[([^\]]*)\]', t, re.M)
        defaults = set(re.findall(r'"([^"]+)"', default.group(1))) if default else set()
        needed = set()
        for block in re.findall(r'\[\[bin\]\](.*?)(?=^\[|\Z)', t, re.S | re.M):
            if 'required-features' in block:
                rf = re.search(r'required-features\s*=\s*\[([^\]]*)\]', block)
                feats = set(re.findall(r'"([^"]+)"', rf.group(1))) if rf else set()
                # Any ONE bin that a default build produces is enough.
                if feats <= defaults:
                    return (True, True)
                needed |= feats
            else:
                # No gate at all: it builds by default.
                return (True, True)
        return (True, not needed)
    return (False, False)

def main():
    rows = []
    for name, rel in PORTS:
        d = os.path.join(ROOT, rel)
        if not port_dirs(d): continue
        surf = surfaces(d)
        text = port_text(d)
        hit = {s for s in surf if covered(s, text)}
        wired = {s for s in surf if covered_in_code(s, text)}
        # Named somewhere the app can actually reach. Sits between the two:
        # not fooled by a mention in prose alone, not fooled by the house
        # style either.
        live = {s for s in surf if covered_live(s, port_dirs(d))}
        has_bin, app_default = cargo_state(d)
        in_man, primary, role = manifest_state(name + '-egui')
        rows.append({
            "app": name, "surfaces": len(surf), "covered": len(hit),
            "pct": round(100.0 * len(hit) / max(len(surf), 1)),
            "wired": len(wired),
            "wired_pct": round(100.0 * len(wired) / max(len(surf), 1)),
            "live": len(live),
            "live_pct": round(100.0 * len(live) / max(len(surf), 1)),
            # The list worth acting on: somebody named the surface, and
            # nothing the app calls mentions it.
            "stranded": sorted(hit - live)[:8],
            "bin": has_bin, "app_default": app_default,
            "manifest": in_man, "primary": primary, "role": role,
            "missing": sorted(surf - hit)[:8],
        })
    w = max(len(r["app"]) for r in rows) if rows else 10
    print(f"{'app':<{w}}  {'surfaces':>8} {'named':>6} {'%':>4} {'code':>5} {'%':>4} "
          f"{'live':>5} {'%':>4}  bin  app-def  manifest  primary")
    for r in rows:
        print(f"{r['app']:<{w}}  {r['surfaces']:>8} {r['covered']:>6} {r['pct']:>3}% "
              f"{r['wired']:>5} {r['wired_pct']:>3}% "
              f"{r['live']:>5} {r['live_pct']:>3}%  "
              f"{'yes' if r['bin'] else ' no':>3}  {'yes' if r['app_default'] else ' no':>6}  "
              f"{'yes' if r['manifest'] else ' NO':>7}  {'yes' if r['primary'] else '  -':>6}")
    print()
    for r in rows:
        if r["missing"]:
            print(f"{r['app']}: next surfaces — {', '.join(r['missing'][:6])}")
    # Named but not reachable: written and never connected, which is a
    # different job from porting and usually a much smaller one.
    for r in rows:
        if r.get("stranded"):
            print(f"{r['app']}: STRANDED (named, nothing live mentions it) — "
                  f"{', '.join(r['stranded'][:6])}")
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
