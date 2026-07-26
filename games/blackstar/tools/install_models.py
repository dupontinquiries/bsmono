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
TAG_ID = {"body": 0, "tire": 1, "tire?": 1, "fan": 2, "door": 3, "armor": 4,
          "arm_l": 5, "arm_r": 6, "leg_l": 7, "leg_r": 8, "head": 9,
          "arm_l_lo": 10, "arm_r_lo": 11, "leg_l_lo": 12, "leg_r_lo": 13,
          "leg_l_ft": 14, "leg_r_ft": 15,
          "morph": 20}

def san(s):
    return re.sub(r"[^A-Za-z0-9_]", "_", s).strip("_").lower()

def display_name(stem, width=28):
    """Readable browser label.

    Long names keep their TAIL, not their head: the suffix (LMB / RIB / PART ...)
    is what distinguishes a variant, and head-truncation silently ate it --
    'SCORPONOK...HUMANLMB' became '...HUMANLM' and the model no longer looked like
    an articulated variant at all."""
    s = re.sub(r"^imported_", "", stem)
    s = re.sub(r"[_\-]+", " ", s).strip().upper()
    if len(s) <= width: return s
    return "~" + s[-(width - 1):]

def unique_display_names(stems, width=28):
    """Browser labels that are guaranteed distinct.

    Truncation collides: 'scorponok...humanlmb' and its neighbours both cut to the
    same 28 chars, so entries silently share a label and you cannot tell which
    model you are looking at. Collisions keep the tail instead of the head, since
    the suffix (LMB/RIB) is what distinguishes them."""
    out, seen = [], {}
    for st in stems:
        base = display_name(st, width)
        if base not in seen:
            seen[base] = 1; out.append(base); continue
        full = re.sub(r"[_\-]+", " ", re.sub(r"^imported_", "", st)).strip().upper()
        cand = full[-width:] if len(full) > width else full     # keep the tail
        while cand in seen:
            seen[base] += 1
            cand = (full[:width - 3] + "~%d" % seen[base])[:width]
        seen[cand] = 1; out.append(cand)
    return out

def read_manifest(path):
    """[(part_file, texture, tag, axis), ...] from a *_parts.txt manifest."""
    out = []
    for line in open(path, errors="ignore").read().splitlines():
        p = line.split()
        if len(p) >= 2:
            tag = p[2] if len(p) > 2 else "body"
            try: axis = int(p[3]) if len(p) > 3 else -1
            except ValueError: axis = -1
            try: off = (float(p[5]), float(p[6]), float(p[7])) if len(p) > 7 else (0.0, 0.0, 0.0)
            except ValueError: off = (0.0, 0.0, 0.0)
            try: par = int(p[8]) if len(p) > 8 else -1
            except ValueError: par = -1
            try: dsp = (float(p[9]), float(p[10]), float(p[11])) if len(p) > 11 else off
            except ValueError: dsp = off
            out.append((p[0], p[1], tag, axis, off, par, dsp))
    return out

def collect(iqm_dir, parts_dir):
    """{name: {'single': path|None, 'parts': [(file,tex,tag,axis)], 'dir': parts_dir}}"""
    models = {}
    if iqm_dir and os.path.isdir(iqm_dir):
        for f in sorted(os.listdir(iqm_dir)):
            if f.lower().endswith(".iqm"):
                name = os.path.splitext(f)[0]
                if name.startswith("imported_"):   # our own output convention:
                    name = name[len("imported_"):] # re-installing must be idempotent
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

def inherit_tuning(name, tuning):
    """Tuning for a DERIVED model, inherited from its source.

    'fallofcybertronjazzlmb' is Jazz with limbs split out; it needs Jazz's
    hand-tuned orientation, not the library default -- otherwise derived models
    appear lying on their backs at the wrong scale, because the default only
    happens to suit models that never needed correcting."""
    ent = tuning.get(name)
    # An entry that is EXACTLY the library default is not really tuned -- earlier
    # runs seeded one for every model. Prefer inheriting from the source in that
    # case, otherwise a derived model can never pick up its parent's orientation.
    is_default = (ent is not None
                  and abs(ent.get("rx", 0) - DEF_RX) < 1e-6
                  and abs(ent.get("ry", 0) - DEF_RY) < 1e-6
                  and abs(ent.get("rz", 0) - DEF_RZ) < 1e-6
                  and abs(ent.get("scale", 0) - DEF_SCL) < 1e-6)
    if ent is not None and not is_default: return ent, None
    stem = name
    for suf in ("lmb", "rib", "veh", "limb", "full"):
        if stem.endswith(suf): stem = stem[:-len(suf)]; break
    key = re.sub(r"[^a-z0-9]", "", stem.lower())
    if not key: return None, None
    def _is_def(e):
        return (abs(e.get("rx", 0) - DEF_RX) < 1e-6 and abs(e.get("ry", 0) - DEF_RY) < 1e-6
                and abs(e.get("rz", 0) - DEF_RZ) < 1e-6 and abs(e.get("scale", 0) - DEF_SCL) < 1e-6)
    # Prefer a source that is genuinely TUNED. Stale keys (older naming schemes,
    # re-import artefacts) hold defaults and often match longer, so a pure
    # longest-match picked them and the inheritance silently did nothing.
    best, best_len, best_tuned = None, 0, False
    for k, e in tuning.items():
        kk = re.sub(r"[^a-z0-9]", "", k.lower())
        if not kk or not (kk in key or key in kk): continue
        tuned = not _is_def(e)
        if (tuned and not best_tuned) or (tuned == best_tuned and len(kk) > best_len):
            best, best_len, best_tuned = k, len(kk), tuned
    if best: return tuning[best], best
    return None, None

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
        print("ERROR: %s not found." % game_c)
        print("  --engine must point at the engine_src DIRECTORY inside the project,")
        print("  e.g.  --engine /path/to/cybertron-cmake-project/engine_src")
        print("  Refusing to write: installing to a path with no game.c silently")
        print("  creates a stray folder and the game never sees the models.")
        sys.exit(2)

    models = collect(a.iqm_dir, a.parts_dir)
    if not models:
        print("no models found"); sys.exit(1)
    names = sorted(models)
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
            # re-installing FROM the data dir (or a staging dir that links into it) is the
            # idempotent case the collect() prefix-strip promises; copy2 raises
            # SameFileError on it, which broke the promise. Same file → nothing to copy.
            if not a.dry_run and not (os.path.exists(dst) and os.path.samefile(m["single"], dst)):
                shutil.copy2(m["single"], dst)
            rec["single"] = f"data/imported_{n}.iqm"
        for (pf, tex, tag, axis, off, par, dsp) in m["parts"]:
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
            rec["parts"].append((f"data/{pf}", tag, axis, off, par, dsp))
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
    for nm in unique_display_names([r["name"] for r in installed]):
        lines.append(f'  "{nm}",')
    lines.append("};")

    inherited = {}
    def _tune_for(nm):
        t, src = inherit_tuning(nm, tuning)
        if src and nm not in inherited: inherited[nm] = src
        return t or {}
    def arr(field, key, fmt):
        vals = []
        for r in installed:
            t = _tune_for(r["name"])
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
        offs  = ", ".join("{%.5ff,%.5ff,%.5ff}" % (p[3][0], p[3][1], p[3][2])
                          for p in r["parts"])
        pars  = ", ".join(str(p[4] if len(p) > 4 else -1) for p in r["parts"])
        disps = ", ".join("{%.5ff,%.5ff,%.5ff}" % (p[5][0], p[5][1], p[5][2])
                          if len(p) > 5 else "{0,0,0}" for p in r["parts"])
        lines.append(f"  {{ {idx}, {len(r['parts'])}, {{ {files} }}, {{0}}, "
                     f"{{ {tags} }}, {{ {axes} }}, {{ {offs} }}, {{ {disps} }}, "
                     f"{{ {pars} }} }},   // {r['name'][:40]}")
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

    if inherited:
        print(f"\n  {len(inherited)} derived model(s) inherited tuning from their source:")
        for k, v in list(inherited.items())[:6]:
            print(f"      {k[:34]:34s} <- {v}")
        if len(inherited) > 6: print(f"      ...and {len(inherited)-6} more")
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
