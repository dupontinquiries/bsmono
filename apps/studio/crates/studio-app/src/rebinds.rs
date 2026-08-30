//! rebinds — pressed key → effective key, per device, resolved once.
//!
//! The keymap (`studio_egui::keys`) is a modal map fed characters; the
//! shell's intercepted keys (`A`/`Q`/`,`/`.`/…) are a match on the same
//! characters. Customization therefore needs to change neither: a rebind
//! is a translation applied to RAW TYPED INPUT only, before either
//! consumer sees it. "Key X now does what Y did" is `map[X] = Y`.
//!
//! Three deliberate properties:
//!
//! * **One step, never a chain.** `resolve` is a single lookup, so a swap
//!   (`d`→`y`, `y`→`d`) means exactly the swap — a chain would make it a
//!   cycle and both keys would do the wrong thing.
//! * **Raw input only.** The palette's `Act::Key('d')` names the DEFAULT
//!   key as a semantic ("delete selection"), deliberately, so the palette
//!   cannot drift from the keyboard — and it must not be translated, or a
//!   rebind would silently repoint every palette verb that delegates. The
//!   shell applies rebinds in its typed-character loop and nowhere else.
//! * **The sidecar owns it.** A rebind is exactly as per-device as a zoom
//!   level — encoded as codepoint pairs (`100>121`), because the keys
//!   most worth rebinding (`,` `.` `<` `>`) are the ones that would break
//!   any punctuation-delimited format.
//!
//! What this does NOT do (named, so nobody reads more in): the hints
//! (`CLIP_HINTS`, palette hint column) still show DEFAULT keys. Making
//! every hint read through the rebind table is real work touching every
//! surface that prints a key, tracked as its own item — a stale hint is
//! bad, but a rebind that half-works is worse, and the model here is the
//! half that must exist first either way.

use std::collections::BTreeMap;

/// The translation table. Absent = identity, so the default is "nothing
/// rebound" at zero cost and zero storage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rebinds {
    map: BTreeMap<char, char>,
}

impl Rebinds {
    /// What a pressed key means. One lookup, never recursive.
    pub fn resolve(&self, ch: char) -> char {
        *self.map.get(&ch).unwrap_or(&ch)
    }

    /// `pressed` now does what `effective`'s default is. Setting a key to
    /// itself removes the entry — identity is the absence of a rebind,
    /// not a stored fact, so a sidecar never fills with no-ops.
    pub fn set(&mut self, pressed: char, effective: char) {
        if pressed == effective {
            self.map.remove(&pressed);
        } else {
            self.map.insert(pressed, effective);
        }
    }

    /// Back to the default for one key.
    pub fn clear(&mut self, pressed: char) { self.map.remove(&pressed); }

    /// Every pressed key currently resolving to `effective` — what the
    /// settings UI prints beside an action so "which key does delete NOW"
    /// has an answer. Includes `effective` itself unless it was rebound
    /// away to mean something else.
    pub fn keys_for(&self, effective: char) -> Vec<char> {
        let mut out: Vec<char> = self.map.iter()
            .filter(|(_, v)| **v == effective)
            .map(|(k, _)| *k)
            .collect();
        if !self.map.contains_key(&effective) {
            out.push(effective);
        }
        out.sort();
        out
    }

    pub fn is_empty(&self) -> bool { self.map.is_empty() }

    /// Codepoint pairs: `100>121,44>46`. Chosen over the characters
    /// themselves because `,` and `>` are nudge keys — the likeliest
    /// rebind candidates would be the format's own delimiters.
    pub fn encode(&self) -> String {
        self.map.iter()
            .map(|(k, v)| format!("{}>{}", *k as u32, *v as u32))
            .collect::<Vec<_>>().join(",")
    }

    /// Lenient like the rest of the sidecar: a garbled pair is skipped,
    /// never an error — a broken line must not cost the rest of the map.
    pub fn decode(s: &str) -> Rebinds {
        let mut r = Rebinds::default();
        for pair in s.split(',') {
            let Some((a, b)) = pair.split_once('>') else { continue };
            let (Ok(a), Ok(b)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>())
                else { continue };
            let (Some(a), Some(b)) = (char::from_u32(a), char::from_u32(b))
                else { continue };
            r.set(a, b);
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_until_told_otherwise_and_one_step_only() {
        let mut r = Rebinds::default();
        assert_eq!(r.resolve('d'), 'd');
        r.set('k', 'd');
        assert_eq!(r.resolve('k'), 'd', "k now deletes");
        assert_eq!(r.resolve('d'), 'd', "d still does, until IT is rebound");
        // The swap: each key does the other's job, and resolution must not
        // chain d→y→d back to where it started.
        r.set('d', 'y');
        r.set('y', 'd');
        assert_eq!(r.resolve('d'), 'y');
        assert_eq!(r.resolve('y'), 'd');
    }

    #[test]
    fn identity_is_absence_and_keys_for_answers_the_ui_question() {
        let mut r = Rebinds::default();
        r.set('k', 'd');
        assert_eq!(r.keys_for('d'), vec!['d', 'k'],
                   "both the default and the rebind reach delete");
        r.set('d', 'y');
        assert_eq!(r.keys_for('d'), vec!['k'],
                   "d was rebound away, so it no longer means delete");
        r.set('k', 'k');
        assert!(r.keys_for('d').is_empty(), "an action can be orphaned — the UI should say so");
        r.set('d', 'd');
        assert!(r.is_empty(), "identity sets are removals, not stored no-ops");
    }

    #[test]
    fn punctuation_survives_the_sidecar_and_garbage_costs_only_itself() {
        let mut r = Rebinds::default();
        r.set(',', '.');
        r.set('<', '>');
        r.set('k', 'd');
        assert_eq!(Rebinds::decode(&r.encode()), r,
                   "the nudge keys are exactly why the format is codepoints");
        let partial = Rebinds::decode("44>46,garbage,99>>,107>100,>,1114112>65");
        let mut want = Rebinds::default();
        want.set(',', '.');
        want.set('k', 'd');
        assert_eq!(partial, want, "each bad pair is skipped alone (incl. an out-of-range codepoint)");
        assert_eq!(Rebinds::decode(""), Rebinds::default());
    }
}
