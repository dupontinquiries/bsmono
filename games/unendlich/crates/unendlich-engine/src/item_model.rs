//! item_model — a procedural voxel-box model per item class, in a grip-local
//! frame, and the 2D projection the inventory slot draws.
//!
//! Until now an item in the hand was ONE box (`hero_forge`: `[1.7, 5.2, 1.7]`
//! in `held_color`) and an item in a slot was ONE `rect_filled(display_color)`.
//! A sword, a pickaxe, an egg and a block all read as "a coloured stick" in
//! the hand and "a coloured square" in the bag.
//!
//! This module is the one source both consumers draw from: a small list of
//! oriented boxes per [`ItemClass`], coloured from a [`Palette`] the caller
//! derives from the item (metal, element, creature, block). The hand
//! (`hero_forge::build`) emits each box as a rig part at the hand pivot; the
//! slot painter projects the same boxes to a unit square with
//! [`icon_quads`]. Change the sword here and it changes in both places.
//!
//! ## Frame
//!
//! Grip-local, three-space like `hero_forge` (so it drops straight into
//! `Body::part` at the hand pivot): `+Y` runs along the item away from the
//! fist (blade tip, pick head), `+Z` is forward (the direction the edge or
//! head points), `+X` is to the right (thickness). The fist is at the origin;
//! a grip extends a little below it (`-Y`), a pommel below that.
//!
//! Sizes and centres are in VOXEL units (`hero_forge::U` per voxel), so a
//! sword blade at `7.0` is seven hero voxels long. The hand converts centres
//! with `U`; the icon never needs the unit at all.
//!
//! Pure: std-only, no allocation beyond the returned `Vec`s, deterministic,
//! no panics on any input.

/// What shape an item is. Coarser than `ItemKind` on purpose: a copper and an
/// iron pickaxe are the same shape in two palettes, and every legendary blade
/// is a sword whatever its element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemClass {
    Sword,
    Dagger,
    Axe,
    Pickaxe,
    Hammer,
    Spear,
    Bow,
    Staff,
    Shield,
    /// r-legendary: the two-handed blade — longer, wider, a heavier guard.
    Greatsword,
    /// r-legendary: double-bitted, on a long haft.
    Greataxe,
    /// r-legendary: a short rod with a focus stone; casts, does not cut.
    Wand,
    Ingot,
    Egg,
    Block,
    Food,
    Generic,
}

impl ItemClass {
    /// Every class, for sweeps and tests.
    pub const ALL: [ItemClass; 17] = [
        ItemClass::Sword,
        ItemClass::Dagger,
        ItemClass::Axe,
        ItemClass::Pickaxe,
        ItemClass::Hammer,
        ItemClass::Spear,
        ItemClass::Bow,
        ItemClass::Staff,
        ItemClass::Shield,
        ItemClass::Ingot,
        ItemClass::Egg,
        ItemClass::Block,
        ItemClass::Food,
        ItemClass::Generic,
        ItemClass::Greatsword,
        ItemClass::Greataxe,
        ItemClass::Wand,
    ];

    /// The `held_item_pose` class index (0 sword, 1 tool, 2 bow, 3 food, 4
    /// shield, 5 rod/staff, other = fallback), so the hand pose and the model
    /// agree on what kind of thing is being held.
    pub fn pose_class(self) -> u8 {
        match self {
            ItemClass::Sword | ItemClass::Dagger | ItemClass::Greatsword => 0,
            ItemClass::Axe | ItemClass::Pickaxe | ItemClass::Hammer | ItemClass::Greataxe => 1,
            ItemClass::Bow => 2,
            ItemClass::Food => 3,
            ItemClass::Shield => 4,
            ItemClass::Spear | ItemClass::Staff | ItemClass::Wand => 5,
            ItemClass::Ingot | ItemClass::Egg | ItemClass::Block | ItemClass::Generic => 255,
        }
    }

    /// Does this class read as a weapon (for the strike overlay in the rig)?
    pub fn is_weapon(self) -> bool {
        matches!(
            self,
            ItemClass::Sword
                | ItemClass::Dagger
                | ItemClass::Axe
                | ItemClass::Hammer
                | ItemClass::Spear
                | ItemClass::Bow
                | ItemClass::Staff
                | ItemClass::Greatsword
                | ItemClass::Greataxe
                | ItemClass::Wand
        )
    }
}

/// The three colours a model is built from. `primary` is the working part
/// (blade, pick head, ingot body, egg shell); `secondary` an accent (guard,
/// binding, egg speckle, block edge); `grip` the haft/handle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub primary: [f32; 3],
    pub secondary: [f32; 3],
    pub grip: [f32; 3],
}

impl Palette {
    /// A palette from one colour: darker grip, lighter accent. What the
    /// legacy single-colour callers get.
    pub fn mono(c: [f32; 3]) -> Palette {
        Palette { primary: c, secondary: lighten(c, 0.25), grip: darken(c, 0.45) }
    }

    /// Metal head on a wooden haft.
    pub fn tool(metal: [f32; 3]) -> Palette {
        Palette { primary: metal, secondary: darken(metal, 0.2), grip: WOOD }
    }
}

/// A plain oak-ish haft colour.
pub const WOOD: [f32; 3] = [0.45, 0.30, 0.16];
/// Leather-wrapped grip.
pub const LEATHER: [f32; 3] = [0.30, 0.20, 0.12];

fn lighten(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] + (1.0 - c[0]) * k, c[1] + (1.0 - c[1]) * k, c[2] + (1.0 - c[2]) * k]
}

fn darken(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * (1.0 - k), c[1] * (1.0 - k), c[2] * (1.0 - k)]
}

/// One box of a model: `size` (full extents, voxel units), `center` (voxel
/// units, grip-local), `color`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelBox {
    pub size: [f32; 3],
    pub center: [f32; 3],
    pub color: [f32; 3],
}

impl ModelBox {
    pub fn half(&self) -> [f32; 3] {
        [self.size[0] * 0.5, self.size[1] * 0.5, self.size[2] * 0.5]
    }
}

/// A finished model: its class and boxes, base first (grip) so a painter that
/// draws in order overlaps sensibly even before a depth sort.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemModel {
    pub class: ItemClass,
    pub boxes: Vec<ModelBox>,
}

impl ItemModel {
    /// Axis-aligned bounds of the whole model (min, max), voxel units.
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for b in &self.boxes {
            let h = b.half();
            for k in 0..3 {
                lo[k] = lo[k].min(b.center[k] - h[k]);
                hi[k] = hi[k].max(b.center[k] + h[k]);
            }
        }
        if self.boxes.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }
        (lo, hi)
    }

    /// Length along the item's long axis (+Y), voxel units. A sword is long, an
    /// egg is short — the rig uses it to scale the swing arc.
    pub fn length(&self) -> f32 {
        let (lo, hi) = self.bounds();
        hi[1] - lo[1]
    }
}

/// The model for `class` in `pal`. Never empty: `Generic` is the old single
/// box, so a caller with an unmapped item draws exactly what it drew before.
pub fn model_for(class: ItemClass, pal: &Palette) -> ItemModel {
    let mut boxes: Vec<ModelBox> = Vec::with_capacity(6);
    let mut b = |size: [f32; 3], center: [f32; 3], color: [f32; 3]| {
        boxes.push(ModelBox { size, center, color });
    };
    match class {
        ItemClass::Sword => {
            b([1.3, 3.2, 1.3], [0.0, -1.2, 0.0], pal.grip); // grip
            b([1.7, 0.9, 1.7], [0.0, -3.1, 0.0], pal.secondary); // pommel
            b([1.4, 0.8, 4.6], [0.0, 0.7, 0.0], pal.secondary); // crossguard (spans Z)
            b([0.8, 6.4, 1.6], [0.0, 4.3, 0.0], pal.primary); // blade
            b([0.6, 1.2, 1.0], [0.0, 8.1, 0.0], pal.primary); // tip taper
        }
        ItemClass::Dagger => {
            b([1.1, 2.4, 1.1], [0.0, -0.9, 0.0], pal.grip);
            b([1.3, 0.6, 3.0], [0.0, 0.5, 0.0], pal.secondary);
            b([0.7, 3.4, 1.3], [0.0, 2.5, 0.0], pal.primary);
            b([0.5, 0.9, 0.8], [0.0, 4.6, 0.0], pal.primary);
        }
        ItemClass::Axe => {
            b([1.2, 7.0, 1.2], [0.0, 1.5, 0.0], pal.grip); // haft
            b([1.0, 2.6, 1.6], [0.0, 4.4, 1.0], pal.secondary); // eye / poll
            b([0.7, 3.6, 2.8], [0.0, 4.4, 2.6], pal.primary); // bit, forward (+Z)
        }
        ItemClass::Pickaxe => {
            b([1.2, 7.2, 1.2], [0.0, 1.4, 0.0], pal.grip);
            b([1.0, 1.2, 6.4], [0.0, 5.0, 0.0], pal.primary); // head spans Z
            b([0.8, 0.9, 1.4], [0.0, 4.4, 3.3], pal.secondary); // fore point
            b([0.8, 0.9, 1.4], [0.0, 4.4, -3.3], pal.secondary); // aft point
        }
        ItemClass::Hammer => {
            b([1.3, 6.2, 1.3], [0.0, 1.0, 0.0], pal.grip);
            b([2.4, 2.2, 4.4], [0.0, 4.9, 0.0], pal.primary); // head
            b([2.6, 0.5, 4.6], [0.0, 3.6, 0.0], pal.secondary); // band
        }
        ItemClass::Spear => {
            b([1.0, 10.0, 1.0], [0.0, 2.0, 0.0], pal.grip); // long shaft
            b([1.2, 1.0, 1.2], [0.0, 7.2, 0.0], pal.secondary); // socket
            b([0.7, 3.4, 1.6], [0.0, 9.3, 0.0], pal.primary); // leaf head
        }
        ItemClass::Bow => {
            b([1.1, 2.6, 1.1], [0.0, 0.0, 0.0], pal.grip); // handle
            b([0.9, 4.2, 1.0], [0.0, 3.2, 0.9], pal.primary); // upper limb, bows forward
            b([0.9, 4.2, 1.0], [0.0, -3.2, 0.9], pal.primary); // lower limb
            b([0.25, 10.2, 0.25], [0.0, 0.0, -0.9], pal.secondary); // string, behind
        }
        ItemClass::Staff => {
            b([1.1, 11.0, 1.1], [0.0, 2.5, 0.0], pal.grip);
            b([1.8, 1.8, 1.8], [0.0, 8.8, 0.0], pal.primary); // focus stone
            b([1.4, 0.8, 1.4], [0.0, 7.5, 0.0], pal.secondary); // collar
        }
        ItemClass::Shield => {
            b([0.9, 7.0, 5.6], [0.0, 1.0, 0.0], pal.primary); // face (spans Z)
            b([0.5, 7.4, 6.0], [-0.5, 1.0, 0.0], pal.secondary); // rim, on the arm side
            b([1.6, 1.6, 1.6], [0.9, 1.0, 0.0], pal.secondary); // boss
            b([1.0, 1.4, 1.0], [-1.0, 1.0, 0.0], pal.grip); // handle
        }
        ItemClass::Ingot => {
            b([2.2, 1.4, 4.4], [0.0, 0.0, 0.0], pal.primary); // bar
            b([1.8, 0.5, 4.0], [0.0, 0.9, 0.0], pal.secondary); // top face, lighter
        }
        ItemClass::Egg => {
            b([2.2, 2.2, 2.2], [0.0, 0.0, 0.0], pal.primary); // body
            b([1.6, 1.4, 1.6], [0.0, 1.5, 0.0], pal.primary); // narrow top
            b([0.6, 0.6, 0.6], [0.9, 0.3, 0.7], pal.secondary); // speckle
            b([0.6, 0.6, 0.6], [-0.7, -0.5, -0.8], pal.secondary); // speckle
        }
        ItemClass::Block => {
            b([3.0, 3.0, 3.0], [0.0, 0.0, 0.0], pal.primary);
            b([3.2, 0.4, 3.2], [0.0, 1.4, 0.0], pal.secondary); // lit top edge
        }
        ItemClass::Food => {
            b([1.8, 1.6, 2.8], [0.0, 0.0, 0.0], pal.primary); // haunch
            b([0.6, 0.6, 2.2], [0.0, 0.2, 2.2], pal.secondary); // bone
        }
        ItemClass::Generic => {
            // The pre-model held item, exactly: one tall box.
            b([1.7, 5.2, 1.7], [0.0, 0.0, 0.0], pal.primary);
        }
        ItemClass::Greatsword => {
            b([1.4, 4.6, 1.4], [0.0, -1.8, 0.0], pal.grip); // two-hand grip
            b([2.0, 1.0, 2.0], [0.0, -4.4, 0.0], pal.secondary); // heavy pommel
            b([1.6, 1.0, 6.2], [0.0, 0.8, 0.0], pal.secondary); // wide guard
            b([1.0, 9.4, 2.2], [0.0, 6.0, 0.0], pal.primary); // broad blade
            b([0.8, 1.6, 1.4], [0.0, 11.4, 0.0], pal.primary); // tip
        }
        ItemClass::Greataxe => {
            b([1.3, 9.0, 1.3], [0.0, 2.0, 0.0], pal.grip); // long haft
            b([1.1, 3.0, 1.8], [0.0, 5.8, 0.0], pal.secondary); // eye
            b([0.8, 4.4, 3.0], [0.0, 5.8, 2.7], pal.primary); // fore bit
            b([0.8, 4.4, 3.0], [0.0, 5.8, -2.7], pal.primary); // aft bit (double)
        }
        ItemClass::Wand => {
            b([0.9, 3.4, 0.9], [0.0, 0.4, 0.0], pal.grip); // short rod
            b([1.3, 1.3, 1.3], [0.0, 2.8, 0.0], pal.primary); // focus stone
            b([1.1, 0.5, 1.1], [0.0, 1.9, 0.0], pal.secondary); // collar
            b([0.7, 0.7, 0.7], [0.0, 3.7, 0.0], pal.secondary); // tip spark
        }
    }
    ItemModel { class, boxes }
}

/// A projected box for the slot painter: a convex quad in the `[0,1]` icon
/// square (`x` right, `y` up), with a back-to-front `depth` key and its colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IconQuad {
    pub pts: [[f32; 2]; 4],
    pub depth: f32,
    pub color: [f32; 3],
}

/// Project a model to icon quads: view straight at the item's `+Z` face
/// (the plane a sword's edge lies in, so a blade reads as a blade), tilted
/// `tilt` radians counter-clockwise about the view axis — the classic
/// diagonal weapon icon at ~0.6, upright at 0. Long-axis `+Y` maps to icon
/// up before the tilt. The whole model is fitted to the unit square with
/// `margin` on each side, aspect preserved. Quads come back sorted far to
/// near so a painter drawing in order overlaps correctly.
pub fn icon_quads(model: &ItemModel, tilt: f32, margin: f32) -> Vec<IconQuad> {
    let (s, c) = tilt.sin_cos();
    // Each box → its front-face rectangle in (u, v) = (X, Y), rotated by tilt.
    let mut quads: Vec<IconQuad> = Vec::with_capacity(model.boxes.len());
    let mut lo = [f32::MAX; 2];
    let mut hi = [f32::MIN; 2];
    for b in &model.boxes {
        let h = b.half();
        // Viewed along -Z: horizontal = X (right), vertical = Y (up); the box's
        // Z extent adds a little pseudo-depth so a pick head reads thicker.
        let corners = [
            [b.center[0] - h[0], b.center[1] - h[1]],
            [b.center[0] + h[0], b.center[1] - h[1]],
            [b.center[0] + h[0], b.center[1] + h[1]],
            [b.center[0] - h[0], b.center[1] + h[1]],
        ];
        let mut pts = [[0.0f32; 2]; 4];
        for (i, p) in corners.iter().enumerate() {
            let u = p[0] * c - p[1] * s;
            let v = p[0] * s + p[1] * c;
            pts[i] = [u, v];
            lo[0] = lo[0].min(u);
            lo[1] = lo[1].min(v);
            hi[0] = hi[0].max(u);
            hi[1] = hi[1].max(v);
        }
        // Boxes that stick out toward the viewer (+Z) are nearer; ties keep
        // model order, so the grip drawn first stays under the guard.
        quads.push(IconQuad { pts, depth: b.center[2] + h[2], color: b.color });
    }
    if quads.is_empty() {
        return quads;
    }
    let m = margin.clamp(0.0, 0.45);
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-3);
    let inner = 1.0 - 2.0 * m;
    let offx = (span - (hi[0] - lo[0])) * 0.5;
    let offy = (span - (hi[1] - lo[1])) * 0.5;
    for q in quads.iter_mut() {
        for p in q.pts.iter_mut() {
            p[0] = m + (p[0] - lo[0] + offx) / span * inner;
            p[1] = m + (p[1] - lo[1] + offy) / span * inner;
        }
    }
    // Stable sort: equal depth keeps emission order.
    quads.sort_by(|a, b| a.depth.partial_cmp(&b.depth).unwrap_or(std::cmp::Ordering::Equal));
    quads
}

/// The conventional icon tilt for a class: weapons and tools lie diagonally
/// (tip up-right), everything else sits upright.
pub fn icon_tilt(class: ItemClass) -> f32 {
    match class {
        ItemClass::Sword
        | ItemClass::Dagger
        | ItemClass::Axe
        | ItemClass::Pickaxe
        | ItemClass::Hammer
        | ItemClass::Spear
        | ItemClass::Staff
        | ItemClass::Greatsword
        | ItemClass::Greataxe
        | ItemClass::Wand => -0.65,
        ItemClass::Bow => -0.35,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAL: Palette = Palette { primary: [0.8, 0.8, 0.85], secondary: [0.6, 0.5, 0.2], grip: [0.3, 0.2, 0.1] };

    #[test]
    fn every_class_has_a_model_and_generic_is_the_legacy_box() {
        for c in ItemClass::ALL {
            let m = model_for(c, &PAL);
            assert!(!m.boxes.is_empty(), "{c:?} has no boxes");
            assert_eq!(m.class, c);
            for b in &m.boxes {
                assert!(b.size.iter().all(|&s| s > 0.0), "{c:?} zero-size box {b:?}");
                assert!(b.center.iter().chain(b.size.iter()).all(|v| v.is_finite()));
            }
        }
        let g = model_for(ItemClass::Generic, &PAL);
        assert_eq!(g.boxes.len(), 1);
        assert_eq!(g.boxes[0].size, [1.7, 5.2, 1.7], "Generic must stay the pre-model held box");
        assert_eq!(g.boxes[0].color, PAL.primary);
    }

    #[test]
    fn classes_are_visually_distinct() {
        // Two classes with the same box list would be the "coloured stick"
        // problem in a new coat. Compare the sorted size lists.
        let mut sigs: Vec<(ItemClass, Vec<[i32; 3]>)> = Vec::new();
        for c in ItemClass::ALL {
            let mut sizes: Vec<[i32; 3]> = model_for(c, &PAL)
                .boxes
                .iter()
                .map(|b| [(b.size[0] * 10.0) as i32, (b.size[1] * 10.0) as i32, (b.size[2] * 10.0) as i32])
                .collect();
            sizes.sort();
            for (oc, os) in &sigs {
                assert_ne!(&sizes, os, "{c:?} and {oc:?} share a box list");
            }
            sigs.push((c, sizes));
        }
    }

    #[test]
    fn long_weapons_are_long_and_a_sword_extends_above_the_fist() {
        let sword = model_for(ItemClass::Sword, &PAL);
        let egg = model_for(ItemClass::Egg, &PAL);
        let spear = model_for(ItemClass::Spear, &PAL);
        assert!(sword.length() > 2.0 * egg.length(), "sword {} vs egg {}", sword.length(), egg.length());
        assert!(spear.length() > sword.length());
        // the two-handers outsize their one-hand kin; a wand is shorter than a staff
        assert!(model_for(ItemClass::Greatsword, &PAL).length() > sword.length() * 1.3);
        assert!(model_for(ItemClass::Greataxe, &PAL).length() > model_for(ItemClass::Axe, &PAL).length());
        assert!(model_for(ItemClass::Wand, &PAL).length() < model_for(ItemClass::Staff, &PAL).length() * 0.5);
        let (_, hi) = sword.bounds();
        assert!(hi[1] > 6.0, "blade tip should be well above the fist, got {}", hi[1]);
        // and the blade is the primary colour, the grip the grip colour
        assert!(sword.boxes.iter().any(|b| b.color == PAL.primary && b.center[1] > 2.0));
        assert!(sword.boxes.iter().any(|b| b.color == PAL.grip && b.center[1] < 0.0));
    }

    #[test]
    fn tool_heads_point_forward_and_span_the_swing_plane() {
        // An axe bit sits forward (+Z) of the haft; a pick head spans Z.
        let axe = model_for(ItemClass::Axe, &PAL);
        let bit = axe.boxes.iter().max_by(|a, b| a.center[2].partial_cmp(&b.center[2]).unwrap()).unwrap();
        assert!(bit.center[2] > 1.5 && bit.color == PAL.primary, "axe bit forward and metal: {bit:?}");
        let pick = model_for(ItemClass::Pickaxe, &PAL);
        let head = pick.boxes.iter().max_by(|a, b| a.size[2].partial_cmp(&b.size[2]).unwrap()).unwrap();
        assert!(head.size[2] > head.size[0] * 3.0, "pick head spans Z: {head:?}");
    }

    #[test]
    fn icon_quads_fit_the_unit_square_with_margin_and_sort_back_to_front() {
        for c in ItemClass::ALL {
            let m = model_for(c, &PAL);
            for tilt in [0.0, icon_tilt(c), 1.2] {
                let q = icon_quads(&m, tilt, 0.1);
                assert_eq!(q.len(), m.boxes.len(), "{c:?}: one quad per box");
                let mut lo = [f32::MAX; 2];
                let mut hi = [f32::MIN; 2];
                for quad in &q {
                    for p in quad.pts {
                        for k in 0..2 {
                            assert!(p[k] >= 0.1 - 1e-4 && p[k] <= 0.9 + 1e-4, "{c:?} tilt {tilt}: point {p:?} outside margin");
                            lo[k] = lo[k].min(p[k]);
                            hi[k] = hi[k].max(p[k]);
                        }
                    }
                }
                // Fills the inner square on its longer axis (aspect preserved).
                let w = hi[0] - lo[0];
                let h = hi[1] - lo[1];
                assert!((w.max(h) - 0.8).abs() < 1e-3, "{c:?} tilt {tilt}: fitted span {w}x{h}, want 0.8");
                for w in q.windows(2) {
                    assert!(w[0].depth <= w[1].depth, "not sorted back-to-front");
                }
            }
        }
    }

    #[test]
    fn upright_icon_keeps_the_grip_below_the_blade() {
        // At tilt 0, +Y is icon up: the grip quad's centre is below the blade's.
        let m = model_for(ItemClass::Sword, &PAL);
        let q = icon_quads(&m, 0.0, 0.05);
        let cy = |quad: &IconQuad| quad.pts.iter().map(|p| p[1]).sum::<f32>() / 4.0;
        let grip = q.iter().find(|x| x.color == PAL.grip).unwrap();
        let blade = q.iter().filter(|x| x.color == PAL.primary).max_by(|a, b| cy(a).partial_cmp(&cy(b)).unwrap()).unwrap();
        assert!(cy(grip) < cy(blade));
    }

    #[test]
    fn tilt_rotates_counter_clockwise_and_negative_tilt_leans_the_tip_right() {
        let m = model_for(ItemClass::Sword, &PAL);
        let q = icon_quads(&m, icon_tilt(ItemClass::Sword), 0.05);
        let cx = |quad: &IconQuad| quad.pts.iter().map(|p| p[0]).sum::<f32>() / 4.0;
        let grip = q.iter().find(|x| x.color == PAL.grip).unwrap();
        let tip = q.iter().filter(|x| x.color == PAL.primary).max_by(|a, b| a.pts[0][1].partial_cmp(&b.pts[0][1]).unwrap()).unwrap();
        assert!(cx(tip) > cx(grip), "the conventional weapon icon points up-right");
    }

    #[test]
    fn empty_model_projects_to_nothing_and_bounds_are_zero() {
        let m = ItemModel { class: ItemClass::Generic, boxes: Vec::new() };
        assert!(icon_quads(&m, 0.3, 0.1).is_empty());
        assert_eq!(m.bounds(), ([0.0; 3], [0.0; 3]));
        assert_eq!(m.length(), 0.0);
    }

    #[test]
    fn palettes_derive_darker_grips_and_pose_classes_match_held_item_pose() {
        let p = Palette::mono([0.8, 0.4, 0.2]);
        assert!(p.grip.iter().zip(p.primary.iter()).all(|(g, c)| g < c));
        assert!(p.secondary.iter().zip(p.primary.iter()).all(|(s, c)| s >= c));
        let t = Palette::tool([0.7, 0.7, 0.75]);
        assert_eq!(t.grip, WOOD);
        // held_item_pose::NUM_CLASSES is 6: every mapped class is below it,
        // the unmapped ones use the fallback.
        for c in ItemClass::ALL {
            let pc = c.pose_class();
            assert!(pc < 6 || pc == 255, "{c:?} -> {pc}");
        }
        assert_eq!(ItemClass::Sword.pose_class(), 0);
        assert_eq!(ItemClass::Pickaxe.pose_class(), 1);
        assert_eq!(ItemClass::Bow.pose_class(), 2);
        assert!(ItemClass::Sword.is_weapon() && !ItemClass::Egg.is_weapon());
    }
}
