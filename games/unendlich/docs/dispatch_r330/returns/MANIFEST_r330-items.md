# MANIFEST — `r330-items` (T-items: item/armor lore tooltip assembly)

**WID:** r330-items
**Base:** unendlich-r330b-mp-net_channels-and-dispatch-VERIFIED
**Status:** **VERIFIED** in-sandbox — pure core, logic actually executed (10/10 tests pass,
compile-clean, zero warnings). No bin/GPU/visual claims (it is pure data composition).

## Files
| File | Change |
|---|---|
| `crates/unendlich-engine/src/sim/item_tooltip.rs` | **new** — the pure tooltip core (264 non-test LOC + 10 tests) |
| `crates/unendlich-engine/src/sim/mod.rs` | **+1 line** — `pub mod item_tooltip;` (SHARED_RULES §4: one decl only) |
| `HANDOFF_items_tooltip.md` | bin-wiring proposal for `render/world_renderer.rs` (propose, not wired) |
| `CHANGES-r330-items.patch` | git-style unified diff (mod.rs hunk + new file); applies `-p1` |

## API added (`sim::item_tooltip`)
- `enum TooltipRole { Title, Affix, Lore, Flavor }`
- `enum Emphasis { Strong, Normal, Muted }`
- `struct TooltipLine { text: String, role: TooltipRole, emphasis: Emphasis }`
- `struct AffixView<'a> { label: &'a str, magnitude: f32 }`
- `struct ItemTooltipView<'a> { name, rarity: u8, kind_label: Option<&str>, slot_label: Option<&str>, affixes: &[AffixView], lore: &str, attribution: Option<&str>, lore_cols: usize }`
- `fn rarity_name(tier: u8) -> &'static str`
- `fn compose(item: &ItemTooltipView) -> Vec<TooltipLine>`
- `fn wrap_lore(text: &str, max_cols: usize) -> Vec<String>`

## Design grounding
`docs/design/item_armor_lore_tooltips.extracted.md` "Tooltip Format Specification":
name → `kind · rarity · slot` subtitle → effect lines → blank separator → italic lore →
`—` attribution. Rarity ordering matches `sim::rarity_tier` (0 Common…5 Mythic). Affix
list/magnitudes are the outputs of `sim::affix_count` / `sim::affix_magnitude`; name/kind
come from `item.rs` (`ItemKind`/`MetalKind`). View is flattened by the caller so the module
is dependency-free and standalone-testable.

## Output contract (deterministic)
title (Title/Strong) → subtitle/rarity (Title/Muted) → affixes (Affix/Normal, **magnitude
desc, stable for ties, NaN sinks last**) → blank separator (Lore/Muted, **only if lore**) →
wrapped lore body (Lore/Normal) → attribution (Flavor/Muted, only if present). A title line
is always emitted (even affix-less, lore-less). `wrap_lore` never splits a word (so never a
grapheme), counts width in `char`s (multibyte-safe), normalises whitespace, returns empty
for empty input, and treats `max_cols == 0` as no-wrap (no loop/division hazard).

## Purity (SHARED_RULES §1)
No `unwrap`/`expect`/`panic!`/`unreachable!`/`todo!`, no IO/clock/RNG in non-test code
(grep-confirmed; the only match was the doc comment naming them). Saturating arithmetic on
column math. No new dependencies. NaN magnitudes handled via `f32::total_cmp` on a NaN→
`NEG_INFINITY` key (no `partial_cmp` unwrap).

## Verify commands + results (exact)
Toolchain note: the documented offline `rustup.zip` (Rust 1.96) was **not** uploaded, and
`static.rust-lang.org` is not allow-listed, so I used the Ubuntu-archive `rustc 1.75`
(allow-listed `archive.ubuntu.com`). For a **dependency-free, `crate::`-free `sim::` module**
this is sufficient — the standalone gate compiles the file directly (no `Cargo.lock`, no v4
lock-dance), and the module uses only edition-2021 std. Version noted for honesty; coordinator
re-runs under 1.96 on merge.

```
$ cp crates/unendlich-engine/src/sim/item_tooltip.rs /tmp/item_tooltip.rs
$ CARGO_INCREMENTAL=0 rustc --edition 2021 --test /tmp/item_tooltip.rs -o /tmp/tt
   → compiles clean, 0 warnings
$ /tmp/tt
   → running 10 tests … test result: ok. 10 passed; 0 failed; 0 ignored
```
Tests: `composes_in_documented_order`, `affixes_sorted_by_magnitude_desc_stably`,
`lore_wraps_without_splitting_words_on_ascii`, `empty_affix_item_still_yields_title_line`,
`compose_is_deterministic`, `nan_magnitude_does_not_panic_and_sorts_last`,
`wrap_lore_handles_zero_cols_empty_and_long_word`, `rarity_name_covers_all_tiers_and_saturates`,
`wrap_lore_never_splits_a_multibyte_char`, `separator_present_only_with_lore`.
(≥6 required — 10 delivered, covering every Verify bullet in the brief.)

Extra static nets: `handoff/check_delimiters.py` → **OK**; `git apply -p1 --check
CHANGES-r330-items.patch` against a pristine base → **APPLIES CLEAN**.

## NOT done (honest scope)
- Full-workspace `cargo build`/`cargo test --features "render-core egui"` **not** run
  (no 1.96 toolchain + lock v4 + glfw native deps; the module is `crate::`-free so the
  standalone gate is the documented substitute — SHARED_RULES §3). Coordinator runs the
  workspace build on merge.
- Bin **not** wired (SHARED_RULES §2) — see `HANDOFF_items_tooltip.md`.

## Merge
First in the dispatch order (`items → bughunt → …`). Single shared-file touch is the one
`pub mod` line. Add an `## r331` ledger entry to `docs/WIRING_STATUS.md` on merge.
