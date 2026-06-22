# Testing Guide

## Running Tests

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_validate_temperature

# Run tests in specific module
cargo test helpers::tests

# Run tests with coverage (requires tarpaulin)
cargo tarpaulin --out Html
```

## Test Organization

### Unit Tests
Located in the same file as the code, in `#[cfg(test)] mod tests { }`

**What to test:**
- Individual functions in isolation
- Edge cases and boundary conditions
- Error handling paths
- Validation logic

**Example:**
```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_clamp() {
        assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp(-5.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(15.0, 0.0, 10.0), 10.0);
    }
}
```

### Integration Tests
Located in `tests/` directory

**What to test:**
- Multiple modules working together
- End-to-end workflows
- File I/O operations
- Database operations

**Example:**
```rust
// tests/model_test.rs
#[test]
fn test_model_save_and_load() {
    let model = Model::new(vec!["sensor1".into()]);
    let path = PathBuf::from("/tmp/test_model.json");
    
    model.save(&path).unwrap();
    let loaded = Model::load(&path).unwrap();
    
    assert_eq!(model.calibration_count, loaded.calibration_count);
}
```

## Test Coverage Goals

| Module | Current | Target |
|--------|---------|--------|
| helpers.rs | 100% ✅ | 100% |
| error.rs | 90% | 100% |
| constants.rs | 100% ✅ | 100% |
| model code | 20% | 80% |
| database code | 10% | 70% |
| UI code | 0% | 30% |

## Writing Good Tests

### 1. Arrange-Act-Assert Pattern

```rust
#[test]
fn test_temperature_validation() {
    // Arrange
    let temp = 25.0;
    let min = -40.0;
    let max = 40.0;
    
    // Act
    let result = validate_temperature(temp, min, max);
    
    // Assert
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 25.0);
}
```

### 2. Test Both Happy and Error Paths

```rust
#[test]
fn test_temperature_validation_valid() {
    assert!(validate_temperature(20.0, -40.0, 40.0).is_ok());
}

#[test]
fn test_temperature_validation_too_low() {
    assert!(validate_temperature(-50.0, -40.0, 40.0).is_err());
}

#[test]
fn test_temperature_validation_too_high() {
    assert!(validate_temperature(50.0, -40.0, 40.0).is_err());
}

#[test]
fn test_temperature_validation_nan() {
    assert!(validate_temperature(f32::NAN, -40.0, 40.0).is_err());
}
```

### 3. Use Descriptive Names

```rust
// ❌ Bad
#[test]
fn test1() { }

// ✅ Good
#[test]
fn test_model_predicts_zero_with_no_calibrations() { }
```

### 4. Test Edge Cases

```rust
#[test]
fn test_average_empty_slice() {
    assert_eq!(average(&[]), 0.0);
}

#[test]
fn test_average_single_value() {
    assert_eq!(average(&[5.0]), 5.0);
}

#[test]
fn test_average_negative_values() {
    assert_eq!(average(&[-1.0, -2.0, -3.0]), -2.0);
}
```

### 5. Use Test Fixtures for Complex Setup

```rust
fn create_test_model() -> Model {
    let sensors = vec!["sensor1".into(), "sensor2".into()];
    Model::new(sensors)
}

#[test]
fn test_with_fixture() {
    let model = create_test_model();
    // ... test code
}
```

## Property-Based Testing (Advanced)

For testing invariants that should always hold:

```rust
// Requires: quickcheck = "1.0"
use quickcheck::{QuickCheck, TestResult};

#[test]
fn test_clamp_always_in_range() {
    fn prop(value: f32, min: f32, max: f32) -> TestResult {
        if min >= max {
            return TestResult::discard();
        }
        
        let result = clamp(value, min, max);
        TestResult::from_bool(result >= min && result <= max)
    }
    
    QuickCheck::new().quickcheck(prop as fn(f32, f32, f32) -> TestResult);
}
```

## Mocking for Tests

### Database Mocking

```rust
trait Database {
    fn store_reading(&self, reading: &SensorReading) -> Result<()>;
}

struct MockDatabase {
    calls: RefCell<Vec<SensorReading>>,
}

impl Database for MockDatabase {
    fn store_reading(&self, reading: &SensorReading) -> Result<()> {
        self.calls.borrow_mut().push(reading.clone());
        Ok(())
    }
}

#[test]
fn test_with_mock_db() {
    let db = MockDatabase { calls: RefCell::new(vec![]) };
    // ... test code
    assert_eq!(db.calls.borrow().len(), 1);
}
```

## Benchmarking

```rust
// Requires: criterion = "0.5" in [dev-dependencies]
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_prediction(c: &mut Criterion) {
    let model = create_test_model();
    let readings = vec![30.0, 31.0, 29.0];
    
    c.bench_function("model_predict", |b| {
        b.iter(|| model.predict(black_box(&readings)))
    });
}

criterion_group!(benches, benchmark_prediction);
criterion_main!(benches);
```

## Continuous Integration

### GitHub Actions Example

```yaml
# .github/workflows/test.yml
name: Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - name: Run tests
        run: cargo test --all-features
      - name: Run clippy
        run: cargo clippy -- -D warnings
      - name: Check formatting
        run: cargo fmt -- --check
```

## Test Data

### Creating Test Files

```rust
use std::fs;
use tempfile::tempdir;

#[test]
fn test_model_load_from_file() {
    // Create temporary directory
    let dir = tempdir().unwrap();
    let file_path = dir.path().join("test_model.json");
    
    // Create test data
    let test_json = r#"{
        "stats": [],
        "bias": 0.0,
        "calibration_count": 5
    }"#;
    fs::write(&file_path, test_json).unwrap();
    
    // Test loading
    let model = Model::load(&file_path).unwrap();
    assert_eq!(model.calibration_count, 5);
    
    // Directory auto-cleaned up when `dir` is dropped
}
```

## Common Pitfalls

### ❌ Don't Use Random Numbers Without Seeding

```rust
// Bad - non-deterministic
#[test]
fn test_something() {
    let value = rand::random::<f32>();
    assert!(value > 0.5); // Flaky test!
}

// Good - deterministic
#[test]
fn test_something() {
    use rand::{SeedableRng, rngs::StdRng};
    let mut rng = StdRng::seed_from_u64(42);
    let value = rng.gen::<f32>();
    assert!(value > 0.5); // Always passes or always fails
}
```

### ❌ Don't Test Implementation Details

```rust
// Bad - testing internal structure
#[test]
fn test_model_internal_hash() {
    let model = Model::new(vec![]);
    assert_eq!(model.last_saved_hash, 0); // Fragile!
}

// Good - testing behavior
#[test]
fn test_model_detects_changes_after_calibration() {
    let mut model = Model::new(vec![]);
    assert!(!model.has_changed());
    model.calibrate(&[], 20.0);
    assert!(model.has_changed()); // Tests public API
}
```

### ❌ Don't Write Slow Tests

```rust
// Bad - takes 10 seconds
#[test]
fn test_calibration() {
    for _ in 0..100 {
        thread::sleep(Duration::from_millis(100));
        // ...
    }
}

// Good - finishes instantly
#[test]
fn test_calibration() {
    // Mock the slow parts or use smaller counts
}
```

## Test Checklist

Before committing code, verify:

- [ ] All tests pass: `cargo test`
- [ ] No compiler warnings: `cargo build --release`
- [ ] No clippy warnings: `cargo clippy -- -D warnings`
- [ ] Code is formatted: `cargo fmt --check`
- [ ] New public functions have tests
- [ ] Edge cases are covered
- [ ] Error paths are tested
- [ ] Tests are deterministic (no randomness)
- [ ] Tests run quickly (<1s each)
- [ ] Test names are descriptive

## Running Full Test Suite

```bash
#!/bin/bash
# run_all_tests.sh

set -e

echo "=== Running Tests ==="
cargo test

echo ""
echo "=== Checking Format ==="
cargo fmt -- --check

echo ""
echo "=== Running Clippy ==="
cargo clippy -- -D warnings

echo ""
echo "=== Building Release ==="
cargo build --release

echo ""
echo "✅ All checks passed!"
```

## Resources

- [Rust Testing Documentation](https://doc.rust-lang.org/book/ch11-00-testing.html)
- [Test Organization Best Practices](https://doc.rust-lang.org/book/ch11-03-test-organization.html)
- [Property Testing with QuickCheck](https://docs.rs/quickcheck/)
- [Benchmarking with Criterion](https://docs.rs/criterion/)
- [Mocking with Mockall](https://docs.rs/mockall/)
