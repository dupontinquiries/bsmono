#![allow(dead_code)]
/// charts.rs — SVG chart generation via plotters
///
/// All charts use Adwaita semantic colours via hardcoded fallbacks that
/// match what Adwaita resolves to — the SVGs are static files so they
/// can't read CSS variables at runtime. For full theme-following the
/// charts would need to be redrawn when the theme changes; that's a
/// future enhancement. For now we use neutral greys + accent blue that
/// look acceptable in both light and dark Adwaita.

use plotters::prelude::*;
use std::path::{Path, PathBuf};

// These match Adwaita's defaults closely enough to look intentional.
// They are NOT Catppuccin — no strong colour opinions.
const BG:      RGBColor = RGBColor(0x24, 0x24, 0x24); // close to Adwaita dark view bg
const SURFACE: RGBColor = RGBColor(0x30, 0x30, 0x30);
const ACCENT:  RGBColor = RGBColor(0x35, 0x84, 0xe4); // Adwaita default blue accent
const ACCENT2: RGBColor = RGBColor(0xe5, 0xa5, 0x0a); // amber — repeated blocks
const TEXT:    RGBColor = RGBColor(0xff, 0xff, 0xff);
const MUTED:   RGBColor = RGBColor(0x77, 0x77, 0x77);
const HIGHLIGHT: RGBColor = RGBColor(0xff, 0xff, 0x44); // yellow — selected token bar

// ── public API ────────────────────────────────────────────────────────────────

/// Return the CSS hex color string (`#rrggbb`) for bar index `i` out of `total`.
/// Used by the token list to paint a matching left-accent on each row.
pub fn bar_color_css(i: usize, total: usize, highlighted: bool) -> String {
    if highlighted {
        return "#ffff44".to_owned();
    }
    let c = blend_gradient(if total > 1 { i as f64 / (total - 1) as f64 } else { 0.0 });
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

/// Standard token frequency bar chart.
pub fn token_frequency_chart(
    freqs: &[(String, u64)],
    out_dir: &Path,
) -> std::io::Result<PathBuf> {
    token_frequency_chart_highlighted(freqs, None, out_dir)
}

/// Token frequency chart with one bar highlighted.
/// `highlighted` is the index into `freqs` of the selected token.
pub fn token_frequency_chart_highlighted(
    freqs: &[(String, u64)],
    highlighted: Option<usize>,
    out_dir: &Path,
) -> std::io::Result<PathBuf> {
    let path = out_dir.join("token_freq.svg");
    draw_token_freq(freqs, highlighted, &path)?;
    Ok(path)
}

/// Block repeat histogram.
pub fn block_histogram_chart(
    histogram: &[(u64, u64)],
    out_dir: &Path,
) -> std::io::Result<PathBuf> {
    let path = out_dir.join("block_hist.svg");
    draw_block_histogram(histogram, &path)?;
    Ok(path)
}

/// Horizontal progress bar.
pub fn progress_chart(done: u64, total: u64, out_dir: &Path) -> std::io::Result<PathBuf> {
    let path = out_dir.join("progress.svg");
    draw_progress(done, total, &path)?;
    Ok(path)
}

// ── renderers ─────────────────────────────────────────────────────────────────

fn draw_token_freq(
    freqs: &[(String, u64)],
    highlighted: Option<usize>,
    path: &Path,
) -> std::io::Result<()> {
    if freqs.is_empty() { return Ok(()); }

    // Compact — chart lives above the list, not beside it.
    // No x-axis labels: token names are already in the list rows below.
    let width  = 600u32;
    let height = 200u32;
    let root = SVGBackend::new(path, (width, height)).into_drawing_area();
    root.fill(&BG).map_err(pe)?;

    let n = freqs.len().min(30);
    let max_count = freqs.iter().take(n).map(|(_, c)| *c).max().unwrap_or(1) as f64;

    let mut chart = ChartBuilder::on(&root)
        .margin(8)
        .x_label_area_size(0)   // no x labels — token names in list below
        .y_label_area_size(52)
        .build_cartesian_2d(0i32..n as i32, 0f64..max_count * 1.12)
        .map_err(pe)?;

    chart
        .configure_mesh()
        .disable_x_mesh()
        .bold_line_style(MUTED.mix(0.20))
        .light_line_style(TRANSPARENT)
        .axis_style(MUTED)
        .label_style(("monospace", 10).into_font().color(&TEXT))
        .y_labels(4)
        .draw()
        .map_err(pe)?;

    chart
        .draw_series((0..n).map(|i| {
            let count  = freqs[i].1 as f64;
            let colour = if Some(i) == highlighted {
                HIGHLIGHT
            } else {
                blend_gradient(if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 })
            };
            Rectangle::new(
                [(i as i32, 0.0), (i as i32 + 1, count)],
                colour.mix(0.88).filled(),
            )
        }))
        .map_err(pe)?;

    root.present().map_err(pe)?;
    Ok(())
}

fn draw_block_histogram(histogram: &[(u64, u64)], path: &Path) -> std::io::Result<()> {
    if histogram.is_empty() { return Ok(()); }

    let width  = 600u32;
    let height = 400u32;
    let root = SVGBackend::new(path, (width, height)).into_drawing_area();
    root.fill(&BG).map_err(pe)?;

    let top: Vec<_> = histogram.iter().take(40).collect();
    let n = top.len();
    let max_count = top.iter().map(|(_, c)| *c).max().unwrap_or(1) as f64;

    let mut chart = ChartBuilder::on(&root)
        .caption("Block repeat frequency (top 40)", ("sans-serif", 18).into_font().color(&TEXT))
        .margin(16)
        .x_label_area_size(30)
        .y_label_area_size(70)
        .build_cartesian_2d(0i32..n as i32, 0f64..max_count * 1.12)
        .map_err(pe)?;

    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_x_axis()
        .bold_line_style(MUTED.mix(0.25))
        .light_line_style(TRANSPARENT)
        .axis_style(MUTED)
        .label_style(("monospace", 11).into_font().color(&TEXT))
        .draw()
        .map_err(pe)?;

    chart
        .draw_series(top.iter().enumerate().map(|(i, (_, count))| {
            let colour = if *count > 1 { ACCENT2 } else { ACCENT };
            Rectangle::new(
                [(i as i32, 0.0), (i as i32 + 1, *count as f64)],
                colour.mix(0.85).filled(),
            )
        }))
        .map_err(pe)?;

    root.draw(&Text::new(
        "■ repeated  ■ unique",
        (20, (height - 20) as i32),
        ("monospace", 11).into_font().color(&MUTED),
    ))
    .map_err(pe)?;

    root.present().map_err(pe)?;
    Ok(())
}

fn draw_progress(done: u64, total: u64, path: &Path) -> std::io::Result<()> {
    let width  = 600u32;
    let height = 60u32;
    let root = SVGBackend::new(path, (width, height)).into_drawing_area();
    root.fill(&SURFACE).map_err(pe)?;

    let frac    = if total == 0 { 0.0 } else { done as f64 / total as f64 };
    let fill_px = ((width - 4) as f64 * frac) as i32;

    root.draw(&Rectangle::new([(2, 8), ((width - 2) as i32, 52)], MUTED.mix(0.3).filled()))
        .map_err(pe)?;
    if fill_px > 0 {
        root.draw(&Rectangle::new([(2, 8), (2 + fill_px, 52)], ACCENT.mix(0.9).filled()))
            .map_err(pe)?;
    }
    root.draw(&Text::new(
        format!("{:.1}%", frac * 100.0),
        (width as i32 / 2 - 20, 16),
        ("monospace", 18).into_font().color(&TEXT),
    ))
    .map_err(pe)?;

    root.present().map_err(pe)?;
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn blend_gradient(t: f64) -> RGBColor {
    // Blue → teal gradient — stays within Adwaita's neutral palette
    let r = (ACCENT.0 as f64 * (1.0 - t) + 0x26_u8 as f64 * t) as u8;
    let g = (ACCENT.1 as f64 * (1.0 - t) + 0xa8_u8 as f64 * t) as u8;
    let b = (ACCENT.2 as f64 * (1.0 - t) + 0xa8_u8 as f64 * t) as u8;
    RGBColor(r, g, b)
}

fn pe(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}
