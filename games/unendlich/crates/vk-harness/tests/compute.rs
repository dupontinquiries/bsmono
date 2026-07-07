//! Headless compute test. Runs on lavapipe (or any Vulkan device). When no
//! device is reachable the test logs and returns rather than failing, so it's
//! safe in environments without Vulkan.

use vk_harness::{ComputeHarness, DOUBLE_SPV};

#[test]
fn doubles_a_buffer_on_a_compute_device() {
    let harness = match ComputeHarness::new() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipping: no Vulkan compute device ({e})");
            return;
        }
    };
    eprintln!("compute device: {} (cpu={})", harness.device_name(), harness.is_cpu_device());

    let input: Vec<u32> = (1..=200u32).collect();
    let out = harness.run_u32_kernel(DOUBLE_SPV, &input).expect("kernel run");

    assert_eq!(out.len(), input.len());
    for (i, (&a, &b)) in input.iter().zip(out.iter()).enumerate() {
        assert_eq!(b, a * 2, "element {i}: expected {} got {}", a * 2, b);
    }
}

#[test]
fn empty_input_is_handled() {
    let harness = match ComputeHarness::new() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipping: no Vulkan compute device ({e})");
            return;
        }
    };
    let out = harness.run_u32_kernel(DOUBLE_SPV, &[]).expect("empty run");
    assert!(out.is_empty());
}
