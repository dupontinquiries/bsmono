#!/usr/bin/env python3
"""orphan_check.py — find .rs files no `mod` declaration reaches.

WHY THIS EXISTS

`gpu-gemm/src/specloop.rs` sat in the tree for months with no `mod specloop`
anywhere. Not "no caller" — NEVER COMPILED. Its six tests had never run, and
because nothing in the build could contradict it, two documents drifted into
disagreeing about it: `dupscan.rs` recorded the audit decision to delete it and
described that as done, while `ASAHI_GPU.md` listed it as the speculative
control loop and told the reader to wire it up. Both were written in good faith
against a file the compiler had never seen.

An orphan is worse than dead code. Dead code is compiled, type-checked and
tested, so it stays honest. An undeclared file rots silently and still reads
like the real implementation to whoever greps for it next — and in one case
here, a directory of 90 syntax-highlighted `.wgsl` shaders is dead while the
GPU runs string literals in `lib.rs`, 13 of which have already drifted apart.

TWO FALSE-POSITIVE LESSONS, both learned the hard way on the first run:

  1. DO NOT ANCHOR to line start. This tree writes
     `#[allow(dead_code)] pub mod archcfg;` with the attribute on the same
     line. An `^\\s*` anchor missed every one and reported archcfg, memplan and
     moe as orphans when their tests demonstrably run.
  2. DO EXCLUDE COMMENTS. Unanchoring alone made a prose comment reading
     "adding `mod kernels;`" count as a declaration — a comment written about
     that very orphan, which then hid it.

     Stripping string literals as well was tried and REVERTED: pathologically
     slow on a 20k-line lib.rs full of raw-string WGSL, and over-eager enough
     to turn feature-gated modules into false orphans. Prose that needs to
     mention a declaration should avoid writing it in scannable form — which
     is a cheaper fix than teaching a regex to lex Rust.
  3. DO READ `[[bin]]` PATHS from Cargo.toml. `gpu-gemm/src/serve.rs` is a
     binary target, not a module, and reporting it as an orphan is noise.

Scope: every crate under the given root. A file is reachable if any `mod <stem>`
declaration appears in a non-comment position anywhere in its crate, since a
module declared inside another module is still compiled.
"""
import pathlib, re, sys

# `mod x;` or `mod x {`, anywhere on a line, but not after a // comment marker.
MOD = re.compile(r'\bmod\s+(\w+)\s*[;{]')
LINE_COMMENT = re.compile(r'//.*$')
BLOCK_COMMENT = re.compile(r'/\*.*?\*/', re.S)
BIN_PATH = re.compile(r'^\s*path\s*=\s*["\']([^"\']+)["\']', re.M)


def declarations(text: str) -> set:
    """Module names declared in CODE — comments and string literals removed.

    Comments only. Stripping STRING LITERALS too was tried and reverted: on
    gpu-gemm's 20k-line lib.rs, which is full of raw-string WGSL, the regex was
    pathologically slow, and it over-stripped enough to turn feature-gated
    modules into false orphans. The one case it would have caught — a Rust
    string containing the phrase `mod kernels;` — was fixed at the source
    instead, by not writing a scannable declaration inside prose.
    """
    text = BLOCK_COMMENT.sub("", text)
    text = "\n".join(LINE_COMMENT.sub("", ln) for ln in text.splitlines())
    return set(MOD.findall(text))


def crate_dirs(root: pathlib.Path):
    for toml in sorted(root.rglob("Cargo.toml")):
        if any(p in toml.parts for p in ("target", "vendor", ".git", "node_modules")):
            continue
        if (toml.parent / "src").is_dir():
            yield toml.parent


def target_paths(crate: pathlib.Path) -> set:
    """Files Cargo.toml names as targets — binaries, examples, benches.

    These are compiled as their own crate roots, so they are reachable without
    any `mod` declaration. Read rather than guessed: `gpu-gemm` has a
    `[[bin]] path = "src/serve.rs"`.
    """
    toml = crate / "Cargo.toml"
    try:
        return {pathlib.Path(m) for m in BIN_PATH.findall(toml.read_text(errors="replace"))}
    except OSError:
        return set()


def scan(crate: pathlib.Path):
    src = crate / "src"
    files = sorted(src.rglob("*.rs"))
    targets = target_paths(crate)
    declared = set()
    for f in files:
        try:
            declared |= declarations(f.read_text(errors="replace"))
        except OSError:
            pass

    orphans = []
    for f in files:
        stem = f.stem
        # Crate roots and auto-discovered binaries are targets, not modules.
        if stem in ("lib", "main", "mod", "build"):
            continue
        if f.parent.name in ("bin", "examples", "tests", "benches"):
            continue
        if f.relative_to(crate) in targets:
            continue
        if stem in declared:
            continue
        try:
            text = f.read_text(errors="replace")
        except OSError:
            continue
        orphans.append((f.relative_to(crate), text.count("\n") + 1, text.count("#[test]")))
    return orphans


def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "/home/claude/repo")
    total_files = total_tests = 0
    crates = 0
    for crate in crate_dirs(root):
        crates += 1
        orphans = scan(crate)
        if not orphans:
            continue
        print(f"\n{crate.relative_to(root)}")
        for rel, lines, tests in sorted(orphans):
            total_files += 1
            total_tests += tests
            print(f"    {str(rel):48s} {lines:6d} lines  {tests:3d} tests")

    print(f"\nscanned {crates} crates")
    print(f"orphaned files: {total_files}, carrying {total_tests} tests that never run")
    return 1 if total_files else 0


if __name__ == "__main__":
    sys.exit(main())
