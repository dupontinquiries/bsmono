//! Render a [`Grid`] to the plain-text form the VT corpus fixtures are written
//! in, so the crate and its reference emulators can be diffed line for line.
//!
//! This lives in the library rather than in the test because two callers need
//! the exact same bytes — `tests/vt_corpus.rs`, which compares against the
//! checked-in `.expect` files, and `examples/vtdump.rs`, which prints a capture
//! while someone is looking at it. A second copy of the format is a second
//! thing to get subtly wrong, and a formatting difference between them would
//! read as a parser bug.
//!
//! The format is described in `tools/render_ref.py`, which produces the other
//! side of the comparison.

use crate::{Grid, Style, TermColor};

/// A colour as the PROTOCOL names it, not as the theme paints it.
///
/// The palette this crate draws with is a theming choice and is allowed to
/// differ from any reference emulator's. What has to agree is which colour the
/// program running in the terminal asked for.
fn color(c: TermColor) -> String {
    match c {
        TermColor::Default => "default".to_string(),
        TermColor::Ansi(i) => format!("i{i}"),
    }
}

fn style_key(s: Style) -> String {
    let mut attrs = String::new();
    if s.bold {
        attrs.push('b');
    }
    if s.inverse {
        attrs.push('r');
    }
    if attrs.is_empty() {
        attrs.push('-');
    }
    format!("{}/{}/{}", color(s.fg), color(s.bg), attrs)
}

/// The grid as corpus text: header, then a text line and a style-run line per
/// row.
///
/// Both lines are needed. Text alone reports OK for an SGR bug — every
/// character in the right cell and the wrong colour — which is precisely what
/// happened to the 256-colour capture before the style half existed.
pub fn dump(grid: &Grid) -> String {
    let mut out = String::new();
    out.push_str(&format!("cols {}\n", grid.cols));
    out.push_str(&format!("rows {}\n", grid.rows));
    out.push_str(&format!("cursor {} {}\n", grid.cur_row, grid.cur_col));
    out.push('\n');
    for r in 0..grid.rows {
        out.push_str(&format!("{:2}|{}\n", r, grid.row_text(r)));
        // The runs cover the whole row including the trailing blanks the text
        // line trims: a background colour painted past the last character is
        // visible on screen, and painting it with the wrong pen is exactly what
        // an erase bug does.
        let mut runs: Vec<(usize, usize, String)> = Vec::new();
        for c in 0..grid.cols {
            let k = style_key(grid.cell(r, c).style);
            match runs.last_mut() {
                Some((_, n, prev)) if *prev == k => *n += 1,
                _ => runs.push((c, 1, k)),
            }
        }
        let text: Vec<String> = runs.iter().map(|(c, n, k)| format!("{c}:{n}:{k}")).collect();
        out.push_str(&format!("{:2}#{}\n", r, text.join(" ")));
    }
    out
}
