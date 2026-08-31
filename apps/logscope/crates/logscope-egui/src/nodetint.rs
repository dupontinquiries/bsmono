//! nodetint — the conversation's node colours, derived from the room.
//!
//! ## What was fixed in place
//!
//! The graph is the centre of the screen, and its nodes were coloured from six hardcoded hues —
//! the same blue/green/amber/violet/coral/teal in every space. In the reference images the node
//! plaques are unmistakably part of the room: brass-on-teal in the drowned library, warm grey on
//! parchment in the observatory. Six fixed colours cannot do that, and once the sixteen procedural
//! rooms joined the picker they were wrong in thirty-five of thirty-seven spaces rather than in a
//! handful.
//!
//! ## Hues from the room, spacing from the wheel
//!
//! The space's accent sets the anchor hue; the rest are placed around the wheel from it, and
//! saturation and value are taken from the palette so a pale room gets pale nodes and a dark room
//! gets luminous ones. Segment 0 is the accent itself, so the first topic of a conversation is
//! literally the room's own colour.
//!
//! Rotating by a fixed step would collide with the anchor on some wheel positions, so the offsets
//! are a fixed irregular set chosen to stay apart under any anchor — and that claim is audited
//! rather than asserted.
//!
//! ## Two properties, audited over every space
//!
//! A derived palette can fail in two ways that are invisible until someone is looking at a
//! particular room: two segments too close to tell apart, or a segment too close to the background
//! to see. Both are contrast ratios, both are computed for all thirty-seven spaces in the test
//! below, and the failure names the space and the pair with their numbers.

use crate::roomtheme::{contrast, force_contrast};
use crate::theme::Palette;
use egui::Color32;

/// How many segment colours the graph cycles through.
pub const SEGMENTS: usize = 6;

/// Hue offsets from the room's accent, in turns. Irregular on purpose: an even 1/6 spacing puts
/// two entries near-opposite the anchor where many palettes already sit, and the pair audit below
/// caught that as a collision. These are spread so no pair lands within 0.09 turns of another.
const HUE_OFFSETS: [f32; SEGMENTS] = [0.0, 0.13, 0.30, 0.47, 0.63, 0.81];

fn rgb_to_hsv(c: Color32) -> (f32, f32, f32) {
    let (r, g, b) = (c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON { 0.0 }
        else if max == r { ((g - b) / d).rem_euclid(6.0) / 6.0 }
        else if max == g { (((b - r) / d) + 2.0) / 6.0 }
        else { (((r - g) / d) + 4.0) / 6.0 };
    (h, if max <= 0.0 { 0.0 } else { d / max }, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color32 {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p), 1 => (q, v, p), 2 => (p, v, t),
        3 => (p, q, v), 4 => (t, p, v), _ => (v, p, q),
    };
    let q8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(q8(r), q8(g), q8(b))
}

/// The segment colours for a space.
pub fn segments(p: &Palette) -> [Color32; SEGMENTS] {
    let (anchor_h, anchor_s, anchor_v) = rgb_to_hsv(p.amber);
    // A near-grey accent carries no hue to build on (several stone rooms have one), so fall back to
    // a neutral anchor and let the offsets supply the variety. Without this every segment in those
    // rooms comes out the same grey.
    let (h0, s) = if anchor_s < 0.12 { (0.58, 0.45) } else { (anchor_h, anchor_s.clamp(0.35, 0.85)) };
    // Nodes must read AGAINST the room, so a dark room gets brighter nodes than its accent and a
    // pale room gets deeper ones.
    let dark = p.dark;
    let v = if dark { anchor_v.max(0.72) } else { anchor_v.min(0.66) };
    let mut out = [Color32::BLACK; SEGMENTS];
    for (i, off) in HUE_OFFSETS.iter().enumerate() {
        // Alternate the saturation slightly so neighbours differ in more than hue — hue alone is
        // weak separation for small marks on a textured surface.
        let sat = if i % 2 == 0 { s } else { (s * 0.78).clamp(0.25, 0.9) };
        // FORCED OFF THE BACKGROUND, not merely value-clamped. Clamping the value was not enough in
        // the pale rooms — Reading Room, Cloudscape, Forest and Mansion Garden each had segments
        // down at 1.2-1.7 against their own background, which the population audit found the first
        // time it ran. A node the colour of the floor it sits on is not a node.
        out[i] = force_contrast(hsv_to_rgb(h0 + off, sat, v), p.bg, 2.1);
    }
    out
}

/// The closest pair of segment colours, as a contrast ratio, and which pair it was.
pub fn worst_pair(cols: &[Color32; SEGMENTS]) -> (f32, usize, usize) {
    let mut worst = (f32::MAX, 0, 0);
    for i in 0..SEGMENTS {
        for j in (i + 1)..SEGMENTS {
            // Contrast alone rates two different hues of equal lightness as identical, so pair
            // separation is hue distance OR luminance contrast — either is enough to tell them
            // apart, and requiring both would reject perfectly readable sets.
            let (hi, _, _) = rgb_to_hsv(cols[i]);
            let (hj, _, _) = rgb_to_hsv(cols[j]);
            let dh = (hi - hj).abs().min(1.0 - (hi - hj).abs());
            let lum = contrast(cols[i], cols[j]);
            // Fold hue distance into a comparable scalar: a tenth of a turn is worth about as much
            // as a 1.35 contrast ratio for telling two dots apart.
            let score = lum.max(1.0 + dh * 3.5);
            if score < worst.0 { worst = (score, i, j); }
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roomtheme;

    fn all_spaces() -> Vec<Palette> { roomtheme::spaces() }

    #[test]
    fn hsv_round_trips() {
        for c in [Color32::from_rgb(0x6e, 0xc6, 0xff), Color32::from_rgb(0x40, 0x40, 0x40),
                  Color32::from_rgb(0xff, 0x00, 0x00), Color32::WHITE, Color32::BLACK] {
            let (h, s, v) = rgb_to_hsv(c);
            let back = hsv_to_rgb(h, s, v);
            for (a, b) in [(c.r(), back.r()), (c.g(), back.g()), (c.b(), back.b())] {
                assert!((a as i32 - b as i32).abs() <= 1, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn segment_zero_is_the_rooms_own_accent_hue() {
        for p in all_spaces() {
            let (ah, asat, _) = rgb_to_hsv(p.amber);
            if asat < 0.12 { continue; } // grey-accent rooms use the neutral anchor by design
            let (sh, _, _) = rgb_to_hsv(segments(&p)[0]);
            let d = (ah - sh).abs().min(1.0 - (ah - sh).abs());
            assert!(d < 0.02, "{}: segment 0 hue {sh:.3} is not the accent's {ah:.3}", p.name);
        }
    }

    /// THE POPULATION AUDIT: every space, both failure modes. A node set that is legible in the
    /// three spaces someone opened and muddy in the other thirty-four is the thing this prevents.
    #[test]
    fn every_space_gives_distinguishable_nodes_that_show_against_it() {
        let mut bad = Vec::new();
        for p in all_spaces() {
            let cols = segments(&p);
            let (score, i, j) = worst_pair(&cols);
            if score < 1.30 {
                bad.push(format!("{}: segments {i} and {j} too close ({score:.2})", p.name));
            }
            for (i, c) in cols.iter().enumerate() {
                let vs_bg = contrast(*c, p.bg);
                if vs_bg < 1.9 {
                    bad.push(format!("{}: segment {i} vs background {vs_bg:.2}", p.name));
                }
            }
        }
        assert!(bad.is_empty(), "{} problem(s):\n  {}", bad.len(), bad.join("\n  "));
    }

    /// The report, printed: the margin each space has, so a change that merely scrapes past shows.
    #[test]
    fn node_separation_report() {
        let mut rows: Vec<(String, f32, f32)> = all_spaces().iter().map(|p| {
            let cols = segments(p);
            let (pair, _, _) = worst_pair(&cols);
            let bg = cols.iter().map(|c| contrast(*c, p.bg)).fold(f32::MAX, f32::min);
            (p.name.to_string(), pair, bg)
        }).collect();
        rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        println!("{:<20} {:>10} {:>10}", "space", "pair", "vs bg");
        for (n, a, b) in rows.iter().take(10) { println!("{n:<20} {a:>10.2} {b:>10.2}"); }
        assert!(rows.len() >= 30);
    }

    /// SAME CORRECTION AS ROOMTHEME'S. `segments` is called at runtime with `theme::active()`, which
    /// is the DAY/NIGHT-ADJUSTED palette, and the audit above feeds it base palettes. Those are
    /// different inputs: the adjustment moves `bg` (the surface these are measured against) and
    /// repairs `amber` (the hue they are anchored on), so passing at base proves nothing about what
    /// renders. This runs the same two checks on the palette the graph will actually be drawn over.
    #[test]
    fn nodes_hold_up_against_the_palette_that_actually_renders() {
        let mut bad = Vec::new();
        for p in all_spaces() {
            for (label, day) in [("night", 0.0f32), ("dusk", 0.5), ("noon", 1.0)] {
                let q = crate::theme::day_night_adjust_at(p, day);
                let cols = segments(&q);
                let (score, i, j) = worst_pair(&cols);
                if score < 1.30 {
                    bad.push(format!("{} at {label}: segments {i}/{j} at {score:.2}", p.name));
                }
                for (i, c) in cols.iter().enumerate() {
                    let vs = contrast(*c, q.bg);
                    if vs < 1.9 { bad.push(format!("{} at {label}: segment {i} vs bg {vs:.2}", p.name)); }
                }
            }
        }
        assert!(bad.is_empty(), "{} problem(s) in the rendered palette:\n  {}",
                bad.len(), bad.join("\n  "));
    }

    /// The point of deriving: two different rooms must not produce the same node colours.
    #[test]
    fn different_rooms_give_different_node_colours() {
        let a = segments(&roomtheme::by_name("Observatory"));
        let b = segments(&roomtheme::by_name("Foundry"));
        assert_ne!(a[0], b[0], "a blue-lit room and an orange-lit one share a first segment");
        // and the room's own hue really is in there
        let (ah, _, _) = rgb_to_hsv(roomtheme::by_name("Foundry").amber);
        let (sh, _, _) = rgb_to_hsv(b[0]);
        assert!((ah - sh).abs().min(1.0 - (ah - sh).abs()) < 0.02);
    }

    /// A room whose accent is grey carries no hue to build on — several stone rooms are like this,
    /// and without the neutral fallback every segment comes out the same grey.
    #[test]
    fn a_grey_accented_room_still_gets_six_distinct_nodes() {
        let mut p = roomtheme::by_name("Vault");
        p.amber = Color32::from_rgb(0x88, 0x88, 0x88);
        let cols = segments(&p);
        let (score, i, j) = worst_pair(&cols);
        assert!(score >= 1.30, "grey accent collapsed segments {i}/{j} to {score:.2}");
        assert!(crate::roomtheme::luminance(cols[0]) > 0.0);
    }
}
