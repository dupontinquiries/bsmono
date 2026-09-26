// Ancestor roster — the thirteen legendary ancestors of Clawbound
// (monorepo/games/clawbound/src/ancestor.rs), the First Warriors of the
// great cat clans of legend. 5 roles, 4 abilities each (52 total).
//
// Each Clawbound kit is a passive instinct, two learned techniques and an
// ultimate. This arena needs four *active* slots, so the mapping is:
//   slot 0/1  = the two techniques      (verb → archetype, see table below)
//   slot 2    = the passive, reforged as a short-cooldown technique
//   slot 3    = the ultimate
// Verb → archetype: Strike→Bolt, Aoe/Knockback/Fear→Nova, Dash→Dash,
// Shield→Shield, Stealth→Blink, Heal/Rain→Heal, Slow/Root→Ring, Pull→Pull,
// Execute→Bolt (heavy), Zone→Storm.
//
// The `GodDef` / `ROSTER` identifiers are kept as code-level names; the
// player-facing vocabulary is "ancestor".

use crate::consts::VOXEL_SIZE;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[derive(Hash)]
pub enum Role {
    Warrior,
    Guardian,
    Mage,
    Hunter,
    Assassin,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Warrior => "Warrior",
            Role::Guardian => "Guardian",
            Role::Hunter => "Hunter",
            Role::Mage => "Mage",
            Role::Assassin => "Assassin",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AutoKind {
    Melee,
    Ranged,
}

/// Damage / mitigation type
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DmgType {
    Physical,
    Magical,
}

// --- ability archetypes -----------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub enum Archetype {
    /// Fast skillshot projectile in aim direction.
    Bolt { speed: f32, dmg: f32, radius: f32 },
    /// Fan of N projectiles.
    Volley { count: u32, spread_deg: f32, dmg: f32, speed: f32 },
    /// Arcing lob that detonates in an AoE at the target point.
    Lob { radius: f32, dmg: f32 },
    /// Instant thick beam from self toward aim point.
    Beam { length: f32, width: f32, dmg: f32 },
    /// Radial burst around self (damage + slow).
    Nova { radius: f32, dmg: f32, slow: f32 },
    /// Delayed expanding ground ring at target point.
    Ring { radius: f32, delay: f32, dmg: f32 },
    /// Persistent storm zone: lightning strikes + slow.
    Storm { radius: f32, duration: f32, dps: f32 },
    /// Dash in move direction, damaging enemies passed through.
    Dash { dist: f32, dmg: f32 },
    /// Teleport toward aim point (clamped).
    Blink { dist: f32, dmg: f32 },
    /// Absorb shield on self (+ nearby ally with lowest HP).
    Shield { amount: f32, duration: f32 },
    /// Self buff: attack speed + movement speed.
    Buff { atk_speed: f32, move_speed: f32, duration: f32 },
    /// Heal self (and nearby allies for a fraction).
    Heal { amount: f32, radius: f32 },
    /// Drag enemies in radius toward the center point, damaging them.
    Pull { radius: f32, dmg: f32 },
    /// Vault into the sky and come down as a falling star at the aim
    /// (within `range`), `vault` seconds later: a blast of `radius`.
    Meteor { range: f32, radius: f32, dmg: f32, vault: f32 },
}

#[derive(Clone, Copy, Debug)]
pub struct AbilityDef {
    pub name: &'static str,
    pub desc: &'static str,
    pub arch: Archetype,
    pub cooldown: f32,
    pub cost: f32,
    /// VFX color (linear-ish RGB)
    pub color: [f32; 3],
    /// 0..=3 slot
    pub key: u32,
    /// Ultimate flag (slot 4 renders with golden frame)
    pub ult: bool,
    pub power_scale: f32, // fraction of god's power stat added to dmg/heal
}

impl AbilityDef {
    /// Mobility abilities (dash/blink) are the only shape where "a
    /// direction" is the whole aim — never a substitute for precise casts,
    /// and never an ultimate (see touch.rs's drag-to-aim, which uses this).
    pub fn quick_aimable(&self) -> bool {
        !self.ult && matches!(self.arch, Archetype::Dash { .. } | Archetype::Blink { .. })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GodDef {
    pub id: u32,
    pub name: &'static str,
    /// the clan of legend they founded
    pub title: &'static str,
    /// the teaching that survives them
    pub philosophy: &'static str,
    pub role: Role,
    pub auto: AutoKind,
    pub dmg_type: DmgType,
    /// pelt / team-collar / secondary-pelt (belly, muzzle, mane) / accent (eyes, crest).
    pub palette: [[f32; 3]; 4],
    pub weapon: WeaponKind,
    pub pelt: PeltKind,
    // base stats + per-level growth
    pub hp: f32,
    pub hp_growth: f32,
    pub mana: f32,
    pub mana_growth: f32,
    pub auto_dmg: f32,
    pub auto_growth: f32,
    pub power: f32,
    pub power_growth: f32,
    pub protection: f32,
    pub prot_growth: f32,
    pub move_speed: f32,
    pub auto_range: f32,
    pub auto_cd: f32,
    pub abilities: [AbilityDef; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WeaponKind {
    /// Ancestors fight with claw and fang; no held weapon mesh.
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
/// Pelt / crest signature — the "who" axis of the model, independent of team
/// colour (which is the "which side" axis, painted onto the collar + tail ring).
pub enum PeltKind {
    Mane,      // Goldenmane — the full lion's mane
    Stripes,   // Duskstripe — dark flank stripes
    Rosettes,  // Rainspots / Riptail — dappled rosettes
    Shadow,    // Nightpelt — near-black, eyes only
    EarTufts,  // Windclaw / Snowhush — tall tufted ears
    BigEars,   // Faryowl — the serval's enormous ears
    Hulk,      // Oldroar — heavy shoulders, ragged mane
    Sabers,    // Twofang — the saber fangs
    SunDisc,   // Suneye — a floating sun disc above the brow
    Antlers,   // Stillsmoke — antlered, scaled, saber-fanged
    Crag,      // Stonewhisper — thick grey pelt, no markings
    Comet,     // Cometail — night-blue, starred back, a tail that streams light
}

impl GodDef {
    pub fn at_level(&self, level: u32) -> (f32, f32, f32, f32, f32) {
        let l = (level - 1) as f32;
        (
            self.hp + self.hp_growth * l,
            self.mana + self.mana_growth * l,
            self.auto_dmg + self.auto_growth * l,
            self.power + self.power_growth * l,
            self.protection + self.prot_growth * l,
        )
    }
}

const fn ab(
    name: &'static str,
    desc: &'static str,
    arch: Archetype,
    cd: f32,
    cost: f32,
    color: [f32; 3],
    key: u32,
    ult: bool,
    scale: f32,
) -> AbilityDef {
    AbilityDef { name, desc, arch, cooldown: cd, cost, color, key, ult, power_scale: scale }
}

pub const ROSTER_LEN: usize = 14;

pub const ROSTER: [GodDef; ROSTER_LEN] = [
    // ── GOLDENMANE — the lion-clan of legend ──
    GodDef {
        id: 0, name: "Goldenmane", title: "the lion-clan of legend",
        philosophy: "A warrior alone is a whisker; a pride is a storm.",
        role: Role::Warrior, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.82, 0.62, 0.30], [0.55, 0.32, 0.14], [0.93, 0.82, 0.58], [0.91, 0.78, 0.35]],
        weapon: WeaponKind::None, pelt: PeltKind::Mane,
        hp: 659.0, hp_growth: 105.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 51.0, auto_growth: 3.8, power: 8.0, power_growth: 2.4,
        protection: 24.0, prot_growth: 2.6, move_speed: 12.4, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Sunstrike Pounce", "Leap the gap and rake on landing, trampling everything in the bound.", Archetype::Dash { dist: 13.0, dmg: 100.0 }, 8.0, 65.0, [1.00, 0.85, 0.40], 0, false, 0.50),
            ab("Mane of Dawn", "A golden guard-hair shield over Goldenmane and the nearest packmate.", Archetype::Shield { amount: 230.0, duration: 5.0 }, 14.0, 80.0, [1.00, 0.90, 0.50], 1, false, 0.35),
            ab("Pride's Warmth", "The pride strikes as one: attack and move speed surge.", Archetype::Buff { atk_speed: 0.40, move_speed: 0.20, duration: 6.0 }, 15.0, 60.0, [1.00, 0.80, 0.35], 2, false, 0.00),
            ab("The Great Roar", "A roar out of the elder days — everything in earshot is battered and turns tail.", Archetype::Nova { radius: 11.0, dmg: 230.0, slow: 0.55 }, 78.0, 120.0, [1.00, 0.78, 0.30], 3, true, 0.70),
        ],
    },
    // ── DUSKSTRIPE — the tiger-clan of legend ──
    GodDef {
        id: 1, name: "Duskstripe", title: "the tiger-clan of legend",
        philosophy: "Strike once, from where they never looked.",
        role: Role::Assassin, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.85, 0.45, 0.16], [0.22, 0.12, 0.06], [0.95, 0.90, 0.80], [0.91, 0.47, 0.16]],
        weapon: WeaponKind::None, pelt: PeltKind::Stripes,
        hp: 617.0, hp_growth: 99.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 60.0, auto_growth: 4.5, power: 8.0, power_growth: 2.4,
        protection: 19.0, prot_growth: 2.1, move_speed: 12.6, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Shadow Maul", "A heavy double-paw blow hurled as a crushing bolt.", Archetype::Bolt { speed: 30.0, dmg: 125.0, radius: 0.9 }, 7.0, 70.0, [1.00, 0.55, 0.20], 0, false, 0.55),
            ab("Stripes in the Reeds", "Melt into the undergrowth and reappear behind the prey.", Archetype::Blink { dist: 13.0, dmg: 85.0 }, 12.0, 70.0, [0.50, 0.40, 0.25], 1, false, 0.40),
            ab("Night's Hunger", "The first blow bites far deeper: attack speed surges.", Archetype::Buff { atk_speed: 0.5, move_speed: 0.3, duration: 5.0 }, 14.0, 60.0, [0.90, 0.50, 0.20], 2, false, 0.00),
            ab("Long Shadows", "Dusk falls early — nearby enemies grope half-blind and slow, struck from the dark.", Archetype::Nova { radius: 8.0, dmg: 255.0, slow: 0.5 }, 78.0, 120.0, [0.45, 0.30, 0.40], 3, true, 0.70),
        ],
    },
    // ── RAINSPOTS — the leopard-clan of legend ──
    GodDef {
        id: 2, name: "Rainspots", title: "the leopard-clan of legend",
        philosophy: "The river never argues; it goes around, and wins.",
        role: Role::Warrior, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.83, 0.69, 0.42], [0.30, 0.20, 0.10], [0.95, 0.90, 0.75], [0.83, 0.69, 0.42]],
        weapon: WeaponKind::None, pelt: PeltKind::Rosettes,
        hp: 596.0, hp_growth: 95.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 53.0, auto_growth: 4.0, power: 8.0, power_growth: 2.4,
        protection: 22.0, prot_growth: 2.4, move_speed: 13.2, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Spotted Blur", "Flicker through the enemy, claws trailing.", Archetype::Dash { dist: 15.0, dmg: 85.0 }, 6.0, 55.0, [0.85, 0.75, 0.45], 0, false, 0.40),
            ab("Fisher's Mercy", "Share the day's catch — the taste knits flesh for the pride.", Archetype::Heal { amount: 240.0, radius: 7.0 }, 15.0, 80.0, [0.50, 0.90, 0.70], 1, false, 0.35),
            ab("Riverborn", "Water is home ground — a surge of river speed and quick claws.", Archetype::Buff { atk_speed: 0.3, move_speed: 0.35, duration: 5.0 }, 13.0, 55.0, [0.40, 0.80, 0.90], 2, false, 0.00),
            ab("Floodrush", "The river answers — a drowning surge batters everything around.", Archetype::Nova { radius: 10.0, dmg: 245.0, slow: 0.45 }, 76.0, 120.0, [0.35, 0.75, 0.95], 3, true, 0.70),
        ],
    },
    // ── NIGHTPELT — the shadow-walkers of the moonless woods ──
    GodDef {
        id: 3, name: "Nightpelt", title: "the shadow-walkers of the moonless woods",
        philosophy: "Never chase. Become where the prey chooses to run.",
        role: Role::Assassin, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.12, 0.11, 0.15], [0.25, 0.18, 0.35], [0.18, 0.17, 0.22], [0.16, 0.16, 0.22]],
        weapon: WeaponKind::None, pelt: PeltKind::Shadow,
        hp: 575.0, hp_growth: 92.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 57.0, auto_growth: 4.3, power: 8.0, power_growth: 2.4,
        protection: 18.0, prot_growth: 2.0, move_speed: 13.0, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Pounce of Midnight", "Drop onto prey and pin it, claws first.", Archetype::Dash { dist: 12.0, dmg: 115.0 }, 9.0, 65.0, [0.45, 0.35, 0.60], 0, false, 0.50),
            ab("Veilwalker", "The dark closes over you like water — vanish and reappear.", Archetype::Blink { dist: 14.0, dmg: 0.0 }, 13.0, 70.0, [0.30, 0.25, 0.45], 1, false, 0.00),
            ab("Silent Footfall", "No scent, no sound — the paws quicken.", Archetype::Buff { atk_speed: 0.35, move_speed: 0.4, duration: 5.0 }, 14.0, 55.0, [0.50, 0.45, 0.70], 2, false, 0.00),
            ab("Eclipse Hunt", "The moon goes out — everything near is struck from the dark.", Archetype::Nova { radius: 9.5, dmg: 265.0, slow: 0.4 }, 80.0, 125.0, [0.55, 0.45, 0.80], 3, true, 0.70),
        ],
    },
    // ── WINDCLAW — the wind-eared hunters of the dry plains ──
    GodDef {
        id: 4, name: "Windclaw", title: "the wind-eared hunters of the dry plains",
        philosophy: "The sky itself can be hunted.",
        role: Role::Assassin, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.80, 0.55, 0.32], [0.42, 0.30, 0.18], [0.95, 0.85, 0.70], [0.79, 0.54, 0.32]],
        weapon: WeaponKind::None, pelt: PeltKind::EarTufts,
        hp: 565.0, hp_growth: 90.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 48.0, auto_growth: 3.6, power: 8.0, power_growth: 2.4,
        protection: 17.0, prot_growth: 1.9, move_speed: 13.8, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Sky Snap", "A vertical snatch that plucks the quick out of the air.", Archetype::Bolt { speed: 36.0, dmg: 105.0, radius: 0.8 }, 6.5, 60.0, [0.95, 0.85, 0.55], 0, false, 0.50),
            ab("Sand Dash", "A dust-raising burst clean through the line.", Archetype::Dash { dist: 17.0, dmg: 60.0 }, 5.5, 50.0, [0.90, 0.75, 0.45], 1, false, 0.30),
            ab("Skyward Hunger", "Every kill quickens the paws — speed stacks with the tally.", Archetype::Buff { atk_speed: 0.45, move_speed: 0.45, duration: 5.0 }, 13.0, 55.0, [1.00, 0.90, 0.60], 2, false, 0.00),
            ab("Hundred Leaps", "A blur of bounds — a storm of cuffing paws leaves everything staggering.", Archetype::Storm { radius: 8.0, duration: 5.0, dps: 66.0 }, 74.0, 120.0, [0.95, 0.80, 0.45], 3, true, 0.40),
        ],
    },
    // ── FARYOWL — the far-yowlers of the singing grasslands ──
    GodDef {
        id: 5, name: "Faryowl", title: "the far-yowlers of the singing grasslands",
        philosophy: "Why cross the clearing when your voice already has?",
        role: Role::Hunter, auto: AutoKind::Ranged, dmg_type: DmgType::Physical,
        palette: [[0.88, 0.75, 0.42], [0.28, 0.20, 0.10], [0.98, 0.95, 0.80], [0.85, 0.75, 0.25]],
        weapon: WeaponKind::None, pelt: PeltKind::BigEars,
        hp: 533.0, hp_growth: 85.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 38.0, auto_growth: 2.9, power: 8.0, power_growth: 2.4,
        protection: 17.0, prot_growth: 1.9, move_speed: 12.8, auto_range: 13.2, auto_cd: 1.05,
        abilities: [
            ab("Piercing Yowl", "One note, aimed like a claw.", Archetype::Bolt { speed: 40.0, dmg: 100.0, radius: 0.7 }, 6.5, 60.0, [1.00, 0.95, 0.55], 0, false, 0.50),
            ab("Wall of Sound", "A caterwaul that flings the close-pressed back.", Archetype::Nova { radius: 7.0, dmg: 95.0, slow: 0.4 }, 11.0, 70.0, [0.95, 0.90, 0.50], 1, false, 0.45),
            ab("The Far Song", "A yowl bites deepest from a distance — a fan of five notes.", Archetype::Volley { count: 5, spread_deg: 24.0, dmg: 55.0, speed: 40.0 }, 8.0, 65.0, [1.00, 0.92, 0.60], 2, false, 0.40),
            ab("The Great Caterwaul", "The song the elders warn of — everything in earshot breaks and runs bleeding.", Archetype::Beam { length: 32.0, width: 2.2, dmg: 290.0 }, 75.0, 120.0, [1.00, 0.95, 0.65], 3, true, 0.60),
        ],
    },
    // ── OLDROAR — the cave-lions of the frozen long-ago ──
    GodDef {
        id: 6, name: "Oldroar", title: "the cave-lions of the frozen long-ago",
        philosophy: "The old world did not fall. It is only sleeping in me.",
        role: Role::Guardian, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.62, 0.48, 0.32], [0.40, 0.28, 0.16], [0.85, 0.75, 0.60], [0.69, 0.50, 0.28]],
        weapon: WeaponKind::None, pelt: PeltKind::Hulk,
        hp: 764.0, hp_growth: 122.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 48.0, auto_growth: 3.6, power: 8.0, power_growth: 2.4,
        protection: 32.0, prot_growth: 3.5, move_speed: 11.8, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Boulder Paw", "A paw like falling stone — the struck are hurled away.", Archetype::Nova { radius: 6.5, dmg: 105.0, slow: 0.35 }, 9.0, 70.0, [0.80, 0.65, 0.45], 0, false, 0.45),
            ab("Shatter Ground", "Stamp the earth until it cracks and grips.", Archetype::Ring { radius: 7.5, delay: 0.8, dmg: 120.0 }, 12.0, 75.0, [0.75, 0.60, 0.40], 1, false, 0.50),
            ab("Deep Wounds, Deep Fury", "Every wound feeds the fury — swell with the old strength.", Archetype::Heal { amount: 260.0, radius: 3.0 }, 16.0, 70.0, [0.90, 0.50, 0.30], 2, false, 0.30),
            ab("Earthshaker", "The roar and the blow together, as the old world made them.", Archetype::Nova { radius: 10.5, dmg: 270.0, slow: 0.5 }, 80.0, 125.0, [0.85, 0.60, 0.35], 3, true, 0.70),
        ],
    },
    // ── TWOFANG — the two-fangs of the elder dawn ──
    GodDef {
        id: 7, name: "Twofang", title: "the two-fangs of the elder dawn",
        philosophy: "Claws begin the hunt. The fangs only ever end it.",
        role: Role::Warrior, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.60, 0.55, 0.50], [0.25, 0.22, 0.24], [0.85, 0.82, 0.78], [0.54, 0.54, 0.57]],
        weapon: WeaponKind::None, pelt: PeltKind::Sabers,
        hp: 627.0, hp_growth: 100.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 57.0, auto_growth: 4.3, power: 8.0, power_growth: 2.4,
        protection: 23.0, prot_growth: 2.5, move_speed: 12.4, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Saber Lunge", "The whole weight behind both fangs.", Archetype::Dash { dist: 12.0, dmg: 110.0 }, 8.5, 65.0, [0.85, 0.85, 0.90], 0, false, 0.50),
            ab("Hamstring", "One rake low, and the running is over.", Archetype::Ring { radius: 5.0, delay: 0.4, dmg: 95.0 }, 9.0, 60.0, [0.80, 0.30, 0.30], 1, false, 0.40),
            ab("The Saber Finishes", "A wounded foe is a finished foe — a thrown killing thrust.", Archetype::Bolt { speed: 32.0, dmg: 130.0, radius: 0.8 }, 9.0, 70.0, [0.90, 0.90, 0.95], 2, false, 0.55),
            ab("The Killing Bite", "The bite the elder dawn is remembered by.", Archetype::Bolt { speed: 34.0, dmg: 380.0, radius: 1.1 }, 70.0, 110.0, [1.00, 0.35, 0.30], 3, true, 0.90),
        ],
    },
    // ── SUNEYE — the sun-priests of the endless dunes ──
    GodDef {
        id: 8, name: "Suneye", title: "the sun-priests of the endless dunes",
        philosophy: "The night hides many things. The sun keeps none of them.",
        role: Role::Mage, auto: AutoKind::Ranged, dmg_type: DmgType::Magical,
        palette: [[0.90, 0.70, 0.40], [0.50, 0.30, 0.12], [0.98, 0.90, 0.70], [0.95, 0.66, 0.24]],
        weapon: WeaponKind::None, pelt: PeltKind::SunDisc,
        hp: 523.0, hp_growth: 84.0, mana: 380.0, mana_growth: 44.0,
        auto_dmg: 38.0, auto_growth: 2.9, power: 12.0, power_growth: 3.2,
        protection: 16.0, prot_growth: 1.8, move_speed: 12.6, auto_range: 14.4, auto_cd: 1.05,
        abilities: [
            ab("Sunspear", "A lance of noon light, thrown from the eye.", Archetype::Bolt { speed: 36.0, dmg: 95.0, radius: 0.7 }, 6.0, 60.0, [1.00, 0.85, 0.40], 0, false, 0.55),
            ab("Noon Blaze", "Scorch the ground — nothing hides on burning sand.", Archetype::Storm { radius: 6.5, duration: 4.0, dps: 50.0 }, 14.0, 80.0, [1.00, 0.60, 0.25], 1, false, 0.35),
            ab("The Sun's Favor", "Under the open sun every strike burns brighter — a searing beam.", Archetype::Beam { length: 22.0, width: 1.6, dmg: 145.0 }, 10.0, 85.0, [1.00, 0.75, 0.30], 2, false, 0.55),
            ab("The Eye of the Sun", "The sun looks down in anger — a great burning gaze no veil survives.", Archetype::Storm { radius: 9.0, duration: 6.0, dps: 68.0 }, 76.0, 130.0, [1.00, 0.70, 0.25], 3, true, 0.40),
        ],
    },
    // ── SNOWHUSH — the deep-snow lynxes of the white silence ──
    GodDef {
        id: 9, name: "Snowhush", title: "the deep-snow lynxes of the white silence",
        philosophy: "You need not be the strongest in the fight. Only the one who chooses where it happens.",
        role: Role::Guardian, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.82, 0.82, 0.85], [0.30, 0.35, 0.45], [0.96, 0.97, 1.00], [0.81, 0.90, 0.95]],
        weapon: WeaponKind::None, pelt: PeltKind::EarTufts,
        hp: 585.0, hp_growth: 94.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 48.0, auto_growth: 3.6, power: 8.0, power_growth: 2.4,
        protection: 28.0, prot_growth: 3.1, move_speed: 13.0, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Snowbind", "The drifts close over their paws — held fast where they stand.", Archetype::Ring { radius: 7.0, delay: 0.6, dmg: 110.0 }, 9.0, 70.0, [0.85, 0.95, 1.00], 0, false, 0.45),
            ab("White Silence", "A settling cold that slows every enemy near.", Archetype::Nova { radius: 8.0, dmg: 80.0, slow: 0.55 }, 8.0, 65.0, [0.80, 0.90, 1.00], 1, false, 0.40),
            ab("Hush of Snows", "Every blow carries the cold — a shell of rime over the pack.", Archetype::Shield { amount: 200.0, duration: 5.0 }, 14.0, 75.0, [0.90, 0.97, 1.00], 2, false, 0.30),
            ab("The Long Winter", "The season itself takes the field — everything near is frozen where it stands.", Archetype::Storm { radius: 11.0, duration: 5.5, dps: 52.0 }, 75.0, 125.0, [0.85, 0.95, 1.00], 3, true, 0.40),
        ],
    },
    // ── RIPTAIL — the drowned-forest jaguars of the flooded dark ──
    GodDef {
        id: 10, name: "Riptail", title: "the drowned-forest jaguars of the flooded dark",
        philosophy: "The river does not chase either. It gathers.",
        role: Role::Guardian, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.72, 0.55, 0.28], [0.15, 0.28, 0.20], [0.90, 0.80, 0.60], [0.23, 0.43, 0.29]],
        weapon: WeaponKind::None, pelt: PeltKind::Rosettes,
        hp: 649.0, hp_growth: 104.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 51.0, auto_growth: 3.8, power: 8.0, power_growth: 2.4,
        protection: 29.0, prot_growth: 3.2, move_speed: 12.5, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Undertow", "The current takes their legs — every enemy near is dragged in close.", Archetype::Pull { radius: 9.0, dmg: 80.0 }, 10.0, 75.0, [0.25, 0.55, 0.40], 0, false, 0.45),
            ab("Crushing Jaws", "The bite that cracks turtle shells.", Archetype::Bolt { speed: 30.0, dmg: 120.0, radius: 0.9 }, 7.5, 65.0, [0.35, 0.60, 0.40], 1, false, 0.55),
            ab("The River Remembers", "The held are already caught — a rake at the slowed and rooted.", Archetype::Nova { radius: 6.5, dmg: 100.0, slow: 0.45 }, 10.0, 65.0, [0.30, 0.55, 0.45], 2, false, 0.50),
            ab("The River Takes", "The flood answers — the whole fight is dragged to one drowning point.", Archetype::Pull { radius: 13.0, dmg: 240.0 }, 78.0, 125.0, [0.25, 0.60, 0.50], 3, true, 0.70),
        ],
    },
    // ── STILLSMOKE — the ninth son of the storm-dragon, of the far-east thunder mountains ──
    GodDef {
        id: 11, name: "Stillsmoke", title: "the ninth son of the storm-dragon, of the far-east thunder mountains",
        philosophy: "Sit still long enough, and even the storm will come to ask your leave.",
        role: Role::Mage, auto: AutoKind::Ranged, dmg_type: DmgType::Magical,
        palette: [[0.42, 0.35, 0.55], [0.22, 0.18, 0.30], [0.62, 0.55, 0.72], [0.35, 0.23, 0.54]],
        weapon: WeaponKind::None, pelt: PeltKind::Antlers,
        hp: 565.0, hp_growth: 90.0, mana: 380.0, mana_growth: 44.0,
        auto_dmg: 38.0, auto_growth: 2.9, power: 12.0, power_growth: 3.2,
        protection: 18.0, prot_growth: 2.0, move_speed: 12.2, auto_range: 9.6, auto_cd: 1.05,
        abilities: [
            ab("Cloud Breath", "A slow exhalation of mountain smoke — all it touches wades as through deep water.", Archetype::Ring { radius: 8.0, delay: 0.7, dmg: 110.0 }, 8.0, 70.0, [0.60, 0.55, 0.75], 0, false, 0.50),
            ab("Sweetrain", "The dragon's rain — packmates mend beneath it.", Archetype::Heal { amount: 220.0, radius: 7.0 }, 15.0, 80.0, [0.55, 0.75, 0.95], 1, false, 0.35),
            ab("The Seated Mountain", "Stillness gathers the storm — a lance of gathered thunder.", Archetype::Bolt { speed: 34.0, dmg: 105.0, radius: 0.8 }, 6.5, 65.0, [0.70, 0.60, 1.00], 2, false, 0.55),
            ab("The Ninth Thunder", "He stands, once — and the thunder his father lent him empties the field.", Archetype::Storm { radius: 9.5, duration: 6.0, dps: 62.0 }, 76.0, 130.0, [0.75, 0.60, 1.00], 3, true, 0.40),
        ],
    },
    // ── STONEWHISPER — the grey ghosts of the high crags ──
    GodDef {
        id: 12, name: "Stonewhisper", title: "the grey ghosts of the high crags",
        philosophy: "The mountain does not hurry, and the mountain is never late.",
        role: Role::Guardian, auto: AutoKind::Melee, dmg_type: DmgType::Physical,
        palette: [[0.60, 0.55, 0.48], [0.32, 0.28, 0.24], [0.80, 0.76, 0.70], [0.63, 0.51, 0.38]],
        weapon: WeaponKind::None, pelt: PeltKind::Crag,
        hp: 701.0, hp_growth: 112.0, mana: 300.0, mana_growth: 32.0,
        auto_dmg: 46.0, auto_growth: 3.4, power: 8.0, power_growth: 2.4,
        protection: 31.0, prot_growth: 3.4, move_speed: 12.0, auto_range: 3.4, auto_cd: 1.00,
        abilities: [
            ab("Crag Leap", "A boulder-to-boulder leap ending in a landslide of a landing.", Archetype::Dash { dist: 11.0, dmg: 105.0 }, 9.0, 65.0, [0.70, 0.65, 0.55], 0, false, 0.50),
            ab("Stone Patience", "Settle like bedrock — blows glance away.", Archetype::Shield { amount: 260.0, duration: 5.5 }, 16.0, 80.0, [0.75, 0.70, 0.60], 1, false, 0.35),
            ab("Sure-Footed", "Patient weight — a stamp that quakes the scree.", Archetype::Nova { radius: 6.5, dmg: 95.0, slow: 0.4 }, 10.0, 65.0, [0.65, 0.60, 0.50], 2, false, 0.45),
            ab("The Crags Answer", "The scree comes down — everything near is buried and battered.", Archetype::Ring { radius: 9.0, delay: 0.9, dmg: 250.0 }, 76.0, 120.0, [0.80, 0.72, 0.58], 3, true, 0.70),
        ],
    },
    // ── COMETAIL — the star-fallen, who came down with the comet ──
    GodDef {
        id: 13, name: "Cometail", title: "the star-fallen, who came down with the comet over the glacier",
        philosophy: "What falls from the sky does not ask where it may land.",
        role: Role::Assassin, auto: AutoKind::Melee, dmg_type: DmgType::Magical,
        palette: [[0.16, 0.20, 0.42], [0.10, 0.12, 0.26], [0.78, 0.82, 0.95], [1.00, 0.86, 0.46]],
        weapon: WeaponKind::None, pelt: PeltKind::Comet,
        hp: 590.0, hp_growth: 94.0, mana: 330.0, mana_growth: 38.0,
        auto_dmg: 50.0, auto_growth: 3.6, power: 11.0, power_growth: 3.0,
        protection: 21.0, prot_growth: 2.5, move_speed: 12.8, auto_range: 3.4, auto_cd: 0.95,
        abilities: [
            ab("Starshot", "A shard of the comet flung from the paw — it streaks, and it burns.", Archetype::Bolt { speed: 40.0, dmg: 95.0, radius: 0.7 }, 5.5, 55.0, [1.00, 0.92, 0.60], 0, false, 0.50),
            ab("Meteor Pounce", "He leaps and comes down burning — everything under the arc is scorched.", Archetype::Dash { dist: 12.0, dmg: 110.0 }, 9.0, 65.0, [1.00, 0.78, 0.40], 1, false, 0.50),
            ab("Flare", "His whole coat lights at once — a white blast that blinds and staggers.", Archetype::Nova { radius: 6.5, dmg: 105.0, slow: 0.35 }, 10.0, 70.0, [1.00, 0.97, 0.85], 2, false, 0.50),
            ab("Falling Star", "He vaults into the night and is gone — then he is the star, and he is falling.", Archetype::Meteor { range: 22.0, radius: 8.5, dmg: 300.0, vault: 1.1 }, 80.0, 130.0, [1.00, 0.88, 0.55], 3, true, 0.75),
        ],
    },
];

pub fn god(id: u32) -> &'static GodDef {
    &ROSTER[id as usize]
}

/// Palette slot indices used by the model builder.
pub const PAL_BODY: usize = 0; // main pelt
pub const PAL_CLOTH: usize = 1; // team collar / war-paint (team-tinted)
pub const PAL_METAL: usize = 2; // secondary pelt: belly, muzzle, mane, inner ear
pub const PAL_ACCENT: usize = 3; // crest colour: eyes + crest (from Clawbound's `crest_color`)

pub fn voxel_scale() -> f32 {
    VOXEL_SIZE
}

#[cfg(test)]
mod quick_aim_tests {
    use super::*;

    #[test]
    fn ultimates_are_never_quick_aimable() {
        for def in ROSTER.iter() {
            for ab in def.abilities.iter().filter(|a| a.ult) {
                assert!(!ab.quick_aimable(), "{}'s ultimate {} must never be a drag-to-aim quick cast", def.name, ab.name);
            }
        }
    }

    #[test]
    fn every_ancestor_has_a_real_mobility_option_to_drag_aim() {
        // not a hard requirement of the game design, but worth knowing if a
        // future ancestor ships with no dash/blink at all — touch's quick
        // aim would then just never activate for them, silently
        let with_mobility = ROSTER.iter().filter(|d| d.abilities.iter().any(|a| a.quick_aimable())).count();
        assert!(with_mobility > ROSTER.len() / 2, "expected most ancestors to have a drag-aimable mobility skill, found {with_mobility}/{}", ROSTER.len());
    }
}
