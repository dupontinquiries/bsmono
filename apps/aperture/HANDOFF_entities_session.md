# aperture — what has landed, and what to do next

Five patches, applied in order to `5db8a71e` (c17unified-090). Each applies
clean, individually and in sequence; verified against a fresh clone of the
bundle after every commit.

| # | patch | verified | not verified |
|---|---|---|---|
| 1 | `aperture-core: buckets` | 169/169 core tests, 4/4 breaks | no shell wiring |
| 2 | `aperture-egui: caret policy` | `camera.rs` standalone 14/14, 4 breaks | all shell wiring |
| 3 | `aperture: Ctrl+D complement` | 177/177, 4 breaks | the aperture-egui half |
| 4 | `aperture-core: entity + nav` | 202/202, 11 breaks | nothing calls either |
| 5 | `aperture-core: edge sources` | 213/213, 6 breaks | `tree-sitter` build |

Core test count went 154 → 213. Everything in `aperture-core` was compiled
and run; NOTHING in `aperture-egui` was, and that split is the single most
important thing to know when picking this up.

## The toolchain caveat, stated once

There is no project toolchain in the sandbox this was written in. Rust came
from apt: **1.75**, against a workspace that needs 1.92 (egui 0.34). What
that cost:

* `aperture-egui` never compiled. Patches 2 and 3 contain shell code that
  has been read carefully and never built. `ctx.input(|i| i.focused)` in
  patch 2 is the single likeliest thing to be wrong — it is the egui 0.34
  spelling from memory.
* `--features tree-sitter` never compiled: `tree-sitter 0.25` pulls
  `indexmap 2.14`, which requires edition2024. So `edgesrc::OutlineRow::
  from_items` — the only feature-gated code added — is unbuilt.
* `apps/aperture/Cargo.lock` is lockfile v4 and cargo 1.75 cannot read it.
  It was moved aside during every test run and restored before every
  commit. **No patch touches it** (checked; one earlier draft did, and that
  was amended out).
* One 1.82-ism in `voice.rs` (`Option::is_none_or`) was temporarily
  back-ported to get the crate to compile at all, and reverted every time.
  It is in none of the patches.

First thing to run on a real machine:

```
cargo test -p aperture-core                      # expect 213
cargo test -p aperture-core --features tree-sitter
cargo check -p aperture-egui --ignore-rust-version
```

## What the five patches add up to

The reconciliation's premise was that aperture had every edge SOURCE the
design conversation named except callers, and no edge LAYER to put them in.
That layer now exists and has three sources feeding it:

* `entity` — `EntityId`, `Relation` with inverses, `Edge` carrying the
  `Site` it was observed at, `Graph` with merge-not-replace and provenance.
* `nav` — paths made of edges, so "which call brought me here" survives.
* `edgesrc` — outline containment, search hits, git blame.
* `bucket` — ordered packs of places, persisted as git refs.

## Next, in order

1. **Wire something.** All of the above is unreachable from the UI. The
   smallest useful step is item E's shell half: push a `NavStep` in
   `jump_to_lsp_definition` and bind Alt+Left. It is the first thing a
   person would actually feel.
2. **Item D** — `textDocument/references` in `aperture-lsp`. The graph has
   no caller edges, which was the one genuine capability gap.
3. **Item C5** — the approximate tree-sitter caller tier. The merge rule is
   already built and tested for it.
4. **Item H's shell half** — `workspace_root()` becomes a
   `bucket::Workspace`, plus the `Mode::Bucket` palette.

Everything else is in `TODO_entities_buckets_paths.md`, which is the file to
read first and the one to keep updated.

## Things found along the way that are not in any patch

* `apps/aperture/HANDOFF.md` says logscope-egui has no `scene-kit`
  dependency. It does now — `logscope-egui/Cargo.toml:24`. Correct it.
* Root `TODO.md`'s note about `power_mode()` silently overruling the
  quality dial is still unfixed and still worth fixing before aperture
  grows a quality dial of its own.
* `maintenance/C12_TODO.md` §7 is the seed of this whole line of work and
  should point at the addendum rather than be deleted.
