//! snowpack -- snow that lies, drifts, and remembers who walked through it.
//!
//! Ultra only, and presentation only: nothing here reaches the sim, so a
//! cat in a drift moves exactly as fast as one on bare stone and lockstep
//! peers on different presets stay in step. What changes is what you see
//! and how the cats look doing it.
//!
//! Two fields:
//!
//! * **The pack** -- snow depth over the whole map on a coarse grid
//!   ([`N`]² cells). It starts laid on the map's own terms (on Starwell:
//!   deep in the sheltered middles of the islands, thin on the old roads,
//!   blown to a skin of powder where the wind scours), builds under
//!   snowfall, and DRIFTS: wind lifts snow off exposed cells (the
//!   powder-blow sections, [`Snowpack::blow`]) and lays it in sheltered
//!   ones downwind. Mass is conserved; only melting removes snow.
//! * **The marks** -- a fine window ([`FN`]² cells of [`FCELL`] u) that
//!   follows the player, holding how much of the snow has been pushed
//!   aside: paw prints, the trench a cat ploughs through a drift, craters
//!   from blasts. Falling snow fills the marks back in; wind smooths them
//!   faster where it blows.
//!
//! Deep and thin read differently on purpose. In deep snow (more than
//! [`DEEP`]) a cat ploughs a continuous trench and bounds (`puppet`), and
//! its prints are deep; on thin, blown powder it leaves separate light
//! prints and kicks up powder that the wind carries off (`vfx`).
//!
//! The renderer samples both fields as textures ([`Snowpack::upload`]): the
//! pack lifts and whitens the terrain, the marks dent it.

use glam::Vec3;

/// Pack grid edge, cells.
pub const N: usize = 256;
/// Marks window edge, cells, and cell size (u): a 51 u window.
pub const FN: usize = 256;
pub const FCELL: f32 = 0.2;
/// Deeper than this is a drift a cat wades through.
pub const DEEP: f32 = 0.45;
/// No cell holds more than this (u).
pub const MAX_DEPTH: f32 = 1.4;
/// Depth encoded in the pack texture: 0..=this maps to 0..=255.
pub const TEX_DEPTH: f32 = 2.0;
/// Snowfall adds this much depth per second at full intensity, on a
/// sheltered cell.
pub const FALL_RATE: f32 = 0.004;
/// Wind moves this share of an exposed cell's snow downwind per second, per
/// unit of wind speed.
pub const DRIFT_RATE: f32 = 0.012;
/// The pack is stepped at this rate (Hz): it moves slowly.
pub const STEP_HZ: f32 = 10.0;
/// Marks fill back in at this rate per second at full snowfall, and this
/// rate per second where the wind blows hardest.
pub const FILL_RATE: f32 = 0.08;
pub const BLOW_FILL_RATE: f32 = 0.05;

/// What kind of hit reached the snow (from an ability's effect).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Throws the snow out of a crater onto its rim.
    Blast,
    /// Melts it.
    Heat,
    /// Lays fresh frost and snow.
    Frost,
}

#[derive(Debug, Clone)]
pub struct Snowpack {
    /// The pack: world origin of cell (0, 0), cell size.
    pub x0: f32,
    pub z0: f32,
    pub cell: f32,
    pub depth: Vec<f32>,
    /// How exposed each cell is to the wind, 0 (sheltered) ..= 1 (scoured).
    pub blow: Vec<f32>,
    /// Can snow lie here at all (floor, not water, not the void).
    pub holds: Vec<bool>,
    /// The marks window: world origin, and pushed-aside share per cell.
    pub fx0: f32,
    pub fz0: f32,
    pub press: Vec<f32>,
    step_acc: f32,
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Can snow lie at (x, z) on the active map?
fn holds_snow(x: f32, z: f32) -> bool {
    match crate::map::active() {
        crate::map::MapId::Sky => crate::sky::floor().sdf(x, z) < crate::sky::RIM_W * 0.5 && !crate::sky::in_pool(x, z),
        crate::map::MapId::Pass => crate::consts::water_depth(x, z) < 0.05,
        crate::map::MapId::Jungle => crate::consts::water_depth(x, z) < 0.05,
        // under the sea: no snow
        crate::map::MapId::Atlantis => false,
        // the maze's climate has no snow in it (weather::climate)
        crate::map::MapId::Maze => false,
    }
}

/// How exposed (x, z) is to the wind on the active map: broad patches of
/// scoured ground from a slow noise, and on the sky map the islands' edges
/// and the open roads.
fn exposure(x: f32, z: f32) -> f32 {
    let n = crate::pass::vnoise(x * 0.018 + 3.7, z * 0.018 - 1.9);
    let patches = smooth(0.52, 0.68, n);
    match crate::map::active() {
        crate::map::MapId::Sky => {
            let edge = smooth(-14.0, -1.0, crate::sky::floor().sdf(x, z));
            let road = 1.0 - smooth(crate::sky::LANE_R - 5.0, crate::sky::LANE_R, crate::render::lane_road(x, z));
            patches.max(edge * 0.85).max(road * 0.55)
        }
        _ => patches,
    }
}

/// The snow a map starts with, u: none in the jungle; on the pass and the
/// sky, deep where sheltered, a skin of powder where blown.
fn initial_depth(x: f32, z: f32, blow: f32) -> f32 {
    let base = match crate::map::active() {
        crate::map::MapId::Sky => 1.0,
        crate::map::MapId::Pass => 0.7,
        crate::map::MapId::Jungle | crate::map::MapId::Atlantis | crate::map::MapId::Maze => 0.0,
    };
    if base == 0.0 {
        return 0.0;
    }
    let lumps = 0.85 + 0.3 * crate::pass::vnoise(x * 0.09, z * 0.09);
    let mut d = base * (1.0 - blow).powf(1.6) * lumps + 0.05;
    if crate::map::active() == crate::map::MapId::Sky && crate::render::lane_road(x, z) < crate::sky::LANE_R - 2.0 {
        // the old roads: trodden and swept
        d *= 0.3;
    }
    d.min(MAX_DEPTH)
}

/// How hard it is snowing for the pack and the flakes: the weather's own
/// snow, and on Ultra a light fall that never stops over the snowy maps.
pub fn snowfall(weather_snow: f32) -> f32 {
    if crate::quality::Quality::effective() != crate::quality::Quality::Ultra {
        return weather_snow;
    }
    let ambient = match crate::map::active() {
        crate::map::MapId::Sky => 0.3,
        crate::map::MapId::Pass => 0.25,
        crate::map::MapId::Jungle | crate::map::MapId::Atlantis | crate::map::MapId::Maze => 0.0,
    };
    weather_snow.max(ambient)
}

impl Snowpack {
    /// The active map's snowpack, laid, if this preset has one (Ultra).
    pub fn for_map() -> Option<Snowpack> {
        (crate::quality::Quality::effective() == crate::quality::Quality::Ultra).then(Snowpack::new)
    }

    /// The active map's snowpack, laid, whatever the preset (tests, shots).
    pub fn new() -> Snowpack {
        let half = crate::consts::ground_half();
        let cell = half * 2.0 / N as f32;
        let (x0, z0) = (-half, -half);
        let mut depth = vec![0.0; N * N];
        let mut blow = vec![0.0; N * N];
        let mut holds = vec![false; N * N];
        for j in 0..N {
            for i in 0..N {
                let (x, z) = (x0 + (i as f32 + 0.5) * cell, z0 + (j as f32 + 0.5) * cell);
                let k = i + j * N;
                holds[k] = holds_snow(x, z);
                if holds[k] {
                    blow[k] = exposure(x, z);
                    depth[k] = initial_depth(x, z, blow[k]);
                }
            }
        }
        Snowpack { x0, z0, cell, depth, blow, holds, fx0: -(FN as f32) * FCELL * 0.5, fz0: -(FN as f32) * FCELL * 0.5, press: vec![0.0; FN * FN], step_acc: 0.0 }
    }

    fn bilinear(&self, v: &[f32], x: f32, z: f32) -> f32 {
        let fx = ((x - self.x0) / self.cell - 0.5).clamp(0.0, (N - 1) as f32 - 1e-3);
        let fz = ((z - self.z0) / self.cell - 0.5).clamp(0.0, (N - 1) as f32 - 1e-3);
        let (i, j) = (fx as usize, fz as usize);
        let (u, w) = (fx - i as f32, fz - j as f32);
        let a = v[i + j * N] * (1.0 - u) + v[i + 1 + j * N] * u;
        let b = v[i + (j + 1) * N] * (1.0 - u) + v[i + 1 + (j + 1) * N] * u;
        a * (1.0 - w) + b * w
    }

    /// Snow lying at (x, z), before any marks, u.
    pub fn depth_at(&self, x: f32, z: f32) -> f32 {
        self.bilinear(&self.depth, x, z)
    }

    /// How blown (x, z) is, 0..=1.
    pub fn blow_at(&self, x: f32, z: f32) -> f32 {
        self.bilinear(&self.blow, x, z)
    }

    /// The share of the snow pushed aside at (x, z), 0..=1 (0 outside the
    /// marks window).
    pub fn press_at(&self, x: f32, z: f32) -> f32 {
        let (i, j) = (((x - self.fx0) / FCELL).floor(), ((z - self.fz0) / FCELL).floor());
        if i < 0.0 || j < 0.0 || i >= FN as f32 || j >= FN as f32 {
            return 0.0;
        }
        self.press[i as usize + j as usize * FN]
    }

    /// Loose snow standing at (x, z) now: the pack less what has been
    /// pushed aside.
    pub fn surface_at(&self, x: f32, z: f32) -> f32 {
        self.depth_at(x, z) * (1.0 - self.press_at(x, z))
    }

    pub fn is_deep(&self, x: f32, z: f32) -> bool {
        self.depth_at(x, z) > DEEP
    }

    /// Keep the marks window round `focus`, moving it in whole steps of 32
    /// cells and carrying the overlap across.
    pub fn follow(&mut self, focus: Vec3) {
        let span = FN as f32 * FCELL;
        let step = 32.0 * FCELL;
        let want_x = ((focus.x - span * 0.5) / step).round() * step;
        let want_z = ((focus.z - span * 0.5) / step).round() * step;
        if (want_x - self.fx0).abs() < 1e-3 && (want_z - self.fz0).abs() < 1e-3 {
            return;
        }
        let (di, dj) = (((want_x - self.fx0) / FCELL).round() as i64, ((want_z - self.fz0) / FCELL).round() as i64);
        let mut next = vec![0.0; FN * FN];
        for j in 0..FN as i64 {
            for i in 0..FN as i64 {
                let (si, sj) = (i + di, j + dj);
                if si >= 0 && sj >= 0 && si < FN as i64 && sj < FN as i64 {
                    next[(i + j * FN as i64) as usize] = self.press[(si + sj * FN as i64) as usize];
                }
            }
        }
        self.press = next;
        self.fx0 = want_x;
        self.fz0 = want_z;
    }

    /// Push snow aside in a disc: `share` of what is there, at most.
    fn press_disc(&mut self, x: f32, z: f32, r: f32, share: f32) {
        let (ci, cj) = ((x - self.fx0) / FCELL, (z - self.fz0) / FCELL);
        let rc = r / FCELL;
        let (i0, i1) = ((ci - rc).floor().max(0.0) as usize, ((ci + rc).ceil() as usize).min(FN - 1));
        let (j0, j1) = ((cj - rc).floor().max(0.0) as usize, ((cj + rc).ceil() as usize).min(FN - 1));
        if ci + rc < 0.0 || cj + rc < 0.0 || ci - rc > FN as f32 || cj - rc > FN as f32 {
            return;
        }
        for j in j0..=j1 {
            for i in i0..=i1 {
                let d = ((i as f32 + 0.5 - ci).powi(2) + (j as f32 + 0.5 - cj).powi(2)).sqrt() / rc.max(1e-3);
                if d < 1.0 {
                    // soft-edged: full in the middle, easing out at the rim
                    let k = share * (1.0 - smooth(0.6, 1.0, d));
                    let p = &mut self.press[i + j * FN];
                    *p = p.max(k);
                }
            }
        }
    }

    /// A paw came down at `pos`. In deep snow it sinks nearly to the ground;
    /// on thin powder it leaves a light print. Returns how deep the print is
    /// (u), so the caller can size the powder it kicks up.
    pub fn paw(&mut self, pos: Vec3) -> f32 {
        let d = self.depth_at(pos.x, pos.z);
        if d < 0.02 {
            return 0.0;
        }
        // in a drift the paw sinks and packs the snow under it; on powder it
        // brushes the skin of snow aside and the ground shows
        let (share, r) = if d > DEEP { (0.85, 0.2) } else { (0.9, 0.22) };
        self.press_disc(pos.x, pos.z, r, share);
        d * share
    }

    /// A body shouldering through deep snow from `a` to `b`: a trench
    /// `width` wide. Nothing on snow thinner than [`DEEP`].
    pub fn plough(&mut self, a: Vec3, b: Vec3, width: f32) {
        if !self.is_deep(b.x, b.z) {
            return;
        }
        let len = Vec3::new(b.x - a.x, 0.0, b.z - a.z).length();
        let n = (len / (FCELL * 1.5)).ceil().clamp(1.0, 60.0) as usize;
        for k in 0..=n {
            let p = a.lerp(b, k as f32 / n as f32);
            self.press_disc(p.x, p.z, width * 0.5, 0.8);
        }
    }

    /// An ability's effect reached the ground at `pos`.
    pub fn hit(&mut self, pos: Vec3, radius: f32, kind: Hit) {
        let r = radius.max(0.5);
        // the pack: cells within the radius
        let (ci, cj) = ((pos.x - self.x0) / self.cell, (pos.z - self.z0) / self.cell);
        let rc = (r / self.cell).max(0.75);
        let reach = (rc * 1.6).ceil() as i64;
        let mut thrown = 0.0;
        let mut rim = Vec::new();
        for dj in -reach..=reach {
            for di in -reach..=reach {
                let (i, j) = (ci.floor() as i64 + di, cj.floor() as i64 + dj);
                if i < 0 || j < 0 || i >= N as i64 || j >= N as i64 {
                    continue;
                }
                let k = i as usize + j as usize * N;
                if !self.holds[k] {
                    continue;
                }
                let d = ((i as f32 + 0.5 - ci).powi(2) + (j as f32 + 0.5 - cj).powi(2)).sqrt() / rc;
                match kind {
                    Hit::Blast if d < 1.0 => {
                        let take = self.depth[k] * 0.8 * (1.0 - d * 0.5);
                        self.depth[k] -= take;
                        thrown += take;
                    }
                    Hit::Blast if d < 1.6 => rim.push(k),
                    Hit::Heat if d < 1.0 => self.depth[k] *= 0.3 + 0.5 * d,
                    Hit::Frost if d < 1.0 => self.depth[k] = (self.depth[k] + 0.25 * (1.0 - d)).min(MAX_DEPTH),
                    _ => {}
                }
            }
        }
        // a blast throws its snow onto the rim: conserved
        if !rim.is_empty() {
            let each = thrown / rim.len() as f32;
            for k in rim {
                self.depth[k] = (self.depth[k] + each).min(MAX_DEPTH);
            }
        }
        // the marks: a clean crater (or a melted pan), none for frost
        match kind {
            Hit::Blast => self.press_disc(pos.x, pos.z, r, 0.95),
            Hit::Heat => self.press_disc(pos.x, pos.z, r, 0.8),
            Hit::Frost => {
                // fresh snow over the old marks
                let (ci, cj) = ((pos.x - self.fx0) / FCELL, (pos.z - self.fz0) / FCELL);
                let rc = r / FCELL;
                for j in 0..FN {
                    for i in 0..FN {
                        if ((i as f32 - ci).powi(2) + (j as f32 - cj).powi(2)).sqrt() < rc {
                            self.press[i + j * FN] *= 0.2;
                        }
                    }
                }
            }
        }
    }

    /// Time passes: snow falls (`snowfall` 0..=1), the wind (`wind`, u/s,
    /// horizontal) drifts the pack from the blown cells into the sheltered
    /// ones downwind, and the marks fill back in.
    pub fn tick(&mut self, dt: f32, snowfall: f32, wind: Vec3) {
        if !(dt > 0.0) {
            return;
        }
        // marks: filled by falling snow, smoothed by the wind where it blows
        let fill = (snowfall * FILL_RATE) * dt;
        let (fx0, fz0) = (self.fx0, self.fz0);
        for j in 0..FN {
            for i in 0..FN {
                let p = &mut self.press[i + j * FN];
                if *p > 0.0 {
                    let (x, z) = (fx0 + (i as f32 + 0.5) * FCELL, fz0 + (j as f32 + 0.5) * FCELL);
                    let blow = {
                        let fx = ((x - self.x0) / self.cell - 0.5).clamp(0.0, (N - 1) as f32);
                        let fz = ((z - self.z0) / self.cell - 0.5).clamp(0.0, (N - 1) as f32);
                        self.blow[fx as usize + fz as usize * N]
                    };
                    let k = fill + blow * BLOW_FILL_RATE * wind.length().min(8.0) / 8.0 * dt;
                    *p = (*p - k).max(0.0);
                }
            }
        }
        // the pack, in fixed steps
        self.step_acc += dt;
        let h = 1.0 / STEP_HZ;
        while self.step_acc >= h {
            self.step_acc -= h;
            self.step_pack(h, snowfall, wind);
        }
    }

    fn step_pack(&mut self, h: f32, snowfall: f32, wind: Vec3) {
        // snowfall: more settles where it is sheltered
        if snowfall > 0.0 {
            for k in 0..N * N {
                if self.holds[k] {
                    self.depth[k] = (self.depth[k] + snowfall * FALL_RATE * h * (1.0 - 0.8 * self.blow[k])).min(MAX_DEPTH);
                }
            }
        }
        // drift: each cell lifts a share of its snow by how exposed it is,
        // and sets it down one cell downwind -- all of it if that cell is
        // sheltered, less if it is exposed too (the rest stays aloft and
        // settles back where it came from, this step)
        let speed = Vec3::new(wind.x, 0.0, wind.z).length();
        if speed < 0.2 {
            return;
        }
        let (dx, dz) = (wind.x / speed, wind.z / speed);
        let (si, sj) = (dx.round() as i64, dz.round() as i64);
        if si == 0 && sj == 0 {
            return;
        }
        let rate = (DRIFT_RATE * speed * h).min(0.5);
        let mut moved = vec![0.0f32; N * N];
        for j in 0..N as i64 {
            for i in 0..N as i64 {
                let k = (i + j * N as i64) as usize;
                if !self.holds[k] || self.depth[k] <= 0.0 {
                    continue;
                }
                // what the wind lifts skips downwind a cell at a time, up to
                // three, landing where the lee is (a sheltered cell keeps
                // nearly all that reaches it, a scoured one almost none);
                // whatever is still aloft after that, or would go over an
                // edge into the sky, settles back where it started
                let mut aloft = self.depth[k] * self.blow[k] * rate;
                let lifted = aloft;
                for hop in 1..=3i64 {
                    let (ti, tj) = (i + si * hop, j + sj * hop);
                    if ti < 0 || tj < 0 || ti >= N as i64 || tj >= N as i64 {
                        break;
                    }
                    let t = (ti + tj * N as i64) as usize;
                    if !self.holds[t] {
                        break;
                    }
                    let lands = aloft * (1.0 - self.blow[t]).powi(2);
                    moved[t] += lands;
                    aloft -= lands;
                }
                moved[k] -= lifted - aloft;
            }
        }
        for k in 0..N * N {
            self.depth[k] = (self.depth[k] + moved[k]).clamp(0.0, MAX_DEPTH);
        }
    }

    /// The fields as texture bytes: the pack's depth (0..=`TEX_DEPTH` u to
    /// 0..=255), and the marks' press (0..=1 to 0..=255).
    pub fn upload(&self) -> (Vec<u8>, Vec<u8>) {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        (self.depth.iter().map(|d| q(d / TEX_DEPTH)).collect(), self.press.iter().map(|p| q(*p)).collect())
    }

    /// Where the fields sit in the world, for the shader: (pack origin x, z,
    /// pack span, on), (marks origin x, z, marks span, `TEX_DEPTH`).
    pub fn params(&self) -> [[f32; 4]; 2] {
        [[self.x0, self.z0, self.cell * N as f32, 1.0], [self.fx0, self.fz0, FCELL * FN as f32, TEX_DEPTH]]
    }

    /// Total snow on the map (u * cells), for conservation checks.
    pub fn total(&self) -> f32 {
        self.depth.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sky() -> Snowpack {
        crate::map::with(crate::map::MapId::Sky, Snowpack::new)
    }

    #[test]
    fn starwell_starts_deep_in_the_shelter_and_thin_where_it_blows() {
        let s = sky();
        let held: Vec<usize> = (0..N * N).filter(|&k| s.holds[k]).collect();
        assert!(held.len() > N * N / 20, "snow lies on the islands: {}", held.len());
        let deep = held.iter().filter(|&&k| s.depth[k] > DEEP).count();
        let thin = held.iter().filter(|&&k| s.depth[k] < 0.2).count();
        assert!(deep * 10 > held.len() && thin * 10 > held.len(), "both drifts and blown powder: deep {deep}, thin {thin} of {}", held.len());
        // exposed cells are the thin ones
        let mean = |f: &dyn Fn(usize) -> bool| {
            let v: Vec<f32> = held.iter().copied().filter(|&k| f(k)).map(|k| s.depth[k]).collect();
            v.iter().sum::<f32>() / v.len().max(1) as f32
        };
        assert!(mean(&|k| s.blow[k] > 0.7) < mean(&|k| s.blow[k] < 0.2) * 0.4);
        // none in the void or the pool; some on the Aerie
        assert!(s.depth_at(0.0, -120.0) < 1e-3 && s.depth_at(0.0, 0.0) < 0.05);
        assert!(s.depth_at(0.0, -100.0) > 0.05, "snow on the Aerie");
    }

    #[test]
    fn wind_moves_snow_from_blown_to_sheltered_and_keeps_it() {
        let mut s = sky();
        let before = s.total();
        let held: Vec<usize> = (0..N * N).filter(|&k| s.holds[k]).collect();
        let exposed = |s: &Snowpack| held.iter().filter(|&&k| s.blow[k] > 0.7).map(|&k| s.depth[k]).sum::<f32>();
        let sheltered = |s: &Snowpack| held.iter().filter(|&&k| s.blow[k] < 0.1).map(|&k| s.depth[k]).sum::<f32>();
        let (e0, s0) = (exposed(&s), sheltered(&s));
        for _ in 0..(60 * 30) {
            s.tick(1.0 / 60.0, 0.0, Vec3::new(6.0, 0.0, 1.0));
        }
        assert!((s.total() - before).abs() < before * 1e-3, "no snow made or lost by wind: {before} -> {}", s.total());
        // thirty seconds of a stiff breeze: a few percent off the scoured
        // ground, and it turns up in the lee
        assert!(exposed(&s) < e0 * 0.97, "the blown ground lost snow: {e0} -> {}", exposed(&s));
        assert!(sheltered(&s) > s0 + (e0 - exposed(&s)) * 0.3, "and the sheltered ground got it: {s0} -> {}", sheltered(&s));
    }

    #[test]
    fn snowfall_builds_the_pack_most_where_it_is_sheltered() {
        let mut s = sky();
        let d0 = s.depth.clone();
        for _ in 0..(60 * 20) {
            s.tick(1.0 / 60.0, 1.0, Vec3::ZERO);
        }
        let k_shel = (0..N * N).find(|&k| s.holds[k] && s.blow[k] < 0.05 && d0[k] < 1.0).unwrap();
        let k_blow = (0..N * N).find(|&k| s.holds[k] && s.blow[k] > 0.95).unwrap();
        let (g_shel, g_blow) = (s.depth[k_shel] - d0[k_shel], s.depth[k_blow] - d0[k_blow]);
        assert!(g_shel > 0.05 && g_blow < g_shel * 0.4, "sheltered +{g_shel}, blown +{g_blow}");
    }

    #[test]
    fn deep_snow_is_ploughed_thin_snow_only_printed_and_both_fill_back() {
        let mut s = sky();
        // find a deep spot and a thin one on the floor
        let spot = |s: &Snowpack, deep: bool| {
            (0..N * N)
                .filter(|&k| s.holds[k] && if deep { s.depth[k] > 0.8 } else { (0.05..0.2).contains(&s.depth[k]) })
                .map(|k| Vec3::new(s.x0 + ((k % N) as f32 + 0.5) * s.cell, 0.0, s.z0 + ((k / N) as f32 + 0.5) * s.cell))
                .next()
                .unwrap()
        };
        let (deep, thin) = (spot(&s, true), spot(&s, false));
        s.follow(deep);
        let d = s.depth_at(deep.x, deep.z);
        s.plough(deep, deep + Vec3::X * 2.0, 0.9);
        assert!(s.surface_at(deep.x + 1.0, deep.z) < d * 0.4, "a trench through the drift");
        let print = s.paw(deep + Vec3::Z * 3.0);
        assert!(print > DEEP * 0.8, "a deep print: {print}");
        s.follow(thin);
        s.plough(thin, thin + Vec3::X * 2.0, 0.9);
        assert!(s.press_at(thin.x + 1.0, thin.z) < 1e-3, "no trench in thin powder");
        let light = s.paw(thin);
        assert!(light > 0.0 && light < 0.15, "a light print: {light}");
        // fresh snow fills the print
        let p0 = s.press_at(thin.x, thin.z);
        for _ in 0..(60 * 5) {
            s.tick(1.0 / 60.0, 1.0, Vec3::ZERO);
        }
        assert!(s.press_at(thin.x, thin.z) < p0 * 0.7, "filled back in");
    }

    #[test]
    fn a_blast_throws_a_crater_heat_melts_frost_lays() {
        let mut s = sky();
        let at = (0..N * N)
            .filter(|&k| s.holds[k] && s.depth[k] > 0.8)
            .map(|k| Vec3::new(s.x0 + ((k % N) as f32 + 0.5) * s.cell, 0.0, s.z0 + ((k / N) as f32 + 0.5) * s.cell))
            .find(|p| (0..9).all(|q| s.holds_near(*p, q)))
            .unwrap();
        s.follow(at);
        let (before, d) = (s.total(), s.depth_at(at.x, at.z));
        s.hit(at, 5.0, Hit::Blast);
        assert!(s.depth_at(at.x, at.z) < d * 0.5, "a crater");
        assert!((s.total() - before).abs() < before * 1e-4, "the snow went to the rim, not away");
        assert!(s.press_at(at.x, at.z) > 0.9);
        let d = s.depth_at(at.x + 30.0, at.z);
        let t = s.total();
        s.hit(at + Vec3::X * 30.0, 4.0, Hit::Heat);
        assert!(s.total() < t && s.depth_at(at.x + 30.0, at.z) <= d);
        let t = s.total();
        s.hit(at, 4.0, Hit::Frost);
        assert!(s.total() > t);
    }

    impl Snowpack {
        fn holds_near(&self, p: Vec3, q: usize) -> bool {
            let o = [(0.0, 0.0), (6.0, 0.0), (-6.0, 0.0), (0.0, 6.0), (0.0, -6.0), (6.0, 6.0), (-6.0, -6.0), (6.0, -6.0), (-6.0, 6.0)][q];
            let (i, j) = (((p.x + o.0 - self.x0) / self.cell) as usize, ((p.z + o.1 - self.z0) / self.cell) as usize);
            i < N && j < N && self.holds[i + j * N]
        }
    }

    #[test]
    fn the_marks_window_follows_and_keeps_what_it_can() {
        let mut s = sky();
        let p = Vec3::new(10.0, 0.0, -150.0);
        s.follow(p);
        s.press_disc(p.x, p.z, 0.5, 1.0);
        assert!(s.press_at(p.x, p.z) > 0.9);
        s.follow(p + Vec3::X * 12.0);
        assert!(s.press_at(p.x, p.z) > 0.9, "still there after the window moved");
        s.follow(p + Vec3::X * 200.0);
        assert_eq!(s.press_at(p.x, p.z), 0.0, "out of the window");
    }

    #[test]
    fn only_ultra_has_a_pack_and_a_ceaseless_fall() {
        crate::map::with(crate::map::MapId::Sky, || {
            let q = crate::quality::Quality::effective();
            let has = Snowpack::for_map().is_some();
            assert_eq!(has, q == crate::quality::Quality::Ultra);
            if q != crate::quality::Quality::Ultra {
                assert_eq!(snowfall(0.0), 0.0);
            }
        });
        assert_eq!(crate::map::with(crate::map::MapId::Jungle, Snowpack::new).total(), 0.0, "the jungle starts bare");
    }
}
