## You
Review this Rust module — I'll ask several follow-up questions about it.

## Assistant
Here's the module under review:

```rust
//! A fixed-size 2D grid with a Conway's-Game-of-Life step, plus a few query helpers.
pub struct Grid {
    w: usize,
    h: usize,
    cells: Vec<u8>, // 0 = dead, 1 = live, row-major
}

impl Grid {
    pub fn new(w: usize, h: usize) -> Self {
        Self { w, h, cells: vec![0; w * h] }
    }

    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.cells[y * self.w + x]
    }

    pub fn set(&mut self, x: usize, y: usize, v: u8) {
        self.cells[y * self.w + x] = v;
    }

    fn live_neighbors(&self, x: usize, y: usize) -> u8 {
        let mut n = 0u8;
        for dy in [-1i32, 0, 1] {
            for dx in [-1i32, 0, 1] {
                if dx == 0 && dy == 0 { continue; }
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx >= 0 && nx < self.w as i32 && ny >= 0 && ny < self.h as i32 {
                    n += self.get(nx as usize, ny as usize);
                }
            }
        }
        n
    }

    pub fn step(&mut self) {
        let mut next = self.cells.clone();
        for y in 0..self.h {
            for x in 0..self.w {
                let live = self.get(x, y) == 1;
                let n = self.live_neighbors(x, y);
                next[y * self.w + x] = match (live, n) {
                    (true, 2) | (true, 3) => 1,
                    (false, 3) => 1,
                    _ => 0,
                };
            }
        }
        self.cells = next;
    }

    pub fn population(&self) -> usize {
        self.cells.iter().filter(|&&c| c == 1).count()
    }
}
```

This is a straightforward row-major grid. The `step` allocates a fresh buffer each tick, bounds-checks
every neighbor lookup, and recomputes the whole board. Ask away.
