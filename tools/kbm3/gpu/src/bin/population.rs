// beatspace-population -- coevolutionary population training on the GPU (Vulkan).
//
// Rust/Burn port of pipeline/population_trainer.py: a population of generators
// and discriminators, cross-played against each other every round (a full
// score matrix, not one pairing), with weak members replaced by a mutated
// (Gaussian-perturbed) copy of a random survivor. Same anti-stagnation
// motivation as the Python version -- see that file's docstring -- but now
// every pairing's forward/backward runs on the Apple GPU via Vulkan instead
// of the CPU, which is where this workload (pop_size^2 pairings x inner_iters
// steps per round) actually needs the throughput.
//
//   WGPU_BACKEND=vulkan cargo run --release --bin population -- smoke
//   WGPU_BACKEND=vulkan cargo run --release --bin population -- train --pop 4 --rounds 200
//
#![recursion_limit = "256"]

#[path = "../model.rs"]
mod model;
#[path = "../data.rs"]
mod data;

use std::time::Instant;

use burn::backend::wgpu::WgpuDevice;
use burn::backend::{Autodiff, Wgpu};
use burn::module::{Module, ModuleMapper, Param};
use burn::optim::{AdamConfig, GradientsParams, Optimizer};
use burn::tensor::activation::sigmoid;
use burn::tensor::{Distribution, Tensor};

use model::{bce_with_logits, Discriminator, GanConfig, Generator};
use data::MelCache;

type BE = Wgpu<f32, i32>;
type AB = Autodiff<BE>;

fn envu(k: &str, d: usize) -> usize {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn envf(k: &str, d: f32) -> f32 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

// --------------------------------------------------------------------------- //
// Mutation: a ModuleMapper that adds Gaussian noise to every float parameter.
// This is the Rust equivalent of the Python side's
// `p.add_(torch.randn_like(p) * std)` applied to every tensor in the model.
// --------------------------------------------------------------------------- //
use burn::tensor::backend::Backend;

struct AddNoise<B: Backend> {
    std: f32,
    device: B::Device,
}
impl<B: burn::tensor::backend::Backend> ModuleMapper<B> for AddNoise<B> {
    fn map_float<const D: usize>(&mut self, param: Param<Tensor<B, D>>) -> Param<Tensor<B, D>> {
        let std = self.std;
        let device = self.device.clone();
        param.map(|t| {
            let noise = Tensor::<B, D>::random(t.shape(), Distribution::Normal(0.0, std as f64), &device);
            t + noise
        })
    }
}

// --------------------------------------------------------------------------- //
// Population member: a model + its optimizer + a running fitness score.
// --------------------------------------------------------------------------- //
struct GMember {
    model: Generator<AB>,
    opt: burn::optim::adaptor::OptimizerAdaptor<burn::optim::Adam, Generator<AB>, AB>,
    fitness: f32,
}
struct DMember {
    model: Discriminator<AB>,
    opt: burn::optim::adaptor::OptimizerAdaptor<burn::optim::Adam, Discriminator<AB>, AB>,
    fitness: f32,
}

fn new_adam() -> AdamConfig {
    AdamConfig::new().with_beta_1(0.5)
}

fn mutate_generator(m: &Generator<AB>, std: f32, device: &WgpuDevice) -> Generator<AB> {
    let mut mapper = AddNoise::<AB> { std, device: device.clone() };
    m.clone().map(&mut mapper)
}
fn mutate_discriminator(m: &Discriminator<AB>, std: f32, device: &WgpuDevice) -> Discriminator<AB> {
    let mut mapper = AddNoise::<AB> { std, device: device.clone() };
    m.clone().map(&mut mapper)
}

// --------------------------------------------------------------------------- //
// One (G_i, D_j) pairing: inner_iters of ordinary adversarial steps on
// synthetic data (swap for the real SQLite mel loader -- see gpu/src/data.rs
// -- exactly as beatspace-gpu's train() does).
// --------------------------------------------------------------------------- //
fn play_pair(
    cfg: &GanConfig,
    g: &mut GMember,
    d: &mut DMember,
    inner_iters: usize,
    batch: usize,
    device: &WgpuDevice,
    cache: Option<&mut MelCache>,
) -> (f32, f32) {
    let lr = 2e-4;
    let (mut d_real, mut d_fake) = (0.5f32, 0.5f32);
    let mut cache = cache;

    for _ in 0..inner_iters {
        let real = match cache.as_deref_mut() {
            Some(c) => c.next_batch::<AB>(batch, device),
            None => Tensor::<AB, 4>::random(
                [batch, 1, cfg.n_mels, cfg.time_frames],
                Distribution::Uniform(-1.0, 1.0),
                device,
            ),
        };
        let z = Tensor::<AB, 2>::random([batch, cfg.latent], Distribution::Normal(0.0, 1.0), device);

        let fake = g.model.forward(z);
        let d_real_logits = d.model.forward(real);
        let d_fake_logits = d.model.forward(fake.clone().detach());
        let loss_d = bce_with_logits(d_real_logits.clone(), 0.9)
            + bce_with_logits(d_fake_logits.clone(), 0.0);
        let grads = loss_d.backward();
        let gp = GradientsParams::from_grads(grads, &d.model);
        d.model = d.opt.step(lr, d.model.clone(), gp);

        let g_logits = d.model.forward(fake);
        let loss_g = bce_with_logits(g_logits, 0.9);
        let grads = loss_g.backward();
        let gp = GradientsParams::from_grads(grads, &g.model);
        g.model = g.opt.step(lr, g.model.clone(), gp);

        d_real = sigmoid(d_real_logits).mean().into_scalar();
        d_fake = sigmoid(d_fake_logits).mean().into_scalar();
    }
    (d_real, d_fake)
}

// --------------------------------------------------------------------------- //
fn run_round(
    cfg: &GanConfig,
    gs: &mut [GMember],
    ds: &mut [DMember],
    inner_iters: usize,
    batch: usize,
    device: &WgpuDevice,
    mut cache: Option<&mut MelCache>,
) -> (Vec<f32>, Vec<f32>) {
    let (ng, nd) = (gs.len(), ds.len());
    let mut s_fool = vec![vec![0f32; nd]; ng];   // how much G_i fooled D_j
    let mut s_sep = vec![vec![0f32; ng]; nd];    // how well D_j separated real/fake vs G_i

    for i in 0..ng {
        for j in 0..nd {
            let (dr, df) = play_pair(cfg, &mut gs[i], &mut ds[j], inner_iters, batch, device, cache.as_deref_mut());
            s_fool[i][j] = df;
            s_sep[j][i] = dr - df;
        }
    }

    let fit_g: Vec<f32> = s_fool.iter().map(|row| row.iter().sum::<f32>() / nd as f32).collect();
    let fit_d: Vec<f32> = s_sep.iter().map(|row| row.iter().sum::<f32>() / ng as f32).collect();
    (fit_g, fit_d)
}

/// Keep the top (1-mutate_frac) by fitness; replace the rest with a mutated
/// clone of a randomly-chosen SURVIVOR (not always the single best).
fn select_and_replace_g(gs: &mut Vec<GMember>, fitness: &[f32], mutate_frac: f32, std: f32, device: &WgpuDevice) -> (Vec<usize>, Vec<usize>) {
    let n = gs.len();
    let n_replace = ((n as f32 * mutate_frac).round() as usize).max(1);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| fitness[b].partial_cmp(&fitness[a]).unwrap());
    let survivors: Vec<usize> = order[..n - n_replace].to_vec();
    let replaced: Vec<usize> = order[n - n_replace..].to_vec();

    for &ri in &replaced {
        let parent = survivors[fastrand::usize(0..survivors.len())];
        let new_model = mutate_generator(&gs[parent].model, std, device);
        gs[ri] = GMember { model: new_model, opt: new_adam().init(), fitness: 0.0 };
    }
    (survivors, replaced)
}
fn select_and_replace_d(ds: &mut Vec<DMember>, fitness: &[f32], mutate_frac: f32, std: f32, device: &WgpuDevice) -> (Vec<usize>, Vec<usize>) {
    let n = ds.len();
    let n_replace = ((n as f32 * mutate_frac).round() as usize).max(1);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| fitness[b].partial_cmp(&fitness[a]).unwrap());
    let survivors: Vec<usize> = order[..n - n_replace].to_vec();
    let replaced: Vec<usize> = order[n - n_replace..].to_vec();

    for &ri in &replaced {
        let parent = survivors[fastrand::usize(0..survivors.len())];
        let new_model = mutate_discriminator(&ds[parent].model, std, device);
        ds[ri] = DMember { model: new_model, opt: new_adam().init(), fitness: 0.0 };
    }
    (survivors, replaced)
}

// --------------------------------------------------------------------------- //
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("smoke");
    let device = WgpuDevice::default();
    println!("Backend: Wgpu (Vulkan on Asahi). Device: {device:?}");

    let cfg = GanConfig::new()
        .with_n_mels(envu("BS_NMELS", 256))
        .with_time_frames(envu("BS_TF", 560))
        .with_g_filters(envu("BS_GF", 256))
        .with_d_filters(envu("BS_DF", 64));

    let pop = arg(&args, "--pop").unwrap_or(4usize);
    let rounds = arg(&args, "--rounds").unwrap_or(200usize);
    let inner_iters = arg(&args, "--inner-iters").unwrap_or(20usize);
    let batch = arg(&args, "--batch").unwrap_or(8usize);
    let mutate_frac = envf("BS_MUTATE_FRAC", 0.5);
    let mutation_std = envf("BS_MUTATION_STD", 0.02);
    let cache_dir = arg::<String>(&args, "--cache");

    match cmd {
        "smoke" => smoke(&cfg, &device),
        "train" => train(&cfg, &device, pop, rounds, inner_iters, batch, mutate_frac, mutation_std, cache_dir.as_deref()),
        other => {
            eprintln!("unknown '{other}'. use: smoke | train [--pop N --rounds N --inner-iters N --batch N --cache DIR]");
            std::process::exit(2);
        }
    }
}

fn smoke(cfg: &GanConfig, device: &WgpuDevice) {
    let mut g = GMember { model: cfg.init_generator::<AB>(device), opt: new_adam().init(), fitness: 0.0 };
    let mut d = DMember { model: cfg.init_discriminator::<AB>(device), opt: new_adam().init(), fitness: 0.0 };
    let t0 = Instant::now();
    let (dr, df) = play_pair(cfg, &mut g, &mut d, 2, 4, device, None);
    println!("one (G,D) pair, 2 inner iters: D(real)={dr:.3} D(fake)={df:.3} ({} ms)", t0.elapsed().as_millis());

    let mutant = mutate_generator(&g.model, 0.02, device);
    let z = Tensor::<AB, 2>::random([2, cfg.latent], Distribution::Normal(0.0, 1.0), device);
    let out = mutant.forward(z);
    println!("mutated generator forward -> {:?} (mutation + forward both OK)", out.dims());
}

fn train(cfg: &GanConfig, device: &WgpuDevice, pop: usize, rounds: usize, inner_iters: usize,
        batch: usize, mutate_frac: f32, mutation_std: f32, cache_dir: Option<&str>) {
    println!("Population training on GPU (Vulkan): {pop}x{pop}, {rounds} rounds x {inner_iters} inner iters.");
    let mut cache = cache_dir.map(|dir| {
        let c = MelCache::open(dir, 32, cfg.n_mels, cfg.time_frames);
        println!("Loaded cache '{dir}': {} valid tracks.", c.len());
        c
    });
    let mut gs: Vec<GMember> = (0..pop)
        .map(|_| GMember { model: cfg.init_generator::<AB>(device), opt: new_adam().init(), fitness: 0.0 })
        .collect();
    let mut ds: Vec<DMember> = (0..pop)
        .map(|_| DMember { model: cfg.init_discriminator::<AB>(device), opt: new_adam().init(), fitness: 0.0 })
        .collect();

    for r in 0..rounds {
        let t0 = Instant::now();
        let (fit_g, fit_d) = run_round(cfg, &mut gs, &mut ds, inner_iters, batch, device, cache.as_mut());
        for (m, &f) in gs.iter_mut().zip(&fit_g) { m.fitness = f; }
        for (m, &f) in ds.iter_mut().zip(&fit_d) { m.fitness = f; }

        let (_, g_replaced) = select_and_replace_g(&mut gs, &fit_g, mutate_frac, mutation_std, device);
        let (_, d_replaced) = select_and_replace_d(&mut ds, &fit_d, mutate_frac, mutation_std, device);

        let best_g = fit_g.iter().cloned().fold(f32::MIN, f32::max);
        let mean_g = fit_g.iter().sum::<f32>() / fit_g.len() as f32;
        let best_d = fit_d.iter().cloned().fold(f32::MIN, f32::max);
        let mean_d = fit_d.iter().sum::<f32>() / fit_d.len() as f32;
        println!(
            "[round {r}/{rounds}] G fitness best={best_g:.3} mean={mean_g:.3} | D fitness best={best_d:.3} mean={mean_d:.3} | replaced G{g_replaced:?} D{d_replaced:?} ({} ms)",
            t0.elapsed().as_millis()
        );
    }

    let best_idx = (0..gs.len()).max_by(|&a, &b| gs[a].fitness.partial_cmp(&gs[b].fitness).unwrap()).unwrap();
    println!("Done. Best generator fitness: {:.3}", gs[best_idx].fitness);
}

fn arg<T: std::str::FromStr>(args: &[String], key: &str) -> Option<T> {
    let i = args.iter().position(|a| a == key)?;
    args.get(i + 1)?.parse().ok()
}
