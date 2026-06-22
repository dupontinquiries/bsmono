//! Debug inventory HUD — renders the hotbar by reusing the engine's existing
//! `debug_overlay` (the F4 overlay), so it needs no GUI framework, no new
//! dependency, and no renderer work. Each hotbar slot becomes a colored cube
//! floating in front of the camera; the selected slot gets a white outline.
//!
//! Cubes can't show stack counts, so [`hotbar_summary`] returns a one-line
//! text dump (name + count per slot) for the log — together they give a quick
//! visual *and* exact numbers while testing.
//!
//! This is throwaway testing scaffolding, not the real inventory UI.

use super::{Inventory, ItemId, PaletteEntry};

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn item_color(palette: &[PaletteEntry], item: ItemId) -> [f32; 3] {
    palette
        .iter()
        .find(|e| e.item == item)
        .map(|e| [e.color[0] as f32 / 255.0, e.color[1] as f32 / 255.0, e.color[2] as f32 / 255.0])
        .unwrap_or([0.8, 0.8, 0.8])
}

/// Add a row of cubes representing the hotbar to the debug overlay `scene`,
/// laid out in front of the camera. Call each frame while the overlay is on.
pub fn build_hotbar_markers(
    scene: &mut crate::debug_overlay::DebugScene,
    inv: &Inventory,
    palette: &[PaletteEntry],
    cam_pos: [f32; 3],
    cam_fwd: [f32; 3],
) {
    let fwd = norm(cam_fwd);
    // Camera-right from forward × world-up, guarding the near-vertical case.
    let mut right = cross(fwd, [0.0, 1.0, 0.0]);
    if right[0].abs() + right[1].abs() + right[2].abs() < 1e-4 {
        right = [1.0, 0.0, 0.0];
    }
    let right = norm(right);
    let up = norm(cross(right, fwd));

    let n = inv.hotbar_size();
    let dist = 2.5_f32; // blocks in front of the camera
    let spacing = 0.45_f32;
    let size = 0.32_f32;
    let vshift = -0.9_f32; // drop the row low into view, hotbar-style

    let base = [
        cam_pos[0] + fwd[0] * dist + up[0] * vshift,
        cam_pos[1] + fwd[1] * dist + up[1] * vshift,
        cam_pos[2] + fwd[2] * dist + up[2] * vshift,
    ];
    let center = (n as f32 - 1.0) * 0.5;

    for i in 0..n {
        let off = (i as f32 - center) * spacing;
        let pos = [
            base[0] + right[0] * off,
            base[1] + right[1] * off,
            base[2] + right[2] * off,
        ];
        let (color, sz) = match inv.slot(i) {
            Some(s) => (item_color(palette, s.item), size),
            None => ([0.25, 0.25, 0.28], size * 0.6),
        };
        scene.add_marker(pos, color, sz);
        if i == inv.selected() {
            let h = sz * 0.75;
            scene.add_aabb(
                [pos[0] - h, pos[1] - h, pos[2] - h],
                [pos[0] + h, pos[1] + h, pos[2] + h],
                [1.0, 1.0, 1.0],
                0.04,
            );
        }
    }
}

/// One-line text summary of the hotbar (selected slot marked with `*`), for
/// logging exact stack counts the cubes can't display.
pub fn hotbar_summary(inv: &Inventory, palette: &[PaletteEntry]) -> String {
    let mut parts = Vec::with_capacity(inv.hotbar_size());
    for i in 0..inv.hotbar_size() {
        let sel = if i == inv.selected() { "*" } else { "" };
        match inv.slot(i) {
            Some(s) => {
                let name = palette
                    .iter()
                    .find(|e| e.item == s.item)
                    .map(|e| e.name)
                    .unwrap_or("?");
                parts.push(format!("{}{}:{}x{}", sel, i + 1, name, s.count));
            }
            None => parts.push(format!("{}{}:-", sel, i + 1)),
        }
    }
    parts.join("  ")
}
