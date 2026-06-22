//! Draw-call coalescing — the merge-render partner to the per-brick mesher. Fine
//! brick granularity means lots of small meshes; issued naively that's one draw
//! call each, which inflates the `main` pass and command recording. But brick
//! meshes living in a shared index buffer can be drawn together: any run of
//! meshes that share render state and sit contiguously in the buffer collapses
//! into a single indexed draw. So you mesh at brick granularity but draw at
//! batch granularity, and the draw-call count stays flat as bricks multiply.
//!
//! This is the CPU side of the engine's existing indirect-draw path
//! (`UNENDLICH_INDIRECT`). Pure, deterministic.

/// One brick mesh's slice of the shared index buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshSpan {
    /// Render-state key (pipeline/material/LOD variant). Only same-state spans merge.
    pub state: u32,
    pub first_index: u32,
    pub index_count: u32,
    /// Base vertex added to every index in this span. Spans can only merge into
    /// one draw if they share it (a single indexed draw applies one base vertex).
    pub vertex_offset: i32,
}

/// A coalesced indexed indirect draw (maps to VkDrawIndexedIndirectCommand with
/// instance_count = 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawCommand {
    pub state: u32,
    pub first_index: u32,
    pub index_count: u32,
    pub vertex_offset: i32,
}

/// Merge spans into the minimal set of draw commands: a run of spans with the
/// same `state` and `vertex_offset` whose index ranges are contiguous becomes one
/// draw. Zero-index spans are dropped. Deterministic (sorted by state, then
/// vertex_offset, then first_index).
pub fn coalesce(spans: &[MeshSpan]) -> Vec<DrawCommand> {
    let mut s: Vec<MeshSpan> = spans.iter().copied().filter(|x| x.index_count > 0).collect();
    s.sort_by(|a, b| {
        a.state
            .cmp(&b.state)
            .then(a.vertex_offset.cmp(&b.vertex_offset))
            .then(a.first_index.cmp(&b.first_index))
    });

    let mut out: Vec<DrawCommand> = Vec::new();
    for span in s {
        if let Some(last) = out.last_mut() {
            let contiguous = span.first_index == last.first_index + last.index_count;
            if last.state == span.state && last.vertex_offset == span.vertex_offset && contiguous {
                last.index_count += span.index_count; // extend the run
                continue;
            }
        }
        out.push(DrawCommand {
            state: span.state,
            first_index: span.first_index,
            index_count: span.index_count,
            vertex_offset: span.vertex_offset,
        });
    }
    out
}

/// Total indices across draw commands — must equal the sum of input span indices
/// (coalescing never drops geometry).
pub fn total_indices(cmds: &[DrawCommand]) -> u64 {
    cmds.iter().map(|c| c.index_count as u64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(state: u32, first: u32, count: u32, voff: i32) -> MeshSpan {
        MeshSpan { state, first_index: first, index_count: count, vertex_offset: voff }
    }

    #[test]
    fn contiguous_same_state_spans_become_one_draw() {
        let spans = [span(0, 0, 6, 0), span(0, 6, 12, 0), span(0, 18, 6, 0)];
        let cmds = coalesce(&spans);
        assert_eq!(cmds.len(), 1, "three contiguous bricks → one draw");
        assert_eq!(cmds[0].first_index, 0);
        assert_eq!(cmds[0].index_count, 24, "indices summed");
    }

    #[test]
    fn a_state_change_breaks_the_run() {
        let spans = [span(0, 0, 6, 0), span(1, 6, 6, 0)];
        let cmds = coalesce(&spans);
        assert_eq!(cmds.len(), 2, "different pipeline/material → separate draws");
    }

    #[test]
    fn a_gap_breaks_the_run() {
        // 0..6 then 12..18 — a hole at 6..12 means they can't be one draw.
        let spans = [span(0, 0, 6, 0), span(0, 12, 6, 0)];
        let cmds = coalesce(&spans);
        assert_eq!(cmds.len(), 2, "non-contiguous index ranges stay separate");
    }

    #[test]
    fn different_vertex_offsets_do_not_merge() {
        let spans = [span(0, 0, 6, 0), span(0, 6, 6, 100)];
        let cmds = coalesce(&spans);
        assert_eq!(cmds.len(), 2, "a single draw applies one base vertex");
    }

    #[test]
    fn geometry_is_never_lost() {
        let spans = [span(2, 30, 9, 0), span(0, 0, 6, 0), span(0, 6, 6, 0), span(1, 99, 3, 5)];
        let total_in: u64 = spans.iter().map(|s| s.index_count as u64).sum();
        let cmds = coalesce(&spans);
        assert_eq!(total_indices(&cmds), total_in, "coalescing preserves all indices");
    }

    #[test]
    fn zero_index_spans_are_dropped_and_empty_input_yields_nothing() {
        assert!(coalesce(&[]).is_empty());
        assert!(coalesce(&[span(0, 0, 0, 0)]).is_empty(), "empty bricks issue no draw");
    }

    #[test]
    fn many_contiguous_bricks_collapse_to_far_fewer_draws() {
        // 64 bricks, each 6 indices, laid out contiguously → 1 draw, not 64.
        let spans: Vec<MeshSpan> = (0..64).map(|i| span(0, i * 6, 6, 0)).collect();
        let cmds = coalesce(&spans);
        assert_eq!(cmds.len(), 1, "64 contiguous bricks → a single draw call");
        assert_eq!(cmds[0].index_count, 64 * 6);
    }
}
