//! B2 parity 门：WGSL 直译噪声核 vs CPU `strata-nodes::noise` 的逐位对拍通路。
//!
//! 本模块只提供 GPU 侧「跑一遍 compute、量化 i16 回读」的机器；CPU 期望值与判决
//! 在集成测试（strata-app/tests）里做——那里同时能引 strata-nodes 与 strata-gpu。
//! 无 adapter / 无 SHADER_INT64 ⇒ 返回 None（不假绿），调用方据此跳过并披露。

#![allow(clippy::must_use_candidate)]

use std::sync::Arc;

const NOISE_SHADER: &str = include_str!("../shaders/noise_parity.wgsl");
const HYDRAULIC_SHADER: &str = include_str!("../shaders/hydraulic_parity.wgsl");

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
    request_i64_device_keyed().map(|(d, q, _)| (d, q))
}

/// 同上但一并返回平台键 `os|adapter|backend`（bench 登记/门用）。
pub fn request_i64_device_keyed() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>, String)> {
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
    let info = adapter.get_info();
    let key = format!("{}|{}|{:?}", std::env::consts::OS, info.name, info.backend);
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
    Some((Arc::new(device), Arc::new(queue), key))
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

// ---------- 水力侵蚀 parity ----------

/// 与 WGSL `hydraulic_parity.wgsl` 中 Params 逐字节对齐：2×u32 + 6×f32 = 32B。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HydroGpuParams {
    n: u32,
    _pad0: u32,
    k_capacity: f32,
    k_dissolve: f32,
    k_deposit: f32,
    rain: f32,
    evaporation: f32,
    min_water: f32,
}

/// 一步 GPU 水力 kernel 的参数（初始三场 + 常数 + 步数）。
pub struct HydroParityParams {
    pub n: u32,
    pub h0: Vec<f32>,
    pub water0: Vec<f32>,
    pub sed0: Vec<f32>,
    pub k_capacity: f32,
    pub k_dissolve: f32,
    pub k_deposit: f32,
    pub rain: f32,
    pub evaporation: f32,
    pub min_water: f32,
    pub steps: u32,
}

fn quant_i16(v: f32) -> i32 {
    (v * 32767.0).round().clamp(-32767.0, 32767.0) as i32
}

/// GPU 跑 `steps` 步水力，返回三场量化 i16 拼接（`[h(L) | water(L) | sed(L)]`）。
/// 无 adapter / 无 SHADER_INT64 / 映射失败 ⇒ None。
pub fn hydraulic_parity_grid(hp: &HydroParityParams) -> Option<Vec<i32>> {
    let (device, queue) = request_i64_device()?;
    let n = hp.n as usize;
    let len = n * n;
    assert_eq!(hp.h0.len(), len);
    assert_eq!(hp.water0.len(), len);
    assert_eq!(hp.sed0.len(), len);

    let params = HydroGpuParams {
        n: hp.n,
        _pad0: 0,
        k_capacity: hp.k_capacity,
        k_dissolve: hp.k_dissolve,
        k_deposit: hp.k_deposit,
        rain: hp.rain,
        evaporation: hp.evaporation,
        min_water: hp.min_water,
    };
    let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hydro_params"),
        size: std::mem::size_of::<HydroGpuParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(&params));

    // state 缓冲 ping-pong：a 起始装载初始三场；每步 out=b，步末 copy b→a。
    let state_bytes = (len * 3 * std::mem::size_of::<f32>()) as u64;
    let mk_state = |label: &'static str, usage: wgpu::BufferUsages| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: state_bytes,
            usage,
            mapped_at_creation: false,
        })
    };
    let st_use =
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
    let state_a = mk_state("hydro_a", st_use);
    let state_b = mk_state("hydro_b", st_use);
    let flow_bytes = (len * 8 * std::mem::size_of::<f32>()) as u64;
    let flow_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hydro_flow"),
        size: flow_bytes,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    // 初始 state 打包：[h | water | sed]。
    let mut init = Vec::with_capacity(len * 3);
    init.extend_from_slice(&hp.h0);
    init.extend_from_slice(&hp.water0);
    init.extend_from_slice(&hp.sed0);
    queue.write_buffer(&state_a, 0, bytemuck::cast_slice(&init));

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("hydraulic_parity"),
        source: wgpu::ShaderSource::Wgsl(HYDRAULIC_SHADER.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("hydro_bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("hydro_layout"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let mk_pipe = |ep: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(ep),
            layout: Some(&layout),
            module: &module,
            entry_point: Some(ep),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    let pipe1 = mk_pipe("pass1_flow");
    let pipe2 = mk_pipe("pass2_integrate");
    if let Some(e) = pollster::block_on(device.pop_error_scope()) {
        eprintln!("[hydro parity] 管线验证错: {e}");
        return None;
    }
    let mk_bg = |sin: &wgpu::Buffer, sout: &wgpu::Buffer| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hydro_bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ubuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: sin.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: flow_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: sout.as_entire_binding(),
                },
            ],
        })
    };
    // pass2 内 state_in 与 state_out 必须不同 buffer；这里固定 a→b。
    let bg_ab = mk_bg(&state_a, &state_b);

    let wg = hp.n.div_ceil(8);
    for _ in 0..hp.steps {
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut p = enc.begin_compute_pass(&Default::default());
            p.set_pipeline(&pipe1);
            p.set_bind_group(0, &bg_ab, &[]);
            p.dispatch_workgroups(wg, wg, 1);
            p.set_pipeline(&pipe2);
            p.set_bind_group(0, &bg_ab, &[]);
            p.dispatch_workgroups(wg, wg, 1);
        }
        // 步末把 b 拷回 a 供下一步的 state_in 使用。
        enc.copy_buffer_to_buffer(&state_b, 0, &state_a, 0, state_bytes);
        queue.submit(std::iter::once(enc.finish()));
    }

    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hydro_read"),
        size: state_bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    {
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&state_a, 0, &read, 0, state_bytes);
        queue.submit(std::iter::once(enc.finish()));
    }
    let slice = read.slice(0..state_bytes);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::Maintain::Wait);
    if !matches!(rx.recv(), Ok(Ok(()))) {
        return None;
    }
    let data = slice.get_mapped_range().to_vec();
    let floats: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
    Some(floats.iter().map(|&v| quant_i16(v)).collect())
}

// ---------- 可复用 GPU 侵蚀步进器（bench 面，一次建资源反复步进） ----------

/// 1024² 区域水力侵蚀的常驻 GPU 资源：device + 两管线 + ping-pong state + flow。
/// `hydraulic_parity_grid` 每次调用重建全部资源（parity 正确性够用），但 bench 计时
/// 绝不能含建 device/管线成本 ⇒ 这里建一次，`step_batch` 只跑真 GPU 步进。
pub struct ErosionStepper {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    platform_key: String,
    pipe1: wgpu::ComputePipeline,
    pipe2: wgpu::ComputePipeline,
    bg: wgpu::BindGroup,
    _ubuf: wgpu::Buffer,
    _flow_buf: wgpu::Buffer,
    state_a: wgpu::Buffer,
    state_b: wgpu::Buffer,
    state_bytes: u64,
    wg: u32,
}

/// bench 用的侵蚀步参数（与 CPU bench 口径一致的降雨/蒸发）。
pub struct ErosionBenchParams {
    pub n: u32,
    pub k_capacity: f32,
    pub k_dissolve: f32,
    pub k_deposit: f32,
    pub rain: f32,
    pub evaporation: f32,
    pub min_water: f32,
}

impl ErosionStepper {
    /// 建一次资源并装载初始态（锥 + 中心水杯）。无 adapter/SHADER_INT64 ⇒ None。
    pub fn new(bp: &ErosionBenchParams) -> Option<Self> {
        let (device, queue, platform_key) = request_i64_device_keyed()?;
        let n = bp.n as usize;
        let len = n * n;
        let params = HydroGpuParams {
            n: bp.n,
            _pad0: 0,
            k_capacity: bp.k_capacity,
            k_dissolve: bp.k_dissolve,
            k_deposit: bp.k_deposit,
            rain: bp.rain,
            evaporation: bp.evaporation,
            min_water: bp.min_water,
        };
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("erosion_params"),
            size: std::mem::size_of::<HydroGpuParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(&params));

        // 初始态：中心隆起锥 + 中心一杯水（数据驱动 min_water 分支，贴近真实）。
        let c = (bp.n - 1) as f32 / 2.0;
        let mut h0: Vec<f32> = (0..len)
            .map(|i| {
                let x = (i % n) as f32;
                let y = (i / n) as f32;
                let d = ((x - c) * (x - c) + (y - c) * (y - c)).sqrt();
                (0.9 - 0.0006 * d).max(0.0)
            })
            .collect();
        let mut water0 = vec![0.0f32; len];
        let center = (n / 2) * n + n / 2;
        water0[center] = 0.5;
        // 全域铺一层薄水，确保大部分格走流动分支（贴近连续降雨后的稳态步进）。
        for w in water0.iter_mut() {
            *w = (*w).max(bp.min_water * 2.0 + 0.02);
        }
        let mut sed0 = vec![0.0f32; len];
        let mut init = Vec::with_capacity(len * 3);
        init.append(&mut h0);
        init.append(&mut water0);
        init.append(&mut sed0);

        let state_bytes = (len * 3 * std::mem::size_of::<f32>()) as u64;
        let st_use = wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC;
        let state_a = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("erosion_a"),
            size: state_bytes,
            usage: st_use,
            mapped_at_creation: true,
        });
        {
            let mut view = state_a.slice(0..state_bytes).get_mapped_range_mut();
            view.copy_from_slice(bytemuck::cast_slice(&init));
        }
        state_a.unmap();
        let state_b = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("erosion_b"),
            size: state_bytes,
            usage: st_use,
            mapped_at_creation: false,
        });
        let flow_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("erosion_flow"),
            size: (len * 8 * std::mem::size_of::<f32>()) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hydraulic_parity"),
            source: wgpu::ShaderSource::Wgsl(HYDRAULIC_SHADER.into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("erosion_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("erosion_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let mk_pipe = |ep: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(ep),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(ep),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let pipe1 = mk_pipe("pass1_flow");
        let pipe2 = mk_pipe("pass2_integrate");
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("erosion_bg"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ubuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: state_a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: flow_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: state_b.as_entire_binding(),
                },
            ],
        });
        Some(Self {
            device,
            queue,
            platform_key,
            pipe1,
            pipe2,
            bg,
            _ubuf: ubuf,
            _flow_buf: flow_buf,
            state_a,
            state_b,
            state_bytes,
            wg: bp.n.div_ceil(8),
        })
    }

    /// 平台键（bench/baseline 登记用）。
    pub fn platform_key(&self) -> &str {
        &self.platform_key
    }

    /// 连跑 `k` 步（每步 = pass1+pass2+copy b→a）在一次提交里，末尾 poll(Wait) 真同步。
    /// 返回 GPU 是否完成（false ⇒ 上层据此 SKIP，不编数）。
    pub fn step_batch(&self, k: u32) -> bool {
        let mut enc = self.device.create_command_encoder(&Default::default());
        for _ in 0..k {
            {
                let mut p = enc.begin_compute_pass(&Default::default());
                p.set_pipeline(&self.pipe1);
                p.set_bind_group(0, &self.bg, &[]);
                p.dispatch_workgroups(self.wg, self.wg, 1);
                p.set_pipeline(&self.pipe2);
                p.set_bind_group(0, &self.bg, &[]);
                p.dispatch_workgroups(self.wg, self.wg, 1);
            }
            // 步内把 b 拷回 a：ping-pong。copy 在 compute pass 结束后（同编码器内有序）。
            enc.copy_buffer_to_buffer(&self.state_b, 0, &self.state_a, 0, self.state_bytes);
        }
        self.queue.submit(std::iter::once(enc.finish()));
        let _ = self.device.poll(wgpu::Maintain::Wait);
        true
    }
}
