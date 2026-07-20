//! genguard.rs — a live runaway guard for the decode loop.
//!
//! Two failure modes a small model hits when its real EOS never fires (wrong template framing, so it
//! never emits its stop token): it loops on a phrase, or it emits template markers AS TEXT (garbled
//! `<|eot_id|>` / `<|start_header_id|>` that aren't the real special token, so nothing stops it).
//! `runaway_stop` catches both so the loop is cut at the first sign instead of running to the cap.
#![allow(dead_code)]
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub enum StopReason { None, Repetition, PseudoMarker }

/// 4-gram repeat ratio over a word window (0 = all distinct .. ~1 = locked in a loop).
pub fn word_4gram_repeat(text: &str) -> f64 {
    let w: Vec<&str> = text.split_whitespace().collect();
    if w.len() < 4 { return 0.0; }
    let (mut seen, mut rep, mut tot) = (HashSet::new(), 0usize, 0usize);
    for g in w.windows(4) { tot += 1; if !seen.insert(g.to_vec()) { rep += 1; } }
    if tot == 0 { 0.0 } else { rep as f64 / tot as f64 }
}

/// True if the text contains a template marker emitted as text — canonical or garbled (extra pipes,
/// spaces, hyphens, parens). The model is trying to end a turn without hitting its real stop token.
pub fn has_pseudo_marker(text: &str) -> bool {
    let bracketish = text.contains("<|") || text.contains("||") || text.contains("|>");
    if !bracketish { return false; }
    // strip everything but [a-z0-9_] so "start - header - id" and "start_header_id" both normalize
    let norm: String = text.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    let norm = norm.replace('_', "");
    ["eotid", "starthead", "endhead", "imend", "imstart", "begintext", "endoftext"]
        .iter().any(|m| norm.contains(m))
}

/// Call each step with the recently decoded tail. Stops on a leaked marker, or on repetition once the
/// window's 4-gram repeat crosses `repeat_threshold` (e.g. 0.5).
pub fn runaway_stop(recent_text: &str, repeat_threshold: f64) -> StopReason {
    if has_pseudo_marker(recent_text) { return StopReason::PseudoMarker; }
    if word_4gram_repeat(recent_text) >= repeat_threshold { return StopReason::Repetition; }
    StopReason::None
}

pub fn gate_genguard() {
    // the actual leaked markers from the transcript (garbled variants)
    let markers = ["<||eot-id||>", "<|(start - header |>struct", "<|end-header-id|||user|||end", "<|eot_id|>"];
    let marker_ok = markers.iter().all(|m| has_pseudo_marker(m));
    // a clean sentence must NOT trip the marker detector
    let clean_marker = !has_pseudo_marker("it prints x then checks even or odd and prints a star");
    // the struct-loop tail from the transcript repeats hard
    let loop_tail = "struct MyStruct a u8 b u16 4x impl crate main rs ".repeat(6);
    let loop_ok = runaway_stop(&loop_tail, 0.5) == StopReason::Repetition;
    // a normal answer keeps going
    let clean = runaway_stop("the program prints the value of x then checks whether i is even", 0.5) == StopReason::None;

    for (label, ok) in [
        ("leaked template markers caught (incl. garbled)", marker_ok),
        ("clean prose not flagged as a marker", clean_marker),
        ("struct-loop tail stops on repetition", loop_ok),
        ("normal answer is not stopped", clean),
    ] {
        println!("  [{}] {}", if ok { "\u{2713}" } else { "\u{2717} FAIL" }, label);
    }
    let all = marker_ok && clean_marker && loop_ok && clean;
    println!("gate-genguard: {}", if all { "runaway guard checks \u{2713}" } else { "FAIL \u{2717}" });
}

// ───────────────────────── robust stop-token resolution ─────────────────────────
// A model can stop on ANY of several special tokens, and which one is model-specific:
// Llama-3-Instruct stops on 128009 (<|eot_id|>), but DeepSeek-R1-Distill-Llama stops on 128001
// (<|end_of_text|> / <｜end▁of▁sentence｜>). Watching a SINGLE id is the looping bug — the app
// resolved 128009 while the model emits 128001, so its stop signal is ignored forever. Collect the
// GGUF-declared eos PLUS every known stop special the tokenizer carries, and break on the whole set.

pub fn stop_token_set(declared_eos: Option<u32>, specials: &[Option<u32>]) -> HashSet<u32> {
    let mut s = HashSet::new();
    if let Some(e) = declared_eos { s.insert(e); }
    for &id in specials { if let Some(id) = id { s.insert(id); } }
    s
}

/// Augment a stop set with EVERY control special the tokenizer carries (see Tokenizer::control_stop_ids).
/// This is the defense-in-depth over `stop_token_set`'s hardcoded eos-name list: a converted checkpoint
/// whose config omits a control token (e.g. <|observation|>, <sop>, [gMASK]) would otherwise print it as
/// chat text and generate past the turn end. Content specials (<think>/<tool_call>/…) are already excluded
/// by control_stop_ids, so reasoning/tool output is preserved.
pub fn stop_token_set_with_controls(base: HashSet<u32>, control_ids: &[u32]) -> HashSet<u32> {
    let mut s = base;
    s.extend(control_ids.iter().copied());
    s
}

pub fn gate_stop() {
    // DeepSeek-R1-Distill-Llama: GGUF declares 128001; <|eot_id|>=128009, <|end_of_text|>=128001.
    // The app watched only 128009 → missed the model's real stop (128001) → looped.
    let ds = stop_token_set(Some(128001), &[Some(128009) /*eot*/, Some(128001) /*end_of_text*/]);
    let ds_ok = ds.contains(&128001) && ds.contains(&128009); // now stops on the one it was missing
    // Llama-3-Instruct: declared 128009
    let llama = stop_token_set(Some(128009), &[Some(128009), Some(128001)]);
    let llama_ok = llama.contains(&128009);
    // smollm2 (ChatML): declared 2 = <|im_end|>
    let smol = stop_token_set(Some(2), &[Some(2) /*im_end*/, None]);
    let smol_ok = smol.contains(&2) && smol.len() == 1;

    for (label, ok) in [
        ("DeepSeek set includes 128001 (the missed stop)", ds_ok),
        ("Llama-3 set includes 128009", llama_ok),
        ("smollm2 set is {2}", smol_ok),
    ] {
        println!("  [{}] {}", if ok { "\u{2713}" } else { "\u{2717} FAIL" }, label);
    }
    println!("gate-stop: {}", if ds_ok && llama_ok && smol_ok { "stop-set resolution checks \u{2713}" } else { "FAIL \u{2717}" });
}

#[cfg(test)]
mod control_augment_tests {
    use super::*;

    #[test]
    fn control_ids_are_added_to_the_stop_set() {
        // base = declared eos + a couple named specials; controls = extra tokenizer control ids the
        // config never declared. The augmented set must contain BOTH.
        let base = stop_token_set(Some(128009), &[Some(128001)]);
        let controls = [40000u32, 40001, 40002]; // e.g. <|observation|>, <sop>, [gMASK]
        let s = stop_token_set_with_controls(base, &controls);
        for id in [128009u32, 128001, 40000, 40001, 40002] {
            assert!(s.contains(&id), "augmented stop set must contain {id}");
        }
    }

    #[test]
    fn no_controls_leaves_base_unchanged() {
        let base = stop_token_set(Some(2), &[]);
        let s = stop_token_set_with_controls(base.clone(), &[]);
        assert_eq!(s, base);
    }
}
