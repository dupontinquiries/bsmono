//! comic — a scene cut as a comic page, with an auto mode.
//!
//! Three pieces, each testable alone:
//!
//! * [`plan`] — the auto mode. Dialogue timing in, a list of timed
//!   [`Layout`]s out. No pixels.
//! * [`compose`] — one layout drawn from the frames of the moment. No timing.
//! * the render job in `framepipe` decodes the sources in lockstep and calls
//!   `compose` per frame.
//!
//! ## The rules of the auto mode
//!
//! Written from the rooftop prototype and from what that scene needs: the
//! distance between the characters is part of the story, so the wide is the
//! home the cut always returns to, and never cropped.
//!
//! 1. **Open wide** for at least `open` seconds: the audience learns the space.
//! 2. **A conversation becomes a page.** When one side has been talking for
//!    `page_after` seconds, fold into a page: the wide across the top, the
//!    talking side large below, the other side's reactions small beside it.
//! 3. **Pages breathe.** After `page_max` seconds of page, return to the wide
//!    for `breathe` seconds, even mid-conversation.
//! 4. **A short line gets an inset**, not a page: a picture-in-picture of that
//!    side over the wide.
//! 5. **When the other side joins, split the screen**: the moment two groups
//!    meet is the moment to show both at once.
//!
//! Sides are where people sit in the wide (left / right), so a panel's place
//! on screen matches its people's place in the scene -- and matches a
//! dialogue pan, if there is one.

/// Which side of the wide a line's speakers sit on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side { Left, Right }

impl Side { pub fn other(self) -> Side { match self { Side::Left => Side::Right, Side::Right => Side::Left } } }

/// A line of dialogue: when, and which side is talking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line { pub start: f64, pub end: f64, pub side: Side }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Layout {
    /// The wide, full frame.
    Wide,
    /// Wide across the top; `talking` side large below, the other small.
    Page { talking: Side },
    /// The wide with a picture-in-picture of one side.
    Inset { side: Side },
    /// Left side | right side.
    Split,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rules {
    pub open: f64,
    pub page_after: f64,
    pub page_max: f64,
    pub breathe: f64,
    pub inset_max_line: f64,
    pub split_hold: f64,
    /// No shot shorter than this: a shorter one is absorbed into the shot
    /// before it. The first auto plan of the rooftop put a 0.36 s flash of
    /// the wide between the last page and the split -- a blip nobody would cut
    /// on purpose.
    pub min_shot: f64,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { open: 4.0, page_after: 1.5, page_max: 8.0, breathe: 2.5, inset_max_line: 3.0, split_hold: 3.0, min_shot: 0.8 }
    }
}

/// One stretch of the cut.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot { pub start: f64, pub end: f64, pub layout: Layout }

/// The auto mode: timed layouts covering [0, length).
pub fn plan(lines: &[Line], length: f64, r: Rules) -> Vec<Shot> {
    let mut lines: Vec<Line> = lines.iter().copied().filter(|l| l.end > l.start && l.start < length).collect();
    lines.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    let mut shots: Vec<Shot> = Vec::new();
    let mut push = |shots: &mut Vec<Shot>, s: f64, e: f64, layout: Layout| {
        let (s, e) = (s.max(0.0), e.min(length));
        if e <= s + 1e-6 { return; }
        if let Some(last) = shots.last_mut() {
            if last.layout == layout && (last.end - s).abs() < 1e-6 { last.end = e; return; }
        }
        shots.push(Shot { start: s, end: e, layout });
    };
    push(&mut shots, 0.0, r.open.min(length), Layout::Wide);
    let mut t: f64 = r.open;
    let mut last_side: Option<Side> = None;
    let mut i = 0;
    while i < lines.len() && t < length {
        let l = lines[i];
        // Rule 5: the other side joins -> split, as the joining line starts
        if let Some(prev) = last_side {
            if l.side != prev && l.start + 1e-6 >= t {
                push(&mut shots, t, l.start, Layout::Wide);
                let e = (l.start + r.split_hold.max(l.end - l.start)).min(length);
                push(&mut shots, l.start, e, Layout::Split);
                t = e; last_side = Some(l.side); i += 1; continue;
            }
        }
        // a run of lines from one side, gaps under a second
        let mut j = i;
        while j + 1 < lines.len() && lines[j + 1].side == l.side && lines[j + 1].start - lines[j].end < 1.0 { j += 1; }
        let (run_s, run_e) = (l.start.max(t), lines[j].end);
        if run_e <= t { i = j + 1; last_side = Some(l.side); continue; }
        if run_e - run_s <= r.inset_max_line {
            // Rule 4: short -> inset over the wide
            push(&mut shots, t, run_s, Layout::Wide);
            push(&mut shots, run_s, run_e, Layout::Inset { side: l.side });
            t = run_e;
        } else {
            // Rules 2 and 3: a page after page_after s, breathing every page_max s
            push(&mut shots, t, run_s + r.page_after, Layout::Wide);
            let mut p = run_s + r.page_after;
            while p < run_e && p < length {
                let pe = (p + r.page_max).min(run_e);
                push(&mut shots, p, pe, Layout::Page { talking: l.side });
                p = pe;
                if p < run_e {
                    push(&mut shots, p, (p + r.breathe).min(run_e), Layout::Wide);
                    p = (p + r.breathe).min(run_e);
                }
            }
            t = run_e;
        }
        last_side = Some(l.side);
        i = j + 1;
    }
    push(&mut shots, t, length, Layout::Wide);
    // Rule 6: absorb blips. A shot under min_shot becomes part of the shot
    // before it (or after it, for the very first), then equal neighbours merge.
    let mut out: Vec<Shot> = Vec::new();
    for s in shots {
        match out.last_mut() {
            Some(prev) if s.end - s.start < r.min_shot => prev.end = s.end,
            Some(prev) if prev.layout == s.layout => prev.end = s.end,
            _ => out.push(s),
        }
    }
    if out.len() > 1 && out[0].end - out[0].start < r.min_shot {
        let first = out.remove(0); out[0].start = first.start;
    }
    out
}

// ── the compositor ─────────────────────────────────────────────────────────

use crate::framepipe::Frame;

/// The look: paper, ink, the party-colour frame, the white keyline.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub paper: [u8; 3],
    pub ink: [u8; 3],
    pub keyline: [u8; 3],
    /// The frame's colours, walked round each panel as dashes.
    pub neon: Vec<[u8; 3]>,
    pub border: u32,
    /// Confetti pieces on the page (0 for a plain page).
    pub confetti: u32,
}

impl Default for Style {
    fn default() -> Style {
        Style { paper: [168, 158, 150], ink: [16, 18, 20], keyline: [242, 238, 235],
                neon: vec![[255, 60, 180], [40, 110, 255], [255, 230, 60], [90, 255, 120]],
                border: 5, confetti: 46 }
    }
}

/// A region of a source frame, as fractions: which part of an angle a panel
/// shows. The editor picks it -- "these people" -- because on real night
/// footage no face detector found the people who mattered (the rooftop test).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region { pub x0: f32, pub y0: f32, pub x1: f32, pub y1: f32 }

impl Region {
    pub const WHOLE: Region = Region { x0: 0.0, y0: 0.0, x1: 1.0, y1: 1.0 };
    /// The smallest a region may be pinched to, as a fraction of the frame:
    /// a panel of a few pixels shows nothing, and a runaway pinch should
    /// stop somewhere usable rather than vanish.
    pub const MIN: f32 = 0.15;

    pub fn w(&self) -> f32 { self.x1 - self.x0 }
    pub fn h(&self) -> f32 { self.y1 - self.y0 }
    pub fn centre(&self) -> (f32, f32) { ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0) }

    /// The same size, centred at `c`, kept inside the frame -- a tap on the
    /// person you want.
    pub fn centred_at(&self, c: (f32, f32)) -> Region {
        let (w, h) = (self.w().min(1.0), self.h().min(1.0));
        let x0 = (c.0 - w / 2.0).clamp(0.0, 1.0 - w);
        let y0 = (c.1 - h / 2.0).clamp(0.0, 1.0 - h);
        Region { x0, y0, x1: x0 + w, y1: y0 + h }
    }

    /// Moved by `d` (fractions of the frame), kept inside -- one finger dragging.
    pub fn moved(&self, d: (f32, f32)) -> Region {
        let c = self.centre();
        self.centred_at((c.0 + d.0, c.1 + d.1))
    }

    /// Resized by `k` about its centre, no smaller than MIN and no larger than
    /// the frame -- two fingers pinching.
    pub fn scaled(&self, k: f32) -> Region {
        // nonsense (a zero-spread first frame of a pinch) leaves it exactly as it was
        if !(k.is_finite() && k > 0.0) || k == 1.0 { return *self; }
        let (w, h) = ((self.w() * k).clamp(Region::MIN, 1.0), (self.h() * k).clamp(Region::MIN, 1.0));
        Region { x0: 0.0, y0: 0.0, x1: w, y1: h }.centred_at(self.centre())
    }
}

fn fill(f: &mut Frame, x0: i64, y0: i64, x1: i64, y1: i64, c: [u8; 3]) {
    for y in y0.max(0)..y1.min(f.h as i64) { for x in x0.max(0)..x1.min(f.w as i64) { f.set(x as u32, y as u32, c); } }
}

/// `src`'s `region`, cropped to the panel's aspect around its centre and
/// scaled into a `w`x`h` panel (bilinear).
fn panel_image(src: &Frame, region: Region, w: u32, h: u32) -> Frame {
    let (sw, sh) = (src.w as f32, src.h as f32);
    let (rx0, ry0, rx1, ry1) = (region.x0 * sw, region.y0 * sh, region.x1 * sw, region.y1 * sh);
    let aspect = w as f32 / h as f32;
    let (mut cw, mut ch) = (rx1 - rx0, (rx1 - rx0) / aspect);
    if ch > ry1 - ry0 { ch = ry1 - ry0; cw = ch * aspect; }
    let (cw, ch) = (cw.min(sw), ch.min(sh));
    let cx = ((rx0 + rx1) / 2.0).clamp(cw / 2.0, sw - cw / 2.0);
    let cy = ((ry0 + ry1) / 2.0).clamp(ch / 2.0, sh - ch / 2.0);
    let mut out = Frame::black(w, h);
    for y in 0..h {
        for x in 0..w {
            let sx = cx - cw / 2.0 + (x as f32 + 0.5) * cw / w as f32 - 0.5;
            let sy = cy - ch / 2.0 + (y as f32 + 0.5) * ch / h as f32 - 0.5;
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let px = |xx: f32, yy: f32| src.at(xx.clamp(0.0, sw - 1.0) as u32, yy.clamp(0.0, sh - 1.0) as u32);
            let (a, b, c, d) = (px(x0, y0), px(x0 + 1.0, y0), px(x0, y0 + 1.0), px(x0 + 1.0, y0 + 1.0));
            let mut v = [0u8; 3];
            for k in 0..3 {
                let top = a[k] as f32 * (1.0 - fx) + b[k] as f32 * fx;
                let bot = c[k] as f32 * (1.0 - fx) + d[k] as f32 * fx;
                v[k] = (top * (1.0 - fy) + bot * fy + 0.5) as u8;
            }
            out.set(x, y, v);
        }
    }
    out
}

/// Draw one panel: ink outline, dashed party frame, the picture, white keyline.
fn draw_panel(page: &mut Frame, img: &Frame, x: i64, y: i64, s: &Style, phase: f32) {
    let (w, h, b) = (img.w as i64, img.h as i64, s.border as i64);
    fill(page, x - b - 3, y - b - 3, x + w + b + 3, y + h + b + 3, s.ink);
    // the frame: dashes walking round the band, colours drifting with `phase`
    let dash = 18i64;
    let perim = 2 * (w + h + 4 * b);
    let mut d = 0i64;
    while d < perim {
        let c = s.neon[((d / dash) as usize + phase as usize) % s.neon.len().max(1)];
        let (bx0, by0, bx1, by1) = (x - b, y - b, x + w + b, y + h + b);
        let (ww, hh) = (bx1 - bx0, by1 - by0);
        let t = d;
        let (px, py, horiz) = if t < ww { (bx0 + t, by0, true) }
            else if t < ww + hh { (bx1 - b + 1, by0 + (t - ww), false) }
            else if t < 2 * ww + hh { (bx1 - (t - ww - hh), by1 - b + 1, true) }
            else { (bx0, by1 - (t - 2 * ww - hh), false) };
        if horiz { fill(page, px, py, px + dash - 2, py + b - 1, c); } else { fill(page, px, py, px + b - 1, py + dash - 2, c); }
        d += dash;
    }
    for yy in 0..h { for xx in 0..w {
        let (X, Y) = (x + xx, y + yy);
        if X >= 0 && Y >= 0 && (X as u32) < page.w && (Y as u32) < page.h { page.set(X as u32, Y as u32, img.at(xx as u32, yy as u32)); }
    }}
    // keyline
    for xx in 0..w { for (yy, k) in [(0i64, 0), (h - 1, 0), (1, 0), (h - 2, 0)] { let _ = k;
        if (0..page.w as i64).contains(&(x + xx)) && (0..page.h as i64).contains(&(y + yy)) { page.set((x + xx) as u32, (y + yy) as u32, s.keyline); } } }
    for yy in 0..h { for xx in [0i64, w - 1, 1, w - 2] {
        if (0..page.w as i64).contains(&(x + xx)) && (0..page.h as i64).contains(&(y + yy)) { page.set((x + xx) as u32, (y + yy) as u32, s.keyline); } } }
}

/// The page background: paper and sparse confetti, deterministic per size.
pub fn paper(w: u32, h: u32, s: &Style) -> Frame {
    let mut f = Frame::black(w, h);
    for y in 0..h { for x in 0..w {
        // a faint halftone screen
        let dot = ((x % 7) as i32 - 3).pow(2) + ((y % 7) as i32 - 3).pow(2) < 3;
        let c = if dot { s.paper.map(|v| v.saturating_sub(16)) } else { s.paper };
        f.set(x, y, c);
    }}
    let mut seed = 0x9E37_79B9u64;
    let mut rnd = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
    for _ in 0..s.confetti {
        let (x, y) = ((rnd() % w as u64) as i64, (rnd() % h as u64) as i64);
        let c = s.neon[(rnd() % s.neon.len().max(1) as u64) as usize];
        // muted: mixed 28% into the paper, so it sits behind the panels
        let m = |a: u8, b: u8| ((a as u32 * 72 + b as u32 * 28) / 100) as u8;
        let cm = [m(s.paper[0], c[0]), m(s.paper[1], c[1]), m(s.paper[2], c[2])];
        fill(&mut f, x, y, x + 6, y + 6, cm);
    }
    f
}

/// Draw `layout` into a `w`x`h` frame from the moment's frames.
/// `left`/`right` are the angles showing each side, with their regions.
pub fn compose(layout: Layout, wide: &Frame, left: (&Frame, Region), right: (&Frame, Region),
               page: &Frame, s: &Style, phase: f32) -> Frame {
    let (w, h) = (page.w, page.h);
    let side_of = |sd: Side| if sd == Side::Left { left } else { right };
    match layout {
        Layout::Wide => panel_image(wide, Region::WHOLE, w, h),
        Layout::Page { talking } => {
            let mut out = page.clone();
            // proportions from the prototype, scaled to the frame
            let m = (w as f32 * 0.031) as i64;
            let top_h = (h as f32 * 0.5) as u32;
            draw_panel(&mut out, &panel_image(wide, Region::WHOLE, w - 2 * m as u32, top_h), m, (h as f32 * 0.04) as i64, s, phase);
            let bot_y = (h as f32 * 0.58) as i64;
            let bot_h = (h as f32 * 0.385) as u32;
            let small_w = (w as f32 * 0.33) as u32;
            let big_w = w - 3 * m as u32 - small_w;
            // the talking side is big; each group keeps its own side of the page
            let (lw, rw) = if talking == Side::Left { (big_w, small_w) } else { (small_w, big_w) };
            let (li, lr) = side_of(Side::Left);
            let (ri, rr) = side_of(Side::Right);
            draw_panel(&mut out, &panel_image(li, lr, lw, bot_h), m, bot_y, s, phase);
            draw_panel(&mut out, &panel_image(ri, rr, rw, bot_h), 2 * m + lw as i64, bot_y, s, phase);
            out
        }
        Layout::Inset { side } => {
            let mut out = panel_image(wide, Region::WHOLE, w, h);
            let (iw, ih) = ((w as f32 * 0.31) as u32, (h as f32 * 0.31) as u32);
            let (img, reg) = side_of(side);
            let m = (w as f32 * 0.027) as i64;
            // the inset sits on its people's side of the frame
            let x = if side == Side::Left { m } else { w as i64 - iw as i64 - m };
            draw_panel(&mut out, &panel_image(img, reg, iw, ih), x, h as i64 - ih as i64 - m, s, phase);
            out
        }
        Layout::Split => {
            let half = w / 2;
            let (li, lr) = side_of(Side::Left);
            let (ri, rr) = side_of(Side::Right);
            let (a, b) = (panel_image(li, lr, half, h), panel_image(ri, rr, w - half, h));
            let mut out = Frame::black(w, h);
            for y in 0..h { for x in 0..half { out.set(x, y, a.at(x, y)); } for x in 0..w - half { out.set(half + x, y, b.at(x, y)); } }
            fill(&mut out, half as i64 - 7, 0, half as i64 + 7, h as i64, s.ink);
            let dash = 18u32;
            for k in 0..h / dash + 1 {
                let c1 = s.neon[(k as usize + phase as usize) % s.neon.len()];
                let c2 = s.neon[(k as usize + 2 + phase as usize) % s.neon.len()];
                fill(&mut out, half as i64 - 6, (k * dash) as i64, half as i64 - 3, (k * dash + dash - 2) as i64, c1);
                fill(&mut out, half as i64 + 3, (k * dash) as i64, half as i64 + 6, (k * dash + dash - 2) as i64, c2);
            }
            fill(&mut out, half as i64 - 1, 0, half as i64 + 1, h as i64, s.keyline);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(a: f64, b: f64, side: Side) -> Line { Line { start: a, end: b, side } }

    fn covers(shots: &[Shot], length: f64) {
        assert_eq!(shots[0].start, 0.0);
        assert!((shots.last().unwrap().end - length).abs() < 1e-9, "{shots:?}");
        for w in shots.windows(2) { assert!((w[0].end - w[1].start).abs() < 1e-9, "gap/overlap: {shots:?}"); }
    }

    #[test]
    fn the_rooftop_dialogue_plans_the_way_the_prototype_was_cut() {
        // C0035 30-55 s, as demo time: Max and Jude (right) talk, then the women (left) arrive
        let lines = [line(0.0, 7.86, Side::Right), line(9.56, 13.10, Side::Right), line(13.48, 14.58, Side::Right),
                     line(15.32, 17.12, Side::Right), line(18.96, 20.72, Side::Right), line(21.58, 22.84, Side::Right),
                     line(23.04, 23.46, Side::Right), line(23.82, 25.0, Side::Left)];
        let shots = plan(&lines, 25.0, Rules::default());
        covers(&shots, 25.0);
        assert_eq!(shots[0].layout, Layout::Wide, "open wide");
        assert!(shots[0].end >= 4.0, "for at least `open` seconds");
        assert!(shots.iter().any(|s| s.layout == Layout::Page { talking: Side::Right }),
                "the men's conversation becomes a page: {shots:?}");
        let last = shots.iter().rev().find(|s| s.layout != Layout::Wide).unwrap();
        assert_eq!(last.layout, Layout::Split, "the women joining splits the screen: {shots:?}");
        assert!((last.start - 23.82).abs() < 1e-6, "as their line starts");
    }

    #[test]
    fn no_shot_is_a_blip() {
        let lines = [line(0.0, 7.86, Side::Right), line(9.56, 13.10, Side::Right), line(13.48, 14.58, Side::Right),
                     line(15.32, 17.12, Side::Right), line(18.96, 20.72, Side::Right), line(21.58, 22.84, Side::Right),
                     line(23.04, 23.46, Side::Right), line(23.82, 25.0, Side::Left)];
        let r = Rules::default();
        let shots = plan(&lines, 25.0, r);
        covers(&shots, 25.0);
        // A fixed floor, not r.min_shot: comparing against the rule's own
        // setting let the test pass with the rule switched off (0 >= 0).
        const WATCHABLE: f64 = 0.8;
        for s in &shots[..shots.len() - 1] {
            assert!(s.end - s.start >= WATCHABLE - 1e-9, "a {:.2} s {:?} blip: {shots:?}", s.end - s.start, s.layout);
        }
        for w in shots.windows(2) { assert_ne!(w[0].layout, w[1].layout, "equal neighbours merge: {shots:?}"); }
    }

    #[test]
    fn pages_breathe_back_to_the_wide() {
        let lines = [line(0.0, 30.0, Side::Left)];  // one long monologue
        let shots = plan(&lines, 30.0, Rules { open: 2.0, ..Rules::default() });
        covers(&shots, 30.0);
        for s in &shots {
            if let Layout::Page { .. } = s.layout { assert!(s.end - s.start <= 8.0 + 1e-9, "no page outstays page_max: {s:?}"); }
        }
        let pages = shots.iter().filter(|s| matches!(s.layout, Layout::Page { .. })).count();
        assert!(pages >= 2, "a long run is broken up by the wide: {shots:?}");
    }

    #[test]
    fn a_short_line_gets_an_inset_on_its_speakers_side() {
        let shots = plan(&[line(6.0, 7.5, Side::Left)], 12.0, Rules::default());
        covers(&shots, 12.0);
        assert!(shots.iter().any(|s| s.layout == Layout::Inset { side: Side::Left } && (s.start - 6.0).abs() < 1e-9), "{shots:?}");
    }

    #[test]
    fn silence_is_the_wide() {
        let shots = plan(&[], 10.0, Rules::default());
        assert_eq!(shots, vec![Shot { start: 0.0, end: 10.0, layout: Layout::Wide }]);
    }

    fn solid(w: u32, h: u32, c: [u8; 3]) -> Frame { let mut f = Frame::black(w, h); for y in 0..h { for x in 0..w { f.set(x, y, c); } } f }

    #[test]
    fn each_side_keeps_its_place_on_the_page_and_in_the_split() {
        let (w, h) = (320u32, 180u32);
        let s = Style::default();
        let page = paper(w, h, &s);
        let wide = solid(w, h, [100, 100, 100]);
        let (l, r) = (solid(w, h, [200, 20, 20]), solid(w, h, [20, 20, 200]));
        let reds = |f: &Frame, x0: u32, x1: u32, y0: u32, y1: u32| {
            let mut n = 0; for y in y0..y1 { for x in x0..x1 { let p = f.at(x, y); if p[0] > 150 && p[2] < 60 { n += 1; } } } n };
        let blues = |f: &Frame, x0: u32, x1: u32, y0: u32, y1: u32| {
            let mut n = 0; for y in y0..y1 { for x in x0..x1 { let p = f.at(x, y); if p[2] > 150 && p[0] < 60 { n += 1; } } } n };
        for talking in [Side::Left, Side::Right] {
            let f = compose(Layout::Page { talking }, &wide, (&l, Region::WHOLE), (&r, Region::WHOLE), &page, &s, 0.0);
            let (by0, by1) = ((h as f32 * 0.6) as u32, h - 10);
            assert!(reds(&f, 0, w / 3, by0, by1) > 100, "the left side's panel is on the left ({talking:?} talking)");
            assert!(blues(&f, 2 * w / 3, w, by0, by1) > 100, "the right side's panel is on the right");
            // the INSIDE of the top panel: its own frame is party-coloured, blue
            // dashes included, which a whole-area count mistook for a side's panel
            let (iy0, iy1) = ((h as f32 * 0.04) as u32 + 8, (h as f32 * 0.54) as u32 - 8);
            assert!(reds(&f, 20, w - 20, iy0, iy1) == 0 && blues(&f, 20, w - 20, iy0, iy1) == 0,
                    "the top is the wide, untouched");
        }
        let f = compose(Layout::Split, &wide, (&l, Region::WHOLE), (&r, Region::WHOLE), &page, &s, 0.0);
        assert!(reds(&f, 0, w / 2 - 10, 0, h) > 1000 && blues(&f, w / 2 + 10, w, 0, h) > 1000);
        let f = compose(Layout::Inset { side: Side::Left }, &wide, (&l, Region::WHOLE), (&r, Region::WHOLE), &page, &s, 0.0);
        assert!(reds(&f, 0, w / 2, h / 2, h) > 100 && reds(&f, w / 2, w, 0, h) == 0, "a left inset sits left");
    }

    #[test]
    fn a_region_follows_the_finger_and_stays_in_the_frame() {
        let r = Region { x0: 0.0, y0: 0.05, x1: 0.55, y1: 0.95 };
        // a tap on the far right: centred there, but never past the edge
        let t = r.centred_at((0.95, 0.5));
        assert!((t.x1 - 1.0).abs() < 1e-6 && (t.w() - r.w()).abs() < 1e-6, "{t:?}");
        // dragging left by a quarter frame moves it a quarter frame
        let m = t.moved((-0.25, 0.0));
        assert!((m.centre().0 - (t.centre().0 - 0.25)).abs() < 1e-6, "{m:?}");
        // a drag past the edge stops at the edge, keeping the size
        let far = m.moved((-5.0, 5.0));
        assert!(far.x0.abs() < 1e-6 && (far.y1 - 1.0).abs() < 1e-6 && (far.w() - r.w()).abs() < 1e-6, "{far:?}");
        // a pinch in halves it about its centre; a runaway pinch stops at MIN
        let half = r.scaled(0.5);
        assert!((half.w() - r.w() * 0.5).abs() < 1e-6 && (half.centre().0 - r.centre().0).abs() < 1e-6);
        let tiny = r.scaled(0.01);
        assert!((tiny.w() - Region::MIN).abs() < 1e-6 && (tiny.h() - Region::MIN).abs() < 1e-6);
        // a pinch out never exceeds the frame, and nonsense input changes nothing
        assert!(r.scaled(10.0).w() <= 1.0 + 1e-6 && r.scaled(f32::NAN) == r);
    }

    #[test]
    fn a_region_picks_which_people_a_panel_shows() {
        // left half red, right half blue: a region over the left shows only red
        let (w, h) = (200u32, 100u32);
        let mut src = Frame::black(w, h);
        for y in 0..h { for x in 0..w { src.set(x, y, if x < w / 2 { [220, 10, 10] } else { [10, 10, 220] }); } }
        let img = panel_image(&src, Region { x0: 0.0, y0: 0.0, x1: 0.45, y1: 1.0 }, 60, 60);
        let blue = (0..60).flat_map(|y| (0..60).map(move |x| (x, y))).filter(|&(x, y)| img.at(x, y)[2] > 100).count();
        assert_eq!(blue, 0, "nothing from outside the region");
    }
}
