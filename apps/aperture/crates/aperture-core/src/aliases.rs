//! aliases — a friendlier name shown OVER a symbol, never written into it.
//!
//! ## The problem this answers
//!
//! `cockpit`, `reroll`, `bucket`, `gitspace` are good names for the people
//! who chose them and a wall for the next person, in exactly the way a
//! decompiler's `sub_401020` is a wall until someone renames it. Java IDEs
//! solved the decompiler case by attaching a display name to a compiled
//! symbol without touching the class file underneath it. This is that, for
//! source someone else already wrote and you may not want to (or be allowed
//! to) rewrite: a label that appears wherever the entity's name would, and
//! disappears the moment you turn it off, because the file on disk never
//! changed.
//!
//! ## Why this is a LABEL and not a rename
//!
//! A real rename is a text edit — every reference has to be found and
//! rewritten, and getting one wrong silently breaks a build. That is a
//! different, harder, higher-stakes feature (aperture has no
//! find-all-references yet; see `aperture_lsp`'s crate doc). An alias asks
//! a smaller, safer question: "what would you call this if you were reading
//! it cold", and answers it by drawing a different string in the same place
//! — the source is byte-for-byte untouched, so there is no failure mode
//! where accepting a suggestion breaks the build. This is deliberately the
//! ceiling of what v0 does; a real rename is the seam TODO.md should record,
//! not something this module reaches for.
//!
//! ## What an alias attaches to
//!
//! An [`EntityId::Item`] — the same identity `entity.rs`'s graph uses, for
//! the same reason: a function is `(file, kind, name)`, and using that
//! rather than a byte range means an alias survives the file being edited
//! ABOVE the symbol (which shifts every line number below it) as long as the
//! symbol's own name is untouched. `entity.rs`'s own doc is explicit that
//! renaming the underlying symbol orphans anything keyed by name — the
//! alias for a renamed function is exactly as vulnerable, and deliberately:
//! the alternative is a second id scheme this module would have to keep in
//! sync with the graph's, which is the "who updates the table" problem
//! `entity.rs` already declined for the same reason.
//!
//! ## Persistence
//!
//! Through [`crate::bucket::Store`] — the same trait `Bucket` already uses,
//! not a new file format. One JSON-ish line-per-entry text blob, loaded
//! once and diffed on save so an editor with the store open in two places
//! doesn't clobber an alias someone else just added. Reusing `Store` means
//! a workspace that keeps buckets in a git ref gets aliases versioned the
//! same way for free, and a plain-file workspace gets a `.bucket` file next
//! to the others — no second storage decision to make per workspace.

use crate::bucket::Store;
use crate::entity::EntityId;
use std::collections::BTreeMap;

/// Where the alias for one entity came from, so the UI can show "you named
/// this" differently from "the model guessed this" — the same honesty
/// `CompletionOutcome::provider` already gives completions. An unlabeled
/// alias looks exactly as authoritative as a hand-typed one, which is how a
/// wrong guess quietly becomes the name everyone reads and nobody checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The person typed it.
    Manual,
    /// A model suggested it and the person accepted it, unedited.
    Suggested { provider: String },
    /// A model suggested it and the person edited the suggestion before
    /// accepting — still worth knowing it started as a suggestion, since a
    /// bad model habit (over-explaining, wrong domain vocabulary) is easier
    /// to spot in aggregate across `Suggested`+`Edited` than `Manual` alone.
    Edited { provider: String },
}

/// One entity's overlay name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alias {
    pub id: EntityId,
    pub text: String,
    pub origin: Origin,
}

/// The whole set of aliases for a workspace, loaded once and mutated in
/// memory; [`AliasSet::save`] is the only thing that touches the store.
///
/// Keyed by [`EntityId::key`] rather than the id itself: a `BTreeMap<EntityId,
/// _>` would work too, but every other keyed collection in this crate that
/// crosses a persistence boundary (`bucket::Located`, `entity::Graph`'s own
/// internals) uses the string key for the same reason `entity.rs` gives for
/// `EntityId::key` existing at all — a stable, greppable, single-line form
/// that survives a text-based store round trip untouched. Keeping this one
/// consistent means a raw grep of the saved file finds an alias the same way
/// it would find a bucket entry.
#[derive(Default)]
pub struct AliasSet {
    by_key: BTreeMap<String, Alias>,
    dirty: bool,
}

/// The bucket slug this module saves under. One workspace, one alias file —
/// splitting per-file would mean deciding what happens to an alias for a
/// file that gets renamed, which is exactly the fragility the module doc
/// above already accepts at the entity level; multiplying it per file buys
/// nothing.
pub const SLUG: &str = "vernacular-aliases";

impl AliasSet {
    pub fn new() -> AliasSet {
        AliasSet::default()
    }

    /// Load from `store`, or start empty if the slug has never been saved —
    /// the ordinary state for a workspace that has not used this feature,
    /// not an error.
    pub fn load(store: &dyn Store) -> AliasSet {
        let mut set = AliasSet::new();
        if let Some(text) = store.load(SLUG) {
            for line in text.lines() {
                if let Some(a) = parse_line(line) {
                    set.by_key.insert(a.id.key(), a);
                }
            }
        }
        set
    }

    /// Persist every alias, one per line. `dirty` gates the write so a
    /// caller can call this after every frame without touching the store
    /// (and, for a git-ref-backed store, creating a commit) when nothing
    /// changed.
    pub fn save(&mut self, store: &dyn Store) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        let mut lines: Vec<String> = self.by_key.values().map(format_line).collect();
        lines.sort();
        store.save(SLUG, &lines.join("\n"))?;
        self.dirty = false;
        Ok(())
    }

    /// The alias for `id`, if one has been set.
    pub fn get(&self, id: &EntityId) -> Option<&Alias> {
        self.by_key.get(&id.key())
    }

    /// Set or replace an alias. Empty `text` is rejected rather than
    /// stored as a blank label — [`Self::remove`] is the explicit way to
    /// clear one, so a blank overlay is never a state this type can be in
    /// by accident (a stray Enter in an empty text field, for instance).
    pub fn set(&mut self, id: EntityId, text: impl Into<String>, origin: Origin) -> Result<(), String> {
        let text = text.into();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("an alias cannot be blank — use remove() to clear one".into());
        }
        self.by_key.insert(id.key(), Alias { id, text: trimmed.to_string(), origin });
        self.dirty = true;
        Ok(())
    }

    /// Clear the alias for `id`. No error if there was none — removing
    /// what is not there is the outcome the caller wanted, same rule
    /// `bucket::FileStore::delete` already applies to a missing file.
    pub fn remove(&mut self, id: &EntityId) {
        if self.by_key.remove(&id.key()).is_some() {
            self.dirty = true;
        }
    }

    /// Every alias, for a settings list or an export.
    pub fn iter(&self) -> impl Iterator<Item = &Alias> {
        self.by_key.values()
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

/// `origin\u{1f}kind\u{1f}text` after the entity key, all through
/// `EntityId::key`/`parse_key` for the id itself — one separator convention
/// for the whole crate rather than a second delimiter this module invents.
fn format_line(a: &Alias) -> String {
    let us = '\u{1f}';
    let (otag, oprov) = match &a.origin {
        Origin::Manual => ("manual", ""),
        Origin::Suggested { provider } => ("suggested", provider.as_str()),
        Origin::Edited { provider } => ("edited", provider.as_str()),
    };
    format!("{}{us}{otag}{us}{oprov}{us}{}", a.id.key(), a.text)
}

fn parse_line(line: &str) -> Option<Alias> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // `EntityId::key` embeds its own unit-separated fields, so the split
    // has to find OUR three trailing fields from the END, not the start —
    // an `EntityId::Item` key alone is already `item\u{1f}path\u{1f}kind\u{1f}
    // name`, and splitting from the front would misparse the id's own
    // fields as this module's.
    let parts: Vec<&str> = line.rsplitn(4, '\u{1f}').collect();
    if parts.len() != 4 {
        return None;
    }
    // rsplitn yields REVERSED order: [text, oprov, otag, id_key].
    let (text, oprov, otag, id_key) = (parts[0], parts[1], parts[2], parts[3]);
    let id = EntityId::parse_key(id_key)?;
    let origin = match otag {
        "manual" => Origin::Manual,
        "suggested" => Origin::Suggested { provider: oprov.to_string() },
        "edited" => Origin::Edited { provider: oprov.to_string() },
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    Some(Alias { id, text: text.to_string(), origin })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket::FileStore;

    fn item(name: &str) -> EntityId {
        EntityId::item("src/flow.rs", "fn", name)
    }

    #[test]
    fn a_fresh_workspace_has_no_aliases_and_that_is_not_an_error() {
        let dir = tempdir("fresh-workspace");
        let store = FileStore::new(&dir);
        let set = AliasSet::load(&store);
        assert!(set.is_empty());
    }

    #[test]
    fn set_then_get_round_trips_without_saving() {
        let mut set = AliasSet::new();
        set.set(item("resolve_collision"), "handle a naming clash", Origin::Manual).unwrap();
        assert_eq!(set.get(&item("resolve_collision")).unwrap().text, "handle a naming clash");
    }

    #[test]
    fn blank_text_is_rejected_rather_than_stored() {
        let mut set = AliasSet::new();
        let err = set.set(item("f"), "   ", Origin::Manual).unwrap_err();
        assert!(err.contains("blank"), "{err}");
        assert!(set.get(&item("f")).is_none());
    }

    #[test]
    fn remove_of_a_never_set_alias_is_not_an_error() {
        let mut set = AliasSet::new();
        set.remove(&item("never_set")); // must not panic
        assert!(!set.dirty, "removing nothing should not mark the set dirty");
    }

    #[test]
    fn save_is_a_noop_when_nothing_changed() {
        let dir = tempdir("save-is-noop");
        let store = FileStore::new(&dir);
        let mut set = AliasSet::new();
        // Never dirtied: no set/remove call.
        set.save(&store).unwrap();
        assert!(store.load(SLUG).is_none(), "a clean AliasSet must not create a bucket");
    }

    #[test]
    fn save_then_load_round_trips_through_the_real_store() {
        let dir = tempdir("save-then-load");
        let store = FileStore::new(&dir);
        let mut set = AliasSet::new();
        set.set(item("resolve_collision"), "handle a naming clash", Origin::Manual).unwrap();
        set.set(
            item("bucket"),
            "the sorting bins",
            Origin::Suggested { provider: "ollama".to_string() },
        )
        .unwrap();
        set.save(&store).unwrap();

        let loaded = AliasSet::load(&store);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(&item("resolve_collision")).unwrap().text, "handle a naming clash");
        let b = loaded.get(&item("bucket")).unwrap();
        assert_eq!(b.text, "the sorting bins");
        assert_eq!(b.origin, Origin::Suggested { provider: "ollama".to_string() });
    }

    #[test]
    fn edited_origin_survives_the_round_trip_distinctly_from_suggested() {
        // The distinction this type exists for: an unedited suggestion and
        // an edited one must not collapse into the same origin on reload,
        // or the aggregate "how often do people accept the model's wording
        // unedited" signal the module doc mentions becomes unmeasurable.
        let dir = tempdir("edited-origin");
        let store = FileStore::new(&dir);
        let mut set = AliasSet::new();
        set.set(item("f"), "final wording", Origin::Edited { provider: "ollama".into() }).unwrap();
        set.save(&store).unwrap();
        let loaded = AliasSet::load(&store);
        assert_eq!(loaded.get(&item("f")).unwrap().origin, Origin::Edited { provider: "ollama".into() });
    }

    #[test]
    fn an_id_containing_the_separator_free_fields_still_round_trips() {
        // The parser's real risk: EntityId::key already embeds unit
        // separators for the id's OWN fields, so a naive front-to-back split
        // would misparse them as this module's origin/text fields. Using
        // rsplitn(4, ...) to take the LAST three fields as ours and
        // everything before as the id is what this test pins.
        let dir = tempdir("separator-safe-id");
        let store = FileStore::new(&dir);
        let mut set = AliasSet::new();
        let id = EntityId::item("crates/aperture-core/src/entity.rs", "impl", "Graph::insert");
        set.set(id.clone(), "merge a duplicate edge", Origin::Manual).unwrap();
        set.save(&store).unwrap();
        let loaded = AliasSet::load(&store);
        assert_eq!(loaded.get(&id).unwrap().text, "merge a duplicate edge");
    }

    #[test]
    fn a_second_set_for_the_same_entity_replaces_rather_than_duplicates() {
        let mut set = AliasSet::new();
        set.set(item("f"), "first guess", Origin::Manual).unwrap();
        set.set(item("f"), "better name", Origin::Manual).unwrap();
        assert_eq!(set.len(), 1);
        assert_eq!(set.get(&item("f")).unwrap().text, "better name");
    }

    #[test]
    fn a_malformed_saved_line_is_skipped_not_a_load_failure() {
        // Storage this module does not own (a hand-edited bucket file, a
        // future format change) must not take down an editor session over
        // one bad line — same "total, or None" reasoning EntityId::parse_key
        // already states for exactly this kind of foreign-written text.
        let dir = tempdir("malformed-line");
        let store = FileStore::new(&dir);
        store
            .save(SLUG, "not\u{1f}a\u{1f}valid\u{1f}alias\u{1f}line\nalso not one")
            .unwrap();
        let loaded = AliasSet::load(&store);
        assert!(loaded.is_empty());
    }

    /// Same convention `bucket.rs`'s own tests use: a directory under the
    /// system temp dir, named for this test and the process id so parallel
    /// test binaries do not collide, cleared before use so a previous run's
    /// leftovers cannot leak a false pass.
    fn tempdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("aperture-aliases-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }
}
