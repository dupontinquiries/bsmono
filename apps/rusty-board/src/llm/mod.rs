//! LLM integration — calls an OpenAI-compatible chat completions
//! API for keyboard-side text tasks: synonyms, "go back and fix
//! letter X", dictation post-processing, freeform prompts.
//!
//! The bridge runs requests on a worker thread (using reqwest's
//! blocking client) and posts results back to the GTK main loop via
//! mpsc channels — same pattern as the voice bridge.
//!
//! When the `llm` feature is off, the public API still compiles
//! but all calls return "LLM disabled" errors. This lets the
//! settings panel and engine reference LLM types unconditionally.

#[cfg(feature = "llm")]
pub mod client;
#[cfg(feature = "llm")]
pub mod service;

#[cfg(feature = "llm")]
pub use service::{LlmRequest, LlmService, LlmResponse};

/// Task taxonomy. Each variant captures enough information to build
/// a prompt and post-process the response. Kept defined regardless
/// of feature gate so call sites compile.
#[derive(Debug, Clone)]
pub enum LlmTask {
    /// "Find a synonym for X." Input: word in context, optional
    /// surrounding text. Output: replacement word.
    Synonym {
        word: String,
        context: Option<String>,
    },
    /// "Rewrite the last sentence/word." Input: surrounding text +
    /// optional natural-language instruction. Output: replacement.
    Rewrite {
        target: String,
        instruction: String,
    },
    /// "Insert text the user described." Output: a string to commit
    /// at the cursor.
    Dictate {
        instruction: String,
    },
    /// "Continue / autocomplete the current text." Input: surrounding
    /// text up to cursor. Output: completion to insert.
    Continue {
        before: String,
        max_words: u32,
    },
    /// Freeform — pass the user's prompt verbatim. Output: model
    /// reply, which is committed to the focused app.
    Freeform(String),
}

impl LlmTask {
    /// Generate the user-side prompt for this task.
    pub fn user_prompt(&self) -> String {
        match self {
            LlmTask::Synonym { word, context } => {
                if let Some(ctx) = context {
                    format!(
                        "Find a single-word synonym for \"{}\" that fits this \
                         context: \"{}\". Respond with only the synonym.",
                        word, ctx
                    )
                } else {
                    format!(
                        "Find a single-word synonym for \"{}\". Respond with \
                         only the synonym.",
                        word
                    )
                }
            }
            LlmTask::Rewrite { target, instruction } => {
                format!(
                    "Rewrite the following text per the instruction. Respond \
                     with only the rewritten text.\n\nText: \"{}\"\nInstruction: \
                     {}",
                    target, instruction
                )
            }
            LlmTask::Dictate { instruction } => {
                format!(
                    "Generate text the user described. Respond with only the \
                     text to insert.\n\nDescription: {}",
                    instruction
                )
            }
            LlmTask::Continue { before, max_words } => {
                format!(
                    "Continue the following text naturally for at most {} \
                     words. Respond with only the continuation, no quoting.\n\n\
                     Text: {}",
                    max_words, before
                )
            }
            LlmTask::Freeform(prompt) => prompt.clone(),
        }
    }

    /// Human-readable label for the microterm status line.
    pub fn label(&self) -> &'static str {
        match self {
            LlmTask::Synonym { .. } => "Synonym",
            LlmTask::Rewrite { .. } => "Rewrite",
            LlmTask::Dictate { .. } => "Dictate",
            LlmTask::Continue { .. } => "Continue",
            LlmTask::Freeform(_) => "Prompt",
        }
    }
}
