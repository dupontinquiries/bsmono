//! Attribute modifiers — base stats plus stackable add/multiply modifiers,
//! resolved with the standard layered pipeline.
//!
//! The engine port of the mod's RPG token capabilities
//! (Warrior / Tank / Scout). In the mod a token applies per-attribute
//! deltas as additive `AttributeModifier`s and a per-player *value* counter
//! tracks the role's level, so the bonus scales with how invested you are.
//! Here that becomes a small, general system any source can feed —
//! tokens now, armor / tools / status effects later — keeping a single
//! authoritative computation of an entity's effective stats.
//!
//! ## The pipeline
//!
//! Each attribute resolves as the canonical three-stage formula (the same
//! `ADD` / `MULTIPLY_BASE` / `MULTIPLY_TOTAL` model the mod's tokens use the
//! `ADD` stage of):
//!
//! ```text
//! x  = base
//! x += Σ amount            (over Add modifiers)
//! x *= 1 + Σ amount        (over MultiplyBase modifiers, summed then once)
//! for each MultiplyTotal m: x *= 1 + m.amount   (applied one at a time)
//! ```
//!
//! Modifiers are tagged by `source` so a source's contribution can be
//! replaced or removed wholesale (a role re-leveling, armor unequipping)
//! rather than stacking forever.

use std::collections::HashMap;

/// A continuous entity stat that modifiers act on. Covers what the mod's
/// tokens touch, plus the obvious neighbors so armor/tools have somewhere
/// to attach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Attribute {
    MaxHealth,
    MovementSpeed,
    AttackDamage,
    AttackSpeed,
    Armor,
    KnockbackResistance,
}

/// How a modifier combines with the running value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModifierOp {
    /// Flat addition to the base sum.
    Add,
    /// Fraction of base; all `MultiplyBase` amounts are summed, then applied
    /// once: `x *= 1 + Σ`.
    MultiplyBase,
    /// Fraction applied multiplicatively, one modifier at a time:
    /// `x *= 1 + amount`.
    MultiplyTotal,
}

/// A single modifier on one attribute, tagged by its `source`.
#[derive(Debug, Clone, PartialEq)]
pub struct Modifier {
    pub source: String,
    pub op: ModifierOp,
    pub amount: f64,
}

/// The three RPG token roles from the mod, each a named source of additive
/// per-attribute deltas (the level-1 values; higher levels scale linearly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenRole {
    Warrior,
    Tank,
    Scout,
}

impl TokenRole {
    /// Per-attribute additive deltas at level 1, ported verbatim from the
    /// mod's `TokenType.setup()`. Higher levels multiply these (see
    /// [`AttributeMap::set_role_level`]).
    pub fn base_deltas(self) -> &'static [(Attribute, f64)] {
        match self {
            TokenRole::Warrior => &[
                (Attribute::AttackDamage, 0.25),
                (Attribute::MaxHealth, -0.15),
            ],
            TokenRole::Tank => &[
                (Attribute::MaxHealth, 1.0),
                (Attribute::MovementSpeed, -0.0005),
            ],
            TokenRole::Scout => &[
                (Attribute::MovementSpeed, 0.0005),
                (Attribute::KnockbackResistance, -0.0002),
            ],
        }
    }

    /// Stable modifier-source tag for this role, so re-leveling replaces.
    pub fn source(self) -> &'static str {
        match self {
            TokenRole::Warrior => "token:warrior",
            TokenRole::Tank => "token:tank",
            TokenRole::Scout => "token:scout",
        }
    }
}

/// Base stats plus a flat list of modifiers; `value` resolves the effective
/// stat for any attribute on demand.
#[derive(Debug, Clone, Default)]
pub struct AttributeMap {
    base: HashMap<Attribute, f64>,
    modifiers: Vec<(Attribute, Modifier)>,
}

impl AttributeMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set an attribute's base value (the unmodified stat).
    pub fn set_base(&mut self, attr: Attribute, value: f64) {
        self.base.insert(attr, value);
    }

    /// The unmodified base for an attribute (0 if unset).
    pub fn base(&self, attr: Attribute) -> f64 {
        self.base.get(&attr).copied().unwrap_or(0.0)
    }

    /// Add a modifier. Modifiers stack; use [`Self::remove_source`] or a
    /// `set_*` helper to replace a source's contribution instead.
    pub fn add(&mut self, attr: Attribute, modifier: Modifier) {
        self.modifiers.push((attr, modifier));
    }

    /// Remove every modifier tagged with `source` (across all attributes).
    /// Returns how many were removed.
    pub fn remove_source(&mut self, source: &str) -> usize {
        let before = self.modifiers.len();
        self.modifiers.retain(|(_, m)| m.source != source);
        before - self.modifiers.len()
    }

    /// Set a token role to `level`, replacing any prior contribution from
    /// that role with additive modifiers of `base_delta * level`. `level == 0`
    /// clears the role. This is the clean version of the mod's "use the token
    /// to raise your value, bonus scales with level" — re-leveling replaces
    /// rather than piling on duplicate modifiers.
    pub fn set_role_level(&mut self, role: TokenRole, level: u32) {
        self.remove_source(role.source());
        if level == 0 {
            return;
        }
        let scale = level as f64;
        for &(attr, delta) in role.base_deltas() {
            self.add(
                attr,
                Modifier {
                    source: role.source().to_string(),
                    op: ModifierOp::Add,
                    amount: delta * scale,
                },
            );
        }
    }

    /// The effective value of an attribute after applying all modifiers via
    /// the layered pipeline.
    pub fn value(&self, attr: Attribute) -> f64 {
        let mut add = 0.0;
        let mut mul_base = 0.0;
        let mut mul_total: Vec<f64> = Vec::new();
        for (a, m) in &self.modifiers {
            if *a != attr {
                continue;
            }
            match m.op {
                ModifierOp::Add => add += m.amount,
                ModifierOp::MultiplyBase => mul_base += m.amount,
                ModifierOp::MultiplyTotal => mul_total.push(m.amount),
            }
        }
        let mut x = self.base(attr) + add;
        x *= 1.0 + mul_base;
        for m in mul_total {
            x *= 1.0 + m;
        }
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn empty_map_returns_base() {
        let mut m = AttributeMap::new();
        m.set_base(Attribute::MaxHealth, 20.0);
        assert!(approx(m.value(Attribute::MaxHealth), 20.0));
        assert!(approx(m.value(Attribute::Armor), 0.0)); // unset → 0
    }

    #[test]
    fn additive_then_multiplicative_pipeline() {
        let mut m = AttributeMap::new();
        m.set_base(Attribute::MaxHealth, 20.0);
        m.add(Attribute::MaxHealth, Modifier { source: "a".into(), op: ModifierOp::Add, amount: 5.0 });
        m.add(Attribute::MaxHealth, Modifier { source: "b".into(), op: ModifierOp::MultiplyBase, amount: 0.1 });
        // (20 + 5) * (1 + 0.1) = 27.5
        assert!(approx(m.value(Attribute::MaxHealth), 27.5), "{}", m.value(Attribute::MaxHealth));
    }

    #[test]
    fn multiply_total_applies_sequentially() {
        let mut m = AttributeMap::new();
        m.set_base(Attribute::AttackDamage, 10.0);
        m.add(Attribute::AttackDamage, Modifier { source: "x".into(), op: ModifierOp::MultiplyTotal, amount: 0.5 });
        m.add(Attribute::AttackDamage, Modifier { source: "y".into(), op: ModifierOp::MultiplyTotal, amount: 0.5 });
        // 10 * 1.5 * 1.5 = 22.5  (NOT 10 * (1 + 1.0) = 20)
        assert!(approx(m.value(Attribute::AttackDamage), 22.5), "{}", m.value(Attribute::AttackDamage));
    }

    #[test]
    fn warrior_token_deltas_scale_with_level() {
        let mut m = AttributeMap::new();
        m.set_base(Attribute::AttackDamage, 1.0);
        m.set_base(Attribute::MaxHealth, 20.0);
        m.set_role_level(TokenRole::Warrior, 2);
        // +0.25*2 attack, -0.15*2 health
        assert!(approx(m.value(Attribute::AttackDamage), 1.5), "{}", m.value(Attribute::AttackDamage));
        assert!(approx(m.value(Attribute::MaxHealth), 19.7), "{}", m.value(Attribute::MaxHealth));
    }

    #[test]
    fn re_leveling_replaces_not_stacks() {
        let mut m = AttributeMap::new();
        m.set_base(Attribute::AttackDamage, 1.0);
        m.set_role_level(TokenRole::Warrior, 1);
        m.set_role_level(TokenRole::Warrior, 3);
        // level 3 only: 1 + 0.25*3 = 1.75 (not 1 + 0.25*1 + 0.25*3)
        assert!(approx(m.value(Attribute::AttackDamage), 1.75), "{}", m.value(Attribute::AttackDamage));
    }

    #[test]
    fn level_zero_clears_role() {
        let mut m = AttributeMap::new();
        m.set_base(Attribute::MaxHealth, 20.0);
        m.set_role_level(TokenRole::Tank, 5);
        assert!(approx(m.value(Attribute::MaxHealth), 25.0));
        m.set_role_level(TokenRole::Tank, 0);
        assert!(approx(m.value(Attribute::MaxHealth), 20.0));
    }

    #[test]
    fn roles_compose_on_shared_attribute() {
        let mut m = AttributeMap::new();
        m.set_base(Attribute::MaxHealth, 20.0);
        m.set_role_level(TokenRole::Warrior, 1); // -0.15
        m.set_role_level(TokenRole::Tank, 1); // +1.0
        // 20 - 0.15 + 1.0 = 20.85
        assert!(approx(m.value(Attribute::MaxHealth), 20.85), "{}", m.value(Attribute::MaxHealth));
    }

    #[test]
    fn remove_source_strips_modifiers() {
        let mut m = AttributeMap::new();
        m.set_base(Attribute::Armor, 0.0);
        m.add(Attribute::Armor, Modifier { source: "armor:chest".into(), op: ModifierOp::Add, amount: 8.0 });
        assert!(approx(m.value(Attribute::Armor), 8.0));
        let removed = m.remove_source("armor:chest");
        assert_eq!(removed, 1);
        assert!(approx(m.value(Attribute::Armor), 0.0));
    }
}
