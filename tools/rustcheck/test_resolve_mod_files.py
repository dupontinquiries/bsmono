#!/usr/bin/env python3
"""Regression tests for `resolve_mod_files` in rustcheck.py.

Written after three false-positive classes were found by a full-tree
`./verify.sh` run flagging apps/voiceterm (10x), apps/ambient-learner (1x)
and apps/rusty-board (1x) as PROBLEMS, when only rusty-board's was a real
bug. Per CLAUDE.md 5b: a check that has never failed has not been tested —
each case here is checked against BOTH a fixture that must still warn (so
the fix didn't just blind the checker) and one that must not.

Run: python3 tools/rustcheck/test_resolve_mod_files.py
"""
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import rustcheck  # noqa: E402

FAILURES = []


def check(name, files, expect_missing):
    """files: {relpath: content}. expect_missing: set of (relpath, modname)
    that resolve_mod_files should report as missing, keyed by the file it
    was found in."""
    with tempfile.TemporaryDirectory() as td:
        root = Path(td)
        for rel, content in files.items():
            fp = root / rel
            fp.parent.mkdir(parents=True, exist_ok=True)
            fp.write_text(content)
        got = set()
        for rel in files:
            fp = root / rel
            if fp.suffix != ".rs":
                continue
            for _, modname, _ in rustcheck.resolve_mod_files(fp, fp.read_text()):
                got.add((rel, modname))
        if got != expect_missing:
            FAILURES.append(f"{name}: expected {expect_missing}, got {got}")
        else:
            print(f"ok   {name}")


# 1. Stacked attributes: `#[path=...]` then `#[allow(...)]` then `mod x;`
# (the exact shape in voiceterm's memlog.rs, bars.rs, etc.) must NOT be
# flagged — but a bare missing mod with no attributes still must be.
check(
    "stacked #[path] then #[allow] suppresses the warning",
    {
        "src/lib.rs": (
            "#[path = \"other.rs\"]\n"
            "#[allow(dead_code, unused_imports)]\n"
            "pub mod aliased;\n"
            "mod genuinely_missing;\n"
        ),
        "src/other.rs": "pub fn f() {}\n",
    },
    expect_missing={("src/lib.rs", "genuinely_missing")},
)

# 2. Nested inline `mod outer { mod inner; }` must resolve `inner` against
# <base>/outer/inner.rs, not <base>/inner.rs (the ambient-learner egui_lib.rs
# shape: `pub mod ui { pub mod egui; }`).
check(
    "nested inline mod block resolves against the nested path",
    {
        "src/lib.rs": (
            "pub mod ui {\n"
            "    pub mod egui;\n"
            "    pub mod missing_inner;\n"
            "}\n"
            "mod top_level_missing;\n"
        ),
        "src/ui/egui.rs": "pub struct Shell;\n",
    },
    expect_missing={
        ("src/lib.rs", "missing_inner"),
        ("src/lib.rs", "top_level_missing"),
    },
)

# 3. A file that is a crate root only by Cargo.toml `[lib] path = "..."`
# declaration (not named main.rs/lib.rs) must resolve its top-level `mod`
# declarations relative to its own directory, not `<dir>/<stem>/`.
check(
    "manifest-declared [lib] path is treated as a crate root",
    {
        "Cargo.toml": (
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n"
            "[lib]\npath = \"src/egui_lib.rs\"\n"
        ),
        "src/egui_lib.rs": (
            "pub mod ui {\n"
            "    pub mod egui;\n"
            "}\n"
        ),
        "src/ui/egui.rs": "pub struct Shell;\n",
    },
    expect_missing=set(),
)

if FAILURES:
    print("\nFAILED:")
    for f in FAILURES:
        print(" -", f)
    sys.exit(1)
print(f"\n{3} groups, all as expected")
