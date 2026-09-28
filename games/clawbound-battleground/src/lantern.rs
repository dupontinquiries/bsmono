//! lantern -- Brasshowl, the lamp-keeper, and what his light does.
//!
//! A sand cat: the smallest of the ancestors, with a voice (sand cats bark)
//! far bigger than he is. He walked the salt roads at night ahead of the
//! caravans with a brass lantern, and what waited outside its light did
//! not come in. In the game he is Hollowstep's answer:
//!
//! * **SHADOWS!** (`Archetype::Lantern`, the ultimate): he raises the lamp
//!   and bellows. Every enemy within its radius is struck, **flees** from
//!   him (`GodUnit::flee_t`: it runs straight away, and cannot attack or
//!   cast while it does), and is **shown up** (`GodUnit::revealed_t`: seen
//!   through cloud by everyone, and it cannot stalk -- the low, small,
//!   quiet carriage is gone). Any of Hollowstep's shadows standing in the
//!   light -- his other, his echoes -- is **driven off**: the echoes are
//!   gone, and his other goes to the far side of the map from its master
//!   (`stalker::elsewhere`), as it does when he respawns.
//! * The rest of the kit is the ordinary archetypes: burning oil (Lob), a
//!   bark (Nova), a watchfire (Shield).
//!
//! Headless; the scene hangs a small flame over a shown-up cat.

use glam::Vec3;

use crate::sim::{Event, Game, TargetRef};

/// How fast a fleeing cat runs, as a share of its speed.
pub const FLEE_SPEED: f32 = 1.0;

impl Game {
    /// Can cat `i` act at all (attack, cast)? Not giggling, not dancing
    /// (`kit`), not fleeing the lamp.
    pub fn lamp_free(&self, i: usize) -> bool {
        self.gods[i].flee_t <= 0.0
    }

    /// Where a fleeing cat's paws take it: straight away from what it
    /// flees, whatever it was steered. Anyone else goes where it was going.
    pub fn flee_dir(&self, i: usize, dir: Vec3) -> Vec3 {
        let g = &self.gods[i];
        if g.flee_t <= 0.0 {
            return dir;
        }
        let away = Vec3::new(g.pos.x - g.flee_from.x, 0.0, g.pos.z - g.flee_from.z);
        if away.length_squared() > 1e-6 {
            away.normalize()
        } else if dir.length_squared() > 1e-6 {
            dir
        } else {
            Vec3::X
        }
    }

    /// SHADOWS!: Brasshowl `gi` raises the lamp over where he stands.
    pub fn raise_the_lamp(&mut self, gi: usize, radius: f32, dmg: f32, flee: f32, reveal: f32) {
        let at = self.gods[gi].pos;
        let team = self.gods[gi].team;
        self.events.push(Event::LanternFx { pos: at, radius });
        for t in self.aoe_targets(at, radius, team) {
            if dmg > 0.0 {
                self.deal_damage(t, dmg, true, Some(gi), 0.0, 0.0);
            }
            if let TargetRef::God(j) = t {
                let g = &mut self.gods[j];
                g.flee_t = g.flee_t.max(flee);
                g.flee_from = at;
                g.revealed_t = g.revealed_t.max(reveal);
                // a wind-up under way is lost
                g.casting = None;
            }
        }
        // no shadow stands in the light
        let flat = |p: Vec3| Vec3::new(p.x - at.x, 0.0, p.z - at.z).length();
        let mut driven = Vec::new();
        self.others.retain(|o| {
            let hit = o.team != team && flat(o.pos) <= radius;
            if hit {
                driven.push((o.pos, o.echo, o.owner));
            }
            !hit
        });
        for (pos, _, _) in &driven {
            self.events.push(Event::ShadowDriven { pos: *pos });
        }
        // his other rises again, far off (`stalker_tick` puts it where it
        // puts it when he respawns: through the centre from him)
    }

    /// Once a tick: the fleeing and the shown-up count down.
    pub fn lantern_tick(&mut self, dt: f32) {
        for g in self.gods.iter_mut() {
            g.flee_t = (g.flee_t - dt).max(0.0);
            g.revealed_t = (g.revealed_t - dt).max(0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::sim::Game;
    use glam::Vec3;

    /// Shown up means seen: a cat the clouds hide from the other team is
    /// seen by it while the lamp's light is on it, and hidden again after.
    #[test]
    fn shown_up_is_seen_through_cloud() {
        let mut g = Game::new_teams(&[0], &[1]);
        let n = g.gods.len();
        g.vision.seen = [vec![false; n], vec![false; n]];
        let blue = g.gods[0].team;
        assert!(!g.sees(blue, 1), "hidden in cloud");
        g.gods[1].revealed_t = 2.0;
        assert!(g.sees(blue, 1), "shown up by the lamp");
        g.gods[1].revealed_t = 0.0;
        assert!(!g.sees(blue, 1));
    }

    /// A fleeing cat's paws take it straight away from the lamp, whatever
    /// it was steered; a cat not fleeing goes where it was steered.
    #[test]
    fn the_fleeing_run_straight_away() {
        let mut g = Game::new_teams(&[0], &[1]);
        g.gods[1].pos = Vec3::new(10.0, 0.0, 0.0);
        let toward = Vec3::new(-1.0, 0.0, 0.0);
        assert_eq!(g.flee_dir(1, toward), toward);
        g.gods[1].flee_t = 1.0;
        g.gods[1].flee_from = Vec3::ZERO;
        assert!(g.flee_dir(1, toward).dot(Vec3::X) > 0.99);
        assert!(!g.lamp_free(1));
    }
}
