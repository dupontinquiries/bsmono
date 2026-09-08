//! chat_dock — the physical dock: its panel, its controls, and what they mean.
//!
//! Fifth cut of `chat.rs`. `physui` models a slab of hardware — knobs with ranges, switches
//! with detents — and these four functions are the whole translation between it and the chat:
//! `phys_dock` builds the panel, `ctx_get`/`ctx_set` bind its controls to the `Ctx` sampler
//! settings, and `dock_action` turns a `ControlEvent` into something the chat does.
//!
//! Worth having together because the four have to AGREE. A knob added to `phys_dock` with no
//! arm in `ctx_get` reads as a control that does nothing, which is the physical-UI equivalent
//! of a dead button — and the two were sixty lines and a scroll apart.
//!
//! `Ctx` itself stays in `chat`: it is the chat's own state and the dock only reads and
//! writes it. Moving state because a helper touches it is how a "split" turns into a
//! rearrangement nobody can follow.

use super::*;

/// Read a `Ctx` field as an f32 by its physical-dial id (for syncing the dial handle to the live value).
pub(super) fn ctx_get(ctx: &Ctx, id: usize) -> f32 {
    match id {
        PARAM_TEMP => ctx.temperature,
        PARAM_NCTX => ctx.n_ctx as f32,
        PARAM_MAXTOK => ctx.max_tokens as f32,
        PARAM_REPEAT => ctx.repeat_penalty,
        PARAM_MINP => ctx.min_p,
        _ => 0.0,
    }
}

/// Write a `Ctx` field from a dragged dial value (rounding the integer params).
pub(super) fn ctx_set(ctx: &mut Ctx, id: usize, v: f32) {
    match id {
        PARAM_TEMP => ctx.temperature = v,
        PARAM_NCTX => ctx.n_ctx = v.round() as u32,
        PARAM_MAXTOK => ctx.max_tokens = v.round() as u32,
        PARAM_REPEAT => ctx.repeat_penalty = v,
        PARAM_MINP => ctx.min_p = v,
        _ => {}
    }
}

/// Build the in-scene control dock: every scene control is a physical object — Overview/Prune/Unprune
/// momentary, Tour/Freecam toggles — plus the Context panel's sampling sliders as physical Dials.
pub(super) fn phys_dock() -> crate::physui::Panel {
    use crate::physui::{ControlKind, Panel};
    let mut p = Panel::new();
    p.add(DOCK_OVERVIEW, ControlKind::Momentary, false);
    p.add(DOCK_TOUR, ControlKind::Toggle, false);
    p.add(DOCK_FREECAM, ControlKind::Toggle, false);
    p.add(DOCK_PRUNE, ControlKind::Momentary, false);
    p.add(DOCK_UNPRUNE, ControlKind::Momentary, false);
    let cd = Ctx::default();
    for &(id, _, lo, hi, _) in &PARAM_DIALS { p.add_dial(id, lo, hi, ctx_get(&cd, id)); }
    p.add_selector(BACKEND_SEL, BackendKind::ALL.len(), 0);
    p.add_selector(PERSONA_SEL, PERSONAS.len(), 0);
    p.add_selector(MODEL_SEL, 1, 0); // real count set each frame via set_selector_count once known
    p.add_selector(ROLE_SEL, 3, 2);  // default Thinker: the safe assumption is that a model is authoritative
    p
}

/// The camera/context change a physical-dock actuation implies. Pure + testable so the button→action
/// mapping (and the fact that non-dock events are ignored) is locked without a live scene; the field
/// mutations that apply it mirror the controls the (now-removed) 2D toolbar used to expose.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum DockAction { Overview, Tour(bool), Freecam(bool), Prune, Unprune }

/// Map a [`physui::ControlEvent`] from the dock to its [`DockAction`], or `None` for events that aren't
/// one of the dock's controls (unknown ids, dial adjusts, etc.).
pub(super) fn dock_action(ev: crate::physui::ControlEvent) -> Option<DockAction> {
    use crate::physui::ControlEvent::*;
    match ev {
        Fired(DOCK_OVERVIEW) => Some(DockAction::Overview),
        Toggled(DOCK_TOUR, on) => Some(DockAction::Tour(on)),
        Toggled(DOCK_FREECAM, on) => Some(DockAction::Freecam(on)),
        Fired(DOCK_PRUNE) => Some(DockAction::Prune),
        Fired(DOCK_UNPRUNE) => Some(DockAction::Unprune),
        _ => None,
    }
}
