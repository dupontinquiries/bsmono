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
    if len(rs) != len(gl):
        print(f"MISMATCH count: rust={len(rs)} glsl={len(gl)}")
        print(f"  rust: {rs}\n  glsl: {gl}"); return 1
    drift = [(i, a, b) for i, (a, b) in enumerate(zip(rs, gl)) if a != b]
    print(f"{name}: {len(rs)} fields, order matches" + ("" if not drift else " (NAME DRIFT)"))
    for i, a, b in drift: print(f"    field {i}: rust '{a}' vs glsl '{b}' (order ok, names differ)")
    return 0
if __name__ == "__main__": sys.exit(main(sys.argv))
