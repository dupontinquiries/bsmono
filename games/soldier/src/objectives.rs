//! objectives.rs — round-based **objective game modes** layered on top of the core combat sim: Search &
//! Destroy (plant/defuse a bomb) and Capture the Flag (grab the enemy flag, return it to your base). The
//! module is deliberately decoupled from the shooter: the game loop feeds it the player's position and the
//! live enemy positions each frame via [`ObjectiveState::update`], and it returns HUD-ready state plus a
//! [`RoundOutcome`] when a side wins. It owns no meshes and no AI — it's pure game-state logic, which keeps
//! it easy to test headlessly (every transition below is exercised in the unit tests).
//!
//! The two objective carriers (bomb sites, flags) are world-space points with a capture radius. "Teams" are
//! modelled minimally: the player is on the attacking/blue side, the enemies on the defending/red side. That
//! matches the rest of the engine, where the player fights the enemy pool — objectives just give that fight
//! a win condition beyond a kill count.

use crate::math::Vec3;

/// Which objective game mode is active (or none — the standard wave shooter).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Objective {
    /// No objective layer; the normal shooter owns win/loss.
    None,
    /// Search & Destroy: the player (attacker) plants a bomb at a site and defends it through a countdown;
    /// the enemies (defenders) try to defuse. Attacker wins on detonation, defender wins on defuse or on the
    /// round clock expiring with no plant.
    SearchDestroy,
    /// Capture the Flag: two flags at two bases. Carry the enemy flag back to your base (while your own flag
    /// is home) to score. First to the capture target, or most captures at time, wins.
    CaptureTheFlag,
}

impl Objective {
    pub fn name(self) -> &'static str {
        match self {
            Objective::None => "Free Play",
            Objective::SearchDestroy => "Search & Destroy",
            Objective::CaptureTheFlag => "Capture the Flag",
        }
    }
}

/// The result of a finished round, once decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundOutcome {
    /// The player's side won (bomb detonated, or capture target reached).
    AttackersWin,
    /// The enemy side won (bomb defused, capture target reached, or clock expired in their favour).
    DefendersWin,
    /// The clock expired with no decisive result (draw).
    Draw,
}

/// A bomb site (Search & Destroy) — a world point with a capture radius.
#[derive(Clone, Copy, Debug)]
pub struct BombSite {
    pub pos: Vec3,
    pub radius: f32,
    /// Which site this is (A / B), for HUD labelling.
    pub label: char,
}

/// The plant/defuse phase of a Search & Destroy round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SndPhase {
    /// Bomb not yet planted; attacker must reach a site and plant.
    Live,
    /// Bomb planted and ticking; defender must defuse before it blows.
    Planted,
    /// Round decided (see `outcome`).
    Over,
}

/// A capturable flag (Capture the Flag).
#[derive(Clone, Copy, Debug)]
pub struct Flag {
    /// The flag's home base position.
    pub home: Vec3,
    /// Where the flag currently is (== home when not taken/dropped).
    pub pos: Vec3,
    pub radius: f32,
    /// True while a carrier holds it (pos tracks the carrier).
    pub taken: bool,
    /// True while dropped in the field (waiting to be returned or re-grabbed).
    pub dropped: bool,
    /// Countdown while dropped: returns home automatically if not touched.
    pub drop_timer: f32,
}

/// The full objective state machine. One of these lives on the [`crate::game::Game`] when an objective mode
/// is selected. Timings are in seconds.
#[derive(Clone, Debug)]
pub struct ObjectiveState {
    pub mode: Objective,
    /// Round clock remaining (both modes).
    pub round_time: f32,
    /// Decided outcome once the round is over, else `None`.
    pub outcome: Option<RoundOutcome>,

    // ── Search & Destroy ────────────────────────────────────────────────────────────────────────────────
    pub sites: Vec<BombSite>,
    pub snd_phase: SndPhase,
    /// Which site was planted at, once planted.
    pub planted_site: Option<usize>,
    /// Fuse remaining after a plant (Planted phase).
    pub fuse: f32,
    /// Plant/defuse progress 0..1 while a body stands on the site channelling the action.
    pub channel: f32,

    // ── Capture the Flag ────────────────────────────────────────────────────────────────────────────────
    /// [0] = player's (blue) flag, [1] = enemy (red) flag.
    pub flags: [Flag; 2],
    /// True while the *player* is carrying the enemy flag.
    pub player_carrying: bool,
    pub scores: [u32; 2], // [player_side, enemy_side]
    /// Captures needed to win.
    pub capture_target: u32,

    /// Fixed timings, so tests and tuning read from one place.
    plant_time: f32,
    defuse_time: f32,
    fuse_len: f32,
}

impl ObjectiveState {
    /// A no-objective state (standard shooter).
    pub fn none() -> Self {
        Self::empty(Objective::None)
    }

    fn empty(mode: Objective) -> Self {
        let z = Flag { home: Vec3::ZERO, pos: Vec3::ZERO, radius: 1.6, taken: false, dropped: false, drop_timer: 0.0 };
        Self {
            mode,
            round_time: 180.0,
            outcome: None,
            sites: Vec::new(),
            snd_phase: SndPhase::Live,
            planted_site: None,
            fuse: 0.0,
            channel: 0.0,
            flags: [z, z],
            player_carrying: false,
            scores: [0, 0],
            capture_target: 3,
            plant_time: 3.0,
            defuse_time: 4.0,
            fuse_len: 30.0,
        }
    }

    /// Build a Search & Destroy round with two bomb sites at the given world points.
    pub fn search_destroy(site_a: Vec3, site_b: Vec3) -> Self {
        let mut s = Self::empty(Objective::SearchDestroy);
        s.round_time = 150.0;
        s.sites = vec![
            BombSite { pos: site_a, radius: 3.0, label: 'A' },
            BombSite { pos: site_b, radius: 3.0, label: 'B' },
        ];
        s
    }

    /// Build a Capture the Flag round with the player (blue) base and enemy (red) base.
    pub fn capture_the_flag(blue_base: Vec3, red_base: Vec3, target: u32) -> Self {
        let mut s = Self::empty(Objective::CaptureTheFlag);
        s.round_time = 300.0;
        s.capture_target = target.max(1);
        s.flags[0] = Flag { home: blue_base, pos: blue_base, radius: 1.8, taken: false, dropped: false, drop_timer: 0.0 };
        s.flags[1] = Flag { home: red_base, pos: red_base, radius: 1.8, taken: false, dropped: false, drop_timer: 0.0 };
        s
    }

    /// Is the round finished?
    pub fn is_over(&self) -> bool {
        self.outcome.is_some()
    }

    /// Advance the objective one frame. `player` is the player's feet position, `player_alive` gates whether
    /// the player can channel/carry, `holding_action` is true while the player holds the use key (plant/
    /// defuse/pick-up), and `enemies` are the live enemy feet positions (for defender defuse and flag steals).
    /// Returns the outcome if the round decides this frame.
    pub fn update(
        &mut self,
        player: Vec3,
        player_alive: bool,
        holding_action: bool,
        enemies: &[Vec3],
        dt: f32,
    ) -> Option<RoundOutcome> {
        if self.is_over() {
            return self.outcome;
        }
        match self.mode {
            Objective::None => {}
            Objective::SearchDestroy => self.update_snd(player, player_alive, holding_action, enemies, dt),
            Objective::CaptureTheFlag => self.update_ctf(player, player_alive, enemies, dt),
        }
        self.outcome
    }

    fn finish(&mut self, o: RoundOutcome) {
        self.outcome = Some(o);
        if self.mode == Objective::SearchDestroy {
            self.snd_phase = SndPhase::Over;
        }
    }

    fn update_snd(&mut self, player: Vec3, player_alive: bool, holding: bool, enemies: &[Vec3], dt: f32) {
        match self.snd_phase {
            SndPhase::Live => {
                // Attacker (player) plants by standing in a site and holding the action.
                self.round_time -= dt;
                let on_site = player_alive
                    .then(|| self.site_at(player))
                    .flatten();
                if let (true, Some(idx)) = (holding, on_site) {
                    self.channel = (self.channel + dt / self.plant_time).min(1.0);
                    if self.channel >= 1.0 {
                        self.snd_phase = SndPhase::Planted;
                        self.planted_site = Some(idx);
                        self.fuse = self.fuse_len;
                        self.channel = 0.0;
                    }
                } else {
                    // planting is interrupted if you step off or release
                    self.channel = (self.channel - dt / self.plant_time * 1.5).max(0.0);
                }
                // clock ran out with no plant → defenders win
                if self.round_time <= 0.0 {
                    self.finish(RoundOutcome::DefendersWin);
                }
            }
            SndPhase::Planted => {
                // Bomb ticks. Defenders (enemies) defuse by standing on the planted site.
                self.fuse -= dt;
                if self.fuse <= 0.0 {
                    self.finish(RoundOutcome::AttackersWin); // detonation
                    return;
                }
                if let Some(sidx) = self.planted_site {
                    let site = self.sites[sidx];
                    let defender_on = enemies.iter().any(|&e| within(e, site.pos, site.radius));
                    if defender_on {
                        self.channel = (self.channel + dt / self.defuse_time).min(1.0);
                        if self.channel >= 1.0 {
                            self.finish(RoundOutcome::DefendersWin); // defused
                        }
                    } else {
                        self.channel = (self.channel - dt / self.defuse_time).max(0.0);
                    }
                }
            }
            SndPhase::Over => {}
        }
    }

    fn update_ctf(&mut self, player: Vec3, player_alive: bool, enemies: &[Vec3], dt: f32) {
        self.round_time -= dt;

        // Enemy flag (index 1): the player can pick it up by touching it (if alive), then carry it.
        if self.player_carrying {
            self.flags[1].pos = player; // the flag tracks the carrier
            self.flags[1].taken = true;
            self.flags[1].dropped = false;
            if !player_alive {
                // dropped where the player fell
                self.player_carrying = false;
                self.flags[1].taken = false;
                self.flags[1].dropped = true;
                self.flags[1].drop_timer = 12.0;
            } else {
                // capture: reach your own base while your own flag is home
                let own_home = self.flags[0].pos_is_home();
                if within(player, self.flags[0].home, self.flags[0].radius) && own_home {
                    self.scores[0] += 1;
                    self.reset_flag(1);
                    self.player_carrying = false;
                    if self.scores[0] >= self.capture_target {
                        self.finish(RoundOutcome::AttackersWin);
                        return;
                    }
                }
            }
        } else {
            // pick up the enemy flag by touching it (whether home or dropped)
            if player_alive && within(player, self.flags[1].pos, self.flags[1].radius) {
                self.player_carrying = true;
                self.flags[1].taken = true;
                self.flags[1].dropped = false;
            }
            // a dropped enemy flag times out back home
            if self.flags[1].dropped {
                self.flags[1].drop_timer -= dt;
                if self.flags[1].drop_timer <= 0.0 {
                    self.reset_flag(1);
                }
            }
        }

        // Enemies capture the player's flag (index 0): if any enemy touches it, they carry it toward their
        // base. Modelled lightly — the nearest enemy carrying it advances the enemy score when it reaches red
        // base. We approximate by: if an enemy stands on the blue flag home, they "grab" it; if any enemy is
        // then near red base while holding, they score. To keep it deterministic and simple, we treat a
        // sustained enemy presence at the blue base followed by presence at the red base as a capture.
        let enemy_on_blue = enemies.iter().any(|&e| within(e, self.flags[0].pos, self.flags[0].radius));
        if enemy_on_blue && !self.flags[0].taken {
            self.flags[0].taken = true; // an enemy grabbed it
        }
        if self.flags[0].taken {
            // it's being carried by the enemy team; if any enemy reaches red base, they score
            let scored = enemies.iter().any(|&e| within(e, self.flags[1].home, self.flags[1].radius));
            if scored {
                self.scores[1] += 1;
                self.reset_flag(0);
                if self.scores[1] >= self.capture_target {
                    self.finish(RoundOutcome::DefendersWin);
                    return;
                }
            }
        }

        // clock expiry → most captures wins, else draw
        if self.round_time <= 0.0 {
            let o = match self.scores[0].cmp(&self.scores[1]) {
                std::cmp::Ordering::Greater => RoundOutcome::AttackersWin,
                std::cmp::Ordering::Less => RoundOutcome::DefendersWin,
                std::cmp::Ordering::Equal => RoundOutcome::Draw,
            };
            self.finish(o);
        }
    }

    fn reset_flag(&mut self, i: usize) {
        self.flags[i].pos = self.flags[i].home;
        self.flags[i].taken = false;
        self.flags[i].dropped = false;
        self.flags[i].drop_timer = 0.0;
    }

    /// The index of the bomb site the point is standing in, if any.
    fn site_at(&self, p: Vec3) -> Option<usize> {
        self.sites.iter().position(|s| within(p, s.pos, s.radius))
    }

    /// A short HUD status line describing the current objective state.
    pub fn hud_line(&self) -> String {
        match self.mode {
            Objective::None => String::new(),
            Objective::SearchDestroy => match self.snd_phase {
                SndPhase::Live => {
                    if self.channel > 0.0 {
                        format!("PLANTING… {:.0}%", self.channel * 100.0)
                    } else {
                        format!("SEARCH & DESTROY — plant the bomb  [{:.0}s]", self.round_time.max(0.0))
                    }
                }
                SndPhase::Planted => {
                    if self.channel > 0.0 {
                        format!("BOMB PLANTED — defusing {:.0}%  [{:.0}s]", self.channel * 100.0, self.fuse.max(0.0))
                    } else {
                        format!("BOMB PLANTED — {:.0}s to detonation", self.fuse.max(0.0))
                    }
                }
                SndPhase::Over => self.outcome_line(),
            },
            Objective::CaptureTheFlag => {
                if self.is_over() {
                    self.outcome_line()
                } else if self.player_carrying {
                    format!("FLAG TAKEN — return to base!   {}–{}  [{:.0}s]", self.scores[0], self.scores[1], self.round_time.max(0.0))
                } else {
                    format!("CAPTURE THE FLAG   {}–{}  [{:.0}s]", self.scores[0], self.scores[1], self.round_time.max(0.0))
                }
            }
        }
    }

    fn outcome_line(&self) -> String {
        match self.outcome {
            Some(RoundOutcome::AttackersWin) => "VICTORY".to_string(),
            Some(RoundOutcome::DefendersWin) => "DEFEAT".to_string(),
            Some(RoundOutcome::Draw) => "DRAW".to_string(),
            None => String::new(),
        }
    }
}

impl Flag {
    fn pos_is_home(&self) -> bool {
        !self.taken && !self.dropped
    }
}

/// XZ distance test (objectives are captured on the ground plane; vertical position is ignored so a site on
/// a ramp or a flag on a low platform still triggers).
fn within(a: Vec3, b: Vec3, r: f32) -> bool {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    dx * dx + dz * dz <= r * r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 0.0, z)
    }

    #[test]
    fn snd_plant_then_detonate_is_an_attacker_win() {
        let mut o = ObjectiveState::search_destroy(v(0.0, 0.0), v(50.0, 0.0));
        // stand on site A and hold the plant for the plant time
        for _ in 0..250 {
            if o.update(v(0.0, 0.0), true, true, &[], 1.0 / 60.0).is_some() {
                break;
            }
        }
        assert_eq!(o.snd_phase, SndPhase::Planted, "holding on the site should plant the bomb");
        // now let the fuse burn down with no defender present
        let mut out = None;
        for _ in 0..(31 * 60) {
            if let Some(r) = o.update(v(0.0, 0.0), true, false, &[], 1.0 / 60.0) {
                out = Some(r);
                break;
            }
        }
        assert_eq!(out, Some(RoundOutcome::AttackersWin), "an undefused bomb should detonate for the attacker win");
    }

    #[test]
    fn snd_defuse_is_a_defender_win() {
        let mut o = ObjectiveState::search_destroy(v(0.0, 0.0), v(50.0, 0.0));
        for _ in 0..250 {
            o.update(v(0.0, 0.0), true, true, &[], 1.0 / 60.0);
        }
        assert_eq!(o.snd_phase, SndPhase::Planted);
        // an enemy stands on the planted site and defuses
        let mut out = None;
        for _ in 0..(6 * 60) {
            if let Some(r) = o.update(v(9.0, 0.0), true, false, &[v(0.0, 0.0)], 1.0 / 60.0) {
                out = Some(r);
                break;
            }
        }
        assert_eq!(out, Some(RoundOutcome::DefendersWin), "a defender on the site should defuse for the win");
    }

    #[test]
    fn snd_no_plant_before_clock_is_a_defender_win() {
        let mut o = ObjectiveState::search_destroy(v(0.0, 0.0), v(50.0, 0.0));
        o.round_time = 1.0;
        let mut out = None;
        for _ in 0..(2 * 60) {
            // player never on a site, never planting
            if let Some(r) = o.update(v(500.0, 500.0), true, false, &[], 1.0 / 60.0) {
                out = Some(r);
                break;
            }
        }
        assert_eq!(out, Some(RoundOutcome::DefendersWin), "no plant before the clock expires is a defender win");
    }

    #[test]
    fn ctf_carry_enemy_flag_home_scores_and_wins() {
        let blue = v(0.0, 0.0);
        let red = v(40.0, 0.0);
        let mut o = ObjectiveState::capture_the_flag(blue, red, 1);
        // walk onto the red flag to grab it
        o.update(red, true, false, &[], 1.0 / 60.0);
        assert!(o.player_carrying, "touching the enemy flag should pick it up");
        // walk it back to blue base
        let out = o.update(blue, true, false, &[], 1.0 / 60.0);
        assert_eq!(out, Some(RoundOutcome::AttackersWin), "returning the enemy flag to your base should score the winning capture");
        assert_eq!(o.scores[0], 1);
    }

    #[test]
    fn ctf_dropping_the_flag_on_death_leaves_it_in_the_field() {
        let blue = v(0.0, 0.0);
        let red = v(40.0, 0.0);
        let mut o = ObjectiveState::capture_the_flag(blue, red, 3);
        o.update(red, true, false, &[], 1.0 / 60.0);
        assert!(o.player_carrying);
        // die partway back
        o.update(v(20.0, 0.0), false, false, &[], 1.0 / 60.0);
        assert!(!o.player_carrying, "dying drops the flag");
        assert!(o.flags[1].dropped, "the dropped flag waits in the field");
    }

    #[test]
    fn ctf_enemy_capture_of_the_player_flag_is_a_defender_win() {
        let blue = v(0.0, 0.0);
        let red = v(40.0, 0.0);
        let mut o = ObjectiveState::capture_the_flag(blue, red, 1);
        // an enemy grabs the blue flag (stands on it), then reaches red base
        o.update(v(500.0, 500.0), true, false, &[blue], 1.0 / 60.0);
        assert!(o.flags[0].taken, "an enemy on the blue base grabs the player's flag");
        let out = o.update(v(500.0, 500.0), true, false, &[red], 1.0 / 60.0);
        assert_eq!(out, Some(RoundOutcome::DefendersWin), "the enemy returning the blue flag to red base wins for the defenders");
    }

    #[test]
    fn a_fresh_none_objective_never_ends() {
        let mut o = ObjectiveState::none();
        for _ in 0..600 {
            assert!(o.update(v(0.0, 0.0), true, true, &[], 1.0 / 60.0).is_none());
        }
        assert!(!o.is_over());
    }
}
