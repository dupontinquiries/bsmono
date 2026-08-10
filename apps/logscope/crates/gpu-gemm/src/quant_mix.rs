//! quant_mix — which checkpoint types survive loading, and what conversion costs.
//!
//! ## The bug this exists for
//!
//! `Llama-4-Scout-17B-16E-Instruct-UD-IQ1_S.gguf` loads slower than a plain q4
//! model and ends up bigger. The name says IQ1_S, and the loader has a native
//! IQ1_S path — but the gate is `t.ggml_type == 19`, and **`UD-` is an Unsloth
//! Dynamic mix**: only some tensors are IQ1_S. The rest are IQ1_M, IQ2_XXS,
//! IQ4_NL, Q5_K, Q6_K, chosen per tensor because that is the entire point of a
//! dynamic quant. Every one of those falls through to fetch-f32-and-requantize.
//!
//! So the file loads as *"the mix, converted to q4"* — which is both slower to
//! load than a real q4 file (a dequantize and requantize pass per tensor that
//! q4 does not pay) and larger in memory (IQ1_M at 1.75 bpw becomes q4's 4.5).
//! The user's report is exactly right, and the two symptoms have one cause.
//!
//! ## The policy
//!
//! A type is either **native** (its file bytes are the resident form), **cheap**
//! (a size-neutral or shrinking GPU-side conversion), or **inflating** — and
//! only the last is worth refusing or warning about. Inflation is the number
//! that matters: converting up *"cannot recover quality the checkpoint already
//! discarded; it only spends the memory."*

/// A GGUF quantisation type, by its ggml type id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct QuantType(pub u32);

/// How the loader can handle a type today.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handling {
    /// File bytes go straight to the GPU; no conversion pass.
    Native,
    /// Converted on the GPU without a host round trip, size-neutral or better.
    CheapConvert,
    /// Dequantised to f32 and requantised to q4 — the slow, inflating path.
    Inflates,
}

/// Bits per weight for a type, from the GGUF block layouts.
pub fn bits_per_weight(t: QuantType) -> Option<f32> {
    Some(match t.0 {
        0 => 32.0,                      // F32
        1 => 16.0,                      // F16
        2 => 4.5,                       // Q4_0
        3 => 5.0,                       // Q4_1
        8 => 8.5,                       // Q8_0
        10 => 2.625,                    // Q2_K
        11 => 3.4375,                   // Q3_K
        12 => 4.5,                      // Q4_K
        13 => 5.5,                      // Q5_K
        14 => 6.5625,                   // Q6_K
        16 => 2.0625,                   // IQ2_XXS
        17 => 2.3125,                   // IQ2_XS
        19 => 1.5625,                   // IQ1_S
        20 => 4.5,                      // IQ4_NL
        21 => 2.5625,                   // IQ3_S
        23 => 1.75,                     // IQ1_M
        24 => 4.25,                     // IQ4_XS
        _ => return None,
    })
}

/// What the resident form costs, in bits per weight.
///
/// q4 for everything the loader converts.
pub const RESIDENT_Q4_BPW: f32 = 4.5;

/// Types the loader holds natively today.
///
/// Deliberately a list rather than a range: it is the answer to "what did this
/// build actually implement", and a range would silently claim types nobody
/// wrote a kernel for.
pub const NATIVE_TODAY: &[u32] = &[8, 12, 19];

/// How a type is handled by the current loader.
pub fn handling(t: QuantType) -> Handling {
    if NATIVE_TODAY.contains(&t.0) { return Handling::Native; }
    match bits_per_weight(t) {
        // Already at or above the resident cost: converting does not inflate,
        // it just costs a pass. Worth doing on the GPU, not worth refusing.
        Some(b) if b >= RESIDENT_Q4_BPW => Handling::CheapConvert,
        _ => Handling::Inflates,
    }
}

/// Memory multiplier from converting `t` to the q4 resident form.
///
/// Above 1.0 means the file grows in memory. This is the number the IQ1_S note
/// already records as 2.88× — reproduced here as arithmetic so it holds for
/// every type in a mix rather than the one somebody measured.
pub fn inflation(t: QuantType) -> Option<f32> {
    bits_per_weight(t).map(|b| RESIDENT_Q4_BPW / b)
}

/// One tensor of a checkpoint, as the loader sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct TensorInfo {
    pub name: String,
    pub ggml_type: u32,
    /// Weight count.
    pub elements: u64,
    /// Row length; the native paths need this to be a whole number of blocks.
    pub k: u64,
}

/// Bytes a tensor occupies in the file.
pub fn file_bytes(t: &TensorInfo) -> Option<u64> {
    bits_per_weight(QuantType(t.ggml_type)).map(|b| (t.elements as f64 * b as f64 / 8.0) as u64)
}

/// Bytes it will occupy resident, under the current loader.
pub fn resident_bytes(t: &TensorInfo) -> Option<u64> {
    match handling(QuantType(t.ggml_type)) {
        Handling::Native => file_bytes(t),
        _ => Some((t.elements as f64 * RESIDENT_Q4_BPW as f64 / 8.0) as u64),
    }
}

/// What a mixed checkpoint costs to load.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MixReport {
    pub file_bytes: u64,
    pub resident_bytes: u64,
    /// Tensors that inflate, with their multiplier, worst first.
    pub inflating: Vec<(String, f32)>,
    /// Type ids present that this build cannot size at all.
    pub unknown_types: Vec<u32>,
    /// Tensors converted only because their row length is not block-aligned.
    pub misaligned: Vec<String>,
}

impl MixReport {
    /// Does this checkpoint pay for conversion it should not?
    pub fn has_avoidable_cost(&self) -> bool {
        !self.inflating.is_empty() || !self.misaligned.is_empty()
    }

    pub fn expansion(&self) -> f32 {
        if self.file_bytes == 0 { return 1.0; }
        self.resident_bytes as f32 / self.file_bytes as f32
    }
}

/// Block size in weights for a type — a native path needs `k` to be a multiple.
pub fn block_weights(t: QuantType) -> u64 {
    match t.0 {
        8 => 32,                        // Q8_0
        2 | 3 => 32,                    // Q4_0 / Q4_1
        _ => 256,                       // K- and I-quants
    }
}

/// Audit a checkpoint's tensor list.
///
/// The point is to answer "why is this file slower and bigger than its name
/// suggests" BEFORE a 17B download finishes, from the header alone.
pub fn audit(tensors: &[TensorInfo]) -> MixReport {
    let mut r = MixReport::default();
    for t in tensors {
        let qt = QuantType(t.ggml_type);
        match file_bytes(t) {
            Some(fb) => {
                r.file_bytes += fb;
                r.resident_bytes += resident_bytes(t).unwrap_or(fb);
            }
            None => {
                if !r.unknown_types.contains(&t.ggml_type) {
                    r.unknown_types.push(t.ggml_type);
                }
                continue;
            }
        }
        if handling(qt) == Handling::Inflates {
            if let Some(m) = inflation(qt) { r.inflating.push((t.name.clone(), m)); }
        } else if handling(qt) == Handling::Native && t.k % block_weights(qt) != 0 {
            // Native in principle, converted in practice — the quiet case, since
            // the type check passes and the alignment check does not.
            r.misaligned.push(t.name.clone());
        }
    }
    r.inflating.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.0.cmp(&b.0)));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str, ty: u32, elements: u64, k: u64) -> TensorInfo {
        TensorInfo { name: name.into(), ggml_type: ty, elements, k }
    }

    /// A UD-IQ1_S mix: the headline type plus the others such files really use.
    fn ud_mix() -> Vec<TensorInfo> {
        vec![
            t("blk.0.ffn_gate_exps.weight", 19, 16 * 4096 * 8192, 4096),   // IQ1_S
            t("blk.0.ffn_up_exps.weight", 23, 16 * 4096 * 8192, 4096),     // IQ1_M
            t("blk.0.attn_q.weight", 14, 4096 * 4096, 4096),               // Q6_K
            t("blk.0.attn_k.weight", 12, 1024 * 4096, 4096),               // Q4_K
            t("output.weight", 20, 4096 * 32000, 4096),                    // IQ4_NL
        ]
    }

    #[test]
    fn the_headline_type_is_native_but_the_mix_is_not() {
        // The bug in one assertion: the file is called IQ1_S and its IQ1_S
        // tensors load natively, while everything else in the mix does not.
        assert_eq!(handling(QuantType(19)), Handling::Native);
        assert_eq!(handling(QuantType(23)), Handling::Inflates, "IQ1_M");
        assert_eq!(handling(QuantType(16)), Handling::Inflates, "IQ2_XXS");
    }

    #[test]
    fn iq1_m_inflates_and_the_report_names_it() {
        // 1.75 bpw to 4.5 is 2.57x. A user sees "IQ1_S" on the tin and gets
        // q4 memory for the largest tensors in the file.
        let r = audit(&ud_mix());
        assert!(r.has_avoidable_cost());
        assert_eq!(r.inflating[0].0, "blk.0.ffn_up_exps.weight",
                   "the worst offender is named first: {:?}", r.inflating);
        assert!((r.inflating[0].1 - 4.5 / 1.75).abs() < 0.01);
    }

    #[test]
    fn the_expansion_matches_the_documented_iq1_s_figure() {
        // The existing note records 2.88x for a pure IQ1_S file; the arithmetic
        // must reproduce it rather than the two disagreeing.
        let pure = vec![t("w", 19, 1_000_000, 4096)];
        let r = audit(&pure);
        assert_eq!(r.expansion(), 1.0, "native: no expansion at all");
        assert!((inflation(QuantType(19)).unwrap() - 2.88).abs() < 0.01,
                "the conversion multiplier is 2.88x if it were converted");
    }

    #[test]
    fn a_type_already_at_or_above_q4_does_not_inflate() {
        // Q6_K and IQ4_NL cost a pass but not memory, so they are worth
        // converting on the GPU rather than refusing.
        assert_eq!(handling(QuantType(14)), Handling::CheapConvert, "Q6_K");
        assert_eq!(handling(QuantType(20)), Handling::CheapConvert, "IQ4_NL");
        assert_eq!(handling(QuantType(13)), Handling::CheapConvert, "Q5_K");
    }

    #[test]
    fn a_pure_q4_file_is_size_neutral_end_to_end() {
        // The comparison the user made: the q4 model loads faster. It should,
        // and this says why — nothing in it converts.
        let q4 = vec![t("a", 12, 4_000_000, 4096), t("b", 12, 4_000_000, 4096)];
        let r = audit(&q4);
        assert!(!r.has_avoidable_cost());
        assert_eq!(r.expansion(), 1.0);
    }

    #[test]
    fn a_misaligned_native_tensor_is_reported_separately() {
        // The quiet case: the type check passes and the alignment check does
        // not, so a tensor the loader believes is native still converts. Two
        // different fixes, so two different lists.
        let odd = vec![t("blk.0.odd.weight", 19, 100 * 200, 200)];
        let r = audit(&odd);
        assert_eq!(r.misaligned, vec!["blk.0.odd.weight"]);
        assert!(r.inflating.is_empty(), "it is not an inflation problem");
        assert!(r.has_avoidable_cost());
    }

    #[test]
    fn an_unknown_type_is_listed_rather_than_guessed() {
        // A newer quant this build predates. Guessing its bit width would
        // report a confident wrong size for the whole checkpoint.
        let r = audit(&[t("w", 99, 1000, 256)]);
        assert_eq!(r.unknown_types, vec![99]);
        assert_eq!(r.file_bytes, 0, "it contributes nothing rather than a guess");
    }

    #[test]
    fn an_unknown_type_is_reported_once_however_many_tensors_use_it() {
        let r = audit(&[t("a", 99, 10, 256), t("b", 99, 10, 256), t("c", 99, 10, 256)]);
        assert_eq!(r.unknown_types.len(), 1);
    }

    #[test]
    fn the_report_orders_by_how_much_is_wasted() {
        // A fix list is only useful in the order you would do it.
        let mix = vec![
            t("mild", 21, 1000, 256),      // IQ3_S 2.5625 -> 1.76x
            t("worst", 19, 1000, 256),     // treated as inflating below
            t("bad", 23, 1000, 256),       // IQ1_M 1.75 -> 2.57x
        ];
        let mut m = mix.clone();
        m[1].ggml_type = 16;               // IQ2_XXS 2.0625 -> 2.18x
        let r = audit(&m);
        let names: Vec<&str> = r.inflating.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["bad", "worst", "mild"]);
    }

    #[test]
    fn block_sizes_match_the_families() {
        // Q8_0 is 32 weights per block, K- and I-quants are 256. Using 256 for
        // Q8_0 would reject aligned tensors as misaligned.
        assert_eq!(block_weights(QuantType(8)), 32);
        assert_eq!(block_weights(QuantType(19)), 256);
        assert_eq!(block_weights(QuantType(12)), 256);
    }

    #[test]
    fn an_empty_checkpoint_reports_no_expansion_rather_than_dividing_by_zero() {
        let r = audit(&[]);
        assert_eq!(r.expansion(), 1.0);
        assert!(!r.has_avoidable_cost());
    }

    #[test]
    fn the_native_list_is_what_this_build_implements() {
        // A range would silently claim types nobody wrote a kernel for. If a
        // kernel lands, this list is the one line to change.
        assert_eq!(NATIVE_TODAY, &[8, 12, 19]);
        for ty in NATIVE_TODAY { assert_eq!(handling(QuantType(*ty)), Handling::Native); }
    }
}

/// A one-screen explanation of why a checkpoint loads slowly, for the load log.
///
/// Written for the moment the user actually has: a file named IQ1_S that is
/// slower and larger than a q4 file. The report names the tensors, the
/// multiplier and the missing kernel, so the answer is on screen rather than
/// inferred from a stopwatch.
pub fn explain(r: &MixReport) -> String {
    if !r.has_avoidable_cost() && r.unknown_types.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    if !r.inflating.is_empty() {
        let mut kinds: Vec<(f32, usize)> = Vec::new();
        for (_, m) in &r.inflating {
            match kinds.iter_mut().find(|(k, _)| (k - m).abs() < 1e-4) {
                Some((_, n)) => *n += 1,
                None => kinds.push((*m, 1)),
            }
        }
        out.push_str(&format!(
            "{} tensor(s) are being converted to q4 because this build has no native kernel \
             for their type. Resident size is {:.2}x the file.\n",
            r.inflating.len(), r.expansion()));
        for (m, n) in kinds {
            out.push_str(&format!("  {n} tensor(s) inflate {m:.2}x\n"));
        }
        out.push_str("  This is why a mixed 'UD-' checkpoint can load slower and larger than a \
                      plain q4 file: q4 pays no conversion pass at all.\n");
    }
    if !r.misaligned.is_empty() {
        out.push_str(&format!(
            "{} tensor(s) have a native type but a row length that is not a whole number of \
             blocks, so they convert anyway.\n", r.misaligned.len()));
    }
    if !r.unknown_types.is_empty() {
        out.push_str(&format!("unknown ggml type(s) {:?} — sizes above exclude them.\n",
                              r.unknown_types));
    }
    out
}

#[cfg(test)]
mod explain_tests {
    use super::*;

    fn t(name: &str, ty: u32, elements: u64, k: u64) -> TensorInfo {
        TensorInfo { name: name.into(), ggml_type: ty, elements, k }
    }

    #[test]
    fn a_clean_checkpoint_explains_nothing() {
        // Silence is the correct output when there is nothing to fix; a line
        // on every load teaches the user to skip the log.
        let r = audit(&[t("a", 12, 1000, 256)]);
        assert!(explain(&r).is_empty());
    }

    #[test]
    fn the_explanation_names_the_multiplier_and_the_cause() {
        // The user's actual question — "why is the IQ1_S file slower than q4" —
        // should be answered on screen rather than inferred from a stopwatch.
        let r = audit(&[t("blk.0.ffn_up_exps.weight", 23, 1_000_000, 4096)]);
        let e = explain(&r);
        assert!(e.contains("2.57"), "names the multiplier: {e}");
        assert!(e.contains("no native kernel"), "names the cause: {e}");
        assert!(e.contains("plain q4"), "answers the comparison the user made: {e}");
    }

    #[test]
    fn identical_multipliers_are_grouped_rather_than_listed_per_tensor() {
        // A 17B mix has hundreds of expert tensors; one line each buries the
        // answer in the thing it is explaining.
        let many: Vec<TensorInfo> = (0..200)
            .map(|i| t(&format!("blk.{i}.ffn_up_exps.weight"), 23, 1000, 256)).collect();
        let e = explain(&audit(&many));
        assert!(e.lines().count() < 6, "grouped, not 200 lines:\n{e}");
        assert!(e.contains("200 tensor(s)"));
    }

    #[test]
    fn misalignment_and_inflation_are_reported_as_different_problems() {
        // They need different fixes: one wants a kernel, the other wants
        // padding or a slice path.
        let r = audit(&[t("a", 23, 1000, 256), t("b", 19, 200, 200)]);
        let e = explain(&r);
        assert!(e.contains("no native kernel"));
        assert!(e.contains("not a whole number of"), "{e}");
    }
}
