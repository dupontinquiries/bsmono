//! battleview.rs — what a renderer needs to know, without a renderer.
//!
//! The battleground is playable in a terminal and reachable from the menu; what
//! is missing is the part that draws it inside the app. That work is mostly
//! *mapping* — take the world, produce marks and numbers — and mapping is worth
//! doing on this side of the seam, where it can be tested, so the code in
//! `app.rs` stays the thin layer that turns marks into meshes and numbers into
//! egui rows.
//!
//! It is also what makes the same battleground drawable twice: `main.rs`'s
//! `battle` render mode and the app's live view ask for the same list, and the
//! TTY client's map is the same information at a coarser resolution.

use crate::ancestor;
use crate::battleground::{Battleground, Carry, Mind, Role, CLAN_A};

/// What a thing on the field is, for a renderer choosing a shape and a colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkKind {
    Ancestor,
    Patrol,
    HealTree,
    FallenTree,
    GreatTree,
    Prey,
    RichPrey,
    Herb,
    Skyrock,
    Scorch,
    Rain,
}

/// One thing to draw.
#[derive(Clone, Copy, Debug)]
pub struct Mark {
    pub kind: MarkKind,
    pub x: f32,
    pub z: f32,
    /// Ground height at the mark, so the renderer does not sample terrain twice.
    pub y: f32,
    /// Facing, where it means anything.
    pub heading: f32,
    /// Which clan owns it: `Some(0)`, `Some(1)`, or `None` for neutral ground.
    pub team: Option<u8>,
    /// Fullness — hp fraction for a warrior or a structure, radius for a zone.
    /// A renderer scales a canopy or a ring by this rather than recomputing it.
    pub fill: f32,
    /// Whether this is the warrior the local player is driving.
    pub is_self: bool,
    /// A carried burden, for the little shape over the shoulders.
    pub carry: Option<Carry>,
}

/// Everything a HUD shows, resolved once so a renderer is not reaching into the
/// sim mid-frame and picking different numbers in different widgets.
#[derive(Clone, Debug)]
pub struct Hud {
    pub time: f32,
    pub night: bool,
    pub leafbare: bool,
    /// Name, hp, max hp of the driven warrior, if there is one.
    pub who: Option<(&'static str, f32, f32)>,
    /// (name, cooldown remaining, ready) for the three ability slots.
    pub abilities: [(&'static str, f32, bool); 3],
    pub pile: [f32; 2],
    pub herbs: [u32; 2],
    pub great_tree: [f32; 2],
    pub trees_standing: [usize; 2],
    pub contention: Option<u8>,
    pub skyrock: Option<u8>,
    pub carrying: Option<Carry>,
    pub down_for: Option<f32>,
    pub outcome: Option<u8>,
}

/// Where the camera should look: the driven warrior, or the middle of one's own
/// living clan when the player is down or absent, so a spectating frame is not
/// staring at the corner of the map.
pub fn focus(bg: &Battleground, side: u8) -> (f32, f32) {
    if let Some(i) = bg.driven(side) {
        if bg.warriors[i].alive() {
            return (bg.warriors[i].x, bg.warriors[i].z);
        }
    }
    let mine: Vec<&_> = bg
        .warriors
        .iter()
        .filter(|w| w.team == side && w.alive() && w.role == Role::Ancestor)
        .collect();
    if mine.is_empty() {
        return bg.camps[side as usize];
    }
    let n = mine.len() as f32;
    (
        mine.iter().map(|w| w.x).sum::<f32>() / n,
        mine.iter().map(|w| w.z).sum::<f32>() / n,
    )
}

/// Everything worth drawing, in one pass. `within` culls to a radius around the
/// focus — a renderer does not want four hundred marks for a screen that shows
/// thirty, and the sim is large on purpose.
pub fn marks(bg: &Battleground, side: u8, within: f32) -> Vec<Mark> {
    let (fx, fz) = focus(bg, side);
    let near = |x: f32, z: f32| ((x - fx).powi(2) + (z - fz).powi(2)).sqrt() <= within;
    let ground = crate::world::height_at;
    let driven = bg.driven(side);
    let mut out = Vec::new();

    for (i, w) in bg.warriors.iter().enumerate() {
        if !w.alive() || !near(w.x, w.z) {
            continue;
        }
        out.push(Mark {
            kind: if w.role == Role::Ancestor { MarkKind::Ancestor } else { MarkKind::Patrol },
            x: w.x,
            z: w.z,
            y: ground(w.x, w.z),
            heading: w.heading,
            team: Some(w.team),
            fill: (w.hp / w.max_hp).clamp(0.0, 1.0),
            is_self: driven == Some(i),
            carry: match w.carry {
                Carry::Nothing => None,
                c => Some(c),
            },
        });
    }
    for t in &bg.heal_trees {
        if !near(t.x, t.z) {
            continue;
        }
        out.push(Mark {
            kind: if t.alive() { MarkKind::HealTree } else { MarkKind::FallenTree },
            x: t.x,
            z: t.z,
            y: ground(t.x, t.z),
            heading: 0.0,
            team: Some(t.team),
            fill: (t.hp / 550.0).clamp(0.0, 1.0),
            is_self: false,
            carry: None,
        });
    }
    for (team, &(cx, cz)) in bg.camps.iter().enumerate() {
        if !near(cx, cz) {
            continue;
        }
        out.push(Mark {
            kind: MarkKind::GreatTree,
            x: cx,
            z: cz,
            y: ground(cx, cz),
            heading: 0.0,
            team: Some(team as u8),
            fill: (bg.clans[team].great_tree_hp / 1400.0).clamp(0.0, 1.0),
            is_self: false,
            carry: None,
        });
    }
    for p in bg.prey.iter().filter(|p| p.alive()) {
        if !near(p.x, p.z) {
            continue;
        }
        out.push(Mark {
            kind: if p.rich() { MarkKind::RichPrey } else { MarkKind::Prey },
            x: p.x,
            z: p.z,
            y: ground(p.x, p.z),
            heading: 0.0,
            team: None,
            fill: 1.0,
            is_self: false,
            carry: None,
        });
    }
    for h in bg.herb_patches.iter().filter(|h| h.ready()) {
        if !near(h.x, h.z) {
            continue;
        }
        out.push(Mark {
            kind: MarkKind::Herb,
            x: h.x,
            z: h.z,
            y: ground(h.x, h.z),
            heading: 0.0,
            team: None,
            fill: 1.0,
            is_self: false,
            carry: None,
        });
    }
    for z in &bg.zones {
        if !near(z.x, z.z) {
            continue;
        }
        out.push(Mark {
            kind: if z.mend { MarkKind::Rain } else { MarkKind::Scorch },
            x: z.x,
            z: z.z,
            y: ground(z.x, z.z),
            heading: 0.0,
            team: Some(z.team),
            fill: z.r,
            is_self: false,
            carry: None,
        });
    }
    let (sx, sz) = bg.skyrock;
    if near(sx, sz) {
        out.push(Mark {
            kind: MarkKind::Skyrock,
            x: sx,
            z: sz,
            y: ground(sx, sz),
            heading: 0.0,
            team: bg.skyrock_holder,
            fill: 1.0,
            is_self: false,
            carry: None,
        });
    }
    out
}

/// The HUD numbers, taken once.
pub fn hud(bg: &Battleground, side: u8) -> Hud {
    let driven = bg.driven(side).or_else(|| {
        bg.warriors
            .iter()
            .position(|w| w.team == side && w.is_player && w.role == Role::Ancestor)
    });
    let w = driven.map(|i| &bg.warriors[i]);
    let kit = w.and_then(|w| w.ancestor).map(ancestor::kit);
    let abilities = match kit {
        Some(k) => [
            (k.abilities[0].name, w.map(|w| w.cds[0]).unwrap_or(0.0), w.map(|w| w.cds[0] <= 0.0).unwrap_or(false)),
            (k.abilities[1].name, w.map(|w| w.cds[1]).unwrap_or(0.0), w.map(|w| w.cds[1] <= 0.0).unwrap_or(false)),
            (k.ult.name, w.map(|w| w.cds[2]).unwrap_or(0.0), w.map(|w| w.cds[2] <= 0.0).unwrap_or(false)),
        ],
        None => [("", 0.0, false); 3],
    };
    let standing = |t: u8| bg.heal_trees.iter().filter(|h| h.team == t && h.alive()).count();
    Hud {
        time: bg.time,
        night: bg.is_night(),
        leafbare: bg.leafbare(),
        who: kit.zip(w).map(|(k, w)| (k.name, w.hp, w.max_hp)),
        abilities,
        pile: [bg.clans[0].pile, bg.clans[1].pile],
        herbs: [bg.clans[0].herbs, bg.clans[1].herbs],
        great_tree: [bg.clans[0].great_tree_hp.max(0.0), bg.clans[1].great_tree_hp.max(0.0)],
        trees_standing: [standing(CLAN_A), standing(1)],
        contention: bg.contention_holder,
        skyrock: bg.skyrock_holder,
        carrying: w.and_then(|w| match w.carry {
            Carry::Nothing => None,
            c => Some(c),
        }),
        down_for: w.filter(|w| w.mind == Mind::Dead).map(|w| w.respawn_t),
        outcome: bg.outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ancestor::Ancestor;

    #[test]
    fn the_view_shows_the_field_and_culls_the_rest() {
        let mut bg = Battleground::new(Ancestor::Lion);
        for _ in 0..30 * 20 {
            bg.step(1.0 / 30.0);
        }
        let all = marks(&bg, CLAN_A, 1e9);
        let near = marks(&bg, CLAN_A, 30.0);
        assert!(near.len() < all.len(), "a radius should cull: {} of {}", near.len(), all.len());
        // the things a battleground is made of are all representable
        let kinds: Vec<MarkKind> = all.iter().map(|m| m.kind).collect();
        for k in [
            MarkKind::Ancestor,
            MarkKind::HealTree,
            MarkKind::GreatTree,
            MarkKind::Prey,
            MarkKind::Skyrock,
        ] {
            assert!(kinds.contains(&k), "{k:?} missing from the view");
        }
        // every mark sits on the ground the renderer would have had to sample
        for m in &all {
            assert!((m.y - crate::world::height_at(m.x, m.z)).abs() < 1e-6);
        }
        // both Great Trees are there, one per clan, scaled by their hp
        let greats: Vec<&Mark> = all.iter().filter(|m| m.kind == MarkKind::GreatTree).collect();
        assert_eq!(greats.len(), 2);
        assert!(greats.iter().all(|m| m.fill > 0.9), "nobody has been gnawed yet");
    }

    #[test]
    fn the_camera_follows_the_player_and_the_clan_when_the_player_is_down() {
        let mut bg = Battleground::new(Ancestor::Serval);
        // nobody driving yet: the focus is the clan's centre, on the map
        let (fx, fz) = focus(&bg, CLAN_A);
        assert!(fx.abs() < 150.0 && fz.abs() < 150.0);
        // now drive, and it follows the warrior
        for _ in 0..30 {
            bg.set_input(1.0, 0.0, false, None);
            bg.step(1.0 / 30.0);
        }
        let i = bg.driven(CLAN_A).expect("driven");
        assert_eq!(focus(&bg, CLAN_A), (bg.warriors[i].x, bg.warriors[i].z));
        // and when that warrior is down, the view moves to what is left of the
        // clan rather than sitting on a corpse
        bg.warriors[i].hp = 0.0;
        bg.warriors[i].mind = Mind::Dead;
        let f = focus(&bg, CLAN_A);
        assert_ne!(f, (bg.warriors[i].x, bg.warriors[i].z));
        let others: Vec<&_> = bg
            .warriors
            .iter()
            .filter(|w| w.team == CLAN_A && w.alive() && w.role == Role::Ancestor)
            .collect();
        assert!(!others.is_empty());
    }

    #[test]
    fn the_hud_reads_the_same_numbers_the_sim_holds() {
        let mut bg = Battleground::new(Ancestor::Lynx);
        bg.clans[0].pile = 7.0;
        bg.clans[1].herbs = 2;
        for _ in 0..30 {
            bg.set_input(0.0, 0.0, false, Some(0));
            bg.step(1.0 / 30.0);
        }
        let h = hud(&bg, CLAN_A);
        assert_eq!(h.who.map(|w| w.0), Some(ancestor::kit(Ancestor::Lynx).name));
        assert_eq!(h.pile[0], 7.0);
        assert_eq!(h.herbs[1], 2);
        assert_eq!(h.trees_standing, [3, 3]);
        assert_eq!(h.abilities[0].0, ancestor::kit(Ancestor::Lynx).abilities[0].name);
        assert!(!h.abilities[0].2, "slot 1 was just cast, so it is not ready");
        assert!(h.abilities[2].2, "the ultimate is untouched");
        assert_eq!(h.outcome, None);
        // a HUD for a clan nobody drives still names that clan's leader, so a
        // spectator or a replay viewer sees a filled panel rather than blanks
        let h2 = hud(&bg, 1);
        assert!(h2.who.is_some());
    }
}
