// src/layer_player.rs
//
// The layered-playback model: a pure, testable representation of "what is
// playing on each concurrent layer right now," plus the decisions for
// adding, removing, and reaping layers. This is the missing piece the
// queue_policy's Synchronize / MidgapLayer modes were waiting on — but
// kept deliberately free of any audio I/O so the bookkeeping is verifiable
// in isolation, exactly like queue_policy.
//
// HOW LAYERING WORKS (the mechanism this models):
//   PulseAudio mixes every stream pointed at a sink. So a second
//   `ffmpeg | paplay --device=<sink>` spawned while the first is still
//   running is heard *on top of* it — no special mixer needed. A "layer"
//   is therefore just one decode->paplay pipe, and concurrent layers are
//   concurrent pipes to the same sink. The catch is process bookkeeping:
//   the existing single-player path reaps ALL our ffmpeg/paplay children
//   before each play, which would murder the other layer. This model
//   tracks a stable id per layer so the driver can reap selectively
//   (everything-except-this-layer) instead of everything.
//
// LAYER ROLES:
//   * Top      - the primary track (what the transport bar controls). The
//                queue advances this layer.
//   * Overlay  - a Synchronize layer: starts time-aligned from 0 with the
//                Top, meant to be heard as one combined thing.
//   * Underlay - a MidgapLayer bed: enters at the Top's current playhead
//                and sits underneath, usually at reduced gain.
//
// There is at most one Top. There may be several Overlay/Underlay layers.

/// A layer's role in the mix. Drives default gain + whether the queue
/// advances it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerRole {
    Top,
    Overlay,
    Underlay,
}

impl LayerRole {
    /// The queue only advances the Top layer; overlay/underlay layers play
    /// their single track and then end.
    pub fn advances_queue(self) -> bool {
        matches!(self, LayerRole::Top)
    }

    /// A sensible default gain (0..=1) for the role. Underlays duck so the
    /// top stays intelligible; overlays play at unity (they're meant to be
    /// equal partners); top is unity.
    pub fn default_gain(self) -> f32 {
        match self {
            LayerRole::Top      => 1.0,
            LayerRole::Overlay  => 1.0,
            LayerRole::Underlay => 0.55,
        }
    }
}

/// One active layer. `id` is a stable handle the driver uses to address
/// this layer's process group (for selective reaping / stop). `offset_ms`
/// is where the layer started within its own track (0 for top/overlay;
/// the top's playhead for an underlay).
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    pub id: u64,
    pub track_id: i64,
    pub role: LayerRole,
    pub gain: f32,
    pub offset_ms: i64,
    pub paused: bool,
}

/// The set of currently-active layers. Pure state; the driver mirrors its
/// decisions into real processes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerSet {
    layers: Vec<Layer>,
    next_id: u64,
}

/// What the driver should DO as a result of a model change. The model
/// never spawns/kills anything; it emits these for the caller to execute.
#[derive(Debug, Clone, PartialEq)]
pub enum LayerAction {
    /// Spawn a decode->sink pipe for this layer (id carried for tracking).
    Spawn { id: u64, track_id: i64, offset_ms: i64, gain: f32, paused: bool },
    /// Stop the pipe(s) belonging to this layer id.
    Stop { id: u64 },
    /// Stop every layer EXCEPT this id (the selective reap that replaces
    /// "kill all ffmpeg/paplay" when a layered session is active).
    StopAllExcept { id: u64 },
}

impl LayerSet {
    pub fn new() -> Self { Self::default() }

    /// Number of active layers.
    pub fn len(&self) -> usize { self.layers.len() }
    pub fn is_empty(&self) -> bool { self.layers.is_empty() }

    /// The current Top layer, if any.
    pub fn top(&self) -> Option<&Layer> {
        self.layers.iter().find(|l| l.role == LayerRole::Top)
    }

    /// Read-only view of all layers.
    pub fn layers(&self) -> &[Layer] { &self.layers }

    fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Set (or replace) the Top layer with a track. Replacing the top
    /// stops the old top but leaves overlay/underlay layers alone (they're
    /// independent beds/overlays). Returns the actions to execute.
    pub fn set_top(&mut self, track_id: i64) -> Vec<LayerAction> {
        let mut actions = Vec::new();
        if let Some(pos) = self.layers.iter().position(|l| l.role == LayerRole::Top) {
            let old = self.layers.remove(pos);
            actions.push(LayerAction::Stop { id: old.id });
        }
        let id = self.alloc_id();
        self.layers.push(Layer {
            id, track_id, role: LayerRole::Top,
            gain: LayerRole::Top.default_gain(), offset_ms: 0, paused: false,
        });
        actions.push(LayerAction::Spawn {
            id, track_id, offset_ms: 0,
            gain: LayerRole::Top.default_gain(), paused: false,
        });
        actions
    }

    /// Add an Overlay layer (Synchronize): aligned from 0 with the top,
    /// mirroring the top's paused state so they stay locked.
    pub fn add_overlay(&mut self, track_id: i64, paused: bool) -> Vec<LayerAction> {
        let id = self.alloc_id();
        let gain = LayerRole::Overlay.default_gain();
        self.layers.push(Layer {
            id, track_id, role: LayerRole::Overlay, gain, offset_ms: 0, paused,
        });
        vec![LayerAction::Spawn { id, track_id, offset_ms: 0, gain, paused }]
    }

    /// Add an Underlay bed (MidgapLayer): enters at `offset_ms` (the top's
    /// playhead) at a ducked gain.
    pub fn add_underlay(&mut self, track_id: i64, offset_ms: i64) -> Vec<LayerAction> {
        let id = self.alloc_id();
        let gain = LayerRole::Underlay.default_gain();
        let off = offset_ms.max(0);
        self.layers.push(Layer {
            id, track_id, role: LayerRole::Underlay, gain, offset_ms: off, paused: false,
        });
        vec![LayerAction::Spawn { id, track_id, offset_ms: off, gain, paused: false }]
    }

    /// Remove one layer by id.
    pub fn remove(&mut self, id: u64) -> Vec<LayerAction> {
        if let Some(pos) = self.layers.iter().position(|l| l.id == id) {
            self.layers.remove(pos);
            vec![LayerAction::Stop { id }]
        } else {
            Vec::new()
        }
    }

    /// Collapse to a single track: stop every layer and start a fresh top.
    /// Used by Replace and by a plain play when a layered session is up.
    pub fn collapse_to(&mut self, track_id: i64) -> Vec<LayerAction> {
        let id = self.alloc_id();
        self.layers.clear();
        self.layers.push(Layer {
            id, track_id, role: LayerRole::Top,
            gain: 1.0, offset_ms: 0, paused: false,
        });
        vec![
            LayerAction::Spawn { id, track_id, offset_ms: 0, gain: 1.0, paused: false },
            LayerAction::StopAllExcept { id },
        ]
    }

    /// True when more than just a lone Top is active - i.e. the driver
    /// must reap selectively rather than "kill all".
    pub fn is_layered(&self) -> bool {
        self.layers.iter().filter(|l| l.role != LayerRole::Top).count() > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_gain_and_queue_rules() {
        assert!(LayerRole::Top.advances_queue());
        assert!(!LayerRole::Overlay.advances_queue());
        assert!(!LayerRole::Underlay.advances_queue());
        assert_eq!(LayerRole::Top.default_gain(), 1.0);
        assert!(LayerRole::Underlay.default_gain() < 1.0, "underlay ducks");
    }

    #[test]
    fn set_top_on_empty_spawns_one() {
        let mut s = LayerSet::new();
        let acts = s.set_top(10);
        assert_eq!(s.len(), 1);
        assert_eq!(s.top().unwrap().track_id, 10);
        assert_eq!(acts, vec![LayerAction::Spawn {
            id: 1, track_id: 10, offset_ms: 0, gain: 1.0, paused: false }]);
    }

    #[test]
    fn set_top_replaces_and_stops_old_top() {
        let mut s = LayerSet::new();
        s.set_top(10);
        let acts = s.set_top(20);
        assert_eq!(s.len(), 1, "still exactly one top");
        assert_eq!(s.top().unwrap().track_id, 20);
        assert!(acts.contains(&LayerAction::Stop { id: 1 }));
        assert!(acts.iter().any(|a| matches!(a, LayerAction::Spawn { id: 2, track_id: 20, .. })));
    }

    #[test]
    fn overlay_does_not_disturb_top() {
        let mut s = LayerSet::new();
        s.set_top(10);
        let acts = s.add_overlay(11, false);
        assert_eq!(s.len(), 2);
        assert_eq!(s.top().unwrap().track_id, 10, "top unchanged");
        assert_eq!(acts, vec![LayerAction::Spawn {
            id: 2, track_id: 11, offset_ms: 0, gain: 1.0, paused: false }]);
        assert!(s.is_layered());
    }

    #[test]
    fn overlay_mirrors_paused() {
        let mut s = LayerSet::new();
        s.set_top(10);
        let acts = s.add_overlay(11, true);
        assert!(matches!(acts[0], LayerAction::Spawn { paused: true, .. }));
    }

    #[test]
    fn underlay_enters_at_offset_and_ducks() {
        let mut s = LayerSet::new();
        s.set_top(10);
        let acts = s.add_underlay(12, 8000);
        match &acts[0] {
            LayerAction::Spawn { offset_ms, gain, .. } => {
                assert_eq!(*offset_ms, 8000);
                assert!(*gain < 1.0, "underlay ducks");
            }
            _ => panic!("expected spawn"),
        }
        let acts2 = s.add_underlay(13, -5);
        assert!(matches!(acts2[0], LayerAction::Spawn { offset_ms: 0, .. }));
    }

    #[test]
    fn remove_stops_only_that_layer() {
        let mut s = LayerSet::new();
        s.set_top(10);
        let overlay = s.add_overlay(11, false);
        let oid = match overlay[0] { LayerAction::Spawn { id, .. } => id, _ => 0 };
        let acts = s.remove(oid);
        assert_eq!(acts, vec![LayerAction::Stop { id: oid }]);
        assert_eq!(s.len(), 1);
        assert_eq!(s.top().unwrap().track_id, 10, "top survives");
        assert!(s.remove(9999).is_empty());
    }

    #[test]
    fn collapse_clears_all_and_starts_fresh_top() {
        let mut s = LayerSet::new();
        s.set_top(10);
        s.add_overlay(11, false);
        s.add_underlay(12, 3000);
        assert_eq!(s.len(), 3);
        let acts = s.collapse_to(99);
        assert_eq!(s.len(), 1);
        assert_eq!(s.top().unwrap().track_id, 99);
        assert!(!s.is_layered());
        assert!(acts.iter().any(|a| matches!(a, LayerAction::Spawn { track_id: 99, .. })));
        assert!(acts.iter().any(|a| matches!(a, LayerAction::StopAllExcept { .. })));
    }

    #[test]
    fn lone_top_is_not_layered() {
        let mut s = LayerSet::new();
        s.set_top(10);
        assert!(!s.is_layered(), "a single top is the normal single-player case");
    }
}
