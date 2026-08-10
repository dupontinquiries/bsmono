//! model_picker — the model-discovery logic from `hf_hub.rs`.
//!
//! Quant detection, fit estimation and URL construction for the GGUF picker.
//! The original says it is *"deliberately split so the parsing and URL
//! construction can be unit-tested standalone (no Qt, no llama.cpp, no
//! network)"* — that split is the port. Nothing here touches a socket, so the
//! whole picker's judgement is testable on a machine with no GPU and no
//! internet.
//!
//! ## Detection is by substring, and the original states why that is safe
//!
//! *"These substrings don't overlap across tiers (q4 vs q40 etc. don't collide
//! because the tier digit directly follows 'q')."* That claim is load-bearing —
//! it is the reason a table of `contains` is acceptable rather than a parser —
//! so it is asserted here rather than trusted, across every tier pair.
//!
//! ## Bytes-per-parameter is deliberately approximate
//!
//! *"Real GGUF sizes vary with the K-quant mixing — but close enough to steer a
//! default pick."* Worth preserving as stated: a picker that promised exact
//! sizes would be wrong on every mixed quant, and the number's whole job is to
//! answer "will this fit" well enough to order a list.

/// A quantisation tier, coarse enough to pick with.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum QuantTier { Q2, Q3, Q4, Q5, Q6, Q8, F16 }

impl QuantTier {
    pub const ALL: [QuantTier; 7] = [
        QuantTier::Q2, QuantTier::Q3, QuantTier::Q4,
        QuantTier::Q5, QuantTier::Q6, QuantTier::Q8, QuantTier::F16,
    ];

    pub fn label(self) -> &'static str {
        match self {
            QuantTier::Q2 => "Q2", QuantTier::Q3 => "Q3", QuantTier::Q4 => "Q4",
            QuantTier::Q5 => "Q5", QuantTier::Q6 => "Q6", QuantTier::Q8 => "Q8",
            QuantTier::F16 => "F16",
        }
    }

    /// Rough bytes per parameter. Approximate by design — see the module docs.
    pub fn bytes_per_param(self) -> f32 {
        match self {
            QuantTier::Q2 => 0.33, QuantTier::Q3 => 0.43, QuantTier::Q4 => 0.56,
            QuantTier::Q5 => 0.68, QuantTier::Q6 => 0.82, QuantTier::Q8 => 1.06,
            QuantTier::F16 => 2.0,
        }
    }

    /// Substrings that identify this tier in a filename.
    ///
    /// Kept as data rather than an if-chain so the non-overlap claim the
    /// original rests on can be checked mechanically.
    pub fn patterns(self) -> &'static [&'static str] {
        match self {
            QuantTier::Q2 => &["q2_", "q2k", "-q2", ".q2"],
            QuantTier::Q3 => &["q3_", "q3k", "-q3", ".q3"],
            QuantTier::Q4 => &["q4_", "q4k", "-q4", ".q4"],
            QuantTier::Q5 => &["q5_", "q5k", "-q5", ".q5"],
            QuantTier::Q6 => &["q6_", "q6k", "-q6", ".q6"],
            QuantTier::Q8 => &["q8_", "q8k", "-q8", ".q8"],
            QuantTier::F16 => &["f16", "fp16", "-16b"],
        }
    }

    /// Estimated file size in GiB for a model of `params_b` billion parameters.
    pub fn approx_gib(self, params_b: f32) -> f32 {
        params_b * 1e9 * self.bytes_per_param() / (1u64 << 30) as f32
    }
}

/// Detect the tier from a GGUF filename, case-insensitively.
///
/// `None` rather than a default: a file whose quant cannot be read is a file
/// whose size cannot be estimated, and guessing Q4 would make it sort as if it
/// were known to fit.
pub fn detect_quant(filename: &str) -> Option<QuantTier> {
    let lc = filename.to_lowercase();
    QuantTier::ALL.iter().copied().find(|t| t.patterns().iter().any(|p| lc.contains(p)))
}

/// Does a model of this tier fit a RAM budget, leaving room to run it?
///
/// The headroom is the point: a file that exactly fills memory cannot be
/// inferred with, since the KV cache and the runtime need somewhere to live.
pub const RUNTIME_HEADROOM_GIB: f32 = 1.5;

pub fn fits(tier: QuantTier, params_b: f32, budget_gib: f32) -> bool {
    tier.approx_gib(params_b) + RUNTIME_HEADROOM_GIB <= budget_gib
}

/// The largest tier that fits a budget — the picker's default.
///
/// Largest rather than smallest: quality is the thing being traded away, so
/// take as little of it as the machine forces.
pub fn best_fitting(params_b: f32, budget_gib: f32) -> Option<QuantTier> {
    QuantTier::ALL.iter().copied().rev().find(|t| fits(*t, params_b, budget_gib))
}

/// Percent-encode a query for a URL.
///
/// Hand-rolled to match the project's no-serde stance. Unreserved characters
/// per RFC 3986 pass through; everything else is escaped — including the space,
/// which is the one a model search will actually contain.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub const HUB: &str = "https://huggingface.co";

/// `GET /api/models?search=…&filter=gguf&sort=downloads`
///
/// `filter=gguf` is not optional: without it the search returns every format
/// the Hub hosts, and a picker that offers a safetensors repo to a GGUF loader
/// is offering a download that cannot be opened.
pub fn search_url(query: &str, limit: u32) -> String {
    format!("{HUB}/api/models?search={}&filter=gguf&sort=downloads&direction=-1&limit={}",
            url_encode(query), limit.clamp(1, 100))
}

pub fn repo_files_url(repo_id: &str) -> String {
    format!("{HUB}/api/models/{}?expand=siblings", repo_id)
}

/// The resolve URL a download actually pulls from.
///
/// `resolve/main` rather than `blob/main`: the blob path returns an HTML page
/// and the file would arrive as a few kilobytes of markup with a .gguf name —
/// a failure that surfaces as a corrupt-model error much later.
pub fn download_url(repo_id: &str, filename: &str) -> String {
    format!("{HUB}/{}/resolve/main/{}", repo_id, filename)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_examples_detect() {
        assert_eq!(detect_quant("model-Q4_K_M.gguf"), Some(QuantTier::Q4));
        assert_eq!(detect_quant("model.Q8_0.gguf"), Some(QuantTier::Q8));
        assert_eq!(detect_quant("model-f16.gguf"), Some(QuantTier::F16));
    }

    #[test]
    fn tier_patterns_do_not_overlap() {
        // The original rests on this: "these substrings don't overlap across
        // tiers (q4 vs q40 etc. don't collide because the tier digit directly
        // follows 'q')". Asserted rather than trusted — a future tier added
        // with a careless pattern would silently capture another's files.
        for (i, a) in QuantTier::ALL.iter().enumerate() {
            for b in QuantTier::ALL.iter().skip(i + 1) {
                for pa in a.patterns() {
                    for pb in b.patterns() {
                        assert!(!pa.contains(pb) && !pb.contains(pa),
                                "{} pattern {pa} collides with {} pattern {pb}",
                                a.label(), b.label());
                    }
                }
            }
        }
    }

    #[test]
    fn detection_is_case_insensitive_both_ways() {
        assert_eq!(detect_quant("MODEL-Q5_K_S.GGUF"), Some(QuantTier::Q5));
        assert_eq!(detect_quant("model-q5_k_s.gguf"), Some(QuantTier::Q5));
    }

    #[test]
    fn an_unreadable_quant_is_none_rather_than_a_default() {
        // Guessing Q4 would make an unknown file sort as if it were known to
        // fit, which is the one error a picker cannot recover from.
        assert_eq!(detect_quant("model.gguf"), None);
        assert_eq!(detect_quant(""), None);
        assert_eq!(detect_quant("llama-3-instruct.gguf"), None);
    }

    #[test]
    fn a_bigger_tier_is_a_bigger_file() {
        // The ordering the picker sorts on; an inversion would recommend the
        // largest file as the safest.
        for w in QuantTier::ALL.windows(2) {
            assert!(w[1].bytes_per_param() > w[0].bytes_per_param(),
                    "{} is not larger than {}", w[1].label(), w[0].label());
        }
    }

    #[test]
    fn the_budget_leaves_room_to_actually_run_the_model() {
        // A file that exactly fills memory cannot be inferred with: the KV
        // cache and the runtime need somewhere to live.
        let exact = QuantTier::Q4.approx_gib(8.0);
        assert!(!fits(QuantTier::Q4, 8.0, exact), "a model that exactly fills RAM was accepted");
        assert!(fits(QuantTier::Q4, 8.0, exact + RUNTIME_HEADROOM_GIB));
    }

    #[test]
    fn the_default_pick_is_the_largest_that_fits() {
        // Quality is what is being traded away, so take as little of it as the
        // machine forces.
        let pick = best_fitting(8.0, 16.0).expect("something fits 16 GiB");
        assert!(pick >= QuantTier::Q4, "picked {} on a 16 GiB machine", pick.label());
        let tight = best_fitting(70.0, 8.0);
        assert!(tight.is_none(), "a 70B does not fit 8 GiB at any tier: {tight:?}");
    }

    #[test]
    fn a_tiny_model_gets_the_best_tier_available() {
        assert_eq!(best_fitting(1.0, 64.0), Some(QuantTier::F16));
    }

    #[test]
    fn a_search_query_with_spaces_is_encoded() {
        // The one a model search will actually contain; an unencoded space
        // makes the request malformed rather than empty, which is harder to
        // diagnose.
        let u = search_url("llama 3 instruct", 20);
        assert!(u.contains("search=llama%203%20instruct"), "{u}");
        assert!(!u.contains(' '), "{u}");
    }

    #[test]
    fn the_search_filters_to_gguf() {
        // Without it the Hub returns every format it hosts, and offering a
        // safetensors repo to a GGUF loader is offering a download that cannot
        // be opened.
        assert!(search_url("qwen", 10).contains("filter=gguf"));
    }

    #[test]
    fn the_limit_is_clamped_rather_than_passed_through() {
        // The Hub caps it anyway; sending 100000 gets an error page instead of
        // a short list.
        assert!(search_url("q", 0).contains("limit=1"));
        assert!(search_url("q", 100_000).contains("limit=100"));
    }

    #[test]
    fn a_download_uses_resolve_not_blob() {
        // blob/main returns an HTML page: the file arrives as a few kilobytes
        // of markup with a .gguf name, and fails much later as a corrupt model.
        let u = download_url("bartowski/Qwen3-14B-GGUF", "Qwen3-14B-Q4_K_M.gguf");
        assert!(u.contains("/resolve/main/"), "{u}");
        assert!(!u.contains("/blob/"), "{u}");
    }

    #[test]
    fn url_encoding_leaves_unreserved_characters_alone() {
        // Over-encoding a repo id would break the path; RFC 3986 unreserved
        // must pass through.
        assert_eq!(url_encode("abc-DEF_123.gguf~"), "abc-DEF_123.gguf~");
        assert_eq!(url_encode("a/b"), "a%2Fb");
        assert_eq!(url_encode("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn a_repo_id_with_a_slash_survives_the_files_url() {
        // Repo ids are always owner/name, so the slash must NOT be encoded
        // here — it is path structure, not data.
        let u = repo_files_url("bartowski/Qwen3-14B-GGUF");
        assert!(u.contains("/api/models/bartowski/Qwen3-14B-GGUF"), "{u}");
        assert!(u.ends_with("?expand=siblings"), "{u}");
    }
}
