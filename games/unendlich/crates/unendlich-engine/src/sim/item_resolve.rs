//! `sim::item_resolve` — the loot→item resolver (round R28).
//!
//! A single pure, seeded-deterministic entry point, [`resolve`], that runs the
//! existing loot/item cores in pipeline order and emits a finished
//! [`ResolvedItem`] descriptor:
//!
//! ```text
//!   drop_chance → loot_roll → loot_quantity → rarity_tier → affix_count
//!     → affix_magnitude → item_modifier → gem_socket → set_bonus
//! ```
//!
//! This module **composes** those cores; it does not reimplement their math.
//! Each stage's role here:
//! * `drop_chance` — gate: a seeded uniform vs the drop probability. Fail ⇒ no item.
//! * `loot_roll`   — gate + identity: which table row landed (first one picked).
//! * `loot_quantity` — the dropped quantity.
//! * `rarity_tier` — the item's rarity `0..=5`, from a seeded quality score.
//! * `affix_count` — the per-item affix budget (level + luck).
//! * `item_modifier` — the affix modifier ids; its count scales with rarity, so
//!   a higher rarity item carries more affixes (capped by the budget).
//! * `affix_magnitude` — one seeded magnitude per kept affix.
//! * `gem_socket` — socketed gems added onto the affix stat block.
//! * `set_bonus` — the set bonus for the equipped piece count.
//!
//! All randomness is derived from the caller-supplied `seed` via documented
//! salts (R20) and a local SplitMix64 — there is NO global RNG, no clock, no
//! I/O, and no `.unwrap()`/`.expect()`. Logic in, data out.

use crate::sim::item_appraise::{appraise, AppraiseInput};
use crate::sim::item_compare::{ItemView, N_AFFIX, N_GENE};
use crate::sim::market_price;
use crate::sim::{
    affix_count, affix_magnitude, drop_chance, gem_socket, item_modifier, loot_quantity,
    loot_roll, rarity_tier, set_bonus,
};

/// Inputs to one loot→item resolution. Borrowed tables keep it allocation-free
/// at the call site; everything is plain data so the result is reproducible
/// across peers and rollback from `seed` alone.
#[derive(Clone, Debug)]
pub struct ResolveInput<'a> {
    /// Master seed; all internal rolls derive from this (documented salts below).
    pub seed: u64,
    // ── drop_chance ──
    pub base_drop_rate: f32,
    pub luck: f32,
    pub rarity_penalty: f32,
    // ── loot_roll ── `(item_id, count, chance)` rows.
    pub loot_table: &'a [(u16, u32, f32)],
    // ── loot_quantity ──
    pub enemy_tier: u32,
    pub max_drops: u32,
    // ── rarity / affixes ──
    pub item_level: u32,
    pub max_affixes: u32,
    pub affix_base: f32,
    /// `(modifier_id, weight)` pool for `item_modifier`.
    pub modifier_pool: &'a [(u16, f32)],
    // ── gem_socket ── `(stat_index, amount)`.
    pub gems: &'a [(u8, f32)],
    // ── set_bonus ──
    pub set_count: u8,
    /// `(required_count, bonus)` thresholds.
    pub set_thresholds: &'a [(u8, f32)],
}

/// A fully-resolved item descriptor. `modifiers` and `affix_magnitudes` are
/// parallel (same length = the number of affixes kept).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedItem {
    pub item_id: u16,
    pub quantity: u32,
    /// Rarity tier `0..=5` (common … mythic).
    pub rarity: u8,
    pub modifiers: Vec<u16>,
    pub affix_magnitudes: Vec<f32>,
    pub set_bonus: f32,
}

// Documented roll salts (R20): each stage mixes the master seed with a distinct
// salt so its stream decorrelates from the others (and from the cores' own use
// of the raw seed).
const SALT_DROP: u64 = 0xD401_0D40_0D40_0001;
const SALT_LOOT: u64 = 0x100D_0100_0D01_0002;
const SALT_RARITY: u64 = 0x9A12_C0DE_9A12_0003;
const SALT_MOD: u64 = 0x33AF_F1C0_33AF_0004;
const SALT_MAG: u64 = 0x5A6E_17DE_5A6E_0005;
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Derive a decorrelated sub-stream u64 from `(seed, salt)`.
fn sub(seed: u64, salt: u64) -> u64 {
    mix64(seed ^ salt.wrapping_mul(GOLDEN))
}

/// Deterministic uniform `[0,1)` for a derived seed.
fn unit(seed: u64) -> f32 {
    (mix64(seed) >> 40) as f32 / (1u64 << 24) as f32
}

fn clamp01(x: f32) -> f32 {
    crate::util::mathx::clamp01(x) // r343 dedup
}

/// Run the loot→item pipeline. Returns `None` when nothing drops (the
/// `drop_chance` roll fails, or the loot table yields no row).
pub fn resolve(input: &ResolveInput) -> Option<ResolvedItem> {
    let seed = input.seed;

    // 1. drop_chance — seeded gate. roll ≥ p ⇒ nothing dropped.
    let p = drop_chance::drop_chance(input.base_drop_rate, input.luck, input.rarity_penalty);
    if unit(sub(seed, SALT_DROP)) >= p {
        return None;
    }

    // 2. loot_roll — which rows landed; pick the first as the resolved item.
    let drops = loot_roll::roll(input.loot_table, sub(seed, SALT_LOOT));
    let (item_id, _row_count) = match drops.first() {
        Some(&row) => row,
        None => return None,
    };

    // 3. loot_quantity — how many of it dropped.
    let quantity = loot_quantity::quantity(input.enemy_tier, input.luck, input.max_drops);

    // 4. rarity_tier — quality score (seed + luck) → tier 0..=5.
    let score = clamp01(unit(sub(seed, SALT_RARITY)) * 0.5 + clamp01(input.luck) * 0.5);
    let rarity = rarity_tier::tier(score);
    let rarity_unit = rarity as f32 / 5.0; // [0,1] for item_modifier

    // 5. affix_count — the affix budget for this item.
    let budget = affix_count::affix_count(input.item_level, input.luck, input.max_affixes) as usize;

    // 7. item_modifier — modifier ids; count scales with rarity. Keep at most
    //    `budget`, so higher rarity ⇒ more affixes (up to the level/luck budget).
    let mut modifiers = item_modifier::roll_mods(rarity_unit, input.modifier_pool, sub(seed, SALT_MOD));
    if modifiers.len() > budget {
        modifiers.truncate(budget);
    }
    let n_affix = modifiers.len();

    // 6. affix_magnitude — one seeded magnitude per kept affix.
    let mut magnitudes: Vec<f32> = Vec::with_capacity(n_affix);
    let mut i = 0usize;
    while i < n_affix {
        let roll = unit(sub(seed, SALT_MAG ^ (i as u64).wrapping_mul(GOLDEN)));
        magnitudes.push(affix_magnitude::magnitude(input.affix_base, input.item_level, roll));
        i += 1;
    }

    // 8. gem_socket — socketed gems add onto the affix stat block (length kept).
    let affix_magnitudes = gem_socket::apply_gems(&magnitudes, input.gems);

    // 9. set_bonus — bonus for the equipped set-piece count.
    let set_bonus = set_bonus::bonus(input.set_count, input.set_thresholds);

    Some(ResolvedItem {
        item_id,
        quantity,
        rarity,
        modifiers,
        affix_magnitudes,
        set_bonus,
    })
}

/// View a resolved loot item as an [`item_compare::ItemView`] so a drop can be
/// compared against the equipped item (the tooltip's up/down arrows). The
/// resolved affix magnitudes map one-per-slot into the integer affix slots
/// (rounded; padded/truncated to `N_AFFIX`) — the same "affix magnitude" concept
/// in both types. Loot carries no heritable genes here, so the gene vector is
/// zero. Pure. Connects the loot resolver to the comparison core (previously
/// without a caller).
pub fn to_item_view(item: &ResolvedItem) -> ItemView {
    let mut affixes = [0i32; N_AFFIX];
    for (slot, mag) in affixes.iter_mut().zip(item.affix_magnitudes.iter()) {
        *slot = mag.round() as i32;
    }
    ItemView { affixes, genes: [0.0; N_GENE], rarity: item.rarity }
}

/// Trade value of a resolved loot item: bridges a [`ResolvedItem`] into
/// [`item_appraise::appraise`]. Loot carries no craft quality or durability
/// context, so quality is neutral and durability is not applicable; the price is
/// driven by `rarity`, modifier count, `base_value`, and the `set_bonus` (folded
/// into the base value — a set piece is worth more). Always finite and `≥ 0`;
/// non-decreasing in rarity, modifier count, base value, and set bonus. Connects
/// the loot resolver to the appraisal core (previously disjoint).
pub fn appraise_resolved(item: &ResolvedItem, base_value: f32) -> f32 {
    let base = base_value.max(0.0) * (1.0 + item.set_bonus.max(0.0));
    appraise(&AppraiseInput {
        base_value: base,
        rarity: item.rarity,
        quality: 1.0,        // loot has no craft-quality roll
        enchant_level: 0,
        durability_cur: 0,
        durability_max: 0,   // not applicable → no durability penalty
        modifiers: &item.modifiers,
    })
}

/// Live market sale price for a resolved loot item: its intrinsic appraised value
/// ([`appraise_resolved`]) put through market `demand`
/// ([`market_price::market_value`], which does not re-apply rarity). One call from
/// a drop to what a vendor pays at the current demand. Pure.
pub fn market_price_resolved(item: &ResolvedItem, base_value: f32, demand: f32) -> f32 {
    market_price::market_value(appraise_resolved(item, base_value), demand)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A generous baseline that always drops (chance 1.0) with a rich modifier
    // pool and budget, so affixes are present to exercise the later stages.
    fn base<'a>(
        seed: u64,
        luck: f32,
        gems: &'a [(u8, f32)],
        set_count: u8,
        thresholds: &'a [(u8, f32)],
    ) -> ResolveInput<'a> {
        ResolveInput {
            seed,
            base_drop_rate: 1.0,
            luck,
            rarity_penalty: 0.0,
            loot_table: &[(7u16, 3u32, 1.0f32)],
            enemy_tier: 2,
            max_drops: 5,
            item_level: 100,
            max_affixes: 5,
            affix_base: 100.0,
            modifier_pool: &[(10u16, 1.0f32), (11, 1.0), (12, 1.0), (13, 1.0), (14, 1.0)],
            gems,
            set_count,
            set_thresholds: thresholds,
        }
    }

    #[test]
    fn failed_drop_yields_no_item() {
        let no_thresh: [(u8, f32); 0] = [];
        for seed in 0..512u64 {
            let mut inp = base(seed, 1.0, &[], 0, &no_thresh);
            inp.base_drop_rate = 0.0; // p == 0 ⇒ always fails
            assert!(resolve(&inp).is_none(), "seed {seed} should drop nothing");
        }
    }

    #[test]
    fn empty_table_yields_no_item() {
        let no_thresh: [(u8, f32); 0] = [];
        let mut inp = base(123, 1.0, &[], 0, &no_thresh);
        inp.loot_table = &[];
        assert!(resolve(&inp).is_none());
    }

    #[test]
    fn fixed_seed_yields_stable_item() {
        let no_thresh: [(u8, f32); 0] = [];
        let inp = base(0xABCD_1234, 0.5, &[], 0, &no_thresh);
        let a = resolve(&inp);
        let b = resolve(&inp);
        assert!(a.is_some());
        assert_eq!(a, b, "same input must resolve identically");
        let item = a.unwrap();
        assert_eq!(item.item_id, 7);
        assert!(item.quantity <= inp.max_drops);
    }

    #[test]
    fn higher_rarity_gates_more_affixes() {
        let no_thresh: [(u8, f32); 0] = [];
        let mut found_strict = false;
        for seed in 0..256u64 {
            let low = resolve(&base(seed, 0.0, &[], 0, &no_thresh));
            let high = resolve(&base(seed, 1.0, &[], 0, &no_thresh));
            // base_drop_rate 1.0 ⇒ both always drop.
            let (low, high) = match (low, high) {
                (Some(l), Some(h)) => (l, h),
                _ => panic!("generous input should always drop (seed {seed})"),
            };
            assert!(high.rarity >= low.rarity, "rarity monotone in luck (seed {seed})");
            assert!(
                high.affix_magnitudes.len() >= low.affix_magnitudes.len(),
                "affixes monotone in rarity (seed {seed})"
            );
            if high.affix_magnitudes.len() > low.affix_magnitudes.len() {
                found_strict = true;
            }
        }
        assert!(found_strict, "higher rarity should produce strictly more affixes for some seed");
    }

    #[test]
    fn sockets_apply_when_present() {
        let no_thresh: [(u8, f32); 0] = [];
        // High luck ⇒ high rarity ⇒ affixes present; gem adds onto stat 0.
        let with_gem = base(99, 1.0, &[(0u8, 1000.0f32)], 0, &no_thresh);
        let without = base(99, 1.0, &[], 0, &no_thresh);
        let g = resolve(&with_gem).expect("drops");
        let n = resolve(&without).expect("drops");
        assert!(!n.affix_magnitudes.is_empty(), "need an affix to socket into");
        assert_eq!(g.affix_magnitudes.len(), n.affix_magnitudes.len(), "gems keep length");
        assert!(
            (g.affix_magnitudes[0] - (n.affix_magnitudes[0] + 1000.0)).abs() < 1e-3,
            "gem adds its amount to stat 0"
        );
    }

    #[test]
    fn set_bonus_applies_when_present() {
        let thresholds = [(2u8, 0.1f32), (4, 0.25), (6, 0.5)];
        let none = base(5, 0.5, &[], 1, &thresholds); // below first threshold
        let met = base(5, 0.5, &[], 4, &thresholds); // crosses (4,0.25)
        assert_eq!(resolve(&none).expect("drops").set_bonus, 0.0);
        assert!((resolve(&met).expect("drops").set_bonus - 0.25).abs() < 1e-6);
    }

    #[test]
    fn invariants_hold_over_many_seeds() {
        let thresholds = [(2u8, 0.1f32), (4, 0.25)];
        for seed in 0..512u64 {
            if let Some(item) = resolve(&base(seed, 0.7, &[(0, 5.0)], 3, &thresholds)) {
                assert!(item.rarity <= 5, "rarity bounded");
                assert!(item.quantity <= 5, "quantity ≤ max_drops");
                assert_eq!(
                    item.modifiers.len(),
                    item.affix_magnitudes.len(),
                    "modifiers and magnitudes are parallel (seed {seed})"
                );
                assert!(
                    item.affix_magnitudes.iter().all(|m| m.is_finite() && *m >= 0.0),
                    "magnitudes finite & non-negative (seed {seed})"
                );
                assert!(item.set_bonus >= 0.0);
            }
        }
    }

    #[test]
    fn quantity_tracks_loot_quantity_core() {
        let no_thresh: [(u8, f32); 0] = [];
        // enemy_tier 2 + luck 0 ⇒ quantity 2 (matches loot_quantity semantics).
        let mut inp = base(1, 0.0, &[], 0, &no_thresh);
        inp.enemy_tier = 2;
        assert_eq!(resolve(&inp).expect("drops").quantity, 2);
    }

    fn resolved(rarity: u8, mods: Vec<u16>, set: f32) -> ResolvedItem {
        ResolvedItem { item_id: 1, quantity: 1, rarity, modifiers: mods, affix_magnitudes: vec![], set_bonus: set }
    }

    #[test]
    fn appraise_resolved_rises_with_rarity_mods_base_and_set() {
        let plain = appraise_resolved(&resolved(1, vec![], 0.0), 10.0);
        assert!(appraise_resolved(&resolved(5, vec![], 0.0), 10.0) > plain, "rarer is worth more");
        assert!(appraise_resolved(&resolved(1, vec![7, 8], 0.0), 10.0) > plain, "more modifiers worth more");
        assert!(appraise_resolved(&resolved(1, vec![], 0.0), 20.0) > plain, "higher base worth more");
        assert!(appraise_resolved(&resolved(1, vec![], 0.5), 10.0) > plain, "set bonus is worth more");
        assert!(plain.is_finite() && plain >= 0.0);
    }

    #[test]
    fn appraise_resolved_zero_base_is_zero() {
        let it = resolved(4, vec![1, 2], 0.9);
        assert_eq!(appraise_resolved(&it, 0.0), 0.0, "no base value ⇒ no trade value");
    }

    #[test]
    fn to_item_view_maps_fields_and_compares_against_equipped() {
        use crate::sim::item_compare::{compare, N_GENE};
        let equipped = to_item_view(&resolved(2, vec![], 0.0)); // rarity 2, no affixes
        let mut drop = resolved(4, vec![], 0.0); // rarer
        drop.affix_magnitudes = vec![3.0, 1.0]; // two affix slots
        let candidate = to_item_view(&drop);
        assert_eq!(candidate.rarity, 4);
        assert_eq!(candidate.affixes[0], 3);
        assert_eq!(candidate.affixes[1], 1);
        assert_eq!(candidate.genes, [0.0; N_GENE], "loot has no genes");
        // candidate vs equipped: rarity and the two affix slots are all upgrades.
        let d = compare(&equipped, &candidate);
        assert_eq!(d.rarity_delta, 2);
        assert_eq!(d.affix_delta[0], 3);
        assert_eq!(d.affix_delta[1], 1);
    }

    #[test]
    fn market_price_resolved_tracks_demand_and_value() {
        let it = resolved(3, vec![1], 0.0);
        let low = market_price_resolved(&it, 50.0, 0.5);
        let high = market_price_resolved(&it, 50.0, 1.5);
        assert!(high > low, "more demand ⇒ higher sale price");
        let richer = resolved(5, vec![1, 2, 3], 0.5);
        assert!(
            market_price_resolved(&richer, 50.0, 1.0) > market_price_resolved(&it, 50.0, 1.0),
            "a rarer, more-modified, set-bonused item sells for more at equal demand"
        );
        assert!(low.is_finite() && low >= 1.0);
    }
}
