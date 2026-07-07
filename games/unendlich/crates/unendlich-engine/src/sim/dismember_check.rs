//! Does a limb detach from this hit? True when the hit overkills the limb by at
//! least `dismember_thresh` past zero (handoff/BODY_PART_MESHING.md).

pub fn detaches(limb_hp: f32, incoming: f32, dismember_thresh: f32) -> bool {
    let thresh = dismember_thresh.max(0.0);
    (limb_hp - incoming) <= -thresh
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn overkill_detaches() { assert!(detaches(10.0, 50.0, 5.0)); }
    #[test] fn survivable_no_detach() { assert!(!detaches(100.0, 10.0, 5.0)); }
    #[test] fn exact_boundary() { assert!(detaches(0.0, 5.0, 5.0)); assert!(!detaches(0.0, 4.0, 5.0)); }
    #[test] fn zero_incoming_no_detach() { assert!(!detaches(50.0, 0.0, 5.0)); }
    #[test] fn monotone_in_incoming() {
        let mut seen_true = false;
        let mut inc = 0.0f32;
        while inc < 120.0 {
            let d = detaches(10.0, inc, 5.0);
            if seen_true { assert!(d); }
            if d { seen_true = true; }
            inc += 1.0;
        }
    }
    #[test] fn deterministic() { assert_eq!(detaches(10.0, 30.0, 5.0), detaches(10.0, 30.0, 5.0)); }
}
