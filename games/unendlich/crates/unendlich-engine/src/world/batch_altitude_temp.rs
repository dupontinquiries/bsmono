//! Perf: batched altitude temperature, bit-identical to the scalar reference.
//! Mirrors altitude_temp: base − altitude · lapse_rate.

pub fn temp_one(base_temp: f32, altitude: f32, lapse_rate: f32) -> f32 {
    let alt = altitude.max(0.0);
    let lr = lapse_rate.max(0.0);
    base_temp - alt * lr
}

pub fn temp_batch(items: &[(f32, f32, f32)]) -> Vec<f32> {
    items.iter().map(|&(b, a, l)| temp_one(b, a, l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn parity() {
        let items = [(20.0f32, 0.0f32, 0.0065f32), (15.0, 2000.0, 0.0065), (25.0, 500.0, 0.01), (10.0, 3000.0, 0.005)];
        let b = temp_batch(&items);
        for (i, &it) in items.iter().enumerate() { assert_eq!(b[i].to_bits(), temp_one(it.0, it.1, it.2).to_bits()); }
    }
    #[test] fn empty_is_empty() { assert!(temp_batch(&[]).is_empty()); }
    #[test] fn single_eq_batch1() { assert_eq!(temp_batch(&[(20.0, 1000.0, 0.0065)])[0].to_bits(), temp_one(20.0, 1000.0, 0.0065).to_bits()); }
    #[test] fn sea_level_base() { assert_eq!(temp_one(20.0, 0.0, 0.0065), 20.0); }
    #[test] fn higher_colder() { assert!(temp_one(20.0, 2000.0, 0.0065) < temp_one(20.0, 0.0, 0.0065)); }
    #[test] fn deterministic() { assert_eq!(temp_batch(&[(20.0, 1000.0, 0.0065)]), temp_batch(&[(20.0, 1000.0, 0.0065)])); }
}
