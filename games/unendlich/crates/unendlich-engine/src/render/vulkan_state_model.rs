//! CPU model of Vulkan per-subresource layout state (v0.24.2c.2).
//!
//! This is a partial simulator of Vulkan's image-subresource layout
//! tracking. It does NOT model:
//! - Pipeline stages or access masks (so it can't catch wrong-stage
//!   barrier bugs)
//! - Cross-queue ownership transfers
//! - Driver-specific behavior
//!
//! It DOES model:
//! - Per-array-layer, per-mip-level layout state for a single image
//! - Layout transitions caused by:
//!   - Explicit image memory barriers
//!   - Render pass `initial_layout` / `final_layout` transitions on
//!     attached subresources
//! - Read-vs-write expectation at sample time
//!
//! The intended use is verifying that a frame's worth of operations
//! leaves the shadow image's layers in the layouts the main fragment
//! shader expects to sample. Specifically: after all cascade render
//! passes complete, each layer 0..NUM_CASCADES must be in
//! `DEPTH_STENCIL_READ_ONLY_OPTIMAL`, the layout the descriptor was
//! created with.
//!
//! This catches a class of bug where I render to a layer and forget
//! to transition it to read-only, or where I accidentally transition
//! a layer back to attachment-optimal between rendering and sampling.
//! It does NOT catch bugs where the actual Vulkan implementation does
//! something different from the spec (i.e. driver issues on
//! Honeykrisp).

#![allow(dead_code)] // model used only in tests; production code uses real Vulkan

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Layout {
    Undefined,
    DepthStencilAttachmentOptimal,
    DepthStencilReadOnlyOptimal,
    General,
    ShaderReadOnlyOptimal,
}

/// Per-subresource layout state for one image. Keyed by
/// `(mip_level, array_layer)`. We only model `level_count` for the
/// max mip we'll see; the shadow image has 1 mip.
#[derive(Debug)]
pub struct ImageLayoutState {
    /// One Layout entry per (mip_level, array_layer). Indexed as
    /// `mip * array_layers + layer`.
    layouts: Vec<Layout>,
    pub mip_levels: u32,
    pub array_layers: u32,
}

impl ImageLayoutState {
    pub fn new(mip_levels: u32, array_layers: u32) -> Self {
        let n = (mip_levels * array_layers) as usize;
        Self {
            layouts: vec![Layout::Undefined; n],
            mip_levels,
            array_layers,
        }
    }

    fn idx(&self, mip: u32, layer: u32) -> usize {
        assert!(mip < self.mip_levels);
        assert!(layer < self.array_layers);
        (mip * self.array_layers + layer) as usize
    }

    pub fn get(&self, mip: u32, layer: u32) -> Layout {
        self.layouts[self.idx(mip, layer)]
    }

    /// Apply an image memory barrier or render-pass-implied transition
    /// over a subresource range. The `from` layout is accepted only if
    /// it matches the current state OR if it's `Undefined` (Vulkan
    /// allows transitioning FROM Undefined regardless of the actual
    /// current layout — it means "discard the contents").
    pub fn transition(
        &mut self,
        base_mip: u32, mip_count: u32,
        base_layer: u32, layer_count: u32,
        from: Layout, to: Layout,
    ) {
        for mip in base_mip..base_mip + mip_count {
            for layer in base_layer..base_layer + layer_count {
                let idx = self.idx(mip, layer);
                let cur = self.layouts[idx];
                if from == Layout::Undefined {
                    // Discard transition; accept any current layout.
                    self.layouts[idx] = to;
                } else if cur == from {
                    self.layouts[idx] = to;
                } else {
                    panic!(
                        "layout transition source mismatch at (mip {}, layer {}): \
                         tracker says {:?}, transition expected {:?}",
                        mip, layer, cur, from,
                    );
                }
            }
        }
    }

    /// Simulate a Vulkan render pass invocation:
    /// - Subpass attachment transitions FROM `initial_layout` TO the
    ///   `subpass_layout` at begin (this is what
    ///   `vkCmdBeginRenderPass` does to attachments).
    /// - At end, transitions TO `final_layout` (what
    ///   `vkCmdEndRenderPass` does).
    ///
    /// The `attachment_subresource` is the subresource range the
    /// framebuffer's attachment view references. Render-pass-implied
    /// transitions apply only to that subresource.
    pub fn run_render_pass(
        &mut self,
        att_base_mip: u32, att_mip_count: u32,
        att_base_layer: u32, att_layer_count: u32,
        initial_layout: Layout, subpass_layout: Layout, final_layout: Layout,
    ) {
        // Begin: initial → subpass.
        self.transition(
            att_base_mip, att_mip_count,
            att_base_layer, att_layer_count,
            initial_layout, subpass_layout,
        );
        // End: subpass → final.
        self.transition(
            att_base_mip, att_mip_count,
            att_base_layer, att_layer_count,
            subpass_layout, final_layout,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: a freshly-created image is all-Undefined.
    #[test]
    fn new_image_is_undefined() {
        let s = ImageLayoutState::new(1, 4);
        for layer in 0..4 {
            assert_eq!(s.get(0, layer), Layout::Undefined);
        }
    }

    /// A render pass on one attachment subresource transitions only
    /// that subresource. Other layers stay Undefined.
    #[test]
    fn render_pass_only_affects_attachment_subresource() {
        let mut s = ImageLayoutState::new(1, 4);
        s.run_render_pass(
            0, 1, /* attachment subresource: */ 0, 1,
            Layout::Undefined,
            Layout::DepthStencilAttachmentOptimal,
            Layout::DepthStencilReadOnlyOptimal,
        );
        assert_eq!(s.get(0, 0), Layout::DepthStencilReadOnlyOptimal);
        // Layers 1, 2, 3 untouched.
        for layer in 1..4 {
            assert_eq!(s.get(0, layer), Layout::Undefined);
        }
    }

    /// Model v0.24.2c's per-frame shadow pre-pass: 4 render passes,
    /// one per cascade, each targeting a different layer. At end of
    /// frame, every layer should be in DEPTH_STENCIL_READ_ONLY_OPTIMAL,
    /// matching the descriptor layout the main pass samples.
    ///
    /// THIS IS THE KEY TEST: it verifies that my mental model of the
    /// v0.24.2c command sequence leaves layer 0 (and 1, 2, 3) in
    /// READ_ONLY at sample time. If the test passes but shadows are
    /// missing in playtest anyway, the bug is in the driver or in a
    /// part of the system I'm not modeling (barriers between non-
    /// render-pass writes, descriptor layout mismatch, push-constant
    /// data, etc.).
    #[test]
    fn shadow_pre_pass_leaves_all_cascade_layers_read_only() {
        const NUM_CASCADES: u32 = 4;
        let mut shadow_image = ImageLayoutState::new(1, NUM_CASCADES);

        for cascade in 0..NUM_CASCADES {
            shadow_image.run_render_pass(
                0, 1, /* attachment subresource: */ cascade, 1,
                // initial_layout = UNDEFINED per the render pass def
                // (so the previous frame's contents are discarded).
                Layout::Undefined,
                Layout::DepthStencilAttachmentOptimal,
                Layout::DepthStencilReadOnlyOptimal,
            );
        }

        for layer in 0..NUM_CASCADES {
            assert_eq!(
                shadow_image.get(0, layer),
                Layout::DepthStencilReadOnlyOptimal,
                "after shadow pre-pass, layer {} should be READ_ONLY",
                layer,
            );
        }
    }

    /// Model the failure scenario "I render to layer 0, then to layer
    /// 1, but accidentally use layer 0's framebuffer for cascade 1's
    /// render pass." Layer 0 gets transitioned back to ATTACHMENT and
    /// then to READ_ONLY — twice, with discarded contents in between.
    ///
    /// This test verifies the LAYOUT ends correctly even in the bad
    /// case, which means a layout-only check CANNOT catch this kind
    /// of bug. Documents the limit of this CPU model.
    #[test]
    fn layout_tracker_does_not_catch_wrong_framebuffer_attachment() {
        const NUM_CASCADES: u32 = 4;
        let mut shadow_image = ImageLayoutState::new(1, NUM_CASCADES);

        // Bug scenario: both cascades use layer 0.
        for _ in 0..2 {
            shadow_image.run_render_pass(
                0, 1, /* attachment: */ 0, 1,
                Layout::Undefined,
                Layout::DepthStencilAttachmentOptimal,
                Layout::DepthStencilReadOnlyOptimal,
            );
        }

        // Layer 0 ends in READ_ONLY (correct layout!), but its
        // contents are wrong because cascade 1 overwrote cascade 0.
        // The layout tracker accepts this. Layouts 1, 2, 3 are still
        // Undefined — that would be a DIFFERENT signal a more
        // thorough model could catch (contents-tracker).
        assert_eq!(shadow_image.get(0, 0), Layout::DepthStencilReadOnlyOptimal);
        for layer in 1..NUM_CASCADES {
            assert_eq!(shadow_image.get(0, layer), Layout::Undefined);
        }
    }

    /// Model the failure "I forget to add `final_layout: READ_ONLY`
    /// to the render pass, so layers end in ATTACHMENT_OPTIMAL".
    /// The main pass's descriptor binding expects READ_ONLY → mismatch.
    #[test]
    fn forgotten_final_layout_transition_is_caught() {
        let mut shadow_image = ImageLayoutState::new(1, 4);
        shadow_image.run_render_pass(
            0, 1, /* attachment: */ 0, 1,
            Layout::Undefined,
            Layout::DepthStencilAttachmentOptimal,
            // BUG: forgot to specify READ_ONLY as final_layout
            Layout::DepthStencilAttachmentOptimal,
        );
        // The descriptor binding expects READ_ONLY at sample time.
        assert_ne!(
            shadow_image.get(0, 0),
            Layout::DepthStencilReadOnlyOptimal,
            "this bug pattern leaves the layer in ATTACHMENT, not READ_ONLY",
        );
    }
}
