//! imrope.rs — what ggml's multi-section RoPE does to TEXT, decided per file rather than assumed.
//!
//! Qwen3.5 / Qwen3.6 / Ornith (dense and MoE) and the Qwen-VL line rope through `ggml_rope_multi`
//! with `rope.dimension_sections`: each rotary pair takes its position from one of four channels
//! (t, h, w, e). This engine ropes text with one position per token. `archfeat` carried the gap as
//! "claimed equivalent for text … UNVERIFIED; check ggml-cpu's rope … do not flip this flag on the
//! equivalence argument alone" — and the dense qwen35 path assumed the collapse anyway, unguarded.
//!
//! This module is that check, transcribed from the source rather than argued:
//!
//!   * `ggml/src/ggml-cpu/ops.cpp`, `ggml_mrope_cache_init`: with `indep_sects` false (every mode
//!     but VISION) all four thetas start at their channel's position and are ALL multiplied by
//!     `theta_scale` every pair. So at pair i, theta_X = p_X · theta_scale^i for every channel —
//!     the channels differ only in p_X, never in frequency.
//!   * `src/llama-graph.cpp`, `llm_graph_input_pos::set_input`: for text tokens under a 4-position
//!     model, "the 3 first dims are the same, and 4th dim is all 0" — p_t = p_h = p_w = pos, p_e = 0.
//!   * the rotation itself is `rotate_pairs(n_dims, n_dims/2, …)` — NEOX pairing — for MROPE and
//!     IMROPE alike.
//!
//! Therefore: for text, multi-section rope IS plain NEOX rope over `n_rot` dims **iff no rotary
//! pair selects the e channel**, because e is the one channel whose position is not `pos`. A pair
//! that does select it gets theta = 0 and is not rotated at all. Whether any pair does depends on
//! the file's sections, so it is decided per file: [11, 11, 10, 0] over 64 dims (every Qwen3.5 /
//! Ornith-1.5 checkpoint) selects none; interleaved [16, 8, 8, 0] sends pairs 25, 26, 28, 29 and 31 to e.
//!
//! What this does NOT cover: image/video tokens, where h and w differ from t. Those need real
//! per-channel positions and are not implemented; this engine is text-only and says so.

/// Which position channel a rotary pair reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel { T, H, W, E }

/// ggml's two non-vision multi-section layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// GGML_ROPE_TYPE_MROPE — contiguous sections [t…][h…][w…][e…] (Qwen2-VL / Qwen2.5-VL).
    Sectioned,
    /// GGML_ROPE_TYPE_IMROPE — t/h/w interleaved by `sector % 3` (Qwen3-VL, Qwen3.5, Qwen3.5-MoE).
    Interleaved,
}

impl Layout {
    /// llama.cpp's `llama_model_rope_type`, for the architectures that use sections at all.
    /// None = this architecture does not rope through sections; ignore the key if present.
    pub fn for_arch(arch: &str) -> Option<Layout> {
        match arch {
            "qwen2vl" => Some(Layout::Sectioned),
            "qwen3vl" | "qwen3vlmoe" | "qwen35" | "qwen3_5" | "qwen35moe" | "qwen36moe" | "qwen3_5moe" | "qwen3next" => Some(Layout::Interleaved),
            _ => None,
        }
    }
}

/// The channel pair `pair` (= i0/2 in ggml) reads. A transcription of the selection in
/// `ggml_mrope_cache_init`; the branch order is ggml's, including its fall-through to e.
pub fn channel_of(pair: usize, sections: [u32; 4], layout: Layout) -> Channel {
    let s = [sections[0] as usize, sections[1] as usize, sections[2] as usize, sections[3] as usize];
    let sect_dims = s[0] + s[1] + s[2] + s[3];
    let sector = pair % sect_dims.max(1);
    match layout {
        Layout::Interleaved => {
            if sector % 3 == 1 && sector < 3 * s[1] { Channel::H }
            else if sector % 3 == 2 && sector < 3 * s[2] { Channel::W }
            else if sector % 3 == 0 && sector < 3 * s[0] { Channel::T }
            else { Channel::E }
        }
        Layout::Sectioned => {
            let sec_w = s[0] + s[1];
            if sector >= sec_w + s[2] { Channel::E }
            else if sector >= sec_w { Channel::W }
            else if sector >= s[0] { Channel::H }
            else { Channel::T }
        }
    }
}

/// Why a file's sections cannot be served by single-position NEOX rope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotPlain {
    /// ggml asserts at least one of t/h/w is non-zero; a file without that would abort there.
    EmptySections,
    /// ggml asserts `sect_dims <= ne0`.
    SectionsExceedHead { sect_dims: usize, head_dim: usize },
    /// These pairs read the e channel, whose text position is 0: ggml leaves them UNROTATED while
    /// plain rope rotates them. Lists every such pair, not just the first.
    PairsOnE { pairs: Vec<usize> },
}

impl std::fmt::Display for NotPlain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotPlain::EmptySections => write!(f, "rope.dimension_sections has no t/h/w section (ggml asserts one)"),
            NotPlain::SectionsExceedHead { sect_dims, head_dim } =>
                write!(f, "rope.dimension_sections sum to {sect_dims}, more than the head's {head_dim} dims (ggml asserts sect_dims <= ne0)"),
            NotPlain::PairsOnE { pairs } =>
                write!(f, "rotary pairs {pairs:?} read the 4th position channel, which is 0 for text: the reference leaves them UNROTATED and single-position rope would rotate them"),
        }
    }
}

/// Is multi-section rope with these sections exactly plain NEOX rope over `n_rot` dims, for text?
pub fn text_is_plain_neox(sections: [u32; 4], n_rot: usize, head_dim: usize, layout: Layout) -> Result<(), NotPlain> {
    if sections[0] == 0 && sections[1] == 0 && sections[2] == 0 { return Err(NotPlain::EmptySections); }
    let sect_dims: usize = sections.iter().map(|&s| s as usize).sum();
    if sect_dims > head_dim { return Err(NotPlain::SectionsExceedHead { sect_dims, head_dim }); }
    let pairs: Vec<usize> = (0..n_rot / 2).filter(|&p| channel_of(p, sections, layout) == Channel::E).collect();
    if pairs.is_empty() { Ok(()) } else { Err(NotPlain::PairsOnE { pairs }) }
}

/// The angle table ggml builds for one token: theta per rotary pair, with `pos` = [t, h, w, e].
/// Iterative `theta *= theta_scale` in f32, as ggml does it (NOT pow per index) — so this can be
/// compared bit-for-bit against `neox_thetas`, which iterates the same way.
pub fn mrope_thetas(pos: [i64; 4], sections: [u32; 4], n_rot: usize, freq_base: f32, layout: Layout) -> Vec<f32> {
    let theta_scale = freq_base.powf(-2.0 / n_rot as f32);
    let mut th = [pos[0] as f32, pos[1] as f32, pos[2] as f32, pos[3] as f32];
    let mut out = Vec::with_capacity(n_rot / 2);
    for pair in 0..n_rot / 2 {
        out.push(match channel_of(pair, sections, layout) {
            Channel::T => th[0], Channel::H => th[1], Channel::W => th[2], Channel::E => th[3],
        });
        for t in th.iter_mut() { *t *= theta_scale; }
    }
    out
}

/// `ggml_rope_cache_init`'s thetas for plain rope at one position.
pub fn neox_thetas(pos: i64, n_rot: usize, freq_base: f32) -> Vec<f32> {
    let theta_scale = freq_base.powf(-2.0 / n_rot as f32);
    let mut theta = pos as f32;
    (0..n_rot / 2).map(|_| { let t = theta; theta *= theta_scale; t }).collect()
}

/// llama.cpp's text positions under a 4-channel model.
pub fn text_pos(pos: i64) -> [i64; 4] { [pos, pos, pos, 0] }

#[cfg(test)]
mod tests {
    use super::*;

    /// Ornith-1.5 (all three sizes) and every Qwen3.5 config: mrope_section [11, 11, 10],
    /// mrope_interleaved true, head_dim 256 × partial_rotary_factor 0.25 = 64 rotary dims.
    const ORNITH: [u32; 4] = [11, 11, 10, 0];

    #[test]
    fn ornith_sections_are_plain_neox_for_text_bit_for_bit() {
        assert_eq!(text_is_plain_neox(ORNITH, 64, 256, Layout::Interleaved), Ok(()));
        // and not merely "no pair on e": the whole angle table is the same f32s
        for pos in [0i64, 1, 7, 4095, 262_143] {
            let m = mrope_thetas(text_pos(pos), ORNITH, 64, 1.0e7, Layout::Interleaved);
            let n = neox_thetas(pos, 64, 1.0e7);
            assert_eq!(m.len(), 32);
            assert!(m.iter().zip(&n).all(|(a, b)| a.to_bits() == b.to_bits()), "pos {pos}: tables differ");
        }
        // the split is what the config says: 11 pairs on t, 11 on h, 10 on w
        let count = |c| (0..32).filter(|&p| channel_of(p, ORNITH, Layout::Interleaved) == c).count();
        assert_eq!((count(Channel::T), count(Channel::H), count(Channel::W), count(Channel::E)), (11, 11, 10, 0));
    }

    /// The complement: sections where the collapse is FALSE, so the check is shown to discriminate
    /// and not just to say yes. Interleaved [16, 8, 8]: h owns sectors ≡ 1 (mod 3) below 24 and w
    /// those ≡ 2 below 24, so 25, 28, 31 and 26, 29 fall through to e. (I first wrote this test
    /// expecting only h's three; an independent transcription of ggml's branch gave five.)
    #[test]
    fn interleaved_sections_that_reach_the_e_channel_are_refused_by_pair() {
        let s = [16, 8, 8, 0];
        assert_eq!(text_is_plain_neox(s, 64, 128, Layout::Interleaved), Err(NotPlain::PairsOnE { pairs: vec![25, 26, 28, 29, 31] }));
        // and the difference is real in the numbers: those pairs get theta 0 (unrotated)
        let m = mrope_thetas(text_pos(100), s, 64, 1.0e6, Layout::Interleaved);
        let n = neox_thetas(100, 64, 1.0e6);
        let on_e = [25usize, 26, 28, 29, 31];
        for p in on_e { assert_eq!(m[p], 0.0); assert!(n[p] != 0.0); }
        for p in (0..32).filter(|p| !on_e.contains(p)) { assert_eq!(m[p].to_bits(), n[p].to_bits(), "pair {p}"); }
        // The SAME sections under the contiguous layout are fine — layout is part of the question.
        assert_eq!(text_is_plain_neox(s, 64, 128, Layout::Sectioned), Ok(()));
    }

    /// Qwen2-VL's contiguous layout, its real sections: plain for text. With a non-empty 4th
    /// section the tail pairs read e and are refused — the sectioned branch's own fall-through.
    #[test]
    fn sectioned_layout_and_its_own_e_tail() {
        assert_eq!(text_is_plain_neox([16, 24, 24, 0], 128, 128, Layout::Sectioned), Ok(()));
        assert_eq!(text_is_plain_neox([8, 8, 8, 8], 64, 64, Layout::Sectioned),
                   Err(NotPlain::PairsOnE { pairs: (24..32).collect() }));
    }

    /// When sections cover fewer pairs than rotate, ggml wraps (`sector = pair % sect_dims`). The
    /// wrapped pairs still read t/h/w, so text stays plain — a short section list is not an error.
    #[test]
    fn sectors_wrap_and_stay_plain() {
        assert_eq!(text_is_plain_neox([4, 4, 4, 0], 64, 64, Layout::Interleaved), Ok(()));
        assert_eq!(channel_of(12, [4, 4, 4, 0], Layout::Interleaved), Channel::T); // 12 % 12 = 0
    }

    #[test]
    fn what_ggml_asserts_is_refused_here_too() {
        assert_eq!(text_is_plain_neox([0, 0, 0, 0], 64, 64, Layout::Interleaved), Err(NotPlain::EmptySections));
        assert_eq!(text_is_plain_neox([0, 0, 0, 8], 64, 64, Layout::Interleaved), Err(NotPlain::EmptySections));
        assert_eq!(text_is_plain_neox([40, 40, 40, 40], 64, 128, Layout::Sectioned),
                   Err(NotPlain::SectionsExceedHead { sect_dims: 160, head_dim: 128 }));
    }

    /// Why this module cannot bless image input: with h != t the tables differ, so "plain" is a
    /// statement about TEXT positions only.
    #[test]
    fn non_text_positions_are_not_plain() {
        let m = mrope_thetas([5, 9, 2, 0], ORNITH, 64, 1.0e7, Layout::Interleaved);
        let n = neox_thetas(5, 64, 1.0e7);
        assert!(m.iter().zip(&n).any(|(a, b)| a != b));
    }

    #[test]
    fn layouts_by_architecture() {
        for a in ["qwen35", "qwen3_5", "qwen35moe", "qwen36moe", "qwen3vl"] { assert_eq!(Layout::for_arch(a), Some(Layout::Interleaved), "{a}"); }
        assert_eq!(Layout::for_arch("qwen2vl"), Some(Layout::Sectioned));
        for a in ["llama", "qwen3", "qwen3moe", "gemma3"] { assert_eq!(Layout::for_arch(a), None, "{a}"); }
    }
}
