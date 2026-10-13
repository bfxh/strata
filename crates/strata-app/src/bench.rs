//! 性能/内存自检执行面（D1/D2 门的唯一测量入口）。
//!
//! `--bench-render N` 在**无窗口**的自持 device 上跑真实帧管线：
//! 每帧 = 漫游增量（regen/upload 只在锚点变化时发生）→ 层 uniform → draw_layers
//! → `device.poll(Wait)` 真同步。所以 `AVG_FPS` 是含 GPU 完成等待的帧成本，
//! 不是提交吞吐假数。口径限制（如实）：debug profile、不含 egui/UI 与表面呈现。
//!
//! 输出 KEY=VALUE 行，由 `cargo xtask bench --check` 对照 `bench/baseline.env` 判红。

use std::time::Instant;

use strata_gpu::renderer::{request_offscreen_device, TerrainGpu};
use strata_nodes::region::NoiseNode;
use strata_render::mesh::TerrainMesh;

use crate::clipmap_state::{demo_scene, Cam3, SceneParams};

pub struct BenchReport {
    pub noise_tile_ms: f64,
    pub clipmap_full_regen_ms: f64,
    pub avg_fps: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub regen_total: u64,
    pub uploads_total: u64,
    pub peak_rss_mb: Option<f64>,
}

impl BenchReport {
    /// 机器可读输出（xtask 逐行解析）。
    pub fn print(&self) {
        println!("NOISE_TILE_MS={:.3}", self.noise_tile_ms);
        println!("CLIPMAP_FULL_REGEN_MS={:.3}", self.clipmap_full_regen_ms);
        println!("AVG_FPS={:.2}", self.avg_fps);
        println!("P50_MS={:.3}", self.p50_ms);
        println!("P95_MS={:.3}", self.p95_ms);
        println!("REGEN_TOTAL={}", self.regen_total);
        println!("UPLOADS_TOTAL={}", self.uploads_total);
        match self.peak_rss_mb {
            Some(mb) => println!("PEAK_RSS_MB={mb:.1}"),
            None => println!("PEAK_RSS_MB=NA"),
        }
    }
}

/// 跑 N 帧渲染基准。无 GPU ⇒ None（打印 SKIP，调用方决定如何处置）。
pub fn run_render_bench(frames: u32) -> Option<BenchReport> {
    let (device, queue) = request_offscreen_device()?;
    let params = SceneParams::default();

    // 1) CPU 噪声核：512² 单 tile 求算（min-of-3，压掉调度抖动）
    let node = NoiseNode::new(0x57_0000_0000_0001, 400_000, 5);
    let mut best = f64::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        let _ = strata_nodes::CpuNode::eval_height(
            &node,
            strata_geo::TilePos {
                x: 0,
                y: 0,
                level: 0,
            },
            512,
            0,
            strata_geo::DEFAULT_TILE_SPAN_PX,
        );
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
    }
    let noise_tile_ms = best;

    // 2) clipmap 全量重生成（10 层）耗时——只计 force_regen 这一段，不含初始构建
    let mut clip = demo_scene();
    let t = Instant::now();
    clip.force_regen_all();
    let full_regen_ms = t.elapsed().as_secs_f64() * 1000.0;

    // 3) 帧管线：斜线漫游 + 真同步
    let mesh = TerrainMesh::new(params.res, 0.02);
    let mut gpu = TerrainGpu::new(&device, wgpu::TextureFormat::Rgba8Unorm);
    gpu.set_mesh(&device, &queue, &mesh.vertices(), &mesh.indices());
    gpu.configure_layers(&device, params.layers as usize, params.res);
    gpu.resize(&device, 1280, 720);
    for i in 0..params.layers as usize {
        gpu.upload_layer(&queue, i, &clip.heights[i]);
    }
    let cam = Cam3 {
        yaw: 0.6,
        pitch: 0.45,
        dist_m: 900.0,
    };
    let eye = cam.eye_m();
    let vp = strata_gpu::math::Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_4,
        1280.0 / 720.0,
        0.5,
        90_000.0,
    )
    .mul(&strata_gpu::math::Mat4::look_at_rh(
        eye,
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
    ))
    .0;
    let mut frame_ms: Vec<f64> = Vec::with_capacity(frames as usize);
    let regen0 = clip.regen_count;
    let up0 = clip.upload_count;
    let mut ex = 25_000_000i64;
    let mut ey = -18_000_000i64;
    for _step in 1..=frames {
        let t = Instant::now();
        ex += 5_000; // 5m/帧 ≈ 300m/s 巡航
        ey += 3_000;
        clip.move_eye(ex, ey);
        for (idx, data) in &clip.take_uploads() {
            gpu.upload_layer(&queue, *idx, data);
        }
        let frames_params = clip.layer_frames(vp);
        if !gpu.draw_layers(&device, &queue, &frames_params) {
            return None;
        }
        // 真同步：等 GPU 做完才算这一帧的成本
        let _ = device.poll(wgpu::Maintain::Wait);
        frame_ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    frame_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let total: f64 = frame_ms.iter().sum();
    let avg_fps = frame_ms.len() as f64 * 1000.0 / total.max(1e-9);
    let p50 = frame_ms[frame_ms.len() / 2];
    let p95 = frame_ms[((frame_ms.len() as f64 * 0.95).floor() as usize).min(frame_ms.len() - 1)];

    Some(BenchReport {
        noise_tile_ms,
        clipmap_full_regen_ms: full_regen_ms,
        avg_fps,
        p50_ms: p50,
        p95_ms: p95,
        regen_total: clip.regen_count - regen0,
        uploads_total: clip.upload_count - up0,
        peak_rss_mb: peak_rss_mb(),
    })
}

/// 峰值工作集（MB）。Windows 实测；其他平台 None（如实 NA，不编数）。
#[allow(unsafe_code)] // 唯一 FFI 点：GetProcessMemoryInfo 是文档化 WinAPI，入参为本进程伪句柄+栈上结构
pub fn peak_rss_mb() -> Option<f64> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{
            GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut pmc = PROCESS_MEMORY_COUNTERS {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            PageFaultCount: 0,
            PeakWorkingSetSize: 0,
            WorkingSetSize: 0,
            QuotaPeakPagedPoolUsage: 0,
            QuotaPagedPoolUsage: 0,
            QuotaPeakNonPagedPoolUsage: 0,
            QuotaNonPagedPoolUsage: 0,
            PagefileUsage: 0,
            PeakPagefileUsage: 0,
        };
        let ok = unsafe {
            GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut pmc,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            )
        };
        if ok != 0 {
            Some(pmc.PeakWorkingSetSize as f64 / (1024.0 * 1024.0))
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}
