//! B2 侵蚀 parity：CPU `strata-nodes::erosion::Hydraulic` vs WGSL 直译（`hydraulic_parity.wgsl`）。
//!
//! 关键判据（实测钉死）：
//! - **单步逐位对拍**（STEPS=1）：本机 Vulkan h/water/sed 三场 max|Δ|=0；跨后端留
//!   ≤1 LSB（同 b2_parity 的 FMA 边界口径）。这证明 compute 核忠实实现了 CPU 的
//!   序无关并行步。
//! - **多步是混沌放大，不承诺 CPU↔GPU 逐位**：本机实测同一 kernel 12 步后
//!   max|Δ|≈652 —— step1 的 sub-ULP 差被侵蚀反馈放大成宏观差。这是 f32 迭代
//!   动力学的固有性质（正对应 P3/P8），门的诚实做法是"单步钉死核正确性 +
//!   GPU 多步自复现"，而非假装多步也能跨后端逐位。
//!
//! 恒判：GPU 双跑逐位自复现（同设备确定性）。负对照：k_capacity×2 ⇒ 单步 Δ≫1。

use std::sync::{Mutex, MutexGuard};
use strata_gpu::parity::{hydraulic_parity_grid, HydroParityParams};
use strata_nodes::erosion::{Hydraulic, HydraulicParams};

static GPU_LOCK: Mutex<()> = Mutex::new(());
fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const N: u32 = 32;
const TOL: i64 = 1; // 单步跨后端量化边界（同 b2_parity），本机实测=0

fn cone(n: u32, amp: f32) -> Vec<f32> {
    let c = (n - 1) as f32 / 2.0;
    (0..n * n)
        .map(|i| {
            let x = (i % n) as f32;
            let y = (i / n) as f32;
            let d = ((x - c) * (x - c) + (y - c) * (y - c)).sqrt();
            (amp - 0.1 * d).max(0.0)
        })
        .collect()
}

fn initial_state() -> (Vec<f32>, Vec<f32>, Vec<f32>) {
    let len = (N * N) as usize;
    let h0 = cone(N, 0.9);
    let mut water0 = vec![0.0f32; len];
    water0[((N / 2) * N + (N / 2)) as usize] = 0.4;
    let sed0 = vec![0.0f32; len];
    (h0, water0, sed0)
}

fn cpu_grid(cap: f32, steps: u32) -> Vec<i32> {
    let (h0, water0, sed0) = initial_state();
    let mut sim = Hydraulic::new(N as usize, h0);
    sim.water = water0;
    sim.sediment = sed0;
    let p = HydraulicParams {
        rain: 0.002,
        evaporation: 0.01,
        k_capacity: cap,
        ..Default::default()
    };
    for _ in 0..steps {
        sim.step(&p);
    }
    let mut out = Vec::with_capacity((N * N * 3) as usize);
    for field in [&sim.h, &sim.water, &sim.sediment] {
        for &v in field {
            out.push((v * 32767.0).round().clamp(-32767.0, 32767.0) as i32);
        }
    }
    out
}

fn build_hp(cap: f32, steps: u32) -> HydroParityParams {
    let (h0, water0, sed0) = initial_state();
    HydroParityParams {
        n: N,
        h0,
        water0,
        sed0,
        k_capacity: cap,
        k_dissolve: 0.004,
        k_deposit: 0.35,
        rain: 0.002,
        evaporation: 0.01,
        min_water: 1e-6,
        steps,
    }
}

fn max_diff(a: &[i32], b: &[i32]) -> i64 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (*x as i64 - *y as i64).abs())
        .max()
        .unwrap_or(0)
}

#[test]
fn b2_hydraulic_single_step_parity() {
    let _g = gpu_lock();
    let hp = build_hp(4.0, 1);
    let Some(gpu) = hydraulic_parity_grid(&hp) else {
        println!("B2-ERO SKIP: 无 adapter/SHADER_INT64");
        return;
    };
    let cpu = cpu_grid(4.0, 1);
    assert_eq!(gpu.len(), cpu.len());

    // 恒判：GPU 双跑逐位自复现（同设备确定性）。
    let again = hydraulic_parity_grid(&build_hp(4.0, 1)).expect("第二跑 adapter 消失？");
    assert_eq!(gpu, again, "B2-ERO: GPU 双跑不可复现");

    let len = (N * N) as usize;
    let d = max_diff(&cpu, &gpu);
    println!(
        "B2-ERO 单步: max|CPU-GPU|={d} (h={}, water={}, sed={}, 容差 {TOL})",
        max_diff(&cpu[..len], &gpu[..len]),
        max_diff(&cpu[len..2 * len], &gpu[len..2 * len]),
        max_diff(&cpu[2 * len..], &gpu[2 * len..]),
    );
    assert!(d <= TOL, "B2-ERO: 单步差 {d} 超容差 ⇒ 核翻译有误（非混沌）");
}

#[test]
fn b2_hydraulic_multistep_gpu_self_reproducible() {
    // 多步不承诺 CPU↔GPU 逐位（混沌），但 GPU 同设备必须逐位自复现。
    let _g = gpu_lock();
    let a = hydraulic_parity_grid(&build_hp(4.0, 12));
    let Some(a) = a else {
        println!("B2-ERO SKIP: 无 adapter/SHADER_INT64");
        return;
    };
    let b = hydraulic_parity_grid(&build_hp(4.0, 12)).expect("第二跑 adapter 消失？");
    assert_eq!(a, b, "B2-ERO: GPU 12 步不可自复现");
    println!("B2-ERO 12 步: GPU 双跑逐位自复现 OK");
}

#[test]
fn b2_hydraulic_negative_control_k_capacity_must_differ() {
    // 比较器有牙：GPU 用 k_capacity=8 与 CPU(k_capacity=4) 单步必差远超 1 LSB。
    let _g = gpu_lock();
    let hp = build_hp(8.0, 1);
    let Some(gpu) = hydraulic_parity_grid(&hp) else {
        println!("B2-ERO NEG-CTL SKIP: 无 adapter/SHADER_INT64");
        return;
    };
    let cpu = cpu_grid(4.0, 1);
    let d = max_diff(&cpu, &gpu);
    assert!(d > TOL, "负对照失效：改 k_capacity 却没让对拍变红 ⇒ 门空转");
    println!("B2-ERO NEG-CTL PASS: k_capacity 扰动使单步 max|Δ|={d} ⇒ 判红");
}
