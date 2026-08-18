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
    than QDialog, and stopping at the Qt classes would miss all of them.

    A source file may opt ITSELF out with a `parity-audit: not-a-surface`
    comment. That exists for a component the original itself does not use —
    gitmind's GestureHost.qml is 27 lines of press/release delta arithmetic
    with a threshold, referenced by nothing in the Qt app, and the kit's
    GestureLayer already does the same job better (detents, barrier fraction,
    takeover lock). Porting it would re-solve a solved problem and add a second
    gesture implementation to a tree that has one.

    The marker goes in the FILE rather than a list here, for the same reason
    the engine column's service opt-out does: an exception list inside the tool
    rots the first time a file moves, and the reasoning belongs where the next
    reader of that file will see it."""
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
                # A file may opt itself out — see this function's docstring.
                try:
                    head = open(p, encoding='utf8', errors='replace').read(400)
                except Exception:
                    head = ''
                if 'parity-audit: not-a-surface' in head:
                    continue
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
                # `App` is deliberately NOT in this list. It names the
                # top-level application root — ambient-learner's iced
                # `AmbientLearnerApp` — which every toolkit has one of and no
                # port can share: the egui version has its own root by
                # definition. Asking for it produced a permanent 94% on an app
                # whose surfaces were all ported.
                for m in re.finditer(r'pub struct\s+(\w+(?:View|Page|Panel|Widget|Dialog|Screen|Tab|Bar|Overlay))\b', txt):
                    found.add(m.group(1))
                # The filename rule, but only when the function actually takes
                # a UI CONTEXT. Matching the name alone made two false
                # surfaces that sat on the "next surfaces" list for weeks:
                #
                #   bo-suite's lyrics_transcriber.rs has
                #     pub fn render(phrases, template, title, artist) -> String
                #   which renders TEXT from a whisper transcription and has no
                #   Qt or egui in it at all — the audit asked for a
                #   "LyricsTranscriber" surface that never existed.
                #
                # A drawing function takes something to draw into. Requiring
                # that in the signature separates a view from any code that
                # happens to use the word render.
                # The context types of EVERY toolkit in this tree, not just
                # the one I had in mind. Requiring egui/iced types alone took
                # bot-arena and fao-codec to ZERO surfaces — both are
                # GTK/libadwaita and draw with `cr: &cairo::Context` or build
                # with `app: &adw::Application`. The comment six lines up says
                # a detector that silently sees nothing is worse than one that
                # over-counts, and I had just made that exact mistake while
                # fixing an over-count.
                if re.search(r'fn\s+\w*(?:ui|draw|render|show|build_ui)\s*(?:<[^>]*>)?\s*\('
                             r'[^)]*(?:&mut\s+(?:egui::)?Ui|&mut\s+Frame|Renderer|'
                             r'Painter|&mut\s+Canvas|cairo::Context|'
                             r'adw::Application|gtk::|&Context)', txt):
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

def kit_use(pdirs):
    """Which of the kit's SHARED-BY-DESIGN facilities a port actually uses.

    A third axis, after surface reachability and engine linkage, and the one
    that answers "the ports look about the same and don't function". An app can
    reach every surface and link its engine while reproducing its Qt original's
    navigation widget for widget — which is what makes it look unchanged.

    Only four modules are counted, chosen because each one's own header says it
    exists to be shared:

      a11y          "accessibility voice, shared across every uikit-egui app"
      nav           "extracted from snapclone into the shared kit"
      adaptive      one layout description driving phone and desktop
      gesture_layer the paging/swipe layer

    Reported as counts, NOT as a verdict. A terminal legitimately owns its
    gestures; nav's two-level page model does not fit every app. The number
    that means something is the comparison: how many files hand-roll pointer
    deltas while these sit unused.
    """
    hits = {}
    for key, pat in (("a11y", r"a11y::"), ("nav", r"nav::Nav|uikit_egui::nav"),
                     ("adaptive", r"adaptive::"), ("gesture", r"GestureLayer|gesture_layer")):
        n = 0
        for pd in pdirs:
            for root, dirs, files in os.walk(pd):
                dirs[:] = [x for x in dirs if x != "target"]
                for f in files:
                    if not f.endswith(".rs"):
                        continue
                    try:
                        body = open(os.path.join(root, f), encoding="utf-8",
                                    errors="ignore").read()
                    except OSError:
                        continue
                    if re.search(pat, body):
                        n += 1
        hits[key] = n
    return hits


def engine_link(app_dir, pdirs):
    """Does the port link this app's OWN engine crates?

    The column the audit was missing, and the one that explains an app scoring
    100% while doing nothing. `named`, `code` and `live` all ask whether a
    SURFACE is reachable — whether some file mentions it and something calls
    that file. None asks whether the port is connected to the thing the app
    actually does.

    bo-suite is the case that motivated it: 47 surfaces, 100% named, 396 tests,
    and its egui frontend depends on config-kit and three UI kits — not
    bo-core, not bo-mux, not bo-controller. It renders the whole application
    and cannot cancel a single decibel, because the ANC engine is not in its
    dependency graph.

    An app with no sibling crates returns (None, None) — n/a rather than
    passing, since there is nothing to link and a clean result would be a
    false clean bill.
    """
    sibs = []
    for name in sorted(os.listdir(app_dir)):
        sub = os.path.join(app_dir, name)
        manifest = os.path.join(sub, "Cargo.toml")
        if not os.path.isdir(sub) or not os.path.isfile(manifest):
            continue
        if any(os.path.abspath(sub) == os.path.abspath(pd) for pd in pdirs):
            continue
        try:
            txt = open(manifest, encoding="utf-8", errors="ignore").read()
        except OSError:
            continue
        m = re.search(r'(?m)^name\s*=\s*"([^"]+)"', txt)
        if not m:
            continue
        # A SERVICE is not a missing link. camera-bridge has main.rs, a
        # server, a client and a TLS protocol: it streams frames from the
        # phone to a desktop PipeWire node, and a UI crate linking it would
        # pull gstreamer and tract-onnx to reach something it should talk to
        # over a socket. snapclone's own camera_source.rs says capture "stays
        # in the host, because a UI crate that opens /dev/video0 cannot be
        # tested" — the port is right and the audit was wrong.
        #
        # A binary with no library is unlinkable, full stop.
        #
        # Checked against the FILESYSTEM, not just the manifest. bo-controller
        # declares neither [[bin]] nor [lib]: it has src/main.rs and cargo
        # infers the binary by convention, so a manifest-only test called it
        # linkable and reported the Qt app as a missing dependency of the egui
        # port. Cargo's own rule is the convention plus the declaration, and
        # this has to match it.
        has_bin = "[[bin]]" in txt or os.path.isfile(os.path.join(sub, "src", "main.rs"))
        has_lib = "[lib]" in txt or os.path.isfile(os.path.join(sub, "src", "lib.rs"))
        if has_bin and not has_lib:
            continue
        # A crate that ships BOTH — camera-bridge has lib.rs and main.rs — is
        # linkable in principle, and whether the port should link it is a
        # design decision no heuristic can make. So it is DECLARED, in the
        # crate itself:
        #
        #     # parity-audit: service
        #
        # camera-bridge is the case: it streams frames phone-to-desktop over
        # TLS into a PipeWire node, and a UI crate linking it would pull
        # gstreamer and tract-onnx to reach something it talks to over a
        # socket. An opt-out in the manifest keeps the judgement next to the
        # crate instead of in a list inside this tool, where it would rot.
        if re.search(r"(?m)^\s*#\s*parity-audit:\s*service\b", txt):
            continue
        sibs.append(m.group(1))
    if not sibs:
        return None, None
    # TRANSITIVE. fao-codec's frontend links fao_codec_lib, which itself
    # depends on fao-codec-core — the engine IS reachable, and reporting it
    # missing because the edge is one hop away is a false positive that would
    # send someone to add a dependency that already exists. So a sibling
    # counts as linked if any DIRECTLY linked sibling depends on it.
    def deps_of(pkg):
        for nm in sorted(os.listdir(app_dir)):
            man = os.path.join(app_dir, nm, "Cargo.toml")
            if not os.path.isfile(man):
                continue
            try:
                body = open(man, encoding="utf-8", errors="ignore").read()
            except OSError:
                continue
            m2 = re.search(r'(?m)^name\s*=\s*"([^"]+)"', body)
            if m2 and m2.group(1) == pkg:
                return body
        return ""

    linked, missing = [], []
    for pd in pdirs:
        try:
            dep_txt = open(os.path.join(pd, "Cargo.toml"),
                           encoding="utf-8", errors="ignore").read()
        except OSError:
            continue
        for pkg in sibs:
            stem = re.escape(pkg).replace(r"\-", "[-_]")
            if re.search(r"(?m)^\s*" + stem + r"\s*=", dep_txt):
                linked.append(pkg)
            else:
                missing.append(pkg)
    # Close over one hop: anything a linked sibling itself pulls in.
    reached = set(linked)
    for pkg in list(linked):
        body = deps_of(pkg)
        for cand in sibs:
            stem = re.escape(cand).replace(r"\-", "[-_]")
            if re.search(r"(?m)^\s*" + stem + r"\s*=", body):
                reached.add(cand)
    return sorted(reached), sorted(set(missing) - reached)


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

def mentions(name, text):
    """Does `text` name this surface, allowing for Rust dropping Qt's suffix?

    The single place that answers "is this surface referred to here". It lived
    in two of the three callers with slightly different bodies, and the
    disagreement was visible in the output: MicrotermView matched in `code`
    but not in `named`, then matched in `named` but not in `live`, appearing
    first as unported work and then as stranded work — both times finished.
    """
    if name in text or snake(name) in text:
        return True
    # A Qt class ending in View/Widget/Page/Dialog/Panel/Window also matches
    # its stem: in Rust the suffix is Qt's noise rather than part of the name,
    # and `microterm.rs` is the complete port of `MicrotermView`.
    # `Tab` belongs here and its absence cost a duplicated port. logscope's
    # GTK CargoTab is fully ported as tabs/cargo.rs — same primary-span rule,
    # same rendered fallback, a worker thread and its own unit test — and the
    # audit called it missing because the literal string "CargoTab" appears
    # nowhere in a tree that organises tabs as tabs/<name>.rs. I believed the
    # tool and wrote the port again.
    for suffix in ('View', 'Widget', 'Page', 'Dialog', 'Panel', 'Window', 'Tab'):
        if name.endswith(suffix) and len(name) > len(suffix):
            stem = name[:-len(suffix)]
            if stem in text or snake(stem) in text:
                return True
    return False


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
    reintroducing the transliteration bias.

    The trailing-noun rule below is shared with `covered_in_code` rather than
    living only there, which is where it used to be. That asymmetry made the
    columns contradict each other: rusty-board's MicrotermView is ported as
    `Microterm` — Rust drops Qt's View suffix — so it counted as in CODE while
    counting as never NAMED, and appeared on the worklist as missing work that
    was finished. `code` is meant to be the stricter reading of `named`, so it
    cannot recognise something `named` does not."""
    return mentions(name, text)


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
                if mentions(name, t):
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
            "engine": engine_link(d, port_dirs(d)),
            "kit": kit_use(port_dirs(d)),
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
    # Reported separately because it is a different failure from an unported
    # surface: the port is COMPLETE and inert.
    for r in rows:
        linked, missing = r.get("engine", (None, None))
        if missing:
            tail = (" (links " + ", ".join(linked) + ")") if linked else " (links none)"
            print(r["app"] + ": NOT LINKED to its own engine — "
                  + ", ".join(missing[:6]) + tail)
    # Kit adoption, reported last because it is the softest of the three and
    # the easiest to misread as a defect.
    print()
    print("kit facilities that exist to be shared — files in each port using them:")
    for r in rows:
        k = r.get("kit", {})
        if not k:
            continue
        used = " ".join(f"{name}={n}" for name, n in k.items())
        print(f"  {r['app']:<16} {used}")

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
