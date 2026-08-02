#!/usr/bin/env python3
"""desktop_check.py — do the .desktop entries and the binaries agree?

A .desktop file matches a running window by application id. Get them out of step
and the app runs perfectly with no icon, no dock grouping and the wrong name —
a failure nobody traces back to a string, because nothing errors.

Checks, in the order they bite:

1. every app with an egui frontend HAS an entry;
2. the entry's Icon= and StartupWMClass= equal its filename stem, which is the
   app id the compositor looks for;
3. the binary sets that same id (`with_app_id`, or the shared helper);
4. the entry has the keys a launcher needs — no Categories means no menu
   placement, Terminal=true on a GUI app opens a stray console.
"""
import os, re, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
APPS = os.path.join(ROOT, 'apps')
NEEDED = ["Type=Application", "Name=", "Exec=", "Icon=", "Categories="]

def entries(app):
    out = []
    for root, dirs, files in os.walk(os.path.join(APPS, app)):
        dirs[:] = [d for d in dirs if d != 'target']
        out += [os.path.join(root, f) for f in files if f.endswith('.desktop')]
    return out

def value(text, key):
    for line in text.splitlines():
        if line.startswith(key): return line[len(key):].strip()
    return None

def main():
    problems = 0
    for app in sorted(os.listdir(APPS)):
        src = os.path.join(APPS, app, 'egui-frontend', 'src')
        if not os.path.isdir(src): continue
        es = entries(app)
        if not es:
            print(f"{app}: NO .desktop entry — the app cannot appear in a menu")
            problems += 1
            continue

        # what the binary sets
        code = ""
        for root, dirs, files in os.walk(src):
            dirs[:] = [d for d in dirs if d != 'target']
            for f in files:
                if f.endswith('.rs'):
                    try: code += open(os.path.join(root, f), encoding='utf8').read()
                    except Exception: pass
        set_ids = set(re.findall(r'APP_ID:\s*&str\s*=\s*"([^"]+)"', code))
        set_ids |= set(re.findall(r'with_app_id\("([^"]+)"\)', code))

        matched = False
        for e in es:
            stem = os.path.basename(e)[:-len('.desktop')]
            text = open(e, encoding='utf8').read()
            for key in NEEDED:
                if key not in text:
                    print(f"{app}: {os.path.basename(e)} missing {key}")
                    problems += 1
            if value(text, "Terminal=") == "true":
                print(f"{app}: {os.path.basename(e)} has Terminal=true — a GUI app "
                      f"will open a stray console")
                problems += 1
            for key in ("Icon=", "StartupWMClass="):
                v = value(text, key)
                # A STOCK icon name (no dots) is legitimate: a helper tool that
                # wants the theme's camera-photo icon should not be forced to
                # ship its own. Only a reverse-DNS icon has to match the stem.
                if key == "Icon=" and v and '.' not in v:
                    continue
                if v is not None and v != stem:
                    print(f"{app}: {os.path.basename(e)} {key}{v} != filename stem {stem}")
                    problems += 1
            # A named icon that does not ship is a blank tile in the launcher.
            # Stock names (no dots) legitimately come from the theme.
            icon = value(text, "Icon=")
            if icon and '.' in icon:
                svg = os.path.join(APPS, app, 'packaging', 'icons', 'hicolor',
                                   'scalable', 'apps', icon + '.svg')
                if not os.path.isfile(svg):
                    print(f"{app}: {os.path.basename(e)} names icon '{icon}' and "
                          f"no packaging/icons/.../{icon}.svg ships — blank tile")
                    problems += 1

            # AppStream metadata — the file a software centre reads. Without it
            # an installed app is invisible in the one place most users look.
            # Looked for BESIDE the entry, not only in packaging/: snapclone
            # keeps a second entry under resources/, and a helper tool like
            # camera-view is not a software-centre app at all.
            meta = os.path.join(os.path.dirname(e), stem + '.metainfo.xml')
            if '.' not in stem:
                pass          # a helper, not a listed application
            elif not os.path.isfile(meta):
                print(f"{app}: no {stem}.metainfo.xml — invisible in software centres")
                problems += 1

            if stem in set_ids: matched = True

        if set_ids and not matched:
            print(f"{app}: binary sets {sorted(set_ids)} but no entry is named that")
            problems += 1
        elif not set_ids:
            print(f"{app}: binary sets NO app id — on Wayland the entry can never match")
            problems += 1

    print(f"\n{problems} desktop-integration problems" if problems
          else "\nevery egui app has a matching .desktop entry and app id")
    return 1 if problems else 0

if __name__ == '__main__':
    sys.exit(main())
