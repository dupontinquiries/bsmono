//! PCG32 — the only randomness source inside the sim.

#[derive(Clone, Debug)]
pub struct Pcg {
    pub state: u64,
    pub inc: u64,
}

impl Pcg {
    pub fn new(seed: u64) -> Pcg {
        let mut r = Pcg {
            state: 0,
            inc: (seed << 1) | 1,
        };
        r.next();
        r.state = r.state.wrapping_add(seed);
        r.next();
        r
    }
    pub fn next(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }
}
