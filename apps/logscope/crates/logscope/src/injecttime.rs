//! injecttime.rs — when an operator's injection lands, not just who receives it.
//!
//! `panelvoice::Share` decides WHO hears an answer. It says nothing about WHEN, and mid-generation
//! that is the whole question: a debater is thirty tokens into a reply when the operator says
//! something. Two options, both lossy.
//!
//! * **Interrupt** — discard the partial turn and restart with the injection in context. Costs the
//!   tokens already produced.
//! * **Queue** — let the turn finish and inject before the next. Costs nothing, and the debater
//!   commits to a position the operator was trying to redirect.
//!
//! ## The decision is arithmetic, not preference
//!
//! Interrupting is worth it when the work REMAINING exceeds the work already done — you save more than
//! you throw away. Early in a turn that is almost always true; near the end it never is. So the
//! crossover is roughly the halfway point of the expected turn, adjusted by how much the injection
//! actually bears on what is being written.
//!
//! ## Voice is different from typing, and the difference is decay
//!
//! A typed injection is deliberate: the operator composed it and it stays relevant. A spoken one is a
//! reaction to what is on screen NOW — "no, not that" means nothing three seconds later when the text
//! has moved on. So a voice injection has a short shelf life, and delivering it late is worse than
//! dropping it: the debater applies a correction to the wrong thing.

/// How the injection arrived.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Source {
    /// Composed. Stays relevant.
    Typed,
    /// Spoken, reacting to what was on screen at that moment.
    Voice,
}

impl Source {
    /// Milliseconds after which the injection no longer refers to what the operator saw.
    ///
    /// Voice decays fast: "no, not that" is about the text on screen when it was said. Typed does not
    /// decay meaningfully — the operator chose the words and can see they still apply.
    pub fn shelf_life_ms(self) -> u64 {
        match self { Source::Voice => 2_500, Source::Typed => u64::MAX }
    }
}

/// What to do with an injection.
#[derive(Clone, PartialEq, Debug)]
pub enum Timing {
    /// Stop the current turn and restart with this in context.
    Interrupt { discarded_tokens: u32 },
    /// Let the turn finish; deliver before the next one.
    Queue,
    /// Too late to be about what the operator saw. Dropped, with the reason shown.
    Stale { age_ms: u64 },
}

impl Timing {
    pub fn is_interrupt(&self) -> bool { matches!(self, Timing::Interrupt { .. }) }

    pub fn explain(&self) -> String {
        match self {
            Timing::Interrupt { discarded_tokens } => format!(
                "interrupting — {discarded_tokens} tokens discarded, which is less than what remains"),
            Timing::Queue => "queued for the next turn — the current one is far enough along that \
restarting costs more than it saves".into(),
            Timing::Stale { age_ms } => format!(
                "dropped: spoken {age_ms}ms ago and the text has moved on. Delivering it now would \
apply a correction to something the operator was not looking at, which is worse than not delivering it"),
        }
    }
}

/// Decide when an injection lands.
///
/// `bearing` is how much the injection relates to what is being written, 0..1 — a redirect scores high,
/// an aside low. It shifts the crossover: a highly relevant injection is worth more discarded tokens.
pub fn timing(src: Source, age_ms: u64, produced: u32, expected_total: u32, bearing: f32) -> Timing {
    if age_ms > src.shelf_life_ms() { return Timing::Stale { age_ms }; }
    if expected_total == 0 { return Timing::Queue; }
    let remaining = expected_total.saturating_sub(produced);
    // Interrupt when what remains exceeds what would be thrown away, scaled by relevance. A bearing of
    // 1.0 interrupts at the halfway point; a bearing of 0.2 needs the turn to be barely started.
    let worth = remaining as f32 * bearing.clamp(0.0, 1.0);
    if worth > produced as f32 { Timing::Interrupt { discarded_tokens: produced } } else { Timing::Queue }
}

/// The line a debater is given when an injection interrupts it.
///
/// Says the previous attempt was DISCARDED, because a model shown its own partial output tends to
/// continue it — and continuing is exactly what the interruption was meant to prevent.
pub const INTERRUPT_PROMPT: &str = "\
Your previous turn was discarded before it finished; it does not exist and you should not continue it. \
The operator said the following while you were writing. Start again with it in mind:";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_injection_early_in_a_turn_INTERRUPTS() {
        // Ten tokens in, ninety to go: restarting throws away ten and saves ninety.
        let t = timing(Source::Typed, 0, 10, 100, 1.0);
        assert!(t.is_interrupt());
        assert!(t.explain().contains("less than what remains"));
    }

    #[test]
    fn an_injection_near_the_end_QUEUES() {
        // Ninety tokens in, ten to go: restarting costs nine times what it saves.
        assert_eq!(timing(Source::Typed, 0, 90, 100, 1.0), Timing::Queue);
    }

    #[test]
    fn a_low_bearing_injection_needs_the_turn_barely_started() {
        // An aside is not worth discarding work for. At bearing 0.2, forty tokens remaining is worth
        // eight — less than the twenty already produced.
        assert_eq!(timing(Source::Typed, 0, 20, 60, 0.2), Timing::Queue);
        assert!(timing(Source::Typed, 0, 2, 60, 0.2).is_interrupt(), "but two tokens in, still worth it");
    }

    #[test]
    fn a_VOICE_injection_goes_STALE_and_typing_does_not() {
        // "No, not that" is about the text on screen when it was said. Three seconds later the text has
        // moved, and delivering it applies a correction to something the operator was not looking at —
        // worse than not delivering it at all.
        let stale = timing(Source::Voice, 4_000, 5, 100, 1.0);
        assert!(matches!(stale, Timing::Stale { .. }));
        assert!(stale.explain().contains("worse than not delivering it"));
        assert!(timing(Source::Typed, 4_000, 5, 100, 1.0).is_interrupt(),
            "a typed injection at the same age still lands: the operator chose the words");
    }

    #[test]
    fn a_voice_injection_INSIDE_its_shelf_life_still_lands() {
        assert!(timing(Source::Voice, 800, 5, 100, 1.0).is_interrupt());
    }

    #[test]
    fn an_unknown_turn_length_QUEUES_rather_than_guessing() {
        // expected_total = 0 means nothing has estimated the turn. Interrupting on no information would
        // discard real work for a saving nobody computed.
        assert_eq!(timing(Source::Typed, 0, 50, 0, 1.0), Timing::Queue);
    }

    #[test]
    fn the_interrupt_prompt_says_the_partial_turn_DOES_NOT_EXIST() {
        // A model shown its own partial output continues it, and continuing is exactly what the
        // interruption was meant to prevent. The wording has to be stronger than "you were stopped".
        assert!(INTERRUPT_PROMPT.contains("does not exist"));
        assert!(INTERRUPT_PROMPT.contains("should not continue it"));
        assert!(INTERRUPT_PROMPT.contains("Start again"));
    }
}
