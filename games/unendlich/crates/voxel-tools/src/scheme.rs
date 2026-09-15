//! scheme — one material choice per PART, not one per build.
//!
//! ## The failure this exists for
//!
//! A 3B model was asked to build a tree. It planned 1653 blocks and produced
//! a trunk and a canopy **both made of `oak_log`** — a brown lollipop. The
//! vocabulary was never the problem: `palette::BLOCKS` has had
//! `leaves` (id 18, `Family::Leaf`, uses "canopy hedge green") all along, and
//! `palette::suggest` would have found it from the word "canopy".
//!
//! The problem is that nothing ever ASKED for a second material. A build
//! picked a block once, near the start, and every solid after that inherited
//! it. There was no structure saying a tree has two parts that must look
//! different, and no check afterwards that noticed they didn't.
//!
//! So this module does three things, in the order they matter:
//!
//! 1. **Names the parts.** [`Role`] — `Trunk`, `Canopy`, `Wall`, `Roof` — so
//!    a build says "canopy" rather than "block 17". A model that is asked
//!    "what is the canopy made of?" cannot answer with the trunk's block
//!    without the mismatch being visible.
//! 2. **Requires them.** [`Archetype::required`] lists the roles an object of
//!    that kind must fill, and [`Archetype::contrast`] the pairs that must
//!    NOT resolve to the same block. A tree with `Trunk == Canopy` is
//!    rejected before a single block is placed.
//! 3. **Asks one at a time.** [`Archetype::steps`] yields a prompt per role
//!    rather than one "pick your blocks" question. A small model answers a
//!    narrow question well and a broad one badly; this is the whole reason
//!    the tree came out brown.
//!
//! ## Terrain has its own scheme
//!
//! [`Scheme::terrain`] is separate and deliberately so. Ground, cliff and
//! water are chosen once for a WORLD and then shared by everything standing
//! on it, while each object carries its own scheme. Folding them together is
//! what produces a stone house on a stone hill in front of a stone tree:
//! every object inheriting the terrain's material because that was the last
//! one chosen.

use std::collections::BTreeMap;

use crate::palette::{self, Family};

/// What a material is FOR within one object.
///
/// Deliberately coarse. A role per architectural detail would be a second
/// vocabulary for a model to get wrong; these are the distinctions that show
/// up in a silhouette at play distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Terrain surface.
    Ground,
    /// Terrain mass below the surface, and cliffs.
    Bedrock,
    Water,
    /// Load-bearing upright: a tree's trunk, a building's posts.
    Trunk,
    /// Foliage.
    Canopy,
    Wall,
    Roof,
    Floor,
    /// Windows and anything seen through.
    Glazing,
    /// Trim, ornament, the thing that keeps a build from reading as one
    /// extruded block.
    Accent,
    /// Emissive.
    Light,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Ground => "ground",
            Role::Bedrock => "bedrock",
            Role::Water => "water",
            Role::Trunk => "trunk",
            Role::Canopy => "canopy",
            Role::Wall => "wall",
            Role::Roof => "roof",
            Role::Floor => "floor",
            Role::Glazing => "glazing",
            Role::Accent => "accent",
            Role::Light => "light",
        }
    }

    /// The question to put to the model for this role, and the words
    /// `palette::suggest` should be given to shortlist candidates.
    ///
    /// Phrased as a question about the PART rather than about a block,
    /// because "what is the canopy made of" has one good answer and "pick a
    /// block" has twenty.
    pub fn question(self) -> (&'static str, &'static str) {
        match self {
            Role::Ground => ("What is the ground surface made of?", "ground lawn field floor"),
            Role::Bedrock => ("What is under the surface, and what do cliffs show?", "foundation mass stone"),
            Role::Water => ("What fills rivers and pools?", "sea river pool"),
            Role::Trunk => ("What is the trunk or the upright structure made of?", "posts beams trunks"),
            Role::Canopy => ("What is the foliage made of? It must not be the trunk's material.", "canopy hedge green leaves"),
            Role::Wall => ("What are the walls made of?", "walls structure masonry"),
            Role::Roof => ("What is the roof made of? It must contrast with the walls.", "roofs trim dark timber"),
            Role::Floor => ("What is the floor made of?", "floors warm interior"),
            Role::Glazing => ("What are the windows made of?", "windows light see-through"),
            Role::Accent => ("What is the trim or ornament made of?", "trim bright fine detail ornament"),
            Role::Light => ("What provides light?", "light glow lantern lamp"),
        }
    }

    /// Families that are sane for this role. Used to reject an answer that
    /// parsed fine but means something absurd — `Family::Liquid` walls.
    pub fn families(self) -> &'static [Family] {
        match self {
            Role::Ground => &[Family::Soil, Family::Stone],
            Role::Bedrock => &[Family::Stone, Family::Soil],
            Role::Water => &[Family::Liquid],
            Role::Trunk => &[Family::Wood, Family::Stone, Family::Metal],
            Role::Canopy => &[Family::Leaf],
            Role::Wall => &[Family::Stone, Family::Wood, Family::Soil, Family::Metal],
            Role::Roof => &[Family::Wood, Family::Stone, Family::Metal, Family::Soil],
            Role::Floor => &[Family::Wood, Family::Stone, Family::Soil],
            Role::Glazing => &[Family::Glass, Family::Crystal],
            Role::Accent => &[Family::Crystal, Family::Metal, Family::Stone, Family::Wood, Family::Light],
            Role::Light => &[Family::Light, Family::Liquid],
        }
    }
}

/// What kind of thing is being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archetype {
    /// The world's own materials, chosen once and shared.
    Terrain,
    Tree,
    Building,
    Tower,
    /// Bare rock, a boulder, a cliff feature. Legitimately one material.
    Rock,
}

impl Archetype {
    /// Roles that MUST be filled. Missing one is a rejected scheme, not a
    /// warning: the tree that started this had no canopy role at all.
    pub fn required(self) -> &'static [Role] {
        match self {
            Archetype::Terrain => &[Role::Ground, Role::Bedrock],
            Archetype::Tree => &[Role::Trunk, Role::Canopy],
            Archetype::Building => &[Role::Wall, Role::Roof, Role::Floor],
            Archetype::Tower => &[Role::Wall, Role::Roof],
            Archetype::Rock => &[Role::Bedrock],
        }
    }

    /// Roles worth filling but not fatal to omit.
    pub fn optional(self) -> &'static [Role] {
        match self {
            Archetype::Terrain => &[Role::Water],
            Archetype::Tree => &[Role::Accent],
            Archetype::Building => &[Role::Glazing, Role::Accent, Role::Light],
            Archetype::Tower => &[Role::Glazing, Role::Accent, Role::Light],
            Archetype::Rock => &[Role::Accent],
        }
    }

    /// Pairs that must not resolve to the SAME block.
    ///
    /// This is the check that would have caught the brown tree. A pair is
    /// listed only where sameness is a genuine mistake rather than a style:
    /// a stone tower with a stone roof is austere, a tree whose leaves are
    /// logs is broken.
    pub fn contrast(self) -> &'static [(Role, Role)] {
        match self {
            Archetype::Tree => &[(Role::Trunk, Role::Canopy)],
            Archetype::Building => &[(Role::Wall, Role::Roof), (Role::Wall, Role::Glazing)],
            Archetype::Tower => &[(Role::Wall, Role::Roof)],
            Archetype::Terrain => &[(Role::Ground, Role::Water)],
            Archetype::Rock => &[],
        }
    }

    /// The prompts to put to the model, in order — one per role.
    ///
    /// Required roles first, so a model that runs out of budget or patience
    /// has answered the ones that matter.
    pub fn steps(self) -> Vec<(Role, &'static str, &'static str)> {
        self.required()
            .iter()
            .chain(self.optional())
            .map(|r| {
                let (q, words) = r.question();
                (*r, q, words)
            })
            .collect()
    }

    /// A working scheme with no model involved.
    ///
    /// Not a fallback to be embarrassed about: a deterministic default means
    /// a failed or refused generation still produces something correct, and
    /// it gives the step prompts a shortlist to offer rather than asking a
    /// small model to recall block names from nothing.
    pub fn default_scheme(self) -> Scheme {
        let b = |name: &str| palette::by_name(name).map(|b| b.id).unwrap_or(1);
        let mut s = Scheme::new(match self {
            Archetype::Terrain => "terrain",
            Archetype::Tree => "tree",
            Archetype::Building => "building",
            Archetype::Tower => "tower",
            Archetype::Rock => "rock",
        });
        match self {
            Archetype::Terrain => {
                s.set(Role::Ground, b("grass"));
                s.set(Role::Bedrock, b("stone"));
                s.set(Role::Water, b("water"));
            }
            Archetype::Tree => {
                s.set(Role::Trunk, b("oak_log"));
                s.set(Role::Canopy, b("leaves"));
            }
            Archetype::Building => {
                s.set(Role::Wall, b("stone_bricks"));
                s.set(Role::Roof, b("dark_oak_planks"));
                s.set(Role::Floor, b("oak_planks"));
                s.set(Role::Glazing, b("glass"));
            }
            Archetype::Tower => {
                s.set(Role::Wall, b("stone_bricks"));
                s.set(Role::Roof, b("dark_oak_planks"));
                s.set(Role::Glazing, b("glass"));
            }
            Archetype::Rock => {
                s.set(Role::Bedrock, b("stone"));
            }
        }
        s
    }
}

/// One object's material choices.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scheme {
    pub name: String,
    roles: BTreeMap<Role, u16>,
}

/// Why a scheme was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemeFault {
    /// A required role has no block. The brown tree's actual defect.
    Missing(Role),
    /// Two roles that must differ resolved to the same block.
    NoContrast { a: Role, b: Role, block: u16 },
    /// The block is not in the catalogue at all.
    UnknownBlock { role: Role, id: u16 },
    /// Parsed, but absurd: liquid walls, log leaves.
    WrongFamily { role: Role, id: u16, got: Family },
    /// Every role is the same block. Caught separately from `NoContrast`
    /// because it names the real behaviour — one choice, reused — rather
    /// than one unlucky pair.
    Monotone { block: u16, roles: usize },
}

impl Scheme {
    pub fn new(name: impl Into<String>) -> Scheme {
        Scheme { name: name.into(), roles: BTreeMap::new() }
    }

    /// The terrain's scheme, kept apart from every object's.
    pub fn terrain() -> Scheme {
        Archetype::Terrain.default_scheme()
    }

    pub fn set(&mut self, role: Role, block: u16) -> &mut Scheme {
        self.roles.insert(role, block);
        self
    }

    pub fn get(&self, role: Role) -> Option<u16> {
        self.roles.get(&role).copied()
    }

    pub fn len(&self) -> usize {
        self.roles.len()
    }
    pub fn is_empty(&self) -> bool {
        self.roles.is_empty()
    }
    pub fn roles(&self) -> impl Iterator<Item = (Role, u16)> + '_ {
        self.roles.iter().map(|(r, b)| (*r, *b))
    }

    /// How many DISTINCT blocks this scheme uses. One is the lollipop.
    pub fn distinct_blocks(&self) -> usize {
        let mut v: Vec<u16> = self.roles.values().copied().collect();
        v.sort_unstable();
        v.dedup();
        v.len()
    }

    /// Check the scheme against what its archetype needs.
    ///
    /// Returns every fault rather than the first: a model fixing one at a
    /// time across several round trips is the slow way to get a tree, and a
    /// 3B model will not infer the second from the first.
    pub fn validate(&self, arch: Archetype) -> Vec<SchemeFault> {
        let mut out = Vec::new();
        for &r in arch.required() {
            match self.get(r) {
                None => out.push(SchemeFault::Missing(r)),
                Some(id) => match palette::by_id(id) {
                    None => out.push(SchemeFault::UnknownBlock { role: r, id }),
                    Some(b) => {
                        if !r.families().contains(&b.family) {
                            out.push(SchemeFault::WrongFamily { role: r, id, got: b.family });
                        }
                    }
                },
            }
        }
        // Optional roles are checked when present -- an optional role filled
        // with nonsense is still nonsense.
        for &r in arch.optional() {
            if let Some(id) = self.get(r) {
                match palette::by_id(id) {
                    None => out.push(SchemeFault::UnknownBlock { role: r, id }),
                    Some(b) => {
                        if !r.families().contains(&b.family) {
                            out.push(SchemeFault::WrongFamily { role: r, id, got: b.family });
                        }
                    }
                }
            }
        }
        for &(a, b) in arch.contrast() {
            if let (Some(x), Some(y)) = (self.get(a), self.get(b)) {
                if x == y {
                    out.push(SchemeFault::NoContrast { a, b, block: x });
                }
            }
        }
        // The headline symptom, reported plainly.
        if self.roles.len() > 1 && self.distinct_blocks() == 1 {
            let block = self.roles.values().next().copied().unwrap_or(0);
            out.push(SchemeFault::Monotone { block, roles: self.roles.len() });
        }
        out
    }

    /// A one-line repair instruction per fault, to feed back to the model.
    ///
    /// Says what to change and why, and names a concrete block, because
    /// "add contrast" is not actionable to a 3B model and "use `leaves`
    /// (id 18) for the canopy" is.
    pub fn explain(fault: &SchemeFault) -> String {
        match fault {
            SchemeFault::Missing(r) => {
                let (q, words) = r.question();
                let pick = palette::suggest(words, 1).first().map(|(b, _)| b.name).unwrap_or("a block");
                format!("no {} chosen. {q} Try `{pick}`.", r.name())
            }
            SchemeFault::NoContrast { a, b, block } => {
                let name = palette::by_id(*block).map(|b| b.name).unwrap_or("that block");
                let (_, words) = b.question();
                let pick = palette::suggest(words, 1).first().map(|(x, _)| x.name).unwrap_or("something else");
                format!("{} and {} are both `{name}` — they must differ. Use `{pick}` for the {}.",
                    a.name(), b.name(), b.name())
            }
            SchemeFault::UnknownBlock { role, id } => {
                format!("{} is set to block {id}, which this world does not have. Choose from the catalogue.", role.name())
            }
            SchemeFault::WrongFamily { role, id, got } => {
                let name = palette::by_id(*id).map(|b| b.name).unwrap_or("it");
                format!("{} cannot be `{name}` ({got:?}). {}", role.name(), role.question().0)
            }
            SchemeFault::Monotone { block, roles } => {
                let name = palette::by_id(*block).map(|b| b.name).unwrap_or("one block");
                format!("all {roles} parts are `{name}`. A block was chosen once and reused; \
                         answer each part separately.")
            }
        }
    }

    /// Every fault as instructions, newline separated. Empty when valid.
    pub fn repair_prompt(&self, arch: Archetype) -> String {
        self.validate(arch)
            .iter()
            .map(Self::explain)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(name: &str) -> u16 {
        palette::by_name(name).expect("catalogue block").id
    }

    /// THE SCREENSHOT. A tree whose trunk and canopy are both oak_log — 1653
    /// blocks of brown lollipop. Must be rejected, and the message must name
    /// the fix.
    #[test]
    fn the_brown_lollipop_tree_is_rejected() {
        let mut s = Scheme::new("tree");
        s.set(Role::Trunk, id("oak_log"));
        s.set(Role::Canopy, id("oak_log"));
        let faults = s.validate(Archetype::Tree);

        assert!(faults.iter().any(|f| matches!(f, SchemeFault::NoContrast { .. })),
            "trunk == canopy must fail contrast: {faults:?}");
        assert!(faults.iter().any(|f| matches!(f, SchemeFault::WrongFamily { role: Role::Canopy, .. })),
            "a canopy of wood is the wrong family: {faults:?}");
        assert!(faults.iter().any(|f| matches!(f, SchemeFault::Monotone { .. })),
            "and the real behaviour -- one block reused -- is named: {faults:?}");

        let msg = s.repair_prompt(Archetype::Tree);
        assert!(msg.contains("leaves"), "must name a concrete block: {msg}");
        assert!(msg.contains("canopy"), "{msg}");
    }

    /// A tree that never mentioned a canopy at all — the more likely
    /// original failure, since the model simply never considered foliage.
    #[test]
    fn a_tree_with_no_canopy_role_is_rejected() {
        let mut s = Scheme::new("tree");
        s.set(Role::Trunk, id("oak_log"));
        let faults = s.validate(Archetype::Tree);
        assert!(faults.contains(&SchemeFault::Missing(Role::Canopy)), "{faults:?}");
        assert!(s.repair_prompt(Archetype::Tree).contains("leaves"));
    }

    /// The defaults must themselves be valid, or the fallback ships the bug.
    #[test]
    fn every_default_scheme_is_valid() {
        for a in [Archetype::Terrain, Archetype::Tree, Archetype::Building,
                  Archetype::Tower, Archetype::Rock] {
            let s = a.default_scheme();
            assert_eq!(s.validate(a), vec![], "{:?} default is not valid", a);
        }
    }

    /// The default tree is the picture that should have been produced.
    #[test]
    fn the_default_tree_has_leaves_on_a_log() {
        let s = Archetype::Tree.default_scheme();
        assert_eq!(s.get(Role::Trunk), Some(id("oak_log")));
        assert_eq!(s.get(Role::Canopy), Some(id("leaves")));
        assert_eq!(s.distinct_blocks(), 2);
    }

    /// Terrain is chosen once for a world and kept apart from objects, so a
    /// stone hill cannot make every tree on it stone.
    #[test]
    fn terrain_is_its_own_scheme_and_does_not_supply_object_roles() {
        let t = Scheme::terrain();
        assert_eq!(t.validate(Archetype::Terrain), vec![]);
        assert_eq!(t.get(Role::Canopy), None, "terrain has no opinion about foliage");
        assert_eq!(t.get(Role::Trunk), None);
        // And a tree's scheme says nothing about the ground.
        assert_eq!(Archetype::Tree.default_scheme().get(Role::Ground), None);
    }

    /// One block across several roles is the behaviour to catch, whatever the
    /// archetype.
    #[test]
    fn monotone_is_reported_for_any_multi_role_scheme() {
        let mut s = Scheme::new("house");
        for r in [Role::Wall, Role::Roof, Role::Floor] {
            s.set(r, id("stone"));
        }
        let faults = s.validate(Archetype::Building);
        assert!(faults.iter().any(|f| matches!(f, SchemeFault::Monotone { roles: 3, .. })), "{faults:?}");
        // A single-role scheme is NOT monotone -- a boulder is allowed to be
        // one material.
        let rock = Archetype::Rock.default_scheme();
        assert!(!rock.validate(Archetype::Rock).iter().any(|f| matches!(f, SchemeFault::Monotone { .. })));
    }

    /// An answer that parses but is absurd must be caught.
    #[test]
    fn nonsense_families_are_refused() {
        let mut s = Archetype::Building.default_scheme();
        s.set(Role::Wall, id("water"));
        assert!(s.validate(Archetype::Building).iter()
            .any(|f| matches!(f, SchemeFault::WrongFamily { role: Role::Wall, .. })));

        let mut s = Archetype::Tree.default_scheme();
        s.set(Role::Canopy, id("obsidian"));
        assert!(s.validate(Archetype::Tree).iter()
            .any(|f| matches!(f, SchemeFault::WrongFamily { role: Role::Canopy, .. })));
    }

    #[test]
    fn a_block_outside_the_catalogue_is_refused() {
        let mut s = Archetype::Tree.default_scheme();
        s.set(Role::Trunk, 60_000);
        assert!(s.validate(Archetype::Tree).iter()
            .any(|f| matches!(f, SchemeFault::UnknownBlock { id: 60_000, .. })));
    }

    /// Step prompts: required roles first, one question each. The fix for a
    /// small model is a narrow question, not a bigger one.
    #[test]
    fn steps_ask_one_role_at_a_time_required_first() {
        let steps = Archetype::Tree.steps();
        assert_eq!(steps[0].0, Role::Trunk);
        assert_eq!(steps[1].0, Role::Canopy);
        assert!(steps.len() > 2, "optional roles follow");
        // Every step carries a real question and words suggest() can use.
        for (role, q, words) in &steps {
            assert!(q.ends_with('?') || q.contains('?'), "{role:?}: {q}");
            assert!(!palette::suggest(words, 1).is_empty(), "{role:?}: no candidate for '{words}'");
        }
    }

    /// The canopy question must itself carry the constraint, so a model that
    /// reads nothing else still knows not to reuse the trunk.
    #[test]
    fn the_canopy_question_states_the_contrast_rule() {
        let (q, _) = Role::Canopy.question();
        assert!(q.contains("not be the trunk"), "{q}");
        let (q, _) = Role::Roof.question();
        assert!(q.contains("contrast"), "{q}");
    }

    /// A valid scheme produces no repair prompt — a model must not be sent
    /// round again for nothing.
    #[test]
    fn a_valid_scheme_has_an_empty_repair_prompt() {
        assert_eq!(Archetype::Tree.default_scheme().repair_prompt(Archetype::Tree), "");
    }
}
