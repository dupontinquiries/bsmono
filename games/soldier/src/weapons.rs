//! weapons.rs — the arsenal, retooled toward a modern-military (Black Ops 2-era) feel: an assault rifle,
//! an SMG, a pump shotgun, and a bolt sniper. Still pure data + hitscan, but now with magazines and a
//! reserve pool you reload from (R), plus per-weapon aim-down-sights zoom and a tighter ADS spread. Each
//! weapon carries a fire interval, pellet count + spread, damage, range, mag/reserve, reload time, a muzzle
//! flash colour, recoil kick, an auto flag, the ADS field-of-view, and whether it's scoped.

use egui::Color32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponKind {
    Ar,
    Smg,
    Shotgun,
    Sniper,
    Lmg,
    Dmr,
    Pistol,
    EmSword,
    /// Single-shot rocket: a real ballistic projectile (not hitscan) that detonates on impact with splash
    /// damage — see `Game::update_rockets`. Slow, heavy, punishes a miss, opens routes through destructibles.
    Launcher,
    /// **EXOTIC — Arc Coil.** A tesla weapon: the hitscan bolt that connects then *chains* to nearby enemies,
    /// arcing lightning between them for reduced damage per jump. Rewards clustered targets.
    ArcCoil,
    /// **EXOTIC — Cryo Lance.** A piercing frost beam: a single shot passes THROUGH every enemy along its line
    /// (no per-target falloff of the ray), hitting all of them. Line up a row and skewer it.
    CryoLance,
    /// **EXOTIC — Void Maul.** A heavy exotic melee: a slow, wide, high-damage swing with a short lunge — a
    /// harder-hitting cousin of the EM sword with more reach.
    VoidMaul,
}

impl WeaponKind {
    /// Every weapon, in a stable order (used for serialisation ids).
    pub const ALL: [WeaponKind; 12] = [
        WeaponKind::Ar,
        WeaponKind::Smg,
        WeaponKind::Shotgun,
        WeaponKind::Sniper,
        WeaponKind::Lmg,
        WeaponKind::Dmr,
        WeaponKind::Pistol,
        WeaponKind::EmSword,
        WeaponKind::Launcher,
        WeaponKind::ArcCoil,
        WeaponKind::CryoLance,
        WeaponKind::VoidMaul,
    ];
    pub fn id(self) -> u8 {
        WeaponKind::ALL.iter().position(|&w| w == self).unwrap_or(0) as u8
    }
    pub fn from_id(id: u8) -> WeaponKind {
        WeaponKind::ALL.get(id as usize).copied().unwrap_or(WeaponKind::Ar)
    }

    /// Primary weapons offered in the loadout builder.
    pub const PRIMARIES: [WeaponKind; 11] = [
        WeaponKind::Ar,
        WeaponKind::Smg,
        WeaponKind::Lmg,
        WeaponKind::Dmr,
        WeaponKind::Shotgun,
        WeaponKind::Sniper,
        WeaponKind::EmSword,
        WeaponKind::Launcher,
        WeaponKind::ArcCoil,
        WeaponKind::CryoLance,
        WeaponKind::VoidMaul,
    ];
    pub fn display(self) -> &'static str {
        self.def().name
    }
    /// A short class tag for the loadout UI.
    pub fn class(self) -> &'static str {
        match self {
            WeaponKind::Ar => "Assault Rifle",
            WeaponKind::Smg => "SMG",
            WeaponKind::Shotgun => "Shotgun",
            WeaponKind::Sniper => "Sniper",
            WeaponKind::Lmg => "LMG",
            WeaponKind::Dmr => "Marksman",
            WeaponKind::Pistol => "Sidearm",
            WeaponKind::EmSword => "Melee",
            WeaponKind::Launcher => "Launcher",
            WeaponKind::ArcCoil => "Exotic · Tesla",
            WeaponKind::CryoLance => "Exotic · Beam",
            WeaponKind::VoidMaul => "Exotic · Melee",
        }
    }
}

#[derive(Clone, Copy)]
pub struct WeaponDef {
    pub kind: WeaponKind,
    pub name: &'static str,
    /// Seconds between shots.
    pub interval: f32,
    /// Pellets per trigger pull (shotgun > 1).
    pub pellets: u32,
    /// Half-angle hip-fire spread in radians applied to each pellet (ADS tightens this).
    pub spread: f32,
    pub damage: f32,
    /// Max effective range in world units.
    pub range: f32,
    /// Rounds per magazine.
    pub mag_size: u32,
    /// Rounds carried in reserve at spawn.
    pub reserve: u32,
    /// Seconds to reload a magazine.
    pub reload: f32,
    pub muzzle: Color32,
    /// Recoil kick (radians of upward pitch per shot) and view punch.
    pub kick: f32,
    /// Horizontal recoil magnitude (radians of yaw sway per shot) — the pattern you fight against.
    pub recoil_h: f32,
    /// Is this weapon automatic (hold to fire)?
    pub auto: bool,
    /// Vertical field-of-view (degrees) when aiming down sights — smaller = more zoom.
    pub ads_fov: f32,
    /// Does ADS show a sniper scope overlay?
    pub scoped: bool,
    /// Is this a melee weapon (no ammo/reload; very short reach)?
    pub melee: bool,
    /// EXOTIC: chain-lightning jumps — after a hit, arc to this many nearby enemies (0 = none). Each jump does
    /// reduced damage.
    pub chain: u32,
    /// EXOTIC: piercing — the shot passes through enemies and keeps going, hitting everything on the line.
    pub pierce: bool,
}

impl WeaponKind {
    pub fn def(self) -> WeaponDef {
        match self {
            WeaponKind::Ar => WeaponDef {
                kind: self,
                name: "M8A1",
                interval: 0.105,
                pellets: 1,
                spread: 0.014,
                damage: 25.0,
                range: 110.0,
                mag_size: 32,
                reserve: 160,
                reload: 1.9,
                muzzle: Color32::from_rgb(255, 226, 150),
                kick: 0.013,
                recoil_h: 0.006,
                auto: true,
                ads_fov: 52.0,
                scoped: false,
                melee: false,
                chain: 0,
                pierce: false,
            },
            WeaponKind::Smg => WeaponDef {
                kind: self,
                name: "PDW-57",
                interval: 0.066,
                pellets: 1,
                spread: 0.024,
                damage: 17.0,
                range: 76.0,
                mag_size: 40,
                reserve: 200,
                reload: 1.7,
                muzzle: Color32::from_rgb(255, 232, 170),
                kick: 0.010,
                recoil_h: 0.010,
                auto: true,
                ads_fov: 56.0,
                scoped: false,
                melee: false,
                chain: 0,
                pierce: false,
            },
            WeaponKind::Shotgun => WeaponDef {
                kind: self,
                name: "R-870",
                interval: 0.85,
                pellets: 8,
                spread: 0.072,
                damage: 13.0,
                range: 34.0,
                mag_size: 6,
                reserve: 42,
                reload: 2.6,
                muzzle: Color32::from_rgb(255, 210, 130),
                kick: 0.05,
                recoil_h: 0.013,
                auto: false,
                ads_fov: 62.0,
                scoped: false,
                melee: false,
                chain: 0,
                pierce: false,
            },
            WeaponKind::Sniper => WeaponDef {
                kind: self,
                name: "DSR-50",
                interval: 1.15,
                pellets: 1,
                spread: 0.0025,
                damage: 95.0,
                range: 240.0,
                mag_size: 5,
                reserve: 30,
                reload: 2.8,
                muzzle: Color32::from_rgb(220, 235, 255),
                kick: 0.06,
                recoil_h: 0.011,
                auto: false,
                ads_fov: 20.0,
                scoped: true,
                melee: false,
                chain: 0,
                pierce: false,
            },
            WeaponKind::Lmg => WeaponDef {
                kind: self,
                name: "MK-48",
                interval: 0.095,
                pellets: 1,
                spread: 0.020,
                damage: 30.0,
                range: 130.0,
                mag_size: 100,
                reserve: 200,
                reload: 4.2,
                muzzle: Color32::from_rgb(255, 222, 150),
                kick: 0.016,
                recoil_h: 0.012,
                auto: true,
                ads_fov: 50.0,
                scoped: false,
                melee: false,
                chain: 0,
                pierce: false,
            },
            WeaponKind::Dmr => WeaponDef {
                kind: self,
                name: "SVD-M",
                interval: 0.22,
                pellets: 1,
                spread: 0.006,
                damage: 55.0,
                range: 180.0,
                mag_size: 12,
                reserve: 72,
                reload: 2.2,
                muzzle: Color32::from_rgb(240, 232, 190),
                kick: 0.03,
                recoil_h: 0.009,
                auto: false,
                ads_fov: 36.0,
                scoped: false,
                melee: false,
                chain: 0,
                pierce: false,
            },
            WeaponKind::Pistol => WeaponDef {
                kind: self,
                name: "B23-R",
                interval: 0.14,
                pellets: 1,
                spread: 0.02,
                damage: 18.0,
                range: 70.0,
                mag_size: 15,
                reserve: 90,
                reload: 1.4,
                muzzle: Color32::from_rgb(255, 236, 180),
                kick: 0.012,
                recoil_h: 0.008,
                auto: false,
                ads_fov: 58.0,
                scoped: false,
                melee: false,
                chain: 0,
                pierce: false,
            },
            WeaponKind::EmSword => WeaponDef {
                kind: self,
                name: "Arc Saber",
                interval: 0.62,   // slow swing
                pellets: 1,
                spread: 0.0,
                damage: 120.0,    // high — a clean kill on most
                range: 4.6,       // melee reach
                mag_size: 1,
                reserve: 0,
                reload: 0.0,
                muzzle: Color32::from_rgb(120, 200, 255),
                kick: 0.02,
                recoil_h: 0.0,
                auto: false,
                ads_fov: 60.0,
                scoped: false,
                melee: true,
                chain: 0,
                pierce: false,
            },
            WeaponKind::Launcher => WeaponDef {
                kind: self,
                name: "RPL-9",
                interval: 1.6,     // slow — this is a burst-the-room tool, not a DPS weapon
                pellets: 1,
                spread: 0.006,
                damage: 130.0,     // direct-hit damage; splash falls off from ROCKET_BLAST in game.rs
                range: 160.0,      // max flight distance before self-detonating (a dud shouldn't vanish silently)
                mag_size: 1,
                reserve: 8,
                reload: 2.6,
                muzzle: Color32::from_rgb(255, 160, 90),
                kick: 0.10,        // real shoulder-fired recoil
                recoil_h: 0.02,
                auto: false,
                ads_fov: 58.0,
                scoped: false,
                melee: false,
                chain: 0,
                pierce: false,
            },
            WeaponKind::ArcCoil => WeaponDef {
                kind: self,
                name: "TESLA-ARC",
                interval: 0.5, // slow per shot, but each shot forks across a cluster
                pellets: 1,
                spread: 0.01,
                damage: 46.0,
                range: 80.0,
                mag_size: 12,
                reserve: 72,
                reload: 2.2,
                muzzle: Color32::from_rgb(140, 210, 255),
                kick: 0.02,
                recoil_h: 0.006,
                auto: false,
                ads_fov: 56.0,
                scoped: false,
                melee: false,
                chain: 3, // arcs to up to 3 additional nearby enemies
                pierce: false,
            },
            WeaponKind::CryoLance => WeaponDef {
                kind: self,
                name: "CRYO-LANCE",
                interval: 0.85, // a charged beam cadence
                pellets: 1,
                spread: 0.0, // a precise beam
                damage: 70.0,
                range: 140.0,
                mag_size: 6,
                reserve: 42,
                reload: 2.4,
                muzzle: Color32::from_rgb(180, 240, 255),
                kick: 0.03,
                recoil_h: 0.0,
                auto: false,
                ads_fov: 46.0,
                scoped: false,
                melee: false,
                chain: 0,
                pierce: true, // passes through every enemy on the line
            },
            WeaponKind::VoidMaul => WeaponDef {
                kind: self,
                name: "VOID-MAUL",
                interval: 0.7, // slow, heavy swings
                pellets: 1,
                spread: 0.0,
                damage: 180.0, // a brutal exotic melee
                range: 3.6,    // longer reach than the EM sword
                mag_size: 0,
                reserve: 0,
                reload: 0.0,
                muzzle: Color32::from_rgb(150, 90, 200),
                kick: 0.05,
                recoil_h: 0.0,
                auto: false,
                ads_fov: 60.0,
                scoped: false,
                melee: true,
                chain: 0,
                pierce: false,
            },
        }
    }
}

/// Per-weapon live state the player carries: a loaded magazine plus a reserve you reload from.
#[derive(Clone, Copy)]
pub struct WeaponState {
    pub kind: WeaponKind,
    pub mag: u32,
    pub reserve: u32,
    pub cooldown: f32,
    /// Seconds left in an in-progress reload (0 = not reloading).
    pub reloading: f32,
    /// Reload-time multiplier from perks (Quick Reload < 1.0). Applied when a reload starts.
    pub reload_mul: f32,
}
impl WeaponState {
    pub fn new(kind: WeaponKind) -> Self {
        let d = kind.def();
        Self { kind, mag: d.mag_size, reserve: d.reserve, cooldown: 0.0, reloading: 0.0, reload_mul: 1.0 }
    }
    /// Ready to fire: off cooldown, not mid-reload, and at least one round chambered.
    pub fn ready(&self) -> bool {
        // melee weapons never run dry
        (self.kind.def().melee || self.mag > 0) && self.cooldown <= 0.0 && self.reloading <= 0.0
    }
    /// Begin a reload if it makes sense (not already reloading, mag not full, reserve available).
    pub fn start_reload(&mut self) {
        let d = self.kind.def();
        if d.melee {
            return;
        }
        if self.reloading <= 0.0 && self.mag < d.mag_size && self.reserve > 0 {
            self.reloading = d.reload * self.reload_mul;
        }
    }
    /// Consume one chambered round (call once per shot).
    pub fn consume(&mut self) {
        if !self.kind.def().melee {
            self.mag = self.mag.saturating_sub(1);
        }
    }
    /// Add rounds to the reserve, capped at 2× the mag size of spare capacity over the base reserve.
    pub fn add_reserve(&mut self, amt: u32) -> bool {
        let d = self.kind.def();
        let cap = d.reserve + d.mag_size * 2;
        if self.reserve >= cap {
            return false;
        }
        self.reserve = (self.reserve + amt).min(cap);
        true
    }
    pub fn tick(&mut self, dt: f32) {
        if self.cooldown > 0.0 {
            self.cooldown -= dt;
        }
        if self.reloading > 0.0 {
            self.reloading -= dt;
            if self.reloading <= 0.0 {
                self.reloading = 0.0;
                let d = self.kind.def();
                let need = d.mag_size - self.mag;
                let take = need.min(self.reserve);
                self.mag += take;
                self.reserve -= take;
            }
        }
    }
}

/// The four-slot loadout the player spawns with, in select order.
pub const LOADOUT: [WeaponKind; 4] = [WeaponKind::Ar, WeaponKind::Smg, WeaponKind::Shotgun, WeaponKind::Sniper];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shotgun_throws_a_spread_of_pellets() {
        let d = WeaponKind::Shotgun.def();
        assert!(d.pellets > 1 && d.spread > 0.0);
    }

    #[test]
    fn cooldown_gates_firing() {
        let mut w = WeaponState::new(WeaponKind::Ar);
        assert!(w.ready());
        w.cooldown = w.kind.def().interval;
        assert!(!w.ready());
        w.tick(1.0); // big dt clears it
        assert!(w.ready());
    }

    #[test]
    fn empty_mag_is_not_ready() {
        let mut w = WeaponState::new(WeaponKind::Ar);
        w.mag = 0;
        assert!(!w.ready());
    }

    #[test]
    fn reload_pulls_from_reserve() {
        let mut w = WeaponState::new(WeaponKind::Ar);
        let mag0 = w.mag;
        w.mag = 4; // spent most of the mag
        let reserve0 = w.reserve;
        w.start_reload();
        assert!(w.reloading > 0.0);
        assert!(!w.ready()); // can't fire mid-reload
        w.tick(10.0); // finish it
        assert_eq!(w.mag, mag0);
        assert_eq!(w.reserve, reserve0 - (mag0 - 4));
    }

    #[test]
    fn reload_is_a_noop_on_full_mag() {
        let mut w = WeaponState::new(WeaponKind::Smg);
        w.start_reload();
        assert_eq!(w.reloading, 0.0);
    }
    #[test]
    fn em_sword_is_a_melee_weapon_with_no_ammo() {
        let d = WeaponKind::EmSword.def();
        assert!(d.melee, "sword should be melee");
        assert!(d.range < 6.0, "melee reach should be short");
        assert!(d.damage >= 100.0, "sword should hit hard");
        // never runs dry: stays ready through repeated swings, and reload is a no-op
        let mut w = WeaponState::new(WeaponKind::EmSword);
        for _ in 0..50 {
            assert!(w.ready(), "melee should always be ready off-cooldown");
            w.consume();
        }
        w.start_reload();
        assert!(w.reloading <= 0.0, "melee should never enter a reload");
    }

}
