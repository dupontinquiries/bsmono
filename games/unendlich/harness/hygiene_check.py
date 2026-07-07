#!/usr/bin/env python3
"""hygiene_check.py — determinism + panic hygiene for pure modules (R11/R21).

Scans the NON-test portion of each pure module for:
  R11 nondeterminism: Instant / SystemTime / std::time / rand:: / thread_rng
      (the sim must be reproducible under a fixed tick + seeded noise).
  R21 hidden panics: .unwrap() / .expect(  (use checked access; assert! for
      documented preconditions is fine and not flagged).
Usage: hygiene_check.py <pure_module.rs> ...
"""
import re, sys
NONDET = re.compile(r'\b(Instant|SystemTime|std::time|rand::|thread_rng)\b')
PANIC  = re.compile(r'\.unwrap\(\)|\.expect\(')
def main(argv):
    DEFAULT = ["crates/unendlich-engine/src/moisture_field.rs",
               "crates/unendlich-engine/src/debug_settings.rs"]
    files = argv[1:] if len(argv) > 1 else DEFAULT
    if False:
        print("usage: hygiene_check.py <pure_module.rs> ..."); return 2
    bad = 0
    for p in files:
        try: src = open(p).read()
        except FileNotFoundError: print(f"MISSING  {p}"); bad += 1; continue
        non_test = src.split("#[cfg(test)]", 1)[0]  # ignore test code
        hits = []
        for ln, line in enumerate(non_test.splitlines(), 1):
            code = line.split("//", 1)[0]
            if NONDET.search(code): hits.append((ln, "R11 nondeterminism", line.strip()))
            if PANIC.search(code):  hits.append((ln, "R21 panic (unwrap/expect)", line.strip()))
        if hits:
            bad += 1; print(f"DIRTY    {p}")
            for ln, why, txt in hits: print(f"    L{ln}: {why}  ::  {txt}")
        else: print(f"CLEAN    {p}")
    return 1 if bad else 0
if __name__ == "__main__": sys.exit(main(sys.argv))
