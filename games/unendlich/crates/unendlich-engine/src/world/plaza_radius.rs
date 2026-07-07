//! Central-plaza radius from settlement population: area ∝ population so radius ∝
//! √population (area = pop·per_capita_area, circle), capped. Pure, deterministic.

pub fn radius(population: f32, per_capita_area: f32, max_radius: f32) -> f32 {
    let pop = population.max(0.0);
    let pca = per_capita_area.max(0.0);
    let mr = max_radius.max(0.0);
    (pop * pca / std::f32::consts::PI).sqrt().min(mr)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn empty_zero() { assert_eq!(radius(0.0, 2.0, 100.0), 0.0); }
    #[test] fn more_pop_bigger() { assert!(radius(1000.0, 2.0, 1.0e6) > radius(100.0, 2.0, 1.0e6)); }
    #[test] fn diminishing() {
        let early = radius(400.0, 2.0, 1.0e6) - radius(0.0, 2.0, 1.0e6);
        let late = radius(900.0, 2.0, 1.0e6) - radius(500.0, 2.0, 1.0e6);
        assert!(early > late);
    }
    #[test] fn capped() { assert_eq!(radius(1.0e9, 2.0, 50.0), 50.0); }
    #[test] fn nonneg() { assert!(radius(500.0, 2.0, 1.0e6) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(radius(500.0, 2.0, 1.0e6).to_bits(), radius(500.0, 2.0, 1.0e6).to_bits()); }
}
