//! cad-nifty — reading niftyplot's **hardware** kind, the one CADscope needs and the one
//! `shared/nifty-core` still has stubbed. The spec is `apps/voiceterm/docs/niftyplot-spec.md`;
//! this implements its Kind 3, and the intent (plan §9, §17.5) is that it upstreams into
//! nifty-core once it has been exercised against real boards.
//!
//! Why CADscope cares: a hardware diagram is a *dimensioned description of something physical*.
//! Its coordinates are millimetres by definition — the spec is explicit that hardware's native
//! unit is mm, not pixels — which makes it the one corpus entry arriving with units already
//! settled, no `ref.toml` declaration needed. "Design an enclosure that fits this board" is then
//! arithmetic over component extents and keep-out zones rather than an act of interpretation.
//!
//! The graph kind is deliberately *not* reimplemented; `shared/nifty-core` already parses it, and
//! a second spelling of one format is worse than none.

use glam::Vec2;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    Graph,
    Gantt,
    #[default]
    Hardware,
    Sequence,
    State,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "graph" => Kind::Graph,
            "gantt" => Kind::Gantt,
            "hardware" => Kind::Hardware,
            "sequence" => Kind::Sequence,
            "state" => Kind::State,
            _ => return None,
        })
    }
}

/// A pin. The spec's descriptors are cardinal words with an optional offset (`right + 5`); both
/// the word and the parsed offset are kept, because CADscope needs the number for clearance
/// arithmetic and the word for display.
#[derive(Clone, Debug, PartialEq)]
pub struct Pin {
    pub name: String,
    pub descriptor: String,
    pub offset_mm: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    /// `chip`, `connector`, `resistor`, `capacitor`, ...
    pub role: String,
    pub name: String,
    pub at: Vec2,
    pub rotation_deg: f32,
    /// Parenthesised value such as `10k` or `100nF`, when present.
    pub value: Option<String>,
    /// Enclosing `plane` block, if any. Components outside a plane sit on top copper.
    pub plane: Option<String>,
    pub pins: Vec<Pin>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trace {
    pub from: (String, String),
    pub to: (String, String),
    pub width_mm: Option<f32>,
    pub ground: bool,
    pub layer: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub name: Option<String>,
    /// `fill copper`, when the zone is a fill rather than a rect.
    pub fill: Option<String>,
    pub rect: Option<(Vec2, Vec2)>,
    pub layer: Option<String>,
}

impl Zone {
    /// The spec names keep-outs by convention rather than by keyword, so this matches the name.
    pub fn is_keepout(&self) -> bool {
        self.name.as_deref().map(|n| n.to_ascii_lowercase().contains("keepout")).unwrap_or(false)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Via {
    pub name: String,
    /// Either a coordinate or a `Component.Pin` reference, as written.
    pub at: String,
    pub at_point: Option<Vec2>,
    pub through_all: bool,
    pub through: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HardwareDoc {
    pub kind: Kind,
    pub title: Option<String>,
    /// As declared by `%units:`. Hardware is millimetres by definition, so this confirms rather
    /// than decides.
    pub units: Option<String>,
    pub planes: Vec<String>,
    pub components: Vec<Component>,
    pub traces: Vec<Trace>,
    pub zones: Vec<Zone>,
    pub vias: Vec<Via>,
    /// Unknown directives and unparseable lines. The spec is explicit that these warn rather than
    /// abort — a diagram still renders with the rest intact — and the same rule serves a corpus
    /// scan, which must never fail on one odd file.
    pub warnings: Vec<String>,
}

impl HardwareDoc {
    /// Bounding box over everything with a position, in millimetres: the extent an enclosure has
    /// to accommodate.
    pub fn extent_mm(&self) -> Option<(Vec2, Vec2)> {
        let mut lo = Vec2::splat(f32::INFINITY);
        let mut hi = Vec2::splat(f32::NEG_INFINITY);
        let mut any = false;
        for c in &self.components {
            lo = lo.min(c.at);
            hi = hi.max(c.at);
            any = true;
        }
        for z in &self.zones {
            if let Some((a, b)) = z.rect {
                lo = lo.min(a.min(b));
                hi = hi.max(a.max(b));
                any = true;
            }
        }
        for v in &self.vias {
            if let Some(p) = v.at_point {
                lo = lo.min(p);
                hi = hi.max(p);
                any = true;
            }
        }
        (any && lo.is_finite() && hi.is_finite()).then_some((lo, hi))
    }

    /// Rectangles nothing may occupy — what a bracket or enclosure must design around.
    pub fn keepouts(&self) -> Vec<(Vec2, Vec2)> {
        self.zones.iter().filter(|z| z.is_keepout()).filter_map(|z| z.rect).collect()
    }
}

fn strip_quotes(s: &str) -> String {
    s.trim().trim_matches('"').trim_matches('\'').to_string()
}

/// `(10, 20)` -> Vec2. Tolerates spaces and a trailing `mm`.
fn parse_point(s: &str) -> Option<Vec2> {
    let inner = s.trim().strip_prefix('(')?.split(')').next()?;
    let mut it = inner.split(',');
    let x: f32 = it.next()?.trim().trim_end_matches("mm").trim().parse().ok()?;
    let y: f32 = it.next()?.trim().trim_end_matches("mm").trim().parse().ok()?;
    Some(Vec2::new(x, y))
}

/// Take the first `(...)` group, returning it and the remainder.
fn take_paren(s: &str) -> Option<(&str, &str)> {
    let start = s.find('(')?;
    let end = s[start..].find(')')? + start;
    Some((&s[start..=end], &s[end + 1..]))
}

fn number_after(s: &str, key: &str) -> Option<f32> {
    let idx = s.find(key)? + key.len();
    let rest = s[idx..].trim_start();
    let tok: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    tok.parse().ok()
}

/// Parse a niftyplot document. Only the hardware kind is interpreted past the header; other kinds
/// report their kind so a corpus can index them without pretending to understand them.
pub fn parse(text: &str) -> HardwareDoc {
    let mut doc = HardwareDoc { kind: Kind::Hardware, ..Default::default() };
    let mut kind_seen = false;
    let mut plane: Option<String> = None;
    let mut component: Option<Component> = None;

    for raw in text.lines() {
        // Strip comments, but not inside quotes — titles routinely contain punctuation.
        let line = {
            let mut out = String::new();
            let mut in_q = false;
            for c in raw.chars() {
                match c {
                    '"' => {
                        in_q = !in_q;
                        out.push(c);
                    }
                    '#' if !in_q => break,
                    _ => out.push(c),
                }
            }
            out.trim().to_string()
        };
        if line.is_empty() {
            continue;
        }

        if line == "}" {
            if let Some(c) = component.take() {
                doc.components.push(c);
            } else {
                plane = None;
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix('%') {
            let (key, value) = rest.split_once(':').unwrap_or((rest, ""));
            match key.trim().to_ascii_lowercase().as_str() {
                "kind" => match Kind::parse(value) {
                    Some(k) => {
                        doc.kind = k;
                        kind_seen = true;
                    }
                    None => doc.warnings.push(format!("unknown kind '{}'", value.trim())),
                },
                "title" => doc.title = Some(strip_quotes(value)),
                "units" => doc.units = Some(strip_quotes(value)),
                "tilt" | "zoom" | "theme" | "layout" => {}
                other => doc.warnings.push(format!("unknown directive '%{other}'")),
            }
            continue;
        }

        if doc.kind != Kind::Hardware {
            continue;
        }

        let mut it = line.split_whitespace();
        let head = it.next().unwrap_or("");
        match head {
            "plane" => {
                let name = line.trim_start_matches("plane").trim().trim_end_matches('{').trim().to_string();
                if !name.is_empty() {
                    doc.planes.push(name.clone());
                    plane = Some(name);
                }
            }
            "pin" => {
                let body = line.trim_start_matches("pin").trim();
                let (name, desc) = body.split_once(':').unwrap_or((body, ""));
                let desc = desc.trim();
                let offset = desc
                    .split_once('+')
                    .and_then(|(_, o)| o.trim().trim_end_matches("mm").trim().parse::<f32>().ok());
                let pin = Pin {
                    name: name.trim().to_string(),
                    descriptor: desc.to_string(),
                    offset_mm: offset.unwrap_or(0.0),
                };
                match component.as_mut() {
                    Some(c) => c.pins.push(pin),
                    None => doc.warnings.push(format!("pin '{}' outside a component", pin.name)),
                }
            }
            "trace" => {
                let body = line.trim_start_matches("trace").trim();
                // The spec writes the arrow as a Unicode arrow; accept the ASCII form too.
                let arrow = body
                    .find('\u{2192}')
                    .map(|i| (i, '\u{2192}'.len_utf8()))
                    .or_else(|| body.find("->").map(|i| (i, 2)));
                let Some((i, alen)) = arrow else {
                    doc.warnings.push(format!("trace without an arrow: {body}"));
                    continue;
                };
                let lhs = body[..i].trim();
                let rhs_full = body[i + alen..].trim();
                let rhs_end = rhs_full
                    .find(" width")
                    .or_else(|| rhs_full.find(" on "))
                    .or_else(|| rhs_full.find(" ground"))
                    .unwrap_or(rhs_full.len());
                let rhs = rhs_full[..rhs_end].trim();
                let split_ref = |s: &str| {
                    s.split_once('.')
                        .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
                        .unwrap_or_else(|| (s.trim().to_string(), String::new()))
                };
                doc.traces.push(Trace {
                    from: split_ref(lhs),
                    to: split_ref(rhs),
                    width_mm: number_after(rhs_full, "width"),
                    ground: rhs_full.contains("ground"),
                    layer: rhs_full.split_once(" on ").map(|(_, l)| strip_quotes(l)),
                });
            }
            "via" => {
                let body = line.trim_start_matches("via").trim();
                let name = strip_quotes(body.split_whitespace().next().unwrap_or(""));
                let at = body.split_once(" at ").map(|(_, a)| a).unwrap_or("");
                let at_end = at.find(" through ").unwrap_or(at.len());
                let at_txt = at[..at_end].trim().to_string();
                doc.vias.push(Via {
                    name,
                    at_point: parse_point(&at_txt),
                    at: at_txt,
                    through_all: body.contains("through all"),
                    through: body
                        .split_once(" through ")
                        .map(|(_, t)| t.split('\u{2192}').map(strip_quotes).filter(|s| !s.is_empty()).collect())
                        .unwrap_or_default(),
                });
            }
            "zone" => {
                let body = line.trim_start_matches("zone").trim();
                let layer = body.split_once(" on ").map(|(_, l)| strip_quotes(l));
                let rect = body.split_once(" to ").and_then(|(a, b)| {
                    let from = take_paren(a).and_then(|(p, _)| parse_point(p))?;
                    let to = take_paren(b).and_then(|(p, _)| parse_point(p))?;
                    Some((from, to))
                });
                let name = body.split_once(':').map(|(n, _)| strip_quotes(n.trim()));
                let fill = body.starts_with("fill").then(|| {
                    body.trim_start_matches("fill").trim().split_whitespace().next().unwrap_or("").to_string()
                });
                doc.zones.push(Zone { name, fill, rect, layer });
            }
            // Anything else carrying an `at (x, y)` reads as a component declaration.
            _ if line.contains(" at ") => {
                let name = it.next().unwrap_or("").to_string();
                let after_at = line.split_once(" at ").map(|(_, a)| a).unwrap_or("");
                let Some((pt, rest)) = take_paren(after_at) else {
                    doc.warnings.push(format!("component '{name}' has no position"));
                    continue;
                };
                let Some(at) = parse_point(pt) else {
                    doc.warnings.push(format!("component '{name}' has an unreadable position"));
                    continue;
                };
                let rotation_deg = number_after(rest, "rotated").unwrap_or(0.0);
                let value = take_paren(rest).map(|(v, _)| strip_quotes(v.trim_matches(['(', ')'])));
                let comp = Component {
                    role: head.to_string(),
                    name,
                    at,
                    rotation_deg,
                    value,
                    plane: plane.clone(),
                    pins: Vec::new(),
                };
                if line.ends_with('{') {
                    component = Some(comp);
                } else {
                    doc.components.push(comp);
                }
            }
            other => doc.warnings.push(format!("unrecognised statement '{other}'")),
        }
    }
    if let Some(c) = component.take() {
        doc.warnings.push(format!("component '{}' block was never closed", c.name));
        doc.components.push(c);
    }
    if !kind_seen {
        doc.warnings.push("no %kind: directive; assumed hardware".into());
    }
    doc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spec's own I2C breakout example, copied from niftyplot-spec.md.
    const BREAKOUT: &str = "%kind: hardware\n\
%title: \"I2C breakout v1\"\n\
%units: mm\n\
\n\
chip MCU at (10, 20) {\n\
    pin VCC : top-right\n\
    pin GND : bottom-left\n\
    pin SDA : right\n\
    pin SCL : right + 5\n\
}\n\
\n\
chip EEPROM at (60, 20) rotated 90\u{00b0} {\n\
    pin VCC : top\n\
    pin GND : bottom\n\
    pin SDA : left\n\
    pin SCL : left + 5\n\
}\n\
\n\
trace MCU.SDA \u{2192} EEPROM.SDA width 0.2\n\
trace MCU.SCL \u{2192} EEPROM.SCL width 0.2\n\
trace MCU.VCC \u{2192} EEPROM.VCC width 0.4\n\
trace MCU.GND \u{2192} EEPROM.GND width 0.6 ground\n";

    #[test]
    fn parses_the_specs_own_example() {
        let d = parse(BREAKOUT);
        assert_eq!(d.kind, Kind::Hardware);
        assert_eq!(d.title.as_deref(), Some("I2C breakout v1"));
        assert_eq!(d.units.as_deref(), Some("mm"));
        assert!(d.warnings.is_empty(), "{:?}", d.warnings);

        assert_eq!(d.components.len(), 2);
        assert_eq!(d.components[0].name, "MCU");
        assert_eq!(d.components[0].at, Vec2::new(10.0, 20.0));
        assert_eq!(d.components[0].pins.len(), 4);
        assert_eq!(d.components[1].rotation_deg, 90.0, "rotated 90 must parse through the degree sign");

        let scl = d.components[0].pins.iter().find(|p| p.name == "SCL").unwrap();
        assert_eq!(scl.offset_mm, 5.0, "the `right + 5` form carries its number");

        assert_eq!(d.traces.len(), 4);
        assert_eq!(d.traces[0].from, ("MCU".to_string(), "SDA".to_string()));
        assert_eq!(d.traces[0].to, ("EEPROM".to_string(), "SDA".to_string()));
        assert_eq!(d.traces[3].width_mm, Some(0.6));
        assert!(d.traces[3].ground);
        assert!(!d.traces[0].ground);

        // Coordinates are millimetres by definition of the kind, so extent needs no declaration.
        let (lo, hi) = d.extent_mm().unwrap();
        assert_eq!(lo, Vec2::new(10.0, 20.0));
        assert_eq!(hi, Vec2::new(60.0, 20.0));
    }

    #[test]
    fn ascii_arrows_and_layers_and_planes() {
        let d = parse("%kind: hardware\nplane top_copper {\n  chip MCU at (1, 2)\n}\ntrace MCU.GND -> MCU.VCC on \"ground plane\"\n");
        assert_eq!(d.planes, vec!["top_copper".to_string()]);
        assert_eq!(d.components[0].plane.as_deref(), Some("top_copper"));
        assert_eq!(d.traces.len(), 1, "the ASCII arrow must work too");
        assert_eq!(d.traces[0].layer.as_deref(), Some("ground plane"));
    }

    #[test]
    fn zones_keepouts_and_vias() {
        let d = parse("%kind: hardware\nzone fill copper on \"ground plane\"\nzone \"keepout\" : rect (40, 10) to (50, 30) on \"top copper\"\nvia \"thermal_1\" at (15, 25) through all layers\n");
        assert_eq!(d.zones.len(), 2);
        assert_eq!(d.zones[0].fill.as_deref(), Some("copper"));
        assert!(d.zones[1].is_keepout());
        assert_eq!(d.zones[1].rect, Some((Vec2::new(40.0, 10.0), Vec2::new(50.0, 30.0))));
        assert_eq!(d.keepouts().len(), 1, "a keep-out is what an enclosure designs around");
        assert_eq!(d.vias.len(), 1);
        assert!(d.vias[0].through_all);
        assert_eq!(d.vias[0].at_point, Some(Vec2::new(15.0, 25.0)));
    }

    #[test]
    fn a_component_value_is_captured() {
        let d = parse("%kind: hardware\nresistor R1 at (25, 30) (10k)\ncapacitor C1 at (30, 30) (100nF)\n");
        assert_eq!(d.components.len(), 2);
        assert_eq!(d.components[0].role, "resistor");
        assert_eq!(d.components[0].value.as_deref(), Some("10k"));
        assert_eq!(d.components[1].value.as_deref(), Some("100nF"));
    }

    #[test]
    fn other_kinds_parse_their_header_and_stop() {
        let d = parse("%kind: graph\nA -> B\nC -> D\n");
        assert_eq!(d.kind, Kind::Graph);
        assert!(d.components.is_empty(), "graph bodies belong to shared/nifty-core, not here");
        assert!(d.warnings.is_empty(), "and passing one through is not an error");
    }

    #[test]
    fn malformed_input_warns_rather_than_failing() {
        let d = parse("%kind: hardware\n%nonsense: 3\nchip BROKEN at\ntrace no arrow here\npin STRAY : top\n");
        assert!(d.warnings.len() >= 3, "{:?}", d.warnings);
        assert!(d.warnings.iter().any(|w| w.contains("nonsense")));
        assert!(d.warnings.iter().any(|w| w.contains("STRAY")));
    }

    #[test]
    fn no_kind_directive_is_reported() {
        let d = parse("chip MCU at (1, 1)\n");
        assert!(d.warnings.iter().any(|w| w.contains("no %kind")));
        assert_eq!(d.components.len(), 1, "the body is still read");
    }
}
