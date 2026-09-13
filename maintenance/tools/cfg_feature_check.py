#!/usr/bin/env python3
"""cfg_feature_check.py — a `#[cfg(feature = "x")]` whose `x` no manifest
declares is code that can never be compiled, and a flag nobody can pass.

WHY THIS EXISTS

Found by scanning for it, after `apps/rusty-board/src/gestures/llm_swipe.rs`
turned out to gate its entire real implementation behind
`#[cfg(feature = "llm-swipe-ffi")]` — a feature `apps/rusty-board/Cargo.toml`
does not have. Ten sites. The consequences compound in a way that is very
quiet:

* the `cfg(feature = ...)` half never compiles, so it is never type-checked
  and can rot freely — this one references `crate::llm_backend`, a module that
  does not exist in that crate at all (it is logscope's);
* the `cfg(not(feature = ...))` half compiles unconditionally, so the MOCK is
  permanently the real code path;
* the crate still builds, all tests pass, and nothing warns loudly enough;
* and the escape hatch is fictional — the error variant literally reads
  "build with --features llm-swipe-ffi", which fails with
  `error: the package 'rusty-board' does not contain this feature`.

Three docs in that app name the feature three different ways
(`llm-swipe-ffi`, `llama-ffi`, `llm`), none of which exists. A reader has no
way to tell from the source that the whole thing is inert.

rustc's `unexpected_cfgs` lint covers some of this since 1.80, but it is a
warning in a build that emits many, and it does not fire for a crate nobody
currently compiles — which is exactly the state these get into.

WHAT COUNTS AS DECLARED

A name in `[features]`, or an optional dependency (cargo makes those implicit
features), including optional deps under `[target.'cfg(...)'.dependencies]`.
Weak/namespaced forms (`dep:x`, `x?/y`) are resolved to their base name.

FALSE POSITIVE THIS DELIBERATELY AVOIDS

`apps/voiceterm/src/build_diagnostics.rs` embeds captured compiler output as a
string literal, and that text contains `#[cfg(feature = "winit")]`. It is not
a cfg, it is a quoted example. So an attribute is only counted when `#[cfg`
opens the line (modulo indentation), or when it appears as the `cfg!()` macro.
That was a real hit on the first run and dropping it is why this checker can
be trusted to be read rather than skimmed.

USAGE

    python3 maintenance/tools/cfg_feature_check.py [repo_root]

Exit 0 with a PASS line, or 1 listing each crate, feature and site count.
Known-inert cases go in KNOWN below with a reason, so the checker stays at
zero and a NEW one is visible.
"""
import os
import re
import sys
import tomllib

SKIP_DIRS = {".git", "target", "node_modules", "vendor"}

# Crate-relative manifest path -> {feature: why it is left undeclared}.
# Empty on purpose: both current findings are BUGS awaiting a decision
# (C18_TODO §12), not accepted states. Add an entry only for a feature that
# is deliberately external — and say who supplies it.
KNOWN = {}

# `#[cfg(...)]` / `#![cfg(...)]` opening a line, or the cfg!() macro anywhere.
ATTR = re.compile(r'^\s*#!?\[\s*cfg', )
MACRO = re.compile(r'\bcfg!\s*\(')
FEAT = re.compile(r'feature\s*=\s*"([^"]+)"')


def declared_features(manifest):
    feats = set(manifest.get("features", {}) or {})
    def opt(deps):
        for name, spec in (deps or {}).items():
            if isinstance(spec, dict) and spec.get("optional"):
                feats.add(spec.get("package", name))
    for key in ("dependencies", "build-dependencies", "dev-dependencies"):
        opt(manifest.get(key))
    for tgt in (manifest.get("target") or {}).values():
        for key in ("dependencies", "build-dependencies", "dev-dependencies"):
            opt(tgt.get(key))
    return feats


def referenced_features(src_dir):
    """feature name -> number of real cfg sites."""
    hits = {}
    for root, dirs, files in os.walk(src_dir):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for f in files:
            if not f.endswith(".rs"):
                continue
            path = os.path.join(root, f)
            for line in open(path, encoding="utf-8", errors="replace"):
                if not (ATTR.match(line) or MACRO.search(line)):
                    continue
                for name in FEAT.findall(line):
                    hits[name] = hits.get(name, 0) + 1
    return hits


def main():
    root = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else ".")
    scanned = 0
    problems = []

    for dp, dn, fn in os.walk(root):
        dn[:] = [d for d in dn if d not in SKIP_DIRS]
        if "Cargo.toml" not in fn:
            continue
        ct = os.path.join(dp, "Cargo.toml")
        try:
            m = tomllib.load(open(ct, "rb"))
        except Exception:
            continue
        if "package" not in m:
            continue
        src = os.path.join(dp, "src")
        if not os.path.isdir(src):
            continue
        scanned += 1
        rel = os.path.relpath(ct, root)
        declared = declared_features(m)
        allowed = KNOWN.get(rel, {})
        missing = {
            name: n for name, n in referenced_features(src).items()
            if name not in declared and name not in allowed
        }
        if missing:
            problems.append((rel, missing))

    # A scan that matched nothing passes every assertion below it, so assert a
    # floor on the scan itself first — CLAUDE.md §5b, habit 3.
    if scanned < 100:
        print(f"cfg_feature_check: FAIL — only {scanned} crates scanned, "
              "expected >= 100. The walk is broken, not the tree.")
        return 1

    if problems:
        print(f"cfg_feature_check: FAIL — {len(problems)} crate(s) of {scanned} "
              "gate code on a feature no manifest declares\n")
        for rel, missing in sorted(problems):
            print(f"  {rel}")
            for name, n in sorted(missing.items(), key=lambda kv: -kv[1]):
                print(f'       cfg(feature = "{name}")  x{n} '
                      "— never compiles; the `not(...)` arm is permanent")
        print("\n  Either declare the feature, delete the dead arm, or archive "
              "the module per CLAUDE.md §7b.")
        return 1

    print(f"cfg_feature_check: PASS — {scanned} crates, every cfg(feature) "
          "resolves to a declared feature or optional dependency")
    return 0


if __name__ == "__main__":
    sys.exit(main())
