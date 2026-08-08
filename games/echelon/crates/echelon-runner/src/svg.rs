//! Battlefield snapshots as standalone SVG — zero dependencies, viewable
//! anywhere. Player 0 is blue, player 1 is coral; alloy teal, flux violet.

use echelon_data::Kind;
use echelon_sim::map::ResKind;
use echelon_sim::World;
use std::io::Write;

const S: i64 = 10; // px per tile

fn pcol(p: u8) -> &'static str {
    if p == 0 {
        "#4da3ff"
    } else {
        "#ff6b52"
    }
}

pub fn write_svg(w: &World, title: &str, path: &str) -> std::io::Result<()> {
    let mut f = std::fs::File::create(path)?;
    let wh = (w.map.w.max(w.map.h) as i64) * S;
    write!(
        f,
        "<svg xmlns='http://www.w3.org/2000/svg' width='{0}' height='{1}' viewBox='0 0 {0} {1}'>",
        wh,
        wh + 24
    )?;
    write!(f, "<rect width='{}' height='{}' fill='#101418'/>", wh, wh + 24)?;
    // terrain (level-shaded, ramps, decks)
    const SHADES: [&str; 6] = [
        "#10151a", "#141a20", "#181f26", "#1d242c", "#222b33", "#28323b",
    ];
    for y in 0..w.map.h {
        for x in 0..w.map.w {
            let fill = if w.map.bridge(x, y) > 0 {
                Some("#3b4037")
            } else if w.map.ramp(x, y).is_some() {
                Some("#26302a")
            } else {
                match w.map.hgt(x, y) {
                    echelon_sim::map::H_VOID => Some("#0b0e12"),
                    lvl => Some(SHADES[(lvl as usize).min(5)]),
                }
            };
            if let Some(fill) = fill {
                write!(
                    f,
                    "<rect x='{}' y='{}' width='{}' height='{}' fill='{}'/>",
                    x as i64 * S,
                    y as i64 * S,
                    S,
                    S,
                    fill
                )?;
            }
        }
    }
    // resources
    for n in &w.map.nodes {
        if n.amount <= 0 {
            continue;
        }
        match n.kind {
            ResKind::Alloy => write!(
                f,
                "<rect x='{}' y='{}' width='8' height='8' fill='#35d0ba'/>",
                n.tx as i64 * S + 1,
                n.ty as i64 * S + 1
            )?,
            ResKind::Flux => write!(
                f,
                "<circle cx='{}' cy='{}' r='4.5' fill='#b06cff'/>",
                n.tx as i64 * S + S / 2,
                n.ty as i64 * S + S / 2
            )?,
        }
    }
    // structures under units
    for e in &w.ents {
        if !e.alive || e.def == u16::MAX {
            continue;
        }
        let d = w.pack.def(e.def);
        if d.kind != Kind::Structure {
            continue;
        }
        let fp = d.fp() as i64;
        let cx = e.pos.x.to_milli() * S / 1000;
        let cy = e.pos.y.to_milli() * S / 1000;
        let (x, y) = (cx - fp * S / 2, cy - fp * S / 2);
        let extra = if e.progress > 0 {
            " stroke-dasharray='4 3' opacity='0.55'"
        } else {
            ""
        };
        write!(
            f,
            "<rect x='{}' y='{}' width='{}' height='{}' rx='2' fill='{}' stroke='#000' stroke-opacity='0.5'{}/>",
            x,
            y,
            fp * S,
            fp * S,
            pcol(e.player),
            extra
        )?;
    }
    // units
    for e in &w.ents {
        if !e.alive || e.def == u16::MAX {
            continue;
        }
        let d = w.pack.def(e.def);
        if d.kind == Kind::Structure {
            continue;
        }
        let cx = e.pos.x.to_milli() * S / 1000;
        let cy = e.pos.y.to_milli() * S / 1000;
        let air = d.mover() == echelon_data::Mover::Air;
        let r = if d.kind == Kind::Worker { 2 } else { 3 };
        let stroke = if air {
            " stroke='#fff' stroke-width='1'"
        } else {
            ""
        };
        write!(
            f,
            "<circle cx='{}' cy='{}' r='{}' fill='{}'{}/>",
            cx,
            cy,
            r,
            pcol(e.player),
            stroke
        )?;
    }
    write!(
        f,
        "<text x='6' y='{}' font-family='monospace' font-size='13' fill='#c8d0da'>{}</text>",
        wh + 17,
        title
    )?;
    write!(f, "</svg>")?;
    Ok(())
}
