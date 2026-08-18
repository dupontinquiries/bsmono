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


def main():
    apps = os.path.join(ROOT, 'apps')
    rows = []
    for name in sorted(os.listdir(apps)):
        src = os.path.join(apps, name, 'egui-frontend', 'src')
        if not os.path.isdir(src): continue
        mods = [m for m in modules_of(src) if m not in ('util', 'config')]
        dead = [m for m in mods if not reached(src, m)]
        rows.append((name, len(mods), dead))
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
        src = os.path.join(apps, name, 'egui-frontend', 'src')
        if not os.path.isdir(src): continue
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
        src = os.path.join(apps, name, 'egui-frontend', 'src')
        if not os.path.isdir(src): continue
        hits = placeholder_feeds(src)
        if not hits: continue
        total_stub += len(hits)
        print(f"\n{name}: views fed placeholder data —")
        for rel, ln, why in hits:
            print(f"  {rel}:{ln}  {why}")
    if total_stub:
        print(f"\n{total_stub} views are wired to nothing. A view fed an empty "
              f"literal is a connected placeholder, and it is worse than an "
              f"unwired module because this checker would otherwise call it done.")
    return 1 if (total_dead or total_stub) else 0

if __name__ == '__main__':
    sys.exit(main())
