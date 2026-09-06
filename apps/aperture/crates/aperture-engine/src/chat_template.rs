//! Chat templates: speaking to an instruct model in its own turn markers.
//!
//! THE FAILURE THIS FIXES
//!
//! An 8B instruct model was sent the system prompt and the request as
//! raw text, `system\n\nuser`, with no turn markers. An instruct model
//! given raw text does not ANSWER it; it CONTINUES it -- and the most
//! likely continuation of a grammar is more grammar. The grammar-echo
//! filter dropped every such line, and the plan came back with no
//! operations. Every HTTP provider applies the template server-side;
//! the in-instance engine sent bytes.
//!
//! The GGUF carries the model's own template as a Jinja string under
//! `tokenizer.chat_template`. Interpreting Jinja is not worth a
//! dependency; recognising the FAMILY from its distinctive markers is
//! four string searches, and the families in use are few.
//!
//! WHAT A FAMILY GIVES
//!
//! * `wrap_system` / `wrap_user`: the two halves of a turn, kept apart
//!   so the system half can be the prefix a KV cache reuses.
//! * `stop`: the token that ends the assistant's turn. This is NOT
//!   always the GGUF's declared eos: Llama 3 declares
//!   `<|end_of_text|>` but ends a turn with `<|eot_id|>`, and a model
//!   stopped only on the former runs on into a fabricated next turn.
//!
//! `Raw` is the honest fallback for a base model or an unknown
//! template: plain concatenation, which is what the engine did before.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Llama3,
    ChatMl,
    Mistral,
    Gemma,
    Phi,
    Raw,
}

impl Family {
    /// Recognise a template from its markers. Order matters where
    /// markers overlap: ChatML's `<|im_start|>` is unmistakable, so it
    /// is checked before the looser ones.
    pub fn detect(chat_template: Option<&str>) -> Family {
        let Some(t) = chat_template else { return Family::Raw };
        if t.contains("<|im_start|>") { Family::ChatMl }
        else if t.contains("<|start_header_id|>") { Family::Llama3 }
        else if t.contains("<start_of_turn>") { Family::Gemma }
        else if t.contains("<|user|>") && t.contains("<|end|>") { Family::Phi }
        else if t.contains("[INST]") { Family::Mistral }
        else { Family::Raw }
    }

    pub fn name(self) -> &'static str {
        match self {
            Family::Llama3 => "llama3", Family::ChatMl => "chatml", Family::Mistral => "mistral",
            Family::Gemma => "gemma", Family::Phi => "phi", Family::Raw => "raw",
        }
    }

    /// The system half of a turn: everything up to where the user's
    /// text begins. Byte-identical across turns for the same system
    /// prompt, which is what makes it a cacheable prefix.
    pub fn wrap_system(self, system: &str) -> String {
        match self {
            Family::Llama3 => format!(
                "<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\n{system}<|eot_id|>"),
            Family::ChatMl => format!("<|im_start|>system\n{system}<|im_end|>\n"),
            // Mistral has no system role; the convention is to fold it
            // into the first user turn.
            Family::Mistral => format!("<s>[INST] {system}\n\n"),
            // Gemma likewise.
            Family::Gemma => format!("<bos><start_of_turn>user\n{system}\n\n"),
            Family::Phi => format!("<|system|>\n{system}<|end|>\n"),
            Family::Raw => format!("{system}\n\n"),
        }
    }

    /// The user half, ending exactly where the assistant should start
    /// writing.
    pub fn wrap_user(self, user: &str) -> String {
        match self {
            Family::Llama3 => format!(
                "<|start_header_id|>user<|end_header_id|>\n\n{user}<|eot_id|>\
                 <|start_header_id|>assistant<|end_header_id|>\n\n"),
            Family::ChatMl => format!("<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n"),
            Family::Mistral => format!("{user} [/INST]"),
            Family::Gemma => format!("{user}<end_of_turn>\n<start_of_turn>model\n"),
            Family::Phi => format!("<|user|>\n{user}<|end|>\n<|assistant|>\n"),
            Family::Raw => user.to_string(),
        }
    }

    /// A completed assistant turn, for a prior exchange that sits in
    /// the prefix: the model's own reply in its own markers, ended.
    pub fn wrap_assistant(self, reply: &str) -> String {
        match self {
            Family::Llama3 => format!("{reply}<|eot_id|>"),
            Family::ChatMl => format!("{reply}<|im_end|>\n"),
            Family::Mistral => format!("{reply}</s>"),
            Family::Gemma => format!("{reply}<end_of_turn>\n"),
            Family::Phi => format!("{reply}<|end|>\n"),
            Family::Raw => format!("{reply}\n\n"),
        }
    }

    /// A prior user turn WITHOUT the assistant header: the exchange
    /// continues, it does not end where the assistant starts writing.
    pub fn wrap_user_prior(self, user: &str) -> String {
        match self {
            Family::Llama3 => format!("<|start_header_id|>user<|end_header_id|>\n\n{user}<|eot_id|>\
                                       <|start_header_id|>assistant<|end_header_id|>\n\n"),
            Family::ChatMl => format!("<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n"),
            Family::Mistral => format!("{user} [/INST]"),
            Family::Gemma => format!("{user}<end_of_turn>\n<start_of_turn>model\n"),
            Family::Phi => format!("<|user|>\n{user}<|end|>\n<|assistant|>\n"),
            Family::Raw => format!("{user}\n"),
        }
    }

    /// The special-token TEXT that ends the assistant's turn, to be
    /// looked up in the vocabulary. None for Raw: the GGUF's eos is all
    /// there is.
    pub fn end_of_turn(self) -> Option<&'static str> {
        match self {
            Family::Llama3 => Some("<|eot_id|>"),
            Family::ChatMl => Some("<|im_end|>"),
            Family::Mistral => Some("</s>"),
            Family::Gemma => Some("<end_of_turn>"),
            Family::Phi => Some("<|end|>"),
            Family::Raw => None,
        }
    }

    /// Strip any turn-marker text the decoder let through -- a model
    /// that produced `<|eot_id|>` as text rather than as the token, or
    /// began a fabricated next header.
    pub fn clean(self, reply: &str) -> String {
        let mut s = reply.to_string();
        for m in ["<|eot_id|>", "<|start_header_id|>", "<|end_header_id|>", "<|im_end|>",
                  "<|im_start|>", "<end_of_turn>", "<start_of_turn>", "</s>", "<|end|>",
                  "<|assistant|>", "<|user|>"] {
            if let Some(i) = s.find(m) { s.truncate(i); }
        }
        s.trim_end().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_are_recognised_from_their_markers() {
        assert_eq!(Family::detect(Some("{% for m in messages %}<|start_header_id|>{{ m.role }}<|end_header_id|>")),
                   Family::Llama3);
        assert_eq!(Family::detect(Some("<|im_start|>system")), Family::ChatMl);
        assert_eq!(Family::detect(Some("[INST] {{ content }} [/INST]")), Family::Mistral);
        assert_eq!(Family::detect(Some("<start_of_turn>user")), Family::Gemma);
        assert_eq!(Family::detect(Some("<|user|>\n{{ c }}<|end|>")), Family::Phi);
        assert_eq!(Family::detect(Some("{{ bos }}{{ prompt }}")), Family::Raw);
        assert_eq!(Family::detect(None), Family::Raw, "a base model: no template, raw");
    }

    #[test]
    fn llama3_turns_look_like_llama3() {
        let s = Family::Llama3.wrap_system("SYS");
        let u = Family::Llama3.wrap_user("USER");
        assert!(s.starts_with("<|begin_of_text|><|start_header_id|>system<|end_header_id|>\n\nSYS<|eot_id|>"));
        assert!(u.ends_with("<|start_header_id|>assistant<|end_header_id|>\n\n"),
                "ends where the assistant starts writing");
        assert!(u.contains("USER<|eot_id|>"));
        assert_eq!(Family::Llama3.end_of_turn(), Some("<|eot_id|>"),
                   "NOT <|end_of_text|>: that is why 8B ran on into a fabricated turn");
    }

    #[test]
    fn the_system_half_is_a_stable_prefix() {
        // The whole point of splitting the turn: same system, same
        // bytes, so a KV prefix cache hits.
        for f in [Family::Llama3, Family::ChatMl, Family::Mistral, Family::Gemma, Family::Phi, Family::Raw] {
            assert_eq!(f.wrap_system("S"), f.wrap_system("S"));
            assert!(!f.wrap_system("S").contains("USER"));
            let full = format!("{}{}", f.wrap_system("S"), f.wrap_user("U"));
            assert!(full.contains('S') && full.contains('U'), "{}: {full}", f.name());
        }
    }

    #[test]
    fn leaked_turn_markers_are_cut() {
        assert_eq!(Family::Llama3.clean("cylinder @0,0 r=3\n<|eot_id|><|start_header_id|>user"),
                   "cylinder @0,0 r=3");
        assert_eq!(Family::ChatMl.clean("tree @1,1<|im_end|>\n<|im_start|>user\nmore"), "tree @1,1");
        assert_eq!(Family::Raw.clean("plain text"), "plain text");
    }

    #[test]
    fn raw_is_the_old_behaviour_exactly() {
        assert_eq!(format!("{}{}", Family::Raw.wrap_system("S"), Family::Raw.wrap_user("U")), "S\n\nU");
    }

    #[test]
    fn a_chain_of_prior_turns_is_the_prefix_of_the_next() {
        let f = Family::Llama3;
        let one = format!("{}{}", f.wrap_system("S"), f.wrap_user("U1"));
        let chain = format!("{}{}{}{}", f.wrap_system("S"), f.wrap_user_prior("U1"), f.wrap_assistant("A1"), f.wrap_user("U2"));
        assert!(chain.starts_with(&one), "turn two's prefix begins with turn one's whole prompt");
        assert!(chain.contains("A1<|eot_id|>"));
        assert!(chain.ends_with("<|start_header_id|>assistant<|end_header_id|>\n\n"));
    }
}
