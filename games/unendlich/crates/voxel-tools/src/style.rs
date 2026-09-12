//! Styles: an axis every form reads.
//!
//! "A house" from the dictionary was one house. A Nordic longhouse, a
//! Tudor cottage, a desert flat-roof and a dwarven hall differ in five
//! things at once -- wall material, roof shape and pitch, proportions,
//! trim, and the finishing that suits them -- and asking a model to set
//! all five per building is asking for a mismatch. A STYLE sets them
//! together, so `form house A B style=nordic` is coherent, and the
//! site can pick one when the player names none.
//!
//! Styles are data. Adding one is a row.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub name: &'static str,
    /// Wall, trim, roof, floor blocks.
    pub wall: u16,
    pub trim: u16,
    pub roof_block: u16,
    pub floor: u16,
    /// gable | hip | flat | pyramid
    pub roof: &'static str,
    /// Roof height as a fraction of the shorter footprint side.
    pub pitch: f32,
    /// Wall height in blocks for a single storey.
    pub storey: i32,
    /// Window spacing along a wall.
    pub window_every: i32,
    /// Chance of a chimney, dormers, a porch: the seeded variants.
    pub chimney: f32,
    pub dormers: f32,
    pub porch: f32,
    /// Finishing words this style implies, run after the build.
    pub finishing: &'static str,
    /// Greeble phrase for decorated surfaces.
    pub greeble: &'static str,
}

pub const STYLES: &[Style] = &[
    // storey 5: a shell includes its ceiling, so 4 leaves two blocks of
    // headroom and the player's head in the rafters
    Style { name: "cottage", wall: 57, trim: 86, roof_block: 86, floor: 57, roof: "gable", pitch: 0.55,
            storey: 5, window_every: 3, chimney: 0.9, dormers: 0.4, porch: 0.5,
            finishing: "cosy", greeble: "clean" },
    Style { name: "tudor", wall: 57, trim: 86, roof_block: 4, floor: 57, roof: "gable", pitch: 0.7,
            storey: 5, window_every: 2, chimney: 1.0, dormers: 0.6, porch: 0.2,
            finishing: "ornate", greeble: "gothic symmetric" },
    Style { name: "nordic", wall: 17, trim: 86, roof_block: 86, floor: 57, roof: "gable", pitch: 0.8,
            storey: 5, window_every: 4, chimney: 0.7, dormers: 0.0, porch: 0.6,
            finishing: "rustic", greeble: "clean" },
    Style { name: "stone", wall: 58, trim: 1, roof_block: 86, floor: 4, roof: "hip", pitch: 0.45,
            storey: 5, window_every: 3, chimney: 0.8, dormers: 0.2, porch: 0.1,
            finishing: "grand", greeble: "gothic" },
    Style { name: "desert", wall: 48, trim: 24, roof_block: 48, floor: 48, roof: "flat", pitch: 0.0,
            storey: 5, window_every: 3, chimney: 0.0, dormers: 0.0, porch: 0.7,
            finishing: "rustic", greeble: "clean minimal" },
    Style { name: "dwarven", wall: 1, trim: 58, roof_block: 1, floor: 4, roof: "pyramid", pitch: 0.35,
            storey: 6, window_every: 5, chimney: 0.5, dormers: 0.0, porch: 0.0,
            finishing: "grand industrial", greeble: "industrial symmetric" },
    Style { name: "elven", wall: 20, trim: 37, roof_block: 18, floor: 19, roof: "gable", pitch: 0.9,
            storey: 5, window_every: 2, chimney: 0.0, dormers: 0.3, porch: 0.8,
            finishing: "sacred", greeble: "gothic" },
    Style { name: "gothic", wall: 58, trim: 86, roof_block: 86, floor: 58, roof: "gable", pitch: 1.0,
            storey: 7, window_every: 2, chimney: 0.6, dormers: 0.5, porch: 0.0,
            finishing: "ornate", greeble: "gothic symmetric" },
];

pub fn lookup(name: &str) -> Option<&'static Style> {
    STYLES.iter().find(|s| s.name.eq_ignore_ascii_case(name.trim()))
}

/// A style from the site, when the player names none. Cold and wooded
/// is Nordic, hot and dry is desert, high ground is stone, the rest is
/// cottage. A default that reads as belonging is the whole point.
pub fn for_site(amb: &crate::ambient::Ambient) -> &'static Style {
    let s = amb.suggest();
    let name = match s.palette {
        "desert" | "mesa" => "desert",
        "snowy" | "frozen" | "darkwood" => "nordic",
        "stone" | "volcanic" => if amb.altitude() > 40 { "dwarven" } else { "stone" },
        "jungle" => "elven",
        _ => "cottage",
    };
    lookup(name).expect("every site style is a real style")
}

/// Words that name a style in prose.
pub fn from_prose(prose: &str) -> Option<&'static Style> {
    let p = prose.to_lowercase();
    let words: Vec<&str> = p.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |ws: &[&str]| ws.iter().any(|w| words.contains(w));
    let name = if has(&["tudor", "timber", "timbered", "half"]) { "tudor" }
        else if has(&["nordic", "norse", "viking", "longhouse"]) { "nordic" }
        else if has(&["desert", "adobe", "pueblo", "sandstone"]) { "desert" }
        else if has(&["dwarven", "dwarf", "dwarvish", "underground"]) { "dwarven" }
        else if has(&["elven", "elvish", "elf", "fae"]) { "elven" }
        else if has(&["gothic", "cathedral", "spired"]) { "gothic" }
        else if has(&["stone", "castle", "keep", "fort"]) { "stone" }
        else if has(&["cottage", "cosy", "cozy", "quaint", "hut"]) { "cottage" }
        else { return None };
    lookup(name)
}

/// The prompt line, generated from the table.
pub fn vocabulary() -> String {
    format!("STYLES (style=NAME on any form): {}",
            STYLES.iter().map(|s| s.name).collect::<Vec<_>>().join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_style_is_coherent() {
        let pals = crate::palettes();
        let known: std::collections::BTreeSet<u16> = pals.values()
            .flat_map(|p| [p.primary, p.secondary, p.accent, p.foliage]).chain([1, 4, 17, 18, 19, 20, 24, 37, 48, 57, 58, 86]).collect();
        for s in STYLES {
            for b in [s.wall, s.trim, s.roof_block, s.floor] {
                assert!(known.contains(&b), "style '{}' uses block {b} nobody knows", s.name);
            }
            assert!(["gable", "hip", "flat", "pyramid"].contains(&s.roof), "{}", s.name);
            assert!(s.storey >= 3 && s.storey <= 8, "{}", s.name);
            assert!(s.window_every >= 2, "{}", s.name);
            // the finishing words must be ones finish.rs knows, or the
            // style promises a treatment it cannot deliver
            let (ir, _) = crate::finish::from_prose(s.finishing, &[], Some(((0, 65, 0), (6, 70, 6))));
            assert!(!ir.is_empty(), "style '{}' finishing '{}' means nothing", s.name, s.finishing);
        }
        assert_eq!(STYLES.len(), STYLES.iter().map(|s| s.name).collect::<std::collections::BTreeSet<_>>().len());
    }

    #[test]
    fn the_site_picks_a_style_that_belongs() {
        use crate::ambient::Ambient;
        assert_eq!(for_site(&Ambient { surface_block: 12, ..Default::default() }).name, "desert");
        assert_eq!(for_site(&Ambient { temperature: 0.1, ..Default::default() }).name, "nordic");
        assert_eq!(for_site(&Ambient { surface_y: 200, temperature: 0.35, humidity: 0.4, ..Default::default() }).name, "dwarven");
        assert_eq!(for_site(&Ambient::default()).name, "cottage");
    }

    #[test]
    fn prose_names_a_style_by_whole_words() {
        assert_eq!(from_prose("a half-timbered inn").map(|s| s.name), Some("tudor"));
        assert_eq!(from_prose("a viking longhouse").map(|s| s.name), Some("nordic"));
        assert_eq!(from_prose("a castle on the hill").map(|s| s.name), Some("stone"));
        assert_eq!(from_prose("a tower").map(|s| s.name), None);
        assert_eq!(from_prose("a sandstorm").map(|s| s.name), None, "substring, not a word");
    }
}
