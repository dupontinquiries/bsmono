//! FLEET — ships as party lobbies, and how you get between them.
//!
//! `net.rs` already gives a session a host and peers; `ship.rs` gives each
//! player a ship with rooms; `stations.rs` makes every screen a place with
//! an address. What none of them answer is the social question: when YOUR
//! ship and MY ship are in the same session, what are we to each other?
//! This module is that answer, as a pure state machine — no sockets, no
//! renderer — built the way `adddevice::Flow` was, so the transport can be
//! wired later without the rules moving.
//!
//! **A fleet is ships flying formation, not players in a list.** Members
//! join the PARTY; ships join the FLEET; the two are related but distinct,
//! because a party member without a ship (downed, dropped, or riding along)
//! is still in the party.
//!
//! **Boarding is tiered, and the tier is the rule, not the screen.** The
//! logscope trust work proved this shape twice over (C12-23e/f: a
//! documented predicate with zero callers enforces nothing, and a UI that
//! grants above the line is a live defect). So access is one ordered enum,
//! one predicate — [`Fleet::may_use`] — and every refusal names its reason
//! instead of silently downgrading, because a guest who asked for the helm
//! and quietly got the corridor would believe the wrong thing about the
//! ship they are standing in.
//!
//!   * `Dock`    — hull and airlock vestibule only
//!   * `Deck`    — walk the corridors and rooms (the visitor default)
//!   * `Station` — use consoles, EXCEPT command surfaces
//!   * `Helm`    — command: the terminal, plotting, the helm. Owner only,
//!                 and not grantable — a ship has one captain.
//!
//! The meaningful line is between `Deck` and `Station` — sightseeing
//! versus touching — the same way logscope's line was Build/Compute, and
//! the visitor default sits BELOW it on purpose: walking onto a friend's
//! ship must not mean reading their terminal.
//!
//! **Two ways across, because two kinds of body.** A spacewalk needs a
//! body that survives vacuum — a suited human, a flight chassis, anything
//! space-rated. Everything else — the wheeled, the walkers, the hollow-body
//! vehicles this whole game is about — crosses on the ENERGY BRIDGE: a
//! hard-light span that ASSEMBLES from both ends and meets in the middle,
//! the way a Machine structure builds itself. The bridge borrows
//! `airlock.rs`'s size-class lesson verbatim: a personnel span does not
//! admit a car, so a bridge has a [`Lane`], and a vehicle on a foot lane
//! is refused with the reason, not wedged. Holding a bridge costs the
//! deploying ship energy per tick, and the span COLLAPSES if the ships
//! drift past its reach — which is what fleet formation is for.
//!
//! `--fleet` runs the whole loop narrated, in the `--raid`/`--fold` house
//! pattern: every decision prints with its reason, because a loop you can
//! read is a loop you can steer.

use std::collections::HashMap;

/// Who someone is on the wire. The net layer's peer identity, opaque here.
pub type PeerId = u32;

/// What a body can survive. Decided from the chassis (flight bots and
/// suited humans are space-rated; a muscle car is not), passed in rather
/// than measured here so the caller's chassis knowledge stays theirs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rating {
    /// vacuum-capable: may spacewalk
    SpaceReady,
    /// needs pressure or a surface: may cross only on a bridge
    Grounded,
}

/// Boarding access, ordered. The derive makes `<` mean "less trusted",
/// and the tests pin the inequalities so inserting a tier later cannot
/// silently move the line.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Access {
    Dock,
    Deck,
    Station,
    Helm,
}

/// What a console is, for the gate. Command surfaces want `Helm`;
/// everything else wants `Station`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Console {
    /// the terminal, the helm, plotting — the ship's command
    Command,
    /// gear, market, vats, ledger — the ship's services
    Service,
}

impl Console {
    pub fn needs(self) -> Access {
        match self {
            Console::Command => Access::Helm,
            Console::Service => Access::Station,
        }
    }
}

/// Why something was refused. Every gate returns one of these instead of
/// false, so the caller can SAY it — the rule about refusals explaining
/// themselves is the module's whole personality.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    NotInFleet,
    NotAboard,
    /// (has, needs)
    Access(Access, Access),
    NotSpaceReady,
    OutOfReach,
    NoBridge,
    BridgeBuilding,
    /// the span's lane class does not admit this body
    LaneTooNarrow,
    NotCaptain,
    /// `Helm` is the captaincy itself, not a grant
    HelmNotGrantable,
    LowPower,
}

impl Refusal {
    pub fn say(self) -> &'static str {
        match self {
            Refusal::NotInFleet => "that ship is not flying with us",
            Refusal::NotAboard => "you are not aboard that ship",
            Refusal::Access(_, _) => "the captain has not opened that to you",
            Refusal::NotSpaceReady => "that body will not survive the walk — take the bridge",
            Refusal::OutOfReach => "too far apart — close formation first",
            Refusal::NoBridge => "no span is up between those ships",
            Refusal::BridgeBuilding => "the span is still assembling",
            Refusal::LaneTooNarrow => "a personnel span does not admit a vehicle",
            Refusal::NotCaptain => "only the captain grants access on their own ship",
            Refusal::HelmNotGrantable => "a ship has one captain; the helm is not a grant",
            Refusal::LowPower => "not enough charge to raise the span",
        }
    }
}

/// The span's width class — `airlock.rs`'s size-class lesson, applied to
/// a bridge: what fits is a property of the opening, decided up front.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Lane {
    /// foot traffic: humans and man-sized bots
    Personnel,
    /// full width: cars, tanks, anything on wheels or treads
    Vehicle,
}

/// How far apart two ships can be and still bridge or walk. One constant
/// for both on purpose: formation range IS the social distance, and two
/// numbers would let the bridge and the walk disagree about it.
pub const REACH_M: f32 = 220.0;
/// Energy to raise a span, and to hold it, per second. Vehicle lanes cost
/// double — twice the width, twice the light.
pub const BRIDGE_RAISE: u32 = 40;
pub const BRIDGE_HOLD_PER_S: u32 = 2;
/// The span assembles from BOTH ends at this fraction per second; the two
/// halves meet in the middle, so a full build takes 1 / (2 * rate).
pub const BUILD_RATE: f32 = 0.20;

/// One ship in the fleet.
pub struct Ship {
    pub captain: PeerId,
    pub name: String,
    /// position, for reach checks — the fleet does not integrate motion,
    /// the game does; this is a mirror the caller keeps current
    pub pos: [f32; 3],
    /// stored charge the bridge draws on
    pub charge: u32,
    /// per-visitor grants ABOVE the default. The captain's own access is
    /// not stored: being captain IS `Helm`, so there is no row to desync.
    grants: HashMap<PeerId, Access>,
}

/// A hard-light span between two fleet ships.
pub struct Bridge {
    pub a: usize,
    pub b: usize,
    pub lane: Lane,
    /// 0..1 from EACH end; the span carries traffic when the halves meet
    pub built: f32,
}

impl Bridge {
    pub fn complete(&self) -> bool {
        self.built >= 0.5 - 1e-6
    }
}

/// Where a party member is standing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Where {
    Aboard(usize),
    /// mid-transfer between two ships (walking or crossing)
    Between(usize, usize),
}

pub struct Member {
    pub id: PeerId,
    pub rating: Rating,
    pub at: Where,
}

#[derive(Default)]
pub struct Fleet {
    pub ships: Vec<Ship>,
    pub members: Vec<Member>,
    pub bridges: Vec<Bridge>,
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

impl Fleet {
    /// A ship joins the fleet under its captain, who boards it at `Helm`
    /// by construction.
    pub fn dock_ship(&mut self, captain: PeerId, name: &str, pos: [f32; 3], charge: u32) -> usize {
        let si = self.ships.len();
        self.ships.push(Ship { captain, name: name.into(), pos, charge, grants: HashMap::new() });
        self.members.push(Member { id: captain, rating: Rating::SpaceReady, at: Where::Aboard(si) });
        si
    }

    /// A member joins the party aboard a ship (a rider, a passenger, a
    /// bot in the hold). Visitor default applies unless they captain it.
    pub fn join(&mut self, id: PeerId, rating: Rating, aboard: usize) {
        self.members.push(Member { id, rating, at: Where::Aboard(aboard) });
    }

    fn member(&self, id: PeerId) -> Option<&Member> {
        self.members.iter().find(|m| m.id == id)
    }
    fn member_mut(&mut self, id: PeerId) -> Option<&mut Member> {
        self.members.iter_mut().find(|m| m.id == id)
    }

    /// THE access answer for one peer on one ship. The captain is `Helm`;
    /// everyone else starts at `Deck` — aboard a friend's ship you may
    /// walk, and nothing more, until the captain says so.
    pub fn access(&self, id: PeerId, ship: usize) -> Access {
        let Some(s) = self.ships.get(ship) else { return Access::Dock };
        if s.captain == id {
            return Access::Helm;
        }
        s.grants.get(&id).copied().unwrap_or(Access::Deck)
    }

    /// THE gate. Consoles ask here and nowhere else — the predicate has
    /// callers from the day it exists, which is the entire lesson of
    /// `Trust::permits` shipping documented, tested, and consulted by
    /// nothing.
    pub fn may_use(&self, id: PeerId, ship: usize, console: Console) -> Result<(), Refusal> {
        let m = self.member(id).ok_or(Refusal::NotInFleet)?;
        if m.at != Where::Aboard(ship) {
            return Err(Refusal::NotAboard);
        }
        let has = self.access(id, ship);
        let needs = console.needs();
        if has < needs {
            return Err(Refusal::Access(has, needs));
        }
        Ok(())
    }

    /// The captain opens (or narrows) a visitor's run of the ship. `Helm`
    /// is refused by name: the helm is the captaincy, not a grant.
    pub fn grant(
        &mut self,
        by: PeerId,
        ship: usize,
        to: PeerId,
        level: Access,
    ) -> Result<(), Refusal> {
        let s = self.ships.get_mut(ship).ok_or(Refusal::NotInFleet)?;
        if s.captain != by {
            return Err(Refusal::NotCaptain);
        }
        if level >= Access::Helm {
            return Err(Refusal::HelmNotGrantable);
        }
        s.grants.insert(to, level);
        Ok(())
    }

    /// Step over the void in a suit or a space-rated chassis. Grounded
    /// bodies are refused BY NAME toward the bridge, because "no" with a
    /// door is a different answer from "no".
    pub fn spacewalk(&mut self, id: PeerId, to: usize) -> Result<(), Refusal> {
        let m = self.member(id).ok_or(Refusal::NotInFleet)?;
        let Where::Aboard(from) = m.at else { return Err(Refusal::NotAboard) };
        if self.ships.get(to).is_none() {
            return Err(Refusal::NotInFleet);
        }
        if m.rating != Rating::SpaceReady {
            return Err(Refusal::NotSpaceReady);
        }
        if dist(self.ships[from].pos, self.ships[to].pos) > REACH_M {
            return Err(Refusal::OutOfReach);
        }
        self.member_mut(id).unwrap().at = Where::Aboard(to);
        Ok(())
    }

    /// Raise a span. Costs the DEPLOYING ship its charge — the bridge is
    /// the host's hospitality, so the host pays for it.
    pub fn raise_bridge(
        &mut self,
        by: PeerId,
        from: usize,
        to: usize,
        lane: Lane,
    ) -> Result<usize, Refusal> {
        let s = self.ships.get(from).ok_or(Refusal::NotInFleet)?;
        if s.captain != by {
            return Err(Refusal::NotCaptain);
        }
        if self.ships.get(to).is_none() {
            return Err(Refusal::NotInFleet);
        }
        if dist(self.ships[from].pos, self.ships[to].pos) > REACH_M {
            return Err(Refusal::OutOfReach);
        }
        let cost = BRIDGE_RAISE * if lane == Lane::Vehicle { 2 } else { 1 };
        let s = &mut self.ships[from];
        if s.charge < cost {
            return Err(Refusal::LowPower);
        }
        s.charge -= cost;
        self.bridges.push(Bridge { a: from, b: to, lane, built: 0.0 });
        Ok(self.bridges.len() - 1)
    }

    /// One tick: spans assemble from both ends, holding costs charge, and
    /// a span whose ships drifted apart — or whose host ran dry —
    /// COLLAPSES, dropping anyone mid-crossing back where they started
    /// (the game decides what falling looks like; the fleet only knows
    /// the span is gone).
    pub fn tick(&mut self, dt: f32) {
        let mut fallen: Vec<usize> = Vec::new();
        for (bi, b) in self.bridges.iter_mut().enumerate() {
            let hold =
                (BRIDGE_HOLD_PER_S as f32 * dt).ceil() as u32 * if b.lane == Lane::Vehicle { 2 } else { 1 };
            let apart = dist(self.ships[b.a].pos, self.ships[b.b].pos) > REACH_M;
            let dry = self.ships[b.a].charge < hold;
            if apart || dry {
                fallen.push(bi);
                continue;
            }
            self.ships[b.a].charge -= hold;
            b.built = (b.built + BUILD_RATE * dt).min(0.5);
        }
        for bi in fallen.into_iter().rev() {
            let b = self.bridges.remove(bi);
            for m in &mut self.members {
                if m.at == Where::Between(b.a, b.b) || m.at == Where::Between(b.b, b.a) {
                    let Where::Between(from, _) = m.at else { unreachable!() };
                    m.at = Where::Aboard(from);
                }
            }
        }
    }

    /// Cross on the span. This is the grounded body's whole route between
    /// ships, and it checks everything the spacewalk does EXCEPT the
    /// vacuum — plus the lane, because a car on a catwalk is a refusal,
    /// not a physics experiment.
    pub fn cross(&mut self, id: PeerId, to: usize, vehicle: bool) -> Result<(), Refusal> {
        let m = self.member(id).ok_or(Refusal::NotInFleet)?;
        let Where::Aboard(from) = m.at else { return Err(Refusal::NotAboard) };
        let b = self
            .bridges
            .iter()
            .find(|b| (b.a == from && b.b == to) || (b.a == to && b.b == from))
            .ok_or(Refusal::NoBridge)?;
        if !b.complete() {
            return Err(Refusal::BridgeBuilding);
        }
        if vehicle && b.lane < Lane::Vehicle {
            return Err(Refusal::LaneTooNarrow);
        }
        self.member_mut(id).unwrap().at = Where::Aboard(to);
        Ok(())
    }
}

// --------------------------------------------------------------- sim ---

/// `--fleet` — the loop, narrated. Two ships fleet up; a suited member
/// walks across and finds out what a visitor may and may not touch; a
/// grounded bot needs the span, watches it assemble, and crosses.
pub fn run(_argv: &[String]) {
    let mut f = Fleet::default();
    let a = f.dock_ship(1, "WIDESKY", [0.0, 0.0, 0.0], 500);
    let b = f.dock_ship(2, "BOARTAIL", [150.0, 0.0, 0.0], 200);
    f.join(3, Rating::SpaceReady, a); // a suited crewmate
    f.join(4, Rating::Grounded, a); // a wheeled bot, not space-rated
    println!("== two ships in formation, {:.0} m apart", dist(f.ships[a].pos, f.ships[b].pos));

    println!("\n-- peer 3 spacewalks WIDESKY -> BOARTAIL");
    match f.spacewalk(3, b) {
        Ok(()) => println!("   across. access there: {:?} (the visitor default)", f.access(3, b)),
        Err(r) => println!("   refused: {}", r.say()),
    }
    println!("-- peer 3 tries BOARTAIL's terminal");
    match f.may_use(3, b, Console::Command) {
        Ok(()) => println!("   ...the gate is broken if you can read this"),
        Err(r) => println!("   refused: {}  ({:?})", r.say(), r),
    }
    println!("-- BOARTAIL's captain grants Station; peer 3 tries the market, then the helm");
    f.grant(2, b, 3, Access::Station).unwrap();
    match f.may_use(3, b, Console::Service) {
        Ok(()) => println!("   market: open"),
        Err(r) => println!("   market refused: {}", r.say()),
    }
    match f.may_use(3, b, Console::Command) {
        Ok(()) => println!("   ...the gate is broken if you can read this"),
        Err(r) => println!("   helm still refused: {}", r.say()),
    }
    println!("-- and the captain tries to grant the helm itself");
    match f.grant(2, b, 3, Access::Helm) {
        Ok(()) => println!("   ...granted, which would be the defect"),
        Err(r) => println!("   refused: {}", r.say()),
    }

    println!("\n-- peer 4 (grounded) tries the spacewalk");
    match f.spacewalk(4, b) {
        Ok(()) => println!("   ...vacuum does not care that this compiled"),
        Err(r) => println!("   refused: {}", r.say()),
    }
    println!("-- WIDESKY raises a VEHICLE span (charge {})", f.ships[a].charge);
    let _ = f.raise_bridge(1, a, b, Lane::Vehicle).unwrap();
    println!("   raised; charge now {}. assembling from both ends:", f.ships[a].charge);
    let mut t = 0.0;
    while !f.bridges[0].complete() {
        f.tick(0.5);
        t += 0.5;
        let each = f.bridges.first().map(|b| b.built).unwrap_or(0.0);
        println!("   t={t:.1}s  each end {:.0}%  span {:.0}%", each * 200.0, each * 200.0);
        if f.bridges.is_empty() {
            println!("   the span collapsed mid-build");
            return;
        }
    }
    println!("-- peer 4 crosses");
    match f.cross(4, b, true) {
        Ok(()) => println!("   across, on wheels. access aboard: {:?}", f.access(4, b)),
        Err(r) => println!("   refused: {}", r.say()),
    }
    println!("-- BOARTAIL breaks formation to 300 m; one tick later:");
    f.ships[b].pos = [300.0, 0.0, 0.0];
    f.tick(0.5);
    println!(
        "   spans standing: {} (a span outlives neither its reach nor its host's charge)",
        f.bridges.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_ships() -> (Fleet, usize, usize) {
        let mut f = Fleet::default();
        let a = f.dock_ship(1, "A", [0.0; 3], 500);
        let b = f.dock_ship(2, "B", [100.0, 0.0, 0.0], 500);
        (f, a, b)
    }

    #[test]
    fn the_visitor_default_sits_below_the_terminal_line() {
        // pinned as inequalities, not equalities, so inserting a tier
        // later cannot silently move the line — logscope's 23e pin shape
        assert!(Access::Deck < Console::Command.needs());
        assert!(Access::Deck < Console::Service.needs());
        assert!(Access::Station >= Console::Service.needs());
        assert!(Access::Station < Console::Command.needs());
    }

    #[test]
    fn a_visitor_walks_the_deck_and_cannot_read_the_terminal() {
        let (mut f, a, b) = two_ships();
        f.join(3, Rating::SpaceReady, a);
        f.spacewalk(3, b).unwrap();
        assert_eq!(f.access(3, b), Access::Deck);
        let r = f.may_use(3, b, Console::Command);
        assert_eq!(r, Err(Refusal::Access(Access::Deck, Access::Helm)));
        // and the refusal explains itself
        assert!(!r.unwrap_err().say().is_empty());
    }

    #[test]
    fn a_grant_opens_services_and_never_the_helm() {
        let (mut f, a, b) = two_ships();
        f.join(3, Rating::SpaceReady, a);
        f.spacewalk(3, b).unwrap();
        assert_eq!(f.grant(3, b, 3, Access::Station), Err(Refusal::NotCaptain));
        f.grant(2, b, 3, Access::Station).unwrap();
        assert!(f.may_use(3, b, Console::Service).is_ok());
        assert!(f.may_use(3, b, Console::Command).is_err());
        assert_eq!(f.grant(2, b, 3, Access::Helm), Err(Refusal::HelmNotGrantable));
    }

    #[test]
    fn vacuum_refuses_the_grounded_and_names_the_bridge() {
        let (mut f, a, b) = two_ships();
        f.join(4, Rating::Grounded, a);
        assert_eq!(f.spacewalk(4, b), Err(Refusal::NotSpaceReady));
        assert!(Refusal::NotSpaceReady.say().contains("bridge"));
    }

    #[test]
    fn the_span_assembles_from_both_ends_and_carries_by_lane() {
        let (mut f, a, b) = two_ships();
        f.join(4, Rating::Grounded, a);
        let _ = f.raise_bridge(1, a, b, Lane::Personnel).unwrap();
        assert_eq!(f.cross(4, b, false), Err(Refusal::BridgeBuilding));
        // both directions pinned at the real boundary: 0.20/s per end
        // meeting at 0.5 means 2.5 s — not complete at 2.0, complete by 2.5
        f.tick(2.0);
        assert!(!f.bridges[0].complete(), "met before both halves could");
        f.tick(0.5);
        assert!(f.bridges[0].complete(), "halves failed to meet");
        // a vehicle on a personnel span is a refusal, not a crossing
        assert_eq!(f.cross(4, b, true), Err(Refusal::LaneTooNarrow));
        f.cross(4, b, false).unwrap();
        assert_eq!(f.member(4).unwrap().at, Where::Aboard(b));
    }

    #[test]
    fn a_span_collapses_out_of_reach_and_when_the_host_runs_dry() {
        let (mut f, a, b) = two_ships();
        f.raise_bridge(1, a, b, Lane::Personnel).unwrap();
        f.ships[b].pos = [REACH_M + 50.0, 0.0, 0.0];
        f.tick(0.5);
        assert!(f.bridges.is_empty(), "outlived its reach");
        // dry host: raise again in reach, then drain the charge
        f.ships[b].pos = [100.0, 0.0, 0.0];
        f.raise_bridge(1, a, b, Lane::Personnel).unwrap();
        f.ships[a].charge = 0;
        f.tick(0.5);
        assert!(f.bridges.is_empty(), "outlived its host's charge");
    }

    #[test]
    fn raising_costs_and_low_power_refuses() {
        let (mut f, a, b) = two_ships();
        f.ships[a].charge = BRIDGE_RAISE * 2 - 1; // one short of a vehicle span
        assert_eq!(f.raise_bridge(1, a, b, Lane::Vehicle), Err(Refusal::LowPower));
        assert!(f.raise_bridge(1, a, b, Lane::Personnel).is_ok());
    }

    #[test]
    fn only_the_captain_raises_from_their_own_ship() {
        let (mut f, a, b) = two_ships();
        f.join(3, Rating::SpaceReady, a);
        assert_eq!(f.raise_bridge(3, a, b, Lane::Personnel), Err(Refusal::NotCaptain));
    }
}
