//! Ship hub — rooms, travel and destinations.
//!
//! The structure is a homebase RPG: a ship you walk around between sorties, with a
//! console that picks where to go next. Everything here is DATA so the layout can
//! change without touching engine code, the same way raids became data.
//!
//! Two decisions worth stating, because both are easy to get wrong:
//!
//! ROOMS ARE A GRAPH, NOT A LIST. A corridor you can only leave the way you came
//! reads as a menu with scenery. Each room names its exits, and the generator
//! refuses a layout where any room is unreachable from the bridge -- an upgrade bay
//! nobody can walk to is worse than no upgrade bay.
//!
//! TRAVEL IS A MODE, NOT A LOADING SCREEN. Each destination declares how you get
//! there -- drive, fly, or a landing sequence -- because the engine already has a
//! landing system and flight pitch/roll, and the arrival should use them rather
//! than cutting to black.

#[derive(Clone, Copy, PartialEq)]
pub enum RoomKind { Bridge, Upgrade, Spacewalk, Vendor, Quest, Hangar, Quarters }

impl RoomKind {
    pub fn code(self) -> u32 {
        match self { RoomKind::Bridge => 0, RoomKind::Upgrade => 1, RoomKind::Spacewalk => 2,
                     RoomKind::Vendor => 3, RoomKind::Quest => 4, RoomKind::Hangar => 5,
                     RoomKind::Quarters => 6 }
    }
    pub fn tag(self) -> &'static str {
        match self { RoomKind::Bridge => "bridge", RoomKind::Upgrade => "upgrade",
                     RoomKind::Spacewalk => "spacewalk", RoomKind::Vendor => "vendor",
                     RoomKind::Quest => "quest", RoomKind::Hangar => "hangar",
                     RoomKind::Quarters => "quarters" }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Travel { Drive, Fly, Land, Walk }
impl Travel {
    pub fn code(self) -> u32 {
        match self { Travel::Drive => 0, Travel::Fly => 1, Travel::Land => 2, Travel::Walk => 3 }
    }
    pub fn tag(self) -> &'static str {
        match self { Travel::Drive => "drive", Travel::Fly => "fly",
                     Travel::Land => "land", Travel::Walk => "walk" }
    }
}

pub struct Room {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: RoomKind,
    /// position in ship-local metres; the engine places the interior from this
    pub pos: [f32; 3],
    pub exits: &'static [&'static str],
    pub blurb: &'static str,
}

/// The ship. Laid out along its spine so the walk between rooms has direction:
/// bridge forward, engineering aft, airlock amidships where the hull is thinnest.
pub const ROOMS: &[Room] = &[
    Room { id: "bridge", name: "Bridge", kind: RoomKind::Bridge, pos: [0.0, 0.0, 24.0],
           exits: &["corridor_fore", "quarters"],
           blurb: "Navigation console. Choose where the ship goes next." },
    Room { id: "corridor_fore", name: "Forward Corridor", kind: RoomKind::Quarters,
           pos: [0.0, 0.0, 12.0], exits: &["bridge", "upgrade_bay", "market"],
           blurb: "Spine of the ship. Viewports on both sides." },
    Room { id: "upgrade_bay", name: "Upgrade Bay", kind: RoomKind::Upgrade,
           pos: [-9.0, 0.0, 6.0], exits: &["corridor_fore", "hangar"],
           blurb: "Chassis rigs and part racks. Rebuild your bot here." },
    Room { id: "market", name: "Trade Deck", kind: RoomKind::Vendor,
           pos: [9.0, 0.0, 6.0], exits: &["corridor_fore", "quarters"],
           blurb: "Whoever docked last is still selling." },
    Room { id: "quarters", name: "Crew Quarters", kind: RoomKind::Quest,
           pos: [7.0, 0.0, -6.0], exits: &["market", "bridge", "corridor_aft"],
           blurb: "Where the crew bring you problems." },
    Room { id: "corridor_aft", name: "Aft Corridor", kind: RoomKind::Quarters,
           pos: [0.0, 0.0, -10.0], exits: &["quarters", "airlock", "hangar"],
           blurb: "Colder back here. The hull ticks as it cools." },
    Room { id: "airlock", name: "Airlock", kind: RoomKind::Spacewalk,
           pos: [0.0, 0.0, -18.0], exits: &["corridor_aft"],
           blurb: "Cycle through and walk the hull. No gravity past this door." },
    Room { id: "hangar", name: "Hangar", kind: RoomKind::Hangar,
           pos: [-8.0, 0.0, -14.0], exits: &["corridor_aft", "upgrade_bay"],
           blurb: "Drop bay. Launches and landings come through here." },
];

pub struct Dest {
    pub id: &'static str,
    pub name: &'static str,
    pub map: &'static str,
    pub travel: Travel,
    pub kind: &'static str,      // mission | raid | free | vendor
    pub blurb: &'static str,
}

/// Destinations the bridge console offers. `map` must exist in maps.json -- the
/// generator checks, because a raid phase pointing at an undefined map was a real
/// bug that shipped.
pub const DESTS: &[Dest] = &[
    Dest { id: "pluto_line", name: "The Pluto Line", map: "pluto", travel: Travel::Land,
           kind: "mission", blurb: "Outer containment. Long fall, hard landing." },
    Dest { id: "rust_flats", name: "Rust Flats", map: "wastes", travel: Travel::Drive,
           kind: "free", blurb: "Open ground. Drive it if you want the scenery." },
    Dest { id: "frost_shelf", name: "Frost Shelf", map: "frostline", travel: Travel::Land,
           kind: "mission", blurb: "Ice shelves calve under weight. Land light." },
    Dest { id: "ember_descent", name: "Ember Descent", map: "emberpit", travel: Travel::Fly,
           kind: "mission", blurb: "Thermals from the lava pools. Fly it in." },
    Dest { id: "the_maw", name: "The Maw", map: "unicron", travel: Travel::Fly,
           kind: "raid", blurb: "Horizon curves the wrong way. Something is awake." },
    Dest { id: "kuiper_drift", name: "Kuiper Drift", map: "kuiper", travel: Travel::Fly,
           kind: "raid", blurb: "Shepherd moons and a frozen debris braid." },
    Dest { id: "foundry_run", name: "Foundry Run", map: "foundry", travel: Travel::Drive,
           kind: "mission", blurb: "Smelt channels still running. Mind the heat." },
    Dest { id: "the_trench", name: "The Trench", map: "trench", travel: Travel::Land,
           kind: "mission", blurb: "Rift dark. Your lights are the only ones." },
    Dest { id: "shield_edge", name: "Shield Edge", map: "shield_edge", travel: Travel::Drive,
           kind: "free", blurb: "The boundary is failing. Watch it flicker." },
    Dest { id: "mars_frame", name: "Mars Frame", map: "mars", travel: Travel::Land,
           kind: "mission", blurb: "A piece of Cybertron fell here. It is still warm." },
];

/// Every room must be reachable from the bridge. An upgrade bay nobody can walk to
/// is worse than no upgrade bay, and a graph typo is invisible until someone tries
/// to reach it in game.
pub fn unreachable() -> Vec<&'static str> {
    let mut seen = vec![false; ROOMS.len()];
    let idx = |id: &str| ROOMS.iter().position(|r| r.id == id);
    let mut stack = vec![match idx("bridge") { Some(i) => i, None => return vec!["bridge"] }];
    while let Some(i) = stack.pop() {
        if seen[i] { continue; }
        seen[i] = true;
        for e in ROOMS[i].exits {
            if let Some(j) = idx(e) { if !seen[j] { stack.push(j); } }
        }
    }
    ROOMS.iter().zip(seen).filter(|(_r, s)| !s).map(|(r, _s)| r.id).collect()
}

/// Exits must be mutual: a door you can walk through one way only is a bug in a
/// hub, however reasonable it looks in the table.
pub fn one_way() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for r in ROOMS {
        for e in r.exits {
            match ROOMS.iter().find(|q| q.id == *e) {
                Some(q) if q.exits.contains(&r.id) => {}
                Some(_) => out.push((r.id, *e)),
                None => out.push((r.id, "MISSING")),
            }
        }
    }
    out
}

// ---------------------------------------------------------------- hull fitting

/// A room placed inside a real hull: centre and half-extents in ship-local metres.
#[derive(Clone, Debug)]
pub struct Fitted {
    pub id: &'static str,
    pub kind: u32,
    pub centre: [f32; 3],
    pub half: [f32; 3],
    pub exits: Vec<usize>,
}

/// Interior cross-section of a hull at a given slice along its long axis.
///
/// Measured from the hull's own vertices rather than assumed: ships are not boxes,
/// and a room sized to the bounding box would poke through a tapering nose. Takes
/// the 15th/85th percentile rather than min/max so a single stray vertex or an
/// antenna does not define the room.
fn section(v: &[[f32; 3]], axis: usize, a: f32, b: f32) -> Option<([f32; 3], [f32; 3])> {
    let sel: Vec<&[f32; 3]> = v.iter().filter(|p| p[axis] >= a && p[axis] < b).collect();
    if sel.len() < 24 { return None; }
    let mut lo = [0.0f32; 3];
    let mut hi = [0.0f32; 3];
    for k in 0..3 {
        let mut c: Vec<f32> = sel.iter().map(|p| p[k]).collect();
        c.sort_by(|x, y| x.total_cmp(y));
        lo[k] = c[(c.len() as f32 * 0.15) as usize];
        hi[k] = c[((c.len() as f32 * 0.85) as usize).min(c.len() - 1)];
    }
    Some((lo, hi))
}

/// Fit the room graph into a hull.
///
/// Rooms keep their ORDER along the ship's long axis and their graph, but their
/// positions and sizes come from the hull. That way a stubby lander and a long
/// cruiser both get a bridge forward and a hangar aft without a second layout.
///
/// `shrink` keeps rooms off the skin -- walking into the hull from inside looks
/// worse than a slightly small room.
pub fn fit_to_hull(v: &[[f32; 3]], shrink: f32) -> Vec<Fitted> {
    if v.is_empty() { return Vec::new(); }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    let ext = [hi[0]-lo[0], hi[1]-lo[1], hi[2]-lo[2]];
    // long axis is the ship's spine, whichever way it was modelled
    let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] { 0 }
               else if ext[2] >= ext[1] { 2 } else { 1 };

    // order rooms fore-to-aft by their authored spine coordinate, so the layout's
    // intent survives being remapped onto a different hull
    let mut order: Vec<usize> = (0..ROOMS.len()).collect();
    order.sort_by(|&a, &b| ROOMS[b].pos[2].total_cmp(&ROOMS[a].pos[2]));

    let n = order.len();
    let step = ext[axis] / n as f32;
    // Minimum room size must be RELATIVE to the hull. A fixed 0.6 was written for
    // metres and silently swallowed every room once the mesh was normalised to
    // -1..1: every half-extent clamped to 0.6 on a 2-unit ship, so all eight rooms
    // became the same oversized box and overlapped each other.
    let min_half = ext[0].max(ext[1]).max(ext[2]) * 0.015;
    let mut out: Vec<Fitted> = Vec::with_capacity(n);
    let mut placed: Vec<Option<Fitted>> = vec![None; ROOMS.len()];
    for (slot, &ri) in order.iter().enumerate() {
        let a = lo[axis] + step * slot as f32;
        let b = a + step;
        let (slo, shi) = match section(v, axis, a, b) {
            Some(x) => x,
            // no geometry in this slice: the hull is shorter than the room list,
            // so fall back to the slice bounds rather than dropping a room
            None => ([lo[0], lo[1], a], [hi[0], hi[1], b]),
        };
        let mut centre = [0.0f32; 3];
        let mut half = [0.0f32; 3];
        for k in 0..3 {
            centre[k] = (slo[k] + shi[k]) * 0.5;
            half[k] = ((shi[k] - slo[k]) * 0.5 * shrink).max(min_half);
        }
        centre[axis] = (a + b) * 0.5;
        half[axis] = (step * 0.5 * shrink).max(min_half);
        placed[ri] = Some(Fitted { id: ROOMS[ri].id, kind: ROOMS[ri].kind.code(),
                                   centre, half, exits: Vec::new() });
    }
    for (i, r) in ROOMS.iter().enumerate() {
        if let Some(mut f) = placed[i].take() {
            f.exits = r.exits.iter()
                .filter_map(|e| ROOMS.iter().position(|q| q.id == *e)).collect();
            out.push(f);
        }
    }
    out
}

/// Rooms that overlap each other. Two rooms sharing space is a hub you can fall
/// between, and it is invisible until someone walks the corridor.
pub fn overlaps(f: &[Fitted]) -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for i in 0..f.len() {
        for j in (i + 1)..f.len() {
            let sep = (0..3).any(|k|
                (f[i].centre[k] - f[j].centre[k]).abs() >= f[i].half[k] + f[j].half[k]);
            if !sep { out.push((f[i].id, f[j].id)); }
        }
    }
    out
}

/// Rooms whose box escapes the hull. A room poking through the skin shows the
/// outside from inside, which reads as a hole rather than a window.
pub fn outside_hull(f: &[Fitted], v: &[[f32; 3]]) -> Vec<&'static str> {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in v { for k in 0..3 { lo[k] = lo[k].min(p[k]); hi[k] = hi[k].max(p[k]); } }
    f.iter().filter(|r| (0..3).any(|k|
        r.centre[k] - r.half[k] < lo[k] - 1e-3 || r.centre[k] + r.half[k] > hi[k] + 1e-3))
     .map(|r| r.id).collect()
}

pub fn fitted_header(f: &[Fitted], ship: &str) -> String {
    let mut l = vec![
        format!("// AUTO-GENERATED by assetkit hub --ship {}. Do not edit.", ship),
        "#ifndef HUB_FIT_H".into(), "#define HUB_FIT_H".into(), String::new(),
        "// Room boxes fitted to the hull: centre + half-extents in ship-local metres.".into(),
        "typedef struct { const char *id; int kind; float cx, cy, cz;".into(),
        "                 float hx, hy, hz; int n_exits; const int *exits; } hub_box_t;".into(),
        String::new(),
    ];
    for (i, r) in f.iter().enumerate() {
        l.push(format!("static const int g_fit_exits_{}[] = {{ {} }};", i,
                       r.exits.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ")));
    }
    l.push(String::new());
    l.push(format!("#define NUM_HUB_BOXES {}", f.len()));
    l.push("static const hub_box_t g_hub_boxes[NUM_HUB_BOXES] = {".into());
    for (i, r) in f.iter().enumerate() {
        l.push(format!("  {{ \"{}\", {}, {:.2}f, {:.2}f, {:.2}f, {:.2}f, {:.2}f, {:.2}f, {}, g_fit_exits_{} }},",
                       r.id, r.kind, r.centre[0], r.centre[1], r.centre[2],
                       r.half[0], r.half[1], r.half[2], r.exits.len(), i));
    }
    l.push("};".into());
    l.push(String::new());
    l.push("#endif".into());
    l.join("
") + "
"
}

pub fn to_json() -> String {
    let mut s = String::from("{\n \"_doc\": \"Ship hub: rooms, exits and console destinations. Rooms form a GRAPH and every one must be reachable from the bridge; travel is a mode (drive/fly/land) rather than a loading screen, since the engine already has landing and flight.\",\n \"rooms\": {\n");
    for (i, r) in ROOMS.iter().enumerate() {
        s.push_str(&format!(
            "  \"{}\": {{ \"name\": \"{}\", \"kind\": \"{}\", \"pos\": [{:.1}, {:.1}, {:.1}], \"exits\": [{}], \"blurb\": \"{}\" }}{}\n",
            r.id, r.name, r.kind.tag(), r.pos[0], r.pos[1], r.pos[2],
            r.exits.iter().map(|e| format!("\"{}\"", e)).collect::<Vec<_>>().join(", "),
            r.blurb, if i + 1 < ROOMS.len() { "," } else { "" }));
    }
    s.push_str(" },\n \"destinations\": {\n");
    for (i, d) in DESTS.iter().enumerate() {
        s.push_str(&format!(
            "  \"{}\": {{ \"name\": \"{}\", \"map\": \"{}\", \"travel\": \"{}\", \"kind\": \"{}\", \"blurb\": \"{}\" }}{}\n",
            d.id, d.name, d.map, d.travel.tag(), d.kind, d.blurb,
            if i + 1 < DESTS.len() { "," } else { "" }));
    }
    s.push_str(" }\n}\n");
    s
}

pub fn to_header() -> String {
    let mut l = vec![
        "// AUTO-GENERATED by assetkit hub. Do not edit.".to_string(),
        "#ifndef HUB_PLAN_H".into(), "#define HUB_PLAN_H".into(), String::new(),
        "// room kind: 0 bridge 1 upgrade 2 spacewalk 3 vendor 4 quest 5 hangar 6 quarters".into(),
        "// travel:    0 drive  1 fly     2 land      3 walk".into(),
        "typedef struct { const char *id; const char *name; int kind; float x, y, z;".into(),
        "                 int n_exits; const int *exits; const char *blurb; } hub_room_t;".into(),
        "typedef struct { const char *id; const char *name; const char *map;".into(),
        "                 int travel; const char *kind; const char *blurb; } hub_dest_t;".into(),
        String::new(),
    ];
    for (i, r) in ROOMS.iter().enumerate() {
        let idxs: Vec<String> = r.exits.iter()
            .filter_map(|e| ROOMS.iter().position(|q| q.id == *e))
            .map(|p| p.to_string()).collect();
        l.push(format!("static const int g_hub_exits_{}[] = {{ {} }};", i, idxs.join(", ")));
    }
    l.push(String::new());
    l.push(format!("#define NUM_HUB_ROOMS {}", ROOMS.len()));
    l.push("static const hub_room_t g_hub_rooms[NUM_HUB_ROOMS] = {".into());
    for (i, r) in ROOMS.iter().enumerate() {
        l.push(format!("  {{ \"{}\", \"{}\", {}, {:.1}f, {:.1}f, {:.1}f, {}, g_hub_exits_{}, \"{}\" }},",
                       r.id, r.name, r.kind.code(), r.pos[0], r.pos[1], r.pos[2],
                       r.exits.len(), i, r.blurb));
    }
    l.push("};".into());
    l.push(String::new());
    l.push(format!("#define NUM_HUB_DESTS {}", DESTS.len()));
    l.push("static const hub_dest_t g_hub_dests[NUM_HUB_DESTS] = {".into());
    for d in DESTS {
        l.push(format!("  {{ \"{}\", \"{}\", \"{}\", {}, \"{}\", \"{}\" }},",
                       d.id, d.name, d.map, d.travel.code(), d.kind, d.blurb));
    }
    l.push("};".into());
    l.push(String::new());
    l.push("#endif".into());
    l.join("\n") + "\n"
}
