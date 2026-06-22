// gitmind-core/src/select.rs
//
// The selection grammar behind `-f`. A list command prints rows with short
// codes (a0, a1, … b0, …), then you type a selection: a single code, a
// range between two codes, several comma-separated, or `*` for all. This
// module is the pure parser; the interactive picker (fzf pipe or inline)
// lives in the CLI and feeds strings here.
//
//   c3            one row
//   a3..b1        inclusive range a3 → b1 (order-independent)
//   a3-b1         same, dash form
//   a0,c3,e2      several
//   *             everything
//   3             plain numeric index also accepted

/// Two-char code for row `i`: letter (a–z) by tens, digit by ones. Rolls
/// over to a numeric code past 260 rows.
pub fn code_for(i: usize) -> String {
    if i < 260 {
        let letter = (b'a' + (i / 10) as u8) as char;
        let digit = (i % 10) as u8;
        format!("{letter}{digit}")
    } else {
        i.to_string()
    }
}

/// Resolve a single code (or numeric index) to a row index.
pub fn index_for(code: &str, n: usize) -> Result<usize, String> {
    let c = code.trim();
    if c.is_empty() {
        return Err("empty selection token".into());
    }
    // Plain number?
    if let Ok(num) = c.parse::<usize>() {
        return if num < n { Ok(num) } else { Err(format!("{num} out of range (0..{n})")) };
    }
    // Two-char code: letter + digit.
    let bytes = c.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_lowercase() && bytes[1].is_ascii_digit() {
        let idx = (bytes[0] - b'a') as usize * 10 + (bytes[1] - b'0') as usize;
        return if idx < n { Ok(idx) } else { Err(format!("{c} out of range")) };
    }
    Err(format!("bad selection token: {c}"))
}

/// Parse a full selection string against a list of length `n`. Returns
/// sorted, de-duplicated indices.
pub fn parse_selection(input: &str, n: usize) -> Result<Vec<usize>, String> {
    let input = input.trim();
    if input == "*" {
        return Ok((0..n).collect());
    }
    let mut out = std::collections::BTreeSet::new();
    for tok in input.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        // Range form X..Y or X-Y (dash only when it's clearly a separator
        // between two valid codes, never inside a token).
        let range = tok
            .split_once("..")
            .or_else(|| split_dash_range(tok));
        if let Some((a, b)) = range {
            let ia = index_for(a, n)?;
            let ib = index_for(b, n)?;
            let (lo, hi) = if ia <= ib { (ia, ib) } else { (ib, ia) };
            for i in lo..=hi {
                out.insert(i);
            }
        } else {
            out.insert(index_for(tok, n)?);
        }
    }
    if out.is_empty() {
        return Err("no rows selected".into());
    }
    Ok(out.into_iter().collect())
}

/// Split `X-Y` only when both sides look like codes/numbers, so a token
/// like `a3` isn't mistaken for a range.
fn split_dash_range(tok: &str) -> Option<(&str, &str)> {
    let idx = tok.find('-')?;
    let (a, b) = (&tok[..idx], &tok[idx + 1..]);
    if a.is_empty() || b.is_empty() {
        return None;
    }
    Some((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_roundtrip() {
        assert_eq!(code_for(0), "a0");
        assert_eq!(code_for(13), "b3");
        assert_eq!(index_for("b3", 100).unwrap(), 13);
        assert_eq!(index_for("0", 100).unwrap(), 0);
    }

    #[test]
    fn single_and_multi() {
        assert_eq!(parse_selection("c3", 100).unwrap(), vec![23]);
        assert_eq!(parse_selection("a0,a2,a4", 100).unwrap(), vec![0, 2, 4]);
    }

    #[test]
    fn ranges_both_forms_and_order_independent() {
        assert_eq!(parse_selection("a3..b1", 100).unwrap(), vec![3, 4, 5, 6, 7, 8, 9, 10, 11]);
        assert_eq!(parse_selection("b1-a3", 100).unwrap(), vec![3, 4, 5, 6, 7, 8, 9, 10, 11]);
    }

    #[test]
    fn star_selects_all() {
        assert_eq!(parse_selection("*", 3).unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn rejects_bad_and_out_of_range() {
        assert!(parse_selection("zz", 5).is_err());
        assert!(parse_selection("a9", 5).is_err()); // index 9 >= 5
        assert!(parse_selection("", 5).is_err());
    }
}
