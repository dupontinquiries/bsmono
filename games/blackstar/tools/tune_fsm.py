#!/usr/bin/env python3
"""tune_fsm.py -- design + tune a Markov behaviour FSM for enemy AI.

The AI runs a Markov chain over 8 combat behaviours: every decision tick an enemy
samples its next behaviour from the row of a transition matrix indexed by its current
behaviour. The behaviour then biases how it moves + fires. This tool:

  1. simulates an abstract skirmish (player kites, K enemies run the FSM) under each
     game context (the 5 modes + the 8 campaign missions), for each of 4 vehicle
     ARCHETYPES (flyer / heavy / fast / skitterer -- 21 vehicle types map onto these),
  2. scores each candidate matrix with a per-context fitness (skirmish wants encircling
     pressure, defence wants a rush to the objective, stalkers want patient creep, ...),
  3. EVOLVES a fine-tuned matrix per (context, archetype) with a small mutation search,
  4. writes content/behavior_fsm.json and (with --emit-c) engine_src/exengine/
     behavior_gen.h -- the weighted matrices + the vehicle->archetype map the runtime
     AI indexes by (mode, vehicle_type).

Deterministic (fixed seeds) so the generated tables are reproducible.
"""
import os, sys, json, math, argparse, random

BEHAVIORS = ["ADVANCE","STRAFE","HOLD","FLANK","CHARGE","RETREAT","HARASS","REGROUP"]
NB = len(BEHAVIORS)
ADVANCE,STRAFE,HOLD,FLANK,CHARGE,RETREAT,HARASS,REGROUP = range(NB)

# vehicle archetypes: (speed, dmg, range, aggression-bias) -- the action profile
ARCHES = {
  "flyer":   dict(speed=1.25, dmg=0.9, rng=30.0, bias=STRAFE),   # dives + strafes from range
  "heavy":   dict(speed=0.70, dmg=1.6, rng=24.0, bias=HOLD),     # slow, hits hard, holds line
  "fast":    dict(speed=1.30, dmg=0.8, rng=20.0, bias=HARASS),   # darts, hit-and-run
  "skitter": dict(speed=0.95, dmg=1.0, rng=16.0, bias=FLANK),    # swarms + flanks up close
}

def vehicle_archetypes(root):
    """Parse the vehicle enum (in order) from vehicle.h and bucket each into an archetype."""
    txt = open(os.path.join(root,"engine_src/exengine/vehicle.h")).read()
    import re
    m = re.search(r"typedef enum\s*\{(.*?)\}\s*ex_vehicle_type;", txt, re.S)
    names = []
    for line in m.group(1).split("\n"):
        mm = re.match(r"\s*(EX_VEH_[A-Z]+)", line)
        if mm and "COUNT" not in mm.group(1): names.append(mm.group(1))
    FLYER = {"JET","DRAGON","HAWK","WRAITH","MANTA","GUNSHIP","BULWARK"}
    HEAVY = {"TANK","DINO","MECH","SENTRY","WHEEL","SPIDER","SCORPION"}
    FAST  = {"CAR","RAPTOR","CYCLE","WOLF"}
    amap = []
    for n in names:
        s = n.replace("EX_VEH_","")
        amap.append("flyer" if s in FLYER else "heavy" if s in HEAVY else
                    "fast" if s in FAST else "skitter")
    return names, amap

# ---- contexts: the 5 modes + 8 campaign missions, each a fitness profile ----
# weights: (pressure, encircle, survive, reach, variety, aggression_target)
CONTEXTS = {
  "arena":      (1.0, 0.8, 0.6, 0.0, 0.7, 0.6),   # skirmish: encircling pressure, stay interesting
  "survival":   (1.1, 1.0, 0.7, 0.0, 0.6, 0.6),   # grind + surround
  "stalkers":   (0.7, 0.5, 1.0, 0.0, 0.3, 0.4),   # patient, relentless creep
  "defend":     (0.7, 0.3, 0.4, 1.4, 0.5, 0.85),  # rush the core objective
  "escalation": (1.1, 1.0, 0.8, 0.2, 0.7, 0.65),  # surround + press, some objective pull
  # campaign missions (by their win-condition flavour)
  "m1_pluto":   (0.9, 0.7, 0.7, 0.2, 0.6, 0.55),  # containment line
  "m2_scav":    (1.0, 0.6, 0.6, 0.0, 0.7, 0.7),   # raiders: aggressive
  "m3_mars":    (0.9, 0.6, 0.6, 0.5, 0.6, 0.7),   # breach: push
  "m4_beach":   (0.8, 0.5, 0.5, 1.2, 0.5, 0.75),  # protect: rush objective
  "m5_vault":   (1.0, 0.9, 0.7, 0.0, 0.6, 0.6),   # grown horrors: surround
  "m6_core":    (1.1, 0.8, 0.7, 0.3, 0.7, 0.7),   # the forge: heavy pressure
  "m7_trench":  (0.8, 0.7, 1.0, 0.2, 0.4, 0.5),   # survive the drowned approach
  "m8_cybertron":(1.2, 1.0, 0.8, 0.3, 0.8, 0.7),  # finale: everything at once
}

# ---- the abstract combat simulation ----------------------------------------
def behaviour_action(b, ex, ez, px, pz, cx, cz, arch):
    """Return (move_dx, move_dz, fire?, accuracy) for behaviour b."""
    dx, dz = px-ex, pz-ez; d = math.hypot(dx,dz) or 1e-3; ux,uz = dx/d, dz/d
    if b == ADVANCE:  return ux, uz, d < arch["rng"], 0.55
    if b == STRAFE:   return -uz, ux, d < arch["rng"], 0.75           # circle, steady aim
    if b == HOLD:     return -ux*0.15, -uz*0.15, d < arch["rng"], 0.85
    if b == FLANK:    return (ux*0.4-uz*0.9), (uz*0.4+ux*0.9), d < arch["rng"]*0.8, 0.5
    if b == CHARGE:   return ux*1.4, uz*1.4, d < arch["rng"]*0.7, 0.4  # fast + wild
    if b == RETREAT:  return -ux, -uz, False, 0.0
    if b == HARASS:   s = 1.0 if d > arch["rng"]*0.6 else -1.0; return ux*s, uz*s, d < arch["rng"], 0.6
    if b == REGROUP:  gx,gz = cx-ex, cz-ez; g=math.hypot(gx,gz) or 1e-3; return gx/g, gz/g, False, 0.2
    return ux, uz, False, 0.3

def simulate(mat, arch, ctx, seed, K=6, T=90):
    rng = random.Random(seed)
    A = ARCHES[arch]; spd = A["speed"]; dmg = A["dmg"]
    # player at origin; enemies ring the arena
    px, pz = 0.0, 0.0; php = 100.0
    ex, ez, ehp, eb = [], [], [], []
    for i in range(K):
        a = rng.uniform(0, 2*math.pi); r = rng.uniform(28, 40)
        ex.append(math.cos(a)*r); ez.append(math.sin(a)*r); ehp.append(30.0); eb.append(A["bias"])
    obj_x, obj_z = 0.0, 0.0            # defence/breach objective = arena centre (player guards it)
    dmg_taken = 0.0; state_hist = [0]*NB; reach_ticks = 0; alive_ticks = 0
    ang_samples = []
    decide = 0
    for t in range(T):
        alive = [i for i in range(K) if ehp[i] > 0]
        if not alive or php <= 0: break
        cx = sum(ex[i] for i in alive)/len(alive); cz = sum(ez[i] for i in alive)/len(alive)
        # player kites: move away from nearest enemy, shoot nearest
        ni = min(alive, key=lambda i:(ex[i]-px)**2+(ez[i]-pz)**2)
        ndx, ndz = px-ex[ni], pz-ez[ni]; nd = math.hypot(ndx,ndz) or 1e-3
        px += ndx/nd*1.1; pz += ndz/nd*1.1
        ehp[ni] -= 9.0                                   # player dps onto nearest
        if decide == 0:                                  # Markov decision every 3 ticks
            for i in alive:
                row = mat[eb[i]]
                r = rng.random(); acc = 0.0; nb = eb[i]
                for j in range(NB):
                    acc += row[j]
                    if r <= acc: nb = j; break
                eb[i] = nb
        decide = (decide+1) % 3
        for i in alive:
            mdx,mdz,fire,accu = behaviour_action(eb[i], ex[i],ez[i], px,pz, cx,cz, A)
            ml = math.hypot(mdx,mdz) or 1e-3
            ex[i] += mdx/ml*spd*1.4; ez[i] += mdz/ml*spd*1.4
            state_hist[eb[i]] += 1
            d = math.hypot(px-ex[i], pz-ez[i])
            if fire and d < A["rng"] and rng.random() < accu*0.5:
                dmg_taken += dmg*2.2; php -= dmg*2.2
            if math.hypot(ex[i]-obj_x, ez[i]-obj_z) < 8.0: reach_ticks += 1
            ang_samples.append(math.atan2(ez[i]-pz, ex[i]-px))
        alive_ticks += len(alive)
    # metrics
    K_end_alive = sum(1 for i in range(K) if ehp[i] > 0)
    pressure = min(dmg_taken/120.0, 1.5)                 # damage dealt to player (capped)
    survive  = alive_ticks / (K*T)
    reach    = reach_ticks / (K*T)
    # encirclement: spread of enemy bearings around the player (higher = surrounded)
    if len(ang_samples) > 8:
        mc = sum(math.cos(a) for a in ang_samples)/len(ang_samples)
        ms = sum(math.sin(a) for a in ang_samples)/len(ang_samples)
        encircle = 1.0 - math.hypot(mc, ms)              # 1 = evenly around, 0 = clumped
    else: encircle = 0.0
    tot = sum(state_hist) or 1
    ps = [h/tot for h in state_hist]
    variety = -sum(p*math.log(p+1e-9) for p in ps)/math.log(NB)   # normalised entropy
    return dict(pressure=pressure, encircle=encircle, survive=survive, reach=reach, variety=variety)

def fitness(mat, arch, ctx, seeds=5):
    w = CONTEXTS[ctx]; wp,we,ws,wr,wv,tgt = w
    acc = 0.0
    for s in seeds if isinstance(seeds, list) else range(seeds):
        m = simulate(mat, arch, ctx, seed=hash((arch,ctx,s)) & 0xffff)
        # pressure is banded around a target so enemies are dangerous but not instakill
        pband = math.exp(-((m["pressure"]-tgt*1.3)**2)/0.5)
        acc += (wp*pband + we*m["encircle"] + ws*m["survive"] + wr*m["reach"] + wv*m["variety"])
    return acc / (len(seeds) if isinstance(seeds,list) else seeds)

# ---- matrices + evolutionary search ----------------------------------------
def normalize(mat):
    for i in range(NB):
        s = sum(mat[i]) or 1.0
        mat[i] = [x/s for x in mat[i]]
    return mat

def seed_matrix(arch, rng):
    A = ARCHES[arch]; m = [[0.0]*NB for _ in range(NB)]
    for i in range(NB):
        for j in range(NB):
            m[i][j] = 0.6 if i==j else 0.10           # sticky by default
        m[i][A["bias"]] += 0.5                        # pull toward the archetype's signature
        m[i][RETREAT]   += 0.05
        for j in range(NB): m[i][j] += rng.random()*0.15
    return normalize(m)

def mutate(mat, rng, rate=0.25):
    m = [row[:] for row in mat]
    for _ in range(rng.randint(1,4)):
        i = rng.randrange(NB); j = rng.randrange(NB)
        m[i][j] = max(0.0, m[i][j] + rng.uniform(-rate, rate))
    return normalize(m)

def optimize(arch, ctx, pop=10, gens=12, seed=0):
    rng = random.Random(hash((arch,ctx,seed)) & 0x7fffffff)
    population = [seed_matrix(arch, rng) for _ in range(pop)]
    scored = [(fitness(m, arch, ctx), m) for m in population]
    for g in range(gens):
        scored.sort(key=lambda x:-x[0])
        elite = [m for _,m in scored[:max(2,pop//3)]]
        children = []
        while len(children) < pop - len(elite):
            parent = rng.choice(elite)
            children.append(mutate(parent, rng, rate=0.30*(1-g/gens)+0.05))
        population = elite + children
        scored = [(fitness(m, arch, ctx), m) for m in population]
    scored.sort(key=lambda x:-x[0])
    return scored[0][1], scored[0][0]

def main():
    ap = argparse.ArgumentParser(description="tune the Markov behaviour FSM")
    ap.add_argument("--root", default=".")
    ap.add_argument("--emit-c", action="store_true")
    ap.add_argument("--quick", action="store_true", help="smaller search (fast smoke run)")
    ap.add_argument("--quiet", action="store_true")
    a = ap.parse_args()
    names, amap = vehicle_archetypes(a.root)
    pop, gens = (6, 5) if a.quick else (10, 12)
    out = {"behaviors": BEHAVIORS, "archetypes": list(ARCHES.keys()),
           "vehicle_archetype": {names[i]: amap[i] for i in range(len(names))},
           "matrices": {}}
    for ctx in CONTEXTS:
        out["matrices"][ctx] = {}
        for arch in ARCHES:
            m, fit = optimize(arch, ctx, pop=pop, gens=gens)
            out["matrices"][ctx][arch] = {"m": [[round(x,4) for x in row] for row in m],
                                          "fitness": round(fit,4)}
            if not a.quiet: print(f"  tuned {ctx:14s} / {arch:8s}  fitness {fit:.3f}")
    jpath = os.path.join(a.root, "content/behavior_fsm.json")
    json.dump(out, open(jpath,"w"), indent=1)
    print(f"wrote {jpath}  ({len(CONTEXTS)} contexts x {len(ARCHES)} archetypes)")
    if a.emit_c: emit_c(a.root, out, names, amap)

def emit_c(root, out, names, amap):
    ctxs = list(CONTEXTS.keys()); arches = list(ARCHES.keys())
    L = ["// AUTO-GENERATED by tools/tune_fsm.py -- do not edit by hand.",
         "#ifndef EX_BEHAVIOR_GEN_H", "#define EX_BEHAVIOR_GEN_H", "",
         f"#define EX_BEH_COUNT {NB}", f"#define EX_BEH_CTX_COUNT {len(ctxs)}",
         f"#define EX_BEH_ARCH_COUNT {len(arches)}", ""]
    L.append("typedef enum { " + ", ".join("EX_BEH_"+b for b in BEHAVIORS) + " } ex_behavior_t;")
    L.append("typedef enum { " + ", ".join("EX_BCTX_"+c.upper() for c in ctxs) + " } ex_beh_ctx_t;")
    L.append("typedef enum { " + ", ".join("EX_BARCH_"+a.upper() for a in arches) + " } ex_beh_arch_t;")
    L.append("")
    # vehicle -> archetype map (indexed by ex_vehicle_type order)
    L.append(f"static const unsigned char ex_beh_vehicle_arch[{len(names)}] = {{")
    row = "  " + ", ".join("EX_BARCH_"+amap[i].upper() for i in range(len(names)))
    L.append(row + " };"); L.append("")
    # the tuned matrices [ctx][arch][NB][NB]
    L.append(f"static const float ex_beh_matrix[EX_BEH_CTX_COUNT][EX_BEH_ARCH_COUNT][EX_BEH_COUNT][EX_BEH_COUNT] = {{")
    for ci,ctx in enumerate(ctxs):
        L.append(f" {{ // {ctx}")
        for ai,arch in enumerate(arches):
            m = out["matrices"][ctx][arch]["m"]
            rows = ", ".join("{" + ",".join(f"{x:.4f}f" for x in r) + "}" for r in m)
            L.append(f"  {{ {rows} }},  // {arch}")
        L.append(" },")
    L.append("};"); L.append(""); L.append("#endif"); L.append("")
    hpath = os.path.join(root, "engine_src/exengine/behavior_gen.h")
    open(hpath,"w").write("\n".join(L))
    print(f"wrote {hpath}")

if __name__ == "__main__":
    main()
