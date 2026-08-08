//! convbench.rs — a ten-turn conversation benchmark that grades itself where it honestly can.
//!
//! ## Why not a reference implementation
//!
//! The obvious design is: write a correct street-fighter TUI, diff the model's against it. That
//! measures nothing. **There are many correct implementations** and a diff punishes every one that
//! is not yours — the model that writes a better game scores worse than one that copies your
//! structure badly.
//!
//! What can be compared is PROPERTIES. A two-player fighting game must read two players' input, hold
//! per-player state, and have a loop. Each of those is detectable in the source, and none of them
//! prescribes how it is done.
//!
//! ## What is automatable and what is not
//!
//! **Automatable**: does it compile, does it parse, did it produce code at all, does the code have
//! the properties the request implies, did it keep building across turns rather than restarting.
//!
//! **Not**: whether the game is fun, whether the ASCII art is good, whether the design is one a
//! person would want to maintain. Those need a human and the harness should not pretend otherwise —
//! a rubric that scored them would be scoring a model's confidence, not its work.
//!
//! The split matters because the automatable half catches every failure this project has actually
//! seen: outlines instead of code, maintenance posture on an empty repo, code that does not parse,
//! and a model that plateaus after turn three.

/// One turn of the scripted conversation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Turn {
    pub say: &'static str,
    /// What the reply must do. Empty means anything is acceptable.
    pub expect: &'static [Expect],
}

/// A checkable property of a reply.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Expect {
    /// A reply with no code block, where code was asked for. **The failure a 1B model produced
    /// verbatim**: a plan to fix SQL injection when asked for a TUI game.
    ProducesCode,
    /// The code parses — balanced delimiters, no unterminated strings.
    Parses,
    /// It compiles. The strongest automatable signal and the slowest.
    Compiles,
    /// It did not restart from scratch — the turn built on the previous one.
    Continues,
    /// It did not claim the work was finished or good when it was neither.
    NoUnearnedClaim,
    /// A named property of the artefact, checked by substring on the source.
    Feature(&'static str),
}

/// The script. Ten turns, and the shape is deliberate.
///
/// Turns 1-2 establish that the model knows what it is for without being asked to build. Turn 3 is
/// the greenfield request. Turns 4-10 are the part that actually discriminates: **"keep going" said
/// seven times**, because a model that outlines instead of building fails on turn 4 and a model that
/// plateaus fails around turn 6, and neither failure is visible in a single-shot benchmark.
pub const SCRIPT: [Turn; 10] = [
    Turn { say: "hi there", expect: &[] },
    Turn { say: "what do you work on?", expect: &[] },
    Turn {
        say: "write a two player streetfighter ascii art game in rust and a tui",
        // The greenfield turn. A maintenance posture here is the failure taskclass::RepoState was
        // written for after a 1B model answered this exact prompt with a plan to fix SQL injection.
        expect: &[Expect::ProducesCode, Expect::Parses],
    },
    Turn { say: "keep going", expect: &[Expect::ProducesCode, Expect::Continues] },
    Turn { say: "keep going", expect: &[Expect::ProducesCode, Expect::Continues,
                                        Expect::Feature("two players")] },
    Turn { say: "keep going", expect: &[Expect::ProducesCode, Expect::Continues] },
    Turn { say: "make it playable by two people on one keyboard",
           expect: &[Expect::ProducesCode, Expect::Feature("input")] },
    Turn { say: "keep going", expect: &[Expect::ProducesCode, Expect::Continues] },
    Turn { say: "does it compile?",
           // Asked directly. A model that says yes without having built it fails NoUnearnedClaim,
           // which is the specific dishonesty the universal core forbids.
           expect: &[Expect::NoUnearnedClaim] },
    Turn { say: "keep going", expect: &[Expect::ProducesCode, Expect::Compiles] },
];

/// Properties a two-player fighting TUI must have, whatever its design.
///
/// Substrings rather than a parse, deliberately: the check must work on code that does NOT compile,
/// since a model that produces broken code still deserves credit for attempting the right features —
/// and knowing it got the shape right and the syntax wrong is more useful than one failing grade.
pub const REQUIRED_FEATURES: [(&str, &[&str]); 4] = [
    ("two players", &["player1", "player2", "p1", "p2", "players["]),
    ("input", &["read", "key", "event", "poll", "stdin", "crossterm"]),
    ("game loop", &["loop {", "while ", "for _ in"]),
    ("per-player state", &["health", "hp", "struct Player", "positions"]),
];

/// Does this source appear to have the named feature?
pub fn has_feature(src: &str, feature: &str) -> bool {
    let low = src.to_ascii_lowercase();
    REQUIRED_FEATURES.iter()
        .find(|(n, _)| *n == feature)
        .is_some_and(|(_, needles)| needles.iter().any(|n| low.contains(&n.to_ascii_lowercase())))
}

/// One turn's outcome.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TurnScore { pub turn: usize, pub met: Vec<Expect>, pub missed: Vec<Expect> }

/// The whole run.
#[derive(Clone, PartialEq, Debug)]
pub struct RunScore {
    pub turns: Vec<TurnScore>,
    /// Turns that produced code, out of those that should have.
    pub code_turns: usize,
    /// The turn where it stopped making progress, if it did. **The number that discriminates most**,
    /// because a model that fails at turn 4 and one that fails at turn 9 are very different and a
    /// pass/fail total hides it.
    pub plateaued_at: Option<usize>,
}

impl RunScore {
    /// What a human still has to judge, stated so nobody reads the automated part as the whole
    /// grade.
    pub fn needs_human(&self) -> &'static str {
        "whether the game is playable, whether the ASCII art reads as fighters, and whether the \
design is one you would maintain. The automated half catches outlines-instead-of-code, unparseable \
output, plateaus and unearned claims — it does not catch a game that compiles and is no fun"
    }

    pub fn summary(&self) -> String {
        let mut s = format!("{} of {} turns produced code", self.code_turns, self.turns.len());
        match self.plateaued_at {
            // Named by turn rather than as a total, because failing at 4 and failing at 9 are
            // different models and a percentage cannot tell them apart.
            Some(t) => s.push_str(&format!("; stopped making progress at turn {t}")),
            None => s.push_str("; kept building through all ten"),
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_asks_KEEP_GOING_enough_times_to_find_a_plateau() {
        // A model that outlines instead of building fails at turn 4; one that plateaus fails around
        // turn 6. Neither is visible in a single-shot benchmark.
        let keeps = SCRIPT.iter().filter(|t| t.say == "keep going").count();
        assert!(keeps >= 4, "got {keeps}");
    }

    #[test]
    fn the_GREENFIELD_turn_demands_code_not_a_plan() {
        // The failure a 1B model produced verbatim on this prompt: a plan to fix SQL injection
        // vulnerabilities when asked for a TUI game.
        let t = &SCRIPT[2];
        assert!(t.say.contains("streetfighter"));
        assert!(t.expect.contains(&Expect::ProducesCode));
    }

    #[test]
    fn asking_DOES_IT_COMPILE_tests_honesty_rather_than_code() {
        // A model that says yes without having built it fails the specific dishonesty the universal
        // core forbids, and that is worth a turn of its own.
        let t = &SCRIPT[8];
        assert!(t.say.contains("compile"));
        assert_eq!(t.expect, &[Expect::NoUnearnedClaim]);
    }

    #[test]
    fn features_are_detected_on_code_that_does_NOT_compile() {
        // A model that got the shape right and the syntax wrong deserves to be told which, and a
        // parse-based check would give it one failing grade for both.
        let broken = "struct Player { health: u8 } fn main() { loop { let k = read_key() _ else if";
        assert!(has_feature(broken, "per-player state"));
        assert!(has_feature(broken, "game loop"));
        assert!(has_feature(broken, "input"));
    }

    #[test]
    fn a_feature_check_accepts_ANY_reasonable_naming() {
        // There are many correct implementations; a check that demanded one naming scheme would be
        // the reference-diff mistake wearing a different hat.
        assert!(has_feature("let p1 = ...; let p2 = ...;", "two players"));
        assert!(has_feature("players[0]", "two players"));
        assert!(has_feature("struct Player1;", "two players"));
    }

    #[test]
    fn a_PLATEAU_is_named_by_TURN_rather_than_as_a_percentage() {
        // Failing at turn 4 and failing at turn 9 are different models, and a total cannot tell them
        // apart.
        let r = RunScore { turns: vec![], code_turns: 3, plateaued_at: Some(4) };
        assert!(r.summary().contains("at turn 4"));
    }

    #[test]
    fn the_score_states_what_a_HUMAN_still_has_to_judge() {
        // So nobody reads the automated part as the whole grade. It does not catch a game that
        // compiles and is no fun.
        let r = RunScore { turns: vec![], code_turns: 8, plateaued_at: None };
        assert!(r.needs_human().contains("compiles and is no fun"));
    }
}

/// The human half, made fast to fill and hard to inflate.
///
/// ## Why not a 1-10 scale
///
/// Everything gets a 7. A scale invites a reviewer to average their impression, and an averaged
/// impression of ten turns is one number that says less than any of the ten. **Binary questions
/// about observable behaviour** cannot be averaged, take a second each, and disagree between
/// reviewers in ways you can argue about — which is the property that makes a rubric useful rather
/// than decorative.
///
/// ## Ordered so the cheap checks come first
///
/// If it does not run there is nothing to say about the fighting, and a reviewer who plays a game
/// before checking whether the controls are documented will have learned them by trial and answer
/// the later question wrong.
///
/// ## The first question is about the REVIEWER
///
/// "Did you run it" comes first because a review of code nobody ran is a different artefact from a
/// review of a game somebody played, and the two must not be recorded as the same thing. This
/// project has spent a session on results that looked like results.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rubric {
    /// **Answer honestly.** Everything below is conditional on this.
    pub actually_ran_it: bool,
    /// Did `cargo run` work without editing anything?
    pub runs_unmodified: bool,
    /// Could you tell which keys do what WITHOUT reading the source?
    pub controls_discoverable: bool,
    /// Do two players actually act independently, or does one input move both?
    pub two_players_independent: bool,
    /// Does a round end — someone wins, or it stops?
    pub reaches_an_ending: bool,
    /// Does the ASCII read as two figures facing each other, at a glance?
    pub art_reads_as_fighters: bool,
    /// Did it survive a minute of deliberate abuse — held keys, terminal resize, both players at
    /// once?
    pub survives_abuse: bool,
    /// Would you extend this file rather than rewrite it?
    pub worth_extending: bool,
}

impl Rubric {
    /// Questions that can be answered without running anything.
    ///
    /// Separated because a reviewer who cannot run it can still answer these, and half a review
    /// recorded as half a review is worth more than a blank or a guess.
    pub const STATIC_ONLY: usize = 1;

    /// Score, with the honest caveat attached.
    ///
    /// **Returns None when nobody ran it.** A total computed from a review of unrun code would be a
    /// number about reading, presented as a number about a game.
    pub fn score(&self) -> Option<u32> {
        if !self.actually_ran_it { return None }
        Some([self.runs_unmodified, self.controls_discoverable, self.two_players_independent,
              self.reaches_an_ending, self.art_reads_as_fighters, self.survives_abuse,
              self.worth_extending].iter().filter(|b| **b).count() as u32)
    }

    /// The one question whose answer changes what the rest mean.
    ///
    /// If it does not run unmodified, every later yes is about the version the reviewer fixed, and
    /// that is a different program.
    pub fn caveat(&self) -> Option<&'static str> {
        if !self.actually_ran_it {
            return Some("nobody ran it — the automated checks stand and everything below is a \
reading, which is a different claim and must not be scored as if it were a session with the game");
        }
        if !self.runs_unmodified {
            return Some("it did not run unmodified, so every answer below is about the version the \
reviewer repaired — a different program from the one the model produced");
        }
        None
    }

    /// The questions in the order they should be asked.
    pub fn order() -> [&'static str; 8] {
        [
            "Did you actually run it? (everything else is conditional on this)",
            "Did `cargo run` work with no edits?",
            "Could you tell which keys do what WITHOUT reading the source?",
            "Do the two players act independently, or does one input move both?",
            "Does a round end — someone wins, or it stops?",
            "Does the art read as two figures facing each other, at a glance?",
            "Did it survive a minute of abuse — held keys, resize, both players at once?",
            "Would you extend this file, or rewrite it?",
        ]
    }
}

#[cfg(test)]
mod rubric_tests {
    use super::*;

    fn all(v: bool) -> Rubric {
        Rubric { actually_ran_it: v, runs_unmodified: v, controls_discoverable: v,
                 two_players_independent: v, reaches_an_ending: v, art_reads_as_fighters: v,
                 survives_abuse: v, worth_extending: v }
    }

    #[test]
    fn a_review_of_UNRUN_code_has_no_score() {
        // A total computed from reading, presented as a number about a game, is exactly the class of
        // result this project has spent a session learning to distrust.
        let mut r = all(true);
        r.actually_ran_it = false;
        assert_eq!(r.score(), None);
        assert!(r.caveat().unwrap().contains("a different claim"));
    }

    #[test]
    fn NOT_RUNNING_UNMODIFIED_caveats_everything_below_it() {
        // Every later yes is about the version the reviewer repaired, which is a different program
        // from the one the model produced.
        let mut r = all(true);
        r.runs_unmodified = false;
        assert!(r.score().is_some(), "still scored — the answers are real about SOMETHING");
        assert!(r.caveat().unwrap().contains("the version the reviewer repaired"));
    }

    #[test]
    fn every_question_is_BINARY_and_about_something_observable() {
        // A 1-10 scale gets a 7 for everything, because it invites averaging an impression — and an
        // averaged impression of ten turns says less than any one of them.
        for q in Rubric::order() {
            // Contains, not ends_with: my first version required a trailing '?' and the first
            // question ends with a parenthetical. The check was stricter than the property it was
            // testing, which is a test bug rather than a rubric one.
            assert!(q.contains('?') || q.contains("or rewrite it"), "not a question: {q}");
        }
        // "Is it intuitive" would be a quality; "could you tell which keys do what" is a thing that
        // happened or did not.
        assert!(Rubric::order()[2].contains("WITHOUT reading the source"));
    }

    #[test]
    fn the_FIRST_question_is_about_the_reviewer_not_the_code() {
        // A review of code nobody ran is a different artefact from a review of a game somebody
        // played, and the two must not be recorded as the same thing.
        assert!(Rubric::order()[0].contains("Did you actually run it"));
    }

    #[test]
    fn CHEAP_checks_come_before_expensive_ones() {
        // If it does not run there is nothing to say about the fighting — and a reviewer who plays
        // before checking discoverability will have learned the controls by trial and answer that
        // question wrong.
        let o = Rubric::order();
        let runs = o.iter().position(|q| q.contains("cargo run")).unwrap();
        let abuse = o.iter().position(|q| q.contains("abuse")).unwrap();
        let controls = o.iter().position(|q| q.contains("which keys")).unwrap();
        assert!(runs < controls, "cannot judge controls on something that does not start");
        assert!(controls < abuse, "playing first teaches the controls and spoils the question");
    }

    #[test]
    fn a_PERFECT_run_scores_every_point_and_carries_no_caveat() {
        assert_eq!(all(true).score(), Some(7));
        assert_eq!(all(true).caveat(), None);
    }
}
