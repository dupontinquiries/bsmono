#!/usr/bin/env python3
"""install_models.py -- import processed models into the game, repeatably.

Turns the output of process_models.py (single-mesh IQMs) and texture_export.py
(per-part textured IQMs + manifests) into:

  engine_src/data/imported_*.iqm        model meshes
  engine_src/data/textures/*            their textures
  engine_src/imported_models.h          AUTO-GENERATED tables (included by game.c)

so adding or replacing models is a command, not a C edit.

TUNING IS PRESERVED. Per-model orientation/scale (the values hand-tuned in the
sandbox) live in model_tuning.json keyed by MODEL NAME, not by array index. Re-
importing, reordering or adding models never loses them; unknown models fall back
to the library-wide defaults measured earlier (ry=-90, scale=0.33).

Usage:
  python3 tools/install_models.py --iqm-dir iqm --parts-dir parts \
      --engine engine_src [--tuning model_tuning.json] [--limit 64]
  python3 tools/install_models.py --parts-dir parts --engine engine_src --dry-run
"""
import sys, os, json, shutil, argparse, re

DEF_RX, DEF_RY, DEF_RZ, DEF_SCL = 0.0, -90.0, 0.0, 0.33   # measured library defaults
TAG_ID = {"body": 0, "tire": 1, "tire?": 1, "fan": 2, "door": 3, "armor": 4}

def san(s):
    return re.sub(r"[^A-Za-z0-9_]", "_", s).strip("_").lower()

# ── categories ────────────────────────────────────────────────────────────
#
# The catalogue is one flat alphabetical list of g_imported_names, because the
# generated header has no category field and nothing in the engine reads one.
# Adding a real one means a UI change in game.c.
#
# This does it WITHOUT touching the engine: the category becomes a short
# prefix on the display name, and since the list is already sorted by name,
# the prefix groups it. Zero engine change, works on the build you have.
#
# A real `g_imp_cat[]` array is ALSO emitted (see below) so the UI can switch
# to proper categories later without re-running anything.

CATS = {
    "forge":  ("FORGE", "generated originals -- tools/forge"),
    "fixed":  ("FIXED", "re-exported with texture_export v13+ (merges all "
                        "pieces of a multi-file OBJ; v9 kept one)"),
    "raw":    ("OLD",   "as previously installed, not re-exported"),
    "ip":     ("IP",    "filename matches a rights holder -- do not ship"),
}
CAT_ORDER = ["forge", "fixed", "raw", "ip"]


def display_name(stem, width=28, cat=None):
    """Readable browser label. Kept longer than the old 20 chars so distinct
    models stop colliding (two identically-truncated licensed names did).

    With a category, the label is prefixed so the existing alphabetical sort
    groups it. Width is spent on the prefix deliberately: the screenshot shows
    names already truncating at 28, and knowing WHICH BUCKET a model is in
    matters more than the last four characters of its name."""
    s = re.sub(r"^imported_", "", stem)
    s = re.sub(r"[_\-]+", " ", s).strip().upper()
    if cat:
        tag = CATS[cat][0]
        return f"{tag} {s}"[:width]
    return s[:width]


def classify(name, ip_patterns, fixed_names, forge_names):
    """Which bucket a model falls in. IP wins over everything: a model that
    is both re-exported and rights-holder material is still unshippable."""
    n = name.lower()
    for rx in ip_patterns:
        if rx.search(n):
            return "ip"
    if name in forge_names:
        return "forge"
    if name in fixed_names:
        return "fixed"
    return "raw"


def load_ip_patterns(path):
    import re as _re
    pats = []
    if path and os.path.isfile(path):
        for line in open(path, errors="ignore"):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            try:
                pats.append(_re.compile(line, _re.I))
            except _re.error as e:
                print(f"  (skipping bad pattern {line!r}: {e})")
    return pats

def read_manifest(path):
    """[(part_file, texture, tag, axis), ...] from a *_parts.txt manifest."""
    out = []
    for line in open(path, errors="ignore").read().splitlines():
        p = line.split()
        if len(p) >= 2:
            tag = p[2] if len(p) > 2 else "body"
            try: axis = int(p[3]) if len(p) > 3 else -1
            except ValueError: axis = -1
            out.append((p[0], p[1], tag, axis))
    return out

def collect(iqm_dir, parts_dir):
    """{name: {'single': path|None, 'parts': [(file,tex,tag,axis)], 'dir': parts_dir}}"""
    models = {}
    if iqm_dir and os.path.isdir(iqm_dir):
        for f in sorted(os.listdir(iqm_dir)):
            if f.lower().endswith(".iqm"):
                name = os.path.splitext(f)[0]
                models.setdefault(name, {"single": None, "parts": []})
                models[name]["single"] = os.path.join(iqm_dir, f)
    if parts_dir and os.path.isdir(parts_dir):
        for f in sorted(os.listdir(parts_dir)):
            m = re.match(r"imported_(.+)_parts\.txt$", f)
            if not m: continue
            name = m.group(1)
            models.setdefault(name, {"single": None, "parts": []})
            models[name]["parts"] = read_manifest(os.path.join(parts_dir, f))
    return models

def load_tuning(path):
    if path and os.path.isfile(path):
        try: return json.load(open(path))
        except Exception as e: print("  (tuning file unreadable:", e, ")")
    return {}

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--iqm-dir", default=None, help="process_models output (single-mesh IQMs)")
    ap.add_argument("--parts-dir", default=None, help="texture_export output (parts + manifests)")
    ap.add_argument("--engine", default="engine_src", help="engine source dir")
    ap.add_argument("--tuning", default="model_tuning.json")
    ap.add_argument("--limit", type=int, default=0, help="cap how many models are installed")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--exclude-file", default=None,
                    help="IP pattern file (one regex per line); matches go in "
                         "the IP category rather than being installed silently")
    ap.add_argument("--fixed-list", default=None,
                    help="file of model names re-exported with texture_export "
                         "v13+, one per line -> FIXED category")
    ap.add_argument("--forge-dir", default=None,
                    help="tools/forge GLB output -> FORGE category")
    ap.add_argument("--no-categories", action="store_true",
                    help="old behaviour: plain names, no prefixes")
    a = ap.parse_args()

    if not a.iqm_dir and not a.parts_dir:
        print("give --iqm-dir and/or --parts-dir"); sys.exit(1)
    data_dir = os.path.join(a.engine, "data")
    tex_dir  = os.path.join(data_dir, "textures")

    # The generated header is only honoured if game.c includes it. Older checkouts
    # still carry hardcoded tables, in which case this tool would write a header
    # that nothing reads -- a silent no-op. Refuse instead.
    game_c = os.path.join(a.engine, "game.c")
    if os.path.isfile(game_c):
        try:
            src = open(game_c, errors="ignore").read()
        except Exception:
            src = ""
        if "imported_models.h" not in src:
            print("ERROR: %s does not #include \"imported_models.h\"." % game_c)
            print("  This engine predates the generated-table change, so the header")
            print("  this tool writes would be ignored and nothing would appear in game.")
            print("  Use the signatures-92 (or later) project, which has:")
            print('      #include "imported_models.h"')
            print("  in engine_src/game.c where the model tables used to be.")
            sys.exit(2)
    else:
        print(f"  (warning: {game_c} not found -- is --engine pointing at engine_src?)")

    models = collect(a.iqm_dir, a.parts_dir)
    if not models:
        print("no models found"); sys.exit(1)
    ip_pats = load_ip_patterns(a.exclude_file)
    fixed_names = set()
    if a.fixed_list and os.path.isfile(a.fixed_list):
        fixed_names = {l.strip() for l in open(a.fixed_list) if l.strip()}
    forge_names = set()
    if a.forge_dir and os.path.isdir(a.forge_dir):
        forge_names = {os.path.splitext(f)[0] for f in os.listdir(a.forge_dir)
                       if f.lower().endswith((".glb", ".iqm"))}

    def cat_of(nm):
        if a.no_categories:
            return None
        return classify(nm, ip_pats, fixed_names, forge_names)

    names = sorted(models, key=lambda nm: (
        CAT_ORDER.index(cat_of(nm)) if cat_of(nm) else 0, nm))
    if a.limit: names = names[:a.limit]
    print(f"{len(models)} model(s) found" + (f", installing {len(names)}" if a.limit else ""))

    tuning = load_tuning(a.tuning)
    kept = sum(1 for n in names if n in tuning)
    print(f"tuning: {len(tuning)} entries on file, {kept} match models being installed")

    if not a.dry_run:
        os.makedirs(data_dir, exist_ok=True); os.makedirs(tex_dir, exist_ok=True)

    src_tex = None
    for cand in ("textures", "tex", "Textures"):
        d = os.path.join(a.parts_dir or "", cand)
        if a.parts_dir and os.path.isdir(d): src_tex = d; break
    if a.parts_dir and not src_tex:
        print("  (warning: no textures/ or tex/ dir beside parts -- models will be untextured)")

    installed = []
    copied_tex = set()
    for n in names:
        m = models[n]
        rec = {"name": n, "single": None, "parts": []}
        if m["single"]:
            dst = os.path.join(data_dir, f"imported_{n}.iqm")
            if not a.dry_run: shutil.copy2(m["single"], dst)
            rec["single"] = f"data/imported_{n}.iqm"
        for (pf, tex, tag, axis) in m["parts"]:
            src = os.path.join(a.parts_dir, pf)
            if not os.path.exists(src): continue
            if not a.dry_run: shutil.copy2(src, os.path.join(data_dir, pf))
            # the part's textures: diffuse + any norm_/spec_/emit_ companions
            stem = os.path.splitext(tex)[0]
            for cand in (tex, "norm_"+stem+".png", "spec_"+stem+".png", "emit_"+stem+".png",
                         "norm_"+tex, "spec_"+tex, "emit_"+tex):
                if not src_tex: break
                s = os.path.join(src_tex, cand)
                if os.path.exists(s) and cand not in copied_tex:
                    if not a.dry_run: shutil.copy2(s, os.path.join(tex_dir, cand))
                    copied_tex.add(cand)
            rec["parts"].append((f"data/{pf}", tag, axis))
        if rec["single"] or rec["parts"]:
            installed.append(rec)

    if not installed:
        print("nothing installable (no .iqm found)"); sys.exit(1)

    # models with no single mesh still need one for the browser: use part 0
    for r in installed:
        if not r["single"] and r["parts"]:
            r["single"] = r["parts"][0][0]

    EX_MAX_TPARTS = 128
    clamped = [r["name"] for r in installed if len(r["parts"]) > EX_MAX_TPARTS]
    for r in installed:
        if len(r["parts"]) > EX_MAX_TPARTS:
            r["parts"] = r["parts"][:EX_MAX_TPARTS]
    if clamped:
        print(f"  !! {len(clamped)} model(s) exceed EX_MAX_TPARTS={EX_MAX_TPARTS} and were "
              f"clamped (extra parts dropped):")
        for nm in clamped[:6]: print(f"       {nm[:60]}")
        if len(clamped) > 6: print(f"       ...and {len(clamped)-6} more")
        print(f"     raise EX_MAX_TPARTS in game.c and re-run to keep them all.")

    n = len(installed)
    max_parts = max((len(r["parts"]) for r in installed), default=0)
    lines = []
    lines.append("// AUTO-GENERATED by tools/install_models.py -- DO NOT EDIT BY HAND.")
    lines.append("// Regenerate:  python3 tools/install_models.py --iqm-dir iqm --parts-dir parts")
    lines.append("// Per-model orientation/scale live in model_tuning.json (keyed by name).")
    lines.append("")
    lines.append(f"#define EX_N_IMPORTED {n}")
    lines.append("static const char *g_imported_files[EX_N_IMPORTED] = {")
    for r in installed: lines.append(f'  "{r["single"]}",')
    lines.append("};")
    lines.append("static const char *g_imported_names[EX_N_IMPORTED] = {")
    for r in installed:
        lines.append(f'  "{display_name(r["name"], cat=cat_of(r["name"]))}",')
    lines.append("};")
    # A real category array, emitted whether or not the UI reads it yet. When
    # game.c grows a category filter this is already here and correct.
    cidx = {c: i for i, c in enumerate(CAT_ORDER)}
    cats = [cidx.get(cat_of(r["name"]) or "raw", 2) for r in installed]
    lines.append(f"#define EX_N_CATEGORIES {len(CAT_ORDER)}")
    lines.append("static const char *g_imp_cat_names[EX_N_CATEGORIES] = { "
                 + ", ".join(f'"{CATS[c][0]}"' for c in CAT_ORDER) + " };")
    lines.append("static const unsigned char g_imp_cat[EX_N_IMPORTED] = { "
                 + ", ".join(str(c) for c in cats) + " };")
    for c in CAT_ORDER:
        lines.append(f"// {CATS[c][0]:<6} {cats.count(cidx[c]):>5}  {CATS[c][1]}")

    def arr(field, key, fmt):
        vals = []
        for r in installed:
            t = tuning.get(r["name"], {})
            vals.append(fmt % t.get(key, {"rx": DEF_RX, "ry": DEF_RY,
                                          "rz": DEF_RZ, "scale": DEF_SCL}[key]))
        return f"static float {field}[EX_N_IMPORTED] = {{ " + ", ".join(vals) + " };"
    lines.append(arr("g_imp_rx_def",  "rx",    "%.0f"))
    lines.append(arr("g_imp_ry_def",  "ry",    "%.0f"))
    lines.append(arr("g_imp_rz_def",  "rz",    "%.0f"))
    lines.append(arr("g_imp_scl_def", "scale", "%.3ff"))
    lines.append("")

    textured = [r for r in installed if len(r["parts"]) > 0]
    lines.append(f"// textured multi-part models ({len(textured)})")
    lines.append("static ex_tmodel_t g_tmodels[] = {")
    for r in textured:
        idx = installed.index(r)
        files = ", ".join(f'"{p[0]}"' for p in r["parts"])
        tags  = ", ".join(str(TAG_ID.get(p[1], 0)) for p in r["parts"])
        axes  = ", ".join(str(max(-1, p[2])) for p in r["parts"])
        lines.append(f"  {{ {idx}, {len(r['parts'])}, {{ {files} }}, {{0}}, "
                     f"{{ {tags} }}, {{ {axes} }} }},   // {r['name'][:40]}")
    lines.append("};")
    header = "\n".join(lines) + "\n"

    hdr_path = os.path.join(a.engine, "imported_models.h")
    if a.dry_run:
        print("\n--- imported_models.h (dry run, first 20 lines) ---")
        print("\n".join(header.splitlines()[:20]))
    else:
        open(hdr_path, "w").write(header)

    # seed/refresh the tuning file so values survive the next import
    if not a.dry_run:
        merged = dict(tuning)
        for r in installed:
            merged.setdefault(r["name"], {"rx": DEF_RX, "ry": DEF_RY,
                                          "rz": DEF_RZ, "scale": DEF_SCL})
        json.dump(merged, open(a.tuning, "w"), indent=1, sort_keys=True)

    if not a.no_categories:
        from collections import Counter
        cc = Counter(cat_of(r["name"]) for r in installed)
        print("\ncategories:")
        for c in CAT_ORDER:
            print(f"  {CATS[c][0]:<6} {cc.get(c,0):>5}  {CATS[c][1]}")
        if cc.get("ip"):
            print(f"\n  !! {cc['ip']} model(s) matched a rights-holder pattern and are")
            print(f"     installed under the IP category. They are STILL INSTALLED --")
            print(f"     this tool labels, it does not prune. Use chunkup/exclude to drop them.")
    print(f"\ninstalled {n} model(s), {len(textured)} textured, "
          f"{max_parts} max parts, {len(copied_tex)} textures")
    if not a.dry_run:
        print(f"  wrote {hdr_path}")
        print(f"  wrote {a.tuning} ({len(tuning)} preserved)")
        print("\nnow rebuild:  cmake --build build -j4")
    else:
        print("  (dry run -- nothing written)")

if __name__ == "__main__":
    main()
