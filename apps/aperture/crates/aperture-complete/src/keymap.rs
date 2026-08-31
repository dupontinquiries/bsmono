//! keymap — which key accepts which block, and letting a person change it.
//!
//! ## Why this is not just a constant
//!
//! `HOME_KEYS` is `['f','j','d','k','s','l','a',';']` — a sensible
//! default and a wrong one for several real people. A Dvorak layout puts
//! entirely different letters under the home row. An AZERTY keyboard has
//! no `;` there at all. Someone using a split ergonomic board may have
//! four comfortable keys per hand rather than four fingers' worth.
//!
//! A hardcoded map is fine until it is not, and when it is not there is
//! nothing to be done about it.
//!
//! ## The rules, and why each exists
//!
//! * **No duplicates.** Two blocks on one key means one of them is
//!   unreachable, and which one depends on iteration order — so it would
//!   look intermittent.
//! * **No key the editor already owns.** Binding `i` here would make
//!   entering insert mode accept a completion instead. The refusal names
//!   the conflict rather than silently dropping the key, because a
//!   binding that vanishes without explanation reads as a broken editor.
//! * **An empty map is legal.** Someone who wants the menu but prefers
//!   to scroll it should be able to say so, and `assign_keys` already
//!   handles blocks with no key — they are shown without one.
//!
//! ## Why the order matters more than the set
//!
//! The first key binds the first insertable block, which is the one the
//! model most likely meant. So the order is the ergonomics: strongest
//! finger first. That is why this is a `Vec<char>` and not a set, and
//! why editing is reorder-and-replace rather than add-and-remove.

/// Keys the modal editor owns, which a completion may not take.
///
/// Not exhaustive by design — it lists the ones whose loss would be
/// CONFUSING rather than merely inconvenient. `x` deleting a character
/// is muscle memory; a rarely-used binding shadowed by a completion key
/// is a trade a person can reasonably choose to make, and refusing every
/// letter in the keymap would leave almost nothing bindable.
pub const RESERVED: &[char] = &[
    'h', 'j', 'k', 'l', // motion
    'i', 'a', 'o', // insert
    'd', 'c', 'x', 'y', 'p', // edit
    'v', // select
    'u', // undo
    ':', '/', // command and search
];

/// The default map. `HOME_KEYS`, in the order that assigns the strongest
/// fingers to the likeliest blocks.
pub fn default_keys() -> Vec<char> {
    crate::multipass::HOME_KEYS.to_vec()
}

/// Why a key was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Already in the map. Names the position so a UI can point at it.
    Duplicate(usize),
    /// The editor owns it.
    Reserved(char),
    /// Not a key a person can press deliberately and see.
    Unusable(char),
}

impl Refused {
    /// What to show. Says WHAT and WHY in one line, because a refusal a
    /// person cannot act on is the same as a refusal with no message.
    pub fn message(&self) -> String {
        match self {
            Refused::Duplicate(i) => {
                format!("already bound at position {}", i + 1)
            }
            Refused::Reserved(c) => {
                format!("'{c}' is an editor motion — binding it would shadow that")
            }
            Refused::Unusable(c) => {
                format!("{c:?} cannot be a binding")
            }
        }
    }
}

/// An editable binding order.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyMap {
    keys: Vec<char>,
}

impl Default for KeyMap {
    fn default() -> Self {
        KeyMap { keys: default_keys() }
    }
}

impl KeyMap {
    pub fn new() -> KeyMap {
        KeyMap::default()
    }

    /// An empty map: the menu still appears, nothing is bound.
    pub fn empty() -> KeyMap {
        KeyMap { keys: Vec::new() }
    }

    pub fn keys(&self) -> &[char] {
        &self.keys
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// May this key be added?
    pub fn check(&self, c: char) -> Result<(), Refused> {
        // Control characters and whitespace: a person cannot press them
        // deliberately and see what happened, and space in particular is
        // scroll in most menus.
        if c.is_control() || c.is_whitespace() {
            return Err(Refused::Unusable(c));
        }
        if let Some(i) = self.keys.iter().position(|k| *k == c) {
            return Err(Refused::Duplicate(i));
        }
        if RESERVED.contains(&c) {
            return Err(Refused::Reserved(c));
        }
        Ok(())
    }

    /// Append a key, or say why not.
    pub fn push(&mut self, c: char) -> Result<(), Refused> {
        self.check(c)?;
        self.keys.push(c);
        Ok(())
    }

    /// Remove the key at `i`. Out of range is a no-op rather than a
    /// panic: this is driven by a UI whose list can change under it.
    pub fn remove(&mut self, i: usize) -> Option<char> {
        if i < self.keys.len() {
            Some(self.keys.remove(i))
        } else {
            None
        }
    }

    /// Move a binding earlier or later.
    ///
    /// Order IS the ergonomics — the first key takes the likeliest block
    /// — so reordering is the main edit, not a nicety.
    pub fn move_by(&mut self, i: usize, delta: isize) -> bool {
        let n = self.keys.len();
        if i >= n {
            return false;
        }
        let j = (i as isize + delta).clamp(0, n as isize - 1) as usize;
        if i == j {
            return false;
        }
        let k = self.keys.remove(i);
        self.keys.insert(j, k);
        true
    }

    pub fn reset(&mut self) {
        self.keys = default_keys();
    }

    /// Parse a saved map, ignoring anything no longer legal.
    ///
    /// Lenient rather than strict: a settings file written when `d` was
    /// bindable must still load after `d` becomes reserved, and refusing
    /// the whole map would cost a person every binding they had chosen
    /// because one is now invalid.
    pub fn parse(s: &str) -> KeyMap {
        let mut m = KeyMap::empty();
        for c in s.chars() {
            let _ = m.push(c);
        }
        m
    }

    /// Serialise. Just the characters — the order is the content.
    pub fn render(&self) -> String {
        self.keys.iter().collect()
    }
}

/// Assign this map's keys to the insertable blocks.
///
/// The `multipass::assign_keys` rule, with the keys supplied: extras keep
/// `None` and are shown without one, because wrapping round would give
/// two blocks the same key — worse than a block you have to scroll to.
pub fn assign(blocks: &mut [crate::multipass::Block], map: &KeyMap) {
    let mut keys = map.keys().iter();
    for b in blocks.iter_mut() {
        if b.kind.insertable() {
            b.key = keys.next().copied();
        } else {
            // Cleared, not left alone. Re-assigning after an edit must
            // not leave a stale key on a block the new map cannot reach.
            b.key = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_home_row_in_order() {
        assert_eq!(KeyMap::new().keys(), crate::multipass::HOME_KEYS);
    }

    #[test]
    fn a_duplicate_is_refused_and_says_where_the_first_one_is() {
        // Two blocks on one key means one is unreachable, and which one
        // depends on iteration order — so it would look intermittent.
        let mut m = KeyMap::empty();
        m.push('f').unwrap();
        m.push('q').unwrap();
        assert_eq!(m.push('f'), Err(Refused::Duplicate(0)));
        assert!(m.push('f').unwrap_err().message().contains("position 1"));
    }

    #[test]
    fn an_editor_motion_is_refused_by_name() {
        // Binding `i` would make entering insert mode accept a
        // completion instead. The message has to say which key and why,
        // or a person cannot act on it.
        let mut m = KeyMap::empty();
        let e = m.push('i').unwrap_err();
        assert_eq!(e, Refused::Reserved('i'));
        assert!(e.message().contains('i'), "{}", e.message());
        assert!(e.message().contains("motion"), "{}", e.message());
    }

    #[test]
    fn whitespace_and_control_characters_are_not_bindings() {
        // A person cannot press them deliberately and see what happened,
        // and space is scroll in most menus.
        let mut m = KeyMap::empty();
        for c in [' ', '\n', '\t'] {
            assert!(matches!(m.push(c), Err(Refused::Unusable(_))), "{c:?} accepted");
        }
    }

    #[test]
    fn an_empty_map_is_legal() {
        // Somebody who wants the menu but prefers to scroll should be
        // able to say so.
        let m = KeyMap::empty();
        assert!(m.is_empty());
        let mut blocks = crate::multipass::plan("```rust\nlet x = 1;\n```").0;
        assign(&mut blocks, &m);
        assert!(blocks.iter().all(|b| b.key.is_none()));
    }

    #[test]
    fn reordering_changes_which_block_gets_the_first_key() {
        // Order IS the ergonomics: the first key takes the likeliest
        // block, so this is the main edit rather than a nicety.
        let mut m = KeyMap::empty();
        m.push('q').unwrap();
        m.push('w').unwrap();
        assert!(m.move_by(1, -1));
        assert_eq!(m.keys(), &['w', 'q']);
    }

    #[test]
    fn moving_past_the_ends_clamps_rather_than_wrapping() {
        // A binding that jumps from first to last when nudged up would
        // be a surprise, and holding the key would cycle forever.
        let mut m = KeyMap::empty();
        m.push('q').unwrap();
        m.push('w').unwrap();
        assert!(!m.move_by(0, -1), "moving the first up should do nothing");
        assert!(!m.move_by(1, 5), "moving the last down should do nothing");
        assert_eq!(m.keys(), &['q', 'w']);
    }

    #[test]
    fn removing_out_of_range_is_a_no_op_not_a_panic() {
        // Driven by a UI whose list can change under it.
        let mut m = KeyMap::empty();
        assert_eq!(m.remove(3), None);
    }

    #[test]
    fn a_saved_map_survives_a_key_becoming_reserved() {
        // Refusing the whole map would cost a person every binding they
        // chose because one of them is now invalid.
        let m = KeyMap::parse("qidw");
        assert!(!m.keys().contains(&'i'), "the reserved key got in: {:?}", m.keys());
        assert_eq!(m.keys(), &['q', 'w'], "the legal keys were lost too");
    }

    #[test]
    fn a_map_round_trips_through_text() {
        let mut m = KeyMap::empty();
        for c in ['q', 'w', 'e'] {
            m.push(c).unwrap();
        }
        assert_eq!(KeyMap::parse(&m.render()), m);
    }

    #[test]
    fn reassigning_clears_a_stale_key_from_an_uninsertable_block() {
        // After an edit, a block the new map cannot reach must not keep
        // the key the old map gave it.
        let (mut blocks, _) = crate::multipass::plan("```rust\nlet x = 1;\n```");
        assign(&mut blocks, &KeyMap::new());
        assign(&mut blocks, &KeyMap::empty());
        assert!(blocks.iter().all(|b| b.key.is_none()), "{blocks:#?}");
    }

    #[test]
    fn plan_with_a_custom_map_uses_it() {
        // The end of the chain: a person edits the map and the menu
        // they see is bound the way they asked.
        let mut m = KeyMap::empty();
        m.push('q').unwrap();
        let (blocks, _) = crate::multipass::plan_with("```rust\nlet x = 1;\n```", &m);
        let first = blocks.iter().find(|b| b.kind.insertable()).expect("a code block");
        assert_eq!(first.key, Some('q'), "{blocks:#?}");
    }

    #[test]
    fn reset_restores_the_default() {
        let mut m = KeyMap::empty();
        m.push('q').unwrap();
        m.reset();
        assert_eq!(m.keys(), crate::multipass::HOME_KEYS);
    }
}
