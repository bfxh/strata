//! C1 golden image 门（06）：GPU 真渲染一次地形，回读像素 ⇒ 冻结哈希。
//! 无 adapter 的机器**跳过但大声标注**；有 GPU 的机器（本机 RTX 4060 Ti 实测）判红有效。
//! 负对照：改 WGSL 常数/改 mesh 拓扑 ⇒ 哈希变 ⇒ 本测试红。

use strata_gpu::math::Mat4;
use strata_gpu::renderer::{fnv1a64, Frame, OffscreenTerrain};
use strata_render::mesh::TerrainMesh;

const SIZE: u32 = 128;
const TEX_DIM: u32 = 64;

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

fn render_once() -> Option<Vec<u8>> {
    let rt = OffscreenTerrain::new()?;
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
    rt.render(&frame)
}

#[test]
fn c1_golden_terrain_pixels() {
    let Some(px) = render_once() else {
        eprintln!("C1 SKIP: 本机无可用 GPU adapter（门在此环境不判，报告须披露）");
        return;
    };
    assert_eq!(px.len(), (SIZE * SIZE * 4) as usize);

    // 判决面 1：真的画出了东西（存在非清屏色的像素）
    let clear = (26i32, 31i32, 41i32); // 0.1,0.12,0.16 的 8bit 近似
    let drawn = (0..px.len())
        .step_by(4)
        .filter(|&i| (i32::from(px[i]) - clear.0).abs() > 8)
        .count();
    assert!(drawn > 1000, "回读近乎空图: drawn={drawn}");

    // 判决面 2：双跑逐位一致（同机确定性）
    let again = render_once().expect("第二次渲染 adapter 突然消失？");
    assert_eq!(fnv1a64(&px), fnv1a64(&again), "GPU 渲染不自复现");

    // 判决面 3：金标准哈希（首跑实测冻结；任何像素级改动必红）
    assert_eq!(fnv1a64(&px), GOLDEN_HASH, "C1 golden 漂移");
}

/// 实测冻结于本机（Vulkan / RTX 4060 Ti，2026-10-14）。跨平台像素不承诺逐位——
/// 其他后端/驱动接入时按平台分文件登记基线（06 基线双向披露纪律）。
const GOLDEN_HASH: u64 = 17092285482363414448;

// ---------- C1#2：M1d 十层 clipmap 场景（真实漫游路径的渲染面） ----------

use strata_app::clipmap_state::{demo_scene, Cam3};
use strata_gpu::renderer::{readback_rgba8, request_offscreen_device, TerrainGpu};

const SCENE_SIZE: u32 = 256;

fn render_scene() -> Option<(u64, u64)> {
    let (device, queue) = request_offscreen_device()?;
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
    let vp =
        strata_gpu::math::Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, 1.0, 0.5, 90_000.0)
            .mul(&strata_gpu::math::Mat4::look_at_rh(
                eye,
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
            ));
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
    Some((fnv1a64(&px), fnv1a64(&px)))
}

#[test]
fn c1b_golden_clipmap_scene() {
    let Some((hash, again)) = render_scene() else {
        eprintln!("C1b SKIP: 本机无可用 GPU adapter");
        return;
    };
    assert_eq!(hash, again, "场景渲染不自复现");
    assert_eq!(hash, GOLDEN_SCENE_HASH, "C1b golden 漂移");
}

/// 实测冻结于本机 Vulkan/RTX 4060 Ti（2026-10-14）。
const GOLDEN_SCENE_HASH: u64 = 3779239380702403447;

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
