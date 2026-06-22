//! estimate.rs — a calmer progress signal than raw tok/s. For an average user, "13.8 tok/s" means
//! nothing; "about halfway" does. We predict how long *this* generation will run by looking at prior
//! generations in the conversation: a turn whose prompt size is close to one we've already run is
//! likely to produce a similar number of tokens at a similar rate. Pure + testable; the UI turns a
//! prediction into a progress fraction (and a loading ring/sphere). Raw tok/s stays available behind
//! LOGSCOPE_DEBUG for power users.

/// One completed generation: how big the prompt was, how many tokens it produced, how long it took.
#[derive(Clone, Copy, Debug)]
pub struct GenSample { pub prompt_tokens: usize, pub output_tokens: usize, pub secs: f32 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prediction {
    pub output_tokens: usize, // expected total tokens this generation will produce
    pub secs: f32,            // expected total wall-clock seconds
    pub from_similar: bool,   // true = matched on prompt-size similarity; false = global fallback
}

/// Predict this generation's size/time from past ones. Prefers samples whose prompt size is within
/// `band` (fractional, e.g. 0.35 = ±35%) of `cur_prompt_tokens` — "this ran with ~similar inputs, so
/// expect ~similar output." Falls back to the median of all samples. None when there's no history.
pub fn predict(history: &[GenSample], cur_prompt_tokens: usize, band: f32) -> Option<Prediction> {
    if history.is_empty() { return None; }
    let cur = cur_prompt_tokens.max(1) as f32;
    let band = band.clamp(0.05, 1.0);
    let similar: Vec<&GenSample> = history.iter()
        .filter(|s| {
            let p = s.prompt_tokens.max(1) as f32;
            (p - cur).abs() / cur <= band
        })
        .collect();
    let (pool, from_similar): (Vec<&GenSample>, bool) = if similar.len() >= 2 {
        (similar, true)
    } else {
        (history.iter().collect(), false)
    };
    // average tokens; derive seconds from the pool's average per-token rate (robust to one slow run)
    let n = pool.len() as f32;
    let avg_out = pool.iter().map(|s| s.output_tokens).sum::<usize>() as f32 / n;
    let total_secs: f32 = pool.iter().map(|s| s.secs).sum();
    let total_out: f32 = pool.iter().map(|s| s.output_tokens.max(1)).sum::<usize>() as f32;
    let per_tok = if total_out > 0.0 { total_secs / total_out } else { 0.0 };
    Some(Prediction {
        output_tokens: avg_out.round().max(1.0) as usize,
        secs: (avg_out * per_tok).max(0.0),
        from_similar,
    })
}

/// Progress fraction in [0,1] given tokens produced so far and a prediction. Clamped, and never quite
/// reaches 1.0 until the stream actually ends (so a ring doesn't sit "full" while still decoding).
pub fn progress(produced: usize, pred: &Prediction) -> f32 {
    if pred.output_tokens == 0 { return 0.0; }
    (produced as f32 / pred.output_tokens as f32).clamp(0.0, 0.98)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(p: usize, o: usize, t: f32) -> GenSample { GenSample { prompt_tokens: p, output_tokens: o, secs: t } }

    #[test]
    fn no_history_no_prediction() {
        assert!(predict(&[], 500, 0.35).is_none());
    }

    #[test]
    fn matches_on_similar_prompt_size() {
        // two small-prompt runs (~100 tok) and two big-prompt runs (~1000 tok) with different outputs
        let h = vec![s(100, 40, 3.0), s(110, 50, 3.5), s(1000, 300, 22.0), s(980, 320, 23.0)];
        let p = predict(&h, 105, 0.35).unwrap();
        assert!(p.from_similar, "a ~105-tok prompt matches the small-prompt runs");
        assert!(p.output_tokens >= 40 && p.output_tokens <= 60, "predicts ~45 tokens, got {}", p.output_tokens);
        let big = predict(&h, 990, 0.35).unwrap();
        assert!(big.output_tokens >= 290 && big.output_tokens <= 330, "big prompt predicts ~310, got {}", big.output_tokens);
    }

    #[test]
    fn falls_back_to_global_median_when_no_match() {
        let h = vec![s(100, 40, 3.0), s(120, 60, 4.0)];
        let p = predict(&h, 5000, 0.35).unwrap(); // nothing near 5000
        assert!(!p.from_similar, "no similar prompt → global fallback");
        assert!(p.output_tokens >= 40 && p.output_tokens <= 60);
    }

    #[test]
    fn progress_clamps_and_never_completes_early() {
        let pred = Prediction { output_tokens: 100, secs: 8.0, from_similar: true };
        assert_eq!(progress(0, &pred), 0.0);
        assert!((progress(50, &pred) - 0.5).abs() < 0.001);
        assert!(progress(200, &pred) <= 0.98, "overshoot clamps below full");
    }
}
