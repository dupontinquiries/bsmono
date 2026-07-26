# sanity — no-compile edit gates

Six single-purpose static primitives for checking an edit BEFORE any build runs.
`tools/rustcheck` answers "did my change break the build?" with the compiler as its
final tier; `sanity` is the layer under it — instant, zero-compile, no cargo, no
deps — for the mistake classes a careful editor makes precisely when compilation is
expensive or rationed. Every primitive below exists because its bug class actually
happened in this repo's history (the motivating incident is named for each).

All tools: `<tool> [--flags] FILE|DIR...` — dirs recurse to the relevant extensions,
findings print as `file:line:col: message` (grep/editor-friendly), exit code is
non-zero iff something was found. std-only; build with plain
`rustc --edition 2021 src/bin/<tool>.rs` or via cargo, either toolchain.

| tool | catches | motivating incident |
|------|---------|--------------------|
| `bracecheck` | unbalanced `()[]{}` — Rust-aware (strings, raw strings, char-vs-lifetime, nested block comments) or `--plain` for C/GLSL/JSON | the extracted test harness died on "unclosed delimiter" after a mis-sliced block |
| `tomlstrings` | TOML basic/literal strings not closed on their line (multi-line `"""`/`'''` respected) | `manifest/projects.toml` shipped a value ending `\"` — the escape ate the closing quote and everything after became string |
| `aritycheck` | call sites whose argument count drifted from the (single) `fn` definition in the scanned set | four `moe_config(...)` tests still passed 4 args after the signature grew a 5th |
| `msrvcheck` | std APIs newer than `--msrv` (default 1.75) | `Option::is_none_or` (1.82) written under the 1.75 toolchain |
| `derivecheck` | `#[derive(Eq)]` / `#[derive(Hash)]` on types with `f32`/`f64` fields | `MoeConfig` gained `scale: f32` and its `Eq` derive stopped compiling |
| `modcheck` | `mod X;` with no `X.rs`/`X/mod.rs`, and orphan `.rs` files no `mod` names | `openworld.rs` sat unwired until its `pub mod` line landed |

Honest limits: these are lexers and pattern scanners, not parsers. `aritycheck`
skips macros (`name!(`), method calls when the definition takes `self`, and any name
defined more than once in the scanned set (overloads-by-module would false-positive).
`msrvcheck` matches method-name occurrences, so a same-named method on your own type
is flagged conservatively. `derivecheck` reads only the item immediately following
the derive. When a primitive is silent that's necessary, not sufficient — rustcheck's
compiler tier remains the truth.
