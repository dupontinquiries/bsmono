//! Recorded matches as regression tests. Every other test in this crate builds
//! a fixture by hand — two warriors, a placed enemy, a held position — because a
//! whole match is tedious to construct. These are whole matches nobody had to
//! construct: thirteen kits, patrols, terrain, the economy and the AI, all
//! running together for half a minute, pinned to the digest they produced.
//!
//! When one of these fails, it does not mean the recording is wrong. It means
//! the sim changed, and the useful question is whether that change was on
//! purpose. Re-record with:
//!
//! ```sh
//! cargo run --release --example battleground_play -- lynx "ddd1ffwwa2" 120 \
//!     --record games/clawbound/tests/replays/snowhush_land0.replay
//! ```

use clawbound_rs::replay::{describe, Replay};

fn check(path: &str) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let r = Replay::from_text(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
    let got = r.play();
    assert_eq!(
        got, r.digest,
        "{path} no longer reproduces: {}\n  recorded {:016x}, replayed {:016x}",
        describe(&r),
        r.digest,
        got
    );
}

#[test]
fn a_recorded_match_still_plays_the_same_way() {
    check("tests/replays/snowhush_land0.replay");
}
