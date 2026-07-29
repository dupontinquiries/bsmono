//! tensornames.rs — one table for what a tensor is CALLED, following llama.cpp's `LLM_TENSOR`.
//!
//! ## What llama.cpp actually unifies, and what it deliberately does not
//!
//! llama.cpp faces the same combinatorial problem and answers it in a specific way:
//!
//! * `LLM_TENSOR_*` maps a logical ROLE (`ATTN_Q`, `FFN_GATE`) to the per-architecture filename.
//! * `LLM_KV` maps a canonical metadata key to its arch-prefixed spelling.
//! * `build_llama()`, `build_qwen2()`, … are **~60 separate graph functions**. There is no generic
//!   parameterised builder.
//!
//! So it table-drives the NAMES and duplicates the GRAPH. And it has no capability matrix at all: an
//! architecture is in the enum and works, or it is not supported. Binary, not a matrix.
//!
//! ## Why that matters here
//!
//! Five bugs this session were a registry disagreeing with what the loader does — `archfeat` claiming
//! Gemma support `archcfg` denied, `gemma4` requirements for a name the loader did not know, `glm-dsa`
//! sharing DeepSeek's metadata prefix, an MLA note four commits stale, and a split-layout flag that
//! stayed false after the code landed. Every one of them is the cost of maintaining a FEATURE MATRIX
//! beside the code rather than a per-arch path that either exists or does not.
//!
//! This does not undo that — the matrix is load-bearing now. What it fixes is the smaller, sharper
//! problem underneath it: tensor names are spelled inline at a dozen call sites, so a model that names
//! `attn_kv_b` differently needs every site found and edited. That is exactly how the split
//! `attn_k_b`/`attn_v_b` layout went unread for four commits after MLA was wired.
//!
//! One table, one place to add an alias, and a test that the aliases stay distinct.

/// What a tensor DOES, independent of what any file calls it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    AttnNorm, AttnQ, AttnK, AttnV, AttnOut,
    /// MLA: the low-rank query pair.
    AttnQA, AttnQB, AttnQANorm,
    /// MLA: the compressed KV path. `KvB` is the legacy fused form; `KB`/`VB` the modern split.
    AttnKvAMqa, AttnKvANorm, AttnKvB, AttnKB, AttnVB,
    FfnNorm, FfnGate, FfnUp, FfnDown,
    /// MoE: stacked per-expert tensors.
    FfnGateExps, FfnUpExps, FfnDownExps, FfnGateInp,
}

impl Role {
    /// Names this role is known by, most canonical first.
    ///
    /// Aliases exist because converters disagree — `attn_kv_a_mqa` and `attn_kv_a` are the same tensor
    /// from different tools. Listing them here means a new spelling is one line rather than a hunt
    /// through every call site.
    pub fn aliases(self) -> &'static [&'static str] {
        match self {
            Role::AttnNorm => &["attn_norm"],
            Role::AttnQ => &["attn_q"],
            Role::AttnK => &["attn_k"],
            Role::AttnV => &["attn_v"],
            Role::AttnOut => &["attn_output", "attn_out"],
            Role::AttnQA => &["attn_q_a", "attn_q_a_proj"],
            Role::AttnQB => &["attn_q_b", "attn_q_b_proj"],
            Role::AttnQANorm => &["attn_q_a_norm", "attn_q_a_layernorm"],
            Role::AttnKvAMqa => &["attn_kv_a_mqa", "attn_kv_a"],
            Role::AttnKvANorm => &["attn_kv_a_norm", "attn_kv_a_layernorm"],
            Role::AttnKvB => &["attn_kv_b", "attn_kv_b_proj"],
            Role::AttnKB => &["attn_k_b", "attn_k_b_trans"],
            Role::AttnVB => &["attn_v_b", "attn_v_b_proj"],
            Role::FfnNorm => &["ffn_norm"],
            Role::FfnGate => &["ffn_gate"],
            Role::FfnUp => &["ffn_up"],
            Role::FfnDown => &["ffn_down"],
            Role::FfnGateExps => &["ffn_gate_exps"],
            Role::FfnUpExps => &["ffn_up_exps"],
            Role::FfnDownExps => &["ffn_down_exps"],
            Role::FfnGateInp => &["ffn_gate_inp"],
        }
    }

    /// Find this role's tensor in block `i`, trying each alias.
    ///
    /// Returns the NAME rather than the tensor, so a caller can report which spelling it found — which
    /// matters when a load fails and the question is whether the tensor is absent or differently named.
    pub fn find(self, g: &crate::gguf::Gguf, i: usize) -> Option<String> {
        self.aliases().iter().map(|a| format!("blk.{i}.{a}.weight"))
            .find(|n| g.tensor(n).is_some())
    }
}

/// Which MLA kv_b layout a checkpoint uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KvBLayout {
    /// One fused `attn_kv_b`. Legacy, per llama.cpp PR #12801.
    Fused,
    /// Separate `attn_k_b` + `attn_v_b`, stored 3-D.
    Split,
    Absent,
}

/// Detect the layout once rather than at each use.
///
/// The split form went unread for four commits after MLA was wired, because the check was spelled
/// inline in one place and the loader looked elsewhere. Naming the question makes it answerable.
pub fn kv_b_layout(g: &crate::gguf::Gguf) -> KvBLayout {
    if Role::AttnKB.find(g, 0).is_some() && Role::AttnVB.find(g, 0).is_some() { return KvBLayout::Split; }
    if Role::AttnKvB.find(g, 0).is_some() { return KvBLayout::Fused; }
    KvBLayout::Absent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_two_roles_share_a_name() {
        // A shared alias would make find() return the same tensor for two roles — silently, and with
        // plausible output. This is the property the table exists to make checkable at all.
        let roles = [Role::AttnNorm, Role::AttnQ, Role::AttnK, Role::AttnV, Role::AttnOut,
                     Role::AttnQA, Role::AttnQB, Role::AttnQANorm, Role::AttnKvAMqa, Role::AttnKvANorm,
                     Role::AttnKvB, Role::AttnKB, Role::AttnVB, Role::FfnNorm, Role::FfnGate,
                     Role::FfnUp, Role::FfnDown, Role::FfnGateExps, Role::FfnUpExps,
                     Role::FfnDownExps, Role::FfnGateInp];
        let mut seen: Vec<(&str, Role)> = Vec::new();
        for r in roles {
            for a in r.aliases() {
                if let Some((_, other)) = seen.iter().find(|(n, _)| n == a) {
                    panic!("{a:?} is claimed by both {r:?} and {other:?}");
                }
                seen.push((a, r));
            }
        }
        assert!(seen.len() > roles.len(), "some roles should have aliases");
    }

    #[test]
    fn the_canonical_name_is_first() {
        // find() tries in order, so the first alias is what a well-formed modern file uses. Putting a
        // legacy spelling first would make every load try the rare name before the common one.
        assert_eq!(Role::AttnOut.aliases()[0], "attn_output");
        assert_eq!(Role::AttnKvAMqa.aliases()[0], "attn_kv_a_mqa");
    }

    #[test]
    fn the_split_and_fused_MLA_layouts_are_distinct_roles() {
        // They are not aliases of each other: a file has one or the other, and treating them as
        // interchangeable is what produced "ships MLA tensors but no kv_lora_rank" against a checkpoint
        // that stated it plainly.
        assert!(!Role::AttnKvB.aliases().iter().any(|a| Role::AttnKB.aliases().contains(a)));
        assert_ne!(Role::AttnKB, Role::AttnVB);
    }

    #[test]
    fn every_role_has_at_least_one_name() {
        for r in [Role::AttnQ, Role::FfnDown, Role::FfnGateInp, Role::AttnVB] {
            assert!(!r.aliases().is_empty(), "{r:?}");
        }
    }
}
