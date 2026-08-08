# Lints that are becoming hard errors

Written after a report that "rust new compiler issues that used to be warnings
are now compilation blockers". I cannot reproduce a newer toolchain here — this
container is on 1.96.0 — so rather than guess at the message, this is a scan for
the patterns that are known to flip, and what the tree actually contains.

## `static_mut_refs` — the one with real exposure

Taking a reference to a `static mut` is a warning today and a hard error in
edition 2024. Three sites:

| site | verdict |
|---|---|
| `apps/ambient-learner/src/main.rs` — `LAST_SVG_HASH`, `CACHED_SVG` | **fixed** |
| `apps/fao-codec/fao_codec_core/src/dct.rs` — `DCT` | probably safe, see below |
| `apps/fao-codec/fao_codec_lib/src/library_db.rs` | already migrated; its comment names the 2024 pattern |

**ambient-learner was the real one.** `if let Some(svg) = &CACHED_SVG` takes a
reference outright, and `CACHED_SVG.is_none()` autorefs. Replaced with a
`thread_local! { RefCell<(u64, Option<String>)> }` — this is a GTK callback and
therefore always on the main thread, so a thread_local is the closest safe
equivalent: same lifetime, same single-threaded assumption, no `unsafe` and no
lint to suppress.

**The DCT one is likely fine and was left alone.** `DCT[k][i] = …` on a
`[[f32; 8]; 8]` is built-in array indexing, which is a place projection rather
than a reference, so `static_mut_refs` should not fire. It is also a hot codec
path, and rewriting it to `OnceLock` without a benchmark is the kind of change
that trades a lint for a regression nobody measures. Left with this note so
whoever hits it on a 2024 build knows it was considered rather than missed.

## What else to expect, unverified

Not present in this tree as far as a scan can tell, but the usual list when a
toolchain moves:

* `unsafe_op_in_unsafe_fn` — unsafe operations inside an `unsafe fn` without an
  inner `unsafe` block. Several crates here already `#![forbid]` it deliberately.
* `dependency_on_unit_never_type_fallback` and
  `never_type_fallback_flowing_into_unsafe`.
* `ambiguous_glob_reexports`.
* `elided_lifetimes_in_associated_constant`.

## The useful diagnostic

`cargo check` on the affected crate with `--message-format=short` and the
toolchain in question names both the lint and the line. The lint NAME is the
thing to carry back — "used to be a warning" describes a hundred lints and fixes
none of them, whereas `static_mut_refs` points at exactly three files here.
