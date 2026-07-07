#!/usr/bin/env python3
"""Descriptor-layout drift tripwire (robust, false-positive-free).

Reflects every shader's descriptor layout from SPIR-V (via spv_reflect) and
compares it to a committed baseline snapshot. When a shader's (set, binding,
descriptor_type) table changes, that's the moment a GLSL binding edit lands —
the exact event after which the matching Rust DescriptorSetLayout must be
updated too. Drift is reported per shader as added / removed / changed bindings
(the same shape as the merged `sim::descriptor_layout_diff` core).

Why a baseline and not a direct GLSL→Rust diff: several pipelines REUSE a shared
descriptor set (e.g. entity_pipeline borrows the camera set and declares no
bindings of its own), so containment against a single pipeline file gives false
positives. A baseline compares like with like and never cries wolf on a clean
tree.

NON-FATAL by default (warns; never fails the harness) so it can't spuriously
break unrelated runs. Promote to fatal with --strict once the baseline-refresh
habit is in place.

  descriptor_gate.py            # check vs baseline (best-effort)
  descriptor_gate.py --strict   # drift -> exit 1
  descriptor_gate.py --update   # regenerate the baseline after an intended change
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SHADER_DIR = "crates/unendlich-engine/src/shaders"
BASELINE = os.path.join(HERE, "shader_descriptor_baseline.txt")


def reflect():
    """name -> normalized layout string, via spv_reflect.py."""
    r = subprocess.run([sys.executable, os.path.join(HERE, "spv_reflect.py"), SHADER_DIR],
                       capture_output=True, text=True)
    out = {}
    for ln in r.stdout.splitlines():
        if ln.startswith("#") or ": " not in ln:
            continue
        name, rest = ln.split(": ", 1)
        out[name] = rest.strip()
    return out


def parse_cells(layout):
    """'set0/bind0:UNIFORM_BUFFER, ...' -> {binding_key: type}."""
    if layout.startswith("(") or not layout:
        return {}
    cells = {}
    for c in layout.split(", "):
        c = c.strip()
        if ":" in c and c.startswith("set"):
            key, t = c.rsplit(":", 1)
            cells[key] = t
    return cells


def main():
    strict = "--strict" in sys.argv
    update = "--update" in sys.argv
    if not os.path.isdir(SHADER_DIR):
        print("descriptor_gate: shader dir absent — skipped")
        return 0
    import shutil
    if not shutil.which("glslangValidator") or not shutil.which("spirv-dis"):
        print("descriptor_gate: skipped (glslangValidator / spirv-dis not installed)")
        return 0

    current = reflect()

    if update or not os.path.exists(BASELINE):
        with open(BASELINE, "w") as fh:
            fh.write("# shader descriptor-layout baseline — regenerate with "
                     "`descriptor_gate.py --update` after an intended binding change.\n")
            for name in sorted(current):
                fh.write(f"{name}: {current[name]}\n")
        print(f"descriptor_gate: wrote baseline ({len(current)} shaders) -> "
              f"{os.path.relpath(BASELINE)}")
        return 0

    baseline = {}
    for ln in open(BASELINE):
        if ln.startswith("#") or ": " not in ln:
            continue
        name, rest = ln.rstrip("\n").split(": ", 1)
        baseline[name] = rest.strip()

    drift = []
    for name in sorted(set(current) | set(baseline)):
        if name not in baseline:
            drift.append(f"{name}: NEW shader (add its Rust DescriptorSetLayout)")
            continue
        if name not in current:
            drift.append(f"{name}: shader REMOVED")
            continue
        cur, base = parse_cells(current[name]), parse_cells(baseline[name])
        for k in sorted(set(cur) | set(base)):
            if k not in base:
                drift.append(f"{name}: {k} ADDED ({cur[k]})")
            elif k not in cur:
                drift.append(f"{name}: {k} REMOVED (was {base[k]})")
            elif cur[k] != base[k]:
                drift.append(f"{name}: {k} CHANGED {base[k]} -> {cur[k]}")

    print(f"descriptor_gate: {len(current)} shaders vs baseline")
    if drift:
        print("  !! DESCRIPTOR LAYOUT DRIFT vs baseline — update the matching Rust")
        print("     DescriptorSetLayout bindings, then refresh with --update:")
        for d in drift:
            print(f"     {d}")
        return 1 if strict else 0
    print("  OK — all shader descriptor layouts match the baseline")
    return 0


if __name__ == "__main__":
    sys.exit(main())
#!/usr/bin/env python3
"""Descriptor-layout drift tripwire (robust, false-positive-free).

Reflects every shader's descriptor layout from SPIR-V (via spv_reflect) and
compares it to a committed baseline snapshot. When a shader's (set, binding,
descriptor_type) table changes, that's the moment a GLSL binding edit lands —
the exact event after which the matching Rust DescriptorSetLayout must be
updated too. Drift is reported per shader as added / removed / changed bindings
(the same shape as the merged `sim::descriptor_layout_diff` core).

Why a baseline and not a direct GLSL→Rust diff: several pipelines REUSE a shared
descriptor set (e.g. entity_pipeline borrows the camera set and declares no
bindings of its own), so containment against a single pipeline file gives false
positives. A baseline compares like with like and never cries wolf on a clean
tree.

NON-FATAL by default (warns; never fails the harness) so it can't spuriously
break unrelated runs. Promote to fatal with --strict once the baseline-refresh
habit is in place.

  descriptor_gate.py            # check vs baseline (best-effort)
  descriptor_gate.py --strict   # drift -> exit 1
  descriptor_gate.py --update   # regenerate the baseline after an intended change
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SHADER_DIR = "crates/unendlich-engine/src/shaders"
BASELINE = os.path.join(HERE, "shader_descriptor_baseline.txt")


def reflect():
    """name -> normalized layout string, via spv_reflect.py."""
    r = subprocess.run([sys.executable, os.path.join(HERE, "spv_reflect.py"), SHADER_DIR],
                       capture_output=True, text=True)
    out = {}
    for ln in r.stdout.splitlines():
        if ln.startswith("#") or ": " not in ln:
            continue
        name, rest = ln.split(": ", 1)
        out[name] = rest.strip()
    return out


def parse_cells(layout):
    """'set0/bind0:UNIFORM_BUFFER, ...' -> {binding_key: type}."""
    if layout.startswith("(") or not layout:
        return {}
    cells = {}
    for c in layout.split(", "):
        c = c.strip()
        if ":" in c and c.startswith("set"):
            key, t = c.rsplit(":", 1)
            cells[key] = t
    return cells


def main():
    strict = "--strict" in sys.argv
    update = "--update" in sys.argv
    if not os.path.isdir(SHADER_DIR):
        print("descriptor_gate: shader dir absent — skipped")
        return 0
    import shutil
    if not shutil.which("glslangValidator") or not shutil.which("spirv-dis"):
        print("descriptor_gate: skipped (glslangValidator / spirv-dis not installed)")
        return 0

    current = reflect()

    if update or not os.path.exists(BASELINE):
        with open(BASELINE, "w") as fh:
            fh.write("# shader descriptor-layout baseline — regenerate with "
                     "`descriptor_gate.py --update` after an intended binding change.\n")
            for name in sorted(current):
                fh.write(f"{name}: {current[name]}\n")
        print(f"descriptor_gate: wrote baseline ({len(current)} shaders) -> "
              f"{os.path.relpath(BASELINE)}")
        return 0

    baseline = {}
    for ln in open(BASELINE):
        if ln.startswith("#") or ": " not in ln:
            continue
        name, rest = ln.rstrip("\n").split(": ", 1)
        baseline[name] = rest.strip()

    drift = []
    for name in sorted(set(current) | set(baseline)):
        if name not in baseline:
            drift.append(f"{name}: NEW shader (add its Rust DescriptorSetLayout)")
            continue
        if name not in current:
            drift.append(f"{name}: shader REMOVED")
            continue
        cur, base = parse_cells(current[name]), parse_cells(baseline[name])
        for k in sorted(set(cur) | set(base)):
            if k not in base:
                drift.append(f"{name}: {k} ADDED ({cur[k]})")
            elif k not in cur:
                drift.append(f"{name}: {k} REMOVED (was {base[k]})")
            elif cur[k] != base[k]:
                drift.append(f"{name}: {k} CHANGED {base[k]} -> {cur[k]}")

    print(f"descriptor_gate: {len(current)} shaders vs baseline")
    if drift:
        print("  !! DESCRIPTOR LAYOUT DRIFT vs baseline — update the matching Rust")
        print("     DescriptorSetLayout bindings, then refresh with --update:")
        for d in drift:
            print(f"     {d}")
        return 1 if strict else 0
    print("  OK — all shader descriptor layouts match the baseline")
    return 0


if __name__ == "__main__":
    sys.exit(main())
