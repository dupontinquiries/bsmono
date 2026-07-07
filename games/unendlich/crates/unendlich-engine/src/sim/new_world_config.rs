//! `sim::new_world_config` — "New Game" screen config → validated `WorldSpec`.
//!
//! Pure model behind the New Game screen. Resolves a seed choice into a concrete
//! u64, validates the config, and produces the `WorldSpec` the bin hands to
//! worldgen. No std RNG, no IO, no panics.
//!
//! ## Note on `GameMode` source (coordinator)
//! The brief referenced `world_catalog::GameMode` and `world_catalog::valid_world_name`,
//! but there is no `world_catalog` module in the tree. The canonical `GameMode`
//! (4 variants) lives in `sim::command_parse`, so this module reuses THAT — no new
//! duplicate enum. There is no existing `valid_world_name` helper either, so the
//! name rule is defined locally here (documented below) instead of importing a
//! nonexistent symbol. If a `world_catalog` is added later, swap the import and
//! delegate `validate`'s name check to it.

use crate::sim::command_parse::GameMode;

// ---------------------------------------------------------------------------
// Name validation rule (local; documented)
// ---------------------------------------------------------------------------

/// Minimum world-name length (after trimming surrounding whitespace).
const NAME_MIN: usize = 1;
/// Maximum world-name length.
const NAME_MAX: usize = 48;

/// Validate a world name. Rules:
/// - 1..=48 chars after trimming leading/trailing whitespace,
/// - no control characters,
/// - no path separators or quotes (`/ \ : * ? " < > |`), so it's filesystem-safe.
///
/// Returns `Ok(())` or a static error message. Pure.
pub fn valid_world_name(name: &str) -> Result<(), &'static str> {
    let trimmed = name.trim();
    let len = trimmed.chars().count();
    if len < NAME_MIN {
        return Err("world name is empty");
    }
    if len > NAME_MAX {
        return Err("world name too long (max 48 characters)");
    }
    for ch in trimmed.chars() {
        if ch.is_control() {
            return Err("world name contains a control character");
        }
        if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            return Err("world name contains an illegal character");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// FNV-1a 64-bit (deterministic text → seed; NOT std RNG)
// ---------------------------------------------------------------------------

const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
const FNV_PRIME:  u64 = 0x0000_0100_0000_01B3;

/// Deterministic FNV-1a hash of a string to a u64 seed.
/// Same text → same seed, on every platform. Pure.
fn fnv1a_64(s: &str) -> u64 {
    let mut h = FNV_OFFSET;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// How the world seed is chosen on the New Game screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SeedChoice {
    /// Use caller-supplied entropy (e.g. a frame/time value from the bin).
    Random,
    /// Use this exact seed.
    Fixed(u64),
    /// Hash this text to a deterministic u64 seed (FNV-1a).
    Text(String),
}

/// The raw config the New Game screen collects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewWorldConfig {
    pub name: String,
    pub seed: SeedChoice,
    pub mode: GameMode,
}

/// The validated, resolved spec the bin hands to worldgen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldSpec {
    pub name: String,
    pub seed: u64,
    pub mode: GameMode,
}

// ---------------------------------------------------------------------------
// Core functions
// ---------------------------------------------------------------------------

/// Resolve a [`SeedChoice`] into a concrete u64.
///
/// - `Random` returns the caller-supplied `entropy` (the bin passes a frame/time
///   value; keeping entropy a parameter makes this pure — entropy in, seed out).
/// - `Fixed(s)` passes `s` through unchanged.
/// - `Text(t)` returns the deterministic FNV-1a hash of `t`.
pub fn resolve_seed(choice: &SeedChoice, entropy: u64) -> u64 {
    match choice {
        SeedChoice::Random      => entropy,
        SeedChoice::Fixed(s)    => *s,
        SeedChoice::Text(t)     => fnv1a_64(t),
    }
}

/// Validate a [`NewWorldConfig`]. Currently checks the world-name rule; extend
/// here as more constraints are added (the caller is responsible for the
/// duplicate-name catalog check, which needs external state).
///
/// Returns `Ok(())` or a static error message. Pure.
pub fn validate(cfg: &NewWorldConfig) -> Result<(), &'static str> {
    valid_world_name(&cfg.name)?;
    Ok(())
}

/// Validate `cfg` and resolve it into a [`WorldSpec`].
///
/// On success the spec carries the trimmed name, the resolved seed, and the mode.
/// On failure returns the validation error message. Pure; never panics.
pub fn to_spec(cfg: &NewWorldConfig, entropy: u64) -> Result<WorldSpec, &'static str> {
    validate(cfg)?;
    Ok(WorldSpec {
        name: cfg.name.trim().to_string(),
        seed: resolve_seed(&cfg.seed, entropy),
        mode: cfg.mode,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str, seed: SeedChoice) -> NewWorldConfig {
        NewWorldConfig { name: name.to_string(), seed, mode: GameMode::Survival }
    }

    // ---- seed resolution ---------------------------------------------------

    #[test]
    fn text_seed_deterministic() {
        let a = resolve_seed(&SeedChoice::Text("hello world".into()), 0);
        let b = resolve_seed(&SeedChoice::Text("hello world".into()), 999);
        assert_eq!(a, b, "same text → same seed, regardless of entropy");
    }

    #[test]
    fn text_seed_differs_by_text() {
        let a = resolve_seed(&SeedChoice::Text("alpha".into()), 0);
        let b = resolve_seed(&SeedChoice::Text("beta".into()), 0);
        assert_ne!(a, b, "different text should (almost always) give different seed");
    }

    #[test]
    fn fixed_seed_passes_through() {
        assert_eq!(resolve_seed(&SeedChoice::Fixed(0xDEAD_BEEF), 123), 0xDEAD_BEEF);
    }

    #[test]
    fn random_uses_entropy() {
        assert_eq!(resolve_seed(&SeedChoice::Random, 0xABCD), 0xABCD);
        assert_eq!(resolve_seed(&SeedChoice::Random, 42), 42);
    }

    #[test]
    fn fnv_known_empty_string() {
        // FNV-1a of empty string is the offset basis.
        assert_eq!(fnv1a_64(""), FNV_OFFSET);
    }

    // ---- name validation ---------------------------------------------------

    #[test]
    fn valid_name_accepted() {
        assert!(valid_world_name("My World").is_ok());
        assert!(valid_world_name("a").is_ok());
    }

    #[test]
    fn empty_name_rejected() {
        assert!(valid_world_name("").is_err());
        assert!(valid_world_name("   ").is_err(), "whitespace-only trims to empty");
    }

    #[test]
    fn too_long_name_rejected() {
        let long = "x".repeat(49);
        assert!(valid_world_name(&long).is_err());
        let ok = "x".repeat(48);
        assert!(valid_world_name(&ok).is_ok(), "exactly 48 is allowed");
    }

    #[test]
    fn illegal_characters_rejected() {
        for bad in ["a/b", "c:d", "e\\f", "g*h", "i?j", "k\"l", "m<n", "o>p", "q|r"] {
            assert!(valid_world_name(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn control_char_rejected() {
        assert!(valid_world_name("bad\u{0007}name").is_err());
    }

    // ---- to_spec / validate ------------------------------------------------

    #[test]
    fn to_spec_round_trips_valid_config() {
        let c = cfg("Test World", SeedChoice::Fixed(7));
        let spec = to_spec(&c, 0).expect("valid config should produce a spec");
        assert_eq!(spec.name, "Test World");
        assert_eq!(spec.seed, 7);
        assert_eq!(spec.mode, GameMode::Survival);
    }

    #[test]
    fn to_spec_trims_name() {
        let c = cfg("  Spacey  ", SeedChoice::Fixed(1));
        let spec = to_spec(&c, 0).unwrap();
        assert_eq!(spec.name, "Spacey", "name should be trimmed in the spec");
    }

    #[test]
    fn to_spec_rejects_invalid_name() {
        let c = cfg("", SeedChoice::Fixed(1));
        assert!(to_spec(&c, 0).is_err(), "empty name should be rejected");
    }

    #[test]
    fn to_spec_text_seed_resolved() {
        let c = cfg("World", SeedChoice::Text("my seed".into()));
        let spec = to_spec(&c, 12345).unwrap();
        assert_eq!(spec.seed, fnv1a_64("my seed"), "text seed should be the FNV hash");
    }

    #[test]
    fn to_spec_random_seed_uses_entropy() {
        let c = cfg("World", SeedChoice::Random);
        let spec = to_spec(&c, 0x1234_5678).unwrap();
        assert_eq!(spec.seed, 0x1234_5678, "random seed should use entropy");
    }

    #[test]
    fn validate_is_deterministic() {
        let c = cfg("Stable", SeedChoice::Fixed(0));
        assert_eq!(validate(&c), validate(&c));
    }

    #[test]
    fn all_game_modes_pass_through() {
        for mode in [GameMode::Survival, GameMode::Creative, GameMode::Adventure, GameMode::Spectator] {
            let c = NewWorldConfig { name: "W".into(), seed: SeedChoice::Fixed(0), mode };
            let spec = to_spec(&c, 0).unwrap();
            assert_eq!(spec.mode, mode);
        }
    }
}
