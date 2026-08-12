//! `.ink` v1 — the portfolio's stroke interchange/corpus format.
//!
//! Line-based text, dependency-free to parse anywhere (the finetuning tools,
//! a Python notebook, `awk`). One drawing per file:
//!
//! ```text
//! ink 1
//! # optional comment lines anywhere
//! stroke tool=pen width=3 color=141418ff
//! p 12.5 -3.25 0.80 0
//! p 13.1 -3.10 0.85 8
//! stroke tool=finger width=3 color=141418ff
//! p 0 0 0.5 0
//! ```
//!
//! `p x y pressure t_ms` — world coords full f64 precision (`{}` formatting,
//! shortest round-trip), pressure 2dp, time ms as written. Unknown `key=val`
//! attrs on `stroke` lines are ignored (forward compat, same permissive
//! stance as niftyplot's parser). Stroke ids are not stored — they're
//! session-local; the loader re-allocates.

use crate::stroke::{InkPoint, Stroke, StrokeId, StrokeStyle, ToolKind};
use crate::Vec2d;

pub const FORMAT_VERSION: u32 = 1;

pub fn emit(strokes: &[Stroke]) -> String {
    let mut out = format!("ink {}\n", FORMAT_VERSION);
    for s in strokes {
        let c = s.style.color;
        out.push_str(&format!(
            "stroke tool={} width={} color={:02x}{:02x}{:02x}{:02x}\n",
            s.style.tool.as_str(),
            s.style.width,
            c[0],
            c[1],
            c[2],
            c[3]
        ));
        for p in &s.points {
            out.push_str(&format!(
                "p {} {} {:.2} {}\n",
                p.pos.x, p.pos.y, p.pressure, p.t_ms
            ));
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, ".ink parse error at line {}: {}", self.line, self.msg)
    }
}
impl std::error::Error for ParseError {}

/// Parse `.ink` text. Ids are assigned 1..n in file order.
pub fn parse(text: &str) -> Result<Vec<Stroke>, ParseError> {
    let mut lines = text.lines().enumerate();
    // header
    let (hno, header) = loop {
        match lines.next() {
            None => {
                return Err(ParseError {
                    line: 0,
                    msg: "empty file".into(),
                })
            }
            Some((i, l)) if l.trim().is_empty() || l.trim_start().starts_with('#') => {
                let _ = i;
                continue;
            }
            Some((i, l)) => break (i + 1, l.trim()),
        }
    };
    let ver: u32 = header
        .strip_prefix("ink ")
        .and_then(|v| v.trim().parse().ok())
        .ok_or(ParseError {
            line: hno,
            msg: format!("expected `ink <version>` header, got {:?}", header),
        })?;
    if ver > FORMAT_VERSION {
        return Err(ParseError {
            line: hno,
            msg: format!("format version {} is newer than supported {}", ver, FORMAT_VERSION),
        });
    }

    let mut strokes: Vec<Stroke> = Vec::new();
    let mut next_id = 1u64;
    for (i, raw) in lines {
        let line_no = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("stroke") {
            let mut style = StrokeStyle::default();
            for kv in rest.split_whitespace() {
                if let Some((k, v)) = kv.split_once('=') {
                    match k {
                        "tool" => {
                            style.tool = ToolKind::parse(v).ok_or(ParseError {
                                line: line_no,
                                msg: format!("unknown tool {:?}", v),
                            })?
                        }
                        "width" => {
                            style.width = v.parse().map_err(|_| ParseError {
                                line: line_no,
                                msg: format!("bad width {:?}", v),
                            })?
                        }
                        "color" => {
                            if v.len() != 8 {
                                return Err(ParseError {
                                    line: line_no,
                                    msg: format!("color must be rrggbbaa, got {:?}", v),
                                });
                            }
                            let mut c = [0u8; 4];
                            for (j, ch) in c.iter_mut().enumerate() {
                                *ch = u8::from_str_radix(&v[j * 2..j * 2 + 2], 16).map_err(
                                    |_| ParseError {
                                        line: line_no,
                                        msg: format!("bad color {:?}", v),
                                    },
                                )?;
                            }
                            style.color = c;
                        }
                        _ => {} // forward compat: ignore unknown attrs
                    }
                }
            }
            strokes.push(Stroke::new(StrokeId(next_id), style));
            next_id += 1;
        } else if let Some(rest) = line.strip_prefix("p ") {
            let cur = strokes.last_mut().ok_or(ParseError {
                line: line_no,
                msg: "point before any `stroke` line".into(),
            })?;
            let mut it = rest.split_whitespace();
            let mut num = |name: &str| -> Result<f64, ParseError> {
                it.next()
                    .and_then(|t| t.parse().ok())
                    .ok_or(ParseError {
                        line: line_no,
                        msg: format!("bad or missing {} in point line", name),
                    })
            };
            let x = num("x")?;
            let y = num("y")?;
            let pressure = num("pressure")? as f32;
            let t_ms = num("t_ms")?;
            cur.points.push(InkPoint {
                pos: Vec2d::new(x, y),
                pressure,
                t_ms,
            });
        } else {
            return Err(ParseError {
                line: line_no,
                msg: format!("unrecognized line {:?}", line),
            });
        }
    }
    Ok(strokes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut a = Stroke::new(
            StrokeId(1),
            StrokeStyle {
                width: 2.5,
                color: [10, 20, 30, 255],
                tool: ToolKind::Pen,
            },
        );
        a.points.push(InkPoint {
            pos: Vec2d::new(1.5, -2.25),
            pressure: 0.8,
            t_ms: 0.0,
        });
        a.points.push(InkPoint {
            pos: Vec2d::new(1e6 + 0.125, 3.0),
            pressure: 0.75,
            t_ms: 16.0,
        });
        let mut b = Stroke::new(StrokeId(2), StrokeStyle::default());
        b.points.push(InkPoint::xy(0.0, 0.0));

        let text = emit(&[a.clone(), b.clone()]);
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].style, a.style);
        assert_eq!(parsed[0].points.len(), 2);
        assert!(parsed[0].points[1].pos.dist(a.points[1].pos) < 1e-9);
        assert!((parsed[0].points[1].pressure - 0.75).abs() < 1e-3);
        assert_eq!(parsed[1].points.len(), 1);
    }

    #[test]
    fn comments_and_unknown_attrs_tolerated() {
        let text = "# a corpus file\nink 1\nstroke tool=finger width=3 color=00000000 future=thing\np 1 2 0.50 0\n";
        let s = parse(text).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].points.len(), 1);
    }

    #[test]
    fn errors_carry_line_numbers() {
        let e = parse("ink 1\nbogus line\n").unwrap_err();
        assert_eq!(e.line, 2);
        let e2 = parse("ink 99\n").unwrap_err();
        assert!(e2.msg.contains("newer"));
        let e3 = parse("ink 1\np 1 2 0.5 0\n").unwrap_err();
        assert!(e3.msg.contains("before any"));
    }
}
