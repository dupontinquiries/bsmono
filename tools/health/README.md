# health — quick repo-health assessment

Four std-only scanners for the monorepo's actual diseases (each was hurting before its
tool existed). Where `tools/sanity` gates a single edit, `health` reads the whole tree
and reports; exit codes are 0 unless `--fail-on` thresholds say otherwise, because a
health report is information first, gate second.

| tool | reports | why it exists here |
|------|---------|--------------------|
| `depdrift` | every external dependency used at ≥2 distinct version requirements across all Cargo.tomls, with the crates on each side; plus a Cargo.lock census (lockfile format v3/v4, presence) | the egui 0.31/0.34 + wgpu 22/29 split is this repo's largest structural cost — it decided what was even buildable per toolchain |
| `hotspots` | longest functions and largest files in a tree (lexer-accurate brace matching, not grep) | fao_app's 12.9K-line main.rs and several 10K-line lib.rs files are where every hard edit slows down |
| `debtscan` | TODO/FIXME/HACK/XXX markers (scanned in comments, where they live) and code-smell calls — unwrap/expect/panic/todo!/unimplemented!/dbg! — scanned in CODE ONLY via the lexer, so a doc-string mention isn't debt | separates "notes to self" from "will abort at runtime"; dbg! in particular must be zero in committed code |
| `testmap` | per-crate test counts (#[test] fns, tests/ files) vs src line counts; crates with ZERO tests listed first | the repo's whole method is suite-gated change; an untested crate is where that method silently doesn't apply |

All: `<tool> [flags] DIR...`, grep-friendly output, `--help` on each. Health numbers are
diagnostic, not moral: a generated file atop hotspots or a bin-only crate in testmap's
zero list can be fine — the point is that someone LOOKED.
