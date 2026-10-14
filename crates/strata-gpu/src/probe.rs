//! GPU 适配器探针：C1/视口都要先知道"这台机器能不能真渲染"。
//! 判据 = 能否拿到 adapter 并建出 device；拿不到就如实报不可用，绝不假装渲染成功。

/// 探测结果：后端名 + adapter 名；None = 无可用 GPU。
pub fn probe() -> Option<(String, String)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .or_else(|| {
        // 独显/核显都拿不到时，试软件回退（llvmpipe/SwiftShader），CI 无头机常走这条
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            force_fallback_adapter: true,
            ..Default::default()
        }))
    })?;
    let info = adapter.get_info();
    Some((format!("{:?}", info.backend), info.name))
}

// ---------- B2 parity 技术探针（非承重：只为量出本机 u64 着色器能力） ----------

use std::sync::Arc;

const PROBE_U64: &str = include_str!("../shaders/probe_u64.wgsl");

/// u64 compute 探针的实测结论（供选路，不作门禁）。
pub struct U64Probe {
    pub shader_int64_feature: bool,
    pub pipeline_built: bool,
    /// Some([gpu[4], cpu[4]])：direct/limb/mix_direct/mix_limb（正确 16 位常量下重测 u64 乘）。
    pub roundtrip: Option<[[u64; 4]; 2]>,
    pub note: String,
}

/// splitmix64（CPU 参考，与 noise.rs 同款常量）——探针直译/limb 模拟共同的期望值源。
fn cpu_mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// 跑一遍 u64 compute；任何失败以 note 记录并返回，绝不 panic。
pub fn u64_parity_probe() -> Option<U64Probe> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    }))?;
    let has_i64 = adapter.features().contains(wgpu::Features::SHADER_INT64);
    // 无论 feature 在不在都先建 device：不在时把 pipeline 创建包进 validation error scope 观测。
    let feats = if has_i64 {
        wgpu::Features::SHADER_INT64
    } else {
        wgpu::Features::empty()
    };
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("strata-u64-probe"),
            required_features: feats,
            required_limits: adapter.limits(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    let dev = Arc::new(device);
    let que = Arc::new(queue);

    dev.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("probe_u64"),
        source: wgpu::ShaderSource::Wgsl(PROBE_U64.into()),
    });
    let pipeline = dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("probe_u64"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let build_err = pollster::block_on(dev.pop_error_scope());
    let built = build_err.is_none();
    if !built {
        return Some(U64Probe {
            shader_int64_feature: has_i64,
            pipeline_built: false,
            roundtrip: None,
            note: build_err
                .map(|e| e.to_string())
                .unwrap_or_else(|| "pipeline Err".to_string()),
        });
    }

    // WGSL Out = 4×u64（direct/limb/mix_direct/mix_limb），偏移 0/8/16/24。
    const SEED: u64 = 0x1234_5678_9abc_def0;
    const C1: u64 = 0xbf58_476d_1ce4_e5b9;
    let cpu: [u64; 4] = [
        SEED.wrapping_mul(C1),
        SEED.wrapping_mul(C1), // limb 模拟须等于直译 wrapping_mul
        cpu_mix64(SEED),
        cpu_mix64(SEED),
    ];
    let buf = dev.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe_out"),
        size: 32,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("probe_bg"),
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    });
    {
        let mut enc = dev.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        que.submit(std::iter::once(enc.finish()));
    }
    // 读回 32B（4×u64）。
    let read = dev.create_buffer(&wgpu::BufferDescriptor {
        label: Some("probe_read"),
        size: 32,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    {
        let mut enc = dev.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&buf, 0, &read, 0, 32);
        que.submit(std::iter::once(enc.finish()));
    }
    let slice = read.slice(0..32);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    dev.poll(wgpu::Maintain::Wait);
    match rx.recv() {
        Ok(Ok(())) => {
            let data = slice.get_mapped_range().to_vec();
            let g64 = |o: usize| u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
            let gpu = [g64(0), g64(8), g64(16), g64(24)];
            Some(U64Probe {
                shader_int64_feature: has_i64,
                pipeline_built: true,
                roundtrip: Some([gpu, cpu]),
                note: String::new(),
            })
        }
        Ok(Err(e)) => Some(U64Probe {
            shader_int64_feature: has_i64,
            pipeline_built: true,
            roundtrip: None,
            note: format!("map_async: {e}"),
        }),
        Err(e) => Some(U64Probe {
            shader_int64_feature: has_i64,
            pipeline_built: true,
            roundtrip: None,
            note: format!("channel: {e}"),
        }),
    }
}

#[cfg(test)]
mod u64_probe_test {
    use super::u64_parity_probe;

    // 非承重探针（B2 选路用，不进硬门）：跑一次把本机 u64 能力打印出来。
    // 无 adapter ⇒ 打印 SKIP；SHADER_INT64 缺失 ⇒ pipeline 建不出来也是合法结论。
    #[test]
    #[ignore = "人工探针：cargo test -p strata-gpu -- --ignored --nocapture"]
    fn print_u64_probe() {
        match u64_parity_probe() {
            None => println!("U64Probe SKIP: 无可用 adapter"),
            Some(p) => {
                println!(
                    "U64Probe feature(SHADER_INT64)={} built={} note={}",
                    p.shader_int64_feature, p.pipeline_built, p.note,
                );
                if let Some([gpu, cpu]) = p.roundtrip {
                    let names = ["direct", "limb", "mix_direct", "mix_limb"];
                    for (name, (g, c)) in names.iter().zip(gpu.iter().zip(cpu.iter())) {
                        println!(
                            "  {:>8}: gpu={:016x} cpu={:016x} {}",
                            name,
                            g,
                            c,
                            if g == c { "MATCH" } else { "*** DIFF ***" }
                        );
                    }
                }
            }
        }
    }
}
