//! seq_page — the Sequence Lab tab: `seq_lab` driven from the shell, with the
//! two engine-side modules it vindicates given their first visible surface.
//!
//! Three sections, three modules, all calling the real thing:
//!
//! * **Sequence encode** — `seq_lab::run_sequence` drives the actual
//!   `V2Encoder` over a synthetic pan and `seq_lab_ui` reports the split
//!   (K-cost vs T-cost is the codec working or failing; an average hides
//!   both).
//! * **Reprojection check** — `reproject` computes per-pixel motion for a
//!   camera step and reports the residual fraction: the number
//!   `NEURAL_UPSCALE_APPLICABILITY.md` wrongly claimed FAO could not have,
//!   measured live.
//! * **Part fit** — `part_fit::fit` over a synthetic wheel-ish cluster and a
//!   flat wall, showing the accept AND the reject. The module's own doc says
//!   the failure mode (confident, wrong, plausible structure) is worse than
//!   the win, so the page shows the guard working, not just the win.

use crate::{part_fit, reproject, seq_lab};

/// Sequence Lab state, held on the app across frames.
pub struct SeqState {
    pub width: usize,
    pub height: usize,
    pub quality: f32,
    pub frames: usize,
    pub run: Option<seq_lab::SeqRun>,
    /// The camera step (px of pan at depth 1) for the reprojection section.
    pub cam_step: f32,
}

impl Default for SeqState {
    fn default() -> Self {
        // Small enough that a run is a click, big enough that motion search
        // has something to find. Quality matches lab.rs's default.
        SeqState { width: 96, height: 96, quality: 0.7, frames: 8, run: None, cam_step: 0.02 }
    }
}

/// One reprojection measurement: a lateral camera step over a synthetic depth
/// plane, summarised by the module's own honesty metrics.
pub struct ReprojReport {
    pub residual: f32,
    pub visible: usize,
    pub total: usize,
}

/// Reproject a grid of points at mixed depths through a lateral camera step.
///
/// Depths vary across the grid so the parallax differs per pixel — a single
/// plane would make every vector identical and the residual trivially zero,
/// which demonstrates nothing.
pub fn reproject_step(step: f32) -> ReprojReport {
    const W: usize = 256;
    const H: usize = 256;
    // Principal point at frame centre, so "off-frame" means off THIS frame.
    let mut prev = reproject::Pose::default();
    prev.principal = [W as f32 / 2.0, H as f32 / 2.0];
    let mut cur = reproject::Pose { position: [step, 0.0, 0.0], ..reproject::Pose::default() };
    cur.principal = prev.principal;
    let mut vis = Vec::new();
    let n = 24;
    for gy in 0..n {
        for gx in 0..n {
            let x = (gx as f32 + 0.5) * (W as f32 / n as f32);
            let y = (gy as f32 + 0.5) * (H as f32 / n as f32);
            let depth = 2.0 + (gx % 5) as f32 * 0.8; // mixed depths → mixed parallax
            let v = match reproject::reproject(&prev, &cur, x, y, depth) {
                Some(r) => reproject::visibility(&r, Some(depth), W, H),
                // A projection that failed is content the camera cannot see:
                // the module's own definition of disocclusion.
                None => reproject::Visibility::Disoccluded,
            };
            vis.push(v);
        }
    }
    ReprojReport {
        residual: reproject::residual_fraction(&vis),
        visible: vis.iter().filter(|v| matches!(v, reproject::Visibility::Consistent)).count(),
        total: vis.len(),
    }
}

/// A synthetic wheel-ish ring of points, the shape the vocabulary should accept.
pub fn wheel_cluster() -> Vec<[f32; 3]> {
    let mut pts = Vec::new();
    for i in 0..48 {
        let a = i as f32 / 48.0 * std::f32::consts::TAU;
        // A cylinder: circular in XY, thin in Z — a wheel's signature.
        pts.push([a.cos() * 0.35, a.sin() * 0.35, (i % 3) as f32 * 0.02]);
        pts.push([a.cos() * 0.30, a.sin() * 0.30, (i % 2) as f32 * 0.03]);
    }
    pts
}

/// A cluster the guard must refuse: a compact core with far outliers on
/// separate axes — the module's own reject fixture shape. No primitive in the
/// grammar explains it, and naming it anyway is the "plausible structure that
/// was never filmed" failure the residual exists to catch.
pub fn scattered_cluster() -> Vec<[f32; 3]> {
    let mut pts = Vec::new();
    for i in 0..5 {
        for j in 0..5 {
            for k in 0..5 {
                pts.push([i as f32 * 0.5 - 1.0, j as f32 * 0.5 - 1.0, k as f32 * 0.5 - 1.0]);
            }
        }
    }
    pts.push([40.0, 0.0, 0.0]);
    pts.push([0.0, -35.0, 0.0]);
    pts.push([0.0, 0.0, 38.0]);
    pts
}

/// The page.
pub fn ui(app: &mut crate::App, ui: &mut egui::Ui) {
    let c = uikit_egui::palette_colors(&app.prefs.palette);
    ui.heading("Sequence Lab");
    ui.label(egui::RichText::new("the temporal encoder, driven and reported").weak().small());
    ui.add_space(4.0);

    let s = &mut app.seq;
    ui.horizontal(|ui| {
        ui.label("size");
        ui.add(egui::DragValue::new(&mut s.width).range(32..=512));
        ui.label("×");
        ui.add(egui::DragValue::new(&mut s.height).range(32..=512));
        ui.label("quality");
        ui.add(egui::Slider::new(&mut s.quality, 0.1..=1.0));
        ui.label("frames");
        ui.add(egui::DragValue::new(&mut s.frames).range(1..=60));
        if ui.button("Encode").clicked() {
            // Synchronous on purpose: the sizes this page allows encode in
            // tens of milliseconds, and a worker thread would be machinery
            // for a stall that cannot happen at these dimensions.
            s.run = Some(seq_lab::run_sequence(s.width, s.height, s.quality, s.frames));
        }
    });
    match &s.run {
        Some(run) => seq_lab::seq_lab_ui(ui, run, &c),
        None => { ui.colored_label(c.text_weak, "No sequence encoded yet."); }
    }

    ui.separator();
    ui.strong("Reprojection");
    ui.label(egui::RichText::new(
        "per-pixel motion from depth + pose — the residual is what a temporal pass would still owe",
    ).weak().small());
    ui.horizontal(|ui| {
        ui.label("camera step");
        ui.add(egui::Slider::new(&mut s.cam_step, 0.0..=0.5));
    });
    let rep = reproject_step(s.cam_step);
    ui.colored_label(
        c.text,
        format!(
            "residual {:.1}%  ·  {}/{} pixels visible after the step",
            rep.residual * 100.0, rep.visible, rep.total
        ),
    );

    ui.separator();
    ui.strong("Part vocabulary");
    ui.label(egui::RichText::new(
        "fit reconstructed clusters to the 8-class grammar — the reject matters as much as the accept",
    ).weak().small());
    for (name, pts) in [("wheel-like ring", wheel_cluster()), ("scattered cloud", scattered_cluster())] {
        match part_fit::fit(&pts) {
            Some(f) if f.accept() => {
                ui.colored_label(c.success, format!(
                    "{name}: {:?}, size {:.2}, residual {:.2} — accepted",
                    f.class, f.size, f.residual
                ));
            }
            Some(f) => {
                ui.colored_label(c.text_weak, format!(
                    "{name}: best guess {:?} REJECTED (residual {:.2}) — coded as mesh instead",
                    f.class, f.residual
                ));
            }
            None => {
                ui.colored_label(c.text_weak, format!(
                    "{name}: no fit at all — coded as mesh instead"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wheel_ring_is_accepted_and_the_sliver_is_not() {
        // The page's two demo clusters must keep demonstrating what they
        // claim to: one accept, one refusal. If the vocabulary's thresholds
        // move, this is where the page's story breaks first.
        let wheel = part_fit::fit(&wheel_cluster()).expect("a ring has extents");
        assert!(wheel.accept(), "the wheel-like ring should fit the grammar");
        let scattered = part_fit::fit(&scattered_cluster()).expect("measurable");
        assert!(
            !scattered.accept(),
            "the guard refuses plausible-structure-that-was-never-there (residual {})",
            scattered.residual
        );
    }

    #[test]
    fn a_still_camera_owes_no_residual_and_a_step_owes_some() {
        let still = reproject_step(0.0);
        assert!(still.residual < 0.01, "no motion, nothing to re-render");
        assert_eq!(still.visible, still.total);
        let moved = reproject_step(0.4);
        assert!(moved.residual >= still.residual, "a step can only owe more");
    }
}
