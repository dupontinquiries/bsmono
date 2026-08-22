#!/usr/bin/env python3
"""wiring_check.py — is a ported module actually CALLED by its app?

The parity audit counts modules that exist. This counts modules the app
reaches. The difference is the whole "dud demo" problem: snapclone had a
camera, an inbox, a composer, contacts, a conversation view and group voice —
six modules, 66 passing tests — and the binary called NONE of them. It drew a
placeholder rectangle labelled "live preview".

A module with tests and no caller is not a port. It is a library nobody uses,
and it looks exactly like an unfinished app because functionally it is one.
"""
import os, re, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))

def modules_of(src):
    """Modules declared in lib.rs."""
    try: t = open(os.path.join(src, 'lib.rs'), encoding='utf8').read()
    except Exception: return []
    return re.findall(r'^pub mod (\w+);', t, re.M)

def reached(src, mod):
    """Is `mod` referenced anywhere outside its own file?

    Aliases count. `use voiceterm_egui::mail as mp;` followed by `mp::inbox_ui`
    is a use of `mail`, and matching only `mail::` reported four wired modules
    as dead — the mirror of the re-export blind spot noted below. The alias is
    resolved per file, because two files may bind the same short name to
    different modules.
    """
    for root, dirs, files in os.walk(src):
        dirs[:] = [d for d in dirs if d != 'target']
        for f in files:
            if not f.endswith('.rs') or f == f'{mod}.rs': continue
            p = os.path.join(root, f)
            try: t = open(p, encoding='utf8').read()
            except Exception: continue
            # a use, a path call, or a field of that type
            if re.search(rf'\b{re.escape(mod)}::', t): return True
            # …or the same thing under a local alias, in either form:
            #   use crate::mail as mp;            → mp::
            #   use crate::{mail as mp, viz};     → mp::
            for alias in re.findall(rf'\b{re.escape(mod)}\s+as\s+(\w+)', t):
                if re.search(rf'\b{re.escape(alias)}::', t): return True
            # A #[macro_use] module is reached through its macros, which carry
            # no path at all. Its declaration in lib.rs IS the wiring.
            if re.search(rf'#\[macro_use\][^\n]*\n(?:#\[[^\]]*\]\n)*pub mod {re.escape(mod)};', t):
                return True
    return False

# Patterns that make a view LOOK wired while showing nothing. Every one of these
# passed the reachability check above, which is how 26 dead modules became 0 with
# no new functionality.
PLACEHOLDERS = [
    (r'Vec::new[(][)]\s*;\s*$', 'a view fed an empty Vec::new()'),
    (r'::new[(]&\[\],\s*&\[\][)]', 'a model built from empty slices'),
    (r'let\s+\w+\s*:\s*Vec<[^>]+>\s*=\s*Vec::new\(\)\s*;', 'a local empty list handed to a view'),
]

def placeholder_feeds(src):
    """Empty literals fed straight into a drawing call, in the same function."""
    hits = []
    for root, dirs, files in os.walk(src):
        dirs[:] = [d for d in dirs if d != 'target']
        for f in files:
            if not f.endswith('.rs'): continue
            p = os.path.join(root, f)
            try: lines = open(p, encoding='utf8').read().split('\n')
            except Exception: continue
            for i, line in enumerate(lines):
                for pat, why in PLACEHOLDERS:
                    if re.search(pat, line.strip()):
                        # only flag it when a draw call follows within a few lines
                        tail = '\n'.join(lines[i:i+6])
                        if re.search(r'_ui\(|render_|\.ui\(|show\(', tail):
                            hits.append((os.path.relpath(p, src), i + 1, why))
    return hits

def uncalled_renderers(src):
    """Drawing functions nothing calls.

    A finer question than `reached`: a module can be called for one of its
    functions while another — a whole page — is never drawn. Six real defects
    today were exactly that, in modules the module-level check called wired:

      bo-suite   queue_ui / now_playing_ui: the controller filled a queue no
                 screen showed, toast included.
      bo-suite   playlists_ui: state on the controller, apply_playlist handling
                 Create with its own test, and no tab to reach it.
      bo-suite   sources_ui: the device's inputs, beside an outputs page that
                 was drawn.
      snapclone  feed_ui: received snaps had nowhere to appear.
      snapclone  reticle_ui: a changeover that wired the state, the trigger and
                 the tick, and left the paint on the old timer.
      voiceterm  mail_view::list_ui in a pane: a Mail pane painting the word
                 "Mail" with the renderer one module away.

    A `pub fn` taking `&mut Ui` exists to put something on a screen. If no
    production code outside its own file calls it, nothing does.

    Test code is excluded on BOTH sides: a renderer called only from a test is
    exercised, not shipped, and that is the case worth reporting.
    """
    defs, texts = [], {}
    for dp, dn, fn in os.walk(src):
        dn[:] = [d for d in dn if d != 'target']
        for f in fn:
            if not f.endswith('.rs'): continue
            q = os.path.join(dp, f)
            try: texts[q] = open(q, encoding='utf8', errors='replace').read()
            except OSError: continue
    for q, txt in texts.items():
        prod = txt.split('#[cfg(test)]')[0]
        for m in re.finditer(r'pub fn (\w+)\s*(?:<[^>]*>)?\s*\([^)]*&mut\s+(?:egui::)?Ui', prod):
            # A function may DECLARE why it is uncalled, in the doc comment
            # directly above it. rusty-board's keyboard_ui_with_signal is the
            # case: an alternative to keyboard_ui for a caller that wants the
            # press on a shared signal, where the GTK side really does use
            # Signal<Key> — but nothing in the egui app owns a registry, and
            # one publisher with no listener is indirection rather than
            # architecture.
            #
            # The marker goes in the doc rather than a list here, for the same
            # reason parity_audit's service opt-out does: the reasoning belongs
            # where the next reader of that function will find it, and a list
            # inside the tool rots the first time something moves.
            doc_start = prod.rfind('\n\n', 0, m.start())
            if 'Not called yet, and that is not an oversight' in prod[doc_start:m.start()]:
                continue
            defs.append((q, m.group(1)))
    out = []
    for q, name in defs:
        called = False
        for r, u in texts.items():
            prod = u.split('#[cfg(test)]')[0]
            if r == q:
                # SAME-FILE CALLERS COUNT. Excluding them was wrong: krapper
                # defines its eframe::App impl beside writer_ui in one lib.rs
                # and calls it there, which is normal for a small crate and was
                # reported as never drawn. A definition is not a call, so the
                # definition line is skipped and anything else counts.
                prod = re.sub(r'pub fn ' + re.escape(name) + r'\s*(?:<[^>]*>)?\s*\(',
                              '', prod)
            if re.search(r'\b' + re.escape(name) + r'\s*\(', prod):
                called = True
                break
        if not called:
            out.append((os.path.relpath(q, src), name))
    return sorted(out)


# WHERE AN APP KEEPS ITS EGUI VIEWS.
#
# `egui-frontend/src` is the common layout and it was the only one this tool
# looked in, so any app that puts its views elsewhere reported "0 modules, 0
# DEAD" — which reads as CLEAN and means NOT SCANNED. ambient-learner has 11
# view files and 3,566 lines under `src/ui/egui/`, and every audit this tool
# has run has skipped all of it.
#
# Returning a list rather than a path because an app may have both, and
# checking one while a second exists is the same silence in a new place.
def egui_dirs(app_dir):
    out = []
    for rel in (('egui-frontend', 'src'), ('src', 'ui', 'egui')):
        d = os.path.join(app_dir, *rel)
        if os.path.isdir(d):
            out.append(d)
    return out


def tested_but_uncalled(src):
    """Public functions that only tests call.

    A different question from `uncalled_renderers`, which asks about DRAWING.
    This one asks about logic, and the case that motivated it is
    `TermWorkspace::close_newest_web_pane`: the real v0.1.881 implementation,
    documented, unit-tested, and reached from nothing but its own tests. The
    dispatch beside it still used the older path, so the behaviour the function
    exists to provide did not happen — and a second implementation of it got
    written before anyone noticed the first.

    That is the failure mode: a tested function looks finished. Coverage says
    it works, and it does; nothing says it is unreachable.

    Deliberately narrow. Only functions with at least one test mention and zero
    production mentions outside their own definition — a function nobody calls
    AND nobody tests is unfinished work, which is a different and more obvious
    thing.
    """
    texts, out = {}, []
    for dp, dn, fn in os.walk(src):
        dn[:] = [d for d in dn if d != 'target']
        for f in fn:
            if not f.endswith('.rs'): continue
            q = os.path.join(dp, f)
            try: texts[q] = open(q, encoding='utf8', errors='replace').read()
            except OSError: continue
    # Split every file once: production above the first #[cfg(test)], tests below.
    prod_all, test_all = [], []
    for txt in texts.values():
        parts = txt.split('#[cfg(test)]')
        prod_all.append(parts[0])
        test_all.extend(parts[1:])
    prod_joined, test_joined = '\n'.join(prod_all), '\n'.join(test_all)

    for q, txt in texts.items():
        prod = txt.split('#[cfg(test)]')[0]
        # MUTATORS ONLY, and the first cut proves why. Matching every `pub fn`
        # reported 416 functions across the fleet — every pure helper a view
        # inlines, every predicate reached through another path. A checker
        # that reports 416 things is one nobody reads, which is the connected-
        # placeholder failure applied to tooling.
        #
        # `&mut self` is the line that separates signal from noise. A pure
        # function nobody calls is an unused helper: harmless, and obvious
        # from its own body. A MUTATOR nobody calls is a state change the
        # application never performs — which is exactly
        # close_newest_web_pane, and exactly the shape worth a report.
        for m in re.finditer(r'pub fn (\w+)\s*(?:<[^>]*>)?\s*\(\s*&mut self', prod):
            name = m.group(1)
            call = name + '('
            prod_hits = prod_joined.count(call)
            # One hit is the definition itself.
            if prod_hits > 1: continue
            if test_joined.count(call) == 0: continue
            doc_start = prod.rfind('\n\n', 0, m.start())
            if 'Not called yet, and that is not an oversight' in prod[doc_start:m.start()]:
                continue
            out.append((os.path.relpath(q, src), name))
    return sorted(set(out))


def main():
    apps = os.path.join(ROOT, 'apps')
    rows = []
    for name in sorted(os.listdir(apps)):
        for src in egui_dirs(os.path.join(apps, name)):
            mods = [m for m in modules_of(src) if m not in ('util', 'config')]
            dead = [m for m in mods if not reached(src, m)]
            # Labelled by the directory when an app has more than one, so a
            # dead module can be found rather than merely counted.
            label = name if src.endswith(os.path.join('egui-frontend', 'src')) \
                else f"{name} (ui/egui)"
            rows.append((label, len(mods), dead))
    w = max((len(r[0]) for r in rows), default=8)
    print(f"{'app':<{w}}  modules  wired  DEAD")
    total_dead = 0
    for name, n, dead in rows:
        total_dead += len(dead)
        print(f"{name:<{w}}  {n:7}  {n-len(dead):5}  {len(dead)}")
    for name, _, dead in rows:
        if dead: print(f"\n{name}: never called — {', '.join(dead)}")
    if total_dead:
        print(f"\n{total_dead} ported modules are unreachable from their app.")

    # Called is not the same as DRAWN. A module can be reached for one function
    # while a whole page inside it is never painted.
    total_unrendered = 0
    for name in sorted(os.listdir(apps)):
      for src in egui_dirs(os.path.join(apps, name)):
        hits = uncalled_renderers(src)
        if not hits: continue
        total_unrendered += len(hits)
        print(f"\n{name}: renderers nothing calls —")
        for rel, fn in hits:
            print(f"  {rel:<28} {fn}")
    if total_unrendered:
        print(f"\n{total_unrendered} drawing functions are never called. Each is a "
              f"surface that exists and cannot be seen — CHECK each one before "
              f"acting: some are helpers a caller inlines, and a few today were "
              f"whole pages with no route in.")

    # Reachable is not the same as FED.
    total_stub = 0
    for name in sorted(os.listdir(apps)):
      for src in egui_dirs(os.path.join(apps, name)):
        hits = placeholder_feeds(src)
        if not hits: continue
        total_stub += len(hits)
        print(f"\n{name}: views fed placeholder data —")
        for rel, ln, why in hits:
            print(f"  {rel}:{ln}  {why}")
    # Reachable is not the same as CALLED, and a test is not a caller.
    total_orphan = 0
    for name in sorted(os.listdir(apps)):
      for src in egui_dirs(os.path.join(apps, name)):
        hits = tested_but_uncalled(src)
        if not hits: continue
        total_orphan += len(hits)
        print(f"\n{name}: tested but never called in production —")
        for rel, fn in hits:
            print(f"  {rel:<28} {fn}")
    if total_orphan:
        print(f"\n{total_orphan} function(s) are exercised only by their own tests. "
              f"A tested function LOOKS finished — coverage says it works, and it "
              f"does; nothing says it is unreachable. CHECK each: some are "
              f"library API for another crate, and some are a behaviour the app "
              f"was supposed to have.")

    if total_stub:
        print(f"\n{total_stub} views are wired to nothing. A view fed an empty "
              f"literal is a connected placeholder, and it is worse than an "
              f"unwired module because this checker would otherwise call it done.")
    return 1 if (total_dead or total_stub) else 0

if __name__ == '__main__':
    sys.exit(main())
