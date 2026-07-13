//! deploy.rs — multi-model, multi-device DEPLOYMENT configuration.
//!
//! Replaces the single-model dropdown with a resolved *plan*: a set of model SLOTS, each a (model, role,
//! device, priority). This one structure backs two things at once — the tile-based picker (check the models
//! you want, assign each a role + device, order them by priority) and the KV cache (each slot is a separate
//! cache tenant, keyed by [`Slot::key`]).
//!
//! ROLES wire up tree / speculative work: a big **Target** verifies a small **Draft**'s guesses (see
//! `specdecode`), while **Delegate** (compression / small side-jobs) and **Embed** (vision / embeddings) run
//! alongside. DEVICES spread the load across the M2 Ultra's two dies or several machines. The UI produces a
//! [`Deployment`]; [`validate`](Deployment::validate) catches the ways a hand-built config goes wrong
//! (no target, two targets, a draft that isn't smaller, a device asked to hold more than it has) before the
//! engine ever loads a weight.
#![allow(dead_code)]

/// What a model does in the deployment. Retrieval/spec-decode branch on this, the cache does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Target,   // the main text model that produces committed tokens
    Draft,    // small model proposing tokens for the target to verify (speculative / tree work)
    Delegate, // small model for compression / summarization / cheap side tasks
    Embed,    // embeddings / vision encoder for text retrieval
    Image,    // image diffusion model (Z-Image / FLUX / SD) — a separate modality
    Voice,    // speech model (Whisper STT / a TTS) — a separate modality
}

impl Role {
    /// Text-generation roles that share the KV/inference engine (vs the separate image/voice modalities).
    pub fn is_text(self) -> bool { matches!(self, Role::Target | Role::Draft | Role::Delegate | Role::Embed) }
    pub fn label(self) -> &'static str {
        match self { Role::Target => "Target", Role::Draft => "Draft", Role::Delegate => "Delegate", Role::Embed => "Embed", Role::Image => "Image", Role::Voice => "Voice" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DeviceId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind { Gpu, Cpu, Remote } // Remote = a network host (an Ollama/other server reached over the LAN)

/// A compute device the deployment can place models on (a GPU die, a CPU, a remote host's accelerator).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub kind: DeviceKind,
    pub mem_bytes: u64, // usable memory budget for model weights + KV on this device
}

/// One model placed in the deployment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Slot {
    pub model: String,     // model name / store key
    pub role: Role,
    pub device: DeviceId,
    pub priority: u32,     // lower = higher priority (drives load order + the UI list order)
    pub enabled: bool,     // a checked-but-parked tile stays in the config but isn't loaded
    pub est_bytes: u64,    // estimated resident footprint (weights + working set) on its device
}

impl Slot {
    /// Stable cache-tenant key: a model on a device is one KV pool. (Same model on two devices = two pools.)
    pub fn key(&self) -> (String, u32) { (self.model.clone(), self.device.0) }
}

/// A resolved multi-model, multi-device plan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Deployment {
    pub devices: Vec<Device>,
    pub slots: Vec<Slot>,
}

/// A problem the UI should surface (in tile/validation styling) before the plan is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Issue {
    Empty,
    NoTarget,
    MultipleTargets(usize),
    DraftNotSmallerThanTarget,
    UnknownDevice(String),                                  // a slot points at a device not in `devices`
    DeviceOverBudget { device: DeviceId, used: u64, cap: u64 },
}

impl Deployment {
    pub fn new(devices: Vec<Device>) -> Self { Deployment { devices, slots: Vec::new() } }

    pub fn add(&mut self, slot: Slot) { self.slots.push(slot); }

    fn device(&self, id: DeviceId) -> Option<&Device> { self.devices.iter().find(|d| d.id == id) }

    /// Enabled slots, highest priority first (lowest `priority` value), then by model name for stability.
    pub fn enabled_by_priority(&self) -> Vec<&Slot> {
        let mut v: Vec<&Slot> = self.slots.iter().filter(|s| s.enabled).collect();
        v.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.model.cmp(&b.model)));
        v
    }

    /// The primary enabled Target slot. Several enabled Target slots are fine as long as they're all
    /// the SAME model on different devices (replicated for fanout speed, not competing targets) —
    /// returns the highest-priority one of those. Genuinely different models both marked Target is
    /// the actual ambiguous case, and returns `None` (surfaced by `validate` as `MultipleTargets`).
    pub fn target(&self) -> Option<&Slot> {
        let mut targets: Vec<&Slot> = self.slots.iter().filter(|s| s.enabled && s.role == Role::Target).collect();
        targets.sort_by_key(|s| s.priority);
        let first = *targets.first()?;
        if targets.iter().all(|s| s.model == first.model) { Some(first) } else { None }
    }

    /// Every enabled device currently running the Target model — what fanout dispatch (fuzz,
    /// summarize, …) round-robins or splits work across. A single-device deployment returns one slot,
    /// same as before; a model replicated on `this device` + a remote host returns both.
    pub fn target_slots(&self) -> Vec<&Slot> {
        let Some(t) = self.target() else { return Vec::new() };
        let mut v: Vec<&Slot> = self.slots.iter().filter(|s| s.enabled && s.role == Role::Target && s.model == t.model).collect();
        v.sort_by_key(|s| s.priority);
        v
    }

    /// The enabled Draft, if any (highest priority wins when several are checked).
    pub fn draft(&self) -> Option<&Slot> {
        self.slots.iter().filter(|s| s.enabled && s.role == Role::Draft).min_by_key(|s| s.priority)
    }

    /// The enabled Image (diffusion) model, if configured — a separate modality from the text stack.
    pub fn image(&self) -> Option<&Slot> {
        self.slots.iter().filter(|s| s.enabled && s.role == Role::Image).min_by_key(|s| s.priority)
    }

    /// The enabled Voice (speech) model, if configured.
    pub fn voice(&self) -> Option<&Slot> {
        self.slots.iter().filter(|s| s.enabled && s.role == Role::Voice).min_by_key(|s| s.priority)
    }

    /// The model currently assigned to a primary pipeline `role` (enabled), for the pipeline
    /// selectors (Large=Target, Small=Draft, Voice, Image).
    pub fn role_model(&self, role: Role) -> Option<&str> {
        let slot = match role {
            Role::Target => self.target(),
            Role::Draft => self.draft(),
            Role::Image => self.image(),
            Role::Voice => self.voice(),
            _ => self.slots.iter().filter(|s| s.enabled && s.role == role).min_by_key(|s| s.priority),
        };
        slot.map(|s| s.model.as_str())
    }

    /// Assign `model` to a single-slot pipeline `role`: update that role's slot in place, or add one.
    /// The pipeline selectors are single-select per role, so this keeps one slot per role. Empty
    /// `model` clears the role.
    pub fn set_role(&mut self, role: Role, model: &str, device: DeviceId, est_bytes: u64) {
        if model.is_empty() {
            self.clear_role(role);
            return;
        }
        match self.slots.iter_mut().find(|s| s.role == role) {
            Some(s) => {
                s.model = model.to_string();
                s.est_bytes = est_bytes;
                s.device = device;
                s.enabled = true;
            }
            None => {
                let priority = self.slots.len() as u32;
                self.add(Slot { model: model.to_string(), role, device, priority, enabled: true, est_bytes });
            }
        }
    }

    /// Remove every slot of `role` (deselect that pipeline stage).
    pub fn clear_role(&mut self, role: Role) {
        self.slots.retain(|s| s.role != role);
    }

    /// Devices of a given kind (e.g. list the Remote hosts for the network-device picker).
    pub fn devices_of(&self, kind: DeviceKind) -> Vec<&Device> {
        self.devices.iter().filter(|d| d.kind == kind).collect()
    }

    /// Total estimated resident bytes assigned to a device (enabled slots only).
    pub fn device_load(&self, id: DeviceId) -> u64 {
        self.slots.iter().filter(|s| s.enabled && s.device == id).map(|s| s.est_bytes).sum()
    }

    /// Whether speculative/tree decoding is configured (a target AND a smaller draft are both enabled).
    pub fn tree_ready(&self) -> bool {
        matches!((self.target(), self.draft()), (Some(t), Some(d)) if d.est_bytes < t.est_bytes)
    }

    /// Validate the plan; empty result = ready to load. Ordered roughly by severity.
    pub fn validate(&self) -> Vec<Issue> {
        let mut issues = Vec::new();
        let enabled: Vec<&Slot> = self.slots.iter().filter(|s| s.enabled).collect();
        if enabled.is_empty() {
            issues.push(Issue::Empty);
            return issues;
        }
        // Distinct Target MODELS, not raw slot count — the same model enabled as Target on two
        // devices (replicated for fanout speed) is one target, not two competing ones.
        let target_models: std::collections::HashSet<&str> = enabled.iter().filter(|s| s.role == Role::Target).map(|s| s.model.as_str()).collect();
        let has_text = enabled.iter().any(|s| s.role.is_text());
        match target_models.len() {
            0 if has_text => issues.push(Issue::NoTarget), // a text stack needs a Target; an image/voice-only plan doesn't
            0 => {}
            1 => {}
            n => issues.push(Issue::MultipleTargets(n)),
        }
        // a draft must be strictly smaller than the target, or speculation is pointless/negative
        if let (Some(t), Some(d)) = (
            enabled.iter().find(|s| s.role == Role::Target),
            enabled.iter().filter(|s| s.role == Role::Draft).min_by_key(|s| s.priority),
        ) {
            if d.est_bytes >= t.est_bytes {
                issues.push(Issue::DraftNotSmallerThanTarget);
            }
        }
        // every slot's device must exist, and no device may be over its memory budget
        for s in &enabled {
            if self.device(s.device).is_none() {
                issues.push(Issue::UnknownDevice(s.model.clone()));
            }
        }
        for d in &self.devices {
            let used = self.device_load(d.id);
            if used > d.mem_bytes {
                issues.push(Issue::DeviceOverBudget { device: d.id, used, cap: d.mem_bytes });
            }
        }
        issues
    }

    pub fn is_valid(&self) -> bool { self.validate().is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: u32, mem: u64) -> Device {
        Device { id: DeviceId(id), name: format!("gpu{id}"), kind: DeviceKind::Gpu, mem_bytes: mem }
    }
    fn slot(model: &str, role: Role, device: u32, priority: u32, est: u64) -> Slot {
        Slot { model: model.into(), role, device: DeviceId(device), priority, enabled: true, est_bytes: est }
    }

    #[test]
    fn a_well_formed_tree_deployment_validates() {
        let mut d = Deployment::new(vec![dev(0, 100), dev(1, 100)]);
        d.add(slot("big-target", Role::Target, 0, 0, 80));
        d.add(slot("small-draft", Role::Draft, 1, 1, 10)); // draft on the second die
        d.add(slot("tiny-delegate", Role::Delegate, 1, 2, 5));
        assert!(d.is_valid(), "{:?}", d.validate());
        assert_eq!(d.target().unwrap().model, "big-target");
        assert_eq!(d.draft().unwrap().model, "small-draft");
        assert!(d.tree_ready(), "target + smaller draft → speculation available");
    }

    #[test]
    fn same_target_model_replicated_across_devices_is_not_ambiguous() {
        // the exact bug report: the same model deployed on BOTH "this device" and a remote host, to
        // fan work out for speed — this is one target, not two competing ones.
        let mut d = Deployment::new(vec![dev(0, 100), dev(1, 100)]);
        d.add(slot("deepseek-r1", Role::Target, 0, 0, 40)); // this device
        d.add(slot("deepseek-r1", Role::Target, 1, 1, 40)); // remote host
        assert!(d.is_valid(), "replicating one target across devices must validate cleanly: {:?}", d.validate());
        assert_eq!(d.target().unwrap().model, "deepseek-r1");
        assert_eq!(d.target().unwrap().device, DeviceId(0), "the lowest-priority (first-added) slot wins as primary");
        let slots = d.target_slots();
        assert_eq!(slots.len(), 2, "fanout needs to see BOTH devices running the target");
        assert_eq!(slots.iter().map(|s| s.device).collect::<Vec<_>>(), vec![DeviceId(0), DeviceId(1)]);
    }

    #[test]
    fn genuinely_different_target_models_are_still_ambiguous() {
        let mut d = Deployment::new(vec![dev(0, 100), dev(1, 100)]);
        d.add(slot("model-a", Role::Target, 0, 0, 40));
        d.add(slot("model-b", Role::Target, 1, 1, 40)); // a DIFFERENT model, also marked Target
        assert!(!d.is_valid());
        assert!(d.validate().contains(&Issue::MultipleTargets(2)));
        assert!(d.target().is_none(), "two different models both claiming Target IS ambiguous");
    }

    #[test]
    fn disabling_one_replica_leaves_the_other_as_sole_target() {
        let mut d = Deployment::new(vec![dev(0, 100), dev(1, 100)]);
        d.add(slot("deepseek-r1", Role::Target, 0, 0, 40));
        d.add(slot("deepseek-r1", Role::Target, 1, 1, 40));
        d.slots[1].enabled = false; // toggle off the remote replica
        assert!(d.is_valid());
        assert_eq!(d.target_slots().len(), 1);
        assert_eq!(d.target_slots()[0].device, DeviceId(0));
    }

    #[test]
    fn priority_orders_the_list() {
        let mut d = Deployment::new(vec![dev(0, 1000)]);
        d.add(slot("c", Role::Delegate, 0, 3, 1));
        d.add(slot("a", Role::Target, 0, 1, 1));
        d.add(slot("b", Role::Draft, 0, 2, 1));
        let order: Vec<&str> = d.enabled_by_priority().iter().map(|s| s.model.as_str()).collect();
        assert_eq!(order, vec!["a", "b", "c"]);
    }

    #[test]
    fn missing_and_duplicate_targets_are_caught() {
        let mut d = Deployment::new(vec![dev(0, 1000)]);
        d.add(slot("draft-only", Role::Draft, 0, 0, 5));
        assert!(d.validate().contains(&Issue::NoTarget));
        d.add(slot("t1", Role::Target, 0, 1, 5));
        d.add(slot("t2", Role::Target, 0, 2, 5));
        assert!(d.validate().contains(&Issue::MultipleTargets(2)));
        assert!(d.target().is_none(), "ambiguous target resolves to none");
    }

    #[test]
    fn a_draft_not_smaller_than_the_target_is_flagged() {
        let mut d = Deployment::new(vec![dev(0, 1000)]);
        d.add(slot("target", Role::Target, 0, 0, 40));
        d.add(slot("fat-draft", Role::Draft, 0, 1, 40)); // not smaller
        assert!(d.validate().contains(&Issue::DraftNotSmallerThanTarget));
        assert!(!d.tree_ready());
    }

    #[test]
    fn a_device_over_its_memory_budget_is_flagged() {
        let mut d = Deployment::new(vec![dev(0, 50)]);
        d.add(slot("target", Role::Target, 0, 0, 40));
        d.add(slot("draft", Role::Draft, 0, 1, 20)); // 40 + 20 = 60 > 50
        let issues = d.validate();
        assert!(issues.iter().any(|i| matches!(i, Issue::DeviceOverBudget { device: DeviceId(0), used: 60, cap: 50 })), "{issues:?}");
        // spreading the draft to a second device fixes it
        let mut d2 = Deployment::new(vec![dev(0, 50), dev(1, 50)]);
        d2.add(slot("target", Role::Target, 0, 0, 40));
        d2.add(slot("draft", Role::Draft, 1, 1, 20));
        assert!(d2.is_valid());
    }

    #[test]
    fn disabled_tiles_dont_count() {
        let mut d = Deployment::new(vec![dev(0, 50)]);
        d.add(slot("target", Role::Target, 0, 0, 40));
        let mut parked = slot("extra", Role::Target, 0, 1, 40); // a second target...
        parked.enabled = false;                                  // ...but parked (unchecked)
        d.add(parked);
        assert!(d.is_valid(), "a disabled duplicate target doesn't break the plan");
        assert_eq!(d.device_load(DeviceId(0)), 40, "parked slot adds no load");
    }

    #[test]
    fn slot_key_partitions_by_model_and_device() {
        let a = slot("m", Role::Target, 0, 0, 1);
        let b = slot("m", Role::Draft, 1, 0, 1); // same model, different device → different cache tenant
        assert_ne!(a.key(), b.key());
        assert_eq!(a.key(), ("m".to_string(), 0));
    }

    #[test]
    fn image_and_voice_are_separate_modalities() {
        let mut d = Deployment::new(vec![dev(0, 1000)]);
        d.add(slot("chat", Role::Target, 0, 0, 40));
        d.add(slot("z-image-turbo", Role::Image, 0, 1, 7));
        d.add(slot("whisper", Role::Voice, 0, 2, 1));
        assert!(d.is_valid(), "{:?}", d.validate());
        assert_eq!(d.image().unwrap().model, "z-image-turbo");
        assert_eq!(d.voice().unwrap().model, "whisper");
        assert!(!Role::Image.is_text() && !Role::Voice.is_text());
        assert!(d.target().is_some());
    }

    #[test]
    fn image_or_voice_only_needs_no_target() {
        // a diffusion-only deployment (no chat model) is valid — Target is a text-stack requirement
        let mut d = Deployment::new(vec![dev(0, 1000)]);
        d.add(slot("z-image-turbo", Role::Image, 0, 0, 7));
        assert!(d.is_valid(), "image-only plan is valid: {:?}", d.validate());
        assert!(!d.validate().contains(&Issue::NoTarget));
    }

    #[test]
    fn remote_devices_are_listed_for_the_network_picker() {
        let devices = vec![
            dev(0, 1000),
            Device { id: DeviceId(1), name: "192.168.1.42:11434".into(), kind: DeviceKind::Remote, mem_bytes: 48_000_000_000 },
        ];
        let mut d = Deployment::new(devices);
        // run the target itself on the remote Ollama host
        d.add(Slot { model: "mixtral:8x7b".into(), role: Role::Target, device: DeviceId(1), priority: 0, enabled: true, est_bytes: 47_000_000_000 });
        let remotes = d.devices_of(DeviceKind::Remote);
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "192.168.1.42:11434");
        assert!(d.is_valid(), "a model placed on a remote host validates: {:?}", d.validate());
        assert_eq!(d.target().unwrap().device, DeviceId(1));
    }
}

#[cfg(test)]
mod pipeline_role_tests {
    use super::*;
    fn dep() -> Deployment {
        Deployment::new(vec![Device { id: DeviceId(0), name: "local".into(), kind: DeviceKind::Gpu, mem_bytes: 64_000_000_000 }])
    }
    #[test]
    fn set_and_clear_role_is_single_slot_per_role() {
        let mut d = dep();
        d.set_role(Role::Target, "big-70b", DeviceId(0), 40_000_000_000);
        d.set_role(Role::Draft, "small-1b", DeviceId(0), 1_000_000_000);
        d.set_role(Role::Voice, "whisper", DeviceId(0), 500_000_000);
        d.set_role(Role::Image, "z-image", DeviceId(0), 6_000_000_000);
        assert_eq!(d.role_model(Role::Target), Some("big-70b"));
        assert_eq!(d.role_model(Role::Draft), Some("small-1b"));
        assert_eq!(d.role_model(Role::Voice), Some("whisper"));
        assert_eq!(d.role_model(Role::Image), Some("z-image"));
        // reassigning a role updates in place — no second Target slot
        d.set_role(Role::Target, "big-8b", DeviceId(0), 8_000_000_000);
        assert_eq!(d.slots.iter().filter(|s| s.role == Role::Target).count(), 1);
        assert_eq!(d.role_model(Role::Target), Some("big-8b"));
        // clearing removes it
        d.clear_role(Role::Voice);
        assert_eq!(d.role_model(Role::Voice), None);
    }
}
