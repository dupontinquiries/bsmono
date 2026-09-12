//! ROOMPLAN — laying out rooms from the ship's OWN shape.
//!
//! `Interior::generate` builds every ship from a single number. It takes
//! a length, derives `hull_half` from fixed ratios
//! (`0.16 / 0.09 / 0.5`), and drops `content::HUB`'s eight rooms into
//! that box at a uniform scale. Every ship in the game is therefore the
//! same ship at a different size: same eight rooms, same relative
//! positions, same proportions. That is why cycling hulls changes the
//! scale of the interior and nothing else about it.
//!
//! A real hull is not a box. It tapers, it has a fat midsection and a
//! thin tail, and where the rooms can GO is a consequence of that shape.
//! This module is that consequence: give it an `Envelope` — the usable
//! half-width and half-height sampled along the ship's length — and it
//! lays out a spine and as many rooms as the shape can actually hold,
//! sized to the local cross-section rather than to a global constant.
//!
//! ## Where an envelope comes from, honestly
//!
//! Three sources, in descending order of how real they are:
//!
//! * [`from_cloud`] / [`Envelope::from_model_verts`] — sampled from a
//!   loaded model's actual surface points (`Model::cloud`, downsampled
//!   geometry in final metres). This is the real one, and it is what
//!   "the roomplan is based on the spaceship model" means: a hull that
//!   pinches at the waist produces a plan that pinches at the waist.
//! * [`Envelope::from_profile`] — from an existing `hullform::Profile`.
//!   Useful for the procedural loft, but note `Profile::fit` derives
//!   itself FROM cells, so planning rooms from it is circular; it is
//!   here for comparison and for tests, not as the intended path.
//! * [`Envelope::from_extents`] — a plain tapered box from a bounding
//!   box. The fallback when nothing better is available, and no worse
//!   than what `Interior::generate` does today.
//!
//! ## What this module does NOT do
//!
//! It does not build an `Interior`. It produces a PLAN — ids, kinds,
//! centres and half-extents — and stops there. Turning a plan into
//! cells, links, doors and an SDF is `interior.rs`'s job and it already
//! knows how; handing it a plan it did not hardcode is the seam. Keeping
//! the two apart is what lets every rule below be tested without a hull,
//! a GPU or a loaded asset.

use crate::content::RoomKind;
use glam::Vec3;

/// How many stations along the length an envelope samples. Matches
/// `hullform::STATIONS` so the two can be compared without resampling.
pub const STATIONS: usize = 24;

/// The usable interior cross-section along a hull, stern (index 0) to
/// bow. Half-extents, in metres, in hull-local space.
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub half_z: f32,
    /// half-width at each station
    pub w: [f32; STATIONS],
    /// half-height at each station
    pub h: [f32; STATIONS],
}

/// Local z of station `i`.
pub fn station_z(i: usize, half_z: f32) -> f32 {
    -half_z + 2.0 * half_z * (i as f32 / (STATIONS - 1).max(1) as f32)
}

/// Sample an envelope from a loaded model's surface point cloud.
///
/// **This is the real "roomplan from the spaceship model" path.**
/// `model::Model` keeps `cloud: Vec<Vec3>` — downsampled surface points
/// in final metres, the same data the reflection SDF bake uses — which
/// is CPU-side and available the moment a ship model loads.
///
/// A correction worth recording: an earlier pass here claimed `Model`
/// keeps its raw vertices and built against that. It does not. `verts`
/// belongs to `RawPrim`, an intermediate thrown away after load; the
/// cloud is what survives. The distinction matters because the cloud is
/// DOWNSAMPLED, so a station with few cloud points is undersampled
/// rather than empty — which is exactly the gap-filling case
/// `from_model_verts` already handles, and why this delegates to it
/// instead of sampling differently.
pub fn from_cloud(cloud: &[Vec3], inset: f32) -> Option<Envelope> {
    let pts: Vec<[f32; 3]> = cloud.iter().map(|p| p.to_array()).collect();
    Envelope::from_model_verts(&pts, inset)
}

impl Envelope {
    /// A plain box with a nose/tail taper — the honest fallback.
    ///
    /// `taper` is how much of the half-length at each end narrows toward
    /// a point; 0 gives a pure box, which is what `Interior::generate`
    /// effectively assumes today.
    pub fn from_extents(half_z: f32, half_w: f32, half_h: f32, taper: f32) -> Envelope {
        let half_z = half_z.max(0.01);
        let taper = taper.clamp(0.0, 0.95);
        let mut w = [0.0f32; STATIONS];
        let mut h = [0.0f32; STATIONS];
        for i in 0..STATIONS {
            // 0 at the ends, 1 across the middle
            let t = (i as f32 / (STATIONS - 1).max(1) as f32) * 2.0 - 1.0; // -1..1
            let shape = 1.0 - taper * t.abs().powf(2.0);
            w[i] = (half_w * shape).max(0.001);
            h[i] = (half_h * shape).max(0.001);
        }
        Envelope { half_z, w, h }
    }

    /// From a fitted `hullform::Profile`.
    ///
    /// Circular if used to plan the rooms that profile was fitted
    /// around — see the module doc. Provided for comparison.
    pub fn from_profile(p: &crate::hullform::Profile) -> Envelope {
        let mut w = [0.0f32; STATIONS];
        let mut h = [0.0f32; STATIONS];
        for i in 0..STATIONS {
            let z = station_z(i, p.half_z);
            let (a, b) = p.at(z);
            w[i] = a.max(0.001);
            h[i] = b.max(0.001);
        }
        Envelope { half_z: p.half_z, w, h }
    }

    /// THE REAL ONE: sample the actual model geometry.
    ///
    /// Bins vertices by z and takes the extreme |x| and |y| in each bin,
    /// so the envelope follows the ship's true silhouette. `inset` pulls
    /// the interior in from the skin (plating, structure) as a fraction
    /// of the local half-extent — an interior flush with the hull would
    /// put rooms inside the plating.
    ///
    /// Empty bins are filled from their nearest non-empty neighbour
    /// rather than left at zero: a gap in the vertex distribution is a
    /// sampling artefact, not a hole in the ship, and leaving it zero
    /// would cut the hull in half at that station.
    pub fn from_model_verts(verts: &[[f32; 3]], inset: f32) -> Option<Envelope> {
        if verts.is_empty() {
            return None;
        }
        let half_z = verts.iter().fold(0.0f32, |m, v| m.max(v[2].abs())).max(0.01);
        let inset = inset.clamp(0.0, 0.9);
        let mut w = [0.0f32; STATIONS];
        let mut h = [0.0f32; STATIONS];
        let mut seen = [false; STATIONS];
        for v in verts {
            // map z in [-half_z, half_z] onto a station index
            let t = ((v[2] + half_z) / (2.0 * half_z)).clamp(0.0, 1.0);
            let i = ((t * (STATIONS - 1) as f32).round() as usize).min(STATIONS - 1);
            w[i] = w[i].max(v[0].abs());
            h[i] = h[i].max(v[1].abs());
            seen[i] = true;
        }
        if !seen.iter().any(|s| *s) {
            return None;
        }
        // fill empty stations from the nearest sampled one
        for i in 0..STATIONS {
            if seen[i] {
                continue;
            }
            let mut best: Option<usize> = None;
            for j in 0..STATIONS {
                if seen[j] && best.map_or(true, |b| (j as i32 - i as i32).abs() < (b as i32 - i as i32).abs()) {
                    best = Some(j);
                }
            }
            if let Some(j) = best {
                w[i] = w[j];
                h[i] = h[j];
            }
        }
        let k = 1.0 - inset;
        for i in 0..STATIONS {
            w[i] = (w[i] * k).max(0.001);
            h[i] = (h[i] * k).max(0.001);
        }
        Some(Envelope { half_z, w, h })
    }

    /// Interpolated half-extents at an arbitrary local z.
    pub fn at(&self, z: f32) -> (f32, f32) {
        let t = ((z + self.half_z) / (2.0 * self.half_z)).clamp(0.0, 1.0);
        let f = t * (STATIONS - 1) as f32;
        let i = (f.floor() as usize).min(STATIONS - 1);
        let j = (i + 1).min(STATIONS - 1);
        let m = f - i as f32;
        (self.w[i] * (1.0 - m) + self.w[j] * m, self.h[i] * (1.0 - m) + self.h[j] * m)
    }

    /// The narrowest cross-section anywhere in `[z0, z1]`.
    ///
    /// Sizing a room from `at(centre)` is wrong on a tapering hull: the
    /// room spans z, and the hull at its forward and aft faces is
    /// narrower than at its middle. That was this module's first real
    /// bug — rooms escaped the envelope, the bow rooms overlapped the
    /// bays, and taper made no difference to how much fitted, all from
    /// the same mistake. A room must fit the WORST section it covers.
    pub fn min_over(&self, z0: f32, z1: f32) -> (f32, f32) {
        let (lo, hi) = if z0 <= z1 { (z0, z1) } else { (z1, z0) };
        let mut w = f32::MAX;
        let mut h = f32::MAX;
        // the endpoints, plus every station in between
        for z in [lo, hi] {
            let (a, b) = self.at(z);
            w = w.min(a);
            h = h.min(b);
        }
        for i in 0..STATIONS {
            let z = station_z(i, self.half_z);
            if z >= lo && z <= hi {
                w = w.min(self.w[i]);
                h = h.min(self.h[i]);
            }
        }
        (w, h)
    }

    /// The widest station's half-width — the beam.
    pub fn beam(&self) -> f32 {
        self.w.iter().fold(0.0f32, |m, v| m.max(*v))
    }
}

/// One planned room. Deliberately not a `Cell`: this is the plan, and
/// `interior.rs` owns what a built room is.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedRoom {
    pub id: String,
    pub kind: RoomKind,
    pub centre: Vec3,
    pub half: Vec3,
}

/// (guess) — how much of the local half-height the deck sits below
/// centreline, so there is headroom above and structure below.
pub const DECK_DROP: f32 = 0.25;
/// (guess) — a room must be at least this many occupant-heights in each
/// horizontal direction to be worth placing at all.
pub const MIN_ROOM_H: f32 = 1.4;
/// (guess) — the spine corridor's half-width as a fraction of the local
/// envelope, leaving room for cabins either side.
pub const SPINE_FRAC: f32 = 0.28;

/// Lay out a plan inside `env` for bodies `occupant_h` tall.
///
/// The shape of the algorithm, and why:
///
/// * A SPINE runs the usable length of the hull, its width following
///   the envelope — this is the one thing every ship has, and making it
///   follow the taper is the whole point of planning from a shape.
/// * Rooms are placed in PAIRS either side of the spine wherever the
///   local cross-section can hold one, walking stern to bow. A hull that
///   is only wide enough amidships gets rooms only amidships, which a
///   fixed table cannot express.
/// * The bow station gets the bridge and the stern the hangar, because
///   those are the two rooms whose position is a fact about a ship
///   rather than a preference.
///
/// Returns an empty plan for a hull too small to hold anything — the
/// honest answer, and the caller can fall back.
pub fn plan(env: &Envelope, occupant_h: f32) -> Vec<PlannedRoom> {
    let occupant_h = occupant_h.max(0.01);
    let min_side = MIN_ROOM_H * occupant_h;
    let mut out: Vec<PlannedRoom> = Vec::new();

    // --- the spine -------------------------------------------------
    // it spans whatever length has enough width to walk down
    let mut z0 = -env.half_z;
    let mut z1 = env.half_z;
    for i in 0..STATIONS {
        if env.w[i] * SPINE_FRAC >= min_side * 0.35 {
            z0 = station_z(i, env.half_z);
            break;
        }
    }
    for i in (0..STATIONS).rev() {
        if env.w[i] * SPINE_FRAC >= min_side * 0.35 {
            z1 = station_z(i, env.half_z);
            break;
        }
    }
    if z1 <= z0 {
        return out;
    }
    // Reserve the bow and stern for the two rooms whose position is a
    // fact about a ship. Bays are placed strictly BETWEEN them, which is
    // what stops a bay and the hangar occupying the same steel.
    let end_zone = ((z1 - z0) * 0.16).min(env.beam() * 1.2);
    let bay_z0 = z0 + end_zone;
    let bay_z1 = z1 - end_zone;

    // --- rooms either side, following the shape --------------------
    // HOW MANY BAYS THE SPAN CAN ACTUALLY HOLD, rather than a fixed six.
    // Six was hardcoded, and on a hull whose usable span is short it
    // made every bay too shallow to stand in — so every one was
    // rejected and the planner returned nothing for a ship that could
    // plainly hold two rooms. Derive the count from the span and the
    // minimum depth a body needs, so a short hull gets fewer, bigger
    // bays instead of an empty plan.
    let min_depth = min_side * 0.5;
    let steps = if bay_z1 > bay_z0 {
        (((bay_z1 - bay_z0) / (min_depth * 2.0 / 0.42)).floor() as usize).clamp(1, 6)
    } else {
        0
    };
    let mut bays: Vec<PlannedRoom> = Vec::new();
    if bay_z1 > bay_z0 && steps > 0 {
        let span = bay_z1 - bay_z0;
        for s in 0..steps {
            let t = (s as f32 + 0.5) / steps as f32;
            let z = bay_z0 + span * t;
            let half_z_room = (span / steps as f32) * 0.42;
            // SIZE AGAINST THE WORST SECTION THIS ROOM COVERS, not the
            // one at its centre -- see `Envelope::min_over`.
            let (w, h) = env.min_over(z - half_z_room, z + half_z_room);
            let spine_w = (w * SPINE_FRAC).max(min_side * 0.35);
            let free = w - spine_w;
            if free < min_side {
                continue; // too pinched here -- a real answer, not a failure
            }
            // leave the DOOR GAP outboard of the spine before the room
            // starts -- see `DOOR_GAP_H`
            let gap = DOOR_GAP_H * occupant_h;
            let room_half_x = ((free - gap) * 0.5).min(w * 0.45);
            if half_z_room < min_side * 0.5 || room_half_x < min_side * 0.5 {
                continue;
            }
            for (side, tag) in [(-1.0f32, "p"), (1.0, "s")] {
                let cx = side * (spine_w + gap + room_half_x);
                bays.push(PlannedRoom {
                    id: format!("bay_{s}{tag}"),
                    kind: RoomKind::Quest,
                    centre: Vec3::new(cx, -h * DECK_DROP, z),
                    half: Vec3::new(room_half_x, h * 0.5, half_z_room),
                });
            }
        }
    }

    // A SPINE WITH NOTHING ON IT IS NOT A SHIP INTERIOR. If the hull
    // could not hold a single bay beside the spine, fall back to a
    // single-file layout before giving up — see `plan_inline`. A narrow
    // hull is not an empty hull; it is a tube with compartments in
    // series, which is what a small craft actually is.
    if bays.is_empty() {
        return plan_inline(env, occupant_h);
    }

    // THE SPINE STOPS SHORT OF THE BOW AND STERN ROOMS. Running it the
    // full length put it straight through the bridge and the hangar --
    // both centred on x=0 and far wider than the corridor -- so those
    // two were fused to it and their doors were decorative.
    let gap = DOOR_GAP_H * occupant_h;
    let sz0 = bay_z0;
    let sz1 = bay_z1;
    let mid_z = (sz0 + sz1) * 0.5;
    let (mw, mh) = env.min_over(sz0, sz1);
    out.push(PlannedRoom {
        id: "spine".into(),
        kind: RoomKind::Corridor,
        centre: Vec3::new(0.0, -mh * DECK_DROP, mid_z),
        half: Vec3::new((mw * SPINE_FRAC).max(min_side * 0.35), mh * 0.5, (sz1 - sz0) * 0.5),
    });
    out.extend(bays);

    // --- the two rooms whose position is a fact --------------------
    for (id, kind, zc, sign) in [
        ("bridge", RoomKind::Bridge, z1, -1.0f32),
        ("hangar", RoomKind::Hangar, z0, 1.0),
    ] {
        // fit inside the end zone with the DOOR GAP held clear of the
        // spine: the bridge used to start 0.11 m inside the corridor.
        let half_z_room = ((end_zone - gap) * 0.5).max(0.0);
        if half_z_room <= 0.0 {
            continue;
        }
        // the gap belongs BETWEEN this room and the spine, not between
        // it and the hull's nose -- the first attempt put it at the
        // wrong end and the bridge still touched the corridor exactly.
        let centre_z = zc + sign * half_z_room;
        let (w, h) = env.min_over(centre_z - half_z_room, centre_z + half_z_room);
        if w < min_side * 0.5 || h <= 0.0 {
            continue;
        }
        out.push(PlannedRoom {
            id: id.into(),
            kind,
            centre: Vec3::new(0.0, -h * DECK_DROP, centre_z),
            half: Vec3::new(w * 0.85, h * 0.5, half_z_room),
        });
    }
    out
}

/// THE WIRING ENTRY POINT: place a NAMED roster by hull shape.
///
/// `plan` invents generic ids (`bay_0p`, `cabin_1`), which is right for
/// proving the geometry and wrong for the game: `stations.rs`,
/// `aboard::stand_in`, `Interior::reachable_from` and a pile of tests
/// all address rooms by their `content::HUB` ids. Swapping those out
/// would be a rename disguised as a layout change.
///
/// So this keeps the roster — every id, name, kind, blurb and fixture
/// set survives — and lets the ENVELOPE decide where each room sits and
/// how big it is. That is the actual thing being asked for: the same
/// ship's rooms, laid out by the shape of the hull they are in rather
/// than by a fixed table scaled from one number.
///
/// **Every roster room is guaranteed a position.** A hull with fewer
/// shape slots than rooms subdivides its spine rather than dropping
/// anyone — a missing room is a missing console, and silently losing
/// the medbay because the hull was narrow is exactly the kind of
/// failure that surfaces three systems away.
pub fn plan_named(
    env: &Envelope,
    occupant_h: f32,
    roster: &[crate::content::Room],
) -> Vec<PlannedRoom> {
    let slots = plan(env, occupant_h);
    if slots.is_empty() || roster.is_empty() {
        return Vec::new();
    }
    // sort the shape's slots into the three roles the roster cares about
    let bow = slots.iter().find(|s| s.kind == RoomKind::Bridge).cloned();
    let stern = slots.iter().find(|s| s.kind == RoomKind::Hangar).cloned();
    let spine = slots.iter().find(|s| s.kind == RoomKind::Corridor).cloned();
    let mut bays: Vec<PlannedRoom> =
        slots.iter().filter(|s| s.kind == RoomKind::Quest).cloned().collect();
    // stern to bow, so walking the roster walks the ship
    bays.sort_by(|a, b| a.centre.z.partial_cmp(&b.centre.z).unwrap_or(std::cmp::Ordering::Equal));

    let mut out = Vec::new();
    let mut bay_i = 0usize;
    // rooms that still need somewhere after the bays run out get a share
    // of the spine, subdivided along its length
    let overflow: Vec<&crate::content::Room> = roster
        .iter()
        .filter(|r| !matches!(r.kind, RoomKind::Bridge | RoomKind::Hangar | RoomKind::Corridor))
        .skip(bays.len())
        .collect();
    let mut overflow_i = 0usize;

    for r in roster {
        let placed = match r.kind {
            RoomKind::Bridge => bow.clone(),
            RoomKind::Hangar => stern.clone(),
            RoomKind::Corridor => spine.clone(),
            _ => {
                if bay_i < bays.len() {
                    let s = bays[bay_i].clone();
                    bay_i += 1;
                    Some(s)
                } else if let Some(sp) = spine.clone() {
                    // subdivide the spine: n slices along its length
                    let n = overflow.len().max(1);
                    let t = (overflow_i as f32 + 0.5) / n as f32;
                    overflow_i += 1;
                    let hz = sp.half.z / n as f32 * 0.8;
                    let zc = (sp.centre.z - sp.half.z) + (sp.half.z * 2.0) * t;
                    Some(PlannedRoom {
                        id: sp.id.clone(),
                        kind: sp.kind,
                        centre: Vec3::new(sp.centre.x, sp.centre.y, zc),
                        half: Vec3::new(sp.half.x, sp.half.y, hz),
                    })
                } else {
                    None
                }
            }
        };
        let Some(s) = placed else { continue };
        out.push(PlannedRoom {
            id: r.id.to_string(),
            kind: r.kind,
            centre: s.centre,
            half: s.half,
        });
    }
    out
}

/// SINGLE-FILE LAYOUT, for a hull too narrow to put rooms beside a
/// spine.
///
/// The side-by-side layout in `plan` needs `spine + room` of half-width,
/// and the smallest fleet hull does not have it: the cutter's half-beam
/// is 2.74 m against the 3.59 m that layout wants. That produced an
/// empty plan, which read as "this ship has no interior" when the truth
/// is "this ship is a different shape of interior". A small craft is a
/// tube you walk down with compartments front to back — no side rooms,
/// no parallel corridor, the spine IS the circulation and the rooms are
/// strung along it.
///
/// Still returns empty for a hull that genuinely cannot hold one
/// compartment, which is a real answer and stays testable.
pub fn plan_inline(env: &Envelope, occupant_h: f32) -> Vec<PlannedRoom> {
    let occupant_h = occupant_h.max(0.01);
    let min_side = MIN_ROOM_H * occupant_h;
    let mut out = Vec::new();

    // usable run: where the hull is at least wide enough to stand in
    let mut z0 = env.half_z;
    let mut z1 = -env.half_z;
    for i in 0..STATIONS {
        if env.w[i] * 2.0 >= min_side {
            z0 = z0.min(station_z(i, env.half_z));
            z1 = z1.max(station_z(i, env.half_z));
        }
    }
    if z1 <= z0 {
        return out;
    }
    let span = z1 - z0;
    // How many compartments the run can hold at a standable depth.
    //
    // The ceiling is 8, not 5, so a tube can hold a whole roster:
    // `content::HUB` needs a bridge, a hangar, a corridor and four other
    // rooms = seven. At five, `plan_named` ran out of slots on the narrow
    // hulls and OVERLAID the leftovers onto the spine, which fused them
    // to the corridor and made their doors decorative -- the same bug the
    // door gap was added to fix, arriving by a different route. Giving
    // the tube enough compartments removes the overflow case rather than
    // managing it.
    let depth = min_side * 0.75;
    let n = ((span / depth).floor() as usize).clamp(0, 8);
    if n == 0 {
        return out;
    }
    let seg = span / n as f32;
    for k in 0..n {
        let zc = z0 + seg * (k as f32 + 0.5);
        let half_z = seg * 0.45;
        let (w, h) = env.min_over(zc - half_z, zc + half_z);
        if w * 2.0 < min_side * 0.6 {
            continue;
        }
        // bow compartment is the bridge, stern is the hangar, the rest
        // are cabins -- the same two positional facts as the wide plan
        let (id, kind) = if k + 1 == n {
            ("bridge".to_string(), RoomKind::Bridge)
        } else if k == 0 {
            ("hangar".to_string(), RoomKind::Hangar)
        } else {
            (format!("cabin_{k}"), RoomKind::Quest)
        };
        out.push(PlannedRoom {
            id,
            kind,
            centre: Vec3::new(0.0, -h * DECK_DROP, zc),
            half: Vec3::new(w * 0.9, h * 0.5, half_z),
        });
    }
    // a single compartment is a cockpit, not a ship with an interior
    if out.len() < 2 {
        out.clear();
        return out;
    }
    // THE TUBE IS THE CORRIDOR. Without a spine slot, `plan_named` has
    // nowhere to put the roster's corridor rooms or any room that
    // overflows the compartments, and silently drops them — which is
    // how the cutter placed five of eight. A single-file hull still has
    // circulation; it is just the same volume you stand in.
    // A SINGLE-FILE HULL HAS NO SEPARATE CORRIDOR, and inventing one was
    // wrong twice over. Spanning the compartments fused them into one
    // void (doors stop being walls); squeezing a link between them ate
    // the middle cabin. In a tube you walk THROUGH the compartments --
    // each one is its own stretch of circulation and they meet at doors.
    //
    // So the last compartment is retagged as the corridor rather than a
    // new box being added. `plan_named` needs a Corridor slot to place
    // the roster's corridor rooms, and this gives it a real one that
    // occupies space nothing else claims.
    if let Some(mid) = out.len().checked_sub(1).map(|n| n / 2) {
        if out[mid].kind == RoomKind::Quest {
            out[mid].kind = RoomKind::Corridor;
            out[mid].id = "spine".into();
        }
    }
    out
}

/// (guess) — the gap between the spine and the rooms it serves, in
/// occupant-heights. This is where the DOOR lives.
///
/// Without it the spine intersects every room, the interior becomes one
/// connected void, and a shut door stops being a wall — which is exactly
/// how wiring this planner into `Interior::generate` broke
/// `a_shut_door_is_a_wall_and_an_open_one_is_a_way_through`. A doorway
/// is a hole you make in a wall, not the absence of the wall.
pub const DOOR_GAP_H: f32 = 0.35;

/// Does the spine intersect any room it serves?
///
/// Separate from `overlaps` because it is a DIFFERENT failure. `overlaps`
/// asks whether two rooms occupy the same steel; this asks whether the
/// corridor has swallowed them, which reads as a perfectly reasonable
/// plan right up until doors stop working. `overlaps` deliberately
/// excluded corridors on the reasoning that an overlap is a doorway —
/// true for one door, false as a layout rule, and nothing was checking
/// the difference.
pub fn spine_intersects(rooms: &[PlannedRoom]) -> Option<String> {
    let spine = rooms.iter().find(|r| r.kind == RoomKind::Corridor)?;
    for r in rooms {
        if r.kind == RoomKind::Corridor {
            continue;
        }
        let d = (spine.centre - r.centre).abs();
        let s = spine.half + r.half;
        if d.x < s.x - 1e-3 && d.y < s.y - 1e-3 && d.z < s.z - 1e-3 {
            return Some(r.id.clone());
        }
    }
    None
}

/// Is every planned room inside the envelope it was planned for?
///
/// The one guarantee this module exists to provide, exposed so a caller
/// can assert it too — `hullform.rs`'s own doc records how expensive it
/// was to discover rooms poking through a hull after the fact.
pub fn all_inside(env: &Envelope, rooms: &[PlannedRoom]) -> bool {
    rooms.iter().all(|r| {
        // check both z-faces of the room, since the envelope tapers
        for zf in [r.centre.z - r.half.z, r.centre.z + r.half.z] {
            if zf.abs() > env.half_z + 1e-3 {
                return false;
            }
            let (w, h) = env.at(zf);
            if r.centre.x.abs() + r.half.x > w + 1e-3 {
                return false;
            }
            if r.centre.y.abs() + r.half.y > h + 1e-3 {
                return false;
            }
        }
        true
    })
}

/// Do any two planned rooms overlap? Corridors are allowed to overlap
/// the rooms they serve — that is a doorway, not a collision — so this
/// only checks room-against-room.
pub fn overlaps(rooms: &[PlannedRoom]) -> Option<(String, String)> {
    let solid: Vec<&PlannedRoom> =
        rooms.iter().filter(|r| r.kind != RoomKind::Corridor).collect();
    for i in 0..solid.len() {
        for j in (i + 1)..solid.len() {
            let (a, b) = (solid[i], solid[j]);
            let d = (a.centre - b.centre).abs();
            let s = a.half + b.half;
            if d.x < s.x - 1e-3 && d.y < s.y - 1e-3 && d.z < s.z - 1e-3 {
                return Some((a.id.clone(), b.id.clone()));
            }
        }
    }
    None
}

/// `--roomplan`: plan every fleet hull and report what its shape can
/// actually hold, so the difference between a box and a real envelope is
/// readable without a screen.
pub fn run() {
    let occupant_h = 1.9;
    println!("ROOMPLAN — rooms derived from hull shape, occupant {occupant_h} m\n");
    println!("{:<10} {:>7} {:>7} {:>7} {:>8} {:>7}  {}", "hull", "len", "beam", "rooms", "inside?", "overlap", "note");
    for (name, heights, note) in crate::aboard::FLEET {
        let len = heights * occupant_h;
        // a tapered envelope from the same extents `Interior::generate`
        // would use, so this is a like-for-like comparison rather than a
        // different ship
        let env = Envelope::from_extents(len * 0.5, len * 0.16, len * 0.09, 0.55);
        let rooms = plan(&env, occupant_h);
        println!(
            "{:<10} {:>7.1} {:>7.2} {:>7} {:>8} {:>7}  {}",
            name,
            len,
            env.beam(),
            rooms.len(),
            if all_inside(&env, &rooms) { "yes" } else { "NO" },
            if overlaps(&rooms).is_some() { "YES" } else { "no" },
            note
        );
    }

    println!("\ntaper changes what a hull can hold (runner, 38 m):");
    let len = 20.0 * occupant_h;
    for taper in [0.0f32, 0.3, 0.6, 0.85] {
        let env = Envelope::from_extents(len * 0.5, len * 0.16, len * 0.09, taper);
        let rooms = plan(&env, occupant_h);
        println!(
            "  taper {taper:.2}: {} rooms, inside={}",
            rooms.len(),
            all_inside(&env, &rooms)
        );
    }

    // THE REAL ASSET, not a formula. Everything above is a synthetic
    // envelope; this is the actual ship model the game loads, sampled
    // through the same `from_cloud` path the game would use. Needs the
    // assets directory and a GPU-less model load, so it reports rather
    // than asserts — a missing asset is not a planner failure.
    println!("\nthe real ship model ({}):", crate::aboard::EXTERIOR_MODEL);
    let dir = crate::assets_dir().join("bots").join(crate::aboard::EXTERIOR_MODEL).join("f00");
    if !dir.is_dir() {
        println!("  not present at {dir:?} — skipped");
        return;
    }
    let (raw, _lo, _hi, _a, _b) = crate::model::load_raw_at(&dir, 40.0, false);
    let cloud: Vec<glam::Vec3> = raw
        .iter()
        .flat_map(|p| p.verts.iter().map(|v| glam::Vec3::from(v.pos)))
        .collect();
    println!("  {} prims, {} sampled points", raw.len(), cloud.len());
    match from_cloud(&cloud, 0.12) {
        Some(env) => {
            let rooms = plan(&env, occupant_h);
            println!(
                "  beam {:.2} m, half_z {:.2} m -> {} rooms, inside={}, overlap={}",
                env.beam(),
                env.half_z,
                rooms.len(),
                all_inside(&env, &rooms),
                overlaps(&rooms).is_some()
            );
            for r in &rooms {
                println!(
                    "    {:<10} {:?} at ({:>6.1},{:>5.1},{:>6.1}) half ({:.1},{:.1},{:.1})",
                    r.id, r.kind, r.centre.x, r.centre.y, r.centre.z,
                    r.half.x, r.half.y, r.half.z
                );
            }
        }
        None => println!("  the cloud produced no envelope"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OCC: f32 = 1.9;

    fn box_env() -> Envelope {
        Envelope::from_extents(19.0, 6.0, 3.4, 0.5)
    }

    #[test]
    fn from_cloud_matches_from_model_verts_on_the_same_points() {
        // `from_cloud` is the real entry point (Model keeps `cloud`, not
        // raw verts -- an earlier pass here got that backwards). It must
        // stay a pure adapter, not a second sampler that can drift from
        // the one every other test exercises.
        let pts = vec![
            Vec3::new(4.0, 2.0, 0.0),
            Vec3::new(-4.0, -2.0, 5.0),
            Vec3::new(3.0, 1.5, -5.0),
        ];
        let raw: Vec<[f32; 3]> = pts.iter().map(|p| p.to_array()).collect();
        assert_eq!(from_cloud(&pts, 0.1), Envelope::from_model_verts(&raw, 0.1));
    }

    #[test]
    fn from_cloud_on_an_empty_cloud_is_none_not_a_zero_hull() {
        // a model whose cloud never got baked must not silently become a
        // ship with no interior volume
        assert!(from_cloud(&[], 0.1).is_none());
    }

    #[test]
    fn a_downsampled_cloud_still_plans_a_usable_ship() {
        // the cloud is DOWNSAMPLED, so it is sparser than the mesh it
        // came from. A planner that only worked on dense input would
        // pass every other test here and fail on the one real input.
        let mut cloud: Vec<Vec3> = Vec::new();
        for k in 0..40 {
            // deliberately few points: 40 across 24 stations
            let z = -14.0 + 28.0 * (k as f32 / 39.0);
            let w = 5.0 - 3.0 * (z / 14.0).abs();
            let h = 3.0 - 1.6 * (z / 14.0).abs();
            cloud.push(Vec3::new(w, h, z));
            cloud.push(Vec3::new(-w, -h, z));
        }
        let env = from_cloud(&cloud, 0.12).expect("a sparse cloud is still a cloud");
        let rooms = plan(&env, OCC);
        assert!(!rooms.is_empty(), "a downsampled hull planned nothing");
        assert!(all_inside(&env, &rooms));
        assert_eq!(overlaps(&rooms), None);
    }

    #[test]
    fn a_plan_fits_inside_the_envelope_it_was_planned_for() {
        // the one guarantee. `hullform.rs` records what it cost to find
        // rooms outside their hull after the fact; this catches it here.
        let env = box_env();
        let rooms = plan(&env, OCC);
        assert!(!rooms.is_empty(), "this hull should hold something");
        assert!(all_inside(&env, &rooms), "a planned room escaped its envelope");
    }

    #[test]
    fn no_two_rooms_overlap() {
        let env = box_env();
        let rooms = plan(&env, OCC);
        assert_eq!(overlaps(&rooms), None);
    }

    #[test]
    fn the_plan_is_deterministic() {
        let env = box_env();
        assert_eq!(plan(&env, OCC), plan(&env, OCC));
    }

    #[test]
    fn a_pinched_hull_holds_fewer_rooms_than_a_fat_one() {
        // THE WHOLE POINT: the plan follows the shape. A heavily tapered
        // hull has less usable cross-section and must place fewer rooms
        // than the same length as a box. A fixed table cannot express
        // this, which is why `Interior::generate` gives every ship the
        // same eight rooms.
        let fat = Envelope::from_extents(19.0, 6.0, 3.4, 0.0);
        let thin = Envelope::from_extents(19.0, 6.0, 3.4, 0.9);
        let a = plan(&fat, OCC);
        let b = plan(&thin, OCC);
        assert!(
            b.len() < a.len(),
            "taper made no difference: {} rooms vs {}",
            b.len(),
            a.len()
        );
    }

    #[test]
    fn a_hull_too_small_to_hold_anything_says_so_instead_of_guessing() {
        // genuinely tiny -- smaller than one compartment, so even the
        // single-file fallback must refuse rather than invent a room
        let tiny = Envelope::from_extents(0.6, 0.2, 0.15, 0.5);
        let rooms = plan(&tiny, OCC);
        assert!(rooms.is_empty(), "a hull smaller than its occupant planned {} rooms", rooms.len());
    }

    #[test]
    fn plan_named_keeps_every_roster_id() {
        // THE WIRING GUARANTEE. A shape-derived layout that silently
        // drops a room takes a console with it, and the failure shows up
        // three systems away in `stations.rs`. Every hull the game can
        // build must place every roster room.
        for (name, heights, _) in crate::aboard::FLEET {
            let len = heights * OCC;
            let env = Envelope::from_extents(len * 0.5, len * 0.16, len * 0.09, 0.55);
            let placed = plan_named(&env, OCC, crate::content::HUB);
            assert_eq!(
                placed.len(),
                crate::content::HUB.len(),
                "{name}: placed {} of {} rooms",
                placed.len(),
                crate::content::HUB.len()
            );
            for r in crate::content::HUB {
                assert!(placed.iter().any(|p| p.id == r.id), "{name}: lost room {}", r.id);
            }
        }
    }

    #[test]
    fn plan_named_puts_the_bridge_forward_of_the_hangar() {
        let env = box_env();
        let placed = plan_named(&env, OCC, crate::content::HUB);
        let bridge = placed.iter().find(|p| p.id == "bridge").expect("bridge");
        let hangar = placed.iter().find(|p| p.id == "hangar").expect("hangar");
        assert!(bridge.centre.z > hangar.centre.z);
    }

    #[test]
    fn plan_named_layout_actually_follows_the_hull_shape() {
        // if a fat hull and a pinched one produce the same positions,
        // the wiring did not change anything and this is a rename.
        let fat = Envelope::from_extents(19.0, 6.0, 3.4, 0.0);
        let thin = Envelope::from_extents(19.0, 6.0, 3.4, 0.85);
        let a = plan_named(&fat, OCC, crate::content::HUB);
        let b = plan_named(&thin, OCC, crate::content::HUB);
        assert_eq!(a.len(), b.len(), "both must still place everyone");
        assert!(
            a.iter().zip(b.iter()).any(|(x, y)| x.centre != y.centre || x.half != y.half),
            "hull shape made no difference to the layout"
        );
    }

    #[test]
    fn the_spine_never_swallows_a_room_it_serves() {
        // THE REGRESSION THIS PLANNER SHIPPED ONCE. Wiring it into
        // `Interior::generate` broke
        // `a_shut_door_is_a_wall_and_an_open_one_is_a_way_through`: the
        // spine intersected every room, so the interior was one
        // connected void and a shut door stopped being a wall. Checked
        // across the whole fleet and both layouts, because the wide plan
        // and the single-file fallback each had their own version of it.
        for (name, heights, _) in crate::aboard::FLEET {
            let len = heights * OCC;
            let env = Envelope::from_extents(len * 0.5, len * 0.16, len * 0.09, 0.55);
            let rooms = plan(&env, OCC);
            assert_eq!(
                spine_intersects(&rooms),
                None,
                "{name}: the spine swallowed a room -- its door is now decorative"
            );
        }
    }

    #[test]
    fn the_spine_leaves_a_real_gap_not_just_a_shared_face() {
        // touching is not separation: two boxes sharing a face still let
        // you walk between them. There must be actual daylight for a
        // door to sit in.
        let env = box_env();
        let rooms = plan(&env, OCC);
        let spine = rooms.iter().find(|r| r.kind == RoomKind::Corridor).expect("a spine");
        let gap = DOOR_GAP_H * OCC;
        for r in &rooms {
            if r.kind == RoomKind::Corridor {
                continue;
            }
            let d = (spine.centre - r.centre).abs();
            let s = spine.half + r.half;
            // separated on at least one axis by at least most of the gap
            let sep = (d.x - s.x).max(d.z - s.z);
            assert!(
                sep > gap * 0.5,
                "{} is only {sep:.3} m from the spine -- no room for a door",
                r.id
            );
        }
    }

    #[test]
    fn the_named_layout_also_keeps_its_doors_working() {
        // plan_named reuses plan()'s slots so it inherits the fix, but it
        // also SUBDIVIDES the spine for overflow rooms — its own chance
        // to reintroduce the bug. Overflow rooms legitimately sit IN the
        // spine volume (they are stretches of corridor), so they are
        // identified by carrying the corridor kind, not by id; every
        // room that is a real room must stay clear.
        for (name, heights, _) in crate::aboard::FLEET {
            let len = heights * OCC;
            let env = Envelope::from_extents(len * 0.5, len * 0.16, len * 0.09, 0.55);
            let placed = plan_named(&env, OCC, crate::content::HUB);
            let spine = placed
                .iter()
                .find(|p| p.kind == RoomKind::Corridor)
                .unwrap_or_else(|| panic!("{name}: no corridor placed"));
            for r in &placed {
                if r.kind == RoomKind::Corridor {
                    continue; // corridor stretches may share the spine
                }
                let d = (spine.centre - r.centre).abs();
                let s = spine.half + r.half;
                assert!(
                    !(d.x < s.x - 1e-3 && d.y < s.y - 1e-3 && d.z < s.z - 1e-3),
                    "{name}: the spine swallowed {} -- its door is decorative",
                    r.id
                );
            }
        }
    }

    #[test]
    fn a_narrow_hull_falls_back_to_compartments_in_series() {
        // THE CUTTER CASE. Half-beam 2.74 m cannot hold a 0.93 m spine
        // plus a 2.66 m room beside it, so the side-by-side layout finds
        // nothing. That is a different SHAPE of interior, not an absent
        // one, and the fallback must produce a real single-file plan.
        let len = 9.0 * OCC; // the cutter, in occupant-heights
        let env = Envelope::from_extents(len * 0.5, len * 0.16, len * 0.09, 0.55);
        let rooms = plan(&env, OCC);
        assert!(!rooms.is_empty(), "the cutter still plans nothing");
        assert!(all_inside(&env, &rooms), "an inline room escaped the hull");
        assert_eq!(overlaps(&rooms), None, "inline compartments overlap");
        // single file: no room sits beside another at the same z
        for r in &rooms {
            assert!(
                r.centre.x.abs() < 1e-3,
                "{} is offset to one side -- this layout is single-file",
                r.id
            );
        }
    }

    #[test]
    fn the_inline_fallback_still_puts_the_bridge_forward() {
        let len = 9.0 * OCC;
        let env = Envelope::from_extents(len * 0.5, len * 0.16, len * 0.09, 0.55);
        let rooms = plan(&env, OCC);
        let bridge = rooms.iter().find(|r| r.kind == RoomKind::Bridge).expect("a bridge");
        let hangar = rooms.iter().find(|r| r.kind == RoomKind::Hangar).expect("a hangar");
        assert!(bridge.centre.z > hangar.centre.z);
    }

    #[test]
    fn a_one_compartment_hull_is_a_cockpit_not_an_interior() {
        // a hull with room for exactly one compartment should refuse:
        // "a ship you can walk around in" needs at least two, and one
        // box is a cockpit. Swept rather than hand-tuned so this does
        // not depend on guessing the exact threshold length.
        let mut saw_single_refused = false;
        for k in 1..40 {
            let len = k as f32 * 0.5 * OCC;
            let env = Envelope::from_extents(len * 0.5, len * 0.16, len * 0.09, 0.55);
            let rooms = plan(&env, OCC);
            assert_ne!(rooms.len(), 1, "len {len:.1}: a lone compartment was returned as an interior");
            if rooms.is_empty() {
                saw_single_refused = true;
            }
        }
        assert!(saw_single_refused, "the sweep never hit a refusing size -- it proves nothing");
    }

    #[test]
    fn every_fleet_hull_now_plans_something_usable() {
        // the wire-up blocker: before the inline fallback the cutter
        // planned zero rooms, so switching the game over would have
        // given the smallest ship no interior at all.
        for (name, heights, _) in crate::aboard::FLEET {
            let len = heights * OCC;
            let env = Envelope::from_extents(len * 0.5, len * 0.16, len * 0.09, 0.55);
            let rooms = plan(&env, OCC);
            assert!(!rooms.is_empty(), "{name} plans no rooms at all");
            assert!(all_inside(&env, &rooms), "{name}: a room escaped");
            assert_eq!(overlaps(&rooms), None, "{name}: rooms overlap");
        }
    }

    #[test]
    fn the_bridge_is_forward_and_the_hangar_is_aft() {
        let env = box_env();
        let rooms = plan(&env, OCC);
        let bridge = rooms.iter().find(|r| r.id == "bridge").expect("a bridge");
        let hangar = rooms.iter().find(|r| r.id == "hangar").expect("a hangar");
        assert!(bridge.centre.z > hangar.centre.z, "the bridge must be forward of the hangar");
    }

    #[test]
    fn every_plan_has_exactly_one_spine() {
        let env = box_env();
        let rooms = plan(&env, OCC);
        let spines = rooms.iter().filter(|r| r.kind == RoomKind::Corridor).count();
        assert_eq!(spines, 1);
    }

    #[test]
    fn bigger_occupants_get_fewer_rooms_in_the_same_hull() {
        // rooms are sized in occupant-heights, so the same steel holds
        // fewer giants than people. If this ever stops being true the
        // sizing has quietly become absolute.
        let env = box_env();
        let small = plan(&env, 0.5);
        let big = plan(&env, 3.0);
        assert!(
            big.len() <= small.len(),
            "a bigger body fitted MORE rooms ({}) than a smaller one ({})",
            big.len(),
            small.len()
        );
    }

    #[test]
    fn an_envelope_from_model_verts_follows_the_geometry() {
        // a dumbbell: fat at both ends, pinched in the middle. The
        // envelope must show the pinch, or it is not reading the model.
        let mut verts: Vec<[f32; 3]> = Vec::new();
        for k in 0..200 {
            let z = -10.0 + 20.0 * (k as f32 / 199.0);
            let fat = if z.abs() > 6.0 { 4.0 } else { 0.8 };
            verts.push([fat, fat, z]);
            verts.push([-fat, -fat, z]);
        }
        let env = Envelope::from_model_verts(&verts, 0.0).expect("verts given");
        let (mid_w, _) = env.at(0.0);
        let (end_w, _) = env.at(9.0);
        assert!(end_w > mid_w * 2.0, "the pinch was not read: mid {mid_w}, end {end_w}");
    }

    #[test]
    fn empty_model_verts_report_none_rather_than_a_zero_hull() {
        assert!(Envelope::from_model_verts(&[], 0.0).is_none());
    }

    #[test]
    fn model_envelope_fills_gaps_instead_of_cutting_the_hull_in_half() {
        // a vertex distribution with a bare patch is a sampling
        // artefact, not a hole. Left unfilled it would read as zero
        // cross-section and the plan would stop dead there.
        let mut verts: Vec<[f32; 3]> = Vec::new();
        for k in 0..100 {
            let z = -10.0 + 20.0 * (k as f32 / 99.0);
            if z.abs() < 3.0 {
                continue; // the bare patch
            }
            verts.push([3.0, 2.0, z]);
        }
        let env = Envelope::from_model_verts(&verts, 0.0).expect("verts given");
        let (w, h) = env.at(0.0);
        assert!(w > 0.1 && h > 0.1, "the gap became a hole: {w} x {h}");
    }

    #[test]
    fn inset_pulls_the_interior_in_from_the_skin() {
        let verts = vec![[4.0f32, 2.0, 0.0], [-4.0, -2.0, 5.0], [4.0, 2.0, -5.0]];
        let flush = Envelope::from_model_verts(&verts, 0.0).unwrap();
        let inset = Envelope::from_model_verts(&verts, 0.25).unwrap();
        assert!(inset.beam() < flush.beam(), "inset did not pull the interior in");
    }

    #[test]
    fn a_model_derived_plan_also_stays_inside_its_envelope() {
        // the same guarantee, but on an envelope that came from geometry
        // rather than from a formula -- the irregular case is where a
        // planner is most likely to poke through.
        let mut verts: Vec<[f32; 3]> = Vec::new();
        for k in 0..300 {
            let z = -14.0 + 28.0 * (k as f32 / 299.0);
            let w = 5.0 - 3.0 * (z / 14.0).abs();
            let h = 3.0 - 1.6 * (z / 14.0).abs();
            verts.push([w, h, z]);
            verts.push([-w, -h, z]);
        }
        let env = Envelope::from_model_verts(&verts, 0.12).unwrap();
        let rooms = plan(&env, OCC);
        assert!(!rooms.is_empty());
        assert!(all_inside(&env, &rooms), "a model-derived plan escaped its hull");
        assert_eq!(overlaps(&rooms), None);
    }

}
