## You
implement a small fixed-capacity LRU cache in Rust

## Assistant
Here's a skeleton to fill in:
```rust
use std::collections::HashMap;

struct Lru {
    cap: usize,
    map: HashMap<u64, u64>,
    order: Vec<u64>,
}
impl Lru {
    fn new(cap: usize) -> Self { todo!() }
    fn get(&mut self, k: u64) -> Option<u64> { todo!() }
    fn put(&mut self, k: u64, v: u64) { unimplemented!() }
}
```
