//! ESCALATION — the War for Cybertron horde mode, as pure logic.
//!
//! Ported from the C engine's `exengine/waves.h`/`waves.c`, which is
//! itself already a "pure logic" module by design — its own header
//! comment: "Pure logic; the game supplies enemy spawns and reports
//! kills." That boundary is kept exactly here. This module answers
//! "what wave is it, what does it cost, what did clearing it earn" —
//! it does not spawn anything, place anything in a world, or resolve
//! a single shot. Turning that into a closed arena you actually fight
//! in needs real 3D enemy entities (position, a body, an AI that
//! moves and shoots), which this codebase does not have anywhere yet
//! for PvE combat — checked: `brain.rs` is real squad-AI *decision*
//! logic wired only to a CLI diagnostic (`--brain`), `skirmish.rs` is
//! an abstract 1-D combat resolver for campaign/raid OUTCOMES, and
//! `raidflow.rs`'s own doc comment says plainly "no troops walk the
//! arena yet." Porting this core keeps faith with the person's own
//! read of the situation ("I think it's mostly just a raid
//! derivation") for the HALF of it that actually is a small, provably
//! correct port — the spawn/AI/arena-generation half is a distinct,
//! much larger foundation this does not attempt to guess at blind.
//!
//! **Escalation's own economy, ported alongside the wave counter**:
//! kills bank energon credits (by enemy kind); on a wave clear the
//! whole bank FORGES into a permanent integrity increase, so playing
//! greedy — banking more before the wave ends — compounds into a
//! stronger run. `spend` is the buy side (perks, gates), same as the
//! research doc's own read of what fans praised: "kills earn credits;
//! spend on weapons/health/ammo/overshields, and BUY-GATES that open
//! new map areas."
//!
//! Every wave-composition/economy number below is the C engine's own,
//! not re-derived or re-tuned — this is a port, not a redesign. The
//! person's own additions (cloakers, a unique "Jhiaxus horror" boss,
//! AI-generated mystery rounds) are real, interesting scope but are
//! NOT in the C reference and are not guessed at here; they belong on
//! top of this core once the arena/spawn foundation exists to hang
//! them on.

/// Which kind of enemy died, for scoring/economy — matches the C
/// engine's own `enemy.h` kind order (0 grunt, 1 corrupted, 2 hijack,
/// 3 apex) exactly, since `ex_waves_on_kill`/`energon_value`'s lookup
/// tables are indexed by that order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EnemyKind {
    Grunt,
    Corrupted,
    Hijack,
    Apex,
}

impl EnemyKind {
    fn index(self) -> usize {
        match self {
            EnemyKind::Grunt => 0,
            EnemyKind::Corrupted => 1,
            EnemyKind::Hijack => 2,
            EnemyKind::Apex => 3,
        }
    }
}

/// A wave's enemy composition — how many of each kind to spawn, and
/// whether this is a TITAN wave (every 5th: a titan-class apex boss
/// joins, and the regular makeup is thinned so the boss always gets a
/// guaranteed spawn slot within a 12-enemy encounter budget).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaveMakeup {
    pub grunts: i32,
    pub corrupted: i32,
    pub hijacks: i32,
    pub titan: bool,
}

/// Enemy composition for a given wave (1-based; escalating). Pure
/// function of the wave number, same as the C original.
pub fn makeup(wave: i32) -> WaveMakeup {
    let wave = wave.max(1);
    let mut m = WaveMakeup {
        // escalate: more grunts each wave; corrupted from wave 2; hijacks from wave 3
        grunts: 2 + wave,
        corrupted: if wave >= 2 { wave / 2 } else { 0 },
        hijacks: if wave >= 3 { wave - 1 } else { 0 },
        titan: wave % 5 == 0,
    };
    // capped by encounter budget
    m.grunts = m.grunts.min(6);
    m.corrupted = m.corrupted.min(3);
    m.hijacks = m.hijacks.min(5);
    if m.titan {
        m.grunts = m.grunts.min(4);
        m.corrupted = m.corrupted.min(2);
        m.hijacks = m.hijacks.min(3);
    }
    m
}

/// Energon credits banked for a kill of this kind: grunt 20 / hijack
/// 30 / corrupted 40 / apex 120 — the C engine's own numbers, not
/// re-derived.
pub fn energon_value(kind: EnemyKind) -> i64 {
    const EV: [i64; 4] = [20, 40, 30, 120];
    EV[kind.index()]
}

/// Score for a kill of this kind: grunt 100 / corrupted 250 / hijack
/// 60 / apex 2000.
fn score_value(kind: EnemyKind) -> i64 {
    const PTS: [i64; 4] = [100, 250, 60, 2000];
    PTS[kind.index()]
}

/// Turns a match into a game loop: successive waves of escalating
/// enemy makeups, a running score, and a wave counter. Between waves
/// there is a short breather; when a wave is cleared the next spawns
/// bigger. Endless (`total_waves == 0`, survival) or a fixed number
/// of waves. The caller supplies enemy spawns and reports kills
/// (`on_kill`) and the current alive count (`update`) — nothing here
/// knows what an enemy IS.
#[derive(Clone, Copy, Debug)]
pub struct Waves {
    /// current wave (1-based); 0 = not started
    pub wave: i32,
    /// 0 = endless
    pub total_waves: i32,
    /// seconds until the next wave spawns (>0 = breather)
    pub intermission: f32,
    /// set the frame the caller should spawn the next wave
    pub pending_spawn: bool,
    pub score: i64,
    pub kills: i32,
    /// all waves cleared (win) — never true in endless mode
    pub finished: bool,
    /// currently banked credits (spent on forge or `spend`)
    pub energon: i64,
    /// lifetime forged credits, display only
    pub forged_total: i64,
    /// set the frame a wave-clear forge fires
    pub forge_event: bool,
    /// permanent max-integrity granted this forge event
    pub forge_hp_amount: f32,
}

impl Waves {
    /// `total_waves = 0` is endless (survival); otherwise a fixed run.
    /// Wave 1 is ready to spawn immediately.
    pub fn new(total_waves: i32) -> Waves {
        Waves {
            wave: 0,
            total_waves,
            intermission: 0.0,
            pending_spawn: true,
            score: 0,
            kills: 0,
            finished: false,
            energon: 0,
            forged_total: 0,
            forge_event: false,
            forge_hp_amount: 0.0,
        }
    }

    /// Advance to the next wave; returns the makeup to spawn, or
    /// `None` if there are no more waves (a fixed run just finished).
    pub fn begin_next(&mut self) -> Option<WaveMakeup> {
        if self.total_waves > 0 && self.wave >= self.total_waves {
            self.finished = true;
            return None;
        }
        self.wave += 1;
        self.pending_spawn = false;
        self.intermission = 0.0;
        Some(makeup(self.wave))
    }

    /// Award score for a kill of the given kind, and bank energon
    /// credits toward the next forge.
    pub fn on_kill(&mut self, kind: EnemyKind) {
        self.score += score_value(kind);
        self.kills += 1;
        self.energon += energon_value(kind);
    }

    /// Tick the intermission; when a wave is cleared (`alive == 0`
    /// and a wave is active) start the breather, flag
    /// `pending_spawn`, and fire the forge (banked energon becomes
    /// `forge_hp_amount`, `forge_event = true`, `energon` resets to
    /// 0, `forged_total` accumulates). `forge_event`/`forge_hp_amount`
    /// are cleared at the start of every call, same as the C original
    /// — they are a one-frame signal, not a sticky flag.
    pub fn update(&mut self, alive: i32, dt: f32) {
        self.forge_event = false;
        self.forge_hp_amount = 0.0;
        if self.finished {
            return;
        }
        if self.intermission > 0.0 {
            self.intermission -= dt;
            if self.intermission <= 0.0 {
                self.intermission = 0.0;
                self.pending_spawn = true;
            }
            return;
        }
        // a wave is active and everything's dead -> clear bonus + breather
        if self.wave >= 1 && alive == 0 && !self.pending_spawn {
            self.score += 500 * self.wave as i64; // wave-clear bonus grows
            if self.energon > 0 {
                self.forge_hp_amount = self.energon as f32 * 0.4;
                self.forged_total += self.energon;
                self.energon = 0;
                self.forge_event = true;
            }
            if self.total_waves > 0 && self.wave >= self.total_waves {
                self.finished = true;
                return;
            }
            self.intermission = 3.0; // breather before the next wave
        }
    }

    /// Try to spend `cost` banked energon on a mid-run purchase
    /// (perks, gates). Returns whether it succeeded; deducts on
    /// success, leaves the bank unchanged on failure.
    pub fn spend(&mut self, cost: i64) -> bool {
        if cost < 0 || self.energon < cost {
            return false;
        }
        self.energon -= cost;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported directly from games/blackstar/tests/test_waves.c, case for
    // case, so this checks the port against the reference rather than
    // against its own idea of what the reference does.

    #[test]
    fn escalation_makeup_matches_the_c_reference() {
        let (m1, m3, m6) = (makeup(1), makeup(3), makeup(6));
        assert!(m1.corrupted == 0 && !m1.hijacks_present(), "wave 1 should be grunts only: {m1:?}");
        assert!(m3.corrupted > 0 && m3.hijacks > 0, "later waves should add variety: {m3:?}");
        assert!(m6.grunts >= m1.grunts, "grunts should escalate: {m1:?} -> {m6:?}");
        assert!(
            m6.grunts <= 6 && m6.corrupted <= 3 && m6.hijacks <= 5,
            "encounter budget cap violated: {m6:?}"
        );
    }

    #[test]
    fn progression_over_a_five_wave_run() {
        let mut w = Waves::new(5);
        assert!(w.pending_spawn, "wave 1 should be ready immediately");
        let got = w.begin_next();
        assert!(got.is_some() && w.wave == 1 && !w.pending_spawn);

        let s0 = w.score;
        w.on_kill(EnemyKind::Grunt); // +100
        w.on_kill(EnemyKind::Corrupted); // +250
        w.on_kill(EnemyKind::Apex); // +2000
        assert_eq!(w.score, s0 + 2350);
        assert_eq!(w.kills, 3);

        assert_eq!(energon_value(EnemyKind::Grunt), 20);
        assert_eq!(energon_value(EnemyKind::Corrupted), 40);
        assert_eq!(energon_value(EnemyKind::Hijack), 30);
        assert_eq!(energon_value(EnemyKind::Apex), 120);
        assert_eq!(w.energon, 20 + 40 + 120, "banked from the 3 kills above");
        let banked = w.energon;

        // clear the wave -> breather + forge
        let before = w.score;
        w.update(0, 0.016);
        assert!(w.score > before, "wave-clear bonus did not fire");
        assert!(w.intermission > 0.0 && !w.pending_spawn);
        assert!(w.forge_event);
        assert_eq!(w.forge_hp_amount, banked as f32 * 0.4);
        assert_eq!(w.energon, 0);
        assert_eq!(w.forged_total, banked);

        // forge_event is one-shot: clears next tick with nothing new banked
        w.update(0, 0.016);
        assert!(!w.forge_event);

        // wait out the breather
        for _ in 0..(60 * 4) {
            if w.pending_spawn {
                break;
            }
            w.update(0, 1.0 / 60.0);
        }
        assert!(w.pending_spawn);
        w.begin_next();
        assert_eq!(w.wave, 2);
    }

    #[test]
    fn a_fixed_run_finishes_and_endless_never_does() {
        let mut w = Waves::new(5);
        w.begin_next();
        let mut guard = 0;
        while !w.finished {
            w.update(0, 0.016);
            if w.pending_spawn && w.begin_next().is_none() {
                break;
            }
            for _ in 0..(60 * 4) {
                if w.intermission <= 0.0 {
                    break;
                }
                w.update(0, 1.0 / 60.0);
            }
            guard += 1;
            assert!(guard < 10_000, "progression loop did not terminate");
        }
        assert!(w.finished && w.wave == 5);

        let mut e = Waves::new(0);
        e.begin_next();
        for _ in 0..20 {
            e.update(0, 0.016);
            for _ in 0..300 {
                if e.intermission <= 0.0 {
                    break;
                }
                e.update(0, 1.0 / 60.0);
            }
            if e.pending_spawn {
                e.begin_next();
            }
        }
        assert!(!e.finished && e.wave > 5, "endless mode should keep escalating: wave {}", e.wave);
    }

    #[test]
    fn titan_waves_land_every_fifth_wave_within_budget() {
        for wv in 1..=10 {
            let m = makeup(wv);
            assert_eq!(m.titan, wv % 5 == 0, "wave {wv}: {m:?}");
            if m.titan {
                assert!(
                    m.grunts + m.corrupted + m.hijacks + 1 <= 12,
                    "titan wave {wv} does not leave room for the boss: {m:?}"
                );
            }
        }
    }

    #[test]
    fn spend_only_succeeds_when_affordable() {
        let mut s = Waves::new(0);
        s.energon = 200;
        assert!(!s.spend(250) && s.energon == 200, "spent more than banked");
        assert!(s.spend(150) && s.energon == 50, "affordable spend was refused");
        assert!(s.spend(50) && s.energon == 0, "exact remainder spend was refused");
    }

    // small helper the ported tests above use for a slightly more
    // readable assertion than `m1.hijacks == 0`
    impl WaveMakeup {
        fn hijacks_present(&self) -> bool {
            self.hijacks > 0
        }
    }
}
