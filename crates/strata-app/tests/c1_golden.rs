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
