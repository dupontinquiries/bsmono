#!/usr/bin/env python3
"""
integrator.py — the coordinator's GROUND-TRUTH ledger + per-round backpressure.

The disease: worker threads get no feedback that their patches actually landed,
so they keep emitting clean-looking islands forever. The cure is a ledger of
every returned action and what *happened* when the integrator applied/built/
tested it against the canonical repo — then a short digest prepended to every
worker brief next round, led by what's BROKEN or OWED.

Two non-negotiable design rules, because the failure mode is narration:
  1. A verdict comes from OBSERVED exit codes (apply/build/test), not from a
     worker saying "done". Entries that are only self-reported are marked
     `unverified` and can never masquerade as integrated.
  2. The digest leads with what's broken/unapplied/owed (the actionable part)
     and collapses the green history to a count, so it stays short and survives
     summarization instead of getting compacted away.

Storage: ledger.jsonl, append-only, one JSON object per line. Latest entry per
`id` wins for status; history is preserved.

Usage
-----
  # record what the integrator observed for a returned action:
  integrator.py record --id r63-07 --round 7 --worker game-bin \
      --claim "run_world + menu bin" --artifact CHANGES-r63-07.patch \
      --apply ok --build fail --test skip \
      --detail "links until app/world_session; GLFW single-init double-inits window" \
      --owed "fix GLFW single-init; wire menu-window handoff; run new-game->ESC->quit smoke"

  # OR let the tool run the real commands and record observed results:
  integrator.py verify --id r63-07 --round 7 --repo /path/unend --patch out/CHANGES-r63-07.patch \
      --build "cargo build --features render,window" --test "bash tests/smoke_menu.sh"

  # mark a worker self-report (NOT observed) — shows up as unverified:
  integrator.py record --id r71-02 --round 8 --worker voice --claimed \
      --claim "wired Action->engine" --detail "worker says done; integrator hasn't applied it"

  # the tableau (human view):
  integrator.py status

  # the block to prepend to EVERY worker brief next round (optionally pin a worker's own items):
  integrator.py digest --round 8 --head "$(git -C /path/unend rev-parse --short HEAD)" --worker game-bin
"""

import argparse, json, os, subprocess, sys, time

LEDGER = os.environ.get("INTEGRATOR_LEDGER", os.path.join(os.path.dirname(__file__), "ledger.jsonl"))

# verdict -> (rank for sorting, short tag). Lower rank = louder in the digest.
VERDICTS = {
    "broken":     (0, "BROKEN"),       # applied but build/test failed
    "unapplied":  (1, "UNAPPLIED"),    # patch won't even apply to HEAD
    "partial":    (2, "PARTIAL"),      # builds but smoke not green / TODOs open
    "unverified": (3, "UNVERIFIED"),   # only a worker claim — integrator never observed it
    "superseded": (5, "superseded"),   # replaced by a later action
    "integrated": (6, "integrated"),   # apply + build + test all green, merged
}

def now(): return time.strftime("%Y-%m-%dT%H:%M:%S")

def derive_verdict(observed, apply_, build, test):
    """Ground-truth verdict from observed states. `observed=False` => unverified."""
    if not observed:
        return "unverified"
    if apply_ not in ("ok", "n/a", None, ""):     # conflict/fail
        return "unapplied"
    if build == "fail" or test == "fail":
        return "broken"
    if build == "ok" and test == "ok":
        return "integrated"
    return "partial"                               # builds but test skipped, or unknown

def append(entry):
    with open(LEDGER, "a") as f:
        f.write(json.dumps(entry, separators=(",", ":")) + "\n")

def load():
    if not os.path.exists(LEDGER):
        return []
    out = []
    with open(LEDGER) as f:
        for line in f:
            line = line.strip()
            if line:
                try: out.append(json.loads(line))
                except json.JSONDecodeError: pass
    return out

def latest(entries):
    """Most-recent entry per id, in insertion order of first appearance."""
    by_id, order = {}, []
    for e in entries:
        if e["id"] not in by_id:
            order.append(e["id"])
        by_id[e["id"]] = e
    # apply explicit supersession
    for e in entries:
        for sid in e.get("supersedes", []):
            if sid in by_id and by_id[sid].get("verdict") != "integrated":
                by_id[sid] = {**by_id[sid], "verdict": "superseded",
                              "detail": f"superseded by {e['id']}"}
    return [by_id[i] for i in order]

# ----------------------------------------------------------------------------- record

def cmd_record(a):
    observed = not a.claimed
    verdict = a.verdict or derive_verdict(observed, a.apply, a.build, a.test)
    entry = {
        "id": a.id, "round": a.round, "worker": a.worker or "",
        "claim": a.claim or "", "artifact": a.artifact or "",
        "observed": observed,
        "apply": a.apply, "build": a.build, "test": a.test,
        "verdict": verdict, "detail": a.detail or "", "owed": a.owed or "",
        "supersedes": a.supersedes or [], "ts": now(),
    }
    append(entry)
    tag = VERDICTS.get(verdict, (9, verdict))[1]
    print(f"recorded {a.id}: {tag}" + (" (UNVERIFIED — integrator did not observe)" if not observed else ""))

# ----------------------------------------------------------------------------- verify

def run(cmd, cwd):
    """Run a shell command, return (rc, tail-of-output)."""
    if not cmd:
        return None, ""
    p = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True)
    out = (p.stdout + p.stderr).strip()
    return p.returncode, out[-1500:]

def cmd_verify(a):
    repo = a.repo
    evidence = []
    # 1) apply check (3-way against HEAD, no mutation)
    apply_state = "n/a"
    if a.patch:
        rc, out = run(f"git apply --check --3way {a.patch}", repo)
        apply_state = "ok" if rc == 0 else "conflict"
        evidence.append(f"apply: {apply_state}")
        if rc != 0 and out:
            evidence.append(out.splitlines()[-1])
    # 2/3) build + test — run as given (integrator should do this on an integration branch)
    build_state = test_state = "skipped"
    if apply_state in ("ok", "n/a") and a.build:
        rc, out = run(a.build, repo)
        build_state = "ok" if rc == 0 else "fail"
        evidence.append(f"build: {build_state}")
        if rc != 0 and out:
            evidence.append(out.splitlines()[-1])
    if build_state == "ok" and a.test:
        rc, out = run(a.test, repo)
        test_state = "ok" if rc == 0 else "fail"
        evidence.append(f"test: {test_state}")
        if rc != 0 and out:
            evidence.append(out.splitlines()[-1])
    verdict = derive_verdict(True, apply_state, build_state, test_state)
    entry = {
        "id": a.id, "round": a.round, "worker": a.worker or "",
        "claim": a.claim or "", "artifact": a.patch or "",
        "observed": True, "apply": apply_state, "build": build_state, "test": test_state,
        "verdict": verdict, "detail": a.detail or "; ".join(evidence),
        "owed": a.owed or "", "supersedes": a.supersedes or [], "ts": now(),
    }
    append(entry)
    print(f"verified {a.id}: {VERDICTS.get(verdict,(9,verdict))[1]}  [{'; '.join(evidence)}]")

# ----------------------------------------------------------------------------- status

def cmd_status(a):
    rows = sorted(latest(load()), key=lambda e: (VERDICTS.get(e["verdict"], (9, ""))[0], e["id"]))
    if not rows:
        print("(ledger empty)"); return
    w_id = max(4, max(len(r["id"]) for r in rows))
    w_wk = max(6, max(len(r.get("worker", "")) for r in rows))
    print(f'{"ID":<{w_id}}  {"WORKER":<{w_wk}}  {"VERDICT":<10}  DETAIL / OWED')
    print("-" * (w_id + w_wk + 70))
    for r in rows:
        tag = VERDICTS.get(r["verdict"], (9, r["verdict"]))[1]
        d = r.get("detail", "")
        owed = r.get("owed", "")
        line = d + (f"   ⟶ owed: {owed}" if owed else "")
        flag = "" if r.get("observed", True) else "  ⚠self-reported"
        print(f'{r["id"]:<{w_id}}  {r.get("worker",""):<{w_wk}}  {tag:<10}  {line[:90]}{flag}')

# ----------------------------------------------------------------------------- digest

def bucket(rows, *verdicts):
    return [r for r in rows if r["verdict"] in verdicts]

def line_for(r):
    s = f'- `{r["id"]}`'
    if r.get("worker"): s += f' ({r["worker"]})'
    if r.get("detail"): s += f': {r["detail"]}'
    if r.get("owed"):   s += f'  — **owed:** {r["owed"]}'
    return s

def cmd_digest(a):
    rows = latest(load())
    head = a.head or "?"
    out = []
    out.append(f"<!-- INTEGRATION-STATE round={a.round} head={head} (replace this block each round) -->")
    out.append(f"## Integration state — round {a.round} · HEAD `{head}`")
    out.append("_Observed by the integrator (apply → build → smoke), not self-reported. "
               "**If your prior action is listed BROKEN / UNAPPLIED / UNVERIFIED below, fix or re-land it before producing anything new.**_")
    out.append("")

    # a worker's own open items, pinned first
    if a.worker:
        mine = [r for r in rows if r.get("worker") == a.worker
                and r["verdict"] in ("broken", "unapplied", "partial", "unverified")]
        if mine:
            out.append(f"### ⟶ YOURS ({a.worker}) — clear these first")
            out += [line_for(r) for r in mine]
            out.append("")

    broken = bucket(rows, "broken")
    unappl = bucket(rows, "unapplied")
    partial = bucket(rows, "partial")
    unver = bucket(rows, "unverified")

    if broken:
        out.append("### 🔴 BROKEN — applied but does not build/pass")
        out += [line_for(r) for r in broken]; out.append("")
    if unappl:
        out.append("### 🟠 UNAPPLIED — never landed on HEAD")
        out += [line_for(r) for r in unappl]; out.append("")
    if partial:
        out.append("### 🟡 PARTIAL / OWED — builds but not proven wired")
        out += [line_for(r) for r in partial]; out.append("")
    if unver:
        out.append("### ⚪ UNVERIFIED — only a worker claim; integrator has not observed it")
        out += [line_for(r) for r in unver]; out.append("")

    integ = bucket(rows, "integrated")
    supers = bucket(rows, "superseded")
    out.append(f"### 🟢 INTEGRATED ({len(integ)}): " +
               (", ".join(f'`{r["id"]}`' for r in integ) if integ else "—") +
               (f"   ·   superseded: {len(supers)}" if supers else ""))
    text = "\n".join(out) + "\n"

    if a.out:
        with open(a.out, "w") as f:
            f.write(text)
        print(f"wrote digest -> {a.out}  (prepend to every worker brief next round)")
    else:
        sys.stdout.write(text)

# ----------------------------------------------------------------------------- cli

def main():
    p = argparse.ArgumentParser(description="coordinator integration ledger + backpressure digest")
    sub = p.add_subparsers(dest="cmd", required=True)

    common = lambda sp: [
        sp.add_argument("--id", required=True),
        sp.add_argument("--round", type=int, default=0),
        sp.add_argument("--worker", default=""),
        sp.add_argument("--claim", default=""),
        sp.add_argument("--detail", default=""),
        sp.add_argument("--owed", default=""),
        sp.add_argument("--supersedes", nargs="*", default=[]),
    ]

    r = sub.add_parser("record", help="record an observed (or --claimed) result")
    common(r)
    r.add_argument("--artifact", default="")
    r.add_argument("--apply", choices=["ok", "conflict", "fail", "n/a"], default=None)
    r.add_argument("--build", choices=["ok", "fail", "skipped"], default=None)
    r.add_argument("--test", choices=["ok", "fail", "skipped"], default=None)
    r.add_argument("--verdict", choices=list(VERDICTS), default=None,
                   help="override; normally derived from apply/build/test")
    r.add_argument("--claimed", action="store_true",
                   help="this is a worker SELF-REPORT, not integrator-observed → unverified")
    r.set_defaults(func=cmd_record)

    v = sub.add_parser("verify", help="run apply/build/test for real and record what was observed")
    common(v)
    v.add_argument("--repo", required=True)
    v.add_argument("--patch", default="")
    v.add_argument("--build", default="")
    v.add_argument("--test", default="")
    v.set_defaults(func=cmd_verify)

    s = sub.add_parser("status", help="print the tableau")
    s.set_defaults(func=cmd_status)

    d = sub.add_parser("digest", help="emit the block to prepend to every worker brief")
    d.add_argument("--round", type=int, default=0)
    d.add_argument("--head", default="")
    d.add_argument("--worker", default="", help="pin this worker's own open items first")
    d.add_argument("--out", default="", help="write to file instead of stdout")
    d.set_defaults(func=cmd_digest)

    a = p.parse_args()
    a.func(a)

if __name__ == "__main__":
    main()
