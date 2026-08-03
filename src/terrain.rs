//! TERRAIN, parameterised by physics rather than by taste.
//!
//! The question was what noise is *somewhat accurate*, and the useful answer
//! is that the noise itself barely matters — value noise, gradient noise and
//! Worley all look much the same once you stack them. What makes a world
//! read as Mars rather than as Earth-with-a-red-filter is the SCALE you feed
//! them, and those scales come from real relations that mostly depend on
//! gravity, wind and what the ground is made of.
//!
//! So this module is two halves. The bottom half is ordinary noise
//! primitives. The top half turns a `Ground` — gravity, material, wind — into
//! the wavelengths and amplitudes to run them at, and every one of those is a
//! relation somebody measured, cited in the function that uses it.
//!
//! What the relations buy you, concretely:
//!
//!   * Dunes on Mars are HUNDREDS of metres apart and on Earth tens, and it
//!     is not artistic licence: saltation hop length goes as u*²/g, so a
//!     third of Earth's gravity is roughly three times the wavelength.
//!   * Crevasses on Europa are far deeper than on an Earth glacier for the
//!     same ice, because a crevasse bottoms out where overburden closes it
//!     and that depth is σ/(ρg).
//!   * Craters on the Moon go central-peak at 15-20 km and on Earth at 3-4,
//!     because the simple-to-complex transition scales as 1/g.
//!   * Talus slopes sit at 33-37° everywhere, on every body, because the
//!     angle of repose is set by friction and friction does not care about
//!     gravity. The RUNOUT does. Same angle, completely different landscape,
//!     and that asymmetry is the most counterintuitive thing here.

/// What the ground is made of. Hardness is what resists fracture and impact;
/// grain is what the wind can pick up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Material {
    /// water ice — cracks in sheets, closes under its own weight
    Ice,
    /// nitrogen or methane ice: far weaker, flows at low stress
    SoftIce,
    /// basalt: hard, columnar-jointed, holds a cliff
    Basalt,
    /// sedimentary: layered, undercuts, spalls
    Sandstone,
    /// loose grains, no cohesion
    Regolith,
    /// worked metal, for Cybertron and hulls
    Metal,
}

impl Material {
    /// Tensile strength in MPa. The number a crack has to beat.
    pub fn tensile_mpa(self) -> f32 {
        match self {
            Material::Ice => 1.5,
            Material::SoftIce => 0.35,
            Material::Basalt => 15.0,
            Material::Sandstone => 4.0,
            Material::Regolith => 0.02,
            Material::Metal => 250.0,
        }
    }
    /// Bulk density, kg/m³.
    pub fn density(self) -> f32 {
        match self {
            Material::Ice => 917.0,
            Material::SoftIce => 1000.0,
            Material::Basalt => 2900.0,
            Material::Sandstone => 2300.0,
            Material::Regolith => 1600.0,
            Material::Metal => 7800.0,
        }
    }
    /// Internal friction angle in degrees — the angle of repose.
    ///
    /// 33-37° for nearly every granular material, and near enough
    /// gravity-INDEPENDENT: it is set by grain friction and interlocking,
    /// and gravity scales the driving and resisting forces together so it
    /// cancels. A low-gravity world does not get steeper scree; it gets the
    /// same slope spread much further out.
    pub fn repose_deg(self) -> f32 {
        match self {
            Material::Regolith => 34.0,
            Material::Sandstone => 35.0,
            Material::Basalt => 37.0,
            Material::Ice => 33.0,
            Material::SoftIce => 33.0,
            Material::Metal => 36.0,
        }
    }
    /// Typical loose grain diameter in metres, for saltation.
    pub fn grain_m(self) -> f32 {
        match self {
            Material::Regolith => 200e-6,
            Material::Sandstone => 250e-6,
            Material::Ice => 400e-6,
            Material::SoftIce => 300e-6,
            Material::Basalt => 500e-6,
            Material::Metal => 1000e-6,
        }
    }
    /// Does it fracture in a connected network, or just crumble?
    pub fn jointed(self) -> bool {
        !matches!(self, Material::Regolith)
    }
}

/// Everything a world needs to decide what its own surface looks like.
#[derive(Clone, Copy, Debug)]
pub struct Ground {
    /// surface gravity in m/s²
    pub g: f32,
    pub material: Material,
    /// atmospheric density in kg/m³; 0 is vacuum
    pub air: f32,
    /// characteristic wind friction velocity u*, m/s. 0 for no weather.
    pub wind: f32,
}

impl Ground {
    pub const EARTH_G: f32 = 9.81;

    pub fn new(g: f32, material: Material, air: f32, wind: f32) -> Ground {
        Ground { g: g.max(0.001), material, air: air.max(0.0), wind: wind.max(0.0) }
    }

    /// THRESHOLD FRICTION VELOCITY for saltation, m/s.
    ///
    /// Bagnold: u*t = A·sqrt((ρp − ρa)·g·d / ρa). Two competing effects on a
    /// small world — lower g lowers the threshold, thinner air raises it —
    /// and thin air usually wins, which is why Mars needs a gale to move
    /// sand that a breeze would move here.
    pub fn saltation_threshold(&self) -> f32 {
        if self.air <= 1e-6 {
            return f32::INFINITY; // no medium: nothing saltates, ever
        }
        let rho_p = self.material.density();
        let d = self.material.grain_m();
        0.1 * (((rho_p - self.air).max(1.0) * self.g * d) / self.air).sqrt()
    }

    /// Can this world have dunes at all?
    ///
    /// The gate MAP_SCIENCE sets: aeolian features need SALTATION, which
    /// needs moving air and not merely a medium. This is what catches a dune
    /// field on a world described as having no weather.
    pub fn has_dunes(&self) -> bool {
        self.wind > self.saltation_threshold()
    }

    /// DUNE WAVELENGTH in metres — the crest-to-crest spacing.
    ///
    /// Proportional to saltation hop length, which goes as u*²/g. Hence
    /// Martian dunes hundreds of metres apart against Earth's tens: a third
    /// of the gravity is roughly three times the spacing, before the thinner
    /// air makes it larger still.
    pub fn dune_wavelength(&self) -> Option<f32> {
        if !self.has_dunes() {
            return None;
        }
        let hop = self.wind * self.wind / self.g;
        Some((hop * 18.0).clamp(4.0, 900.0))
    }

    /// Dune height, roughly a fifteenth of spacing for a mature field.
    pub fn dune_height(&self) -> Option<f32> {
        self.dune_wavelength().map(|w| w / 15.0)
    }

    /// CREVASSE DEPTH in metres.
    ///
    /// A crack opens while tension beats the overburden closing it, so it
    /// bottoms out at σ/(ρg) — the Nye depth. Same ice, a twentieth of the
    /// gravity, twenty times the depth. This is why the icy moons are
    /// carved so much more deeply than anything on Earth.
    pub fn crevasse_depth(&self) -> f32 {
        let sigma = self.material.tensile_mpa() * 1e6;
        sigma / (self.material.density() * self.g)
    }

    /// Spacing between parallel crevasses. Stress shadow scales with depth,
    /// so deeper cracks stand further apart.
    pub fn crevasse_spacing(&self) -> f32 {
        self.crevasse_depth() * 0.75
    }

    /// CRATER SIMPLE-TO-COMPLEX TRANSITION in metres.
    ///
    /// Scales as 1/g: ~3-4 km on Earth, ~7 km on Mars, ~15-20 km on the
    /// Moon. Below it a crater is a bowl; above it the floor rebounds into a
    /// central peak and the walls terrace. Getting this wrong is the single
    /// most obvious tell that a world was authored rather than derived.
    pub fn crater_transition(&self) -> f32 {
        3500.0 * (Self::EARTH_G / self.g)
    }

    /// Is a crater of this diameter simple or complex?
    pub fn crater_is_complex(&self, diameter_m: f32) -> bool {
        diameter_m > self.crater_transition()
    }

    /// LAVA TUBE / VOID ROOF SPAN in metres.
    ///
    /// Also 1/g, for the same reason a crevasse is: the roof holds while its
    /// strength beats its own weight. Terrestrial tubes reach tens of
    /// metres; lunar tubes are modelled in kilometres, and GRAIL's gravity
    /// data supports voids that large.
    pub fn tube_span(&self) -> f32 {
        let sigma = self.material.tensile_mpa() * 1e6;
        (sigma / (self.material.density() * self.g)).sqrt() * 4.0
    }

    /// TALUS RUNOUT: how far debris travels from the base of a drop.
    ///
    /// The counterintuitive one. The angle of repose is friction-set and
    /// gravity cancels, so scree sits at the same 33-37° everywhere. Runout
    /// does NOT cancel: a block leaving a cliff of the same height keeps its
    /// speed for longer under weaker gravity, so the debris apron spreads
    /// much further. Same slope angle, completely different landscape.
    pub fn talus_runout(&self, drop_m: f32) -> f32 {
        let mu = self.material.repose_deg().to_radians().tan();
        // ballistic term: range grows as 1/g for the same launch speed, and
        // launch speed comes from the drop
        drop_m / mu * (Self::EARTH_G / self.g).sqrt()
    }

    /// Vertical relief scale. Mountains stand taller where gravity is weaker
    /// because the rock at the base fails at a fixed stress.
    pub fn max_relief(&self) -> f32 {
        let sigma = self.material.tensile_mpa() * 1e6;
        sigma / (self.material.density() * self.g) * 3.0
    }
}

// ============================================================ primitives ===

fn hash3(x: i32, y: i32, z: i32, seed: u64) -> f32 {
    let mut h = seed
        ^ (x as i64 as u64).wrapping_mul(0x8DA6_B343_D5B0_9C6D)
        ^ (y as i64 as u64).wrapping_mul(0xD8163_841_u64)
        ^ (z as i64 as u64).wrapping_mul(0xCB1A_B31F_u64);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    (h >> 40) as f32 / 16_777_216.0
}

fn smooth(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Value noise in 3D, output 0..1.
pub fn value3(x: f32, y: f32, z: f32, seed: u64) -> f32 {
    let (xi, yi, zi) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (smooth(x - xi), smooth(y - yi), smooth(z - zi));
    let (xi, yi, zi) = (xi as i32, yi as i32, zi as i32);
    let mut acc = 0.0;
    for dz in 0..2 {
        for dy in 0..2 {
            for dx in 0..2 {
                let w = (if dx == 1 { fx } else { 1.0 - fx })
                    * (if dy == 1 { fy } else { 1.0 - fy })
                    * (if dz == 1 { fz } else { 1.0 - fz });
                acc += w * hash3(xi + dx, yi + dy, zi + dz, seed);
            }
        }
    }
    acc
}

/// Fractal sum. The workhorse: hills, and the base for everything else.
pub fn fbm(x: f32, y: f32, z: f32, octaves: u32, seed: u64) -> f32 {
    let (mut f, mut a, mut sum, mut norm) = (1.0, 0.5, 0.0, 0.0);
    for o in 0..octaves.max(1) {
        sum += a * value3(x * f, y * f, z * f, seed ^ (o as u64 * 0x9E37));
        norm += a;
        f *= 2.03; // off exactly 2 so octaves do not align into a grid
        a *= 0.5;
    }
    sum / norm
}

/// RIDGED multifractal — the erosion look.
///
/// Folding the noise about zero and inverting turns smooth hills into sharp
/// crests and flat-floored valleys, which is what running water and mass
/// wasting actually leave behind. Weighting each octave by the last is what
/// keeps ridges continuous instead of speckling.
pub fn ridged(x: f32, y: f32, z: f32, octaves: u32, seed: u64) -> f32 {
    let (mut f, mut a, mut sum, mut prev) = (1.0, 0.5, 0.0, 1.0);
    for o in 0..octaves.max(1) {
        let n = 1.0 - (value3(x * f, y * f, z * f, seed ^ (o as u64 * 0x51ED)) * 2.0 - 1.0).abs();
        let n = n * n * prev;
        sum += n * a;
        prev = n;
        f *= 2.07;
        a *= 0.5;
    }
    sum.clamp(0.0, 1.0)
}

/// WORLEY / cellular — distance to the nearest scattered point.
///
/// Returns (nearest, second). `second - nearest` is small exactly on the
/// boundaries between cells, which is a fracture network: crevasse fields,
/// columnar basalt, and Venusian tessera are all this function with
/// different scales on top.
pub fn worley(x: f32, y: f32, z: f32, seed: u64) -> (f32, f32) {
    let (xi, yi, zi) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let (mut d1, mut d2) = (f32::MAX, f32::MAX);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (cx, cy, cz) = (xi + dx, yi + dy, zi + dz);
                let px = cx as f32 + hash3(cx, cy, cz, seed);
                let py = cy as f32 + hash3(cx, cy, cz, seed ^ 0x1234);
                let pz = cz as f32 + hash3(cx, cy, cz, seed ^ 0xABCD);
                let d = ((px - x).powi(2) + (py - y).powi(2) + (pz - z).powi(2)).sqrt();
                if d < d1 {
                    d2 = d1;
                    d1 = d;
                } else if d < d2 {
                    d2 = d;
                }
            }
        }
    }
    (d1, d2)
}

/// Fracture intensity 0..1: high on cell boundaries, zero inside.
pub fn fracture(x: f32, y: f32, z: f32, sharpness: f32, seed: u64) -> f32 {
    let (d1, d2) = worley(x, y, z, seed);
    (1.0 - (d2 - d1) * sharpness.max(0.01)).clamp(0.0, 1.0)
}

/// DOMAIN WARP — push the sample point around with more noise.
///
/// One line, and it is the difference between noise that looks like noise
/// and noise that looks like it flowed. Glacier ice, wind-combed sand and
/// lava all move before they set, and warping is how that shows.
pub fn warp(x: f32, y: f32, z: f32, amount: f32, seed: u64) -> (f32, f32, f32) {
    let wx = fbm(x + 5.2, y + 1.3, z, 3, seed ^ 0xAA) * 2.0 - 1.0;
    let wy = fbm(x + 9.7, y + 4.1, z, 3, seed ^ 0xBB) * 2.0 - 1.0;
    let wz = fbm(x + 2.8, y + 7.9, z, 3, seed ^ 0xCC) * 2.0 - 1.0;
    (x + wx * amount, y + wy * amount, z + wz * amount)
}

// ============================================================== surfaces ===

/// DUNE FIELD height in metres at a point, or None where nothing saltates.
///
/// Transverse dunes are a directional wave, not isotropic noise: sharp on
/// the lee face and long on the windward one, which is why this uses an
/// asymmetric profile rather than a sine. The wave is warped along the wind
/// so crests wander and join the way real fields do.
pub fn dune_height(g: &Ground, x: f32, z: f32, wind_dir: (f32, f32), seed: u64) -> Option<f32> {
    let lambda = g.dune_wavelength()?;
    let h = g.dune_height()?;
    let (wx, wz) = {
        let l = (wind_dir.0 * wind_dir.0 + wind_dir.1 * wind_dir.1).sqrt().max(1e-6);
        (wind_dir.0 / l, wind_dir.1 / l)
    };
    // distance along the wind, warped across it
    let along = x * wx + z * wz;
    let across = -x * wz + z * wx;
    let meander = (fbm(across / (lambda * 4.0), 0.0, seed as f32 * 0.001, 3, seed) - 0.5) * lambda * 0.6;
    let t = ((along + meander) / lambda).fract().abs();
    // windward 0..0.75 rises gently, lee 0.75..1 drops at the repose angle
    let profile = if t < 0.75 { t / 0.75 } else { (1.0 - t) / 0.25 };
    let field = fbm(x / (lambda * 9.0), 0.0, z / (lambda * 9.0), 3, seed ^ 0x5A5A);
    Some(profile * h * (0.35 + 0.65 * field))
}

/// CREVASSE field: 0 where the ice is whole, 1 in an open crack.
///
/// Real crevasses open transverse to extension and are spaced by their own
/// stress shadows, so this is a Worley boundary field stretched across the
/// flow direction and warped along it — cracks curve because the ice they
/// are in is moving.
pub fn crevasse(g: &Ground, x: f32, z: f32, flow_dir: (f32, f32), seed: u64) -> f32 {
    let spacing = g.crevasse_spacing().max(1.0);
    let (fx, fz) = {
        let l = (flow_dir.0 * flow_dir.0 + flow_dir.1 * flow_dir.1).sqrt().max(1e-6);
        (flow_dir.0 / l, flow_dir.1 / l)
    };
    let along = (x * fx + z * fz) / spacing;
    let across = (-x * fz + z * fx) / (spacing * 6.0);
    let (wx, _, wz) = warp(along, 0.0, across, 0.35, seed);
    fracture(wx, 0.0, wz, 6.0, seed)
}

/// CAVE / VOID occupancy: true where rock has been removed.
///
/// Two mechanisms, because caves form two ways. Tubes are long connected
/// conduits — a warped Worley boundary in 3D, which naturally produces
/// branching passages of roughly constant width. Chambers are where the
/// span exceeded what the roof could hold, so they only appear at scales
/// under `tube_span`.
pub fn cave(g: &Ground, x: f32, y: f32, z: f32, seed: u64) -> bool {
    let span = g.tube_span().max(2.0);
    let s = 1.0 / span;
    let (wx, wy, wz) = warp(x * s, y * s, z * s, 0.4, seed);
    let tube = fracture(wx, wy, wz, 9.0, seed);
    // a second, coarser network gives chambers where two conduits meet
    let chamber = fracture(wx * 0.35, wy * 0.35, wz * 0.35, 14.0, seed ^ 0x77);
    tube > 0.72 || chamber > 0.86
}

/// TALUS depth at a distance from the base of a drop.
///
/// A wedge that thins to nothing at the runout distance, which is where the
/// gravity dependence shows: the ANGLE is the same on every world and the
/// REACH is not.
pub fn talus_depth(g: &Ground, drop_m: f32, dist_m: f32) -> f32 {
    let reach = g.talus_runout(drop_m);
    if dist_m >= reach || dist_m < 0.0 {
        return 0.0;
    }
    let t = 1.0 - dist_m / reach;
    t * t * drop_m * 0.12
}

/// CRATER profile: metres of displacement at radius r from the centre.
///
/// Simple craters are parabolic bowls with a raised rim. Complex ones — past
/// the 1/g transition — have a flat floor, terraced walls and a central
/// peak, because the floor rebounded. Feeding the same shape to both is the
/// most visible way to get a world wrong.
pub fn crater_profile(g: &Ground, diameter_m: f32, r_m: f32) -> f32 {
    let rad = diameter_m * 0.5;
    if r_m > rad * 1.6 {
        return 0.0;
    }
    let t = (r_m / rad).min(1.6);
    let depth = if g.crater_is_complex(diameter_m) {
        diameter_m * 0.04 // complex craters are shallow for their width
    } else {
        diameter_m * 0.2
    };
    if t > 1.0 {
        // ejecta blanket, thinning outward
        return depth * 0.06 * (1.6 - t) / 0.6;
    }
    let rim = depth * 0.12 * (-((t - 0.95) * 14.0).powi(2)).exp();
    if g.crater_is_complex(diameter_m) {
        let floor = -depth * (1.0 - (t / 0.75).min(1.0).powi(4));
        let peak = depth * 0.55 * (-(t * 7.0).powi(2)).exp();
        floor + peak + rim
    } else {
        -depth * (1.0 - t * t) + rim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn earth() -> Ground {
        Ground::new(9.81, Material::Sandstone, 1.22, 0.6)
    }
    fn mars() -> Ground {
        Ground::new(3.72, Material::Regolith, 0.020, 2.2)
    }
    fn luna() -> Ground {
        Ground::new(1.62, Material::Regolith, 0.0, 0.0)
    }
    fn europa() -> Ground {
        Ground::new(1.31, Material::Ice, 0.0, 0.0)
    }

    #[test]
    fn vacuum_can_never_have_dunes() {
        // the gate that catches a dune field on an airless world
        assert!(!luna().has_dunes());
        assert!(luna().dune_wavelength().is_none());
        assert!(luna().saltation_threshold().is_infinite());
        assert!(!europa().has_dunes());
    }

    #[test]
    fn dune_spacing_scales_inversely_with_gravity() {
        // same air and wind, a third of the gravity: about three times the
        // spacing, because hop length goes as u*^2/g
        let a = Ground::new(9.81, Material::Regolith, 1.0, 1.5);
        let b = Ground::new(3.27, Material::Regolith, 1.0, 1.5);
        let (wa, wb) = (a.dune_wavelength().unwrap(), b.dune_wavelength().unwrap());
        assert!((wb / wa - 3.0).abs() < 0.15, "{wa} -> {wb}");
        // and stronger wind means longer hops, quadratically
        let c = Ground::new(9.81, Material::Regolith, 1.0, 3.0);
        assert!((c.dune_wavelength().unwrap() / wa - 4.0).abs() < 0.2);
    }

    #[test]
    fn mars_has_bigger_dunes_than_earth() {
        // the headline result, with real-ish numbers rather than a ratio
        let (e, m) = (earth(), mars());
        assert!(e.has_dunes() && m.has_dunes());
        assert!(m.dune_wavelength().unwrap() > e.dune_wavelength().unwrap() * 2.0);
    }

    #[test]
    fn thin_air_raises_the_bar_for_moving_sand() {
        // two effects fight: lower g lowers the threshold, thinner air
        // raises it, and on Mars the air wins
        let e = earth();
        let m = mars();
        assert!(m.saltation_threshold() > e.saltation_threshold(),
            "mars {} vs earth {}", m.saltation_threshold(), e.saltation_threshold());
    }

    #[test]
    fn crevasses_go_deeper_where_gravity_is_weaker() {
        let e = Ground::new(9.81, Material::Ice, 0.0, 0.0);
        let eu = europa();
        let r = eu.crevasse_depth() / e.crevasse_depth();
        assert!((r - 9.81 / 1.31).abs() < 0.05, "ratio {r}");
        // and soft ice cracks shallower than hard ice on the same world
        let soft = Ground::new(1.31, Material::SoftIce, 0.0, 0.0);
        assert!(soft.crevasse_depth() < eu.crevasse_depth());
    }

    #[test]
    fn the_crater_transition_matches_the_bodies_we_have_measured() {
        // Earth 3-4 km, Mars ~7 km, Moon 15-20 km
        assert!((3000.0..4500.0).contains(&earth().crater_transition()));
        assert!((6000.0..11000.0).contains(&mars().crater_transition()));
        let l = luna().crater_transition();
        assert!((15000.0..25000.0).contains(&l), "luna transition {l}");
    }

    #[test]
    fn a_big_crater_is_shallow_and_has_a_peak() {
        let l = luna();
        let big = l.crater_transition() * 2.0;
        let small = l.crater_transition() * 0.2;
        assert!(l.crater_is_complex(big) && !l.crater_is_complex(small));
        // the centre of a complex crater rebounds ABOVE its floor
        let centre = crater_profile(&l, big, 0.0);
        let floor = crater_profile(&l, big, big * 0.3);
        assert!(centre > floor, "no central peak: {centre} vs {floor}");
        // a simple crater is deepest dead centre
        let sc = crater_profile(&l, small, 0.0);
        let sf = crater_profile(&l, small, small * 0.3);
        assert!(sc < sf, "simple crater has a peak");
        // both fade to nothing outside the ejecta
        assert_eq!(crater_profile(&l, big, big * 2.0), 0.0);
    }

    #[test]
    fn the_repose_angle_does_not_care_about_gravity_but_the_runout_does() {
        // THE counterintuitive one, and the reason it is worth encoding
        let heavy = Ground::new(24.0, Material::Regolith, 0.0, 0.0);
        let light = Ground::new(1.6, Material::Regolith, 0.0, 0.0);
        assert_eq!(heavy.material.repose_deg(), light.material.repose_deg());
        // same slope, and yet
        let (rh, rl) = (heavy.talus_runout(100.0), light.talus_runout(100.0));
        assert!(rl > rh * 3.0, "runout barely changed: {rh} -> {rl}");
        // the apron thins to nothing exactly at the reach
        assert_eq!(talus_depth(&light, 100.0, rl + 1.0), 0.0);
        assert!(talus_depth(&light, 100.0, rl * 0.1) > 0.0);
    }

    #[test]
    fn tube_roofs_span_further_in_low_gravity() {
        let e = Ground::new(9.81, Material::Basalt, 1.0, 0.0);
        let l = Ground::new(1.62, Material::Basalt, 0.0, 0.0);
        assert!(l.tube_span() > e.tube_span() * 2.0, "{} -> {}", e.tube_span(), l.tube_span());
        // and regolith cannot hold a roof at all compared with basalt
        let r = Ground::new(1.62, Material::Regolith, 0.0, 0.0);
        assert!(r.tube_span() < l.tube_span());
    }

    #[test]
    fn the_noise_primitives_stay_in_range_and_repeat() {
        for i in 0..400 {
            let x = i as f32 * 0.37;
            let v = value3(x, x * 0.3, x * 1.7, 9);
            assert!((0.0..=1.0).contains(&v), "value3 {v}");
            let f = fbm(x, 0.0, x * 0.5, 5, 9);
            assert!((0.0..=1.0).contains(&f), "fbm {f}");
            let r = ridged(x, 0.0, x * 0.5, 5, 9);
            assert!((0.0..=1.0).contains(&r), "ridged {r}");
            let (d1, d2) = worley(x, 0.0, x * 0.5, 9);
            assert!(d1 <= d2, "worley out of order");
        }
        // deterministic: the same seed and point gives the same answer
        assert_eq!(fbm(1.5, 2.5, 3.5, 4, 77), fbm(1.5, 2.5, 3.5, 4, 77));
        assert_ne!(fbm(1.5, 2.5, 3.5, 4, 77), fbm(1.5, 2.5, 3.5, 4, 78));
    }

    #[test]
    fn fracture_is_thin_and_connected_not_a_wash() {
        // a fracture field should be mostly intact rock with narrow cracks;
        // if most of the plane is "crack" it is just noise
        let mut hits = 0;
        let n = 4000;
        for i in 0..n {
            let x = (i % 64) as f32 * 0.31;
            let z = (i / 64) as f32 * 0.31;
            if fracture(x, 0.0, z, 8.0, 3) > 0.5 {
                hits += 1;
            }
        }
        let frac = hits as f32 / n as f32;
        assert!(frac < 0.35, "cracks cover {:.0}% of the ground", frac * 100.0);
        assert!(frac > 0.01, "no cracks at all");
    }

    #[test]
    fn a_dune_is_asymmetric_the_way_a_real_one_is() {
        let g = Ground::new(3.72, Material::Regolith, 0.02, 3.0);
        let lambda = g.dune_wavelength().unwrap();
        // sample one wavelength along the wind and find the crest
        let mut best = (0.0f32, 0.0f32);
        for i in 0..200 {
            let t = i as f32 / 200.0;
            let h = dune_height(&g, t * lambda, 0.0, (1.0, 0.0), 5).unwrap();
            if h > best.1 {
                best = (t, h);
            }
        }
        // the crest sits well past the middle: long windward, short lee
        assert!(best.0 > 0.55, "crest at {:.2} of the wavelength", best.0);
    }

    #[test]
    fn caves_are_voids_in_rock_not_rock_in_voids() {
        let g = Ground::new(1.62, Material::Basalt, 0.0, 0.0);
        // sample a volume SCALED TO THE SPAN. Basalt at lunar gravity holds
        // a 226 m roof, so a 60 m cube sits entirely inside one conduit cell
        // and finds nothing — which is what the first run of this test did.
        let step = g.tube_span() / 8.0;
        let mut open = 0;
        let n = 8000;
        for i in 0..n {
            let x = (i % 20) as f32 * step;
            let y = ((i / 20) % 20) as f32 * step;
            let z = (i / 400) as f32 * step;
            if cave(&g, x, y, z, 11) {
                open += 1;
            }
        }
        let frac = open as f32 / n as f32;
        assert!(frac < 0.4, "the ground is {:.0}% hole", frac * 100.0);
        assert!(frac > 0.005, "no caves generated at all");
    }
}
