//! The chat box: keys in, actions out.
//!
//! Engine-agnostic. The game maps its key events onto `Key`, feeds
//! them to `ChatBox::key`, and acts on the returned `Action`. The box
//! owns the buffer, cursor, history, and autocomplete state; it does
//! not draw, and it does not know what a command does. That split is
//! what makes it testable here and reusable in egui, a terminal, or a
//! voice transcript that "types" into it.
//!
//! Autocomplete is card-based: every command has a one-line summary,
//! an argument signature, and an example. Tab cycles matches; the
//! current match's card is exposed so the UI can render it beside the
//! box. The cards are the discoverability layer — a player who types
//! `/` and sees eight cards learns the whole vocabulary without a
//! manual.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter, Escape, Tab, ShiftTab, Up, Down, Backspace,
    /// Ctrl+Enter: commit the pending plan without leaving the box.
    CtrlEnter,
    /// Ctrl+Z: undo the last build.
    CtrlZ,
    PageUp, PageDown,
    Char(char),
}

/// What the game should do after a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Box opened (Enter while closed).
    Opened,
    /// Box closed without submitting.
    Closed,
    /// A full line was submitted; parse it with `command_parse`.
    Submit(String),
    /// Commit the pending plan (Ctrl+Enter).
    Commit,
    /// Undo the last build (Ctrl+Z).
    Undo,
    /// Focus tier up / down (PageUp / PageDown).
    FocusBigger,
    FocusSmaller,
    /// Buffer changed; redraw.
    Edited,
    Nothing,
}

/// A help card for one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub command: &'static str,
    pub signature: &'static str,
    pub summary: &'static str,
    pub example: &'static str,
    /// Short list of accepted argument words, for a second-level
    /// completion (e.g. `/focus bigger`).
    pub arg_words: &'static [&'static str],
}

pub const CARDS: &[Card] = &[
    Card { command: "/build", signature: "/build <what you want>",
           summary: "Describe a structure in plain words; the model plans it where you're looking.",
           example: "/build a small stone watchtower with a mossy base",
           arg_words: &["enableVoice", "disableVoice"] },
    Card { command: "/plan", signature: "/plan",
           summary: "Show the pending plan again: the arithmetic and a dry-run block count.",
           example: "/plan", arg_words: &[] },
    Card { command: "/commit", signature: "/commit",
           summary: "Build the pending plan. Refuses if you've looked away since the preview.",
           example: "/commit", arg_words: &[] },
    Card { command: "/undo", signature: "/undo",
           summary: "Reverse the last build as a unit, every block of it.",
           example: "/undo", arg_words: &[] },
    Card { command: "/focus", signature: "/focus [bigger|smaller]",
           summary: "Report or change the scale the model reads at: detail, structure, landscape.",
           example: "/focus bigger",
           arg_words: &["bigger", "smaller"] },
    Card { command: "/anim", signature: "/anim <ground-up|physics|random|instant>",
           summary: "How builds reveal: rising walls, falling blocks, scattered, or all at once.",
           example: "/anim physics",
           arg_words: &["ground-up", "physics", "random", "instant"] },
    Card { command: "/model", signature: "/model [name]",
           summary: "List models in ~/models, or load one. Tab completes; partial names work.",
           example: "/model qwen",
           arg_words: &[] },
    Card { command: "/find", signature: "/find <structure|mob|biome|water>",
           summary: "Locate the nearest thing of a kind.",
           example: "/find water",
           arg_words: &["structure", "mob", "biome", "water"] },
    Card { command: "/locate", signature: "/locate",
           summary: "Report your own coordinates.",
           example: "/locate", arg_words: &[] },
];

pub fn card_for(command: &str) -> Option<&'static Card> {
    CARDS.iter().find(|c| c.command == command)
}

/// Completions for a partial line. Completes the verb, or — once the
/// verb is complete — its argument words. Returns full replacement
/// lines, so the UI can just swap the buffer.
pub fn complete(partial: &str) -> Vec<(String, &'static Card)> {
    let p = partial.trim_start();
    if !p.starts_with('/') { return Vec::new(); }
    let mut parts = p.splitn(2, ' ');
    let verb = parts.next().unwrap_or("");
    let rest = parts.next();
    match rest {
        // still typing the verb
        None => CARDS.iter()
            .filter(|c| c.command.starts_with(verb))
            .map(|c| (format!("{} ", c.command), c))
            .collect(),
        // verb complete; complete an argument word
        Some(arg) => {
            let Some(card) = card_for(verb) else { return Vec::new() };
            let a = arg.trim_start();
            card.arg_words.iter()
                .filter(|w| w.starts_with(a) && !a.is_empty() || a.is_empty())
                .map(|w| (format!("{} {}", card.command, w), card))
                .collect()
        }
    }
}

#[derive(Debug, Default)]
pub struct ChatBox {
    pub open: bool,
    pub buffer: String,
    /// Runtime completions for commands whose arguments are not known
    /// at compile time -- currently `/model`, filled from the model
    /// registry after a scan. Without this, Tab works for every verb
    /// except the one where it matters most.
    pub dynamic_args: Vec<(&'static str, Vec<String>)>,
    history: Vec<String>,
    /// Index into history while browsing with Up/Down; None = editing new.
    hist_pos: Option<usize>,
    /// Current autocomplete cycle: candidates and the selected index.
    completions: Vec<(String, &'static Card)>,
    comp_pos: Option<usize>,
    /// Draft saved when the player starts browsing history.
    draft: String,
}

impl ChatBox {
    pub fn new() -> Self { Self::default() }

    /// The card to show beside the box: for the current completion if
    /// cycling, else for whatever verb has been typed so far.
    pub fn current_card(&self) -> Option<&'static Card> {
        if let Some(i) = self.comp_pos {
            return self.completions.get(i).map(|(_, c)| *c);
        }
        let verb = self.buffer.trim_start().split(' ').next().unwrap_or("");
        if verb.starts_with('/') { card_for(verb).or_else(|| {
            // partial verb: show the first matching card as a hint
            CARDS.iter().find(|c| c.command.starts_with(verb))
        }) } else { None }
    }

    /// All cards matching the current buffer, for a dropdown.
    pub fn matching_cards(&self) -> Vec<&'static Card> {
        complete(&self.buffer).into_iter().map(|(_, c)| c).collect::<Vec<_>>()
            .into_iter().fold(Vec::new(), |mut v, c| { if !v.contains(&c) { v.push(c); } v })
    }

    pub fn key(&mut self, k: Key) -> Action {
        if !self.open {
            return match k {
                Key::Enter => { self.open = true; self.buffer.clear(); self.reset_cycle(); Action::Opened }
                Key::CtrlZ => Action::Undo,
                Key::PageUp => Action::FocusBigger,
                Key::PageDown => Action::FocusSmaller,
                _ => Action::Nothing,
            };
        }
        match k {
            Key::Escape => { self.open = false; self.reset_cycle(); self.hist_pos = None; Action::Closed }
            Key::Enter => {
                let line = self.buffer.trim().to_string();
                self.open = false;
                self.reset_cycle();
                self.hist_pos = None;
                if line.is_empty() { return Action::Closed; }
                if self.history.last() != Some(&line) { self.history.push(line.clone()); }
                self.buffer.clear();
                Action::Submit(line)
            }
            Key::CtrlEnter => Action::Commit,
            Key::CtrlZ => Action::Undo,
            Key::PageUp => Action::FocusBigger,
            Key::PageDown => Action::FocusSmaller,
            Key::Tab | Key::ShiftTab => {
                // A cycle of one whose only candidate is already in the
                // buffer is exhausted: recompute so the NEXT level of
                // completion appears. Without this, `/model` + Tab
                // completed the verb to `/model ` and every further Tab
                // did nothing at all -- the completion looked broken at
                // exactly the moment a player first reaches for it.
                if self.comp_pos.is_some()
                    && self.completions.len() <= 1
                    && self.completions.first().map(|(s, _)| s.as_str()) == Some(self.buffer.as_str())
                {
                    self.reset_cycle();
                }
                if self.comp_pos.is_none() {
                    self.completions = self.complete_all();
                    if self.completions.is_empty() { return Action::Nothing; }
                    self.comp_pos = Some(0);
                } else {
                    let n = self.completions.len();
                    let i = self.comp_pos.unwrap();
                    self.comp_pos = Some(if k == Key::Tab { (i + 1) % n } else { (i + n - 1) % n });
                }
                self.buffer = self.completions[self.comp_pos.unwrap()].0.clone();
                Action::Edited
            }
            Key::Up => {
                if self.history.is_empty() { return Action::Nothing; }
                let next = match self.hist_pos {
                    None => { self.draft = self.buffer.clone(); self.history.len() - 1 }
                    Some(0) => 0,
                    Some(i) => i - 1,
                };
                self.hist_pos = Some(next);
                self.buffer = self.history[next].clone();
                self.reset_cycle();
                Action::Edited
            }
            Key::Down => {
                match self.hist_pos {
                    None => Action::Nothing,
                    Some(i) if i + 1 < self.history.len() => {
                        self.hist_pos = Some(i + 1);
                        self.buffer = self.history[i + 1].clone();
                        Action::Edited
                    }
                    Some(_) => {
                        self.hist_pos = None;
                        self.buffer = std::mem::take(&mut self.draft);
                        Action::Edited
                    }
                }
            }
            Key::Backspace => { self.buffer.pop(); self.reset_cycle(); Action::Edited }
            Key::Char(c) => {
                // a completed verb already ends in a space; swallow a
                // second one so "/anim  physics" never happens
                if c == ' ' && self.buffer.ends_with(' ') { return Action::Nothing; }
                self.buffer.push(c); self.reset_cycle(); Action::Edited
            }
        }
    }

    /// Static card completions plus any runtime argument lists.
    fn complete_all(&self) -> Vec<(String, &'static Card)> {
        let mut out = complete(&self.buffer);
        let p = self.buffer.trim_start();
        if let Some((verb, arg)) = p.split_once(' ') {
            if let Some((_, values)) = self.dynamic_args.iter().find(|(v, _)| *v == verb) {
                if let Some(card) = card_for(verb) {
                    let a = arg.trim_start().to_lowercase();
                    // Prefix matches first, then substring -- the same rule
                    // `ModelRegistry::complete` uses. Plain substring put
                    // `Falcon-H1-Tiny` ahead of every Llama for the input
                    // "l", because "Falcon" contains an l. People type the
                    // start of the name they mean.
                    let (mut pre, mut sub) = (Vec::new(), Vec::new());
                    for v in values {
                        let lv = v.to_lowercase();
                        if a.is_empty() || lv.starts_with(&a) { pre.push(v); }
                        else if lv.contains(&a) { sub.push(v); }
                    }
                    pre.extend(sub);
                    out.extend(pre.into_iter().map(|v| (format!("{verb} {v}"), card)));
                }
            }
        }
        out
    }

    /// Supply runtime completions, e.g. model names after a scan.
    pub fn set_dynamic_args(&mut self, verb: &'static str, values: Vec<String>) {
        self.dynamic_args.retain(|(v, _)| *v != verb);
        self.dynamic_args.push((verb, values));
    }

    fn reset_cycle(&mut self) { self.completions.clear(); self.comp_pos = None; }

    pub fn history(&self) -> &[String] { &self.history }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_str(b: &mut ChatBox, s: &str) { for c in s.chars() { b.key(Key::Char(c)); } }

    #[test]
    fn enter_opens_types_and_submits() {
        let mut b = ChatBox::new();
        assert_eq!(b.key(Key::Enter), Action::Opened);
        type_str(&mut b, "/locate");
        assert_eq!(b.key(Key::Enter), Action::Submit("/locate".into()));
        assert!(!b.open);
        assert_eq!(b.history(), &["/locate".to_string()]);
    }

    #[test]
    fn escape_closes_without_submitting() {
        let mut b = ChatBox::new();
        b.key(Key::Enter);
        type_str(&mut b, "/build a hut");
        assert_eq!(b.key(Key::Escape), Action::Closed);
        assert!(b.history().is_empty());
    }

    #[test]
    fn tab_cycles_verb_completions_and_shows_the_card() {
        let mut b = ChatBox::new();
        b.key(Key::Enter);
        type_str(&mut b, "/f");
        assert_eq!(b.key(Key::Tab), Action::Edited);
        let first = b.buffer.clone();
        assert!(first.starts_with("/f"), "{first}");
        let card = b.current_card().expect("card while cycling");
        assert!(first.trim().starts_with(card.command));
        b.key(Key::Tab);
        assert_ne!(b.buffer, first, "second tab moves to the next match");
        // ShiftTab goes back
        b.key(Key::ShiftTab);
        assert_eq!(b.buffer, first);
    }

    #[test]
    fn tab_completes_argument_words_once_the_verb_is_known() {
        let mut b = ChatBox::new();
        b.key(Key::Enter);
        type_str(&mut b, "/anim p");
        b.key(Key::Tab);
        assert_eq!(b.buffer, "/anim physics");
        assert_eq!(b.current_card().unwrap().command, "/anim");
    }

    #[test]
    fn partial_verb_shows_a_hint_card_before_tab() {
        let mut b = ChatBox::new();
        b.key(Key::Enter);
        type_str(&mut b, "/bu");
        assert_eq!(b.current_card().map(|c| c.command), Some("/build"));
        assert_eq!(b.matching_cards().len(), 1);
    }

    #[test]
    fn history_browses_and_restores_the_draft() {
        let mut b = ChatBox::new();
        for line in ["/locate", "/plan"] {
            b.key(Key::Enter); type_str(&mut b, line); b.key(Key::Enter);
        }
        b.key(Key::Enter);
        type_str(&mut b, "/bu");
        b.key(Key::Up);   assert_eq!(b.buffer, "/plan");
        b.key(Key::Up);   assert_eq!(b.buffer, "/locate");
        b.key(Key::Up);   assert_eq!(b.buffer, "/locate", "stops at oldest");
        b.key(Key::Down); assert_eq!(b.buffer, "/plan");
        b.key(Key::Down); assert_eq!(b.buffer, "/bu", "draft restored past the newest");
    }

    #[test]
    fn hotkeys_work_with_the_box_closed() {
        let mut b = ChatBox::new();
        assert_eq!(b.key(Key::CtrlZ), Action::Undo);
        assert_eq!(b.key(Key::PageUp), Action::FocusBigger);
        assert_eq!(b.key(Key::Char('w')), Action::Nothing, "movement keys are not ours");
    }

    #[test]
    fn every_card_example_starts_with_its_own_command() {
        for c in CARDS {
            assert!(c.example.starts_with(c.command), "{}: example {}", c.command, c.example);
            assert!(!c.summary.is_empty());
        }
    }

    #[test]
    fn model_names_complete_from_the_registry_at_runtime() {
        let mut b = ChatBox::new();
        b.set_dynamic_args("/model", vec![
            "qwen2.5-7b-instruct-q4_k_m".into(), "llama-3.2-3b-instruct-q8_0".into()]);
        b.key(Key::Enter);
        type_str(&mut b, "/model qwen");
        b.key(Key::Tab);
        assert_eq!(b.buffer, "/model qwen2.5-7b-instruct-q4_k_m");
        // substring works too: people type the size
        let mut b2 = ChatBox::new();
        b2.set_dynamic_args("/model", vec![
            "qwen2.5-7b-instruct-q4_k_m".into(), "llama-3.2-3b-instruct-q8_0".into()]);
        b2.key(Key::Enter);
        type_str(&mut b2, "/model 3b");
        b2.key(Key::Tab);
        assert!(b2.buffer.contains("llama-3.2-3b"), "{}", b2.buffer);
    }

    #[test]
    fn tab_does_not_stall_after_completing_a_verb() {
        let mut b = ChatBox::new();
        b.set_dynamic_args("/model", vec!["alpha-7b".into(), "beta-3b".into()]);
        b.key(Key::Enter);
        type_str(&mut b, "/model");
        b.key(Key::Tab);
        assert_eq!(b.buffer, "/model ", "first Tab completes the verb");
        b.key(Key::Tab);
        assert_eq!(b.buffer, "/model alpha-7b", "second Tab must offer arguments");
        b.key(Key::Tab);
        assert_eq!(b.buffer, "/model beta-3b", "and keep cycling");
    }

    #[test]
    fn dynamic_completion_prefers_the_prefix() {
        let mut b = ChatBox::new();
        // "Falcon" contains an l; "Llama" starts with one
        b.set_dynamic_args("/model", vec![
            "Falcon-H1-Tiny-90M".into(), "Llama-3.2-1B-Instruct".into()]);
        b.key(Key::Enter);
        type_str(&mut b, "/model l");
        b.key(Key::Tab);
        assert_eq!(b.buffer, "/model Llama-3.2-1B-Instruct",
                   "a prefix match must beat an incidental substring");
    }
}
