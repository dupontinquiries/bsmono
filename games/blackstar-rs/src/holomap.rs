//! HOLOMAP — the solar system on a console table, in a choice of skins.
//!
//! The Starmap station is a list: twenty places and what each will do to
//! you. Correct, and it looks like a spreadsheet on a three-kilometre
//! carrier. This module is the other presentation — the system as a
//! HOLOGRAM over the plot table, worlds moving on their orbits, the
//! current transfer drawn as the ellipse it actually is — built headless
//! in the house shape: a pure `scene()` that emits a draw list, so the
//! same answer drives the renderer, the ASCII sim, and a test without
//! three copies of the arithmetic.
//!
//! **The hard problem is 44 AU on a 1.4 m table.** Linear scale puts
//! Earth 3 cm from the sun and Mercury inside it; every real orrery
//! cheats, and this one cheats HONESTLY: radii are LOG-compressed
//! ([`table_r`]), which keeps order and adjacency truthful while giving
//! the inner system room to read. The compression is one function with
//! tests pinning monotonicity and the inner-separation floor, because a
//! map that reorders worlds is worse than a spreadsheet.
//!
//! **Motion is Kepler's, sped up.** Period goes as au^1.5, so Titan
//! visibly crawls while Mars laps it — the ordering is the physics even
//! though the clock is theatrical. Phases hash from the world id, so the
//! system never opens as a police lineup of planets on one ray.
//!
//! **Skins are data, and the ink is the whole difference.** A [`Skin`]
//! is a palette, a ring style, and a sweep rate — three console
//! characters players cycle with one control:
//!
//!   * `COALITION TACNET` — cold cyan wireframe, fast sweep, dashed
//!     rings: the military plot.
//!   * `FORGE ORRERY`     — ember and brass, slow sweep, solid rings:
//!     the Cybertronian instrument, built not rendered.
//!   * `HIJACK VEIL`      — violet on near-black, no grid, slowest
//!     sweep: the stolen chart that does not want to be read.
//!
//! **The AU chart lives here and is machine-checked.** `patrol::ROUTE`
//! carries no orbital radii, and a second table that could drift from
//! the route is exactly the voiceprint-parity failure mode — so
//! `every_route_world_is_on_the_chart` fails the moment a destination
//! ships without a place on the map.
//!
//! `--holo [skin]` renders the table in ASCII at a few beats, cycles the
//! skins, and draws a sample transfer — the visible loop, per the
//! `--raid`/`--fold` pattern. What no test here claims: how it LOOKS.
//! There is no display in this sandbox; the draw list is verified, the
//! glow is not.

use glam::{Vec2, Vec3};

/// Console-local table radius, metres. The plot fits inside this.
pub const R_TABLE: f32 = 0.70;
/// The innermost ring, so the sun's own glyph has room.
pub const R_INNER: f32 = 0.10;
/// One visual year for a 1 AU orbit, seconds. Theatrical, not physical.
pub const YEAR_S: f32 = 40.0;

/// One world on the chart: patrol id, semi-major axis, a visual weight.
pub struct ChartBody {
    pub id: &'static str,
    pub au: f32,
    /// glyph size on the table, relative 0..1 — a gas giant reads bigger
    /// than a moonlet even though neither is to scale
    pub weight: f32,
}

/// The chart. AUs are the real system's where the world is real, and the
/// lore's where it is not (the clone homeworlds sit in the outer dark).
/// Moons ride at their PRIMARY's radius with a small offset — the map is
/// heliocentric and a moon three pixels from its giant says more than a
/// moon on its own invisible ring.
pub const CHART: &[ChartBody] = &[
    ChartBody { id: "earth", au: 1.00, weight: 0.55 },
    ChartBody { id: "luna", au: 1.04, weight: 0.25 },
    ChartBody { id: "mars", au: 1.52, weight: 0.45 },
    ChartBody { id: "ceres", au: 2.77, weight: 0.30 },
    ChartBody { id: "europa", au: 5.20, weight: 0.30 },
    ChartBody { id: "io", au: 5.10, weight: 0.30 },
    ChartBody { id: "titan", au: 9.54, weight: 0.35 },
    ChartBody { id: "enceladus", au: 9.60, weight: 0.25 },
    ChartBody { id: "triton", au: 30.1, weight: 0.30 },
    ChartBody { id: "pluto", au: 39.5, weight: 0.30 },
    ChartBody { id: "glaciem", au: 46.0, weight: 0.40 },
    ChartBody { id: "carrion", au: 50.0, weight: 0.40 },
    ChartBody { id: "helix", au: 54.0, weight: 0.40 },
    ChartBody { id: "outer_rim", au: 44.0, weight: 0.35 },
    ChartBody { id: "shield_edge", au: 55.0, weight: 0.35 },
];

/// DESTINATIONS THIS TABLE CANNOT RING. Cybertron and the exoplanets
/// orbit OTHER STARS, and `uncharted` is the wildcard; a heliocentric
/// plot that gave them an AU would be lying with geometry. A real chart
/// marks them as BEARINGS at the rim — a direction and a name, distance
/// unplottable — which is also the cinematic answer: distant contacts
/// at the table's edge. Bearings hash from the id, stable forever.
pub const BEACONS: &[&str] =
    &["cybertron", "proxima_b", "trappist1e", "trappist1g", "kepler186f", "uncharted"];

/// A beacon's rim bearing, radians — hashed, not authored, same as the
/// orbital phases.
pub fn beacon_bearing(id: &str) -> f32 {
    let mut h: u32 = 2166136261;
    for b in id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    (h % 6283) as f32 / 1000.0
}

/// AU bounds the compression maps between. Derived from the chart at
/// compile-worthy cost rather than constants that drift from it.
fn au_bounds() -> (f32, f32) {
    let lo = CHART.iter().map(|b| b.au).fold(f32::MAX, f32::min);
    let hi = CHART.iter().map(|b| b.au).fold(0.0f32, f32::max);
    (lo * 0.8, hi * 1.1)
}

/// THE CHEAT, stated: log-compress an orbital radius onto the table.
/// Monotonic (order survives), bounded (everything fits), and generous
/// to the inner system (where the game mostly happens).
pub fn table_r(au: f32) -> f32 {
    let (lo, hi) = au_bounds();
    let x = (au.max(lo) / lo).ln() / (hi / lo).ln();
    R_INNER + x.clamp(0.0, 1.0) * (R_TABLE - R_INNER)
}

/// Kepler phase at time `t`: period ∝ au^1.5, phase seeded from the id
/// so the system opens mid-motion, not in a lineup.
pub fn angle_at(id: &str, au: f32, t: f32) -> f32 {
    let mut h: u32 = 2166136261;
    for b in id.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    let phase0 = (h % 6283) as f32 / 1000.0;
    phase0 + t * std::f32::consts::TAU / (YEAR_S * au.powf(1.5))
}

/// A skin: the console's whole character as data.
pub struct Skin {
    pub id: &'static str,
    pub name: &'static str,
    /// [sun, world, ring, grid, route] as rgba
    pub ink: [[f32; 4]; 5],
    pub rings_dashed: bool,
    pub grid: bool,
    /// radar-sweep revolutions per second; 0 disables the sweep
    pub sweep_hz: f32,
    pub note: &'static str,
}

pub const SKINS: &[Skin] = &[
    Skin {
        id: "tacnet",
        name: "COALITION TACNET",
        ink: [
            [0.85, 0.95, 1.00, 1.0],
            [0.45, 0.85, 1.00, 1.0],
            [0.20, 0.55, 0.75, 0.6],
            [0.10, 0.30, 0.45, 0.35],
            [1.00, 0.75, 0.25, 1.0],
        ],
        rings_dashed: true,
        grid: true,
        sweep_hz: 0.50,
        note: "the military plot: cold, gridded, and it never stops sweeping",
    },
    Skin {
        id: "orrery",
        name: "FORGE ORRERY",
        ink: [
            [1.00, 0.82, 0.45, 1.0],
            [1.00, 0.60, 0.25, 1.0],
            [0.75, 0.45, 0.20, 0.8],
            [0.30, 0.18, 0.08, 0.30],
            [0.55, 1.00, 0.75, 1.0],
        ],
        rings_dashed: false,
        grid: false,
        sweep_hz: 0.12,
        note: "the Cybertronian instrument: ember and brass, built rather than rendered",
    },
    Skin {
        id: "veil",
        name: "HIJACK VEIL",
        ink: [
            [0.80, 0.60, 1.00, 0.9],
            [0.60, 0.40, 0.95, 0.9],
            [0.35, 0.20, 0.55, 0.5],
            [0.0, 0.0, 0.0, 0.0],
            [1.00, 0.35, 0.55, 1.0],
        ],
        rings_dashed: true,
        grid: false,
        sweep_hz: 0.05,
        note: "the stolen chart: violet on black, and it does not want to be read",
    },
];

/// One element of the hologram, in console-local metres (table centre at
/// the origin, +Y up off the table). The renderer draws these; the sim
/// rasterises them to characters; the tests count and measure them.
pub enum Glyph {
    Ring { r: f32, color: [f32; 4], dashed: bool },
    Body { pos: Vec3, size: f32, color: [f32; 4], label: &'static str },
    /// the transfer, as a polyline sampled from the true half-ellipse
    Route { pts: Vec<Vec2>, color: [f32; 4] },
    Sweep { angle: f32, color: [f32; 4] },
    Grid { color: [f32; 4] },
    /// an extrasolar contact at the rim: a bearing, not an orbit
    Beacon { angle: f32, color: [f32; 4], label: &'static str },
}

/// One console's state: which skin, and what the plot is showing.
pub struct Holo {
    pub skin: usize,
    /// a live transfer to draw: (from_au, to_au, progress 0..1)
    pub route: Option<(f32, f32, f32)>,
}

impl Default for Holo {
    fn default() -> Self {
        Holo { skin: 0, route: None }
    }
}

impl Holo {
    pub fn skin(&self) -> &'static Skin {
        &SKINS[self.skin % SKINS.len()]
    }

    /// One control, both directions, wraps.
    pub fn cycle_skin(&mut self, d: i32) {
        let n = SKINS.len() as i32;
        self.skin = ((self.skin as i32 + d).rem_euclid(n)) as usize;
    }

    /// The cinematic camera: a slow orbit over the table, tilted down at
    /// it, breathing with the skin's own sweep tempo. Pure of renderer:
    /// (eye, look-at) in console space.
    pub fn cam(&self, t: f32) -> (Vec3, Vec3) {
        let s = self.skin();
        let a = t * (0.03 + s.sweep_hz * 0.06) * std::f32::consts::TAU;
        let eye = Vec3::new(a.cos() * 1.15, 0.85, a.sin() * 1.15);
        (eye, Vec3::new(0.0, 0.12, 0.0))
    }

    /// THE SCENE at time `t` — every glyph the hologram shows, themed by
    /// the current skin. Deterministic in `t`, so two consoles on two
    /// ships in one session show the same sky.
    pub fn scene(&self, t: f32) -> Vec<Glyph> {
        let s = self.skin();
        let mut out = Vec::new();
        if s.grid {
            out.push(Glyph::Grid { color: s.ink[3] });
        }
        // one ring per DISTINCT radius (moons share their primary's ring)
        let mut radii: Vec<f32> = CHART.iter().map(|b| table_r(b.au)).collect();
        radii.sort_by(|a, b| a.partial_cmp(b).unwrap());
        radii.dedup_by(|a, b| (*a - *b).abs() < 0.012);
        for r in radii {
            out.push(Glyph::Ring { r, color: s.ink[2], dashed: s.rings_dashed });
        }
        out.push(Glyph::Body {
            pos: Vec3::ZERO,
            size: 0.045,
            color: s.ink[0],
            label: "SOL",
        });
        for b in CHART {
            let a = angle_at(b.id, b.au, t);
            let r = table_r(b.au);
            out.push(Glyph::Body {
                pos: Vec3::new(a.cos() * r, 0.0, a.sin() * r),
                size: 0.012 + b.weight * 0.020,
                color: s.ink[1],
                label: b.id,
            });
        }
        for id in BEACONS {
            out.push(Glyph::Beacon { angle: beacon_bearing(id), color: s.ink[0], label: id });
        }
        if let Some((from, to, progress)) = self.route {
            out.push(Glyph::Route { pts: transfer_arc(from, to, progress), color: s.ink[4] });
        }
        if s.sweep_hz > 0.0 {
            out.push(Glyph::Sweep {
                angle: (t * s.sweep_hz).fract() * std::f32::consts::TAU,
                color: s.ink[2],
            });
        }
        out
    }
}

/// The Hohmann half-ellipse from `from_au` to `to_au`, sampled in AU and
/// THEN compressed point by point — compressing the endpoints and
/// lerping between them would draw a spiral that touches neither orbit.
/// `progress` clips the polyline to how far along the leg the ship is,
/// so the plot shows the road travelled, not the whole road.
pub fn transfer_arc(from_au: f32, to_au: f32, progress: f32) -> Vec<Vec2> {
    let a = (from_au + to_au) * 0.5; // semi-major of the transfer
    let e = (to_au - from_au).abs() / (to_au + from_au);
    let n = 48usize;
    let end = ((progress.clamp(0.0, 1.0) * n as f32) as usize).max(1);
    let outward = to_au >= from_au;
    (0..=end.min(n))
        .map(|i| {
            // true anomaly runs 0..π from periapsis; an outward leg starts
            // at periapsis (the inner orbit), an inward one at apoapsis
            let th = i as f32 / n as f32 * std::f32::consts::PI;
            let th = if outward { th } else { std::f32::consts::PI - th };
            let r_au = a * (1.0 - e * e) / (1.0 + e * th.cos());
            let r = table_r(r_au);
            Vec2::new(th.cos() * r, th.sin() * r)
        })
        .collect()
}

// --------------------------------------------------------------- sim ---

/// `--holo [skin]` — the table in characters, at a few beats, in every
/// skin, with a sample Earth→Titan transfer half-flown.
pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--holo").unwrap();
    let mut h = Holo::default();
    if let Some(want) = argv.get(i + 1) {
        if let Some(k) = SKINS.iter().position(|s| s.id == want.as_str()) {
            h.skin = k;
        }
    }
    h.route = Some((1.0, 9.54, 0.55));
    for _ in 0..SKINS.len() {
        let s = h.skin();
        println!("== {}  ({})", s.name, s.note);
        ascii(&h, 30.0);
        h.cycle_skin(1);
    }
    println!("(one control cycles the skins; the same scene() drives the renderer)");
}

fn ascii(h: &Holo, t: f32) {
    const W: i32 = 63;
    const HH: i32 = 27;
    let mut g = vec![vec![' '; W as usize]; HH as usize];
    let put = |g: &mut Vec<Vec<char>>, x: f32, z: f32, c: char| {
        let cx = ((x / R_TABLE) * (W as f32 / 2.2) + W as f32 / 2.0) as i32;
        let cz = ((z / R_TABLE) * (HH as f32 / 2.2) + HH as f32 / 2.0) as i32;
        if cx >= 0 && cx < W && cz >= 0 && cz < HH {
            g[cz as usize][cx as usize] = c;
        }
    };
    for gl in h.scene(t) {
        match gl {
            Glyph::Ring { r, dashed, .. } => {
                let n = 90;
                for k in 0..n {
                    if dashed && k % 4 >= 2 {
                        continue;
                    }
                    let a = k as f32 / n as f32 * std::f32::consts::TAU;
                    put(&mut g, a.cos() * r, a.sin() * r, '.');
                }
            }
            Glyph::Route { pts, .. } => {
                for p in pts {
                    put(&mut g, p.x, p.y, '=');
                }
            }
            Glyph::Body { pos, label, .. } => {
                let c = if label == "SOL" { '@' } else { label.chars().next().unwrap_or('?').to_ascii_uppercase() };
                put(&mut g, pos.x, pos.z, c);
            }
            Glyph::Sweep { angle, .. } => {
                for k in 0..24 {
                    let r = R_INNER + k as f32 / 24.0 * (R_TABLE - R_INNER);
                    put(&mut g, angle.cos() * r, angle.sin() * r, '/');
                }
            }
            Glyph::Beacon { angle, .. } => {
                put(&mut g, angle.cos() * R_TABLE, angle.sin() * R_TABLE, '*');
            }
            Glyph::Grid { .. } => {}
        }
    }
    for row in g {
        println!("   {}", row.into_iter().collect::<String>());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_route_world_is_on_the_chart() {
        // the drift check, with a floor so an empty scan cannot pass
        assert!(crate::patrol::ROUTE.len() >= 10, "the route shrank under the scan");
        for d in crate::patrol::ROUTE {
            assert!(
                CHART.iter().any(|b| b.id == d.id) || BEACONS.contains(&d.id),
                "{} ships on the route and has no place on the map",
                d.id
            );
        }
    }

    #[test]
    fn compression_keeps_order_and_fits_the_table() {
        let mut sorted: Vec<&ChartBody> = CHART.iter().collect();
        sorted.sort_by(|a, b| a.au.partial_cmp(&b.au).unwrap());
        for w in sorted.windows(2) {
            assert!(
                table_r(w[1].au) >= table_r(w[0].au),
                "the map reordered {} and {}",
                w[0].id,
                w[1].id
            );
        }
        for b in CHART {
            let r = table_r(b.au);
            assert!(r >= R_INNER - 1e-6 && r <= R_TABLE + 1e-6, "{} off the table: {r}", b.id);
        }
        // the inner system gets ROOM — earth and mars must be readable
        // apart, which is the whole point of paying for a log scale
        assert!(table_r(1.52) - table_r(1.0) > 0.04, "the inner system collapsed");
    }

    #[test]
    fn kepler_ordering_outer_worlds_crawl() {
        let dt = 5.0;
        let inner = (angle_at("mars", 1.52, dt) - angle_at("mars", 1.52, 0.0)).abs();
        let outer = (angle_at("pluto", 39.5, dt) - angle_at("pluto", 39.5, 0.0)).abs();
        assert!(inner > outer * 5.0, "mars {inner} vs pluto {outer}: the sky lost its physics");
    }

    #[test]
    fn skins_are_plural_distinct_and_cycle_wraps() {
        assert!(SKINS.len() >= 2, "\"a couple\" means at least two");
        for i in 0..SKINS.len() {
            for j in i + 1..SKINS.len() {
                assert!(
                    SKINS[i].ink != SKINS[j].ink,
                    "{} and {} are one skin in two coats",
                    SKINS[i].id,
                    SKINS[j].id
                );
            }
        }
        let mut h = Holo::default();
        for _ in 0..SKINS.len() {
            h.cycle_skin(1);
        }
        assert_eq!(h.skin, 0, "a full forward cycle must come home");
        h.cycle_skin(-1);
        assert_eq!(h.skin, SKINS.len() - 1, "backward must wrap too");
    }

    #[test]
    fn the_transfer_arc_touches_both_orbits() {
        for (from, to) in [(1.0f32, 9.54f32), (9.54, 1.0)] {
            let pts = transfer_arc(from, to, 1.0);
            let first = pts.first().unwrap().length();
            let last = pts.last().unwrap().length();
            assert!((first - table_r(from)).abs() < 0.01, "{from}->{to}: departure off its orbit");
            assert!((last - table_r(to)).abs() < 0.01, "{from}->{to}: arrival off its orbit");
            for p in &pts {
                assert!(p.length() <= R_TABLE + 0.02, "the arc left the table");
            }
        }
        // progress clips: half a leg is visibly shorter than the whole
        assert!(transfer_arc(1.0, 9.54, 0.5).len() < transfer_arc(1.0, 9.54, 1.0).len());
    }

    #[test]
    fn the_scene_carries_the_sky() {
        let mut h = Holo::default();
        h.route = Some((1.0, 9.54, 0.5));
        let scene = h.scene(12.0);
        let bodies = scene.iter().filter(|g| matches!(g, Glyph::Body { .. })).count();
        assert_eq!(bodies, CHART.len() + 1, "a world (or the sun) fell off the plot");
        let beacons = scene.iter().filter(|g| matches!(g, Glyph::Beacon { .. })).count();
        assert_eq!(beacons, BEACONS.len(), "an extrasolar contact fell off the rim");
        assert!(scene.iter().any(|g| matches!(g, Glyph::Route { .. })), "the live leg is not drawn");
        // determinism: two consoles on two ships show one sky
        let again = h.scene(12.0);
        assert_eq!(scene.len(), again.len());
    }
}
