//! C1 golden image 门（06）：GPU 真渲染地形回读像素 ⇒ 冻结哈希。
//!
//! 分层判据（防"跨 GPU 假红"掩盖真红）：
//! - **所有有 adapter 的平台**：非空图 + 双跑逐位复现（任一失败即红）；
//! - **已登记平台**（PINNED_PLATFORM）：追加冻结哈希断言（像素级回归即红）；
//! - 无 adapter：SKIP 并披露；未登记平台：哈希项 SKIP 披露（登记走分文件基线，债务已记 gates.md）。
//!
//! 负对照：改 WGSL 常数 / mesh 拓扑 ⇒ 登记平台哈希变红 + 任何平台双跑异常。

use std::sync::{Mutex, MutexGuard};
use strata_gpu::math::Mat4;
use strata_gpu::renderer::{fnv1a64, Frame, OffscreenTerrain};
use strata_render::mesh::TerrainMesh;

/// 进程内 GPU 测试串行锁：cargo test 默认多线程，同二进制里两个测试
/// 并发创建 Vulkan 设备曾实测互抢挂死（本机 33min CPU 空转案例）。
static GPU_LOCK: Mutex<()> = Mutex::new(());

fn gpu_lock() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

const SIZE: u32 = 128;
const TEX_DIM: u32 = 64;

/// 哈希基线生效的平台（2026-10-14 实测登记）。
const PINNED_PLATFORM: &str = "windows|NVIDIA GeForce RTX 4060 Ti|Vulkan";
/// C1#1 单层 legacy 场景冻结哈希（本机实测）。
const GOLDEN_HASH: u64 = 17092285482363414448;
/// C1#2 十层 clipmap 场景冻结哈希（本机实测）。
const GOLDEN_SCENE_HASH: u64 = 3779239380702403447;

/// 确定性解析高度场（只用乘加，无超越函数）：碗状 + 棋盘扰动。
fn height_field() -> Vec<f32> {
    let mut h = Vec::with_capacity((TEX_DIM * TEX_DIM) as usize);
    for y in 0..TEX_DIM {
        for x in 0..TEX_DIM {
            let fx = x as f32 / TEX_DIM as f32 - 0.5;
            let fy = y as f32 / TEX_DIM as f32 - 0.5;
            let d = fx * fx + fy * fy;
            let chk = if (x + y) % 2 == 0 { 0.05 } else { -0.05 };
            h.push(-2.0 * d + chk);
        }
    }
    h
}

fn render_once() -> Option<(Vec<u8>, String)> {
    let rt = OffscreenTerrain::new()?;
    let platform = rt.platform_key().to_string();
    let mesh = TerrainMesh::new(32, 0.05);
    let vp = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 0.05, 50.0).mul(
        &Mat4::look_at_rh([2.0, 1.6, 2.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    );
    let frame = Frame {
        height: &height_field(),
        tex_dim: TEX_DIM,
        verts: &mesh.vertices(),
        indices: &mesh.indices(),
        view_proj: vp.0,
        height_scale: 0.6,
        size: SIZE,
    };
    Some((rt.render(&frame)?, platform))
}

/// 三判决面：非空图 → 双跑复现 → 登记平台才核冻结哈希。
fn judge(label: &str, px: &[u8], again: &[u8], platform: &str, golden: u64) {
    assert_eq!(px.len(), again.len());
    // 判决面 1：真的画出了东西（存在非清屏色的像素）
    let drawn = (0..px.len())
        .step_by(4)
        .filter(|&i| (i32::from(px[i]) - 26).abs() > 8)
        .count();
    assert!(drawn > 1000, "{label} 回读近乎空图: drawn={drawn}");
    // 判决面 2：双跑逐位一致（同机确定性，任何平台都判）
    assert_eq!(fnv1a64(px), fnv1a64(again), "{label} GPU 渲染不自复现");
    // 判决面 3：登记平台核冻结哈希
    if platform == PINNED_PLATFORM {
        assert_eq!(fnv1a64(px), golden, "{label} golden 漂移");
    } else {
        println!("{label} 哈希项 SKIP：平台未登记 ({platform})，复现性断言已判");
    }
}

#[test]
fn c1_golden_terrain_pixels() {
    let _g = gpu_lock();
    let Some((px, platform)) = render_once() else {
        eprintln!("C1 SKIP: 本机无可用 GPU adapter（门在此环境不判，报告须披露）");
        return;
    };
    let (again, _) = render_once().expect("第二次渲染 adapter 突然消失？");
    judge("C1", &px, &again, &platform, GOLDEN_HASH);
}

// ---------- C1#2：M1d 十层 clipmap 场景（真实漫游路径的渲染面） ----------

use strata_app::clipmap_state::{demo_scene, Cam3};
use strata_gpu::renderer::{readback_rgba8, request_offscreen_device, TerrainGpu};

const SCENE_SIZE: u32 = 256;

fn render_scene() -> Option<(Vec<u8>, String)> {
    let dev = request_offscreen_device()?;
    let (device, queue) = (dev.device, dev.queue);
    let mesh = TerrainMesh::new(64, 0.02);
    let clip = demo_scene();
    let mut gpu = TerrainGpu::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    gpu.set_mesh(&device, &queue, &mesh.vertices(), &mesh.indices());
    gpu.configure_layers(&device, clip.specs().len(), 64);
    for i in 0..clip.specs().len() {
        gpu.upload_layer(&queue, i, &clip.heights[i]);
    }
    gpu.resize(&device, SCENE_SIZE, SCENE_SIZE);
    let cam = Cam3 {
        yaw: 0.6,
        pitch: 0.45,
        dist_m: 900.0,
    };
    let eye = cam.eye_m();
    let vp = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 0.5, 90_000.0)
        .mul(&Mat4::look_at_rh(eye, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]));
    let frames = clip.layer_frames(vp.0);
    if !gpu.draw_layers(&device, &queue, &frames) {
        return None;
    }
    let px = readback_rgba8(
        &device,
        &queue,
        &gpu.target_texture()?,
        SCENE_SIZE,
        SCENE_SIZE,
    )?;
    Some((px, dev.platform_key))
}

#[test]
fn c1b_golden_clipmap_scene() {
    let _g = gpu_lock();
    let Some((px, platform)) = render_scene() else {
        eprintln!("C1b SKIP: 本机无可用 GPU adapter");
        return;
    };
    let (again, _) = render_scene().expect("第二次渲染 adapter 突然消失？");
    judge("C1b", &px, &again, &platform, GOLDEN_SCENE_HASH);
}

#[test]
fn roam_regen_is_incremental_not_full_rebake() {
    // P3 漫游实证：37 帧斜线漫游的重生成次数必须远小于 37×层数（全图重烤），
    // 且大于 0（证明不是"从不更新"的假绿）。
    let mut clip = demo_scene();
    let base = clip.regen_count;
    let layers = clip.specs().len() as u64;
    for step in 1..=37i64 {
        let (ex, ey) = clip.eye_mm();
        clip.move_eye(ex + step * 5_000, ey + step * 3_000);
    }
    let delta = clip.regen_count - base;
    assert!(delta > 0, "漫游 37 帧零重生成——流式失效");
    assert!(
        delta < 37 * layers,
        "漫游退化为全图重烤: delta={delta} 上限={}",
        37 * layers
    );
}
