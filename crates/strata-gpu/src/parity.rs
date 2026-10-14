//! B2 parity 门：WGSL 直译噪声核 vs CPU `strata-nodes::noise` 的逐位对拍通路。
//!
//! 本模块只提供 GPU 侧「跑一遍 compute、量化 i16 回读」的机器；CPU 期望值与判决
//! 在集成测试（strata-app/tests）里做——那里同时能引 strata-nodes 与 strata-gpu。
//! 无 adapter / 无 SHADER_INT64 ⇒ 返回 None（不假绿），调用方据此跳过并披露。

#![allow(clippy::must_use_candidate)]

use std::sync::Arc;

const NOISE_SHADER: &str = include_str!("../shaders/noise_parity.wgsl");

/// 与 WGSL `Params` 逐字节对齐：10×u32 内容 + 2 pad = 48B（uniform 需 16 对齐）。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuParams {
    seed_lo: u32,
    seed_hi: u32,
    step_mm: i32,
    wavelength_mm: i32,
    octaves: u32,
    has_warp: u32,
    warp_amp_mm: i32,
    warp_wl_mm: i32,
    grid_n: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

/// GPU 噪声核的一次求算参数（全部 i32 域，|v|<2^24 保证 f32 精确）。
pub struct ParityParams {
    pub seed: u64,
    pub step_mm: i32,
    pub wavelength_mm: i32,
    pub octaves: u32,
    pub warp: Option<(i32, i32)>, // (amplitude_mm, wavelength_mm)
    pub grid_n: u32,
}

fn request_i64_device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))?;
    if !adapter.features().contains(wgpu::Features::SHADER_INT64) {
        eprintln!("[parity] SKIP: adapter 无 SHADER_INT64（u64 噪声核跑不了）");
        return None;
    }
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("strata-parity"),
            required_features: wgpu::Features::SHADER_INT64,
            required_limits: adapter.limits(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    Some((Arc::new(device), Arc::new(queue)))
}

/// 跑 GPU 噪声核，返回 grid_n×grid_n 行主序的量化 i16（存 i32）。
/// 无 adapter / 无 SHADER_INT64 / 映射失败 ⇒ None。
pub fn noise_parity_grid(pp: &ParityParams) -> Option<Vec<i32>> {
    let (device, queue) = request_i64_device()?;
    let n = pp.grid_n as usize;
    let params = GpuParams {
        seed_lo: pp.seed as u32,
        seed_hi: (pp.seed >> 32) as u32,
        step_mm: pp.step_mm,
        wavelength_mm: pp.wavelength_mm,
        octaves: pp.octaves,
        has_warp: u32::from(pp.warp.is_some()),
        warp_amp_mm: pp.warp.map_or(0, |w| w.0),
        warp_wl_mm: pp.warp.map_or(0, |w| w.1),
        grid_n: pp.grid_n,
        _pad0: 0,
        _pad1: 0,
        _pad2: 0,
    };
    let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parity_params"),
        size: std::mem::size_of::<GpuParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(&params));

    let out_bytes = (n * n * std::mem::size_of::<i32>()) as u64;
    let sbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parity_out"),
        size: out_bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("noise_parity"),
        source: wgpu::ShaderSource::Wgsl(NOISE_SHADER.into()),
    });
    let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("noise_parity"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("parity_bg"),
        layout: &pipe.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ubuf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: sbuf.as_entire_binding(),
            },
        ],
    });
    {
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipe);
            pass.set_bind_group(0, &bg, &[]);
            let wg = pp.grid_n.div_ceil(8);
            pass.dispatch_workgroups(wg, wg, 1);
        }
        queue.submit(std::iter::once(enc.finish()));
    }

    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("parity_read"),
        size: out_bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    {
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&sbuf, 0, &read, 0, out_bytes);
        queue.submit(std::iter::once(enc.finish()));
    }
    let slice = read.slice(0..out_bytes);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    if !matches!(rx.recv(), Ok(Ok(()))) {
        return None;
    }
    let data = slice.get_mapped_range().to_vec();
    Some(
        (0..n * n)
            .map(|i| {
                let o = i * 4;
                i32::from_le_bytes(data[o..o + 4].try_into().unwrap())
            })
            .collect(),
    )
}
