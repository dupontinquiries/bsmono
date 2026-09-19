// source — the lines around a stop, as data first and as a LayoutJob second.
//
// Wrapped, never horizontally scrolled: on the Stack page a sideways drag walks
// the frames, and a sideways-scrolling child would take that drag for itself.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

/// (line number, text, is the current line) for `line` +/- `context`, clamped
/// to the file. Empty when `line` is past the end: a stale binary whose source
/// has since shrunk should show nothing rather than the wrong lines.
pub fn window(lines: &[String], line: u32, context: u32) -> Vec<(u32, &str, bool)> {
    if line == 0 || line as usize > lines.len() {
        return Vec::new();
    }
    let lo = line.saturating_sub(context).max(1);
    let hi = (line + context).min(lines.len() as u32);
    (lo..=hi).map(|n| (n, lines[n as usize - 1].as_str(), n == line)).collect()
}

pub struct Tints {
    pub text: Color32,
    pub weak: Color32,
    pub current_bg: Color32,
}

pub fn layout(lines: &[String], line: u32, context: u32, tints: &Tints, size: f32, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    let font = FontId::monospace(size);
    let win = window(lines, line, context);
    let digits = win.last().map_or(1, |(n, _, _)| n.to_string().len());
    for (n, text, current) in win {
        let background = if current { tints.current_bg } else { Color32::TRANSPARENT };
        let gutter = format!("{n:>digits$} {} ", if current { '>' } else { ' ' });
        job.append(&gutter, 0.0, TextFormat { font_id: font.clone(), color: tints.weak, background, ..Default::default() });
        job.append(text, 0.0, TextFormat { font_id: font.clone(), color: tints.text, background, ..Default::default() });
        job.append("\n", 0.0, TextFormat { font_id: font.clone(), color: tints.text, ..Default::default() });
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    #[test]
    fn window_clamps_at_both_ends_and_marks_exactly_one_line() {
        let f = file(10);
        let w = window(&f, 2, 3);
        assert_eq!(w.iter().map(|l| l.0).collect::<Vec<_>>(), [1, 2, 3, 4, 5]);
        let w = window(&f, 10, 3);
        assert_eq!(w.iter().map(|l| l.0).collect::<Vec<_>>(), [7, 8, 9, 10]);
        for line in 1..=10 {
            let w = window(&f, line, 3);
            assert_eq!(w.iter().filter(|l| l.2).count(), 1);
            assert_eq!(w.iter().find(|l| l.2).unwrap().1, format!("line {line}"));
        }
    }

    #[test]
    fn a_line_the_file_does_not_have_shows_nothing() {
        assert!(window(&file(10), 11, 3).is_empty());
        assert!(window(&file(10), 0, 3).is_empty());
        assert!(window(&[], 1, 3).is_empty());
    }

    #[test]
    fn layout_carries_the_gutter_and_highlights_only_the_current_line() {
        let f = file(120);
        let t = Tints { text: Color32::WHITE, weak: Color32::GRAY, current_bg: Color32::RED };
        let job = layout(&f, 100, 1, &t, 12.0, 300.0);
        assert!(job.text.contains(" 99   line 99\n"));
        assert!(job.text.contains("100 > line 100\n"));
        let lit: String = job.sections.iter().filter(|s| s.format.background == Color32::RED).map(|s| &job.text[s.byte_range.clone()]).collect();
        assert_eq!(lit, "100 > line 100");
    }
}
