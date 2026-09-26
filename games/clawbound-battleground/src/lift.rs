//! lift -- the starlight updrafts between Starwell Glacier's islands and
//! the Aerie above them.
//!
//! Each lift is a pair of pads (`sky::lifts`): one on the islands, one on an
//! islet of the Aerie `sky::UPPER_Y` above. A cat that steps onto a pad is
//! taken by the updraft: it rises straight up in a column of starlight,
//! drifts across, and settles on the other pad -- up from below, down from
//! above. The ride takes [`RIDE_SECS`] and cannot be steered; the cat can be
//! struck on the way (it is in plain sight in a column of light).
//!
//! A pad only takes a cat that has just stepped ONTO it: one set down by
//! the lift is standing on the far pad, and must step off and back on to
//! ride again (`GodUnit::lift_pad`). Bots do not ride (their routes are the
//! lanes; a bot carried up would have no way to its lane).
//!
//! Pure path ([`Ride::at`]) plus a small tick (`Game::lifts_tick`), called
//! from the movement system.

use glam::Vec3;

/// A pad's radius: stand within this of its centre to be taken.
pub const PAD_R: f32 = 2.2;
/// A ride's length, seconds.
pub const RIDE_SECS: f32 = 3.2;
/// How far above the higher pad the updraft carries a cat before it drifts.
pub const CREST: f32 = 4.0;

/// A cat on its way up or down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ride {
    pub from: Vec3,
    pub to: Vec3,
    pub t: f32,
    /// Which lift, and which pad it will stand on at the end.
    pub lift: u8,
    pub to_pad: u8,
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

impl Ride {
    /// Where the rider is `t` into the ride: up the column (from below) or
    /// lifted off (from above) to the crest, across, and down onto the pad.
    pub fn at(&self, t: f32) -> Vec3 {
        let u = (t / RIDE_SECS).clamp(0.0, 1.0);
        let crest = self.from.y.max(self.to.y) + CREST;
        let rise = smooth(u / 0.4);
        let across = smooth((u - 0.3) / 0.5);
        let settle = smooth((u - 0.75) / 0.25);
        let xz = self.from.lerp(self.to, across);
        let y = self.from.y + (crest - self.from.y) * rise - (crest - self.to.y) * settle;
        Vec3::new(xz.x, y, xz.z)
    }
    /// Vertical over horizontal speed now, for the puppet's flight pose --
    /// held to a gentle pitch (the cat is carried, not leaping: straight up
    /// the column it rides level-ish, paws tucked).
    pub fn climb(&self) -> f32 {
        let (a, b) = (self.at(self.t), self.at(self.t + 0.05));
        let run = Vec3::new(b.x - a.x, 0.0, b.z - a.z).length().max(0.05);
        ((b.y - a.y) / run).clamp(-0.6, 0.6)
    }
    pub fn done(&self) -> bool {
        self.t >= RIDE_SECS
    }
}

/// Every pad on the active map, as (lift, which end: 0 below / 1 above,
/// centre).
pub fn pads() -> Vec<(u8, u8, Vec3)> {
    if crate::map::active() != crate::map::MapId::Sky {
        return Vec::new();
    }
    let mut v = Vec::new();
    for (k, (lo, hi)) in crate::sky::lifts().into_iter().enumerate() {
        v.push((k as u8, 0, lo));
        v.push((k as u8, 1, hi));
    }
    v
}

impl crate::sim::Game {
    /// Lifts, each tick: a cat stepping onto a pad is taken; riders move
    /// along the updraft and are set down on the far pad.
    pub fn lifts_tick(&mut self, dt: f32) {
        let pads = pads();
        if pads.is_empty() {
            return;
        }
        let mut arrivals = Vec::new();
        for i in 0..self.gods.len() {
            let g = &mut self.gods[i];
            if g.respawn_t > 0.0 {
                g.lift = None;
                g.lift_pad = None;
                continue;
            }
            if let Some(mut r) = g.lift {
                r.t += dt;
                g.pos = r.at(r.t);
                g.move_dir = glam::Vec3::ZERO;
                if r.done() {
                    g.pos = r.to;
                    g.lift = None;
                    g.lift_pad = Some(r.lift * 2 + r.to_pad);
                    arrivals.push(r.to);
                } else {
                    g.lift = Some(r);
                }
                continue;
            }
            let grounded = g.leap.is_none() && g.dash.is_none() && g.fall.is_none() && g.riding.is_none() && g.skyfall.is_none() && g.perch.is_none();
            let on = pads.iter().find(|(_, _, c)| Vec3::new(g.pos.x - c.x, 0.0, g.pos.z - c.z).length() < PAD_R && (g.pos.y - c.y).abs() < 2.0);
            match on {
                None => g.lift_pad = None,
                Some(&(k, end, c)) => {
                    let id = k * 2 + end;
                    if grounded && g.ai.is_none() && g.lift_pad != Some(id) {
                        let (lo, hi) = crate::sky::lifts()[k as usize];
                        let to = if end == 0 { hi } else { lo };
                        g.lift = Some(Ride { from: Vec3::new(g.pos.x, c.y, g.pos.z), to, t: 0.0, lift: k, to_pad: 1 - end });
                        // facing where it is carried
                        g.yaw = (to.z - g.pos.z).atan2(to.x - g.pos.x);
                        g.lift_pad = Some(id);
                        self.events.push(crate::sim::Event::NovaFx { pos: c, radius: PAD_R, color: [0.85, 0.92, 1.0] });
                    } else {
                        g.lift_pad = Some(id);
                    }
                }
            }
        }
        for p in arrivals {
            self.events.push(crate::sim::Event::NovaFx { pos: p, radius: PAD_R, color: [0.85, 0.92, 1.0] });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ride_rises_crosses_and_settles() {
        let r = Ride { from: Vec3::new(0.0, 0.0, -134.0), to: Vec3::new(0.0, 20.0, -106.0), t: 0.0, lift: 0, to_pad: 1 };
        assert!(r.at(0.0).distance(r.from) < 1e-3 && r.at(RIDE_SECS).distance(r.to) < 1e-3);
        // up first: well above the pad before it has moved much across
        let p = r.at(RIDE_SECS * 0.35);
        assert!(p.y > 15.0 && (p.z - r.from.z).abs() < 5.0, "the column first: {p}");
        // never below either pad on the way, never a jump
        let mut last = r.at(0.0);
        for k in 1..=100 {
            let q = r.at(RIDE_SECS * k as f32 / 100.0);
            assert!(q.y >= -1e-3 && (q - last).length() < 2.0, "smooth: {last} -> {q}");
            last = q;
        }
    }
}
