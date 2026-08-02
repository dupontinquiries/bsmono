import sys, json, numpy as np
sys.path.insert(0,'.')
import pandapower as pp
from pandapower import networks as nw

CASES = ["case9","case14","case30","case57","case118","case300"]

for name in CASES:
    try:
        net = getattr(nw, name)()
        pp.runpp(net, algorithm="nr", tolerance_mva=1e-10, max_iteration=30, numba=False)
        if not bool(net["converged"]):
            print(f"{name}: DID NOT CONVERGE"); continue
        ppc = net["_ppc"]
        bus, branch, gen = ppc["bus"], ppc["branch"], ppc["gen"]
        # MATPOWER column indices
        out = {
            "name": name,
            "baseMVA": float(ppc["baseMVA"]),
            # bus: i, type(1=PQ,2=PV,3=REF), Pd, Qd, Gs, Bs, Vm, Va(deg)
            "bus": [[float(r[0]), float(r[1]), float(r[2]), float(r[3]),
                     float(r[4]), float(r[5]), float(r[7]), float(r[8])] for r in bus],
            # branch: f, t, r, x, b, tap, shift(deg), status
            "branch": [[float(r[0]), float(r[1]), float(r[2].real), float(r[3].real),
                        float(r[4].real), float(r[8].real), float(r[9].real), float(r[10].real)]
                       for r in branch],
            # gen: bus, Pg, Qg, Qmax, Qmin, Vg, status
            "gen": [[float(r[0]), float(r[1]), float(r[2]), float(r[3]),
                     float(r[4]), float(r[5]), float(r[7])] for r in gen],
            # the answer
            "solution_vm": [float(r[7]) for r in bus],
            "solution_va_deg": [float(r[8]) for r in bus],
        }
        path = f"/home/claude/fixtures/{name}.json"
        json.dump(out, open(path,"w"), indent=1)
        print(f"{name}: {len(out['bus'])} bus, {len(out['branch'])} branch, {len(out['gen'])} gen -> {path}")
    except Exception as e:
        print(f"{name}: FAILED {type(e).__name__}: {e}")
