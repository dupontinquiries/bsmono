//! POWERS — the human interface, and what it takes.
//!
//! The Watch (`watch.rs`) is one ability with a form selector, mastered
//! before the story starts: costed, limited, and NOT degrading. The Morpher
//! knows what it does to him and it does not do more each year.
//!
//! The human interface is the other thing entirely, and the archaeology layer
//! already established what: `archive::CLAIMS` carries
//! `human_interface_unstable` — "the interface degrades its host; the
//! technology was never meant for that body" — corroborated by
//! `interface_autopsy` (tissue carrying more energy than tissue can, damaged
//! by working CORRECTLY for years) and by `rotation_ledger`, four revisions
//! of a schedule cycling interface staff out sooner each time. Nine years,
//! then four.
//!
//! So a human power is not a cooldown and not a resource bar. It is a
//! **withdrawal against a body**, and the balance never refills.
//!
//! ## Why burden is permanent, and what that costs the design
//!
//! A per-mission meter is the easy version and it makes the interface a
//! stamina system: spend it, sleep, spend it again, and the fiction's central
//! claim quietly stops being true. Permanent burden means a player who leans
//! on a power in Film 3 is a worse operative in Film 12, which is a real cost
//! — it can feel like being punished for playing — and it is the only version
//! that matches "they were not injured, they were used correctly".
//!
//! The mitigation is that burden is VISIBLE and the arithmetic is honest:
//! `remaining_years` tells you what you are spending before you spend it, and
//! `--powers` prints the whole ledger. A cost you can see and plan around is
//! a decision; a cost you discover afterwards is a trap.
//!
//! ## The Archivist
//!
//! He has carried it longest, and his burden is the story's own answer to why
//! he wants every fragment destroyed. `starting_burden` puts him near the
//! ceiling — not as flavour, but so his readout IS his motive. A player who
//! runs the villain campaign is playing a man whose meter is nearly full and
//! who has done the arithmetic on everyone else's.

use crate::archive::Lens;

/// What a power actually does. Verbs, not damage numbers — the same
/// reasoning `weapons.rs` gives for its own roster.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verb {
    /// route current through a structure, or a person
    Discharge,
    /// read a room's wiring, cameras and locks as a live map
    Sense,
    /// move faster than a body should, briefly
    Surge,
    /// take a hit a body should not survive
    Absorb,
    /// make somebody else's interface fail
    Sever,
}

impl Verb {
    pub fn label(self) -> &'static str {
        match self {
            Verb::Discharge => "DISCHARGE",
            Verb::Sense => "SENSE",
            Verb::Surge => "SURGE",
            Verb::Absorb => "ABSORB",
            Verb::Sever => "SEVER",
        }
    }
}

pub struct Power {
    pub id: &'static str,
    pub lens: Lens,
    pub verb: Verb,
    /// Burden per use, in months of a host's remaining service.
    ///
    /// Months rather than an abstract score because the fiction quotes the
    /// cost in years and the rotation ledger is a calendar. A number the
    /// player can convert into "how many more jobs" is a number they can
    /// decide with.
    pub months: f32,
    pub note: &'static str,
}

/// What the interface allows, per character.
pub static POWERS: &[Power] = &[
    // ---- DRAGON PULSE: discipline, and a body that pays ----
    Power {
        id: "pulse_strike",
        lens: Lens::DragonPulse,
        verb: Verb::Discharge,
        months: 2.0,
        note: "everything he carries, through one point of contact. the cheapest \
               thing he does and the reason his chart looks the way it does.",
    },
    Power {
        id: "pulse_hold",
        lens: Lens::DragonPulse,
        verb: Verb::Absorb,
        months: 6.0,
        note: "stand in front of it. the tissue damage in the autopsy file is \
               this, repeated, by somebody doing it correctly.",
    },
    // ---- VOLTAGE: charm and electromagnetics ----
    Power {
        id: "read_the_room",
        lens: Lens::Voltage,
        verb: Verb::Sense,
        months: 0.5,
        note: "every cable, camera and mag-lock in the building, as a map. cheap, \
               and he uses it constantly, which is its own kind of expensive.",
    },
    Power {
        id: "brownout",
        lens: Lens::Voltage,
        verb: Verb::Discharge,
        months: 3.0,
        note: "drop a block. the loud option, in a campaign where loud is the \
               cost (see tradecraft::seen_cost).",
    },
    // ---- KINETIC: movement, distance, and what left ----
    Power {
        id: "closing_speed",
        lens: Lens::Kinetic,
        verb: Verb::Surge,
        months: 1.5,
        note: "faster than a body should move, for about four seconds. the four \
               seconds are not the limit; the joints are.",
    },
    Power {
        id: "read_the_leaving",
        lens: Lens::Kinetic,
        verb: Verb::Sense,
        months: 0.5,
        note: "what moved through here, how fast, and how long ago.",
    },
    // ---- THE ARCHIVIST ----
    Power {
        id: "unmake",
        lens: Lens::Archivist,
        verb: Verb::Sever,
        months: 8.0,
        note: "make somebody else's interface fail. the most expensive thing in \
               the table, used by the person with the least left to spend.",
    },
    Power {
        id: "hold_the_line",
        lens: Lens::Archivist,
        verb: Verb::Absorb,
        months: 6.0,
        note: "he has done this longer than anyone and it shows in the arithmetic \
               rather than in his face.",
    },
];

/// A body carrying the interface.
#[derive(Clone, Copy, Debug)]
pub struct Host {
    pub lens: Lens,
    /// months already spent
    pub burden: f32,
}

/// What the first revision of the schedule allowed, in months.
///
/// The fiction's own figure: revision one gives nine years. `rotation_ledger`
/// has it shrinking to four across four revisions, which is what the
/// Directorate learned and did not write down.
pub const SERVICE_MONTHS: f32 = 9.0 * 12.0;

/// Where the interface starts failing: the last fifth of a service life.
pub const FAILING_AT: f32 = 0.8;

impl Host {
    pub fn new(lens: Lens) -> Host {
        Host { lens, burden: starting_burden(lens) }
    }

    /// Months of service left before the ceiling.
    pub fn remaining_months(&self) -> f32 {
        (SERVICE_MONTHS - self.burden).max(0.0)
    }
    pub fn remaining_years(&self) -> f32 {
        self.remaining_months() / 12.0
    }
    /// 0 fresh .. 1 spent.
    pub fn spent(&self) -> f32 {
        (self.burden / SERVICE_MONTHS).clamp(0.0, 1.0)
    }
    /// Past this the interface misfires; the body is the failing part.
    pub fn failing(&self) -> bool {
        self.spent() >= FAILING_AT
    }
    pub fn done(&self) -> bool {
        self.remaining_months() <= 0.0
    }

    /// Can this host use this power at all?
    pub fn may_use(&self, p: &Power) -> Result<(), &'static str> {
        if p.lens != self.lens {
            return Err("that is not their interface");
        }
        if self.done() {
            return Err("there is nothing left to spend");
        }
        Ok(())
    }

    /// Spend. Returns the months actually taken.
    ///
    /// A failing interface costs MORE, which is the trap the fiction
    /// describes: the closer a host is to the end, the faster the last of it
    /// goes, so the schedule revisions kept shortening.
    pub fn use_power(&mut self, p: &Power) -> Result<f32, &'static str> {
        self.may_use(p)?;
        let mult = if self.failing() { 1.5 } else { 1.0 };
        let cost = (p.months * mult).min(self.remaining_months());
        self.burden += cost;
        Ok(cost)
    }
}

/// Where each character starts.
///
/// The Archivist's is the load-bearing one: he has carried this longest, and
/// a readout near the ceiling is his motive stated as a number rather than as
/// a monologue.
pub fn starting_burden(lens: Lens) -> f32 {
    match lens {
        Lens::Archivist => SERVICE_MONTHS * 0.86,
        // Dragon Pulse has been doing it eleven years by his own account in
        // `archive::READINGS`, against a schedule that allowed nine
        Lens::DragonPulse => SERVICE_MONTHS * 0.62,
        Lens::Voltage | Lens::Kinetic => SERVICE_MONTHS * 0.15,
        // the Transformer does not carry a human interface at all
        Lens::Patrol => 0.0,
    }
}

pub fn power(id: &str) -> Option<&'static Power> {
    POWERS.iter().find(|p| p.id == id)
}

/// What this character can do.
pub fn powers_of(lens: Lens) -> Vec<&'static Power> {
    POWERS.iter().filter(|p| p.lens == lens).collect()
}

// --------------------------------------------------------------- driver ---

/// `--powers` — the interface, and what each of them has left.
pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--powers").unwrap_or(0);
    let only = argv.get(i + 1).and_then(|s| Lens::parse(s));

    println!("THE HUMAN INTERFACE\n");
    println!("  it degrades its host. burden is permanent and the balance never");
    println!("  refills, because \"they were not injured, they were used correctly\".");
    println!("  a full service life is {:.0} years.\n", SERVICE_MONTHS / 12.0);

    for lens in Lens::all() {
        if lens == Lens::Patrol || only.map_or(false, |l| l != lens) {
            continue;
        }
        let h = Host::new(lens);
        println!(
            "  {} — {:.1} of {:.0} years left ({:.0}% spent){}",
            lens.name(),
            h.remaining_years(),
            SERVICE_MONTHS / 12.0,
            h.spent() * 100.0,
            if h.failing() { "  FAILING" } else { "" }
        );
        for p in powers_of(lens) {
            let mut probe = h;
            let cost = probe.use_power(p).unwrap_or(0.0);
            println!(
                "      {:<16} {:<10} {:>4.1} mo   {:.1} yr left after",
                p.id,
                p.verb.label(),
                cost,
                probe.remaining_years()
            );
            println!("          {}", wrap(p.note, 62, 10));
        }
        println!();
    }

    if only.is_none() {
        println!("  the Archivist's readout is his motive. he has done the arithmetic");
        println!("  on everyone else's too, and concluded none of it should exist.");
    }
}

fn wrap(text: &str, w: usize, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let (mut out, mut col) = (String::new(), 0);
    for word in text.split_whitespace() {
        if col > 0 && col + 1 + word.len() > w {
            out.push('\n');
            out.push_str(&pad);
            col = 0;
        } else if col > 0 {
            out.push(' ');
            col += 1;
        }
        out.push_str(word);
        col += word.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_power_belongs_to_a_human_and_the_transformer_has_none() {
        for p in POWERS {
            assert_ne!(p.lens, Lens::Patrol, "{} is a human interface power", p.id);
            assert!(p.lens.human());
            assert!(p.months > 0.0, "{} is free, which the fiction forbids", p.id);
            assert!(p.note.len() > 30, "{} has no reasoning", p.id);
        }
        assert!(powers_of(Lens::Patrol).is_empty());
        // and each human has something to do
        for l in [Lens::DragonPulse, Lens::Voltage, Lens::Kinetic, Lens::Archivist] {
            assert!(!powers_of(l).is_empty(), "{} has no powers", l.name());
        }
    }

    #[test]
    fn burden_is_permanent_and_never_refills() {
        // the property that stops this being a stamina bar
        let mut h = Host::new(Lens::Voltage);
        let start = h.remaining_months();
        let p = power("brownout").unwrap();
        h.use_power(p).unwrap();
        let after = h.remaining_months();
        assert!(after < start);
        // there is no path back: nothing in the API reduces burden
        h.use_power(p).unwrap();
        assert!(h.remaining_months() < after, "burden went the wrong way");
    }

    #[test]
    fn a_host_cannot_borrow_another_ones_interface() {
        let h = Host::new(Lens::Voltage);
        assert!(h.may_use(power("unmake").unwrap()).is_err(), "Voltage used the Archivist's");
        assert!(h.may_use(power("brownout").unwrap()).is_ok());
    }

    #[test]
    fn a_failing_interface_costs_more_which_is_the_trap() {
        let p = power("pulse_strike").unwrap();
        let mut fresh = Host { lens: Lens::DragonPulse, burden: 0.0 };
        let mut late = Host { lens: Lens::DragonPulse, burden: SERVICE_MONTHS * 0.9 };
        assert!(!fresh.failing() && late.failing());
        let a = fresh.use_power(p).unwrap();
        let b = late.use_power(p).unwrap();
        assert!(b > a, "the last of it should go faster: {b} vs {a}");
    }

    #[test]
    fn spending_stops_at_the_ceiling_rather_than_going_negative() {
        let mut h = Host { lens: Lens::Archivist, burden: SERVICE_MONTHS - 1.0 };
        let p = power("unmake").unwrap();
        let cost = h.use_power(p).unwrap();
        assert!(cost <= 1.0, "took {cost} months from a host with one left");
        assert!(h.done());
        assert_eq!(h.remaining_months(), 0.0);
        assert!(h.use_power(p).is_err(), "a spent host kept going");
    }

    #[test]
    fn the_archivist_is_nearly_out_and_that_is_the_point() {
        // his readout IS his motive: a man near the ceiling who has done the
        // arithmetic on everyone else's
        let a = Host::new(Lens::Archivist);
        assert!(a.failing(), "the Archivist should be past the failing mark");
        for other in [Lens::Voltage, Lens::Kinetic, Lens::DragonPulse] {
            assert!(
                Host::new(other).remaining_years() > a.remaining_years(),
                "{} has less left than the Archivist",
                other.name()
            );
        }
        // and the most expensive power in the table is his
        let dearest = POWERS.iter().max_by(|a, b| a.months.total_cmp(&b.months)).unwrap();
        assert_eq!(dearest.lens, Lens::Archivist);
    }

    #[test]
    fn dragon_pulse_has_outlived_the_schedule_that_allowed_him() {
        // `archive::READINGS` has him at eleven years against a nine-year
        // service life; his burden should reflect a man past the line the
        // rotation ledger drew
        let h = Host::new(Lens::DragonPulse);
        assert!(h.spent() > 0.5, "he reads as fresh, which contradicts his own line");
        assert!(!h.failing(), "he is not finished yet — that is later in the story");
    }
}
