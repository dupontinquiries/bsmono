//! note -- a page of handwriting that two devices can both edit and then merge without a conflict.
//!
//! A [`Note`] is a SET of strokes plus a set of erased stroke ids (tombstones), which is the simplest structure
//! that merges safely: take the union of both sets of strokes and both sets of erasures, in any order, any
//! number of times, and every device ends with the same page. A stroke's id is `time << 24 | random`, so ids
//! are unique across devices and sorting by id is drawing order (older underneath). The title is the one
//! last-writer-wins field. The binary codec keeps points at 1/16 pixel and sends them as deltas, so a page of
//! writing is a few kilobytes: one or two QR frames.

use ink_kit::stroke::{InkPoint, Stroke, StrokeId, StrokeStyle, ToolKind};
use ink_kit::Vec2d;
use std::collections::{BTreeMap, BTreeSet};

pub const MAGIC: &[u8; 4] = b"JNT1";
/// World units are stored at this many steps per unit.
const Q: f64 = 16.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Note {
    pub id: [u8; 8],
    title: String,
    /// When the title was last set (milliseconds, any device's clock), and by whom (id bytes), to break ties.
    title_at: (u64, [u8; 8]),
    strokes: BTreeMap<u64, Stroke>,
    erased: BTreeSet<u64>,
}

/// A stroke id for a stroke begun at `now_ms` (milliseconds since the Unix epoch is fine) with 24 random bits.
pub fn stroke_id(now_ms: u64, rand24: u32) -> StrokeId { StrokeId((now_ms << 24) | (rand24 as u64 & 0xff_ffff)) }

impl Note {
    pub fn new(id: [u8; 8], title: &str, now_ms: u64, by: [u8; 8]) -> Note {
        Note { id, title: title.to_string(), title_at: (now_ms, by), strokes: BTreeMap::new(), erased: BTreeSet::new() }
    }

    pub fn title(&self) -> &str { &self.title }

    pub fn set_title(&mut self, t: &str, now_ms: u64, by: [u8; 8]) {
        if (now_ms, by) >= self.title_at { self.title = t.to_string(); self.title_at = (now_ms, by); }
    }

    pub fn add(&mut self, s: Stroke) {
        if !self.erased.contains(&s.id.0) { self.strokes.insert(s.id.0, s); }
    }

    /// Erase a stroke. Remembered, so a device that still has it cannot bring it back by syncing.
    pub fn erase(&mut self, id: StrokeId) {
        self.strokes.remove(&id.0);
        self.erased.insert(id.0);
    }

    /// The strokes in drawing order.
    pub fn strokes(&self) -> impl Iterator<Item = &Stroke> { self.strokes.values() }
    pub fn len(&self) -> usize { self.strokes.len() }
    pub fn is_empty(&self) -> bool { self.strokes.is_empty() }

    /// Fold `other` (the same note on another device) in. Commutative, associative and idempotent.
    pub fn merge(&mut self, other: &Note) {
        for id in &other.erased { self.erased.insert(*id); }
        for (id, s) in &other.strokes {
            if !self.erased.contains(id) { self.strokes.entry(*id).or_insert_with(|| s.clone()); }
        }
        let gone: Vec<u64> = self.strokes.keys().filter(|k| self.erased.contains(k)).copied().collect();
        for g in gone { self.strokes.remove(&g); }
        if other.title_at > self.title_at { self.title = other.title.clone(); self.title_at = other.title_at; }
    }

    /// A summary two devices swap first (id, how many strokes and erasures, a hash of the ids), so a sync can skip
    /// notes that are already the same.
    pub fn digest(&self) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mut feed = |x: u64| { for b in x.to_be_bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); } };
        for id in self.strokes.keys() { feed(*id); }
        feed(u64::MAX);
        for id in &self.erased { feed(*id); }
        feed(self.title_at.0);
        h
    }

    // ---- the binary form ---------------------------------------------------------------------

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(MAGIC);
        o.extend_from_slice(&self.id);
        put_str(&mut o, &self.title);
        put_var(&mut o, self.title_at.0);
        o.extend_from_slice(&self.title_at.1);
        put_var(&mut o, self.strokes.len() as u64);
        for s in self.strokes.values() {
            o.extend_from_slice(&s.id.0.to_be_bytes());
            o.push(match s.style.tool { ToolKind::Pen => 0, ToolKind::Finger => 1, ToolKind::Mouse => 2 });
            o.extend_from_slice(&(s.style.width as f32).to_be_bytes());
            o.extend_from_slice(&s.style.color);
            put_var(&mut o, s.points.len() as u64);
            let (mut px, mut py) = (0i64, 0i64);
            for p in &s.points {
                let (x, y) = ((p.pos.x * Q).round() as i64, (p.pos.y * Q).round() as i64);
                put_var(&mut o, zig(x - px));
                put_var(&mut o, zig(y - py));
                o.push((p.pressure.clamp(0.0, 1.0) * 255.0).round() as u8);
                (px, py) = (x, y);
            }
        }
        put_var(&mut o, self.erased.len() as u64);
        for id in &self.erased { o.extend_from_slice(&id.to_be_bytes()); }
        o
    }

    pub fn from_bytes(b: &[u8]) -> Option<Note> {
        let mut r = Rd { b, i: 0 };
        if r.take(4)? != MAGIC { return None; }
        let id: [u8; 8] = r.take(8)?.try_into().ok()?;
        let title = r.str()?;
        let t_at = r.var()?;
        let t_by: [u8; 8] = r.take(8)?.try_into().ok()?;
        let n = r.var()? as usize;
        if n > 2_000_000 { return None; }
        let mut strokes = BTreeMap::new();
        for _ in 0..n {
            let sid = u64::from_be_bytes(r.take(8)?.try_into().ok()?);
            let tool = match r.take(1)?[0] { 0 => ToolKind::Pen, 1 => ToolKind::Finger, 2 => ToolKind::Mouse, _ => return None };
            let width = f32::from_be_bytes(r.take(4)?.try_into().ok()?) as f64;
            let color: [u8; 4] = r.take(4)?.try_into().ok()?;
            let np = r.var()? as usize;
            if np > 5_000_000 { return None; }
            let mut st = Stroke::new(StrokeId(sid), StrokeStyle { width, color, tool });
            let (mut x, mut y) = (0i64, 0i64);
            for _ in 0..np {
                x += unzig(r.var()?);
                y += unzig(r.var()?);
                let pr = r.take(1)?[0] as f32 / 255.0;
                st.points.push(InkPoint::new(Vec2d::new(x as f64 / Q, y as f64 / Q), pr, 0.0));
            }
            strokes.insert(sid, st);
        }
        let ne = r.var()? as usize;
        if ne > 4_000_000 { return None; }
        let mut erased = BTreeSet::new();
        for _ in 0..ne { erased.insert(u64::from_be_bytes(r.take(8)?.try_into().ok()?)); }
        if r.i != b.len() { return None; }
        let strokes: BTreeMap<u64, Stroke> = strokes.into_iter().filter(|(k, _)| !erased.contains(k)).collect();
        Some(Note { id, title, title_at: (t_at, t_by), strokes, erased })
    }
}

fn zig(v: i64) -> u64 { ((v << 1) ^ (v >> 63)) as u64 }
fn unzig(v: u64) -> i64 { ((v >> 1) as i64) ^ -((v & 1) as i64) }

fn put_var(o: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 { o.push((v as u8 & 0x7f) | 0x80); v >>= 7; }
    o.push(v as u8);
}
fn put_str(o: &mut Vec<u8>, s: &str) { put_var(o, s.len() as u64); o.extend_from_slice(s.as_bytes()); }

struct Rd<'a> { b: &'a [u8], i: usize }
impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    fn var(&mut self) -> Option<u64> {
        let (mut v, mut sh) = (0u64, 0u32);
        loop {
            let b = self.take(1)?[0];
            if sh >= 64 { return None; }
            v |= ((b & 0x7f) as u64) << sh;
            if b & 0x80 == 0 { return Some(v); }
            sh += 7;
        }
    }
    fn str(&mut self) -> Option<String> {
        let n = self.var()? as usize;
        if n > 1 << 20 { return None; }
        Some(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: [u8; 8] = [1; 8];
    const YOU: [u8; 8] = [2; 8];

    /// A stroke of `n` points walking from (x0, y0).
    fn stroke(id: u64, x0: f64, y0: f64, n: usize) -> Stroke {
        let mut s = Stroke::new(StrokeId(id), StrokeStyle { width: 2.5, color: [20, 20, 24, 255], tool: ToolKind::Pen });
        for k in 0..n {
            s.points.push(InkPoint::new(Vec2d::new(x0 + k as f64 * 1.37, y0 + (k as f64 * 0.9).sin() * 6.0), 0.2 + (k % 7) as f32 * 0.1, 0.0));
        }
        s
    }

    fn page(strokes: &[Stroke]) -> Note {
        let mut n = Note::new([7; 8], "Meeting", 1000, ME);
        for s in strokes { n.add(s.clone()); }
        n
    }

    #[test]
    fn a_note_round_trips_within_a_sixteenth_of_a_pixel_and_stays_small() {
        let n = page(&[stroke(stroke_id(5_000, 1).0, 10.0, 20.0, 80), stroke(stroke_id(6_000, 2).0, -300.5, 1200.25, 120)]);
        let bytes = n.to_bytes();
        let back = Note::from_bytes(&bytes).expect("decodes");
        assert_eq!((back.id, back.title(), back.len()), (n.id, "Meeting", 2));
        for (a, b) in n.strokes().zip(back.strokes()) {
            assert_eq!((a.id, a.style.tool, a.style.color, a.points.len()), (b.id, b.style.tool, b.style.color, b.points.len()));
            for (p, q) in a.points.iter().zip(&b.points) {
                assert!((p.pos.x - q.pos.x).abs() <= 1.0 / 32.0 + 1e-9 && (p.pos.y - q.pos.y).abs() <= 1.0 / 32.0 + 1e-9, "{:?} vs {:?}", p.pos, q.pos);
                assert!((p.pressure - q.pressure).abs() < 0.01);
            }
        }
        assert!(bytes.len() < 900, "200 points in under a kilobyte: {}", bytes.len());
        // truncation and garbage never panic
        for cut in [0, 3, 10, bytes.len() / 2, bytes.len() - 1] { assert!(Note::from_bytes(&bytes[..cut]).is_none(), "cut at {cut}"); }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(Note::from_bytes(&trailing).is_none(), "trailing bytes are refused");
        assert!(Note::from_bytes(&[0xff; 64]).is_none());
    }

    #[test]
    fn two_devices_that_both_wrote_merge_to_the_same_page_in_any_order() {
        let base = page(&[stroke(10 << 24, 0.0, 0.0, 10)]);
        let mut a = base.clone();
        let mut b = base.clone();
        a.add(stroke((11 << 24) | 1, 5.0, 5.0, 8));
        a.add(stroke((13 << 24) | 2, 9.0, 9.0, 8));
        b.add(stroke((12 << 24) | 3, 7.0, 7.0, 8));
        b.erase(StrokeId(10 << 24));
        let mut ab = a.clone();
        ab.merge(&b);
        let mut ba = b.clone();
        ba.merge(&a);
        assert_eq!(ab.to_bytes(), ba.to_bytes(), "commutative");
        assert_eq!(ab.len(), 3, "a's two and b's one; the erased base stroke is gone from both sides");
        assert!(ab.strokes().all(|s| s.id.0 != 10 << 24));
        let ids: Vec<u64> = ab.strokes().map(|s| s.id.0).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "drawing order is id order: {ids:?}");
        let before = ab.to_bytes();
        ab.merge(&b);
        ab.merge(&a);
        assert_eq!(ab.to_bytes(), before, "idempotent: merging again changes nothing");
        // associative: (a+b)+c == a+(b+c)
        let mut c = base.clone();
        c.add(stroke((14 << 24) | 4, 1.0, 1.0, 4));
        let mut l = a.clone(); l.merge(&b); l.merge(&c);
        let mut bc = b.clone(); bc.merge(&c);
        let mut r = a.clone(); r.merge(&bc);
        assert_eq!(l.to_bytes(), r.to_bytes(), "associative");
        assert_eq!(l.digest(), r.digest());
    }

    #[test]
    fn an_erased_stroke_stays_erased_even_if_a_stale_device_syncs_it_back() {
        let s = stroke(5 << 24, 0.0, 0.0, 5);
        let mut tablet = page(&[s.clone()]);
        let stale_phone = tablet.clone();
        tablet.erase(s.id);
        assert!(tablet.is_empty());
        tablet.merge(&stale_phone);
        assert!(tablet.is_empty(), "the stale copy does not resurrect it");
        let mut phone = stale_phone.clone();
        phone.merge(&tablet);
        assert!(phone.is_empty(), "and the erasure travels to the phone");
        tablet.add(s.clone());
        assert!(tablet.is_empty(), "adding an erased id is ignored");
    }

    #[test]
    fn the_title_is_last_writer_wins_with_a_stable_tiebreak() {
        let mut a = page(&[]);
        let mut b = a.clone();
        a.set_title("Plan A", 2000, ME);
        b.set_title("Plan B", 3000, YOU);
        let (mut ab, mut ba) = (a.clone(), b.clone());
        ab.merge(&b);
        ba.merge(&a);
        assert_eq!((ab.title(), ba.title()), ("Plan B", "Plan B"));
        // the same instant: the higher device id wins on both sides
        let (mut x, mut y) = (page(&[]), page(&[]));
        x.set_title("from me", 5000, ME);
        y.set_title("from you", 5000, YOU);
        let (mut xy, mut yx) = (x.clone(), y.clone());
        xy.merge(&y);
        yx.merge(&x);
        assert_eq!(xy.title(), yx.title());
        a.set_title("older", 10, ME);
        assert_eq!(a.title(), "Plan A", "an older rename does not override a newer one");
    }

    #[test]
    fn the_digest_changes_with_the_page_and_not_with_the_order_it_was_built_in() {
        let (s1, s2) = (stroke(1 << 24, 0.0, 0.0, 3), stroke(2 << 24, 1.0, 1.0, 3));
        let a = page(&[s1.clone(), s2.clone()]);
        let b = page(&[s2.clone(), s1.clone()]);
        assert_eq!(a.digest(), b.digest());
        let mut c = a.clone();
        c.erase(s1.id);
        assert_ne!(a.digest(), c.digest());
        assert_ne!(page(&[]).digest(), a.digest());
    }
}
