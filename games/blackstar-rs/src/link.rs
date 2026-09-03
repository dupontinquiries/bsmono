//! THE LINK — where the transformation is actually being done.
//!
//! `watch.rs` prices a shift against the BEARER: resolve, spent by
//! shifting, repaid slowly by rest. That is the man's side of the cost
//! and it does not change here. This module is the DEVICE's side, and
//! it exists because the Watch is a receiver, not an engine — the
//! authority over matter lives in the machine on the makers' world and
//! reaches the Watch down a quantum tunnel.
//!
//! Two states, and they are the same device:
//!
//! * **TETHERED** — the tunnel is up. The machine does the work; the
//!   Watch is an interface. Shifts are unhurried and cost the device
//!   nothing, so `watch.rs` alone governs them and this module gets out
//!   of the way. Everything shipped today is this state.
//! * **SEVERED** — occluded, out of range, jammed, deep inside
//!   something. No remote authority, so the Watch runs on the reservoir
//!   it carries. Finite, crude, and countable.
//!
//! ## The fare home
//!
//! The design decision embedded here, and the reason this is a module
//! rather than a counter: the LAST unit of charge is not a shift, it is
//! the way back. Severed, the reservoir spends down to one, and that
//! one is only spendable on a return to the bearer's native form. The
//! question a player carries stops being "how many can I spend" and
//! becomes "can I still get back", which is a question you can answer
//! wrong under pressure.
//!
//! Spend it anyway on something else and you cannot — `authorize`
//! refuses. Reach zero in a form that is not home and you are
//! STRANDED: playable, not a loss, and the way out is diegetic
//! (restore the tunnel, or reach something that can refill the
//! reservoir). What stranding costs is deliberately not combat power —
//! being stuck in ICE is not even a penalty, since ICE is the form that
//! opens the leap. It costs the human campaign, which `tradecraft.rs`
//! builds on a man being unremarkable enough to work a payphone.
//!
//! ## What this module refuses to do
//!
//! * **It does not touch resolve.** Two scarcities, two owners: the
//!   pool is what the man has, the reservoir is what the device has. A
//!   caller consults `authorize` and then calls `Watch::shift`, and
//!   both costs land.
//! * **It does not open Photon Man off the tether.** The ceiling is
//!   reached exactly once in six films at a cost that puts the bearer
//!   under for three days; letting it fall out of a reservoir would
//!   make it a resource. Severed, it is refused outright.
//! * **It does not know the fiction of WHY the tunnel dropped.** A
//!   caller severs it; this module only knows that it is down.

use crate::watch::{slot_named, Watch, NATIVE, PHOTON};

/// Where the authority for a shift is coming from right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tether {
    /// The tunnel is up. The machine is doing the work.
    Open,
    /// Cut off. The Watch is running on what it carries.
    Severed,
}

/// Why a shift was refused. Distinct variants because a caller wants to
/// say different things to the player for each — "you have nothing
/// left" and "that is your way home" are not the same refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Reservoir empty. If the current form is not home, this is the
    /// stranding.
    Dry,
    /// One charge left and it is reserved for the return. Only the home
    /// form can be bought with it.
    FareReserved,
    /// The ceiling does not come out of a tank.
    PhotonOffTether,
    /// Slot is not a mapped form.
    NoSuchForm,
}

/// What was granted, and what it cost the device. `Tethered` costs
/// nothing — the machine paid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    Tethered,
    /// A local shift off the reservoir; `left` is the charge remaining
    /// AFTER it is committed.
    Local { left: u8 },
    /// The reserved last charge, spent on the way back. `left` is 0.
    FareHome,
}

/// Reservoir size when full. Small on purpose: severed play is meant to
/// be counted on one hand, not managed on a bar.
pub const CAPACITY: u8 = 4;

pub struct Link {
    tether: Tether,
    charge: u8,
    capacity: u8,
}

impl Default for Link {
    /// Tethered and full — the state every shipped mission is already
    /// in, so adding this module changes nothing until a caller severs
    /// it.
    fn default() -> Link {
        Link { tether: Tether::Open, charge: CAPACITY, capacity: CAPACITY }
    }
}

/// The form the fare buys. ICE is the Morpher's native state per
/// `watch.rs`'s own `Default`, and this module asserts that rather than
/// hardcoding a second opinion about it.
fn home_slot() -> usize {
    slot_named("ICE").expect("ICE is a native form")
}

impl Link {
    pub fn tether(&self) -> Tether {
        self.tether
    }
    pub fn charge(&self) -> u8 {
        self.charge
    }
    pub fn capacity(&self) -> u8 {
        self.capacity
    }
    pub fn severed(&self) -> bool {
        self.tether == Tether::Severed
    }

    pub fn sever(&mut self) {
        self.tether = Tether::Severed;
    }
    /// The tunnel comes back. The reservoir does NOT refill with it —
    /// it is a tank, and it is filled by reaching something that can
    /// fill it, not by regaining signal.
    pub fn reconnect(&mut self) {
        self.tether = Tether::Open;
    }
    /// Refill, clamped. Returns what was actually taken on.
    pub fn recharge(&mut self, units: u8) -> u8 {
        let before = self.charge;
        self.charge = (self.charge.saturating_add(units)).min(self.capacity);
        self.charge - before
    }

    /// Can this shift happen at all, and what does the device pay for
    /// it. Pure — nothing is spent until `commit`, so a UI can preview
    /// a refusal without half-performing it.
    pub fn authorize(&self, slot: usize, desperate: bool) -> Result<Grant, Refusal> {
        let _ = desperate; // the last-resort declaration is watch.rs's lever, not this one
        if slot != PHOTON && slot >= NATIVE.len() {
            return Err(Refusal::NoSuchForm);
        }
        if self.tether == Tether::Open {
            return Ok(Grant::Tethered);
        }
        if slot == PHOTON {
            return Err(Refusal::PhotonOffTether);
        }
        match self.charge {
            0 => Err(Refusal::Dry),
            1 if slot == home_slot() => Ok(Grant::FareHome),
            1 => Err(Refusal::FareReserved),
            n => Ok(Grant::Local { left: n - 1 }),
        }
    }

    /// Take the grant. Separate from `authorize` so the caller can
    /// order this against `Watch::shift` however it needs — and so a
    /// refused `Watch::shift` (spent resolve) does not silently drain
    /// the reservoir.
    pub fn commit(&mut self, grant: Grant) {
        match grant {
            Grant::Tethered => {}
            Grant::Local { .. } | Grant::FareHome => {
                self.charge = self.charge.saturating_sub(1);
            }
        }
    }

    /// Severed, dry, and not in the form he started in. Playable, but
    /// he is not walking into a newsstand like this.
    pub fn stranded(&self, w: &Watch) -> bool {
        self.severed() && self.charge == 0 && w.current() != Some(home_slot())
    }

    /// `tether:charge:capacity` — one lenient line, same spirit as
    /// `watch.rs`'s and `chi.rs`'s.
    pub fn from_line(line: &str) -> Link {
        let mut l = Link::default();
        let mut parts = line.trim().split(':');
        if let Some(t) = parts.next() {
            l.tether = if t == "SEVERED" { Tether::Severed } else { Tether::Open };
        }
        if let Some(c) = parts.next().and_then(|s| s.parse().ok()) {
            l.charge = c;
        }
        if let Some(c) = parts.next().and_then(|s| s.parse().ok()) {
            l.capacity = c;
        }
        l.charge = l.charge.min(l.capacity);
        l
    }
    pub fn line(&self) -> String {
        format!(
            "{}:{}:{}",
            if self.severed() { "SEVERED" } else { "OPEN" },
            self.charge,
            self.capacity
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::slot_named;

    #[test]
    fn tethered_play_is_unchanged_and_costs_the_device_nothing() {
        let mut l = Link::default();
        for i in NATIVE {
            assert_eq!(l.authorize(i, false), Ok(Grant::Tethered));
            l.commit(Grant::Tethered);
        }
        assert_eq!(l.authorize(PHOTON, true), Ok(Grant::Tethered));
        assert_eq!(l.charge(), CAPACITY, "the machine paid, not the tank");
    }

    #[test]
    fn severed_shifts_come_out_of_the_reservoir() {
        let mut l = Link::default();
        l.sever();
        let fire = slot_named("FIRE").unwrap();
        let mut spent = 0;
        while let Ok(g) = l.authorize(fire, false) {
            l.commit(g);
            spent += 1;
            assert!(spent < 10, "the reservoir never ran down");
        }
        assert_eq!(l.charge(), 1, "it should have stopped ON the fare, not past it");
    }

    #[test]
    fn the_last_charge_only_buys_the_way_home() {
        let mut l = Link::default();
        l.sever();
        while l.charge() > 1 {
            let g = l.authorize(slot_named("FIRE").unwrap(), false).unwrap();
            l.commit(g);
        }
        assert_eq!(l.authorize(slot_named("FIRE").unwrap(), false), Err(Refusal::FareReserved));
        assert_eq!(l.authorize(slot_named("METAL").unwrap(), false), Err(Refusal::FareReserved));
        assert_eq!(l.authorize(slot_named("ICE").unwrap(), false), Ok(Grant::FareHome));
    }

    #[test]
    fn the_ceiling_is_never_available_off_the_tether() {
        let mut l = Link::default();
        l.sever();
        assert_eq!(l.authorize(PHOTON, true), Err(Refusal::PhotonOffTether));
        assert_eq!(l.authorize(PHOTON, false), Err(Refusal::PhotonOffTether));
        l.recharge(CAPACITY);
        assert_eq!(l.authorize(PHOTON, true), Err(Refusal::PhotonOffTether), "a full tank is not the tunnel");
    }

    #[test]
    fn stranding_is_being_dry_in_a_form_that_is_not_home() {
        let mut w = Watch::default();
        let mut l = Link::default();
        l.sever();
        // spend down to the fare, then take the last one somewhere else
        // — which authorize refuses, so force the state the hard way to
        // prove `stranded` reads the world and not a flag.
        while l.charge() > 0 {
            l.commit(Grant::Local { left: 0 });
        }
        assert!(!l.stranded(&w), "still in ICE — dry, but home");
        w.tick(100.0);
        w.shift(slot_named("FIRE").unwrap(), false).unwrap();
        assert!(l.stranded(&w), "dry and not home is the stranding");
        l.reconnect();
        assert!(!l.stranded(&w), "the tunnel came back; he can shift again");
    }

    #[test]
    fn reconnecting_does_not_refill_the_tank() {
        let mut l = Link::default();
        l.sever();
        while l.charge() > 1 {
            let g = l.authorize(slot_named("WIND").unwrap(), false).unwrap();
            l.commit(g);
        }
        l.reconnect();
        assert_eq!(l.charge(), 1, "signal is not fuel");
        assert_eq!(l.recharge(99), CAPACITY - 1, "recharge clamps to capacity");
        assert_eq!(l.charge(), CAPACITY);
    }

    #[test]
    fn the_save_line_roundtrips_and_survives_junk() {
        let mut l = Link::default();
        l.sever();
        l.commit(Grant::Local { left: 0 });
        let back = Link::from_line(&l.line());
        assert!(back.severed());
        assert_eq!(back.charge(), l.charge());
        assert_eq!(back.capacity(), l.capacity());
        let junk = Link::from_line("garbage::");
        assert!(!junk.severed());
        assert_eq!(junk.charge(), CAPACITY);
    }
}
