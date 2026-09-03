//! TRADECRAFT — how what you found gets where it has to go.
//!
//! The Transformer campaign has a ship, and the ship has a chartroom: you
//! carry an artifact home and put it under a lamp. The human campaign was
//! going to get the same thing with a different address, and the source says
//! not to. The scene after the warehouse is explicit — *old car, three people
//! inside, no secret base, no holograms* — and it is the scene the whole team
//! is built out of. A safehouse with a wall of monitors would take it away.
//!
//! So the human side has no home base. It has a CASE you carry, a car you
//! stage out of, and a scattered set of ways to get information to whoever
//! you answer to. That is period-correct for 1998–2005 — payphones, dead
//! drops, a contact who works a newsstand — and it turns out to be a better
//! mechanic than a base, for one reason:
//!
//! ## Finding stops being free
//!
//! In the ship campaign, scanning something puts it on the map. Here, finding
//! it only puts it in the CASE. Nothing reaches the archaeological map until
//! it is RELAYED, and every relay costs something:
//!
//! * **reach** — how much you can push through it at once. A payphone call is
//!   a sentence; a hideout is an afternoon with a photocopier.
//! * **exposure** — who else hears. Morse flashed off a rooftop reaches your
//!   own people and anyone with line of sight and patience.
//! * **latency** — a dead drop is serviced when it is serviced.
//!
//! Which makes the case a real inventory decision instead of a bag: you are
//! carrying evidence that is not safe, and the map does not know it yet. Get
//! caught and it is gone. That is the Hitman half of the register — the
//! problem is rarely the shooting, it is walking out with the thing.
//!
//! ## Exposure is not a penalty, it is a plot
//!
//! A high-exposure relay tells rival societies what you found. That is how the
//! official record gets written by somebody who was not there — and this
//! project already has the payoff built: `facility_built_2003` is a claim the
//! Directorate ASSERTED, contradicted by three dig dates. Exposure is the
//! mechanism by which such records come to exist. Reporting is a move in the
//! argument, not a save button.
//!
//! ## What this module does and does not do
//!
//! It models the relay network, the case, and what a report costs. It does not
//! decide the fiction of who betrays whom, and it does not simulate being
//! caught — losing a case is a mission outcome, and the mission layer owns it.

use crate::groundside::Era;

/// Who you answer to. The three philosophies plus the people who have not
/// picked one, which early on is the protagonists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Society {
    Directorate,
    FreeState,
    Covenant,
    Unaligned,
}

impl Society {
    pub fn all() -> [Society; 4] {
        [Society::Directorate, Society::FreeState, Society::Covenant, Society::Unaligned]
    }
    pub fn id(self) -> &'static str {
        match self {
            Society::Directorate => "directorate",
            Society::FreeState => "free_state",
            Society::Covenant => "covenant",
            Society::Unaligned => "unaligned",
        }
    }
    /// How this society's people fight, as a `content::COMMANDERS` id.
    ///
    /// The commander archetypes needed NO translation, which is the whole
    /// finding. Four dials — aggression, caution, encircle, regroup —
    /// describe how an armed group behaves under pressure, and nothing in
    /// them is Cybertronian. A Directorate strike team surrounds you for the
    /// same reason a Sundered squad does: numbers and a doctrine that says
    /// close the ring.
    ///
    /// So the human campaign gets `skirmish.rs` for free, and the mapping is
    /// an argument about each society rather than an engineering problem:
    ///
    /// * the Directorate has numbers and legitimacy, and wants you contained
    ///   rather than dead — it encircles;
    /// * the Free State has neither, so it trades and leaves — it skirmishes;
    /// * the Covenant does not want a fight at all, it wants the discovery to
    ///   stop, so it holds ground and makes every metre cost something;
    /// * unaligned is the trio, who are three people and press when they have
    ///   to because they cannot afford a long engagement.
    pub fn doctrine(self) -> &'static str {
        match self {
            Society::Directorate => "surrounder",
            Society::FreeState => "skirmisher",
            Society::Covenant => "warden",
            Society::Unaligned => "aggressor",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Society::Directorate => "THE DIRECTORATE",
            Society::FreeState => "THE FREE STATE",
            Society::Covenant => "THE COVENANT",
            Society::Unaligned => "NOBODY YET",
        }
    }
}

/// How a relay works, which is mostly a statement about the era.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    /// A car with a notebook in the door pocket. Stages, does not transmit —
    /// the one "base" the human campaign gets, and it moves.
    Car,
    /// A call. Short, quiet, needs a specific box that still works.
    Payphone,
    /// Left somewhere, collected later. No exposure and no hurry.
    DeadDrop,
    /// A person. Lossy, because they retell it in their own words.
    Contact,
    /// A lamp off a rooftop. Enormous reach, and anyone with line of sight
    /// and patience is also receiving.
    Heliograph,
    /// Somebody's room with a photocopier. Everything at once, if you can
    /// get to it and if it is still yours.
    Hideout,
}

impl Channel {
    /// How many case items one use can carry.
    pub fn reach(self) -> usize {
        match self {
            Channel::Car => 0, // stages only; nothing leaves the car
            Channel::Payphone => 1,
            Channel::Contact => 1,
            Channel::DeadDrop => 3,
            Channel::Heliograph => 2,
            Channel::Hideout => 8,
        }
    }
    /// Who else hears, 0..3.
    pub fn exposure(self) -> u8 {
        match self {
            Channel::Car | Channel::DeadDrop => 0,
            Channel::Hideout => 1,
            Channel::Payphone => 1,
            Channel::Contact => 2,
            Channel::Heliograph => 3,
        }
    }
    /// Whether what arrives is exactly what you sent. A person retelling a
    /// serial number is not the same as a photocopy of it.
    pub fn faithful(self) -> bool {
        !matches!(self, Channel::Contact)
    }
    pub fn name(self) -> &'static str {
        match self {
            Channel::Car => "THE CAR",
            Channel::Payphone => "A PAYPHONE",
            Channel::DeadDrop => "A DEAD DROP",
            Channel::Contact => "A CONTACT",
            Channel::Heliograph => "A ROOFTOP LAMP",
            Channel::Hideout => "A HIDEOUT",
        }
    }
}

/// One place you can get something out from.
pub struct Relay {
    pub id: &'static str,
    pub channel: Channel,
    /// a `groundside::PLACES` id, or None for the car, which is wherever you
    /// left it
    pub at: Option<&'static str>,
    /// whose relay this is. `Unaligned` means anyone can use it.
    pub society: Society,
    pub blurb: &'static str,
}

pub static RELAYS: &[Relay] = &[
    Relay {
        id: "the_car",
        channel: Channel::Car,
        at: None,
        society: Society::Unaligned,
        blurb: "an old car with a notebook in the door pocket. it is not a base and \
                it does not transmit. it is where the three of you sit while nobody \
                completely trusts anyone yet, and that is the whole of its function.",
    },
    Relay {
        id: "port_payphone",
        channel: Channel::Payphone,
        at: Some("the_port"),
        society: Society::Unaligned,
        blurb: "the box by the freight gate, which works because the port pays for \
                the line and nobody has audited it since the eighties.",
    },
    Relay {
        id: "museum_drop",
        channel: Channel::DeadDrop,
        at: Some("the_museum"),
        society: Society::Directorate,
        blurb: "behind the radiator in the second-floor gallery. serviced on \
                Thursdays, or whenever somebody remembers.",
    },
    Relay {
        id: "newsstand_contact",
        channel: Channel::Contact,
        at: Some("the_bunker"),
        society: Society::FreeState,
        blurb: "he has worked this corner for nineteen years and knows what a \
                government car sounds like. he will pass it on. he will also pass \
                on his own opinion of it, which is the problem.",
    },
    Relay {
        id: "ridge_lamp",
        channel: Channel::Heliograph,
        at: Some("the_ridge"),
        society: Society::Unaligned,
        blurb: "the high ground above the water, and a lamp. it reaches everyone \
                who matters. it also reaches everyone who does not, which is how \
                the other version of events gets written before yours arrives.",
    },
    Relay {
        id: "observatory_room",
        channel: Channel::Hideout,
        at: Some("the_observatory"),
        society: Society::Covenant,
        blurb: "a room under the dome with a photocopier and a door that locks from \
                inside. everything at once, if it is still yours when you get there.",
    },
];

pub fn relay(id: &str) -> Option<&'static Relay> {
    RELAYS.iter().find(|r| r.id == id)
}

/// Relays usable at a place.
pub fn relays_at(place_id: &str) -> Vec<&'static Relay> {
    RELAYS.iter().filter(|r| r.at == Some(place_id)).collect()
}

/// What you are carrying and have not reported.
///
/// Deliberately NOT a `Dig`. The dig is what the map knows; this is what you
/// know and the map does not, and the gap between them is the mechanic.
#[derive(Clone, Default, Debug)]
pub struct Case {
    held: Vec<String>,
}

/// How much fits, by default. Small on purpose — the pressure to relay before
/// the case is full IS the loop, and a roomy case removes it. Six is a guess
/// at that pressure and nothing more, so it is `cfg case_cap` too.
pub const CASE_CAP: usize = 6;

impl Case {
    /// Lenient in, canonical out — same contract as the rest of `save.rs`.
    /// An id this build no longer knows is dropped rather than kept as a
    /// ghost that can never be relayed.
    pub fn from_line(line: &str, findings: &'static [crate::provenance::Finding]) -> Case {
        let mut c = Case::default();
        for id in line.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if findings.iter().any(|f| f.id == id) && !c.held.iter().any(|h| h == id) {
                c.held.push(id.to_string());
            }
        }
        c
    }

    /// Carry order is preserved: it decides what a narrow channel sends.
    pub fn line(&self) -> String {
        self.held.join(",")
    }

    /// Take, at whatever capacity the config says.
    ///
    /// There is no fixed-capacity `take`: it existed, delegated straight to
    /// this, and nothing called it once the driver started reading
    /// `cfg case_cap`. Two doors to one room where one of them is never the
    /// right one.
    pub fn take_capped(&mut self, finding_id: &str, cap: usize) -> Result<(), &'static str> {
        if self.held.iter().any(|h| h == finding_id) {
            return Err("already in the case");
        }
        if self.held.len() >= cap.max(1) {
            return Err("the case is full; get something out first");
        }
        self.held.push(finding_id.to_string());
        Ok(())
    }
    pub fn held(&self) -> &[String] {
        &self.held
    }
    pub fn len(&self) -> usize {
        self.held.len()
    }
}

/// What a report actually did.
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// reached the map, and is now evidence
    pub landed: Vec<String>,
    /// still in the case, because the channel was too narrow
    pub kept: Vec<String>,
    /// arrived, but garbled — a contact retelling it in their own words
    pub degraded: bool,
    /// who else now knows, beyond the intended recipient
    pub overheard_by: Vec<Society>,
}

/// Push what fits through a relay.
///
/// Order matters and is the player's: the front of the case goes first, so
/// deciding what to report through a narrow channel is a real choice rather
/// than an automatic one.
pub fn report(case: &mut Case, r: &Relay) -> Report {
    let n = r.channel.reach().min(case.held.len());
    let landed: Vec<String> = case.held.drain(..n).collect();
    let overheard_by = if r.channel.exposure() >= 2 {
        // loud channels are heard by everyone who is not the recipient
        Society::all().into_iter().filter(|s| *s != r.society && *s != Society::Unaligned).collect()
    } else {
        Vec::new()
    };
    Report {
        landed,
        kept: case.held.clone(),
        degraded: !r.channel.faithful(),
        overheard_by,
    }
}

/// Is this relay usable in this era? The car and the street exist in both;
/// a rooftop lamp is not how anybody signals in the Blackstar present.
pub fn available(r: &Relay, era: Era) -> bool {
    match era {
        Era::Archive => true,
        // the present has the ship. tradecraft is the era without one.
        Era::Present => matches!(r.channel, Channel::Car | Channel::DeadDrop),
    }
}

// ------------------------------------------------------- getting inside ---
//
// The raid system asks a question the detective campaign asks too: two ways
// in, trading against each other, judged against a clock. `foothold.rs` asks
// it in Cybertronian — weather windows, habitability, a ship that has to
// stay grounded. None of those nouns exist in 1998 Los Angeles, but the
// question survives the translation intact, so the arithmetic is shared
// (`approach.rs`) and only the domain is rewritten here.
//
// The translation, term for term:
//
//   weather window       -> how long before somebody else arrives
//   habitability         -> nothing; a person in LA is already survivable
//   shelter from a front -> somewhere you are not visible
//   a grounded ship      -> a car parked where it can be seen
//
// The last one is the nicest, because the car is already the campaign's only
// fixed point and parking it badly costing you for the rest of the job is
// exactly the kind of decision the scene it comes from is about.

/// A way into a building.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WayIn {
    /// Straight in. Fast, and you are visible the whole time.
    FrontDoor,
    /// Longer, and you are out of sight the moment the door shuts.
    ServiceEntrance,
    /// Slowest by far, needs the car left somewhere awkward, and nobody is
    /// ever looking at it.
    Roof,
}

impl WayIn {
    pub fn all() -> [WayIn; 3] {
        [WayIn::FrontDoor, WayIn::ServiceEntrance, WayIn::Roof]
    }
    pub fn label(self) -> &'static str {
        match self {
            WayIn::FrontDoor => "THE FRONT DOOR",
            WayIn::ServiceEntrance => "THE SERVICE ENTRANCE",
            WayIn::Roof => "THE ROOF",
        }
    }
    /// Seconds to get in.
    pub fn take_s(self) -> f32 {
        match self {
            WayIn::FrontDoor => 20.0,
            WayIn::ServiceEntrance => 150.0,
            WayIn::Roof => 340.0,
        }
    }
    /// Are you in view for the whole of it? The front door is the LZ of this
    /// campaign: quick, and exposed from first step to last.
    pub fn exposed_throughout(self) -> bool {
        self == WayIn::FrontDoor
    }
    /// Once you are in, are you out of sight?
    ///
    /// ALWAYS, and getting this wrong cost me an afternoon. I first modelled
    /// the front door as "does not shelter you", which made the shared core
    /// reject it outright whenever a clock was running — so the fast, loud
    /// option could never be chosen and the trade the whole module exists for
    /// did not exist.
    ///
    /// The error was putting one cost in two places. Once you are through ANY
    /// door you are inside a building and out of view; what differs is how
    /// much of the entry happened in the open (`exposed_throughout`) and what
    /// being noticed there costs (`seen_cost`). Shelter is about where you
    /// end up, not how you got there.
    pub fn unseen_after(self) -> bool {
        true
    }
    /// Does it leave the car somewhere it can be noticed?
    pub fn parks_badly(self) -> bool {
        self == WayIn::Roof
    }
    /// How exposed this way in is: how far from a watcher, and how much of
    /// you they can see.
    ///
    /// This replaced three hard-coded `seen_cost` numbers — 300, 40, 0 —
    /// which were a model's output written down as if it were an input. The
    /// costs are now DERIVED (see `perception::seen_cost`), so they move when
    /// the situation does: a service entrance forty metres from a bored guard
    /// is not the same door as one ten metres from a sentry, and the constant
    /// could not tell you that.
    pub fn exposure(self) -> (f32, crate::perception::Cover) {
        use crate::perception::Cover;
        match self {
            // straight past whoever is looking at the entrance
            WayIn::FrontDoor => (8.0, Cover::OPEN),
            // round the side, some of it behind the building
            WayIn::ServiceEntrance => (35.0, Cover::PARTIAL),
            // nobody looks up
            WayIn::Roof => (50.0, Cover::GOOD),
        }
    }

    /// What being noticed on the way in costs, against a given watcher.
    pub fn seen_cost(self, w: &crate::perception::Watcher) -> f32 {
        let (d, cover) = self.exposure();
        let open_s =
            if self.exposed_throughout() { self.take_s() } else { self.take_s() * 0.25 };
        crate::perception::seen_cost(w, d, cover, open_s)
    }
}

/// Assess the ways into a place. `window_s` is how long until whoever else
/// wants this arrives; infinite when nobody is coming.
#[cfg(test)]
pub fn infiltrate(
    window_s: f32,
    walk_s: f32,
    eyes: &crate::perception::Watcher,
) -> Vec<(WayIn, crate::approach::Verdict)> {
    WayIn::all()
        .into_iter()
        .map(|w| {
            let o = crate::approach::Opening {
                reach_spare_s: window_s - walk_s,
                take_s: w.take_s(),
                exposed_during_take: w.exposed_throughout(),
                shelters_after: w.unseen_after(),
                lingering_cost: w.parks_badly(),
                effort: w.take_s(),
                seen_cost: w.seen_cost(eyes),
            };
            (w, crate::approach::assess(&o, window_s))
        })
        .collect()
}

/// The way in to actually take, or none if the place cannot be entered in
/// time.
// NO `is_watched`. An earlier version filtered out any door where you would
// be noticed before finishing, and then ALSO charged `seen_cost` for it —
// the same fact counted twice, and the front door was ruled out by
// definition, since being seen is what the front door IS. Being noticed is a
// cost. Whether it is worth paying is `approach::choose`'s job, and it
// already knows how.

pub fn best_way_in(
    window_s: f32,
    walk_s: f32,
    eyes: &crate::perception::Watcher,
) -> Option<WayIn> {
    let ways: Vec<WayIn> = WayIn::all().to_vec();
    let openings: Vec<crate::approach::Opening> = ways
        .iter()
        .copied()
        .map(|w| crate::approach::Opening {
            reach_spare_s: window_s - walk_s,
            take_s: w.take_s(),
            exposed_during_take: w.exposed_throughout(),
            shelters_after: w.unseen_after(),
            lingering_cost: w.parks_badly(),
            effort: w.take_s(),
            seen_cost: w.seen_cost(eyes),
        })
        .collect();
    crate::approach::choose(&openings, window_s).map(|i| ways[i])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_relay_sits_somewhere_real() {
        for r in RELAYS {
            if let Some(p) = r.at {
                assert!(crate::groundside::place(p).is_some(), "{} is at unknown place {p}", r.id);
            } else {
                assert_eq!(r.channel, Channel::Car, "only the car is placeless");
            }
            assert!(!r.blurb.is_empty());
        }
        let ids: BTreeSet<_> = RELAYS.iter().map(|r| r.id).collect();
        assert_eq!(ids.len(), RELAYS.len(), "duplicate relay id");
    }

    #[test]
    fn the_car_stages_and_never_transmits() {
        // the scene the human campaign is built out of says "no secret base".
        // the car is the closest thing to one, and it must not become one.
        let car = relay("the_car").unwrap();
        assert_eq!(car.channel.reach(), 0, "the car started transmitting");
        assert_eq!(car.channel.exposure(), 0);
        let mut c = Case::default();
        c.take_capped("bunker_alloy", CASE_CAP).unwrap();
        let rep = report(&mut c, car);
        assert!(rep.landed.is_empty());
        assert_eq!(c.len(), 1, "the car ate the evidence");
    }

    #[test]
    fn reach_limits_what_gets_out_and_the_rest_stays_carried() {
        let mut c = Case::default();
        for id in ["bunker_alloy", "bunker_roster", "museum_specimen"] {
            c.take_capped(id, CASE_CAP).unwrap();
        }
        let phone = relay("port_payphone").unwrap();
        let rep = report(&mut c, phone);
        assert_eq!(rep.landed.len(), 1, "a phone call is one thing");
        assert_eq!(rep.landed[0], "bunker_alloy", "the front of the case goes first");
        assert_eq!(c.len(), 2, "the rest should still be carried");
        assert_eq!(rep.kept.len(), 2);

        // a hideout takes the lot
        let hide = relay("observatory_room").unwrap();
        let rep = report(&mut c, hide);
        assert_eq!(rep.landed.len(), 2);
        assert_eq!(c.len(), 0);
    }

    #[test]
    fn the_loud_channel_is_heard_by_the_people_you_did_not_call() {
        let lamp = relay("ridge_lamp").unwrap();
        assert_eq!(lamp.channel.exposure(), 3);
        let mut c = Case::default();
        c.take_capped("thermal_plate", CASE_CAP).unwrap();
        let rep = report(&mut c, lamp);
        assert!(!rep.overheard_by.is_empty(), "a rooftop lamp is not a private line");
        // ... and a dead drop is not overheard at all
        let drop = relay("museum_drop").unwrap();
        let mut c2 = Case::default();
        c2.take_capped("thermal_plate", CASE_CAP).unwrap();
        assert!(report(&mut c2, drop).overheard_by.is_empty());
    }

    #[test]
    fn a_person_retelling_it_is_not_a_photocopy() {
        assert!(!Channel::Contact.faithful());
        for ch in [Channel::Payphone, Channel::DeadDrop, Channel::Heliograph, Channel::Hideout] {
            assert!(ch.faithful(), "{ch:?} should arrive intact");
        }
        let mut c = Case::default();
        c.take_capped("port_reel", CASE_CAP).unwrap();
        assert!(report(&mut c, relay("newsstand_contact").unwrap()).degraded);
    }

    #[test]
    fn the_case_is_small_and_refuses_duplicates() {
        let mut c = Case::default();
        for i in 0..CASE_CAP {
            c.take_capped(&format!("f{i}"), CASE_CAP).unwrap();
        }
        assert!(c.take_capped("one_more", CASE_CAP).is_err(), "the case has no bottom");
        assert!(c.take_capped("f0", CASE_CAP).is_err(), "the case took the same thing twice");
        assert_eq!(c.len(), CASE_CAP);
    }

    #[test]
    fn every_society_has_somewhere_to_report_and_the_loud_one_is_shared() {
        for s in [Society::Directorate, Society::FreeState, Society::Covenant] {
            assert!(
                RELAYS.iter().any(|r| r.society == s),
                "{} has no relay of its own",
                s.name()
            );
        }
        // and there is at least one anyone can use, or an unaligned
        // protagonist cannot report at all
        assert!(RELAYS.iter().any(|r| r.society == Society::Unaligned && r.channel.reach() > 0));
    }

    #[test]
    fn every_society_fights_like_a_real_commander() {
        use crate::content::commander;
        let mut seen = std::collections::BTreeSet::new();
        for s in Society::all() {
            let d = s.doctrine();
            assert!(commander(d).is_some(), "{} has no such doctrine '{d}'", s.name());
            seen.insert(d);
        }
        // four societies, four doctrines: if two collapse together, one of
        // them is not a distinct faction in play whatever the fiction says
        assert_eq!(seen.len(), 4, "two societies fight identically: {seen:?}");

        // and the mapping is not arbitrary — the Covenant wants the digging
        // to stop rather than to win a firefight, so it must be the most
        // cautious of the four
        let cov = commander(Society::Covenant.doctrine()).unwrap();
        for s in Society::all() {
            if s == Society::Covenant {
                continue;
            }
            assert!(
                cov.caution >= commander(s.doctrine()).unwrap().caution,
                "{} is more cautious than the Covenant",
                s.name()
            );
        }
    }

    #[test]
    fn every_way_in_is_right_somewhere() {
        use crate::perception::Watcher;
        // the test of a real trade, and the reason `approach` needed a
        // seen-cost axis at all: each door has a window where it is correct.
        let guard = Watcher::guard();

        // plenty of time: the slow invisible way
        assert_eq!(best_way_in(600.0, 0.0, &guard), Some(WayIn::Roof));

        // not enough for the roof's eighty-five seconds in the open, but
        // enough to go round the side
        assert_eq!(best_way_in(80.0, 0.0, &guard), Some(WayIn::ServiceEntrance));

        // thirty seconds: only the front door fits at all, and being seen
        // beats not getting in
        assert_eq!(best_way_in(30.0, 0.0, &guard), Some(WayIn::FrontDoor));

        // fifteen: nothing
        assert_eq!(best_way_in(15.0, 0.0, &guard), None);
    }

    #[test]
    fn who_is_looking_changes_what_it_costs() {
        use crate::perception::Watcher;
        // THE POINT OF DERIVING IT. The doors did not change; the watcher
        // did. Three constants could not express this at all.
        let guard = Watcher::guard();
        let sentry = Watcher::sentry();
        for w in WayIn::all() {
            let cheap = w.seen_cost(&guard);
            let dear = w.seen_cost(&sentry);
            assert!(
                dear >= cheap,
                "{:?} costs no more against a sentry than a bored guard",
                w
            );
        }
        // and an attentive watcher makes the loud way disproportionately
        // worse, because the cost is squared in the exposure ratio
        let f = WayIn::FrontDoor;
        assert!(f.seen_cost(&sentry) > f.seen_cost(&guard) * 1.5);
    }

    #[test]
    fn the_hunter_is_answered_with_distance_and_nothing_else() {
        use crate::perception::Watcher;
        // the Assay case, and the second caller that justifies perception
        // existing. A hunter sees less far than a floodlit sentry and never
        // stops, so cover and patience buy nothing — only being outside its
        // range does.
        let hunter = Watcher::hunter();
        assert!(hunter.relentless);
        // the front door, eight metres out in the open, is suicide
        assert!(WayIn::FrontDoor.seen_cost(&hunter) > 1000.0);
        // the roof at fifty metres is outside its thirty-five, and free
        assert_eq!(WayIn::Roof.seen_cost(&hunter), 0.0);
        assert_eq!(best_way_in(600.0, 0.0, &hunter), Some(WayIn::Roof));
    }

    #[test]
    fn the_walk_counts_against_the_clock() {
        use crate::perception::Watcher;
        let g = Watcher::guard();
        assert!(best_way_in(600.0, 0.0, &g).is_some());
        assert_eq!(best_way_in(600.0, 599.0, &g), None, "the walk ate the window");
    }

    #[test]
    fn every_way_in_says_why_when_it_is_refused() {
        use crate::perception::Watcher;
        let g = Watcher::guard();
        for (w, v) in infiltrate(30.0, 0.0, &g) {
            if !v.viable() {
                assert!(!v.note().is_empty(), "{:?} was refused with no reason", w);
            }
        }
        assert!(infiltrate(600.0, 0.0, &g).iter().any(|(_, v)| v.viable()));
    }

    #[test]
    fn the_present_has_a_ship_so_it_keeps_almost_none_of_this() {
        let present: Vec<_> = RELAYS.iter().filter(|r| available(r, Era::Present)).collect();
        let archive: Vec<_> = RELAYS.iter().filter(|r| available(r, Era::Archive)).collect();
        assert_eq!(archive.len(), RELAYS.len(), "tradecraft is the archive era's whole method");
        assert!(present.len() < archive.len(), "the present should not need a rooftop lamp");
        assert!(!present.is_empty(), "a car and a dead drop still work in any century");
    }
}
