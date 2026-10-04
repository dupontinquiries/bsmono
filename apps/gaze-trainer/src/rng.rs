//! A small deterministic random source (SplitMix64): training must be repeatable, and a test must be able to say why a
//! number came out as it did.

#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng { Rng(seed) }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn unit(&mut self) -> f64 { (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64 }

    /// Standard normal (Box-Muller).
    pub fn normal(&mut self) -> f64 {
        let (u1, u2) = (self.unit().max(1e-300), self.unit());
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }

    /// Uniform in 0..n.
    pub fn below(&mut self, n: usize) -> usize { (self.next_u64() % n.max(1) as u64) as usize }

    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() { v.swap(i, self.below(i + 1)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_stream_and_the_numbers_are_spread() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        for _ in 0..50 { assert_eq!(a.next_u64(), b.next_u64()); }
        assert_ne!(Rng::new(7).next_u64(), Rng::new(8).next_u64());
        let mut r = Rng::new(1);
        let u: Vec<f64> = (0..20000).map(|_| r.unit()).collect();
        assert!(u.iter().all(|&x| (0.0..1.0).contains(&x)));
        let mean = u.iter().sum::<f64>() / u.len() as f64;
        assert!((mean - 0.5).abs() < 0.01, "{mean}");
        let n: Vec<f64> = (0..20000).map(|_| r.normal()).collect();
        let (m, v) = (n.iter().sum::<f64>() / n.len() as f64, n.iter().map(|x| x * x).sum::<f64>() / n.len() as f64);
        assert!(m.abs() < 0.03 && (v - 1.0).abs() < 0.05, "mean {m} var {v}");
    }

    #[test]
    fn shuffle_keeps_every_element_and_changes_the_order() {
        let mut v: Vec<usize> = (0..30).collect();
        Rng::new(3).shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..30).collect::<Vec<_>>());
        assert_ne!(v, (0..30).collect::<Vec<_>>());
    }
}
