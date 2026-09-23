//! Ambience: the shell learning what places are like, and dressing to
//! match. A place is recognized by the WiFi it is on or by a snapclone
//! landmark (a typed record with an id and a photo fingerprint); each visit
//! logs the light and sound floor by hour and the dominant colours of what
//! the camera pane saw. A profile then yields design hints: an accent drawn
//! from the place's own colours, its warmth, and whether it is a quiet
//! place (softer motion, quieter notifications).
//!
//! The contract for other programs (see deploy/AMBIENCE-API.md):
//! * a place hint file: `XDG_RUNTIME_DIR/tideshell.place`, one line,
//!   `landmark=<id> name=<text>` or `wifi=<bssid> name=<ssid>`;
//!   snapclone writes it when it recognizes a landmark, the shell writes
//!   the wifi form itself;
//! * the log: `~/.local/share/tideshell/places.log`, one profile per line
//!   (`PlaceProfile::to_line` / `from_line`), human-readable and append-safe.
//! Nothing here leaves the device.

use crate::Millis;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PlaceKey {
    Wifi(String),
    Landmark(String),
}

impl PlaceKey {
    pub fn parse(s: &str) -> Option<(PlaceKey, String)> {
        let mut key = None;
        let mut name = String::new();
        for kv in s.split_whitespace() {
            let (k, v) = kv.split_once('=')?;
            match k {
                "wifi" => key = Some(PlaceKey::Wifi(v.to_string())),
                "landmark" => key = Some(PlaceKey::Landmark(v.to_string())),
                "name" => name = v.replace('_', " "),
                _ => {}
            }
        }
        key.map(|k| (k, name))
    }

    pub fn token(&self) -> String {
        match self {
            PlaceKey::Wifi(b) => format!("wifi:{b}"),
            PlaceKey::Landmark(i) => format!("landmark:{i}"),
        }
    }

    fn from_token(t: &str) -> Option<PlaceKey> {
        let (k, v) = t.split_once(':')?;
        match k {
            "wifi" => Some(PlaceKey::Wifi(v.to_string())),
            "landmark" => Some(PlaceKey::Landmark(v.to_string())),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaceProfile {
    pub key: PlaceKey,
    pub name: String,
    pub visits: u32,
    /// (sum of log10 lux, samples) per hour of the day.
    pub lux_by_hour: [(f32, u32); 24],
    /// Running mean of the sound floor, dBFS, and its sample count.
    pub sound_floor: (f32, u32),
    /// Dominant colours seen here, most frequent first (RGB).
    pub colors: Vec<[u8; 3]>,
    pub last_seen_ms: Millis,
}

impl PlaceProfile {
    pub fn new(key: PlaceKey, name: &str) -> PlaceProfile {
        PlaceProfile { key, name: name.to_string(), visits: 0, lux_by_hour: [(0.0, 0); 24], sound_floor: (0.0, 0), colors: Vec::new(), last_seen_ms: 0 }
    }

    pub fn note_lux(&mut self, lux: f32, hour: u32) {
        let h = &mut self.lux_by_hour[(hour % 24) as usize];
        h.0 += (lux.max(0.0) + 1.0).log10();
        h.1 += 1;
    }

    pub fn note_sound(&mut self, floor_dbfs: f32) {
        let (m, n) = self.sound_floor;
        self.sound_floor = ((m * n as f32 + floor_dbfs) / (n + 1) as f32, n + 1);
    }

    /// Merge a photo's dominant colours: known ones move to the front,
    /// new ones append; at most 6 kept.
    pub fn note_colors(&mut self, colors: &[[u8; 3]]) {
        for c in colors {
            if let Some(i) = self.colors.iter().position(|k| close(k, c)) {
                let known = self.colors.remove(i);
                self.colors.insert(0, known);
            } else {
                self.colors.push(*c);
            }
        }
        self.colors.truncate(6);
    }

    /// Typical lux at this hour, if seen.
    pub fn typical_lux(&self, hour: u32) -> Option<f32> {
        let (s, n) = self.lux_by_hour[(hour % 24) as usize];
        (n > 0).then(|| 10f32.powf(s / n as f32) - 1.0)
    }

    pub fn is_quiet(&self) -> bool {
        self.sound_floor.1 >= 3 && self.sound_floor.0 < -50.0
    }

    pub fn hints(&self, hour: u32) -> DesignHints {
        // Warmth from the place's colours: red/yellow-heavy = warm.
        let warmth = if self.colors.is_empty() {
            0.0
        } else {
            let mut w = 0.0;
            for c in &self.colors {
                let (r, g, b) = (c[0] as f32, c[1] as f32, c[2] as f32);
                w += ((r + 0.5 * g) - 1.5 * b) / 255.0;
            }
            (w / self.colors.len() as f32).clamp(-1.0, 1.0)
        };
        DesignHints { accent: self.colors.first().copied(), warmth, typical_lux: self.typical_lux(hour), quiet: self.is_quiet(), name: self.name.clone() }
    }

    /// One line: `key|name|visits|lux=h:s/n,...|sound=m/n|colors=rrggbb,...|last=ms`.
    pub fn to_line(&self) -> String {
        let lux: Vec<String> = self.lux_by_hour.iter().enumerate().filter(|(_, (_, n))| *n > 0).map(|(h, (s, n))| format!("{h}:{s:.3}/{n}")).collect();
        let colors: Vec<String> = self.colors.iter().map(|c| format!("{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect();
        format!(
            "{}|{}|{}|lux={}|sound={:.1}/{}|colors={}|last={}",
            self.key.token(),
            self.name.replace('|', " "),
            self.visits,
            lux.join(","),
            self.sound_floor.0,
            self.sound_floor.1,
            colors.join(","),
            self.last_seen_ms
        )
    }

    pub fn from_line(line: &str) -> Option<PlaceProfile> {
        let mut parts = line.trim().split('|');
        let key = PlaceKey::from_token(parts.next()?)?;
        let mut p = PlaceProfile::new(key, parts.next()?);
        p.visits = parts.next()?.parse().ok()?;
        for field in parts {
            let Some((k, v)) = field.split_once('=') else { continue };
            match k {
                "lux" => {
                    for e in v.split(',').filter(|e| !e.is_empty()) {
                        let (h, rest) = e.split_once(':')?;
                        let (s, n) = rest.split_once('/')?;
                        let h: usize = h.parse().ok()?;
                        if h < 24 {
                            p.lux_by_hour[h] = (s.parse().ok()?, n.parse().ok()?);
                        }
                    }
                }
                "sound" => {
                    let (m, n) = v.split_once('/')?;
                    p.sound_floor = (m.parse().ok()?, n.parse().ok()?);
                }
                "colors" => {
                    p.colors = v.split(',').filter(|c| c.len() == 6).filter_map(|c| u32::from_str_radix(c, 16).ok()).map(|x| [(x >> 16) as u8, (x >> 8) as u8, x as u8]).collect();
                }
                "last" => p.last_seen_ms = v.parse().ok()?,
                _ => {}
            }
        }
        Some(p)
    }
}

fn close(a: &[u8; 3], b: &[u8; 3]) -> bool {
    (a[0] as i32 - b[0] as i32).abs() + (a[1] as i32 - b[1] as i32).abs() + (a[2] as i32 - b[2] as i32).abs() < 60
}

#[derive(Debug, Clone, PartialEq)]
pub struct DesignHints {
    pub accent: Option<[u8; 3]>,
    pub warmth: f32,
    pub typical_lux: Option<f32>,
    pub quiet: bool,
    pub name: String,
}

/// Dominant colours of an RGBA image: a coarse hue/saturation/value
/// histogram, saturated bins preferred, up to `n` colours, most populous
/// first. Grey (low saturation) counts only when nothing colourful exists.
pub fn dominant_colors(rgba: &[u8], n: usize) -> Vec<[u8; 3]> {
    const HB: usize = 12;
    let mut bins = vec![(0u32, [0u64; 3]); HB * 3 * 3];
    let mut grey = (0u32, [0u64; 3]);
    for p in rgba.chunks_exact(4).step_by(3) {
        let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
        // Grey by absolute chroma too: a dark blue-grey (30,40,50) has a
        // saturation ratio of 0.4 but no colour to speak of (found by a test).
        if sat < 0.18 || max - min < 40.0 || max < 30.0 {
            grey.0 += 1;
            grey.1[0] += p[0] as u64;
            grey.1[1] += p[1] as u64;
            grey.1[2] += p[2] as u64;
            continue;
        }
        let d = max - min;
        let mut h = if max == r { (g - b) / d } else if max == g { 2.0 + (b - r) / d } else { 4.0 + (r - g) / d };
        h = (h * 60.0 + 360.0) % 360.0;
        let hb = ((h / 360.0) * HB as f32) as usize % HB;
        let sb = if sat < 0.45 { 0 } else if sat < 0.75 { 1 } else { 2 };
        let vb = if max < 100.0 { 0 } else if max < 190.0 { 1 } else { 2 };
        let bin = &mut bins[(hb * 3 + sb) * 3 + vb];
        bin.0 += 1;
        bin.1[0] += p[0] as u64;
        bin.1[1] += p[1] as u64;
        bin.1[2] += p[2] as u64;
    }
    let mut order: Vec<&(u32, [u64; 3])> = bins.iter().filter(|b| b.0 > 0).collect();
    order.sort_by(|a, b| b.0.cmp(&a.0));
    let mean = |b: &(u32, [u64; 3])| [(b.1[0] / b.0 as u64) as u8, (b.1[1] / b.0 as u64) as u8, (b.1[2] / b.0 as u64) as u8];
    let mut out: Vec<[u8; 3]> = order.into_iter().take(n).map(mean).collect();
    if out.is_empty() && grey.0 > 0 {
        out.push(mean(&grey));
    }
    out
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ambience {
    pub places: Vec<PlaceProfile>,
    pub current: Option<PlaceKey>,
}

impl Ambience {
    pub fn load(text: &str) -> Ambience {
        Ambience { places: text.lines().filter_map(PlaceProfile::from_line).collect(), current: None }
    }

    pub fn save(&self) -> String {
        self.places.iter().map(|p| p.to_line()).collect::<Vec<_>>().join("\n") + "\n"
    }

    /// Arrived somewhere (or the same place again): counts a visit when the
    /// key changes.
    pub fn arrive(&mut self, key: PlaceKey, name: &str, now: Millis) -> bool {
        let changed = self.current.as_ref() != Some(&key);
        let p = match self.places.iter_mut().position(|p| p.key == key) {
            Some(i) => &mut self.places[i],
            None => {
                self.places.push(PlaceProfile::new(key.clone(), name));
                self.places.last_mut().unwrap()
            }
        };
        if !name.is_empty() {
            p.name = name.to_string();
        }
        if changed {
            p.visits += 1;
        }
        p.last_seen_ms = now;
        self.current = Some(key);
        changed
    }

    pub fn here(&mut self) -> Option<&mut PlaceProfile> {
        let key = self.current.clone()?;
        self.places.iter_mut().find(|p| p.key == key)
    }

    pub fn hints(&self, hour: u32) -> Option<DesignHints> {
        let key = self.current.as_ref()?;
        self.places.iter().find(|p| &p.key == key).map(|p| p.hints(hour))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_round_trip_through_lines() {
        let mut p = PlaceProfile::new(PlaceKey::Wifi("aa:bb".into()), "Home|net");
        p.visits = 3;
        p.note_lux(300.0, 9);
        p.note_lux(3000.0, 9);
        p.note_sound(-55.0);
        p.note_colors(&[[200, 120, 40], [30, 60, 120]]);
        p.last_seen_ms = 12345;
        let line = p.to_line();
        let q = PlaceProfile::from_line(&line).unwrap();
        assert_eq!(q.key, p.key);
        assert_eq!(q.name, "Home net", "the separator is escaped");
        assert_eq!(q.visits, 3);
        assert_eq!(q.lux_by_hour[9].1, 2);
        assert!((q.typical_lux(9).unwrap() - 949.0).abs() < 5.0, "geometric mean of 300 and 3000: {:?}", q.typical_lux(9));
        assert_eq!(q.sound_floor, (-55.0, 1));
        assert_eq!(q.colors, vec![[200, 120, 40], [30, 60, 120]]);
        assert_eq!(PlaceProfile::from_line("garbage"), None);
    }

    #[test]
    fn place_hints_and_keys() {
        let (k, name) = PlaceKey::parse("landmark=lm-42 name=Cafe_Nord").unwrap();
        assert_eq!((k, name.as_str()), (PlaceKey::Landmark("lm-42".into()), "Cafe Nord"));
        assert!(PlaceKey::parse("name=nothing").is_none());
        let mut a = Ambience::default();
        assert!(a.arrive(PlaceKey::Wifi("x".into()), "Office", 0));
        assert!(!a.arrive(PlaceKey::Wifi("x".into()), "", 10), "same place: not a new visit");
        assert!(a.arrive(PlaceKey::Landmark("l".into()), "Lake", 20));
        assert_eq!(a.places.len(), 2);
        {
            let h = a.here().unwrap();
            h.note_colors(&[[220, 170, 60]]);
            for _ in 0..3 {
                h.note_sound(-62.0);
            }
        }
        let hints = a.hints(12).unwrap();
        assert_eq!(hints.accent, Some([220, 170, 60]));
        assert!(hints.warmth > 0.3, "an amber place is warm: {}", hints.warmth);
        assert!(hints.quiet);
        assert_eq!(hints.name, "Lake");
        let saved = a.save();
        let b = Ambience::load(&saved);
        assert_eq!(b.places, a.places);
    }

    #[test]
    fn dominant_colours_prefer_the_colourful_and_fall_back_to_grey() {
        let mut img = vec![0u8; 100 * 100 * 4];
        for (i, p) in img.chunks_exact_mut(4).enumerate() {
            let c: [u8; 4] = if i % 100 < 60 { [30, 40, 50, 255] } else if i % 100 < 90 { [200, 90, 40, 255] } else { [40, 160, 220, 255] };
            p.copy_from_slice(&c);
        }
        let c = dominant_colors(&img, 3);
        assert!(c.len() >= 2);
        assert!(close(&c[0], &[200, 90, 40]), "the orange, not the (larger) grey: {c:?}");
        let grey = vec![120u8, 120, 120, 255].repeat(50);
        assert_eq!(dominant_colors(&grey, 2), vec![[120, 120, 120]]);
    }
}
