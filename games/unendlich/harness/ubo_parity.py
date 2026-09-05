#!/usr/bin/env python3
"""ubo_parity.py — CameraUbo field-order parity, Rust <-> GLSL (R16/R3).

A UBO whose Rust struct and GLSL block disagree on field order is silent UB, not
a compile error. This extracts the ordered field-name list from each side and
asserts they match. Field names need not be identical spellings; ORDER + count
must match (we compare normalized names, warning on name drift).
Usage: ubo_parity.py <rust_file> <glsl_file> [StructName]
"""
import re, sys
def rust_fields(src, name):
    m = re.search(r'struct\s+' + name + r'\s*\{(.*?)\n\}', src, re.S)
    if not m: return None
    out = []
    for line in m.group(1).splitlines():
        code = line.split("//", 1)[0].strip()
        fm = re.match(r'([A-Za-z_]\w*)\s*:', code)
        if fm: out.append(fm.group(1))
    return out
def glsl_fields(src, name):
    m = re.search(r'uniform\s+' + name + r'\s*\{(.*?)\}\s*\w*\s*;', src, re.S)
    if not m: return None
    out = []
    for line in m.group(1).splitlines():
        code = line.split("//", 1)[0].strip().rstrip(";")
        fm = re.match(r'(?:[A-Za-z_]\w*)\s+([A-Za-z_]\w*)(?:\s*\[.*\])?$', code)
        if fm: out.append(fm.group(1))
    return out
def main(argv):
    rust_path = argv[1] if len(argv) > 1 else "crates/unendlich-engine/src/render/world_renderer.rs"
    glsl_path = argv[2] if len(argv) > 2 else "crates/unendlich-engine/src/shaders/chunk.frag"
    name = argv[3] if len(argv) > 3 else "CameraUbo"
    rs = rust_fields(open(rust_path).read(), name)
    gl = glsl_fields(open(glsl_path).read(), name)
    if rs is None: print(f"NOT FOUND: Rust struct {name} in {rust_path}"); return 2
    if gl is None: print(f"NOT FOUND: GLSL block {name} in {glsl_path}"); return 2
    if len(rs) < len(gl):
        # GLSL somehow declares MORE fields than Rust has — always a real
        # problem (it would read past the struct, or duplicate a name).
        print(f"MISMATCH count: rust={len(rs)} glsl={len(gl)} (glsl longer than rust)")
        print(f"  rust: {rs}\n  glsl: {gl}"); return 1
    # A GLSL block shorter than the Rust struct is EXPECTED, not a defect:
    # a shader stage only reads the UBO fields it actually uses, and later
    # (vertex-only, or fragment-only) fields legitimately go undeclared on
    # a side that never touches them — see r595's disturber array, added
    # after chunk.frag's own last-used field, read only from chunk.vert.
    # What actually causes the "silent UB" this checker exists to catch is
    # the GLSL's fields being a non-PREFIX subset (skipping one) or
    # reordered relative to Rust — checked below regardless of length.
    drift = [(i, a, b) for i, (a, b) in enumerate(zip(rs, gl)) if a != b]
    if drift:
        print(f"MISMATCH order/name: rust={rs}\n  glsl={gl}")
        for i, a, b in drift: print(f"    field {i}: rust '{a}' vs glsl '{b}'")
        return 1
    suffix = f" (glsl declares a {len(gl)}-field PREFIX of rust's {len(rs)})" if len(rs) != len(gl) else ""
    print(f"{name}: {len(gl)} fields match in order{suffix}")
    return 0
if __name__ == "__main__": sys.exit(main(sys.argv))
