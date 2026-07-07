//! menu_layout.rs — pure menu / world-creation layout solver.
//!
//! Lays out a vertical stack of UI elements (buttons / panels / fields) into a
//! horizontally-centred column with consistent spacing, responsive to the screen
//! size. Pure integer math, deterministic, panic-free (R1/R11/R21). Same spirit
//! as `viewport_layout`; the renderer/egui side consumes these rects.

/// Integer pixel rectangle (menu-local screen space).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// A UI element's minimum size request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Element {
    pub min_w: u32,
    pub min_h: u32,
}

/// Spacing / sizing style for the column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MenuStyle {
    /// Empty border kept around the whole menu, in pixels.
    pub margin: u32,
    /// Vertical gap between consecutive elements, in pixels.
    pub gap: u32,
    /// Preferred column width; clamped to the available width.
    pub max_width: u32,
}

impl Default for MenuStyle {
    fn default() -> Self {
        MenuStyle { margin: 40, gap: 16, max_width: 480 }
    }
}

/// Lay out `elements` as a centred vertical column on a `screen_w` x `screen_h`
/// screen. The column is horizontally centred at a width of
/// `min(style.max_width, screen - 2*margin)`; the stack is vertically centred
/// when it fits, otherwise top-aligned at the margin. Consecutive elements are
/// separated by exactly `style.gap`. Returns one rect per element (empty input
/// yields an empty vec).
///
/// For inputs that fit (the menu is designed for a screen large enough to show
/// it), every rect lies within the screen and no two overlap.
pub fn layout_menu(
    screen_w: u32,
    screen_h: u32,
    elements: &[Element],
    style: MenuStyle,
) -> Vec<Rect> {
    let n = elements.len();
    if n == 0 {
        return Vec::new();
    }
    let margin = style.margin;
    let gap = style.gap;

    let avail_w = screen_w.saturating_sub(2 * margin).max(1);
    let content_w = style.max_width.min(avail_w).max(1);
    let x = margin + (avail_w - content_w) / 2;

    let sum_h: u32 = elements.iter().map(|e| e.min_h).sum();
    let total_h = sum_h + gap.saturating_mul((n as u32).saturating_sub(1));
    let avail_h = screen_h.saturating_sub(2 * margin);
    let start_y = if total_h <= avail_h {
        margin + (avail_h - total_h) / 2
    } else {
        margin
    };

    let mut rects = Vec::with_capacity(n);
    let mut y = start_y;
    for e in elements {
        rects.push(Rect { x, y, w: content_w, h: e.min_h });
        y = y.saturating_add(e.min_h).saturating_add(gap);
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREENS: &[(u32, u32)] = &[
        (800, 600),
        (1280, 720),
        (1920, 1080),
        (2560, 1440),
        (3840, 2160),
        (1080, 1920), // portrait
    ];

    fn sample_menu() -> Vec<Element> {
        // A "create world" form: title panel + 4 fields + 2 buttons.
        vec![
            Element { min_w: 200, min_h: 64 },
            Element { min_w: 200, min_h: 40 },
            Element { min_w: 200, min_h: 40 },
            Element { min_w: 200, min_h: 40 },
            Element { min_w: 200, min_h: 40 },
            Element { min_w: 200, min_h: 48 },
            Element { min_w: 200, min_h: 48 },
        ]
    }

    #[test]
    fn prop_within_screen() {
        let style = MenuStyle::default();
        let els = sample_menu();
        for &(w, h) in SCREENS {
            for k in 1..=els.len() {
                let rects = layout_menu(w, h, &els[..k], style);
                assert_eq!(rects.len(), k);
                for r in &rects {
                    assert!(r.w > 0 && r.h > 0, "degenerate {}x{} k={}", w, h, k);
                    assert!(r.x + r.w <= w, "x overflow {}x{} k={}", w, h, k);
                    assert!(r.y + r.h <= h, "y overflow {}x{} k={}", w, h, k);
                }
            }
        }
    }

    #[test]
    fn prop_no_overlap() {
        let style = MenuStyle::default();
        let els = sample_menu();
        for &(w, h) in SCREENS {
            let rects = layout_menu(w, h, &els, style);
            for i in 1..rects.len() {
                let prev_bottom = rects[i - 1].y + rects[i - 1].h;
                assert!(rects[i].y >= prev_bottom, "overlap at {} on {}x{}", i, w, h);
            }
        }
    }

    #[test]
    fn prop_spacing_consistent() {
        let style = MenuStyle::default();
        let els = sample_menu();
        for &(w, h) in SCREENS {
            let rects = layout_menu(w, h, &els, style);
            for i in 1..rects.len() {
                let g = rects[i].y - (rects[i - 1].y + rects[i - 1].h);
                assert_eq!(g, style.gap, "gap drift at {} on {}x{}", i, w, h);
            }
        }
    }

    #[test]
    fn prop_centred_column_same_x_and_width() {
        let style = MenuStyle::default();
        let els = sample_menu();
        for &(w, h) in SCREENS {
            let rects = layout_menu(w, h, &els, style);
            let x0 = rects[0].x;
            let w0 = rects[0].w;
            for r in &rects {
                assert_eq!(r.x, x0);
                assert_eq!(r.w, w0);
            }
            // horizontally centred: left margin ~= right margin (±1px rounding).
            let left = x0;
            let right = w.saturating_sub(x0 + w0);
            assert!((left as i64 - right as i64).abs() <= 1, "not centred on {}x{}", w, h);
        }
    }

    #[test]
    fn prop_deterministic() {
        let style = MenuStyle::default();
        let els = sample_menu();
        for &(w, h) in SCREENS {
            assert_eq!(layout_menu(w, h, &els, style), layout_menu(w, h, &els, style));
        }
    }

    #[test]
    fn empty_input_yields_empty() {
        assert!(layout_menu(1920, 1080, &[], MenuStyle::default()).is_empty());
    }
}
