#!/usr/bin/env python3
"""export-roster.py — every foldable blackstar chassis into skydribble.

    ./tools/export-roster.py [poses]

Reads foldview_tuning.json for the hand-tuned models, drops the ones
foldview's own README lists as welded into a single island (they cannot
fold), and exports each as a pose strip under assets/bots/.

Naming: the category prefix decides behaviour in the engine. `flyer` and
`hovertank` chassis get a `jet_` prefix, which physics reads as "never
grounded, lift and nose thrust". Everything else drives.

Scaling note, because "all of them" is the obvious next ask: there are 1012
imported_*.iqm and one pose takes about a second, so the full set is ~85
minutes of export and roughly 4.7 GB of glTF at five poses each — before the
engine tries to load it. Every pose is a model with its own SDF bake at
startup. The tuned set is the one worth shipping; the rest needs a
streaming/on-demand loader first.
"""
import json, os, re, subprocess, sys, time

POSES = int(sys.argv[1]) if len(sys.argv) > 1 else 5
HERE = os.path.dirname(os.path.abspath(__file__))
SKY = os.path.dirname(HERE)
BS = os.path.join(SKY, "..", "blackstar")
FV = os.path.join(BS, "rust", "foldview")
DATA = os.path.join(BS, "engine_src", "data")
OUT = os.path.join(SKY, "assets", "bots")

WELDED = {"akimbo", "echotron", "reverser", "batwing", "tempest"}
FLY = ("flyer", "hovertank")


# --------------------------------------------------------------------------
# FACING: the fold turns the body around, and only sometimes.
#
# Measured across the roster: the robot pose ALWAYS faces -Z, which is the
# engine's convention. The vehicle pose faces +Z on 22 of 26 chassis and -Z
# on the other four. So the two modes point opposite ways on most of the
# fleet, and no single orient.json can fix it — one sidecar per chassis
# cannot rotate f00 and leave f04 alone.
#
# The fix is to spend the 180 degrees ACROSS the strip instead of hiding it
# at one end: pose k is yawed by 180 * smoothstep(k/(n-1)). f00 keeps the
# vehicle's own facing, f04 lands on the robot's, and the poses between turn
# through it. That makes the mismatch into the thing it should have been --
# the body pivoting as it stands up.
#
# Detection reads the head/visor/face part's Z sign, which needs the UNMERGED
# export (merging by colour throws the names away), so each chassis is probed
# once at t=0 and t=1 before its strip is written.
import math, struct


def robot_yaw(d):
    """Degrees the robot pose must turn to face -Z, from its own anatomy.

    An earlier version of this read the head/visor part's Z SIGN, which was
    wrong twice over: a folded-away head is not the nose, and a sign test
    cannot tell 90 degrees from 180. Measured properly — left/right limb pair
    gives the lateral axis, cross it with +Y for forward — every chassis in
    the roster already faces -Z to within 7 degrees, so this returns ~0 and
    nothing is rotated. It stays because the measurement is the thing worth
    keeping: if a future import does arrive turned, this catches it.
    """
    g = json.load(open(os.path.join(d, "scene.gltf")))
    buf = open(os.path.join(d, "scene.bin"), "rb").read()
    cent = {}
    for n in g["nodes"]:
        m = re.match(r"[a-z]+", n.get("name", ""))
        if not m:
            continue
        a = g["accessors"][g["meshes"][n["mesh"]]["primitives"][0]["attributes"]["POSITION"]]
        bv = g["bufferViews"][a["bufferView"]]
        off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        sx = sz = 0.0
        for i in range(a["count"]):
            x, _, z = struct.unpack_from("<3f", buf, off + i * 12)
            sx += x
            sz += z
        c = a["count"] or 1
        cent.setdefault(m.group(0), []).append((sx / c, sz / c))
    avg = {k: (sum(p[0] for p in v) / len(v), sum(p[1] for p in v) / len(v))
           for k, v in cent.items()}
    for l, r in (("uarml", "uarmr"), ("clavl", "clavr"), ("shinl", "shinr"), ("thighl", "thighr")):
        if l in avg and r in avg:
            lx, lz = avg[l][0] - avg[r][0], avg[l][1] - avg[r][1]
            n = math.hypot(lx, lz)
            if n < 1e-4:
                continue
            lx, lz = lx / n, lz / n
            # forward = lateral x up, with up = +Y
            fx, fz = -lz, lx
            return math.degrees(math.atan2(fx, -fz))
    return None


WALK_PHASES = 4


def walk_cmd(iqm, d, k):
    """One phase of a walk cycle, at the ROBOT end of the fold.

    A gait is composed with the fold at the joint rather than applied after
    it — see motion::pose_folded_with — so this is the same export with an
    extra rotation per limb joint. Same passes as the fold strip, so the
    vertex counts match and the two strips are interchangeable.
    """
    phase = k / WALK_PHASES
    return [os.path.join(FV, "target", "release", "foldview"), "--data", DATA,
            "--content", os.path.join(BS, "content"),
            "--veh", os.path.join(DATA, iqm),
            "--gltf", os.path.join(d, f"w{k:02d}"), "--gltf-merge",
            # --tuck IS OUT. It measures a part's overhang in the ROBOT pose and
         # applies the lift in REST space, which is the vehicle frame — so a
         # part correct in one mode is displaced in the other. Measured: it
         # shifts blackstar's vehicle extent by 0.0225 while fixing its robot
         # pose. That is a part detaching from a car to make a bot stand
         # right, which is the "more detached parts from vehicles" report.
         #
         # Tried to fix it by lifting along the rest axis the fold maps onto
         # world up, and the drift did not move. Out of the export until the
         # frame conversion is right; the wheels-below-feet problem comes back
         # and it is the smaller of the two.
         "--repair", "--reattach", "--seams", "0.03", "--ribs", "-1",
            "--gait", f"{phase:.4f}", "--t", "1.0"]


def strip_signature(gj):
    """What must match between poses for `xform::Morph` to blend a strip.

    THIS USED TO BE A TOTAL, and a total cannot see the fault that
    actually occurs. Measured across the roster (skydribble
    `--fold shapes <bot>`): every pose of a strip has the SAME primitive
    count and the SAME total vertex count, but emits its primitives in a
    DIFFERENT ORDER. blackstar is 34 prims / 31973 verts in all five
    poses, and its per-prim counts still read [.., 24, 16] in f00 and
    [.., 8, 24] in f01.

    Morph::build compares poses positionally -- poses[k][p].len() against
    poses[0][p].len() -- so a permutation fails it, build returns None,
    and the engine silently falls back to STEPPING the strip. That is
    the flipbook xform.rs believes it replaced, and it is what every
    chassis currently ships. The old guard summed all VEC3 accessors,
    which is exactly the quantity that stays equal under a permutation,
    so it passed every time and reported nothing.

    So the signature is the ORDERED tuple of per-primitive vertex
    counts, which is precisely what Morph checks.
    """
    out = []
    for mesh in gj.get("meshes", []):
        for prim in mesh.get("primitives", []):
            ai = prim.get("attributes", {}).get("POSITION")
            if ai is None:
                continue
            out.append(gj["accessors"][ai]["count"])
    return tuple(out)


def base_cmd(iqm, d, k, t):
    """The export command for one pose, so the parity retry can reuse it."""
    c = [os.path.join(FV, "target", "release", "foldview"), "--data", DATA, "--content", os.path.join(BS, "content"),
         "--veh", os.path.join(DATA, iqm),
         "--gltf", os.path.join(d, f"f{k:02d}"), "--gltf-merge",
         # --group IS NOT SHIPPED. It absorbs loose panels correctly — 25 on
         # a BAC Mono — and it is nondeterministic on a few chassis: a panel
         # contested by two hosts lands differently between poses, Morph
         # rejects the mismatched strip, and the chassis loses its fold. That
         # is a worse regression than the panels it fixes. Run foldview with
         # --group by hand to see it; it stays out of the roster until the
         # merge is a property of the geometry rather than of loop order.
         # --ribs -1 asks the body what it is: fineness ratio decides whether
         # a rib is an armoured arch or an aerodynamic spar. Vertex-count
         # preserving, so strip parity survives it — unlike --group.
         # --tuck lifts anything poking below the foot plane. The reported
         # "legs too short" was never the legs — it was wheels standing 3% of
         # body height below the feet, so the bot sat on its tyres.
         # --seams closes hinge gaps between parts whose joints ARE linked.
         # It does not touch the reported elbow and shoulder gaps, because
         # those are between parts whose joints are not linked at all — a
         # topology fault wearing a seam's clothes.
         # --tuck IS OUT. It measures a part's overhang in the ROBOT pose and
         # applies the lift in REST space, which is the vehicle frame — so a
         # part correct in one mode is displaced in the other. Measured: it
         # shifts blackstar's vehicle extent by 0.0225 while fixing its robot
         # pose. That is a part detaching from a car to make a bot stand
         # right, which is the "more detached parts from vehicles" report.
         #
         # Tried to fix it by lifting along the rest axis the fold maps onto
         # world up, and the drift did not move. Out of the export until the
         # frame conversion is right; the wheels-below-feet problem comes back
         # and it is the smaller of the two.
         "--repair", "--reattach", "--seams", "0.03", "--ribs", "-1",
         "--t", f"{t:.4f}"]
    if k == POSES - 1:
        c += ["--joints-json", os.path.join(d, "joints.json")]
    return c


def yaw_pose(d, deg, axis="y"):
    """Rotate every position in a written pose, in place.

    Three axes now, because "upside-down" cannot be said in yaw. A body
    rotated 180 about Y is still the same way up; it takes a ROLL about the
    forward axis to turn one over, and lynx_6x6, alphastar and hawkeye all
    needed exactly that. Pitch is here for completeness — a nose-up import
    is the same kind of fault about the third axis.
    """
    if abs(deg) < 1e-6:
        return
    r = math.radians(deg)
    c, s = math.cos(r), math.sin(r)
    g = json.load(open(os.path.join(d, "scene.gltf")))
    path = os.path.join(d, "scene.bin")
    buf = bytearray(open(path, "rb").read())
    for ai, a in enumerate(g["accessors"]):
        if a.get("type") != "VEC3":
            continue
        bv = g["bufferViews"][a["bufferView"]]
        off = bv.get("byteOffset", 0) + a.get("byteOffset", 0)
        lo = [1e30] * 3
        hi = [-1e30] * 3
        for i in range(a["count"]):
            o = off + i * 12
            x, y, z = struct.unpack_from("<3f", buf, o)
            if axis == "y":
                nx, ny, nz = c * x + s * z, y, -s * x + c * z
            elif axis == "z":
                nx, ny, nz = c * x - s * y, s * x + c * y, z
            else:  # "x" — pitch
                nx, ny, nz = x, c * y - s * z, s * y + c * z
            struct.pack_into("<3f", buf, o, nx, ny, nz)
            for k, v in enumerate((nx, ny, nz)):
                lo[k] = min(lo[k], v)
                hi[k] = max(hi[k], v)
        a["min"], a["max"] = lo, hi
    open(path, "wb").write(buf)
    json.dump(g, open(os.path.join(d, "scene.gltf"), "w"))


def smoothstep(t):
    return t * t * (3.0 - 2.0 * t)


def label(iqm):
    """imported_racer_grit_aka_rocket_car.iqm -> ('racer', 'grit')"""
    n = re.sub(r"^imported_", "", iqm).replace(".iqm", "")
    n = re.sub(r"_aka_.*$", "", n)
    parts = n.split("_", 1)
    cat = parts[0]
    name = parts[1] if len(parts) > 1 else parts[0]
    return cat, name


def load_overrides():
    """Human verdicts about which way a chassis faces."""
    p = os.path.join(HERE, "orient-overrides.json")
    try:
        return json.load(open(p)).get("overrides", {})
    except Exception:
        return {}


OVERRIDES = load_overrides()


def main():
    # ROSTER SOURCE. foldview_tuning.json is a list of models someone sat
    # down and tuned, not a list of models that work — and there are over a
    # thousand imports. tools/screen-foldable.py measures which actually
    # stand up (robot height clearly above vehicle height, AND height as its
    # own longest axis, so a body on a diagonal does not pass). Its output
    # is used when present; the tuned list is the fallback.
    # tools/roster.json is the CURATED list: everything already shipping,
    # plus the body plans the screener was never handed (spiders, hawks, a
    # beast, a dragonflyer, mechs), plus the raid bosses. 259 screened
    # chassis is more than a five-pose export can carry; this is the subset
    # worth carrying.
    screened = os.environ.get("ROSTER", os.path.join(HERE, "roster.json"))
    if not os.path.isfile(screened):
        screened = "/tmp/foldable.json"
    if os.path.isfile(screened):
        tuning = json.load(open(screened))
        print(f"roster from {screened}: {len(tuning)} screened chassis")
    else:
        tuning = json.load(open(os.path.join(FV, "foldview_tuning.json")))
        print(f"roster from foldview_tuning.json: {len(tuning)} tuned chassis")
    todo = []
    for iqm in tuning:
        if any(w in iqm for w in WELDED):
            continue
        if not os.path.isfile(os.path.join(DATA, iqm)):
            continue
        cat, name = label(iqm)
        todo.append((("jet_" if cat in FLY else "") + name, iqm))
    todo.sort()

    print(f"{len(todo)} chassis x {POSES} poses")
    fv = os.path.join(FV, "target", "release", "foldview")
    ok, fail = 0, []
    t0 = time.time()
    for i, (name, iqm) in enumerate(todo, 1):
        d = os.path.join(OUT, name)
        subprocess.run(["rm", "-rf", d])

        # probe the facing, unmerged, so part names survive
        # DECLARED CORRECTIONS FIRST. A vehicle's nose is not reliably
        # measurable (see tools/orient-overrides.json), so a human verdict
        # beats a heuristic and this is where it goes.
        ov = OVERRIDES.get(name, {})
        flat_yaw = float(ov.get("yaw", 0.0))
        turn = float(ov.get("turn", 0.0))
        roll = float(ov.get("roll", 0.0))
        pitch = float(ov.get("pitch", 0.0))
        if turn == 0.0 and flat_yaw == 0.0:
            subprocess.run(
            [fv, "--data", DATA, "--content", os.path.join(BS, "content"),
             "--veh", os.path.join(DATA, iqm), "--gltf", "/tmp/_probe_r", "--t", "1.0"],
            capture_output=True, timeout=180)
            try:
                y = robot_yaw("/tmp/_probe_r")
            except Exception:
                y = None
            # only act on a real misalignment; a few degrees is pose, not
            # framing. Measured across the roster this fires on nothing —
            # every robot already lands within 11 degrees of -Z — but it
            # stays in for an import that does arrive turned.
            if y is not None and abs(y) > 20.0:
                turn = round(y / 90.0) * 90.0

        good = True
        for k in range(POSES):
            t = k / (POSES - 1) if POSES > 1 else 0.0
            # --repair --reattach on EVERY pose: they change the joint tree,
            # so a strip where only one pose got them would fold one way at
            # f04 and another at f00, and the morph blends between poses that
            # no longer describe the same body.
            cmd = base_cmd(iqm, d, k, t)
            _unused = [fv, "--data", DATA, "--content", os.path.join(BS, "content"),
                   "--veh", os.path.join(DATA, iqm),
                   "--gltf", os.path.join(d, f"f{k:02d}"), "--gltf-merge",
                   # --group ABSORBS LOOSE PANELS into the volume they sit
                   # on. A car bottom is not a separate object, it is the
                   # underside of the body, and the reported "shells
                   # fracturing into big obstructive panels" is those panels
                   # swinging on joints of their own. This is the first
                   # change in several rounds intended to be VISIBLE rather
                   # than only measurable.
                   # --group IS NOT SHIPPED. It absorbs loose panels correctly — 25 on
         # a BAC Mono — and it is nondeterministic on a few chassis: a panel
         # contested by two hosts lands differently between poses, Morph
         # rejects the mismatched strip, and the chassis loses its fold. That
         # is a worse regression than the panels it fixes. Run foldview with
         # --group by hand to see it; it stays out of the roster until the
         # merge is a property of the geometry rather than of loop order.
         # --ribs -1 asks the body what it is: fineness ratio decides whether
         # a rib is an armoured arch or an aerodynamic spar. Vertex-count
         # preserving, so strip parity survives it — unlike --group.
         # --tuck lifts anything poking below the foot plane. The reported
         # "legs too short" was never the legs — it was wheels standing 3% of
         # body height below the feet, so the bot sat on its tyres.
         # --seams closes hinge gaps between parts whose joints ARE linked.
         # It does not touch the reported elbow and shoulder gaps, because
         # those are between parts whose joints are not linked at all — a
         # topology fault wearing a seam's clothes.
         # --tuck IS OUT. It measures a part's overhang in the ROBOT pose and
         # applies the lift in REST space, which is the vehicle frame — so a
         # part correct in one mode is displaced in the other. Measured: it
         # shifts blackstar's vehicle extent by 0.0225 while fixing its robot
         # pose. That is a part detaching from a car to make a bot stand
         # right, which is the "more detached parts from vehicles" report.
         #
         # Tried to fix it by lifting along the rest axis the fold maps onto
         # world up, and the drift did not move. Out of the export until the
         # frame conversion is right; the wheels-below-feet problem comes back
         # and it is the smaller of the two.
         "--repair", "--reattach", "--seams", "0.03", "--ribs", "-1",
                   "--t", f"{t:.4f}"]
            # THE JOINTS, ONCE PER CHASSIS. They are a property of the rig
            # rather than of a pose, so the robot end writes them and the
            # engine loads one file per bot. --repair refits pivots to their
            # seam first, so what ships is the corrected set.
            if k == POSES - 1:
                cmd += ["--joints-json", os.path.join(d, "joints.json")]
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
            if r.returncode != 0 or not os.path.isfile(os.path.join(d, f"f{k:02d}", "scene.gltf")):
                good = False
                fail.append((name, (r.stderr or "").strip().splitlines()[-1:] or ["?"]))
                break
            # flat_yaw turns the whole strip; turn spends the rotation
            # across the fold. They compose, so a chassis can do both.
            pose_dir = os.path.join(d, f"f{k:02d}")
            # roll and pitch apply flat across the strip: a body that is
            # upside down is upside down in both modes, and spending that
            # across the fold would mean it is only half wrong in the middle.
            if roll:
                yaw_pose(pose_dir, roll, "z")
            if pitch:
                yaw_pose(pose_dir, pitch, "x")
            deg = flat_yaw + turn * smoothstep(t)
            if deg:
                yaw_pose(pose_dir, deg, "y")
        # PARITY GUARD. --group is nondeterministic on a few chassis: a panel
        # contested by two hosts can land differently between poses, and
        # Morph rejects a strip whose poses disagree, so the chassis loses
        # its fold entirely. That is a worse regression than the panels the
        # grouping fixes.
        #
        # So: check the strip, and if it disagrees, re-export that ONE
        # chassis without --group. It keeps its loose panels and keeps its
        # fold, which is the right way round.
        if good:
            counts = []
            for k in range(POSES):
                sg = os.path.join(d, f"f{k:02d}", "scene.gltf")
                try:
                    gj = json.load(open(sg))
                    counts.append(strip_signature(gj))
                except Exception:
                    counts.append(None)
            if len(set(counts)) > 1:
                # THE RETRY IS DELIBERATELY GONE. It re-ran the export
                # with --group filtered out, which made sense when
                # --group was the suspected cause; --group is not in
                # base_cmd at all any more (see its note above), so the
                # retry re-ran an IDENTICAL command five times and then
                # tagged the chassis "ungrouped" whether or not anything
                # changed. With the signature above actually detecting
                # the permutation -- which it does, on every chassis
                # measured -- that dead retry would burn five exports
                # per bot across the whole roster and still emit a
                # mismatched strip.
                #
                # The honest response to a fault this tool cannot fix is
                # to say so. The permutation comes from --gltf-merge
                # grouping by colour and emitting the groups in an order
                # that is not stable between poses; fixing it means
                # making that order deterministic inside foldview, not
                # re-running foldview differently from here.
                widths = ", ".join(str(len(c)) if c else "?" for c in counts)
                tag = (tag + ", STRIP PERMUTED") if tag else "STRIP PERMUTED"
                print(f"    {name}: strip poses disagree on primitive ORDER "
                      f"(prims per pose: {widths}) -- Morph will refuse this "
                      f"strip and the fold will STEP, not blend")

        # THE WALK CYCLE, four phases at the robot end. Written beside the
        # fold strip rather than inside it: a fold is indexed by t and a walk
        # by phase, and collapsing two axes into one list is what would make
        # the engine unable to tell them apart.
        if good:
            for k in range(WALK_PHASES):
                subprocess.run(walk_cmd(iqm, d, k), capture_output=True, text=True, timeout=180)
            # the same flat corrections the fold strip gets, or the walk
            # would face a different way from the model it belongs to
            for k in range(WALK_PHASES):
                wd = os.path.join(d, f"w{k:02d}")
                if not os.path.isdir(wd):
                    continue
                if roll:
                    yaw_pose(wd, roll, "z")
                if pitch:
                    yaw_pose(wd, pitch, "x")
                deg = flat_yaw + turn      # a walk happens at t = 1
                if deg:
                    yaw_pose(wd, deg, "y")

        if good:
            # A RAID BOSS IS BIGGER THAN A BOT, and the scale ships BESIDE the
            # poses rather than being hard-coded in the engine, so adding a
            # boss later is a data change. 2.6x reads as a boss at the
            # distance you first see one without pushing it past the terrain
            # collision the world can actually support.
            scale = 2.6 if iqm.startswith(("imported_raidboss", "imported_raid_")) else 1.0
            with open(os.path.join(d, "chassis.json"), "w") as f:
                json.dump({"scale": scale, "boss": scale > 1.0, "source": iqm}, f)
            ok += 1
        tag = ", ".join(
            [x for x in (f"yaw {flat_yaw:.0f}" if flat_yaw else "",
                         f"turn {turn:.0f}" if turn else "",
                         f"roll {roll:.0f}" if roll else "",
                         f"pitch {pitch:.0f}" if pitch else "") if x]
        ) or "as exported"
        print(f"  [{i:2d}/{len(todo)}] {name:22s} {'ok' if good else 'FAILED'}  {tag}")
    print(f"\n{ok} exported, {len(fail)} failed, {time.time()-t0:.0f}s")
    for n, msg in fail:
        print(f"  {n}: {msg[0][:100]}")


if __name__ == "__main__":
    main()
