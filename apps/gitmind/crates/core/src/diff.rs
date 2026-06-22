// gitmind-core/src/diff.rs
//
// A small, dependency-free line differ and unified-diff engine, so the native
// layer can EXPORT a commit as a git-apply-compatible patch and APPLY a
// unified diff to file content — all without shelling out to git. Tests
// validate the output by round-tripping through real `git apply` (git used
// only as an oracle, never in the code path).
//
// The diff is LCS-based (O(n*m) memory) — fine for normal source files.

/// One line-level edit operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Eq(String),
    Del(String),
    Ins(String),
}

/// Split text into lines, dropping the phantom empty element a trailing
/// newline produces. Returns (lines, ends_with_newline).
fn to_lines(s: &str) -> (Vec<String>, bool) {
    if s.is_empty() {
        return (Vec::new(), false);
    }
    let ends_nl = s.ends_with('\n');
    let mut v: Vec<String> = s.split('\n').map(|l| l.to_string()).collect();
    if ends_nl {
        v.pop(); // remove the empty element after the final newline
    }
    (v, ends_nl)
}

fn from_lines(v: &[String], ends_nl: bool) -> String {
    let mut s = v.join("\n");
    if ends_nl && !v.is_empty() {
        s.push('\n');
    }
    s
}

fn lines(s: &str) -> Vec<String> {
    to_lines(s).0
}

/// LCS-based line diff.
pub fn diff_lines(a: &[String], b: &[String]) -> Vec<Op> {
    let n = a.len();
    let m = b.len();
    // dp[i][j] = LCS length of a[i..], b[j..]
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push(Op::Eq(a[i].clone()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(Op::Del(a[i].clone()));
            i += 1;
        } else {
            ops.push(Op::Ins(b[j].clone()));
            j += 1;
        }
    }
    while i < n {
        ops.push(Op::Del(a[i].clone()));
        i += 1;
    }
    while j < m {
        ops.push(Op::Ins(b[j].clone()));
        j += 1;
    }
    ops
}

// An item with line positions for hunk assembly.
struct Item {
    op: u8, // 0 Eq, 1 Del, 2 Ins
    text: String,
    old_no: usize,
    new_no: usize,
}

/// Produce the unified-diff body (hunks) for two texts. Returns None if equal.
/// `context` is the number of surrounding equal lines per hunk (git uses 3).
pub fn unified_hunks(a_text: &str, b_text: &str, context: usize) -> Option<String> {
    let a = lines(a_text);
    let b = lines(b_text);
    let ops = diff_lines(&a, &b);
    if ops.iter().all(|o| matches!(o, Op::Eq(_))) {
        return None;
    }
    // positionally tagged items
    let mut items = Vec::new();
    let (mut o, mut nn) = (1usize, 1usize);
    for op in &ops {
        match op {
            Op::Eq(t) => {
                items.push(Item { op: 0, text: t.clone(), old_no: o, new_no: nn });
                o += 1;
                nn += 1;
            }
            Op::Del(t) => {
                items.push(Item { op: 1, text: t.clone(), old_no: o, new_no: 0 });
                o += 1;
            }
            Op::Ins(t) => {
                items.push(Item { op: 2, text: t.clone(), old_no: 0, new_no: nn });
                nn += 1;
            }
        }
    }
    let change_idx: Vec<usize> =
        items.iter().enumerate().filter(|(_, it)| it.op != 0).map(|(i, _)| i).collect();
    if change_idx.is_empty() {
        return None;
    }
    // group changes into hunks with `context` lines of padding
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    let mut start = change_idx[0].saturating_sub(context);
    let mut end = (change_idx[0] + context).min(items.len() - 1);
    for &ci in &change_idx[1..] {
        if ci.saturating_sub(context) <= end + 1 {
            end = (ci + context).min(items.len() - 1);
        } else {
            hunks.push((start, end));
            start = ci.saturating_sub(context);
            end = (ci + context).min(items.len() - 1);
        }
    }
    hunks.push((start, end));

    let mut out = String::new();
    for (s, e) in hunks {
        let slice = &items[s..=e];
        let old_len = slice.iter().filter(|it| it.op == 0 || it.op == 1).count();
        let new_len = slice.iter().filter(|it| it.op == 0 || it.op == 2).count();
        let old_start = slice.iter().find(|it| it.op == 0 || it.op == 1).map(|it| it.old_no).unwrap_or(0);
        let new_start = slice.iter().find(|it| it.op == 0 || it.op == 2).map(|it| it.new_no).unwrap_or(0);
        out.push_str(&format!("@@ -{},{} +{},{} @@\n", old_start, old_len, new_start, new_len));
        for it in slice {
            let prefix = match it.op {
                0 => ' ',
                1 => '-',
                _ => '+',
            };
            out.push(prefix);
            out.push_str(&it.text);
            out.push('\n');
        }
    }
    Some(out)
}

/// Build a per-file patch section (git-apply compatible), or None if equal.
/// `old`/`new` are None to denote /dev/null (creation or deletion).
pub fn file_patch(
    path: &str,
    old: Option<&str>,
    new: Option<&str>,
    context: usize,
) -> Option<String> {
    let a = old.unwrap_or("");
    let b = new.unwrap_or("");
    if a == b {
        return None;
    }
    let mut out = String::new();
    out.push_str(&format!("diff --git a/{path} b/{path}\n"));
    match (old, new) {
        (None, Some(_)) => {
            out.push_str("new file mode 100644\n");
            out.push_str("--- /dev/null\n");
            out.push_str(&format!("+++ b/{path}\n"));
        }
        (Some(_), None) => {
            out.push_str("deleted file mode 100644\n");
            out.push_str(&format!("--- a/{path}\n"));
            out.push_str("+++ /dev/null\n");
        }
        _ => {
            out.push_str(&format!("--- a/{path}\n"));
            out.push_str(&format!("+++ b/{path}\n"));
        }
    }
    out.push_str(&unified_hunks(a, b, context)?);
    Some(out)
}

// ---------------------------------------------------------------------------
// Applying a unified diff (strict context match, small offset search)
// ---------------------------------------------------------------------------

/// Apply a single file's hunks (the text after the `+++` line) to `content`.
/// Returns the patched text, or an error if a hunk's context doesn't match.
pub fn apply_hunks(content: &str, hunks: &str) -> Result<String, String> {
    let (src, _src_nl) = to_lines(content);
    let mut out: Vec<String> = Vec::new();
    let mut src_idx = 0usize; // 0-based index into src

    let mut lines_iter = hunks.lines().peekable();
    while let Some(line) = lines_iter.next() {
        if !line.starts_with("@@") {
            continue;
        }
        // parse "@@ -ostart,olen +nstart,nlen @@"
        let nums: Vec<&str> = line.trim_start_matches('@').trim().split(' ').collect();
        let ostart = nums.iter().find(|s| s.starts_with('-'))
            .and_then(|s| s.trim_start_matches('-').split(',').next())
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or("bad hunk header")?;
        // target index for the first line of this hunk
        let target = ostart.saturating_sub(1);
        // collect this hunk's body lines
        let mut body: Vec<String> = Vec::new();
        while let Some(peek) = lines_iter.peek() {
            if peek.starts_with("@@") {
                break;
            }
            body.push(lines_iter.next().unwrap().to_string());
        }
        // find where the hunk's leading context/deletions match, allowing a
        // small offset search around `target`.
        let want: Vec<(char, String)> = body.iter()
            .filter(|l| l.starts_with(' ') || l.starts_with('-') || l.starts_with('+'))
            .map(|l| (l.chars().next().unwrap(), l[1..].to_string()))
            .collect();
        let old_seq: Vec<&String> = want.iter().filter(|(c, _)| *c == ' ' || *c == '-').map(|(_, t)| t).collect();

        let matches_at = |idx: usize| -> bool {
            if idx + old_seq.len() > src.len() {
                return false;
            }
            old_seq.iter().enumerate().all(|(k, t)| &&src[idx + k] == t)
        };
        let mut found = None;
        if matches_at(target) {
            found = Some(target);
        } else {
            for delta in 1..=64usize {
                if target >= delta && matches_at(target - delta) { found = Some(target - delta); break; }
                if matches_at(target + delta) { found = Some(target + delta); break; }
            }
        }
        let at = found.ok_or_else(|| "hunk context did not match".to_string())?;
        // copy unchanged source up to the match point
        while src_idx < at {
            out.push(src[src_idx].clone());
            src_idx += 1;
        }
        // apply the hunk
        for (c, t) in &want {
            match c {
                ' ' => { out.push(t.clone()); src_idx += 1; }
                '-' => { src_idx += 1; }
                '+' => { out.push(t.clone()); }
                _ => {}
            }
        }
    }
    // copy any remaining source
    while src_idx < src.len() {
        out.push(src[src_idx].clone());
        src_idx += 1;
    }
    // Re-terminate with a trailing newline for non-empty results (we assume
    // text files end in newline; "\ No newline at end of file" is not modeled).
    Ok(from_lines(&out, !out.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_modify() {
        let a = "one\ntwo\nthree\nfour\n";
        let b = "one\nTWO\nthree\nfour\nfive\n";
        let hunks = unified_hunks(a, b, 3).unwrap();
        let applied = apply_hunks(a, &hunks).unwrap();
        assert_eq!(applied, b);
    }

    #[test]
    fn no_change_is_none() {
        assert!(unified_hunks("x\ny\n", "x\ny\n", 3).is_none());
    }
}
