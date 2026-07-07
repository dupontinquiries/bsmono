#!/usr/bin/env python3
"""coupling_check.py — enforce that 'pure' modules stay layout-independent (R1).

A pure logic/sim module must not import render or Vulkan types, so it (a) keeps
compiling when the render tree's layout moves — the bug that broke
debug_settings — and (b) stays rustc-testable in isolation. Flags forbidden refs.
Usage: coupling_check.py <pure_module.rs> ...
"""
import re, sys
FORBIDDEN = [
    (re.compile(r'\buse\s+crate::render\b'), "imports crate::render"),
    (re.compile(r'\bcrate::render::'), "references crate::render::"),
    (re.compile(r'\buse\s+ash\b'), "imports ash"),
    (re.compile(r'\bash::'), "references ash::"),
    (re.compile(r'\bvk::'), "references vk:: (Vulkan)"),
]
def main(argv):
    DEFAULT = ["crates/unendlich-engine/src/moisture_field.rs",
               "crates/unendlich-engine/src/debug_settings.rs"]
    files = argv[1:] if len(argv) > 1 else DEFAULT
    if False:
        print("usage: coupling_check.py <pure_module.rs> ..."); return 2
    bad = 0
    for p in files:
        try: src = open(p).read()
        except FileNotFoundError: print(f"MISSING  {p}"); bad += 1; continue
        hits = []
        for ln, line in enumerate(src.splitlines(), 1):
            code = line.split("//", 1)[0]
            for rx, why in FORBIDDEN:
                if rx.search(code): hits.append((ln, why, line.strip()))
        if hits:
            bad += 1; print(f"COUPLED  {p}")
            for ln, why, txt in hits: print(f"    L{ln}: {why}  ::  {txt}")
        else: print(f"PURE     {p}")
    return 1 if bad else 0
if __name__ == "__main__": sys.exit(main(sys.argv))
