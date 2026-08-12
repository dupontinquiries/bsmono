//! The document: strokes + a uniform-grid spatial index + undo.
//!
//! The index is a coarse hash grid keyed by world cell; a stroke registers in
//! every cell its visual bbox overlaps. That's enough for (a) view culling and
//! (b) eraser hit-tests without going O(strokes) per query. Cell size is fixed
//! at construction: pick roughly the size of a typical stroke bbox at your
//! canonical zoom (128 world units default). Extremes degrade gracefully —
//! a huge stroke just registers in many cells.
//!
//! Undo is op-based (Add/Remove of whole strokes), matching how ink apps feel:
//! one stroke = one undo step; an eraser drag that removes N strokes = one
//! step. No partial-stroke edits in v1 — that's a deliberate cut, and the
//! `UndoOp` enum is the seam where segment-erase would land.

use crate::stroke::{Stroke, StrokeId};
use crate::{RectD, Vec2d};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub enum UndoOp {
    Add(StrokeId),
    /// Full stroke payloads so redo of an erase can restore them.
    Remove(Vec<Stroke>),
}

pub struct Canvas {
    strokes: HashMap<StrokeId, Stroke>,
    order: Vec<StrokeId>, // draw order (insertion)
    grid: HashMap<(i64, i64), Vec<StrokeId>>,
    cell: f64,
    next_id: u64,
    undo: Vec<UndoOp>,
    redo: Vec<(UndoOp, Vec<Stroke>)>, // op + payload needed to re-apply
}

impl Canvas {
    pub fn new() -> Self {
        Self::with_cell_size(128.0)
    }
    pub fn with_cell_size(cell: f64) -> Self {
        Canvas {
            strokes: HashMap::new(),
            order: Vec::new(),
            grid: HashMap::new(),
            cell: cell.max(1e-6),
            next_id: 1,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn alloc_id(&mut self) -> StrokeId {
        let id = StrokeId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
    pub fn get(&self, id: StrokeId) -> Option<&Stroke> {
        self.strokes.get(&id)
    }
    /// Strokes in draw order.
    pub fn iter(&self) -> impl Iterator<Item = &Stroke> {
        self.order.iter().filter_map(move |id| self.strokes.get(id))
    }

    fn cells_of(&self, r: &RectD) -> impl Iterator<Item = (i64, i64)> {
        let (cx0, cy0, cx1, cy1) = if r.is_empty() {
            (0, 0, -1, -1) // empty range
        } else {
            (
                (r.min.x / self.cell).floor() as i64,
                (r.min.y / self.cell).floor() as i64,
                (r.max.x / self.cell).floor() as i64,
                (r.max.y / self.cell).floor() as i64,
            )
        };
        (cx0..=cx1).flat_map(move |cx| (cy0..=cy1).map(move |cy| (cx, cy)))
    }

    fn index_insert(&mut self, id: StrokeId, bbox: &RectD) {
        let cells: Vec<_> = self.cells_of(bbox).collect();
        for c in cells {
            self.grid.entry(c).or_default().push(id);
        }
    }
    fn index_remove(&mut self, id: StrokeId, bbox: &RectD) {
        let cells: Vec<_> = self.cells_of(bbox).collect();
        for c in cells {
            if let Entry::Occupied(mut e) = self.grid.entry(c) {
                e.get_mut().retain(|s| *s != id);
                if e.get().is_empty() {
                    e.remove();
                }
            }
        }
    }

    /// Add a finished stroke (empty/1-point strokes are accepted; dots are
    /// legitimate ink). Records one undo step and clears redo.
    pub fn add(&mut self, stroke: Stroke) {
        let id = stroke.id;
        let bbox = stroke.visual_bbox();
        debug_assert!(!self.strokes.contains_key(&id), "duplicate StrokeId");
        self.strokes.insert(id, stroke);
        self.order.push(id);
        if !bbox.is_empty() {
            self.index_insert(id, &bbox);
        }
        self.undo.push(UndoOp::Add(id));
        self.redo.clear();
    }

    fn remove_internal(&mut self, id: StrokeId) -> Option<Stroke> {
        let s = self.strokes.remove(&id)?;
        self.order.retain(|x| *x != id);
        let bbox = s.visual_bbox();
        if !bbox.is_empty() {
            self.index_remove(id, &bbox);
        }
        Some(s)
    }

    /// Erase every stroke within `radius` (world units) of `p`. Returns the
    /// number removed; the whole batch is one undo step (call once per eraser
    /// *drag sample*; merge steps at a higher level if you want one step per
    /// drag — see [`Canvas::erase_batch_begin`] pattern in the egui sketch).
    pub fn erase_at(&mut self, p: Vec2d, radius: f64) -> usize {
        let probe = RectD {
            min: Vec2d::new(p.x - radius, p.y - radius),
            max: Vec2d::new(p.x + radius, p.y + radius),
        };
        let mut candidates: HashSet<StrokeId> = HashSet::new();
        for c in self.cells_of(&probe) {
            if let Some(v) = self.grid.get(&c) {
                candidates.extend(v.iter().copied());
            }
        }
        let mut hit: Vec<StrokeId> = candidates
            .into_iter()
            .filter(|id| {
                self.strokes
                    .get(id)
                    .map(|s| s.dist_to(p) <= radius + s.style.width * 0.5)
                    .unwrap_or(false)
            })
            .collect();
        hit.sort(); // determinism
        if hit.is_empty() {
            return 0;
        }
        let removed: Vec<Stroke> = hit
            .iter()
            .filter_map(|id| self.remove_internal(*id))
            .collect();
        let n = removed.len();
        self.undo.push(UndoOp::Remove(removed));
        self.redo.clear();
        n
    }

    /// Strokes whose visual bbox intersects `view`, in draw order (culling).
    pub fn visible(&self, view: &RectD) -> Vec<&Stroke> {
        let mut ids: HashSet<StrokeId> = HashSet::new();
        for c in self.cells_of(view) {
            if let Some(v) = self.grid.get(&c) {
                ids.extend(v.iter().copied());
            }
        }
        self.order
            .iter()
            .filter(|id| ids.contains(id))
            .filter_map(|id| self.strokes.get(id))
            .filter(|s| s.visual_bbox().intersects(view))
            .collect()
    }

    /// Bounds of the whole drawing (visual).
    pub fn bounds(&self) -> RectD {
        let mut r = RectD::EMPTY;
        for s in self.iter() {
            let b = s.visual_bbox();
            if !b.is_empty() {
                r.include(b.min);
                r.include(b.max);
            }
        }
        r
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        match self.undo.pop() {
            None => false,
            Some(UndoOp::Add(id)) => {
                if let Some(s) = self.remove_internal(id) {
                    self.redo.push((UndoOp::Add(id), vec![s]));
                }
                true
            }
            Some(UndoOp::Remove(strokes)) => {
                let ids: Vec<StrokeId> = strokes.iter().map(|s| s.id).collect();
                for s in &strokes {
                    let bbox = s.visual_bbox();
                    self.strokes.insert(s.id, s.clone());
                    self.order.push(s.id);
                    if !bbox.is_empty() {
                        self.index_insert(s.id, &bbox);
                    }
                }
                self.redo.push((
                    UndoOp::Remove(strokes),
                    ids.iter().filter_map(|i| self.strokes.get(i).cloned()).collect(),
                ));
                true
            }
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.redo.pop() {
            None => false,
            Some((UndoOp::Add(_), mut payload)) => {
                // re-apply the add
                if let Some(s) = payload.pop() {
                    let id = s.id;
                    let bbox = s.visual_bbox();
                    self.strokes.insert(id, s);
                    self.order.push(id);
                    if !bbox.is_empty() {
                        self.index_insert(id, &bbox);
                    }
                    self.undo.push(UndoOp::Add(id));
                }
                true
            }
            Some((UndoOp::Remove(strokes), _)) => {
                let mut removed = Vec::new();
                for s in &strokes {
                    if let Some(x) = self.remove_internal(s.id) {
                        removed.push(x);
                    }
                }
                self.undo.push(UndoOp::Remove(removed));
                true
            }
        }
    }
}

impl Default for Canvas {
    fn default() -> Self {
        Canvas::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{InkPoint, StrokeStyle};

    fn line(c: &mut Canvas, x0: f64, x1: f64, y: f64) -> StrokeId {
        let id = c.alloc_id();
        let mut s = Stroke::new(id, StrokeStyle::default());
        s.points.push(InkPoint::xy(x0, y));
        s.points.push(InkPoint::xy(x1, y));
        c.add(s);
        id
    }

    #[test]
    fn erase_hits_only_nearby() {
        let mut c = Canvas::new();
        let _a = line(&mut c, 0.0, 100.0, 0.0);
        let _b = line(&mut c, 0.0, 100.0, 500.0);
        assert_eq!(c.erase_at(Vec2d::new(50.0, 3.0), 5.0), 1);
        assert_eq!(c.len(), 1);
        assert_eq!(c.iter().next().unwrap().points[0].pos.y, 500.0);
    }

    #[test]
    fn undo_redo_add_and_erase() {
        let mut c = Canvas::new();
        line(&mut c, 0.0, 10.0, 0.0);
        line(&mut c, 0.0, 10.0, 50.0);
        assert_eq!(c.len(), 2);
        c.erase_at(Vec2d::new(5.0, 0.0), 3.0);
        assert_eq!(c.len(), 1);
        assert!(c.undo()); // un-erase
        assert_eq!(c.len(), 2);
        assert!(c.undo()); // un-add second line
        assert_eq!(c.len(), 1);
        assert!(c.redo());
        assert_eq!(c.len(), 2);
        assert!(c.redo()); // re-erase
        assert_eq!(c.len(), 1);
        // index consistency after churn: erase remaining
        assert_eq!(c.erase_at(Vec2d::new(5.0, 50.0), 3.0), 1);
        assert!(c.is_empty());
    }

    #[test]
    fn visible_culls() {
        let mut c = Canvas::new();
        line(&mut c, 0.0, 10.0, 0.0);
        line(&mut c, 100000.0, 100010.0, 0.0);
        let view = RectD {
            min: Vec2d::new(-50.0, -50.0),
            max: Vec2d::new(50.0, 50.0),
        };
        let v = c.visible(&view);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].points[0].pos.x, 0.0);
    }

    #[test]
    fn giant_stroke_still_indexed() {
        let mut c = Canvas::with_cell_size(16.0);
        line(&mut c, -4000.0, 4000.0, 0.0);
        assert_eq!(c.erase_at(Vec2d::new(3999.0, 0.0), 4.0), 1);
    }
}
