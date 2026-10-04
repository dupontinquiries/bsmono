//! 8-bit grey frames and the few image operations the detector needs, with no dependencies.

#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub d: Vec<u8>,
}

impl Gray {
    pub fn new(w: usize, h: usize, fill: u8) -> Gray { Gray { w, h, d: vec![fill; w * h] } }

    pub fn from_fn(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Gray {
        let mut d = Vec::with_capacity(w * h);
        for y in 0..h { for x in 0..w { d.push(f(x, y)); } }
        Gray { w, h, d }
    }

    #[inline]
    pub fn at(&self, x: usize, y: usize) -> u8 { self.d[y * self.w + x] }

    /// A binary PGM (P5, maxval 255), what `ffmpeg -f image2pipe -vcodec pgm` writes.
    pub fn from_pgm(b: &[u8]) -> Option<Gray> {
        fn token(b: &[u8], pos: &mut usize) -> Option<usize> {
            loop {
                while *pos < b.len() && b[*pos].is_ascii_whitespace() { *pos += 1; }
                if *pos < b.len() && b[*pos] == b'#' {
                    while *pos < b.len() && b[*pos] != b'\n' { *pos += 1; }
                } else { break; }
            }
            let s = *pos;
            while *pos < b.len() && !b[*pos].is_ascii_whitespace() { *pos += 1; }
            std::str::from_utf8(&b[s..*pos]).ok()?.parse().ok()
        }
        if b.get(..2)? != b"P5" { return None; }
        let mut pos = 2;
        let (w, h, max) = (token(b, &mut pos)?, token(b, &mut pos)?, token(b, &mut pos)?);
        if max != 255 || w == 0 || h == 0 { return None; }
        pos += 1; // the single whitespace after maxval
        let d = b.get(pos..pos.checked_add(w.checked_mul(h)?)?)?.to_vec();
        Some(Gray { w, h, d })
    }

    pub fn to_pgm(&self) -> Vec<u8> {
        let mut out = format!("P5\n{} {}\n255\n", self.w, self.h).into_bytes();
        out.extend_from_slice(&self.d);
        out
    }

    /// Crop the centre to `tw:th`, then resize to exactly `tw` x `th` (bilinear): `crop_to_aspect_ratio` of the original.
    pub fn crop_resize(&self, tw: usize, th: usize) -> Gray {
        let (cw, ch) = (self.w as f64, self.h as f64);
        let want = tw as f64 / th as f64;
        let (sw, sh, ox, oy) = if cw / ch > want {
            let nw = (want * ch) as usize;
            (nw, self.h, (self.w - nw) / 2, 0)
        } else {
            let nh = (cw / want) as usize;
            (self.w, nh, 0, (self.h - nh) / 2)
        };
        Gray::from_fn(tw, th, |x, y| {
            // pixel centres, as OpenCV's INTER_LINEAR
            let fx = ((x as f64 + 0.5) * sw as f64 / tw as f64 - 0.5).clamp(0.0, sw as f64 - 1.0);
            let fy = ((y as f64 + 0.5) * sh as f64 / th as f64 - 0.5).clamp(0.0, sh as f64 - 1.0);
            let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
            let (ax, ay) = (fx - x0 as f64, fy - y0 as f64);
            let p = |xx: usize, yy: usize| self.at(ox + xx, oy + yy) as f64;
            let v = p(x0, y0) * (1.0 - ax) * (1.0 - ay) + p(x1, y0) * ax * (1.0 - ay) + p(x0, y1) * (1.0 - ax) * ay + p(x1, y1) * ax * ay;
            (v + 0.5) as u8
        })
    }
}

/// A binary image: true is foreground.
#[derive(Debug, Clone, PartialEq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub d: Vec<bool>,
}

impl Mask {
    /// `THRESH_BINARY_INV`: foreground where the pixel is at or below `t`.
    pub fn dark(g: &Gray, t: i32) -> Mask { Mask { w: g.w, h: g.h, d: g.d.iter().map(|&p| (p as i32) <= t).collect() } }

    /// Keep only the square of side `size` centred on `c`.
    pub fn keep_square(&mut self, c: (usize, usize), size: usize) {
        let half = size / 2;
        let (x0, y0) = (c.0.saturating_sub(half), c.1.saturating_sub(half));
        let (x1, y1) = ((c.0 + half).min(self.w), (c.1 + half).min(self.h));
        for y in 0..self.h { for x in 0..self.w {
            if !(x >= x0 && x < x1 && y >= y0 && y < y1) { self.d[y * self.w + x] = false; }
        } }
    }

    /// Dilate by a square of half-width `r` (a 5x5 kernel twice is `r = 4`). Separable.
    pub fn dilate_square(&self, r: usize) -> Mask {
        let mut tmp = vec![false; self.w * self.h];
        for y in 0..self.h {
            // running count of foreground in the window along the row
            let row = &self.d[y * self.w..(y + 1) * self.w];
            let mut count = 0usize;
            for x in 0..=r.min(self.w - 1) { if row[x] { count += 1; } }
            for x in 0..self.w {
                tmp[y * self.w + x] = count > 0;
                if x + r + 1 < self.w && row[x + r + 1] { count += 1; }
                if x >= r && row[x - r] { count -= 1; }
            }
        }
        let mut out = vec![false; self.w * self.h];
        for x in 0..self.w {
            let mut count = 0usize;
            for y in 0..=r.min(self.h - 1) { if tmp[y * self.w + x] { count += 1; } }
            for y in 0..self.h {
                out[y * self.w + x] = count > 0;
                if y + r + 1 < self.h && tmp[(y + r + 1) * self.w + x] { count += 1; }
                if y >= r && tmp[(y - r) * self.w + x] { count -= 1; }
            }
        }
        Mask { w: self.w, h: self.h, d: out }
    }

    /// The 8-connected foreground blobs, each with holes filled (what `findContours(RETR_EXTERNAL)` + `contourArea`
    /// measures: the area inside the outer boundary).
    pub fn blobs(&self) -> Vec<Blob> {
        let mut seen = vec![false; self.w * self.h];
        let mut out = Vec::new();
        for sy in 0..self.h { for sx in 0..self.w {
            let i = sy * self.w + sx;
            if !self.d[i] || seen[i] { continue; }
            let mut stack = vec![(sx, sy)];
            seen[i] = true;
            let mut px = Vec::new();
            let (mut x0, mut y0, mut x1, mut y1) = (sx, sy, sx, sy);
            while let Some((x, y)) = stack.pop() {
                px.push((x, y));
                x0 = x0.min(x); x1 = x1.max(x); y0 = y0.min(y); y1 = y1.max(y);
                for dy in -1i32..=1 { for dx in -1i32..=1 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= self.w as i32 || ny >= self.h as i32 { continue; }
                    let j = ny as usize * self.w + nx as usize;
                    if self.d[j] && !seen[j] { seen[j] = true; stack.push((nx as usize, ny as usize)); }
                } }
            }
            out.push(Blob::filled(px, (x0, y0, x1, y1)));
        } }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Blob {
    /// Bounding box, inclusive: (x0, y0, x1, y1).
    pub bbox: (usize, usize, usize, usize),
    /// Pixels inside the outer boundary (holes filled).
    pub area: usize,
    pub cx: f64,
    pub cy: f64,
    /// Central second moments of the filled blob.
    pub mxx: f64,
    pub myy: f64,
    pub mxy: f64,
    /// The outer outline in order, with the points inside straight runs dropped: what `findContours(RETR_EXTERNAL,
    /// CHAIN_APPROX_SIMPLE)` hands `fitEllipse`.
    pub contour: Vec<(f64, f64)>,
}

impl Blob {
    fn filled(px: Vec<(usize, usize)>, bbox: (usize, usize, usize, usize)) -> Blob {
        let (w, h) = (bbox.2 - bbox.0 + 1, bbox.3 - bbox.1 + 1);
        // flood the background in from a one-pixel margin; whatever it cannot reach is inside the blob
        let (pw, ph) = (w + 2, h + 2);
        let mut solid = vec![false; pw * ph];
        for (x, y) in &px { solid[(y - bbox.1 + 1) * pw + (x - bbox.0 + 1)] = true; }
        let mut outside = vec![false; pw * ph];
        let mut stack = vec![(0usize, 0usize)];
        outside[0] = true;
        while let Some((x, y)) = stack.pop() {
            let mut push = |nx: usize, ny: usize| {
                let j = ny * pw + nx;
                if !solid[j] && !outside[j] { outside[j] = true; stack.push((nx, ny)); }
            };
            if x > 0 { push(x - 1, y); }
            if y > 0 { push(x, y - 1); }
            if x + 1 < pw { push(x + 1, y); }
            if y + 1 < ph { push(x, y + 1); }
        }
        let contour = trace_outline(&solid, pw, ph).into_iter().map(|(x, y)| ((x + bbox.0 - 1) as f64, (y + bbox.1 - 1) as f64)).collect();
        let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
        for y in 0..h { for x in 0..w {
            if !outside[(y + 1) * pw + (x + 1)] {
                let (fx, fy) = ((bbox.0 + x) as f64, (bbox.1 + y) as f64);
                n += 1.0; sx += fx; sy += fy; sxx += fx * fx; syy += fy * fy; sxy += fx * fy;
            }
        } }
        let (cx, cy) = (sx / n, sy / n);
        Blob { bbox, area: n as usize, cx, cy, mxx: sxx / n - cx * cx, myy: syy / n - cy * cy, mxy: sxy / n - cx * cy, contour }
    }

    pub fn width(&self) -> usize { self.bbox.2 - self.bbox.0 + 1 }
    pub fn height(&self) -> usize { self.bbox.3 - self.bbox.1 + 1 }

    /// The ellipse with the same second moments: full axis lengths (major, minor) and the major axis' angle.
    pub fn ellipse(&self) -> (f64, f64, f64) {
        let t = ((self.mxx - self.myy) / 2.0).hypot(self.mxy);
        let mean = (self.mxx + self.myy) / 2.0;
        let (l1, l2) = ((mean + t).max(0.0), (mean - t).max(0.0));
        let ang = 0.5 * (2.0 * self.mxy).atan2(self.mxx - self.myy);
        (4.0 * l1.sqrt(), 4.0 * l2.sqrt(), ang.to_degrees())
    }
}

/// Moore-neighbour trace of the outer border of the solid pixels in a padded grid (a one-pixel margin of false), then
/// the CHAIN_APPROX_SIMPLE compression: a point goes when the step into it and the step out of it are the same
/// horizontal, vertical or diagonal direction.
fn trace_outline(solid: &[bool], pw: usize, ph: usize) -> Vec<(usize, usize)> {
    let Some(first) = solid.iter().position(|&s| s) else { return Vec::new() };
    let start = (first % pw, first / pw);
    // clockwise from west, y down
    const DIRS: [(i32, i32); 8] = [(-1, 0), (-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1)];
    let at = |x: i32, y: i32| x >= 0 && y >= 0 && (x as usize) < pw && (y as usize) < ph && solid[y as usize * pw + x as usize];
    let mut path = vec![start];
    let (mut c, mut from) = (start, 0usize); // we arrived from the west (background)
    let mut first_step: Option<((usize, usize), usize)> = None;
    for _ in 0..pw * ph * 8 {
        let mut found = None;
        for k in 0..8 {
            let d = (from + k) % 8;
            let (nx, ny) = (c.0 as i32 + DIRS[d].0, c.1 as i32 + DIRS[d].1);
            if at(nx, ny) { found = Some((d, (nx as usize, ny as usize))); break; }
        }
        let Some((d, next)) = found else { break }; // a single pixel
        if let Some(fs) = first_step {
            if c == start && (next, d) == fs { path.pop(); break; } // back at the start, about to repeat the first step
        } else {
            first_step = Some((next, d));
        }
        // the background neighbour examined just before `d` is where the next search starts: turn back by one step
        from = (d + 5) % 8; // opposite of the move, then one further round
        c = next;
        path.push(c);
    }
    // CHAIN_APPROX_SIMPLE
    let n = path.len();
    if n < 3 { return path; }
    let step = |a: (usize, usize), b: (usize, usize)| (b.0 as i32 - a.0 as i32, b.1 as i32 - a.1 as i32);
    (0..n).filter(|&i| step(path[(i + n - 1) % n], path[i]) != step(path[i], path[(i + 1) % n])).map(|i| path[i]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mask(w: usize, h: usize, f: impl Fn(usize, usize) -> bool) -> Mask { Mask { w, h, d: (0..w * h).map(|i| f(i % w, i / w)).collect() } }

    #[test]
    fn a_filled_rectangle_traces_to_its_four_corners() {
        let m = mask(30, 20, |x, y| (5..15).contains(&x) && (4..12).contains(&y));
        let b = &m.blobs()[0];
        let mut c = b.contour.clone();
        c.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(c, vec![(5.0, 4.0), (5.0, 11.0), (14.0, 4.0), (14.0, 11.0)], "straight runs are dropped, the corners stay");
        assert_eq!((b.area, b.width(), b.height()), (80, 10, 8));
    }

    #[test]
    fn a_disc_traces_a_closed_outline_of_boundary_pixels_only_and_a_hole_is_filled() {
        let ring = mask(60, 60, |x, y| { let d = (((x as f64) - 30.0).powi(2) + ((y as f64) - 30.0).powi(2)).sqrt(); d <= 20.0 && d > 8.0 });
        let b = &ring.blobs()[0];
        let r: Vec<f64> = b.contour.iter().map(|p| ((p.0 - 30.0).powi(2) + (p.1 - 30.0).powi(2)).sqrt()).collect();
        assert!(r.len() > 20, "{} points", r.len());
        assert!(r.iter().all(|&d| d > 18.0 && d <= 20.5), "the OUTER outline, not the inner edge of the ring: {r:?}");
        assert!(b.area > (std::f64::consts::PI * 19.0 * 19.0) as usize, "the hole is inside the outline: {}", b.area);
    }

    #[test]
    fn single_pixels_and_diagonal_lines_do_not_hang_or_vanish() {
        let one = mask(10, 10, |x, y| x == 4 && y == 4);
        assert_eq!(one.blobs()[0].contour, vec![(4.0, 4.0)]);
        let diag = mask(20, 20, |x, y| x == y && x < 12);
        let b = &diag.blobs()[0];
        assert_eq!(b.area, 12);
        assert!(b.contour.len() >= 2, "{:?}", b.contour);
    }

    #[test]
    fn dilation_by_four_equals_a_5x5_kernel_twice() {
        let m = mask(40, 40, |x, y| x == 20 && y == 20);
        let d = m.dilate_square(4);
        let on: Vec<(usize, usize)> = (0..1600).filter(|&i| d.d[i]).map(|i| (i % 40, i / 40)).collect();
        assert_eq!(on.len(), 81);
        assert!(on.iter().all(|&(x, y)| (16..=24).contains(&x) && (16..=24).contains(&y)));
    }

    #[test]
    fn pgm_round_trips_and_garbage_is_none() {
        let g = Gray::from_fn(7, 5, |x, y| (x * 30 + y) as u8);
        assert_eq!(Gray::from_pgm(&g.to_pgm()), Some(g));
        assert!(Gray::from_pgm(b"P6\n1 1\n255\nabc").is_none());
        assert!(Gray::from_pgm(b"P5\n4 4\n255\nshort").is_none());
        assert!(Gray::from_pgm(b"").is_none());
    }
}
