//! Lossless greedy merge of a row of voxel cells into runs — the compute-saving
//! mesh op scoped to an entity part (handoff/BODY_PART_MESHING.md). 0 = empty
//! (breaks runs); nonzero = material id. Lossless: Σ run lengths == nonzero count.

pub fn merge_runs(cells: &[u16]) -> Vec<(usize, usize, u16)> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < cells.len() {
        let m = cells[i];
        if m == 0 {
            i += 1;
            continue;
        }
        let start = i;
        while i < cells.len() && cells[i] == m {
            i += 1;
        }
        runs.push((start, i - start, m));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn uniform_one_run() { assert_eq!(merge_runs(&[7, 7, 7, 7]), vec![(0, 4, 7)]); }
    #[test] fn alternating_many_runs() { assert_eq!(merge_runs(&[1, 2, 1, 2]).len(), 4); }
    #[test] fn zeros_break_runs() { assert_eq!(merge_runs(&[1, 1, 0, 1]), vec![(0, 2, 1), (3, 1, 1)]); }
    #[test] fn empty_is_empty() { assert!(merge_runs(&[]).is_empty()); }
    #[test] fn lossless() {
        let cells = [0u16, 3, 3, 0, 5, 5, 5, 0, 9];
        let nonzero = cells.iter().filter(|&&c| c != 0).count();
        let sum: usize = merge_runs(&cells).iter().map(|r| r.1).sum();
        assert_eq!(sum, nonzero);
    }
    #[test] fn deterministic() { assert_eq!(merge_runs(&[1, 1, 2]), merge_runs(&[1, 1, 2])); }
}
