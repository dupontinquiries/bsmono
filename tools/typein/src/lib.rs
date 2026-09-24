//! typein — type text and press keys through dotool, under whatever keyboard
//! layout the compositor happens to be using.
//!
//! dotool sends KEYCODES through a uinput device, and the compositor maps
//! them with its own xkb layout. When the two disagree you get the right keys
//! in the wrong places: `type warp` under a Dvorak compositor arrives as
//! `,apl`, because the QWERTY positions of w, a, r, p are , a p l there.
//!
//! dotool can simulate a chosen layout (`DOTOOL_XKB_LAYOUT`,
//! `DOTOOL_XKB_VARIANT`); it only needs to be told which. [`layout`] finds
//! that out, [`keyspec`] turns a command line into dotool's script, and the
//! binary joins them. Both halves are pure and tested; only the binary
//! touches the process or the environment.
pub mod keyspec;
pub mod layout;
