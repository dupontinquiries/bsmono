//! spatial_audio — bridge logscope's 3D conversation-graph layout ([`crate::spatial`]) to a
//! bo-suite soundscape via `audiomix-kit`, so an "aggressive"/spatial view can *sound* where its
//! nodes sit: recent/near messages up front, deep threads to the rear, left/right by lateral
//! position. This is the mind-palace → soundscape step-up realized for the conversation graph.
//! Pure (no egui, no gui feature), so it builds/tests independent of the heavy front-end.

use crate::spatial::Vec3;
use audiomix_kit::{soundscape, Layout, Placement, RouteMatrix};

/// Project laid-out node positions to normalized 2D (x: left→right from world-x, y: front→rear
/// from world-z depth) and build per-node soundscape routing into `layout`. Positions are
/// normalized against the graph's own bounds so any layout scale maps into the room.
pub fn graph_soundscape(nodes: &[Vec3], ids: &[String], layout: Layout) -> Vec<(String, RouteMatrix)> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let (mut minx, mut maxx, mut minz, mut maxz) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    for n in nodes {
        minx = minx.min(n.x);
        maxx = maxx.max(n.x);
        minz = minz.min(n.z);
        maxz = maxz.max(n.z);
    }
    let sx = (maxx - minx).max(1e-3);
    let sz = (maxz - minz).max(1e-3);
    let placements: Vec<Placement> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| Placement {
            id: ids.get(i).cloned().unwrap_or_else(|| i.to_string()),
            x: ((n.x - minx) / sx).clamp(0.0, 1.0),
            y: ((n.z - minz) / sz).clamp(0.0, 1.0),
            level: 1.0,
        })
        .collect();
    soundscape(&placements, layout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_nodes_become_positioned_sources() {
        // three nodes spread on x; leftmost should route mostly to a left channel
        let nodes = vec![Vec3::new(-1.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.5)];
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let scene = graph_soundscape(&nodes, &ids, Layout::Quad);
        assert_eq!(scene.len(), 3);
        let (_, left) = &scene[0];
        assert!(left.gain(audiomix_kit::Channel::C, audiomix_kit::Channel::FL) > 0.5, "leftmost node favours front-left");
        assert!(graph_soundscape(&[], &[], Layout::Stereo).is_empty());
    }
}
