#!/usr/bin/env python3
"""Reflect every engine shader's descriptor layout from its compiled SPIR-V.

For each GLSL source under src/shaders/, compile to SPIR-V (glslangValidator),
disassemble (spirv-dis), and extract the (set, binding, descriptor_type) table
the shader actually declares. This is the GROUND TRUTH the Rust descriptor-set
layouts must match; it's the reflection half of the descriptor-layout gate.

Robust + best-effort: a shader that won't compile or disassemble is reported and
skipped, never crashes the run. Emits one normalized line per binding so the
output is diffable and a downstream checker can compare it to the Rust side.

Usage:  spv_reflect.py [SHADER_DIR]   (default: crates/unendlich-engine/src/shaders)
Exit code is always 0 unless --strict is passed and a shader failed to reflect.
"""
import os
import re
import subprocess
import sys

STAGE_EXT = {".vert": "vert", ".frag": "frag", ".comp": "comp",
             ".geom": "geom", ".tesc": "tesc", ".tese": "tese"}


def sh(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def compile_spv(src, out):
    """GLSL -> SPIR-V. Returns (ok, message)."""
    r = sh(["glslangValidator", "-V", "-o", out, src])
    if r.returncode != 0:
        return False, (r.stdout + r.stderr).strip().splitlines()[-1:] or ["compile failed"]
    return True, ""


def disasm(spv):
    r = sh(["spirv-dis", "--no-color", spv])
    return r.stdout if r.returncode == 0 else None


# --- SPIR-V disassembly parsing -------------------------------------------
DECO_SET = re.compile(r"OpDecorate (%\w+) DescriptorSet (\d+)")
DECO_BIND = re.compile(r"OpDecorate (%\w+) Binding (\d+)")
DECO_BLOCK = re.compile(r"OpDecorate (%\w+) Block\b")
DECO_BUFBLOCK = re.compile(r"OpDecorate (%\w+) BufferBlock\b")
VARIABLE = re.compile(r"(%\w+) = OpVariable (%\w+) (\w+)")
TYPE_PTR = re.compile(r"(%\w+) = OpTypePointer (\w+) (%\w+)")
TYPE_IMAGE = re.compile(r"(%\w+) = OpTypeImage %\w+ (\w+) \d+ \d+ \d+ (\d+) (\w+)")
TYPE_SAMPLED_IMAGE = re.compile(r"(%\w+) = OpTypeSampledImage (%\w+)")
TYPE_ARRAY = re.compile(r"(%\w+) = OpTypeArray (%\w+) (%\w+)")
TYPE_RUNTIME_ARRAY = re.compile(r"(%\w+) = OpTypeRuntimeArray (%\w+)")


def reflect(dis):
    """Parse spirv-dis text -> sorted list of (set, binding, dtype)."""
    sets, binds = {}, {}
    blocks, bufblocks = set(), set()
    var_ptr, var_storage = {}, {}
    ptr_storage, ptr_pointee = {}, {}
    images, sampled_images, arrays, runtime_arrays = {}, {}, {}, {}

    for ln in dis.splitlines():
        ln = ln.strip()
        if (m := DECO_SET.search(ln)):
            sets[m[1]] = int(m[2])
        elif (m := DECO_BIND.search(ln)):
            binds[m[1]] = int(m[2])
        elif (m := DECO_BLOCK.search(ln)):
            blocks.add(m[1])
        elif (m := DECO_BUFBLOCK.search(ln)):
            bufblocks.add(m[1])
        elif (m := VARIABLE.search(ln)):
            var_ptr[m[1]], var_storage[m[1]] = m[2], m[3]
        elif (m := TYPE_PTR.search(ln)):
            ptr_storage[m[1]], ptr_pointee[m[1]] = m[2], m[3]
        elif (m := TYPE_IMAGE.search(ln)):
            images[m[1]] = (m[2], int(m[3]))  # (dim, sampled-flag)
        elif (m := TYPE_SAMPLED_IMAGE.search(ln)):
            sampled_images[m[1]] = m[2]
        elif (m := TYPE_ARRAY.search(ln)):
            arrays[m[1]] = m[2]
        elif (m := TYPE_RUNTIME_ARRAY.search(ln)):
            runtime_arrays[m[1]] = m[2]

    def unwrap_array(t):
        seen = set()
        while t in arrays or t in runtime_arrays:
            if t in seen:
                break
            seen.add(t)
            t = arrays.get(t) or runtime_arrays.get(t)
        return t

    def classify(var):
        storage = var_storage.get(var, "")
        ptr = var_ptr.get(var)
        pointee = unwrap_array(ptr_pointee.get(ptr, "")) if ptr else ""
        if storage in ("Uniform", "UniformConstant"):
            if pointee in sampled_images:
                return "COMBINED_IMAGE_SAMPLER"
            if pointee in images:
                _, sampled = images[pointee]
                return "STORAGE_IMAGE" if sampled == 2 else "SAMPLED_IMAGE"
            if pointee in bufblocks:
                return "STORAGE_BUFFER"
            if pointee in blocks:
                return "UNIFORM_BUFFER"
            return "UNIFORM_BUFFER" if storage == "Uniform" else "SAMPLED_IMAGE"
        if storage == "StorageBuffer":
            return "STORAGE_BUFFER"
        return f"?{storage}"

    out = []
    for var, s in sets.items():
        if var in binds:
            out.append((s, binds[var], classify(var)))
    return sorted(out)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    strict = "--strict" in sys.argv
    sdir = args[0] if args else "crates/unendlich-engine/src/shaders"
    if not os.path.isdir(sdir):
        print(f"spv_reflect: no shader dir {sdir}", file=sys.stderr)
        return 0

    srcs = sorted(f for f in os.listdir(sdir)
                  if os.path.splitext(f)[1] in STAGE_EXT)
    failed = 0
    print(f"# SPIR-V descriptor reflection ({len(srcs)} shaders in {sdir})")
    for f in srcs:
        spv = f"/tmp/{f}.spv"
        ok, msg = compile_spv(os.path.join(sdir, f), spv)
        if not ok:
            failed += 1
            print(f"{f}: COMPILE-FAIL {msg}")
            continue
        dis = disasm(spv)
        if dis is None:
            failed += 1
            print(f"{f}: DISASM-FAIL")
            continue
        layout = reflect(dis)
        if not layout:
            print(f"{f}: (no descriptor bindings)")
        else:
            cells = ", ".join(f"set{s}/bind{b}:{t}" for s, b, t in layout)
            print(f"{f}: {cells}")
    print(f"# {len(srcs) - failed}/{len(srcs)} reflected"
          + (f", {failed} failed" if failed else ""))
    return 1 if (strict and failed) else 0


if __name__ == "__main__":
    sys.exit(main())
#!/usr/bin/env python3
"""Reflect every engine shader's descriptor layout from its compiled SPIR-V.

For each GLSL source under src/shaders/, compile to SPIR-V (glslangValidator),
disassemble (spirv-dis), and extract the (set, binding, descriptor_type) table
the shader actually declares. This is the GROUND TRUTH the Rust descriptor-set
layouts must match; it's the reflection half of the descriptor-layout gate.

Robust + best-effort: a shader that won't compile or disassemble is reported and
skipped, never crashes the run. Emits one normalized line per binding so the
output is diffable and a downstream checker can compare it to the Rust side.

Usage:  spv_reflect.py [SHADER_DIR]   (default: crates/unendlich-engine/src/shaders)
Exit code is always 0 unless --strict is passed and a shader failed to reflect.
"""
import os
import re
import subprocess
import sys

STAGE_EXT = {".vert": "vert", ".frag": "frag", ".comp": "comp",
             ".geom": "geom", ".tesc": "tesc", ".tese": "tese"}


def sh(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, **kw)


def compile_spv(src, out):
    """GLSL -> SPIR-V. Returns (ok, message)."""
    r = sh(["glslangValidator", "-V", "-o", out, src])
    if r.returncode != 0:
        return False, (r.stdout + r.stderr).strip().splitlines()[-1:] or ["compile failed"]
    return True, ""


def disasm(spv):
    r = sh(["spirv-dis", "--no-color", spv])
    return r.stdout if r.returncode == 0 else None


# --- SPIR-V disassembly parsing -------------------------------------------
DECO_SET = re.compile(r"OpDecorate (%\w+) DescriptorSet (\d+)")
DECO_BIND = re.compile(r"OpDecorate (%\w+) Binding (\d+)")
DECO_BLOCK = re.compile(r"OpDecorate (%\w+) Block\b")
DECO_BUFBLOCK = re.compile(r"OpDecorate (%\w+) BufferBlock\b")
VARIABLE = re.compile(r"(%\w+) = OpVariable (%\w+) (\w+)")
TYPE_PTR = re.compile(r"(%\w+) = OpTypePointer (\w+) (%\w+)")
TYPE_IMAGE = re.compile(r"(%\w+) = OpTypeImage %\w+ (\w+) \d+ \d+ \d+ (\d+) (\w+)")
TYPE_SAMPLED_IMAGE = re.compile(r"(%\w+) = OpTypeSampledImage (%\w+)")
TYPE_ARRAY = re.compile(r"(%\w+) = OpTypeArray (%\w+) (%\w+)")
TYPE_RUNTIME_ARRAY = re.compile(r"(%\w+) = OpTypeRuntimeArray (%\w+)")


def reflect(dis):
    """Parse spirv-dis text -> sorted list of (set, binding, dtype)."""
    sets, binds = {}, {}
    blocks, bufblocks = set(), set()
    var_ptr, var_storage = {}, {}
    ptr_storage, ptr_pointee = {}, {}
    images, sampled_images, arrays, runtime_arrays = {}, {}, {}, {}

    for ln in dis.splitlines():
        ln = ln.strip()
        if (m := DECO_SET.search(ln)):
            sets[m[1]] = int(m[2])
        elif (m := DECO_BIND.search(ln)):
            binds[m[1]] = int(m[2])
        elif (m := DECO_BLOCK.search(ln)):
            blocks.add(m[1])
        elif (m := DECO_BUFBLOCK.search(ln)):
            bufblocks.add(m[1])
        elif (m := VARIABLE.search(ln)):
            var_ptr[m[1]], var_storage[m[1]] = m[2], m[3]
        elif (m := TYPE_PTR.search(ln)):
            ptr_storage[m[1]], ptr_pointee[m[1]] = m[2], m[3]
        elif (m := TYPE_IMAGE.search(ln)):
            images[m[1]] = (m[2], int(m[3]))  # (dim, sampled-flag)
        elif (m := TYPE_SAMPLED_IMAGE.search(ln)):
            sampled_images[m[1]] = m[2]
        elif (m := TYPE_ARRAY.search(ln)):
            arrays[m[1]] = m[2]
        elif (m := TYPE_RUNTIME_ARRAY.search(ln)):
            runtime_arrays[m[1]] = m[2]

    def unwrap_array(t):
        seen = set()
        while t in arrays or t in runtime_arrays:
            if t in seen:
                break
            seen.add(t)
            t = arrays.get(t) or runtime_arrays.get(t)
        return t

    def classify(var):
        storage = var_storage.get(var, "")
        ptr = var_ptr.get(var)
        pointee = unwrap_array(ptr_pointee.get(ptr, "")) if ptr else ""
        if storage in ("Uniform", "UniformConstant"):
            if pointee in sampled_images:
                return "COMBINED_IMAGE_SAMPLER"
            if pointee in images:
                _, sampled = images[pointee]
                return "STORAGE_IMAGE" if sampled == 2 else "SAMPLED_IMAGE"
            if pointee in bufblocks:
                return "STORAGE_BUFFER"
            if pointee in blocks:
                return "UNIFORM_BUFFER"
            return "UNIFORM_BUFFER" if storage == "Uniform" else "SAMPLED_IMAGE"
        if storage == "StorageBuffer":
            return "STORAGE_BUFFER"
        return f"?{storage}"

    out = []
    for var, s in sets.items():
        if var in binds:
            out.append((s, binds[var], classify(var)))
    return sorted(out)


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    strict = "--strict" in sys.argv
    sdir = args[0] if args else "crates/unendlich-engine/src/shaders"
    if not os.path.isdir(sdir):
        print(f"spv_reflect: no shader dir {sdir}", file=sys.stderr)
        return 0

    srcs = sorted(f for f in os.listdir(sdir)
                  if os.path.splitext(f)[1] in STAGE_EXT)
    failed = 0
    print(f"# SPIR-V descriptor reflection ({len(srcs)} shaders in {sdir})")
    for f in srcs:
        spv = f"/tmp/{f}.spv"
        ok, msg = compile_spv(os.path.join(sdir, f), spv)
        if not ok:
            failed += 1
            print(f"{f}: COMPILE-FAIL {msg}")
            continue
        dis = disasm(spv)
        if dis is None:
            failed += 1
            print(f"{f}: DISASM-FAIL")
            continue
        layout = reflect(dis)
        if not layout:
            print(f"{f}: (no descriptor bindings)")
        else:
            cells = ", ".join(f"set{s}/bind{b}:{t}" for s, b, t in layout)
            print(f"{f}: {cells}")
    print(f"# {len(srcs) - failed}/{len(srcs)} reflected"
          + (f", {failed} failed" if failed else ""))
    return 1 if (strict and failed) else 0


if __name__ == "__main__":
    sys.exit(main())
