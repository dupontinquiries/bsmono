#!/usr/bin/env python3
"""Apply rustc's own fix for `float_literal_f32_fallback` across a crate: `1.0` -> `1.0_f32`.

    python3 maintenance/tools/f32_fallback_fix.py [--dry-run] [--features F] [--no-default-features] CRATE_DIR... [-- CARGO_ARGS...]

The lint (rust-lang/rust#154024, `future_incompatible`, on by default since 1.9x) fires on a bare float
literal that type inference resolved to f32 only through a single `impl From<f64>`-shaped bound -- which is
every `egui::Stroke::new(1.0, ...)`, `CornerRadius::from(4.0)`, `.rounding(2.0)` in this tree. It WILL
become a hard error, at which point every egui frontend here stops compiling at once. rustc attaches a
MachineApplicable suggestion to each site; this script runs `cargo check --all-targets
--message-format=json` in each crate and applies exactly those suggestions, nothing else (`cargo fix`
would also apply every other lint's suggestion, mixing concerns in one commit).

Sites are deduplicated by (file, byte range) -- the same literal is reported once per target that compiles
the file -- and edits are applied from the end of each file so earlier offsets stay valid. Files outside
the repo (registry crates) are never touched. Prints one line per file changed and a total; exit 1 if a
crate failed to check, since then its diagnostics are incomplete.

Honours CARGO_TARGET_DIR (the fleet sweep's cache is warm) and sets CARGO_BUILD_JOBS=1 unless already set,
for the same reason the rest of maintenance/tools does.
"""

import json
import os
import subprocess
import sys

LINT = "float_literal_f32_fallback"


def diagnostics(crate_dir, extra_args):
    env = dict(os.environ)
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("CARGO_PROFILE_DEV_DEBUG", "0")
    cmd = ["cargo", "check", "--all-targets", "--message-format=json"] + extra_args
    p = subprocess.run(cmd, cwd=crate_dir, env=env, stdin=subprocess.DEVNULL, capture_output=True, text=True)
    msgs = []
    for line in p.stdout.splitlines():
        try:
            m = json.loads(line)
        except json.JSONDecodeError:
            continue
        if m.get("reason") == "compiler-message":
            msgs.append(m["message"])
    return p.returncode, msgs, p.stderr


def workspace_root(crate_dir):
    """rustc's relative paths are relative to the WORKSPACE root, which for a member crate is not its own dir."""
    p = subprocess.run(["cargo", "locate-project", "--workspace", "--message-format", "plain"], cwd=crate_dir, capture_output=True, text=True)
    return os.path.dirname(p.stdout.strip()) if p.returncode == 0 and p.stdout.strip() else crate_dir


def suggestions(msgs, crate_dir, repo_root):
    """{abs file: {(start, end): replacement}} for every MachineApplicable fix of LINT."""
    out = {}
    crate_dir = workspace_root(crate_dir)
    for m in msgs:
        code = (m.get("code") or {}).get("code")
        if code != LINT:
            continue
        for child in m.get("children", []):
            for s in child.get("spans", []):
                rep = s.get("suggested_replacement")
                if rep is None or s.get("suggestion_applicability") != "MachineApplicable":
                    continue
                f = s["file_name"]
                f = f if os.path.isabs(f) else os.path.normpath(os.path.join(crate_dir, f))
                if not os.path.abspath(f).startswith(repo_root + os.sep):
                    continue  # a registry crate's warning: not ours to edit
                out.setdefault(f, {})[(s["byte_start"], s["byte_end"])] = rep
    return out


def apply(fixes, dry_run):
    total = 0
    for f, spans in sorted(fixes.items()):
        with open(f, "rb") as fh:
            data = fh.read()
        n = 0
        for (start, end), rep in sorted(spans.items(), reverse=True):
            old = data[start:end].decode("utf-8", "replace")
            # the span must still be the literal rustc saw; if a file changed under us, skip rather than corrupt it
            if not rep.startswith(old):
                print(f"  {f}:{start}: span is {old!r}, expected a prefix of {rep!r}; skipped", file=sys.stderr)
                continue
            data = data[:start] + rep.encode() + data[end:]
            n += 1
        if n and not dry_run:
            with open(f, "wb") as fh:
                fh.write(data)
        print(f"{'would fix' if dry_run else 'fixed'} {n:3d} in {f}")
        total += n
    return total


def main(argv):
    dry = "--dry-run" in argv
    argv = [a for a in argv if a != "--dry-run"]
    extra = []
    dirs = []
    if "--" in argv:  # everything after `--` goes to cargo verbatim (a --profile, --config lines)
        extra = argv[argv.index("--") + 1:]
        argv = argv[:argv.index("--")]
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--features":
            extra += [a, argv[i + 1]]
            i += 2
            continue
        if a.startswith("--"):
            extra.append(a)
        else:
            dirs.append(a)
        i += 1
    if not dirs:
        print(__doc__)
        return 2
    repo_root = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True).stdout.strip() or os.getcwd()
    repo_root = os.path.abspath(repo_root)
    failed = 0
    total = 0
    for d in dirs:
        d = os.path.abspath(d)
        rc, msgs, err = diagnostics(d, extra)
        fixes = suggestions(msgs, d, repo_root)
        sites = sum(len(v) for v in fixes.values())
        print(f"== {os.path.relpath(d, repo_root)}: check rc={rc}, {sites} {LINT} site(s) in {len(fixes)} file(s)")
        if rc != 0:
            failed += 1
            print("   check failed; its diagnostics may be incomplete:", err.strip().splitlines()[-1] if err.strip() else "(no stderr)", file=sys.stderr)
        total += apply(fixes, dry)
    print(f"total: {total} literal(s) {'would be ' if dry else ''}suffixed _f32 across {len(dirs)} crate(s)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
