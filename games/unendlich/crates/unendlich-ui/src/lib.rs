//! **unendlich-ui** — the shared egui component kit and design tokens.
//!
//! Single source of truth for the look, consumed by:
//!   - `unendlich-dash` (the telemetry dashboard), and
//!   - the engine's in-game egui UI behind `egui_debug` (inventory, menus, dialogs).
//!
//! Pure presentation: depends only on `egui` (0.34), no app/model types, so both
//! the native dashboard and the game can reuse the exact same widgets. Build
//! against egui 0.34; if an API name differs in your egui point release, it'll be
//! a localized nudge.

use egui::epaint::Shadow;
use egui::{Color32, CornerRadius, Frame, Label, Margin, Response, RichText, Sense, Stroke, StrokeKind, TextWrapMode, Ui};

/// In-game menu components ported from the `menu_mockup.html` design
/// playground — transparent-first, for compositing over the live scene.
pub mod menu;
pub mod widget;

// r315: buttons live in the layered `widget` module (egui base -> unify -> button
// work -> toolkit abstraction -> toolkit language). Thin re-exports keep the public
// API (`unendlich_ui::button` / `button_style` / ...) unchanged.
/// Set the active button style candidate (0..=9). See [`widget`].
pub fn set_button_style(v: u8) {
    widget::set_button_style(v)
}
/// The active button style candidate.
pub fn button_style() -> u8 {
    widget::button_style()
}

// ── palette (mirrors the dashboard; the canonical values live here) ──────────
pub const BG: Color32 = Color32::from_rgb(0x0a, 0x0c, 0x10);
pub const PANEL: Color32 = Color32::from_rgb(0x10, 0x14, 0x1b);
pub const PANEL2: Color32 = Color32::from_rgb(0x16, 0x1b, 0x24);
pub const LINE: Color32 = Color32::from_rgb(0x22, 0x2a, 0x36);
pub const INK: Color32 = Color32::from_rgb(0xcd, 0xd6, 0xe3);
pub const DIM: Color32 = Color32::from_rgb(0x7b, 0x87, 0x98);
pub const FAINT: Color32 = Color32::from_rgb(0x4b, 0x55, 0x63);
pub const AMBER: Color32 = Color32::from_rgb(0xf7, 0xa2, 0x3b);
pub const AMBER_SOFT: Color32 = Color32::from_rgb(0xff, 0xce, 0x8a);
pub const CYAN: Color32 = Color32::from_rgb(0x57, 0xd6, 0xc9);
pub const GREEN: Color32 = Color32::from_rgb(0x8f, 0xd6, 0x94);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x63, 0x63);
pub const VIOLET: Color32 = Color32::from_rgb(0x9a, 0x8c, 0xff);

// ── theming: structural tokens as data, so an in-game panel can be themed by its
// SOURCE (a wooden workbench reads warm, an iron anvil cold) instead of the fixed
// dashboard palette. Status colours (CYAN/GREEN/RED/VIOLET) stay global — they
// mean fixed things. Propagated via context memory (like `Surface`), so opting in
// is one `set_theme` call and the default reproduces the dashboard look. ────────
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub bg: Color32,
    pub panel: Color32,
    pub panel2: Color32,
    pub line: Color32,
    pub ink: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
}

/// Linear per-channel blend of two colours (`t=0` → `a`, `t=1` → `b`).
pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

impl Theme {
    /// The canonical dark dashboard palette — the default everywhere.
    pub const DARK: Theme = Theme {
        bg: BG,
        panel: PANEL,
        panel2: PANEL2,
        line: LINE,
        ink: INK,
        dim: DIM,
        faint: FAINT,
        accent: AMBER,
        accent_soft: AMBER_SOFT,
    };

    /// Derive a theme from a material's albedo (e.g. `block_color`). The accent IS
    /// the material colour; surfaces are the material crushed toward near-black;
    /// ink is a near-white tinted by the material so text reads "warm" on wood and
    /// "cool" on iron. This is how a panel inherits the look of the thing it
    /// belongs to.
    pub fn from_material(albedo: Color32) -> Theme {
        let black = Color32::from_rgb(8, 9, 12);
        let white = Color32::from_rgb(245, 248, 252);
        let ink = mix(white, albedo, 0.18);
        let panel = mix(albedo, black, 0.86);
        Theme {
            bg: mix(albedo, black, 0.92),
            panel,
            panel2: mix(albedo, black, 0.80),
            line: mix(albedo, black, 0.60),
            ink,
            dim: mix(ink, panel, 0.45),
            faint: mix(ink, panel, 0.70),
            accent: albedo,
            accent_soft: mix(albedo, white, 0.40),
        }
    }

    /// Modulate the surface tones by ambient `light` ∈ [0,1] (text/accent kept
    /// readable). A panel in a dark cave dims; in daylight it brightens — so the
    /// lighting environment bleeds into the UI.
    pub fn lit(mut self, light: f32) -> Theme {
        let k = 0.45 + 0.55 * light.clamp(0.0, 1.0);
        self.bg = self.bg.gamma_multiply(k);
        self.panel = self.panel.gamma_multiply(k);
        self.panel2 = self.panel2.gamma_multiply(k);
        self.line = self.line.gamma_multiply(k);
        self
    }
}

fn theme_id() -> egui::Id {
    egui::Id::new("unendlich_ui::theme")
}

/// Set the active theme for subsequent components in this frame (context memory).
pub fn set_theme(ctx: &egui::Context, t: Theme) {
    ctx.memory_mut(|m| m.data.insert_temp(theme_id(), t));
}

/// The active theme, or [`Theme::DARK`] if none was set.
pub fn current_theme(ctx: &egui::Context) -> Theme {
    ctx.memory(|m| m.data.get_temp(theme_id())).unwrap_or(Theme::DARK)
}


/// Corner radius helper (kept as a fn since `CornerRadius::same` is const but
/// verbose at call sites).
#[inline]
pub fn r(n: u8) -> CornerRadius {
    CornerRadius::same(n)
}

/// A soft *colored* glow as an egui shadow. `offset = [0,0]` makes it a bloom
/// around the element rather than a drop shadow. `intensity` ∈ [0,1] scales both
/// the blur radius and the glow alpha — animate it for hover/active states.
pub fn glow(accent: Color32, intensity: f32) -> Shadow {
    let i = intensity.clamp(0.0, 1.0);
    Shadow {
        offset: [0, 0],
        blur: (12.0 + 10.0 * i) as u8,
        spread: 1,
        color: accent.gamma_multiply(0.30 * i),
    }
}

/// A panel/card: rounded frame, hairline border, soft accent glow, with an
/// optional accent lead-bar + title. The dashboard's panels and the game's menus
/// share this.
pub fn card(ui: &mut Ui, accent: Color32, title: Option<&str>, body: impl FnOnce(&mut Ui)) {
    let th = current_theme(ui.ctx());
    let fill = surface_fill(ui);
    Frame {
        inner_margin: Margin::same(16),
        outer_margin: Margin { bottom: 14, ..Default::default() },
        fill,
        stroke: Stroke::new(1.0, th.line),
        corner_radius: r(12),
        shadow: glow(accent, 0.45),
        ..Default::default()
    }
    .show(ui, |ui| {
        if let Some(t) = title {
            ui.horizontal(|ui| {
                let (bar, _) = ui.allocate_exact_size(egui::vec2(3.0, 13.0), Sense::hover());
                ui.painter().rect_filled(bar, r(1), accent);
                ui.add_space(7.0);
                ui.add(Label::new(RichText::new(t).color(th.dim).size(11.0)).wrap_mode(TextWrapMode::Extend));
            });
            ui.add_space(9.0);
        }
        body(ui);
    });
}

/// [`card`] themed by an explicit [`Theme`] (e.g. `Theme::from_material(albedo)`),
/// so a panel can inherit the look of its source material. Uses the theme's panel
/// fill directly (solid); status colours inside the body stay global.
pub fn card_themed(ui: &mut Ui, theme: Theme, title: Option<&str>, body: impl FnOnce(&mut Ui)) {
    // Propagate to inner components (slots, pills, labels) for this panel.
    set_theme(ui.ctx(), theme);
    Frame {
        inner_margin: Margin::same(16),
        outer_margin: Margin { bottom: 14, ..Default::default() },
        fill: theme.panel,
        stroke: Stroke::new(1.0, theme.line),
        corner_radius: r(12),
        shadow: glow(theme.accent, 0.45),
        ..Default::default()
    }
    .show(ui, |ui| {
        if let Some(t) = title {
            ui.horizontal(|ui| {
                let (bar, _) = ui.allocate_exact_size(egui::vec2(3.0, 13.0), Sense::hover());
                ui.painter().rect_filled(bar, r(1), theme.accent);
                ui.add_space(7.0);
                ui.add(Label::new(RichText::new(t).color(theme.dim).size(11.0)).wrap_mode(TextWrapMode::Extend));
            });
            ui.add_space(9.0);
        }
        body(ui);
    });
}
///
/// Measured widget: it allocates exactly what it paints (so it can never draw a
/// doubled/overlapping border), keeps its text on **one line** while it fits a
/// full row — so a wrapping row moves the *whole* pill to the next line rather
/// than collapsing it into a glyph-column — and only wraps its *text* when a
/// single pill is wider than the entire row, so it can never overflow/clip.
/// Returns the `Response` (hover it for a tooltip on truncated values, click it
/// for item-property / ability chips later).
pub fn pill(ui: &mut Ui, text: &str, color: Color32, filled: bool) -> Response {
    let th = current_theme(ui.ctx());
    let (fill, stroke, ink) = if filled {
        (color.gamma_multiply(0.12), color, color)
    } else {
        (Color32::TRANSPARENT, th.line, th.dim)
    };
    let font = egui::FontId::monospace(10.0);
    let pad = egui::vec2(8.0, 3.0);
    // Width of a full row, were this pill to wrap onto its own line.
    let max_text_w = (ui.max_rect().width() - pad.x * 2.0 - 2.0).max(24.0);
    let single = ui.painter().layout_no_wrap(text.to_owned(), font.clone(), ink);
    // Wrap the text only if even a full row can't hold it on one line.
    let galley = if single.size().x <= max_text_w {
        single
    } else {
        ui.painter().layout(text.to_owned(), font, ink, max_text_w)
    };
    let size = galley.size() + pad * 2.0;
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, r(10), fill);
    p.rect_stroke(rect, r(10), Stroke::new(1.0, stroke), StrokeKind::Inside);
    p.galley(rect.min + pad, galley, ink);
    resp
}

/// An inventory/hotbar slot: a square cell that highlights (border + glow) when
/// selected or hovered, with an optional centered label (item count, key hint,
/// glyph). Returns the click `Response`.
pub fn slot(ui: &mut Ui, px: f32, selected: bool, label: Option<&str>) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(px, px), Sense::click());
    let t = ui.ctx().animate_bool(resp.id, resp.hovered() || selected);
    let th = current_theme(ui.ctx());
    let p = ui.painter();
    p.rect_filled(rect, r(6), th.panel2);
    let border = if selected { th.accent } else { th.line };
    let width = if selected { 1.5 } else { 1.0 };
    p.rect_stroke(rect, r(6), Stroke::new(width, border.gamma_multiply(0.55 + 0.45 * t)), StrokeKind::Inside);
    // Centered label (was silently dropped previously, leaving slots blank).
    if let Some(l) = label {
        let galley = p.layout_no_wrap(l.to_string(), egui::FontId::monospace(11.0), th.ink);
        let pos = rect.center() - galley.size() * 0.5;
        p.galley(pos, galley, th.ink);
    }
    resp
}

/// Like [`slot`] but tinted with a `swatch` colour (e.g. a block's albedo) so
/// items are identifiable at a glance. The swatch fills the upper portion of the
/// cell; the label sits along the bottom. `None` falls back to a neutral cell.
pub fn slot_swatch(
    ui: &mut Ui,
    px: f32,
    selected: bool,
    swatch: Option<Color32>,
    label: Option<&str>,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(px, px), Sense::click());
    let t = ui.ctx().animate_bool(resp.id, resp.hovered() || selected);
    let th = current_theme(ui.ctx());
    let p = ui.painter();
    p.rect_filled(rect, r(6), th.panel2);
    if let Some(c) = swatch {
        let sw = egui::Rect::from_min_max(
            rect.min + egui::vec2(5.0, 5.0),
            egui::pos2(rect.max.x - 5.0, rect.max.y - 14.0),
        );
        p.rect_filled(sw, r(4), c);
    }
    let border = if selected { th.accent } else { th.line };
    let width = if selected { 1.5 } else { 1.0 };
    p.rect_stroke(
        rect,
        r(6),
        Stroke::new(width, border.gamma_multiply(0.55 + 0.45 * t)),
        StrokeKind::Inside,
    );
    if let Some(l) = label {
        let galley = p.layout_no_wrap(l.to_string(), egui::FontId::monospace(9.0), th.ink);
        let pos = egui::pos2(rect.center().x - galley.size().x * 0.5, rect.max.y - 12.0);
        p.galley(pos, galley, th.ink);
    }
    resp
}

/// A classic bottom-bar hotbar: a subtle backing strip + a horizontal row of
/// `count` cells, filled by `render(ui, i)` (typically a [`slot`]/[`slot_swatch`]).
/// Themed via the active theme. The caller positions it (an `Area` near the screen
/// bottom); [`hotbar_width`] gives the strip's pixel width for centering.
pub fn hotbar(ui: &mut Ui, count: usize, gap: f32, mut render: impl FnMut(&mut Ui, usize)) {
    let th = current_theme(ui.ctx());
    Frame {
        inner_margin: Margin::same(6),
        fill: th.panel.gamma_multiply(0.88),
        stroke: Stroke::new(1.0, th.line),
        corner_radius: r(10),
        shadow: glow(th.accent, 0.30),
        ..Default::default()
    }
    .show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for i in 0..count {
                render(ui, i);
            }
        });
    });
}

/// Pixel width of a [`hotbar`] with `count` cells of `cell_px` and `gap` spacing
/// (includes the 6px inner margins). Use it to center the hotbar's `Area`.
pub fn hotbar_width(count: usize, cell_px: f32, gap: f32) -> f32 {
    let n = count.max(1) as f32;
    n * cell_px + (n - 1.0).max(0.0) * gap + 12.0
}

/// A toolkit button (layered implementation + the live style candidates live in
/// [`widget`]). Returns the click `Response`. Dashboard actions and game menus.
pub fn button(ui: &mut Ui, label: &str, accent: Color32) -> Response {
    widget::button(ui, label, accent)
}

/// Like [`button`] but the box is at least `min_size` (label stays centered), and
/// honors the live style index from [`button_style`] (a debug-menu slider toggles
/// it 0..=9). The variants are visually distinct so one build can A/B many looks.
/// r314.
pub fn button_sized(ui: &mut Ui, label: &str, accent: Color32, min_size: egui::Vec2) -> Response {
    widget::button_sized(ui, label, accent, min_size)
}

/// The "elegant" button (cursor-tied dither infill) — see [`widget::elegant_button`].
pub fn elegant_button(ui: &mut Ui, label: &str) -> Response {
    widget::elegant_button(ui, label)
}
/// Sized elegant button.
pub fn elegant_button_sized(ui: &mut Ui, label: &str, min_size: egui::Vec2) -> Response {
    widget::elegant_button_sized(ui, label, min_size)
}

/// A tab button (selected state) — see [`widget::tab_button`].
pub fn tab_button(ui: &mut Ui, label: &str, selected: bool) -> Response {
    widget::tab_button(ui, label, selected)
}

// ── intent: the bubble-up "ask-first" interaction model ──────────────────────
/// Components are never disabled top-down. Instead, anything *above* — a modal,
/// an in-progress drag — registers a [`intent::Policy`] each frame; a component
/// about to fire calls [`intent::may_fire`], which asks every active policy
/// "should this still fire?" Any veto wins. Each component decides for itself,
/// but never acts against what the user is actually doing.
///
/// Order-independent on purpose: egui draws the background before overlays, so
/// policies live in a small time-stamped registry in context memory (a policy
/// counts while it's been refreshed within the last ~50 ms). A modal that stops
/// refreshing — because it closed — stops vetoing on its own, no teardown.
pub mod intent {
    use egui::{Context, Id, Pos2, Rect, Response, Ui};
    use std::collections::HashMap;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum IntentKind {
        Tap,
        Hold,
        DragStart,
        DragCommit,
    }

    #[derive(Clone, Copy)]
    pub struct Intent {
        pub kind: IntentKind,
        pub pos: Pos2,
    }
    impl Intent {
        pub fn tap(pos: Pos2) -> Self {
            Self { kind: IntentKind::Tap, pos }
        }
        pub fn hold(pos: Pos2) -> Self {
            Self { kind: IntentKind::Hold, pos }
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Verdict {
        Allow,
        Veto,
    }

    #[derive(Clone, Copy)]
    pub enum Policy {
        /// A modal occupies `rect`. A tap *outside* it is vetoed — it bubbles up to
        /// "dismiss the modal" instead of firing a background widget. A modal with
        /// no top-down scrim.
        ModalAbove { rect: Rect },
        /// A drag is in progress; stray taps elsewhere are suppressed until release.
        DragActive,
    }
    impl Policy {
        fn verdict(&self, intent: &Intent) -> Verdict {
            match self {
                Policy::ModalAbove { rect } => {
                    if intent.kind == IntentKind::Tap && !rect.contains(intent.pos) {
                        Verdict::Veto
                    } else {
                        Verdict::Allow
                    }
                }
                Policy::DragActive => {
                    if intent.kind == IntentKind::Tap {
                        Verdict::Veto
                    } else {
                        Verdict::Allow
                    }
                }
            }
        }
    }

    #[derive(Clone, Default)]
    struct Registry {
        guards: HashMap<Id, (f64, Policy)>,
    }

    fn reg_id() -> Id {
        Id::new("unendlich_ui::intent::registry")
    }

    /// Re-register a policy for this frame. A container above other widgets calls
    /// this every frame it's active (a modal passes its current rect).
    pub fn register(ctx: &Context, id: Id, policy: Policy) {
        let now = ctx.input(|i| i.time);
        ctx.memory_mut(|m| {
            let mut reg: Registry = m.data.get_temp(reg_id()).unwrap_or_default();
            reg.guards.insert(id, (now, policy));
            m.data.insert_temp(reg_id(), reg);
        });
    }

    /// Ask every active (recently-refreshed) policy whether `intent` may fire.
    pub fn propose(ctx: &Context, intent: Intent) -> Verdict {
        let now = ctx.input(|i| i.time);
        let reg: Registry = ctx.memory(|m| m.data.get_temp(reg_id()).unwrap_or_default());
        for (_, (last, policy)) in reg.guards.iter() {
            if now - *last < 0.05 && policy.verdict(&intent) == Verdict::Veto {
                return Verdict::Veto;
            }
        }
        Verdict::Allow
    }

    /// The common case: a component saw a click and asks whether it may act on it.
    pub fn may_fire(ui: &Ui, resp: &Response) -> bool {
        let pos = resp
            .interact_pointer_pos()
            .or_else(|| ui.input(|i| i.pointer.interact_pos()))
            .unwrap_or_else(|| resp.rect.center());
        propose(ui.ctx(), Intent::tap(pos)) == Verdict::Allow
    }
}

/// A modal dialog: a centered [`card`] on the foreground layer. No top-down
/// scrim — it registers an `intent::ModalAbove` policy so background widgets
/// ask-first, and a tap outside it bubbles up to dismiss. Call every frame while
/// open; returns `true` when the user asked to close. Reuse for in-game menus
/// and confirmations.
pub fn dialog(ctx: &egui::Context, accent: Color32, title: &str, body: impl FnOnce(&mut Ui)) -> bool {
    let id = egui::Id::new(("unendlich_ui::dialog", title));
    let inner = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_max_width(440.0);
            card(ui, accent, Some(title), body);
        });
    let rect = inner.response.rect;
    intent::register(ctx, id, intent::Policy::ModalAbove { rect });
    ctx.input(|i| i.pointer.any_click() && i.pointer.interact_pos().map_or(false, |p| !rect.contains(p)))
}

/// A radar / spider chart comparing one or more series across shared axes.
/// Per-series `values` are pre-normalized to 0..1 (the caller scales each axis
/// however it likes — usually per-axis so small + large metrics are both
/// visible). Generic on purpose: run-vs-run now, mob-vs-mob or player stat
/// profiles later. Returns the `Response` (hover for future per-vertex tips).
pub fn radar(ui: &mut Ui, size: f32, axes: &[&str], series: &[(&str, Color32, Vec<f32>)]) -> Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let center = rect.center();
    let radius = (size * 0.5 - 28.0).max(10.0);
    let n = axes.len().max(1);
    let angle = |i: usize| -> f32 {
        -std::f32::consts::FRAC_PI_2 + (i as f32) * std::f32::consts::TAU / (n as f32)
    };
    let p = ui.painter();
    let th = current_theme(ui.ctx());
    // concentric rings
    for k in 1..=4 {
        p.circle_stroke(center, radius * (k as f32 / 4.0), Stroke::new(1.0, th.line.gamma_multiply(0.6)));
    }
    // spokes + axis labels
    for (i, ax) in axes.iter().enumerate() {
        let a = angle(i);
        let dir = egui::vec2(a.cos(), a.sin());
        p.line_segment([center, center + dir * radius], Stroke::new(1.0, th.line.gamma_multiply(0.6)));
        p.text(center + dir * (radius + 14.0), egui::Align2::CENTER_CENTER, *ax, egui::FontId::monospace(9.0), th.dim);
    }
    // series polygons (fill + outline) with vertex dots
    for (_, color, vals) in series.iter() {
        let pts: Vec<egui::Pos2> = (0..n)
            .map(|i| {
                let v = vals.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
                let a = angle(i);
                center + egui::vec2(a.cos(), a.sin()) * (radius * v)
            })
            .collect();
        p.add(egui::Shape::convex_polygon(pts.clone(), color.gamma_multiply(0.12), Stroke::new(2.0, *color)));
        for v in &pts {
            p.circle_filled(*v, 2.5, *color);
        }
    }
    resp
}

/// Lay `count` items into a responsive grid that *fills* the available width: as
/// many equal columns as fit at >= `min_col_w`, dropping to fewer columns — more
/// rows — only when space shrinks. Cells stretch to share the width (like the
/// workload bars), so a wide page doesn't leave dead space. `render(ui, i)` draws
/// item `i` into its column.
///
/// Open-world overlays have no implicit parent width, so give the group its own
/// box first — a sized/anchored container, the way [`dialog`] anchors to the
/// screen rect — and this fills *that* box, not the whole screen.
pub fn grid_fit(ui: &mut Ui, min_col_w: f32, gap: f32, count: usize, mut render: impl FnMut(&mut Ui, usize)) {
    if count == 0 {
        return;
    }
    let avail = ui.available_width().max(min_col_w);
    let cols = (((avail + gap) / (min_col_w + gap)).floor() as usize).clamp(1, count);
    let mut i = 0;
    while i < count {
        let row_n = cols.min(count - i);
        ui.columns(cols, |columns| {
            for c in 0..row_n {
                render(&mut columns[c], i + c);
            }
        });
        i += row_n;
        ui.add_space(gap);
    }
}

// ── surface: opaque (dashboard) vs frosted glass (in-game overlays) ──────────
/// Whether panels are opaque or frosted glass over live content. Ambient: set
/// once per context/region with [`set_surface`]; components read it when they
/// paint their background, so the same `card`/`dialog` is solid on the dashboard
/// and frosted floating over the world.
#[derive(Clone, Copy)]
pub enum Surface {
    /// Opaque PANEL fill (dashboard, or any UI not over live content).
    Solid,
    /// Frosted glass. `blur` is the engine's blurred-scene texture; until the
    /// blur pass exists it's `None` and we fall back to a translucent `tint` that
    /// shows the raw scene behind. `tint` is the legibility wash over the backdrop.
    Frosted { blur: Option<egui::TextureId>, tint: Color32 },
}
impl Surface {
    /// Frosted with no blur texture yet — a translucent wash (the blur pass
    /// upgrades this by supplying the texture).
    pub fn frosted() -> Self {
        Surface::Frosted { blur: None, tint: PANEL.gamma_multiply(0.74) }
    }
    /// Frosted backed by the engine's blurred-scene texture + a legibility tint.
    pub fn frosted_blur(blur: egui::TextureId, tint: Color32) -> Self {
        Surface::Frosted { blur: Some(blur), tint }
    }
}

fn surface_id() -> egui::Id {
    egui::Id::new("unendlich_ui::surface")
}
/// Set the ambient surface for components drawn after this call in `ctx`.
pub fn set_surface(ctx: &egui::Context, s: Surface) {
    ctx.memory_mut(|m| m.data.insert_temp(surface_id(), s));
}
/// The current ambient surface (defaults to [`Surface::Solid`]).
pub fn current_surface(ctx: &egui::Context) -> Surface {
    ctx.memory(|m| m.data.get_temp(surface_id())).unwrap_or(Surface::Solid)
}
/// Panel fill for the current surface: opaque PANEL, or the frosted tint. (The
/// blur step adds the actual blurred-scene texture under the tint.)
pub fn surface_fill(ui: &Ui) -> Color32 {
    match current_surface(ui.ctx()) {
        Surface::Solid => current_theme(ui.ctx()).panel,
        // With a blur texture the backdrop is painted by `frosted_backdrop` (so the
        // frame fill is transparent); without one, the tint *is* the translucent fill.
        Surface::Frosted { blur: Some(_), .. } => Color32::TRANSPARENT,
        Surface::Frosted { blur: None, tint } => tint,
    }
}

/// Paint the frosted backdrop — the blurred-scene texture sampled under `rect`,
/// plus the legibility tint — for a container that knows its rect (a fixed or
/// anchored in-game box). Call it BEFORE drawing the card so the blur sits behind
/// the content. No-op for Solid or untextured frosted. The texture is the full
/// frame's blurred scene; `rect / screen_rect` gives the slice behind this panel.
pub fn frosted_backdrop(ui: &Ui, rect: egui::Rect, corner: CornerRadius) {
    if let Surface::Frosted { blur: Some(tex), tint } = current_surface(ui.ctx()) {
        let screen = ui.ctx().content_rect();
        let sz = screen.size();
        let uv = egui::Rect::from_min_max(
            egui::pos2((rect.min.x - screen.min.x) / sz.x, (rect.min.y - screen.min.y) / sz.y),
            egui::pos2((rect.max.x - screen.min.x) / sz.x, (rect.max.y - screen.min.y) / sz.y),
        );
        let p = ui.painter();
        p.image(tex, rect, uv, Color32::WHITE);
        p.rect_filled(rect, corner, tint);
        // r228: edge-weighted inner glow (the mockup's "thicker glass at the
        // rim than the centre") — a few inset rings, brightest at the edge,
        // fading inward. Tinted by the theme accent. Pure overlay; only runs
        // for textured frosted panels.
        let accent = current_theme(ui.ctx()).accent;
        for i in 0..4 {
            let inset = 1.0 + i as f32 * 2.0;
            let alpha = (0.14 * (1.0 - i as f32 / 4.0) + 0.05).clamp(0.0, 1.0);
            p.rect_stroke(
                rect.shrink(inset),
                corner,
                Stroke::new(1.0, accent.gamma_multiply(alpha)),
                StrokeKind::Inside,
            );
        }
        // top rim highlight
        p.line_segment(
            [rect.left_top() + egui::vec2(4.0, 1.0), rect.right_top() + egui::vec2(-4.0, 1.0)],
            Stroke::new(1.4, Color32::from_rgba_unmultiplied(210, 235, 255, 90)),
        );
    }
}


// ── r191: menu window kit (moved out of the renderer so every window
// reuses ONE implementation; the renderer keeps thin aliases) ──────────
/// Destiny-esque menu button: full-width hit area, hover-growing accent
/// bar, letterspaced caps.
pub fn dest_button(ui: &mut egui::Ui, label: &str) -> bool {
    let w = ui.available_width().min(260.0).max(180.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 34.0), egui::Sense::click());
    let tt = ui.ctx().animate_bool(resp.id, resp.hovered());
    let p = ui.painter();
    p.rect_filled(rect, egui::Rounding::same(3),
        egui::Color32::from_rgba_unmultiplied(24, 30, 38, (130.0 + 70.0 * tt) as u8));
    p.rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(3.0 + 9.0 * tt, rect.height())),
        egui::Rounding::ZERO,
        egui::Color32::from_rgba_unmultiplied(150, 200, 230, (140.0 + 110.0 * tt) as u8));
    let spaced: String = label.to_uppercase().chars().flat_map(|c| [c, '\u{2009}']).collect();
    p.text(egui::pos2(rect.min.x + 18.0 + 4.0 * tt, rect.center().y),
        egui::Align2::LEFT_CENTER, spaced.trim_end(),
        egui::FontId::proportional(15.0),
        egui::Color32::from_rgba_unmultiplied(225, 235, 245, (200.0 + 55.0 * tt) as u8));
    resp.clicked()
}

/// Glass frame at open-interpolation t in [0,1] (margin + alpha ease).
pub fn glass_at(t01: f32) -> egui::Frame {
    egui::Frame::new()
        .fill(egui::Color32::from_rgba_unmultiplied(14, 18, 24, (120.0 + 88.0 * t01) as u8))
        .stroke(egui::Stroke::new(1.0,
            egui::Color32::from_rgba_unmultiplied(120, 160, 190, (40.0 + 50.0 * t01) as u8)))
        .rounding(egui::Rounding::same(8))
        .inner_margin(egui::Margin::same((6.0 + 8.0 * t01) as i8))
}

/// r203 liquid-glass v3 — layered glassy treatment: bright top rim, a
/// soft top highlight, side glints, a BOTTOM inner-glow (light pooling at
/// the base, the hallmark of "liquid glass"), and TWO offset diagonal
/// sheen sweeps for a living shimmer. True refraction warp = PostFx pass.
pub fn fresnel(ui: &egui::Ui, r: egui::Rect, a: f32) {
    let p = ui.painter();
    // bright top rim
    p.line_segment(
        [r.left_top() + egui::vec2(6.0, 1.0), r.right_top() + egui::vec2(-6.0, 1.0)],
        egui::Stroke::new(1.6, egui::Color32::from_rgba_unmultiplied(215, 240, 255, (160.0 * a) as u8)));
    // top highlight: 8 stacked bands fading downward (smoother than v2's 6)
    let bands = 8;
    for i in 0..bands {
        let f = i as f32 / bands as f32;
        let y0 = r.top() + 2.0 + f * r.height() * 0.34;
        let y1 = y0 + r.height() * 0.34 / bands as f32;
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(r.left() + 2.0, y0), egui::pos2(r.right() - 2.0, y1)),
            0.0,
            egui::Color32::from_rgba_unmultiplied(190, 220, 245, ((28.0 * (1.0 - f)) * a) as u8));
    }
    // r203 BOTTOM inner-glow: light pools at the base — 5 bands fading UP
    // from the bottom edge. This is what reads as "liquid" glass vs flat.
    let bbands = 5;
    for i in 0..bbands {
        let f = i as f32 / bbands as f32; // 0 at bottom edge
        let y1 = r.bottom() - 2.0 - f * r.height() * 0.22;
        let y0 = y1 - r.height() * 0.22 / bbands as f32;
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(r.left() + 2.0, y0), egui::pos2(r.right() - 2.0, y1)),
            0.0,
            egui::Color32::from_rgba_unmultiplied(120, 180, 230, ((20.0 * (1.0 - f)) * a) as u8));
    }
    // side glints
    for (x0, x1) in [(r.left(), r.left() + 1.2), (r.right() - 1.2, r.right())] {
        p.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, r.top() + 2.0), egui::pos2(x1, r.top() + r.height() * 0.45)),
            0.0,
            egui::Color32::from_rgba_unmultiplied(185, 220, 245, (48.0 * a) as u8));
    }
    // r203 two offset diagonal sheen sweeps (different phase + speed) so the
    // surface shimmers like a liquid instead of a single robotic pass.
    let t = ui.ctx().input(|i| i.time) as f32;
    for (period, width, alpha, span) in [(0.11f32, 60.0f32, 16.0f32, 160.0f32),
                                          (0.067f32, 34.0f32, 10.0f32, 200.0f32)] {
        let sweep = ((t * period).fract()) * (r.width() + span) - span * 0.5;
        let x = r.left() + sweep;
        p.line_segment(
            [egui::pos2(x, r.bottom()), egui::pos2(x + width, r.top())],
            egui::Stroke::new(width * 0.23,
                egui::Color32::from_rgba_unmultiplied(200, 230, 255, (alpha * a) as u8)));
    }
    ui.ctx().request_repaint(); // keep the sweeps moving on idle menus
}
