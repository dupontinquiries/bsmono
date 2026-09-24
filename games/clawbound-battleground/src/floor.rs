//! floor — where the ground is, on a map carved out of rock.
//!
//! The jungle map is an open disc with things standing on it; `Blockers`
//! lists the things. A canyon map is the other way round: solid rock with a
//! floor carved out of it, and listing the rock as discs and capsules would
//! take thousands of them. So the floor is described instead -- as the union
//! of capsules (corridors, ledges) and discs (clearings, plazas) -- and
//! everything outside it is cliff.
//!
//! The union's signed distance is exact and cheap: the minimum over shapes
//! of (distance to the shape's axis - its radius). Negative is floor.
//! That one number serves collision (`Blockers` with a floor), the terrain
//! (a cliff rises with distance outside), prop placement (how far in from
//! the edge) and the planner (a cell is a wall if a unit does not fit).
//!
//! Pure geometry; no map knowledge. `pass` builds one.

use crate::blockers::Shape;
use glam::Vec3;

#[derive(Debug, Clone, Default)]
pub struct FloorPlan {
    pub shapes: Vec<Shape>,
    /// Which shapes open into which, and where: built on first use.
    links: std::sync::OnceLock<Vec<Vec<Link>>>,
}

/// Two floor shapes overlap wide enough for a unit to pass: `to`, through
/// the point `portal`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Link {
    pub to: usize,
    pub portal: Vec3,
}

/// A unit must fit through a portal this wide (the overlap's depth).
pub const PORTAL_MIN: f32 = 1.6;

fn axis(s: &Shape) -> (Vec3, Vec3, f32) {
    match *s {
        Shape::Circle { x, z, r } => (Vec3::new(x, 0.0, z), Vec3::new(x, 0.0, z), r),
        Shape::Segment { x0, z0, x1, z1, r } => (Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1), r),
    }
}

/// Closest points between two segments (either may be a point).
fn closest(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> (Vec3, Vec3) {
    let (d1, d2, r) = (a1 - a0, b1 - b0, a0 - b0);
    let (a, e, f) = (d1.dot(d1), d2.dot(d2), d2.dot(r));
    let (s, t);
    if a < 1e-9 && e < 1e-9 {
        return (a0, b0);
    }
    if a < 1e-9 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(r);
        if e < 1e-9 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(d2);
            let den = a * e - b * b;
            let mut ss = if den > 1e-9 { ((b * f - c * e) / den).clamp(0.0, 1.0) } else { 0.0 };
            let mut tt = (b * ss + f) / e;
            if tt < 0.0 {
                tt = 0.0;
                ss = (-c / a).clamp(0.0, 1.0);
            } else if tt > 1.0 {
                tt = 1.0;
                ss = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = ss;
            t = tt;
        }
    }
    (a0 + d1 * s, b0 + d2 * t)
}

impl FloorPlan {
    pub fn new(shapes: Vec<Shape>) -> FloorPlan {
        FloorPlan { shapes: shapes.into_iter().filter(|s| shape_ok(s)).collect(), links: Default::default() }
    }

    /// The shape graph: for each shape, the shapes it opens into.
    pub fn links(&self) -> &[Vec<Link>] {
        self.links.get_or_init(|| {
            let n = self.shapes.len();
            let mut out = vec![Vec::new(); n];
            for i in 0..n {
                let (a0, a1, ra) = axis(&self.shapes[i]);
                for j in (i + 1)..n {
                    let (b0, b1, rb) = axis(&self.shapes[j]);
                    let (pa, pb) = closest(a0, a1, b0, b1);
                    let gap = (pa - pb).length();
                    if gap < ra + rb - PORTAL_MIN {
                        // the portal: between the axes, inside both shapes
                        let dir = (pb - pa).normalize_or_zero();
                        let mid = pa + dir * ((gap + ra - rb) * 0.5).clamp(0.0, gap);
                        out[i].push(Link { to: j, portal: mid });
                        out[j].push(Link { to: i, portal: mid });
                    }
                }
            }
            out
        })
    }

    /// Is the straight walk from `a` to `b` all floor, with `margin` to
    /// spare? Sampled every half unit.
    pub fn clear_line(&self, a: Vec3, b: Vec3, margin: f32) -> bool {
        let d = Vec3::new(b.x - a.x, 0.0, b.z - a.z);
        let n = ((d.length() / 0.5).ceil() as usize).max(1);
        (0..=n).all(|k| {
            let p = a + d * (k as f32 / n as f32);
            self.sdf(p.x, p.z) <= -margin
        })
    }

    /// Where to head now along [`route`](Self::route): the furthest of its
    /// next few points in a clear straight line from `from` (string
    /// pulling). A portal is a point, and fifteen cats aiming at the same
    /// point where three lanes meet a base is a jam -- the first version
    /// did exactly that. `None` when the goal is in plain sight or no
    /// route exists.
    pub fn next_waypoint(&self, from: Vec3, to: Vec3, margin: f32) -> Option<Vec3> {
        let route = self.route(from, to);
        if route.len() <= 1 {
            return None;
        }
        let mut best = None;
        for p in route.iter().take(4) {
            if self.clear_line(from, *p, margin) {
                best = Some(*p);
            } else {
                break;
            }
        }
        match best {
            Some(p) if p == to => None,
            Some(p) => Some(p),
            None => route.into_iter().find(|p| Vec3::new(p.x - from.x, 0.0, p.z - from.z).length() > 1.0),
        }
    }

    /// The shape a point is in (or nearest to).
    pub fn shape_at(&self, x: f32, z: f32) -> Option<usize> {
        self.nearest(x, z).map(|(_, i)| i)
    }

    /// A route over the floor from `from` to `to`: portal points to walk
    /// through, then `to`. Just `[to]` when both are in the same shape.
    /// Dijkstra over shapes, each entered at a portal, cost = straight
    /// distance between successive points -- short routes, not optimal
    /// ones, and cheap: a hundred-odd nodes.
    pub fn route(&self, from: Vec3, to: Vec3) -> Vec<Vec3> {
        let (Some(s), Some(g)) = (self.shape_at(from.x, from.z), self.shape_at(to.x, to.z)) else { return vec![to] };
        if s == g {
            return vec![to];
        }
        let links = self.links();
        let n = self.shapes.len();
        let mut cost = vec![f32::INFINITY; n];
        let mut at = vec![Vec3::ZERO; n];
        let mut prev = vec![usize::MAX; n];
        let mut done = vec![false; n];
        cost[s] = 0.0;
        at[s] = from;
        loop {
            let mut u = usize::MAX;
            let mut best = f32::INFINITY;
            for k in 0..n {
                if !done[k] && cost[k] < best {
                    best = cost[k];
                    u = k;
                }
            }
            if u == usize::MAX || u == g {
                break;
            }
            done[u] = true;
            for l in &links[u] {
                let c = cost[u] + (l.portal - at[u]).length();
                if c < cost[l.to] {
                    cost[l.to] = c;
                    at[l.to] = l.portal;
                    prev[l.to] = u;
                }
            }
        }
        if prev[g] == usize::MAX {
            return vec![to]; // not connected: leave it to local steering
        }
        let mut pts = vec![to];
        let mut k = g;
        while k != s {
            pts.push(at[k]);
            k = prev[k];
        }
        pts.reverse();
        pts
    }

    /// Signed distance to the floor's edge: negative inside the floor.
    /// `+inf` for an empty plan (all rock); a NaN point is treated as rock.
    pub fn sdf(&self, x: f32, z: f32) -> f32 {
        self.nearest(x, z).map_or(f32::INFINITY, |(d, _)| d)
    }

    /// The shape whose surface is nearest, with the signed distance to it.
    fn nearest(&self, x: f32, z: f32) -> Option<(f32, usize)> {
        if !(x.is_finite() && z.is_finite()) {
            return None;
        }
        let mut best: Option<(f32, usize)> = None;
        for (i, s) in self.shapes.iter().enumerate() {
            let (ax, az, r) = s.nearest_axis(x, z);
            let d = (x - ax).hypot(z - az) - r;
            if best.map_or(true, |(b, _)| d < b) {
                best = Some((d, i));
            }
        }
        best
    }

    /// Does a round unit of `radius` fit at `(x, z)`, fully on the floor?
    pub fn fits(&self, x: f32, z: f32, radius: f32) -> bool {
        self.sdf(x, z) <= -radius
    }

    /// The nearest point where a unit of `radius` fits. Pushes along the
    /// nearest shape's own axis direction -- exact for the union, since that
    /// shape's surface IS the union's nearest surface. A shape narrower than
    /// the unit (a ledge path for a big cat) centres the unit on its axis:
    /// on the floor, if a squeeze.
    pub fn push_in(&self, x: f32, z: f32, radius: f32) -> (f32, f32) {
        let Some((d, i)) = self.nearest(x, z) else { return (x, z) };
        if d <= -radius {
            return (x, z);
        }
        let (ax, az, r) = self.shapes[i].nearest_axis(x, z);
        let (dx, dz) = (x - ax, z - az);
        let len = dx.hypot(dz);
        let keep = (r - radius).max(0.0);
        if len < 1e-5 {
            return (ax, az);
        }
        (ax + dx / len * keep, az + dz / len * keep)
    }
}

fn shape_ok(s: &Shape) -> bool {
    match *s {
        Shape::Circle { x, z, r } => x.is_finite() && z.is_finite() && r.is_finite() && r > 0.0,
        Shape::Segment { x0, z0, x1, z1, r } => [x0, z0, x1, z1, r].iter().all(|v| v.is_finite()) && r > 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> FloorPlan {
        FloorPlan::new(vec![
            Shape::Circle { x: 0.0, z: 0.0, r: 10.0 },
            Shape::Segment { x0: 10.0, z0: 0.0, x1: 40.0, z1: 0.0, r: 3.0 },
        ])
    }

    #[test]
    fn inside_is_negative_outside_positive_and_exact() {
        let p = plan();
        assert!((p.sdf(0.0, 0.0) + 10.0).abs() < 1e-5);
        assert!((p.sdf(25.0, 0.0) + 3.0).abs() < 1e-5, "corridor centre");
        assert!((p.sdf(25.0, 5.0) - 2.0).abs() < 1e-5, "2 u into the rock beside the corridor");
        assert!((p.sdf(0.0, 14.0) - 4.0).abs() < 1e-5);
        assert_eq!(FloorPlan::default().sdf(0.0, 0.0), f32::INFINITY, "no floor is all rock");
        assert_eq!(p.sdf(f32::NAN, 0.0), f32::INFINITY);
    }

    #[test]
    fn a_unit_fits_where_it_has_room() {
        let p = plan();
        assert!(p.fits(25.0, 0.0, 0.55));
        assert!(!p.fits(25.0, 2.8, 0.55), "too near the corridor wall");
        assert!(!p.fits(0.0, 30.0, 0.55));
    }

    /// Push-in lands where the unit fits, moves nothing that already fits,
    /// and never throws a unit across a wall into another shape.
    #[test]
    fn push_in_is_the_nearest_fit() {
        let p = plan();
        assert_eq!(p.push_in(3.0, 2.0, 0.55), (3.0, 2.0));
        for (x, z) in [(0.0, 12.0), (25.0, 4.5), (-11.0, -3.0), (44.0, 0.5)] {
            let (nx, nz) = p.push_in(x, z, 0.55);
            assert!(p.fits(nx, nz, 0.55 - 1e-3), "({x},{z}) -> ({nx},{nz}) still in the rock: sdf {}", p.sdf(nx, nz));
            // no further than it had to go
            let moved = (nx - x).hypot(nz - z);
            assert!(moved <= p.sdf(x, z) + 0.55 + 1e-3, "({x},{z}) moved {moved}");
        }
    }

    #[test]
    fn a_ledge_narrower_than_the_unit_centres_it() {
        let p = FloorPlan::new(vec![Shape::Segment { x0: 0.0, z0: 0.0, x1: 10.0, z1: 0.0, r: 0.4 }]);
        let (x, z) = p.push_in(5.0, 1.0, 0.55);
        assert!((x - 5.0).abs() < 1e-5 && z.abs() < 1e-5);
    }

    /// An L of corridors round a block of rock: the route turns the corner
    /// through the portal, it does not aim through the rock.
    #[test]
    fn a_route_turns_the_corner() {
        let p = FloorPlan::new(vec![
            Shape::Segment { x0: 0.0, z0: 0.0, x1: 30.0, z1: 0.0, r: 3.0 },
            Shape::Segment { x0: 30.0, z0: 0.0, x1: 30.0, z1: 30.0, r: 3.0 },
            Shape::Circle { x: 0.0, z: 30.0, r: 4.0 }, // an island, unconnected
        ]);
        let r = p.route(Vec3::new(2.0, 0.0, 0.0), Vec3::new(30.0, 0.0, 28.0));
        assert_eq!(r.len(), 2, "{r:?}");
        assert!((r[0] - Vec3::new(30.0, 0.0, 0.0)).length() < 3.1, "through the corner: {}", r[0]);
        for w in r.windows(2) {
            for k in 0..=20 {
                let q = w[0].lerp(w[1], k as f32 / 20.0);
                assert!(p.sdf(q.x, q.z) < 0.0, "leg crosses rock at {q}");
            }
        }
        assert_eq!(p.route(Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 30.0)), vec![Vec3::new(0.0, 0.0, 30.0)], "unreachable: just the goal");
        assert_eq!(p.route(Vec3::new(2.0, 0.0, 0.0), Vec3::new(20.0, 0.0, 1.0)).len(), 1, "same shape: straight there");
    }

    #[test]
    fn nonsense_shapes_are_dropped() {
        let p = FloorPlan::new(vec![Shape::Circle { x: f32::NAN, z: 0.0, r: 3.0 }, Shape::Circle { x: 0.0, z: 0.0, r: -1.0 }]);
        assert!(p.shapes.is_empty());
    }
}
