//! Custom CSS provider for RustyBoard.

use crate::config::{AppearanceConfig, LayoutConfig};
use std::cell::RefCell;

const BUILTIN_CSS: &str = include_str!("../../resources/styles/keyboard.css");

thread_local! {
    /// Tracks the most recently installed CSS provider so that
    /// `install_css_provider` can remove it before installing a
    /// replacement. Without this, every settings change leaks
    /// another CssProvider into GTK's display-level provider stack
    /// (each holds compiled CSS data ~5-10KB) and the stack grows
    /// unboundedly over the keyboard's session lifetime.
    static CURRENT_PROVIDER: RefCell<Option<gtk4::CssProvider>> = const { RefCell::new(None) };
}

/// Install our CSS as the lowest-priority custom provider on the default
/// display, so the user's GTK theme can still override individual rules
/// where it wants to.
pub fn install_css_provider(appearance: &AppearanceConfig, layout: &LayoutConfig) {
    let provider = gtk4::CssProvider::new();
    let css = build_css_with_overrides(appearance, layout);
    provider.load_from_string(&css);

    if let Some(display) = gtk4::gdk::Display::default() {
        // Drop the previous provider before adding the new one —
        // otherwise GTK's provider stack grows by one entry per
        // settings change.
        CURRENT_PROVIDER.with(|slot| {
            if let Some(prev) = slot.borrow_mut().take() {
                gtk4::style_context_remove_provider_for_display(&display, &prev);
            }
        });
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        CURRENT_PROVIDER.with(|slot| *slot.borrow_mut() = Some(provider));
    }
}

fn build_css_with_overrides(appearance: &AppearanceConfig, layout: &LayoutConfig) -> String {
    // The builtin stylesheet handles all the structural rules; we
    // append a small override block at the end so user config flows
    // through as concrete CSS values.
    let radius = appearance.key_radius;
    let opacity = appearance.opacity;
    let accent = sanitize_hex(&appearance.accent_color);
    let font = &appearance.font_family;

    // Color overrides: each is only emitted when the user actually set
    // a hex value (empty string = "use theme default"). For each color
    // we write a small CSS rule that targets the right selector. The
    // user's accent color picker takes precedence over the legacy
    // `accent_color` field if both are set.
    let c = &appearance.colors;
    let mut color_overrides = String::new();
    if !c.background.is_empty() {
        color_overrides.push_str(&format!(
            ".keyboard-view {{ background-color: #{}; }}\n",
            sanitize_hex(&c.background)
        ));
    }
    if !c.key_fill.is_empty() {
        color_overrides.push_str(&format!(
            ".key-button.key-normal {{ background-color: #{}; }}\n",
            sanitize_hex(&c.key_fill)
        ));
    }
    if !c.key_fill_pressed.is_empty() {
        let hex = sanitize_hex(&c.key_fill_pressed);
        color_overrides.push_str(&format!(
            ".key-button:active, .key-button.pressed {{ background-color: #{}; }}\n",
            hex
        ));
    }
    if !c.key_text.is_empty() {
        color_overrides.push_str(&format!(
            ".key-label {{ color: #{}; }}\n",
            sanitize_hex(&c.key_text)
        ));
    }
    if !c.modifier_fill.is_empty() {
        color_overrides.push_str(&format!(
            ".key-button.key-modifier {{ background-color: #{}; }}\n",
            sanitize_hex(&c.modifier_fill)
        ));
    }
    if !c.special_fill.is_empty() {
        color_overrides.push_str(&format!(
            ".key-button.key-special, .key-button.key-action {{ background-color: #{}; }}\n",
            sanitize_hex(&c.special_fill)
        ));
    }
    // The accent override: prefer the new picker value, fall back to
    // the legacy accent_color field. We always emit this so `:root`
    // has a usable accent variable.
    let accent_final = if c.accent.is_empty() {
        accent
    } else {
        sanitize_hex(&c.accent)
    };

    // Compact mode: reduce padding & min-height on every key button.
    // The keyboard root gets `.compact` when layout.compact is true.
    // Aggressive for phone displays at 300% scaling — every saved
    // logical pixel is 3 actual screen pixels.
    let compact_rules = if layout.compact {
        ".keyboard-view { padding: 2px; }\n\
         .keyboard-row { margin: 0; }\n\
         .key-button { min-height: 38px; padding: 1px; }\n\
         .key-label { font-size: 14px; }\n"
            .to_string()
    } else {
        String::new()
    };

    // Direction hints — small greyed labels at N/E/S/W of cardinal-grid
    // keys. Sized for readability at 300% phone scaling (Plasma Mobile
    // FP5 default): the absolute glyph height on screen is roughly
    // 11px ÷ 3 ≈ 3.7 actual-pixels, which is the minimum that's
    // recognizable. Color is 50% alpha so hints don't fight the center
    // label for attention — they're "scaffolding for new users,"
    // ignored by touch typists.
    let hint_rules = "\
        .key-hint { \
            font-size: 11px; \
            color: alpha(@window_fg_color, 0.50); \
            margin: 2px 4px; \
            padding: 0; \
            font-weight: 300; \
        }\n";

    format!(
        "{builtin}\n\
         :root {{\n\
            --rb-key-radius: {radius}px;\n\
            --rb-keyboard-opacity: {opacity};\n\
            --rb-accent: #{accent};\n\
         }}\n\
         .key-button {{ font-family: \"{font}\", sans-serif; }}\n\
         {hint_rules}\
         {color_overrides}\
         {compact_rules}",
        builtin = BUILTIN_CSS,
        radius = radius,
        opacity = opacity,
        accent = accent_final,
        font = font,
        hint_rules = hint_rules,
        color_overrides = color_overrides,
        compact_rules = compact_rules,
    )
}

fn sanitize_hex(input: &str) -> String {
    input
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(6)
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_hash() {
        assert_eq!(sanitize_hex("#3584e4"), "3584e4");
    }

    #[test]
    fn sanitize_drops_invalid_chars() {
        assert_eq!(sanitize_hex("3584e4!@#"), "3584e4");
    }

    #[test]
    fn sanitize_caps_to_six_chars() {
        assert_eq!(sanitize_hex("3584e4abcdef"), "3584e4");
    }
}
