// src/layout_detect.rs — zero-alloc layout inference
#[derive(Clone, Debug, PartialEq)]
pub enum LayoutKind { Column, Row, RowWrap, Grid, Inline }

pub fn detect_layout(tag: &str, style: Option<&str>, classes: Option<&str>) -> LayoutKind {
    match tag {
        "span"|"em"|"strong"|"b"|"i"|"small"|"a"|"code"|"abbr"|"cite"|"sub"|"sup"|"time"|"label" => return LayoutKind::Inline,
        "nav"|"header"|"footer"|"menu" => {
            if let Some(s) = style { if s.to_ascii_lowercase().contains("flex-direction") && s.to_ascii_lowercase().contains("column") { return LayoutKind::Column; } }
            return LayoutKind::Row;
        }
        _ => {}
    }
    if let Some(st) = style {
        let s = st.to_ascii_lowercase();
        if s.contains("display") {
            if s.contains("grid") { return LayoutKind::Grid; }
            if s.contains("flex") || s.contains("inline-flex") {
                if s.contains("flex-direction") && s.contains("column") { return LayoutKind::Column; }
                return if s.contains("flex-wrap") && s.contains("wrap") { LayoutKind::RowWrap } else { LayoutKind::Row };
            }
            if s.contains("inline-block") { return LayoutKind::Row; }
        }
        if s.contains("float") && (s.contains("left") || s.contains("right")) { return LayoutKind::Row; }
    }
    if let Some(cls) = classes {
        for w in cls.split_ascii_whitespace() {
            let wl = w.to_ascii_lowercase();
            match wl.as_str() {
                "flex"|"d-flex"|"inline-flex"|"flex-row"|"hstack"|"row"|"items-center"|"justify-between"|"justify-center" => {
                    // Check if column override exists
                    if cls.split_ascii_whitespace().any(|x| { let x = x.to_ascii_lowercase(); x == "flex-col" || x == "flex-column" || x == "vstack" }) { return LayoutKind::Column; }
                    return if cls.split_ascii_whitespace().any(|x| x.to_ascii_lowercase() == "flex-wrap") { LayoutKind::RowWrap } else { LayoutKind::Row };
                }
                "d-grid" => return LayoutKind::Grid,
                "toolbar"|"breadcrumb"|"pagination"|"btn-group"|"tabs"|"nav-links" => return LayoutKind::Row,
                "gallery"|"card-grid"|"masonry"|"image-gallery" => return LayoutKind::RowWrap,
                _ => {
                    // v0.1.467: `grid-cols-*` (Tailwind) declares a
                    // grid CONTAINER → Grid. But `col-*` (Bootstrap:
                    // col-6, col-md-4, …) is a grid CELL — an ITEM
                    // inside a .row, NOT a container. The prior code
                    // matched `col-` too, so every Bootstrap column
                    // was rendered as a 2-col Grid laying out ITS
                    // children in a grid — wrong, and a contributor
                    // to the "items appear off the right of the
                    // screen" overflow (a col-6 wrapping normal
                    // stacked content became a grid). Bootstrap cells
                    // now fall through to the Column default, which
                    // is the correct shape for a content wrapper.
                    if wl.starts_with("grid-cols") { return LayoutKind::Grid; }
                }
            }
        }
    }
    LayoutKind::Column
}

#[inline]
pub fn qml_container(k: &LayoutKind) -> &'static str {
    match k { LayoutKind::Column=>"Column", LayoutKind::Row=>"Row", LayoutKind::RowWrap=>"Flow", LayoutKind::Grid=>"Grid", LayoutKind::Inline=>"Row" }
}

/// Returns static str — zero allocation
#[inline]
pub fn qml_layout_props(k: &LayoutKind, w: &str) -> String {
    // Use write! to single buffer instead of format!
    let mut o = String::with_capacity(48);
    match k {
        LayoutKind::Column => { o.push_str("width: "); o.push_str(w); o.push_str("\nspacing: 4"); }
        LayoutKind::Row => { o.push_str("width: "); o.push_str(w); o.push_str("\nspacing: 6"); }
        LayoutKind::RowWrap => { o.push_str("width: "); o.push_str(w); o.push_str("\nspacing: 6\nflow: Flow.LeftToRight"); }
        LayoutKind::Grid => { o.push_str("width: "); o.push_str(w); o.push_str("\ncolumns: 2\nspacing: 6"); }
        LayoutKind::Inline => { o.push_str("spacing: 4"); }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_tags() {
        for t in ["span", "em", "strong", "a", "code", "time", "label"] {
            assert_eq!(detect_layout(t, None, None), LayoutKind::Inline,
                "tag {} should be Inline", t);
        }
    }

    #[test]
    fn nav_defaults_row() {
        assert_eq!(detect_layout("nav", None, None), LayoutKind::Row);
        assert_eq!(detect_layout("header", None, None), LayoutKind::Row);
        assert_eq!(detect_layout("footer", None, None), LayoutKind::Row);
    }

    #[test]
    fn nav_column_override() {
        assert_eq!(
            detect_layout("nav", Some("display:flex; flex-direction: column"), None),
            LayoutKind::Column);
    }

    #[test]
    fn style_flex_row_default() {
        assert_eq!(detect_layout("div", Some("display: flex"), None), LayoutKind::Row);
    }

    #[test]
    fn style_flex_column() {
        assert_eq!(
            detect_layout("div", Some("display:flex; flex-direction:column"), None),
            LayoutKind::Column);
    }

    #[test]
    fn style_flex_wrap() {
        assert_eq!(
            detect_layout("div", Some("display:flex; flex-wrap: wrap"), None),
            LayoutKind::RowWrap);
    }

    #[test]
    fn style_grid() {
        assert_eq!(detect_layout("div", Some("display: grid"), None), LayoutKind::Grid);
    }

    #[test]
    fn style_inline_block_is_row() {
        assert_eq!(detect_layout("div", Some("display: inline-block"), None), LayoutKind::Row);
    }

    #[test]
    fn style_float_is_row() {
        assert_eq!(detect_layout("div", Some("float: left"), None), LayoutKind::Row);
    }

    #[test]
    fn class_flex_row() {
        assert_eq!(detect_layout("div", None, Some("flex")), LayoutKind::Row);
    }

    #[test]
    fn class_flex_col_override() {
        assert_eq!(detect_layout("div", None, Some("flex flex-col")), LayoutKind::Column);
        assert_eq!(detect_layout("div", None, Some("d-flex vstack")), LayoutKind::Column);
    }

    #[test]
    fn class_flex_wrap() {
        assert_eq!(detect_layout("div", None, Some("flex flex-wrap")), LayoutKind::RowWrap);
    }

    #[test]
    fn class_grid_variants() {
        assert_eq!(detect_layout("div", None, Some("d-grid")), LayoutKind::Grid);
        assert_eq!(detect_layout("div", None, Some("grid grid-cols-3")), LayoutKind::Grid);
    }

    #[test]
    fn class_gallery_is_wrap() {
        assert_eq!(detect_layout("div", None, Some("gallery")), LayoutKind::RowWrap);
        assert_eq!(detect_layout("div", None, Some("card-grid")), LayoutKind::RowWrap);
    }

    #[test]
    fn class_toolbar_is_row() {
        assert_eq!(detect_layout("div", None, Some("toolbar")), LayoutKind::Row);
        assert_eq!(detect_layout("ul", None, Some("pagination")), LayoutKind::Row);
    }

    #[test]
    fn plain_div_is_column() {
        assert_eq!(detect_layout("div", None, None), LayoutKind::Column);
        assert_eq!(detect_layout("section", None, None), LayoutKind::Column);
    }

    #[test]
    fn bootstrap_col_is_not_grid() {
        // v0.1.467: col-* are grid CELLS (items inside a .row), not
        // grid CONTAINERS. They must NOT classify as Grid — that
        // would lay out the cell's own children in a grid (a cause
        // of off-screen overflow). They fall through to Column.
        for c in ["col-6", "col-md-4", "col-lg-12", "col"] {
            let k = detect_layout("div", None, Some(c));
            assert_ne!(k, LayoutKind::Grid,
                "{} should NOT be a Grid container, got {:?}", c, k);
        }
        // grid-cols-* (the real container declaration) is still Grid.
        assert_eq!(detect_layout("div", None, Some("grid-cols-2")), LayoutKind::Grid);
    }

    #[test]
    fn container_mapping() {
        assert_eq!(qml_container(&LayoutKind::Column), "Column");
        assert_eq!(qml_container(&LayoutKind::Row), "Row");
        assert_eq!(qml_container(&LayoutKind::RowWrap), "Flow");
        assert_eq!(qml_container(&LayoutKind::Grid), "Grid");
        assert_eq!(qml_container(&LayoutKind::Inline), "Row");
    }

    #[test]
    fn layout_props_include_width() {
        let p = qml_layout_props(&LayoutKind::Column, "100");
        assert!(p.contains("width: 100"));
        // Inline omits explicit width (uses implicitWidth upstream).
        let pi = qml_layout_props(&LayoutKind::Inline, "100");
        assert!(!pi.contains("width: 100"));
    }
}
