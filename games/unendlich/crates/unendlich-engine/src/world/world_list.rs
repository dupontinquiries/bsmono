//! `world_list` — the registry of known worlds plus the ruleset that governs WHEN a world may be
//! loaded. The immediate job (operator): make it impossible to have two worlds loaded at once.
//! The ruleset is an enum so the policy can grow later (per-permission, server-capacity, etc.);
//! for now the only — and default — rule is `NoConcurrent`: a load is denied while another world
//! is already loaded. Pure and host-agnostic so it is fully testable; the bin/server owns the
//! actual load/unload side effects and just consults this for the yes/no.

pub type WorldId = u64;

/// A world the host knows about (not necessarily loaded).
#[derive(Debug, Clone, PartialEq)]
pub struct WorldEntry {
    pub id: WorldId,
    pub name: String,
    pub seed: u64,
}

/// When is a world allowed to load? Extensible; `NoConcurrent` is the starting rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadRule {
    /// Only one world may be loaded at a time. A load is denied while another is loaded.
    NoConcurrent,
}

impl Default for LoadRule {
    fn default() -> Self {
        LoadRule::NoConcurrent
    }
}

/// Why a load was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadDenied {
    /// No such world is registered.
    UnknownWorld(WorldId),
    /// A different world is already loaded and the rule forbids concurrency.
    AnotherLoaded(WorldId),
    /// A whitelist is active and this world is not on it.
    NotWhitelisted(WorldId),
}

/// One recorded load request and its outcome — the "tracker" the host keeps so it can audit what
/// asked to load and whether it was granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadRequest {
    pub id: WorldId,
    pub granted: bool,
    pub denied: Option<LoadDenied>,
}

#[derive(Debug, Default)]
pub struct WorldList {
    worlds: Vec<WorldEntry>,
    loaded: Option<WorldId>,
    rule: LoadRule,
    /// If `Some`, only these worlds may load (a request whitelist). `None` = no whitelist (any
    /// registered world may load, subject to the concurrency rule).
    whitelist: Option<std::collections::HashSet<WorldId>>,
    /// Audit trail of every load attempt, in order.
    request_log: Vec<LoadRequest>,
}

impl WorldList {
    pub fn new() -> Self {
        Self {
            worlds: Vec::new(),
            loaded: None,
            rule: LoadRule::default(),
            whitelist: None,
            request_log: Vec::new(),
        }
    }

    /// Build with an explicit rule (forward-compat for when more rules exist).
    pub fn with_rule(rule: LoadRule) -> Self {
        let mut s = Self::new();
        s.rule = rule;
        s
    }

    pub fn rule(&self) -> LoadRule {
        self.rule
    }

    pub fn set_rule(&mut self, rule: LoadRule) {
        self.rule = rule;
    }

    /// Add `id` to the request whitelist (creating it if absent). Once a whitelist exists, only
    /// whitelisted worlds may load.
    pub fn allow(&mut self, id: WorldId) {
        self.whitelist.get_or_insert_with(Default::default).insert(id);
    }

    /// Remove `id` from the whitelist (no-op if there is no whitelist or it wasn't present).
    pub fn disallow(&mut self, id: WorldId) {
        if let Some(w) = self.whitelist.as_mut() {
            w.remove(&id);
        }
    }

    /// Drop the whitelist entirely (any registered world may load again).
    pub fn clear_whitelist(&mut self) {
        self.whitelist = None;
    }

    pub fn is_whitelisted(&self, id: WorldId) -> bool {
        match &self.whitelist {
            None => true,
            Some(w) => w.contains(&id),
        }
    }

    /// The recorded load attempts, oldest first.
    pub fn request_log(&self) -> &[LoadRequest] {
        &self.request_log
    }

    /// Register a world (idempotent on id — re-registering updates name/seed).
    pub fn register(&mut self, entry: WorldEntry) {
        if let Some(existing) = self.worlds.iter_mut().find(|w| w.id == entry.id) {
            *existing = entry;
        } else {
            self.worlds.push(entry);
        }
    }

    pub fn worlds(&self) -> &[WorldEntry] {
        &self.worlds
    }

    pub fn loaded(&self) -> Option<WorldId> {
        self.loaded
    }

    pub fn is_known(&self, id: WorldId) -> bool {
        self.worlds.iter().any(|w| w.id == id)
    }

    /// Would loading `id` be allowed right now? Pure check, no state change. Loading the world
    /// that's already loaded is a no-op success. Order: known → whitelisted → concurrency.
    pub fn can_load(&self, id: WorldId) -> Result<(), LoadDenied> {
        if !self.is_known(id) {
            return Err(LoadDenied::UnknownWorld(id));
        }
        if !self.is_whitelisted(id) {
            return Err(LoadDenied::NotWhitelisted(id));
        }
        match self.rule {
            LoadRule::NoConcurrent => match self.loaded {
                Some(cur) if cur != id => Err(LoadDenied::AnotherLoaded(cur)),
                _ => Ok(()),
            },
        }
    }

    /// Attempt to load `id`, enforcing the rule + whitelist. Records the attempt in the request
    /// log either way. On success the world becomes the loaded one.
    pub fn try_load(&mut self, id: WorldId) -> Result<WorldId, LoadDenied> {
        match self.can_load(id) {
            Ok(()) => {
                self.loaded = Some(id);
                self.request_log.push(LoadRequest { id, granted: true, denied: None });
                Ok(id)
            }
            Err(e) => {
                self.request_log.push(LoadRequest { id, granted: false, denied: Some(e) });
                Err(e)
            }
        }
    }

    /// Unload the current world (if any). Returns the id that was unloaded.
    pub fn unload(&mut self) -> Option<WorldId> {
        self.loaded.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: WorldId, name: &str) -> WorldEntry {
        WorldEntry { id, name: name.to_string(), seed: id }
    }

    #[test]
    fn unknown_world_denied() {
        let mut l = WorldList::new();
        assert_eq!(l.try_load(7), Err(LoadDenied::UnknownWorld(7)));
    }

    #[test]
    fn loads_a_registered_world() {
        let mut l = WorldList::new();
        l.register(w(1, "alpha"));
        assert_eq!(l.try_load(1), Ok(1));
        assert_eq!(l.loaded(), Some(1));
    }

    #[test]
    fn no_concurrent_blocks_second_world() {
        let mut l = WorldList::new();
        l.register(w(1, "alpha"));
        l.register(w(2, "beta"));
        assert_eq!(l.try_load(1), Ok(1));
        // the whole point: a second, different world cannot load while one is loaded
        assert_eq!(l.try_load(2), Err(LoadDenied::AnotherLoaded(1)));
        assert_eq!(l.loaded(), Some(1)); // unchanged
    }

    #[test]
    fn reloading_same_world_is_ok() {
        let mut l = WorldList::new();
        l.register(w(1, "alpha"));
        assert_eq!(l.try_load(1), Ok(1));
        assert_eq!(l.try_load(1), Ok(1)); // idempotent
    }

    #[test]
    fn unload_then_load_other_succeeds() {
        let mut l = WorldList::new();
        l.register(w(1, "alpha"));
        l.register(w(2, "beta"));
        l.try_load(1).unwrap();
        assert_eq!(l.unload(), Some(1));
        assert_eq!(l.loaded(), None);
        assert_eq!(l.try_load(2), Ok(2));
    }

    #[test]
    fn register_is_idempotent_on_id() {
        let mut l = WorldList::new();
        l.register(w(1, "alpha"));
        l.register(WorldEntry { id: 1, name: "alpha-renamed".into(), seed: 99 });
        assert_eq!(l.worlds().len(), 1);
        assert_eq!(l.worlds()[0].name, "alpha-renamed");
        assert_eq!(l.worlds()[0].seed, 99);
    }

    #[test]
    fn can_load_does_not_mutate() {
        let mut l = WorldList::new();
        l.register(w(1, "alpha"));
        l.register(w(2, "beta"));
        l.try_load(1).unwrap();
        assert!(l.can_load(2).is_err());
        assert_eq!(l.loaded(), Some(1)); // can_load is a pure query
    }

    #[test]
    fn whitelist_gates_loads() {
        let mut l = WorldList::new();
        l.register(w(1, "alpha"));
        l.register(w(2, "beta"));
        l.allow(1); // whitelist now active: only world 1
        assert_eq!(l.try_load(2), Err(LoadDenied::NotWhitelisted(2)));
        assert_eq!(l.try_load(1), Ok(1));
    }

    #[test]
    fn no_whitelist_allows_any_registered() {
        let mut l = WorldList::new();
        l.register(w(5, "gamma"));
        assert!(l.is_whitelisted(5)); // None whitelist → everything allowed
        assert_eq!(l.try_load(5), Ok(5));
    }

    #[test]
    fn clearing_whitelist_reopens() {
        let mut l = WorldList::new();
        l.register(w(1, "a"));
        l.register(w(2, "b"));
        l.allow(1);
        assert!(l.can_load(2).is_err());
        l.clear_whitelist();
        assert!(l.can_load(2).is_ok());
    }

    #[test]
    fn request_log_records_grants_and_denials() {
        let mut l = WorldList::new();
        l.register(w(1, "a"));
        l.register(w(2, "b"));
        l.try_load(1).unwrap(); // grant
        let _ = l.try_load(2); // deny (concurrent)
        let _ = l.try_load(99); // deny (unknown)
        let log = l.request_log();
        assert_eq!(log.len(), 3);
        assert!(log[0].granted && log[0].id == 1);
        assert_eq!(log[1].denied, Some(LoadDenied::AnotherLoaded(1)));
        assert_eq!(log[2].denied, Some(LoadDenied::UnknownWorld(99)));
    }
}
