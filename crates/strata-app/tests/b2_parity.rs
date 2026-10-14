//! B2 CPU/GPU parity 门（06/P8）：CPU 噪声核 vs WGSL 直译核，量化 i16 后对拍。
//!
//! 与 C1 golden 不同——这是**同机差分**判据（不依赖平台哈希）：只要 GPU 能建
//! SHADER_INT64 device，就把 CPU 现算的期望值和 GPU 回读逐位对拍。
//! - 有 device：GPU 双跑逐位自复现（恒严）+ GPU vs CPU 在分层容差内；
//! - 无 adapter / 无 SHADER_INT64：SKIP 并披露（不假绿）。
//!
//! 分层容差（实测钉死，P8 的第一手证据）：
//! 分层容差（实测钉死，P8 的第一手证据）：
//! - **plain fBm（地形实际采样路径）：max|Δ| = 0**（4096/4096 逐位一致）；
//! - **warped fBm：max|Δ| ≤ 1**（本机实测 4094/4096 一致，2 格差 1 LSB）。
//!   warp 那层「量化到整毫米」的粗 `.round()` 会把驱动 FMA 融合（WGSL 规范允许的
//!   `a*b+c`→fma）带来的 1-ulp 浮点差放大成 ±1 —— 这正是 CPU/GPU parity 的经典痛点，
//!   用容差显式登记而非假装不存在。真正的常数/拓扑错误会让 Δ 远超此界 ⇒ 必红。
//!
//! 负对照：seed+1 的 GPU 输出与 CPU(seed=42) 期望比较 ⇒ 必须 DIFF（证明有牙）。

use std::sync::{Mutex, MutexGuard};
use strata_gpu::parity::{noise_parity_grid, ParityParams};
use strata_nodes::noise::Warp;
use strata_nodes::region::NoiseNode;

static GPU_LOCK: Mutex<()> = Mutex::new(());
fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const N: u32 = 64;
const STEP_MM: i32 = 97;
/// 分层容差：plain 承重路径逐位=0；含 warp 允许 1 LSB（FMA 边界，见文件头）。
const TOL_PLAIN: i64 = 0;
const TOL_WARPED: i64 = 1;

/// CPU 侧：NoiseNode 采样整网格并量化成 i32（与 strata-field::quantize_i16 同式）。
fn cpu_grid(node: &NoiseNode) -> Vec<i32> {
    let mut v = Vec::with_capacity((N * N) as usize);
    for y in 0..N {
        for x in 0..N {
            let wx = i64::from(x) * i64::from(STEP_MM);
            let wy = i64::from(y) * i64::from(STEP_MM);
            let h = node.sample(wx, wy);
            v.push((h * 32767.0).round().clamp(-32767.0, 32767.0) as i32);
        }
    }
    v
}

fn node(warp: Option<Warp>) -> NoiseNode {
    NoiseNode {
        seed: 42,
        wavelength_mm: 512,
        octaves: 4,
        warp,
    }
}

fn warp_of(on: bool) -> Option<Warp> {
    if on {
        Some(Warp {
            amplitude_mm: 48,
            wavelength_mm: 256,
        })
    } else {
        None
    }
}

fn parity_params(warp: bool) -> ParityParams {
    ParityParams {
        seed: 42,
        step_mm: STEP_MM,
        wavelength_mm: 512,
        octaves: 4,
        warp: if warp { Some((48, 256)) } else { None },
        grid_n: N,
    }
}

fn run_case(label: &str, warp: bool, tol: i64) {
    let _g = gpu_lock();
    let Some(gpu) = noise_parity_grid(&parity_params(warp)) else {
        println!("B2 SKIP [{label}]: 无 adapter 或无 SHADER_INT64（本机不判，报告须披露）");
        return;
    };
    assert_eq!(gpu.len(), (N * N) as usize);

    // 判决面 1：GPU 双跑逐位自复现（同机确定性，恒严=0 容差）。
    let again = noise_parity_grid(&parity_params(warp)).expect("第二跑 adapter 消失？");
    assert_eq!(gpu, again, "B2 [{label}]: GPU 自身不可复现");

    // 判决面 2：GPU vs CPU（量化 i16）在分层容差内。
    let cpu = cpu_grid(&node(warp_of(warp)));
    let diffs: Vec<i64> = (0..cpu.len())
        .map(|i| (i64::from(cpu[i]) - i64::from(gpu[i])).abs())
        .collect();
    let n_exact = diffs.iter().filter(|&&d| d == 0).count();
    let max_diff = *diffs.iter().max().unwrap();
    println!(
        "B2 [{label}]: 逐位相等 {}/{}, max|CPU-GPU|={} (容差 {tol})",
        n_exact,
        cpu.len(),
        max_diff
    );
    assert!(
        max_diff <= tol,
        "B2 [{label}]: CPU/GPU 差 {max_diff} 超分层容差 {tol}（真回归非 FMA 边界）"
    );
}

#[test]
fn b2_parity_noise_plain_bitexact() {
    run_case("无 warp（承重）", false, TOL_PLAIN);
}

#[test]
fn b2_parity_noise_warped_within_1ulp() {
    run_case("有 warp", true, TOL_WARPED);
}

#[test]
fn b2_negative_control_seed_shift_must_differ() {
    // 比较器有牙：seed+1 的 GPU 输出与该网格的 CPU(seed=42) 期望必须大幅不同。
    let _g = gpu_lock();
    let mut p = parity_params(false);
    p.seed = 43;
    let Some(gpu) = noise_parity_grid(&p) else {
        println!("B2 NEG-CTL SKIP: 无 adapter/SHADER_INT64");
        return;
    };
    let cpu = cpu_grid(&node(None));
    let max_diff = (0..cpu.len())
        .map(|i| (i64::from(cpu[i]) - i64::from(gpu[i])).abs())
        .max()
        .unwrap();
    assert!(
        max_diff > TOL_PLAIN,
        "负对照失效：改 seed 却没让对拍变红 ⇒ 门是空转"
    );
    println!("B2 NEG-CTL PASS: seed 变化使 max|Δ|={max_diff} ⇒ 判红");
}
