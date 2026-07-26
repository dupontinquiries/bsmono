#!/usr/bin/env python3
"""
content_build.py -- the multi-relational content compiler.

Reads content/*.json (part groups, chassis, signatures, weapons, abilities,
troops, maps, modes, missions, animations), RESOLVES the cross-references into a
single graph, and VALIDATES every relation:

  * every part-group mesh exists on disk (engine_src/data/)
  * every troop's chassis / signature / fire_mode / abilities exist
  * every signature backpiece index + fire-mode cannon index is in range
  * every addon 'converts_to' names a real fire mode
  * every mission's map / objective / champion(troop) exist
  * every 'enum' referenced actually exists in the C headers (drift guard)

On success it writes content/_manifest.json (the resolved graph, ready for a
runtime loader or a codegen step) and prints a summary. Exits non-zero on any
error so it can gate a build / CI.

Usage:  python3 tools/content_build.py [--root .] [--quiet]
"""
import json, os, sys, glob, re

ROOT = "."
def load(name):
    with open(os.path.join(ROOT, "content", name)) as f:
        return json.load(f)

def main():
    global ROOT
    args = sys.argv[1:]
    if "--root" in args: ROOT = args[args.index("--root")+1]
    quiet = "--quiet" in args
    errors, warnings = [], []
    def err(m): errors.append(m)
    def warn(m): warnings.append(m)

    C = {n[:-5]: load(n) for n in
         ["part_groups.json","chassis.json","signatures.json","weapons.json",
          "abilities.json","troops.json","maps.json","modes.json","missions.json","animations.json",
          "styling.json","bots.json"]}

    datadir = os.path.join(ROOT, "engine_src", "data")
    on_disk = set(os.path.basename(p) for p in glob.glob(os.path.join(datadir, "*.iqm")))

    # ---- part groups: every mesh must exist on disk ----
    groups = C["part_groups"]["groups"]
    for gname, g in groups.items():
        for m in g["meshes"]:
            if m not in on_disk:
                err(f"part_group '{gname}' references missing mesh '{m}'")
    n_backpiece = len(groups.get("backpiece", {}).get("meshes", []))
    n_cannon    = len(groups.get("cannon", {}).get("meshes", []))

    chassis = C["chassis"]["chassis"]
    sigs    = C["signatures"]["signatures"]
    fmodes  = C["weapons"]["fire_modes"]
    addons  = C["weapons"]["addons"]
    abils   = C["abilities"]["abilities"]
    troops  = C["troops"]["troops"]
    maps    = C["maps"]["maps"]
    modes   = C["modes"]["modes"]

    # ---- signatures: backpiece index in range ----
    for sn, s in sigs.items():
        if not (0 <= s["backpiece"] < n_backpiece):
            err(f"signature '{sn}' backpiece index {s['backpiece']} out of range (0..{n_backpiece-1})")
    # ---- fire modes: cannon index in range ----
    for fn, f in fmodes.items():
        if not (0 <= f["cannon"] < n_cannon):
            err(f"fire_mode '{fn}' cannon index {f['cannon']} out of range (0..{n_cannon-1})")
    # ---- addons: converts_to must be a real fire mode ----
    for an, a in addons.items():
        if "converts_to" in a and a["converts_to"] not in fmodes:
            err(f"addon '{an}' converts_to unknown fire_mode '{a['converts_to']}'")

    # ---- troops: the relational core ----
    frames = set()
    if "bots" in C:
        frames = set(k for k in C["bots"].get("variants", {}).keys() if k != "comment")
    for tn, t in troops.items():
        if t.get("chassis") not in chassis:
            err(f"troop '{tn}' unknown chassis '{t.get('chassis')}'")
        if t.get("signature") not in sigs:
            err(f"troop '{tn}' unknown signature '{t.get('signature')}'")
        if t.get("fire_mode") not in fmodes:
            err(f"troop '{tn}' unknown fire_mode '{t.get('fire_mode')}'")
        for ab in t.get("abilities", []):
            if ab not in abils:
                err(f"troop '{tn}' unknown ability '{ab}'")
        if "of" in t and t["of"] not in troops:
            err(f"troop '{tn}' variant-of unknown troop '{t['of']}'")
        if "frame" in t and frames and t["frame"] not in frames:
            err(f"troop '{tn}' unknown frame '{t['frame']}' (bots.json variants: {sorted(frames)})")

    # ---- styling: cinematic per-troop attributes; refs + sane ranges ----
    styling = C["styling"]["styling"]
    RANGES = {"posture_lean": (-5, 30), "shoulder_hunch": (0.0, 0.4), "gait_cadence": (0.4, 2.0),
              "gait_swagger": (0.5, 2.0), "arm_len": (0.7, 1.4), "leg_len": (0.7, 1.4),
              "visor_narrow": (0.0, 1.0), "visor_angle": (-20, 20), "idle_restless": (0.0, 1.0),
              "aggression": (0.0, 1.0)}
    for tn, st in styling.items():
        if tn not in troops:
            err(f"styling '{tn}' has no matching troop")
        for k, (lo, hi) in RANGES.items():
            if k in st and not (lo <= st[k] <= hi):
                err(f"styling '{tn}'.{k}={st[k]} out of sane range [{lo},{hi}]")
    for tn in troops:
        if tn not in styling:
            warn(f"troop '{tn}' has no styling entry (will use defaults)")

    # ---- missions: map / objective / champion ----
    objectives = set(C["missions"]["objectives"])
    orders = []
    for m in C["missions"]["missions"]:
        orders.append(m["order"])
        if m["map"] not in maps:
            err(f"mission '{m['id']}' unknown map '{m['map']}'")
        if m["objective"] not in objectives:
            err(f"mission '{m['id']}' unknown objective '{m['objective']}'")
        if m.get("champion") and m["champion"] not in troops:
            err(f"mission '{m['id']}' unknown champion troop '{m['champion']}'")
    if sorted(orders) != list(range(1, len(orders)+1)):
        err(f"mission orders not a clean 1..N sequence: {sorted(orders)}")

    # ---- enum drift guard: every 'enum' must exist in the C headers ----
    hdr_text = ""
    for h in glob.glob(os.path.join(ROOT, "engine_src", "exengine", "*.h")):
        try: hdr_text += open(h).read()
        except OSError: pass
    def check_enums(d, label):
        for k, v in d.items():
            if isinstance(v, dict) and "enum" in v:
                if not re.search(r"\b" + re.escape(v["enum"]) + r"\b", hdr_text):
                    warn(f"{label} '{k}' enum '{v['enum']}' not found in any C header (drift?)")
    for label, d in [("chassis",chassis),("signature",sigs),("fire_mode",fmodes),
                     ("addon",addons),("ability",abils),("mode",modes)]:
        check_enums(d, label)

    # ---- resolve + emit the manifest (the flattened graph) ----
    resolved_troops = {}
    for tn, t in troops.items():
        s = sigs.get(t.get("signature"), {})
        resolved_troops[tn] = {
            "kind": t.get("kind"),
            "chassis": t.get("chassis"),
            "signature": t.get("signature"),
            "backpiece_mesh": groups["backpiece"]["meshes"][s["backpiece"]] if s and 0 <= s.get("backpiece",-1) < n_backpiece else None,
            "fire_mode": t.get("fire_mode"),
            "cannon_mesh": groups["cannon"]["meshes"][fmodes[t["fire_mode"]]["cannon"]] if t.get("fire_mode") in fmodes else None,
            "abilities": t.get("abilities", []),
            "tint": t.get("tint"),
        }
    manifest = {
        "counts": {k: len(C[k].get(list(C[k].keys())[-1], {})) if False else 0 for k in C},
        "summary": {
            "part_groups": len(groups), "chassis": len(chassis), "signatures": len(sigs),
            "fire_modes": len(fmodes), "addons": len(addons), "abilities": len(abils),
            "troops": len(troops), "maps": len(maps), "modes": len(modes),
            "missions": len(C["missions"]["missions"]),
        },
        "troops": resolved_troops,
    }
    if not errors:
        with open(os.path.join(ROOT, "content", "_manifest.json"), "w") as f:
            json.dump(manifest, f, indent=2)

    # ---- optional: emit a C header from the JSON (single-source-of-truth codegen) ----
    if not errors and "--emit-c" in args:
        outp = os.path.join(ROOT, "engine_src", "exengine", "content_gen.h")
        lines = ["/* GENERATED by tools/content_build.py --emit-c -- do not edit by hand.",
                 "   Mirrors content/chassis.json; verified against the C table by test_content_gen. */",
                 "#ifndef EX_CONTENT_GEN_H", "#define EX_CONTENT_GEN_H",
                 "typedef struct { const char *name; float h,bulk,arm,leg,head,shoulder;"
                 " int fold_spin; float xform_dur; int greeble; } ex_gen_chassis_t;",
                 f"static const ex_gen_chassis_t ex_gen_chassis[{len(chassis)}] = {{"]
        for name, c in chassis.items():
            lines.append(f'  {{ "{name}", {c["h"]:.4f}f,{c["bulk"]:.4f}f,{c["arm"]:.4f}f,'
                         f'{c["leg"]:.4f}f,{c["head"]:.4f}f,{c["shoulder"]:.4f}f, '
                         f'{c["fold_spin"]}, {c["xform_dur"]:.4f}f, {c["greeble"]} }},')
        lines += ["};", f"#define EX_GEN_CHASSIS_COUNT {len(chassis)}"]

        # per-class cinematic styling, emitted in player-class enum order
        order = ["vanguard","interceptor","titan","specialist","reaver","warden",
                 "pyre","prime","scout","siege"]
        lines += ["",
                  "typedef struct { float posture_lean, shoulder_hunch, gait_cadence, gait_swagger,",
                  "                 arm_len, leg_len, visor_narrow, visor_angle, idle_restless, aggression; } ex_gen_style_t;",
                  f"static const ex_gen_style_t ex_gen_style[{len(order)}] = {{"]
        for name in order:
            st = styling.get(name, {})
            g = lambda k, d: st.get(k, d)
            lines.append("  { %.2ff,%.2ff,%.2ff,%.2ff, %.2ff,%.2ff, %.2ff,%.1ff,%.2ff,%.2ff },  /* %s */" % (
                g("posture_lean",0), g("shoulder_hunch",0), g("gait_cadence",1), g("gait_swagger",1),
                g("arm_len",1), g("leg_len",1), g("visor_narrow",0.5), g("visor_angle",0),
                g("idle_restless",0.4), g("aggression",0.5), name))
        lines += ["};", "#define EX_GEN_STYLE_COUNT %d" % len(order)]

        # per-class FRAME VARIANT (bots.json variants), indexed by player-class enum.
        # -1 = shared base frame; else index into the engine's variant template arrays.
        frame_idx = {"prime": 0, "corsair": 1, "watch": 2}
        lines += ["", "/* frame variant per player class: -1 base, else index into g_tpl_*_var */",
                  "static const int ex_gen_frame[%d] = {" % len(order)]
        frow = []
        for name in order:
            fr = troops.get(name, {}).get("frame")
            frow.append("%d /*%s*/" % (frame_idx.get(fr, -1), name))
        lines += ["  " + ", ".join(frow), "};"]

        # styling indexed by ex_body_kind (PLAYER,GRUNT,CORRUPTED,HIJACK,APEX,HUMAN)
        # -- each enemy kind borrows a troop's cinematic read.
        body_map = ["grunt", "grunt", "ravager", "slipstream", "ravager", None]
        lines += ["", f"static const ex_gen_style_t ex_gen_style_body[{len(body_map)}] = {{"]
        for bt in body_map:
            st = styling.get(bt, {}) if bt else {}
            g = lambda k, d: st.get(k, d)
            lines.append("  { %.2ff,%.2ff,%.2ff,%.2ff, %.2ff,%.2ff, %.2ff,%.1ff,%.2ff,%.2ff },  /* %s */" % (
                g("posture_lean",0), g("shoulder_hunch",0), g("gait_cadence",1), g("gait_swagger",1),
                g("arm_len",1), g("leg_len",1), g("visor_narrow",0.5), g("visor_angle",0),
                g("idle_restless",0.4), g("aggression",0.5), bt or "human/neutral"))
        lines += ["};"]

        # frame variant indexed by ex_body_kind: baseline bodies stay base; the
        # named castes (raider/ravager/slip) upgrade via engine caste routing.
        # APEX is the standing elite tier -> corsair. Explicit, not tied to the
        # styling map (whose ravager-borrow is a cinematic read, not a frame).
        frame_body_troop = [None, None, None, None, "ravager", None]  # PLAYER,GRUNT,CORRUPTED,HIJACK,APEX,HUMAN
        lines += ["", "/* frame per ex_body_kind (PLAYER,GRUNT,CORRUPTED,HIJACK,APEX,HUMAN): -1 base */",
                  "static const int ex_gen_frame_body[%d] = {" % len(frame_body_troop)]
        fbrow = []
        for bt in frame_body_troop:
            fr = troops.get(bt, {}).get("frame") if bt else None
            fbrow.append("%d /*%s*/" % (frame_idx.get(fr, -1), bt or "base"))
        lines += ["  " + ", ".join(fbrow), "};"]
        lines += ["#endif", ""]
        with open(outp, "w") as f: f.write("\n".join(lines))
        if not quiet: print(f"emitted {outp} ({len(chassis)} chassis)")

    if not quiet:
        s = manifest["summary"]
        print("content graph:",
              f"{s['part_groups']} part-groups, {s['chassis']} chassis, {s['signatures']} sigs, "
              f"{s['fire_modes']} fire-modes, {s['addons']} addons, {s['abilities']} abilities,")
        print("              ",
              f"{s['troops']} troops, {s['maps']} maps, {s['modes']} modes, {s['missions']} missions")
        for w in warnings: print("  WARN:", w)
    for e in errors:   print("  ERROR:", e)
    if errors:
        print(f"FAILED with {len(errors)} error(s)."); return 1
    print(f"content OK ({len(warnings)} warning(s)); wrote content/_manifest.json")
    return 0

if __name__ == "__main__":
    sys.exit(main())
