#!/usr/bin/env python3
"""bootstrap_registry.py — sweep the engine for unwired modules and GENERATE
registry adapters for every pattern it can synthesize, then self-heal.

Two output tiers (both linkme distributed-slice registrations):

  sim/service_registry_gen.rs  -> SERVICES  (stateful: ctor + &mut method)
  sim/kernel_registry_gen.rs   -> KERNELS   (pure fns with synthesizable args)

The sweeping generalization: any parameter type in SYNTH below can be conjured
from a deterministic per-tick KernelCtx/ServiceCtx. Any pub fn / struct whose
whole signature is conjurable gets wired, no human in the loop.

Self-healing: after generation it runs `cargo check`; rustc errors inside the
generated files are mapped back to their `// GEN-ENTRY` block and that entry is
pruned, then it re-checks — looping until green. Then it runs the registry
sweep tests; a panic inside some module prunes every generated entry sourced
from that module and re-tests. The survivors are, by construction, entries
that compile and tick panic-free.

Usage:  python3 tools/bootstrap_registry.py [--no-verify]
"""
import os, re, subprocess, sys, json, collections
ALLOWED_FLAGS = {"--no-verify"}
_bad = [a for a in sys.argv[1:] if a not in ALLOWED_FLAGS]
if _bad:
    print(__doc__)
    print(f"unknown arg(s): {_bad} — refusing to run the sweep.")
    sys.exit(2)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "crates/unendlich-engine/src")
GEN_SERVICES = os.path.join(SRC, "sim/service_registry_gen.rs")
GEN_KERNELS = os.path.join(SRC, "sim/kernel_registry_gen.rs")
BIN = os.path.join(SRC, "bin/unendlich_fullgame.rs")

# ---------------------------------------------------------------- discovery
def declared_pub(parent_file, name):
    try:
        t = open(parent_file, encoding="utf-8", errors="replace").read()
    except OSError:
        return False
    return re.search(r"pub\s+mod\s+%s\s*;" % re.escape(name), t) is not None

def modules():
    out = {}
    for dp, dn, fns in os.walk(SRC):
        if "bin" in dp.split(os.sep):
            continue
        for f in fns:
            if not f.endswith(".rs"):
                continue
            p = os.path.join(dp, f)
            rel = os.path.relpath(p, SRC)
            parts = rel[:-3].split(os.sep)
            if parts[-1] == "mod":
                parts = parts[:-1]
            if parts and parts[-1] in ("lib",):
                continue
            # Only modules publicly declared in their parent are addressable
            # from the generated files (private/undeclared files are unreachable
            # via crate:: paths).
            ok = True
            for i, name in enumerate(parts):
                pdir = os.path.join(SRC, *parts[:i])
                cands = [os.path.join(pdir, "mod.rs"), pdir + ".rs",
                         os.path.join(SRC, "lib.rs") if i == 0 else None]
                if not any(c and declared_pub(c, name) for c in cands):
                    ok = False
                    break
            if ok:
                out["::".join(parts)] = p
    return out

def strip_tests_and_comments(t):
    t = re.split(r"#\[cfg\(test\)\]", t)[0]
    t = re.sub(r"/\*.*?\*/", "", t, flags=re.S)
    t = re.sub(r"//[^\n]*", "", t)
    return t

def strip_inner_mods(t):
    """Remove inner `mod name { ... }` bodies — fns inside them have a different
    path than the file module and are often private."""
    out, i, n = [], 0, len(t)
    pat = re.compile(r"\bmod\s+[a-z_][a-z0-9_]*\s*\{")
    while i < n:
        m = pat.search(t, i)
        if not m:
            out.append(t[i:]); break
        out.append(t[i:m.start()])
        depth, j = 1, m.end()
        while j < n and depth:
            if t[j] == "{": depth += 1
            elif t[j] == "}": depth -= 1
            j += 1
        i = j
    return "".join(out)

def strip_impl_blocks(t):
    """Remove impl { ... } bodies so remaining `pub fn` are free functions."""
    out, i, n = [], 0, len(t)
    while i < n:
        m = re.compile(r"\bimpl\b").search(t, i)
        if not m:
            out.append(t[i:])
            break
        b = t.find("{", m.start())
        if b < 0:
            out.append(t[i:])
            break
        out.append(t[i:m.start()])
        depth, j = 1, b + 1
        while j < n and depth:
            if t[j] == "{":
                depth += 1
            elif t[j] == "}":
                depth -= 1
            j += 1
        i = j
    return "".join(out)

def impl_blocks(t):
    """Yield (type_name, body) for each `impl Name {` block."""
    for m in re.finditer(r"\bimpl\s+([A-Za-z0-9_]+)\s*\{", t):
        depth, j = 1, m.end()
        while j < len(t) and depth:
            if t[j] == "{":
                depth += 1
            elif t[j] == "}":
                depth -= 1
            j += 1
        yield m.group(1), t[m.end():j - 1]

IMPURE_RE = re.compile(
    r"std::(fs|io|net|process|env|time)\b|use std::fs|File::(open|create)|"
    r"read_to_string|read_dir|write_all|env::var|Command::new|Instant::now|"
    r"SystemTime|TcpStream|UdpSocket")

def impure(file_text):
    return IMPURE_RE.search(file_text) is not None

# ---------------------------------------------------------------- synthesis
INT_RE = re.compile(r"^(u8|u16|u32|u64|usize|i8|i16|i32|i64|isize)$")
ARR_RE = re.compile(r"^\[\s*(f32|f64|i32|u8|u32)\s*;\s*(\d+)\s*\]$")
SLICE_RE = re.compile(r"^&\s*\[\s*(f32|f64|i32|u32)\s*\]$")
TUP_RE = re.compile(r"^\(\s*f32\s*,\s*f32\s*\)$")

def synth(ty, file_text, mod=None):
    """Return a Rust expr conjuring `ty` from `c: &KernelCtx`, or None."""
    ty = ty.strip()
    # ---- r132 widened classes (top blockers from the census) ----
    m = re.match(r"&\[\(([^)]+)\)\]$", ty)
    if m:  # &[(A, B, ...)] -> empty slice
        return "&[]"
    if re.match(r"&\[\[[a-z0-9 ;]+\]\]$", ty):  # &[[f32; 3]] etc.
        return "&[]"
    if re.match(r"&\[(u8|u16|u32|i32|f32|f64|usize|bool)\]$", ty):
        return "&[]"
    if re.match(r"&mut \[(f32|f64|u8|u32|i32|usize)\]$", ty):
        return "&mut []"
    if ty in ("&Genome", "Genome"):
        # r133c: TWO crate types share this name (genetics::Genome and
        # sim::dragon_archetype::Genome — both expose from_seed(u64)).
        # Resolve from the module's own imports/paths; orphan if ambiguous.
        ft = file_text or ""
        path = None
        if re.search(r"use\s+crate::sim::dragon_archetype(::|\s*\{)[^;]*Genome|dragon_archetype::Genome", ft):
            path = "crate::sim::dragon_archetype::Genome"
        elif re.search(r"use\s+crate::genetics(::|\s*\{)[^;]*Genome|genetics::Genome", ft):
            path = "crate::genetics::Genome"
        elif mod and mod.startswith("sim::dragon"):
            path = "crate::sim::dragon_archetype::Genome"
        if path is None:
            return None
        e = f"{path}::from_seed(c.tick)"
        return "&" + e if ty.startswith("&") else e
    if ty in ("Vec3", "&Vec3", "glm::Vec3", "nalgebra_glm::Vec3"):
        # r133b: respect a module-local alias (e.g. world::flight's
        # `type Vec3 = [f64; 3]`) — the nalgebra conjure mismatches there.
        e = None
        am = re.search(r"type\s+Vec3\s*=\s*\[\s*f(32|64)\s*;\s*3\s*\]", file_text or "")
        if am:
            unit = "c.phase as f64" if am.group(1) == "64" else "c.phase"
            e = f"[{unit}, {unit}, {unit}]"
        elif file_text and re.search(r"use\s+nalgebra_glm|glm::Vec3", file_text):
            e = "nalgebra_glm::vec3(c.phase, c.phase, c.phase)"
        if e is None:
            return None  # unknown Vec3 flavor — let the fn stay orphan
        return "&" + e if ty.startswith("&") else e
    if ty == "CreatureKind":
        return "crate::item::CreatureKind::Fauna"
    if ty == "Loudness":
        return "(c.tick % 64) as i32"
    if ty in ("Biome", "crate::world::biomes::Biome"):
        return "crate::world::biomes::Biome::Plains"
    if ty in ("BlockId", "crate::world::blocks::BlockId", "&BlockId"):
        e = "crate::world::blocks::BlockId::Stone"
        return "&" + e if ty.startswith("&") else e
    if ty == "Element":
        ft = file_text or ""
        if re.search(r"use\s+crate::sim::elemental[^;]*Element|elemental::Element", ft):
            return "crate::sim::elemental::Element::Earth"
        if re.search(r"gesture_resolve[^;]*Element", ft):
            return "crate::sim::gesture_resolve::Element::Earth"
        return None
    if ty == "&crate::heat_source::HeatSourceMap":
        return "&crate::heat_source::HeatSourceMap::new()"
    # r137: GENERIC slice-of-anything -> empty slice. The element type is
    # inferred from the callee's signature; an empty slice is always a valid
    # value of &[T] regardless of T (compile-heal prunes the rare fn that
    # demands non-empty via panic — runtime quarantine catches those).
    if re.match(r"&\[(?:crate::)?[A-Za-z_][A-Za-z0-9_:<> ]*\]$", ty) and "&mut" not in ty:
        return "&[]"
    m = re.match(r"&\[\[f32;\s*(\d+)\];\s*(\d+)\]$", ty.replace(" ", ""))
    if m:
        return f"&[[0.0f32; {m.group(1)}]; {m.group(2)}]"
    if ty == "ChunkCoord":
        return "crate::world::chunk::ChunkCoord { x: 0, y: 0, z: 0 }"
    if ty == "f32":
        return "c.phase"
    if ty == "f64":
        return "c.phase as f64"
    if ty == "bool":
        return "c.tick % 2 == 0"
    m = INT_RE.match(ty)
    if m:
        # nonzero, small, positive-leaning: safe for sizes & denominators
        if ty.startswith("u") or ty == "usize":
            return f"((c.tick % 7) + 1) as {ty}"
        return f"((c.tick % 9) as {ty}) - 4"
    m = ARR_RE.match(ty)
    if m:
        el, n = m.group(1), m.group(2)
        e = {"f32": "c.phase", "f64": "c.phase as f64", "i32": "(c.tick % 9) as i32 - 4",
             "u8": "(c.tick % 250) as u8", "u32": "(c.tick % 7 + 1) as u32"}[el]
        return f"[{e}; {n}]"
    m = SLICE_RE.match(ty)
    if m:
        el = m.group(1)
        e = {"f32": "c.phase", "f64": "c.phase as f64",
             "i32": "(c.tick % 9) as i32 - 4", "u32": "(c.tick % 7 + 1) as u32"}[el]
        return f"&[{e}, {e}, {e}]"
    if TUP_RE.match(ty):
        return "(c.phase, c.signed)"
    if ty == "Fixed" and "sim::fixed" in file_text:
        return "crate::sim::fixed::Fixed::from_frac((c.tick % 100) as i64, 100)"
    if ty == "Vec3" and re.search(r"use crate::sim::fixed::\{[^}]*Vec3", file_text):
        return "crate::sim::fixed::Vec3::ZERO"
    # r115 sweeping generalizations -------------------------------------------
    if ty == "&str":
        return '"probe"'
    if ty == "char":
        return "'a'"
    if re.match(r"^Option\s*<.+>$", ty):
        return "None"  # param type is concrete at the call site, so bare None infers
    m = re.match(r"^Vec\s*<(.+)>$", ty)
    if m and synth(m.group(1).strip(), file_text, mod):
        return "Vec::new()"
    m = re.match(r"^\(\s*i32\s*,\s*i32\s*\)$", ty)
    if m:
        return "((c.tick % 9) as i32 - 4, (c.tick % 7) as i32 - 3)"
    m = re.match(r"^\(\s*i32\s*,\s*i32\s*,\s*i32\s*\)$", ty)
    if m:
        return "((c.tick % 9) as i32 - 4, (c.tick % 7) as i32 - 3, (c.tick % 5) as i32 - 2)"
    m = re.match(r"^&\s*\[\s*(f32|f64|i32|u32)\s*;\s*(\d+)\s*\]$", ty)
    if m:
        el, n = m.group(1), m.group(2)
        e = {"f32": "c.phase", "f64": "c.phase as f64",
             "i32": "(c.tick % 9) as i32 - 4", "u32": "(c.tick % 7 + 1) as u32"}[el]
        return "&[%s; %s]" % (e, n)
    if "BlockId" == ty and "world::blocks" in file_text:
        return "crate::world::blocks::BlockId::Air"
    # Same-file struct with Default (owned or by-ref): conjure ::default()
    m = re.match(r"^(&\s*)?([A-Z][A-Za-z0-9_]*)$", ty)
    if m and mod:
        name = m.group(2)
        if re.search(r"impl\s+Default\s+for\s+%s\b" % name, file_text) or \
           re.search(r"#\[derive\([^)]*Default[^)]*\)\]\s*pub struct %s\b" % name, file_text):
            expr = "crate::%s::%s::default()" % (mod, name)
            return ("&" + expr) if m.group(1) else expr
    return None

def parse_params(sig):
    """Split a param list, respecting <>, (), [] nesting. Returns list of type strings."""
    depth, cur, parts = 0, "", []
    for ch in sig:
        if ch in "<([":
            depth += 1
        elif ch in ">)]":
            depth -= 1
        if ch == "," and depth == 0:
            parts.append(cur)
            cur = ""
        else:
            cur += ch
    if cur.strip():
        parts.append(cur)
    out = []
    for p in parts:
        p = p.strip()
        if p in ("&self", "&mut self", "self", "mut self"):
            out.append(("SELF", p))
            continue
        if ":" not in p:
            return None
        name, ty = p.split(":", 1)
        out.append((name.strip().lstrip("mut "), ty.strip()))
    return out

# gen1 prefs first (stable names for gen1 reruns), gen2 widens the net.
TICK_PREFS = ("tick", "update", "step", "advance")
TICK_PREFS_WIDE = ("tick", "update", "step", "advance", "process", "run",
                   "apply", "evaluate", "integrate", "settle", "decay", "build")
ALT_CTORS = ("full", "empty", "with_defaults", "neutral", "identity", "calm", "clean")

FN_RE = re.compile(
    r"pub\s+(?:const\s+)?fn\s+([a-z_][a-z0-9_]*)\s*(<[^>]*>)?\s*\(", re.S)

def extract_fns(body):
    """Yield (name, generics, params_str) for pub fns in `body`."""
    for m in FN_RE.finditer(body):
        # find matching close paren
        depth, j = 1, m.end()
        while j < len(body) and depth:
            if body[j] == "(":
                depth += 1
            elif body[j] == ")":
                depth -= 1
            j += 1
        yield m.group(1), m.group(2), body[m.end():j - 1]

# ---------------------------------------------------------------- generators
HEADER = """\
//! GENERATED by tools/bootstrap_registry.py — do not hand-edit entries.
//! Re-run the script to regenerate; it prunes anything that fails to
//! compile or panics during the sweep, so this file is self-consistent.
#![allow(clippy::all)]
use linkme::distributed_slice;
"""

def gen_kernel_entries(mods, skip_mods):
    entries = []
    for mod, path in sorted(mods.items()):
        if mod in skip_mods:
            continue
        raw = open(path, encoding="utf-8", errors="replace").read()
        t = strip_tests_and_comments(raw)
        if impure(t):
            continue
        free = strip_impl_blocks(strip_inner_mods(t))
        for name, gen, params in extract_fns(free):
            if gen:  # generic — skip
                continue
            ps = parse_params(params)
            if ps is None or any(p[0] == "SELF" for p in ps):
                continue
            args = []
            ok = True
            for pname, ty in ps:
                e = synth(ty, t, mod)
                if e is None:
                    ok = False
                    break
                args.append(e)
            if not ok:
                continue
            entries.append((mod, name, args))
    return entries

def emit_kernels(entries):
    lines = [HEADER,
             "use crate::sim::kernel_registry::{KernelCtx, KernelEntry, KERNELS};",
             "use std::hint::black_box;", ""]
    for i, (mod, name, args) in enumerate(entries):
        ident = f"GK_{i:04d}"
        kname = f"{mod.replace('::', '_')}__{name}"
        call_args = ", ".join(f"black_box({a})" for a in args)
        lines += [
            f"// GEN-ENTRY: {kname} src={mod}",
            f"fn gk_{i:04d}(c: &KernelCtx) {{",
            f"    let _ = c;",
            f"    black_box(crate::{mod}::{name}({call_args}));",
            f"}}",
            f"#[distributed_slice(KERNELS)]",
            f"static {ident}: KernelEntry = KernelEntry {{ name: \"{kname}\", run: gk_{i:04d} }};",
            f"// GEN-END",
            "",
        ]
    return "\n".join(lines)

def gen_service_entries(mods, skip_mods, already_named, wide=False):
    global TICK_PREFS
    saved = TICK_PREFS
    if wide:
        TICK_PREFS = TICK_PREFS_WIDE
    try:
        return _gen_service_entries(mods, skip_mods, already_named, wide)
    finally:
        TICK_PREFS = saved

def _gen_service_entries(mods, skip_mods, already_named, wide=False):
    entries = []
    for mod, path in sorted(mods.items()):
        if mod in skip_mods:
            continue
        raw = open(path, encoding="utf-8", errors="replace").read()
        t = strip_tests_and_comments(raw)
        if impure(t):
            continue
        has_default = collections.defaultdict(bool)
        for m in re.finditer(r"impl\s+Default\s+for\s+([A-Za-z0-9_]+)", t):
            has_default[m.group(1)] = True
        for m in re.finditer(r"#\[derive\(([^)]*)\)\]\s*pub struct ([A-Za-z0-9_]+)", t):
            if "Default" in m.group(1):
                has_default[m.group(2)] = True
        for tyname, body in impl_blocks(t):
            ctor = None
            if has_default[tyname]:
                ctor = f"crate::{mod}::{tyname}::default()"
            tick = None
            cands = []
            for name, gen, params in extract_fns(body):
                if gen:
                    continue
                ps = parse_params(params)
                if ps is None:
                    continue
                if name in ("new",) and ctor is None and not any(p[0] == "SELF" for p in ps):
                    args, ok = [], True
                    for pn, ty in ps:
                        e = synth(ty, t, mod)
                        if e is None:
                            ok = False
                            break
                        args.append(e)
                    if ok:
                        ctor = f"crate::{mod}::{tyname}::new({', '.join(a.replace('c.', 'c0.') for a in args)})"
                if wide and ctor is None and not ps and name in ALT_CTORS:
                    # zero-arg constructor-ish fn returning Self (full(),
                    # empty(), with_defaults(), ...). We can't see the return
                    # type cheaply; the compile-heal prunes wrong guesses.
                    ctor = f"crate::{mod}::{tyname}::{name}()"
                if ps and ps[0][1] == "&mut self":
                    args, ok = [], True
                    for pn, ty in ps[1:]:
                        e = synth(ty, t, mod)
                        if e is None:
                            ok = False
                            break
                        args.append(e)
                    if ok:
                        cands.append((name, args))
            for pref in TICK_PREFS:
                for n, a in cands:
                    if n == pref and tick is None:
                        tick = (n, a)
            if tick is None and cands:
                tick = cands[0]
            sname = f"{mod.replace('::', '_')}__{tyname}"
            if ctor and tick and sname not in already_named:
                entries.append((mod, tyname, ctor, tick))
    return entries

def emit_services(entries):
    lines = [HEADER,
             "use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};",
             "use crate::sim::kernel_registry::KernelCtx;", ""]
    for i, (mod, tyname, ctor, (tick, args)) in enumerate(entries):
        ident = f"GS_{i:04d}"
        sname = f"{mod.replace('::', '_')}__{tyname}"
        call_args = ", ".join(args)
        lines += [
            f"// GEN-ENTRY: {sname} src={mod}",
            f"struct S{i:04d}(crate::{mod}::{tyname}, u64);",
            f"impl Service for S{i:04d} {{",
            f"    fn name(&self) -> &'static str {{ \"{sname}\" }}",
            f"    fn tick(&mut self, ctx: &ServiceCtx) {{",
            f"        let _ = ctx;",
            f"        self.1 = self.1.wrapping_add(1);",
            f"        let c = KernelCtx::at(self.1);",
            f"        let _ = &c;",
            f"        let _ = self.0.{tick}({call_args});",
            f"    }}",
            f"}}",
            f"#[distributed_slice(SERVICES)]",
            f"static {ident}: ServiceEntry = ServiceEntry {{",
            f"    name: \"{sname}\",",
            f"    make: || {{",
            f"        let c0 = KernelCtx::at(1);",
            f"        let _ = &c0;",
            f"        Box::new(S{i:04d}({ctor}, 0))",
            f"    }},",
            f"}};",
            f"// GEN-END",
            "",
        ]
    return "\n".join(lines)

# ---------------------------------------------------------------- prune loop
def prune_file(path, bad_lines=None, bad_mods=None):
    """Remove GEN-ENTRY blocks containing any bad line number / sourced from bad mods."""
    src = open(path).read().splitlines()
    keep, i, removed = [], 0, 0
    while i < len(src):
        if src[i].startswith("// GEN-ENTRY:"):
            j = i
            while j < len(src) and not src[j].startswith("// GEN-END"):
                j += 1
            block_lines = set(range(i + 1, j + 2))
            srcmod = src[i].split("src=")[1].strip()
            bad = False
            if bad_lines and block_lines & bad_lines:
                bad = True
            if bad_mods and srcmod in bad_mods:
                bad = True
            if bad:
                removed += 1
            else:
                keep.extend(src[i:j + 1])
                keep.append("")
            i = j + 1
            if i < len(src) and src[i] == "":
                i += 1
        else:
            keep.append(src[i])
            i += 1
    open(path, "w").write("\n".join(keep) + "\n")
    return removed

def cargo(args):
    return subprocess.run(["cargo"] + args, cwd=ROOT, capture_output=True, text=True,
                          env={**os.environ, "CARGO_INCREMENTAL": "0"})

def heal_compile():
    for it in range(30):
        r = cargo(["check", "-p", "unendlich-engine", "--message-format=short"])
        errs = collections.defaultdict(set)
        for line in (r.stdout + r.stderr).splitlines():
            m = re.match(r"(.+?\.rs):(\d+):\d+:\s*error", line)
            if m:
                errs[os.path.abspath(os.path.join(ROOT, m.group(1)))].add(int(m.group(2)))
        gen_errs = {p: ls for p, ls in errs.items() if p in (GEN_SERVICES, GEN_KERNELS)}
        if r.returncode == 0:
            print(f"[heal] compile green after {it} prune round(s)")
            return True
        if not gen_errs:
            print("[heal] non-generated compile errors — aborting:")
            print((r.stdout + r.stderr)[-3000:])
            return False
        total = 0
        for p, ls in gen_errs.items():
            total += prune_file(p, bad_lines=ls)
        print(f"[heal] round {it}: pruned {total} entries from generated files")
    return False

def heal_panics():
    for it in range(30):
        r = cargo(["test", "-p", "unendlich-engine", "--lib",
                   "kernel_registry", "--", "--test-threads=1"])
        out = r.stdout + r.stderr
        if r.returncode == 0:
            r2 = cargo(["test", "-p", "unendlich-engine", "--lib", "service_registry"])
            if r2.returncode == 0:
                print(f"[heal] sweep tests green after {it} panic-prune round(s)")
                return True
            out = r2.stdout + r2.stderr
        m = re.search(r"panicked at ([^\s:]+\.rs):\d+", out)
        if not m:
            print("[heal] test failure with no parseable panic location:")
            print(out[-3000:])
            return False
        pf = os.path.abspath(os.path.join(ROOT, m.group(1)))
        rel = os.path.relpath(pf, SRC)
        parts = rel[:-3].split(os.sep)
        if parts[-1] == "mod":
            parts = parts[:-1]
        bad_mod = "::".join(parts)
        n = prune_file(GEN_KERNELS, bad_mods={bad_mod}) + \
            prune_file(GEN_SERVICES, bad_mods={bad_mod})
        print(f"[heal] round {it}: panic in {bad_mod} — pruned {n} entries from that module")
        if n == 0:
            print("[heal] panic not in a generated entry — aborting")
            print(out[-2000:])
            return False
    return False

# ---------------------------------------------------------------- main
def main():
    mods = modules()
    # Modules already covered by hand-written registrations: skip generating for them.
    skip = set()
    for dp, dn, fns in os.walk(SRC):
        for f in fns:
            p = os.path.join(dp, f)
            if not f.endswith(".rs") or p in (GEN_SERVICES, GEN_KERNELS):
                continue
            t = open(p, encoding="utf-8", errors="replace").read()
            if "distributed_slice(SERVICES)" in t or "distributed_slice(KERNELS)" in t:
                rel = os.path.relpath(p, SRC)
                parts = rel[:-3].split(os.sep)
                if parts[-1] == "mod":
                    parts = parts[:-1]
                skip.add("::".join(parts))
    # Modules referenced by the central batch file count as covered too.
    batch = open(os.path.join(SRC, "sim/service_registry_batch.rs")).read()
    for m in re.finditer(r"crate::((?:\w+::)*\w+)::\w+::(?:new|default|hanging|seeded|fireball|summon|neutral|none)", batch):
        skip.add(m.group(1))
    skip |= {"sim::service_registry", "sim::service_registry_batch", "sim::kernel_registry",
             "sim::service_registry_gen", "sim::kernel_registry_gen"}

    if "--gen2" in sys.argv:
        # second pass: wide prefs + alt ctors, into *_gen2.rs, skipping every
        # service name already emitted by gen1/batch/hand registrations.
        named = set()
        for f in (GEN_SERVICES, os.path.join(SRC, "sim/service_registry_batch.rs")):
            if os.path.exists(f):
                named |= set(re.findall(r'name: "([^"]+)"', open(f).read()))
        g2 = os.path.join(SRC, "sim/service_registry_gen2.rs")
        s2_all = gen_service_entries(mods, skip, set(), wide=True)
        s2 = [e for e in s2_all
              if f"{e[0].replace('::', '_')}__{e[1]}" not in named]
        open(g2, "w").write(emit_services(s2))
        print(f"[gen2] {len(s2)} additional service entries ({len(s2_all)-len(s2)} already covered)")
        modrs = os.path.join(SRC, "sim/mod.rs")
        t2 = open(modrs).read()
        if "pub mod service_registry_gen2;" not in t2:
            t2 = t2.replace("pub mod service_registry_gen;",
                            "pub mod service_registry_gen;\npub mod service_registry_gen2;", 1)
            open(modrs, "w").write(t2)
        return

    k = gen_kernel_entries(mods, skip)
    s = gen_service_entries(mods, skip, set())
    open(GEN_KERNELS, "w").write(emit_kernels(k))
    open(GEN_SERVICES, "w").write(emit_services(s))
    print(f"[gen] {len(k)} kernel entries, {len(s)} service entries generated")

    modrs = os.path.join(SRC, "sim/mod.rs")
    t = open(modrs).read()
    for decl in ("pub mod kernel_registry;", "pub mod kernel_registry_gen;", "pub mod service_registry_gen;"):
        if decl not in t:
            t = t.replace("pub mod service_registry;", "pub mod service_registry;\n" + decl, 1)
    open(modrs, "w").write(t)

    if "--no-verify" in sys.argv:
        return
    if not heal_compile():
        sys.exit(1)
    if not heal_panics():
        sys.exit(1)
    sk = open(GEN_KERNELS).read().count("// GEN-ENTRY")
    ss = open(GEN_SERVICES).read().count("// GEN-ENTRY")
    print(f"[done] survivors: {sk} kernels, {ss} services")

if __name__ == "__main__":
    main()
