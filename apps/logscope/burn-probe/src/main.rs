//! Proves Burn's wgpu backend runs on Vulkan here: a real tensor matmul + sum,
//! cross-checked against the known CPU answer. This is the route the `Wgpu`
//! Backend seam in logscope-egui can take instead of hand-written kernels.
use burn::backend::Wgpu;
use burn::backend::wgpu::WgpuDevice;
use burn::tensor::Tensor;

type B = Wgpu; // f32 / i32

fn main() {
    let device = WgpuDevice::default();
    println!("burn wgpu device: {device:?}");

    // 2x3 · 3x2 = 2x2
    let a = Tensor::<B, 2>::from_floats([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]], &device);
    let b = Tensor::<B, 2>::from_floats([[1.0, 0.0], [0.0, 1.0], [1.0, 1.0]], &device);
    let c = a.matmul(b);
    let sum = c.clone().sum().into_scalar();

    println!("matmul =\n{}", c.into_data());
    println!("sum    = {sum}");
    println!("expect = [[4,5],[10,11]], sum=30  -> {}", if (sum - 30.0).abs() < 1e-4 { "MATCH (Burn→Vulkan OK)" } else { "MISMATCH" });
}
