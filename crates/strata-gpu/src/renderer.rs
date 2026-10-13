//! 地形 GPU 管线：terrain pass（离屏 Rgba8Unorm + 深度）+ sprite pass（贴到显示面）。
//!
//! 设计约束（M1c 核实于 egui-wgpu 0.31.1）：egui 主 render pass **没有深度附件**，
//! 所以地形必须先画进自持离屏纹理，再以 sprite 三角形 + viewport 贴进 egui pass。
//! [`TerrainGpu`] 不持有 device/queue（方法参数传入），离屏自持与 eframe 复用
//! egui 设备两条路共用同一套管线。没有可用 adapter 时上层返回 None——
//! 调用方（含 C1 测试）据此跳过而非假绿。

#![allow(clippy::must_use_candidate)]

use std::sync::{Arc, Mutex};

const SHADER: &str = include_str!("../shaders/terrain.wgsl");

/// 与 WGSL `Uniforms` 逐字节对齐：mat4(64) + height_scale + tex_dim + pad(8) = 80。
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [f32; 16],
    height_scale: f32,
    tex_dim: f32,
    _pad: [f32; 2],
}

struct Target {
    color: wgpu::Texture,
    view: wgpu::TextureView,
    #[allow(dead_code)]
    depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    sprite_bg: wgpu::BindGroup,
    w: u32,
    h: u32,
}

/// 地形渲染管线组（terrain → 自持离屏纹理；sprite → 显示面格式）。
pub struct TerrainGpu {
    terrain_pipeline: wgpu::RenderPipeline,
    sprite_pipeline: wgpu::RenderPipeline,
    material_bgl: wgpu::BindGroupLayout,
    sprite_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    ubuf: wgpu::Buffer,
    height: Option<(wgpu::Texture, wgpu::BindGroup, u32)>,
    mesh: Option<(wgpu::Buffer, wgpu::Buffer, u32)>,
    target: Mutex<Option<Target>>,
}

fn tex_info(texture: &wgpu::Texture) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
    }
}

fn make_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    contents: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: contents.len() as u64,
        usage,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buf, 0, contents);
    buf
}

impl TerrainGpu {
    /// `display_format` = sprite 最终落到的表面格式（离屏自持用 Rgba8Unorm；
    /// eframe 用 `RenderState::target_format`）。
    pub fn new(device: &wgpu::Device, display_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let material_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain_material"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let sprite_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        // 两条管线各用单组 layout：wgpu 要求 pass 绑定 pipeline layout 的全部组，
        // 若共用双组 layout 则 terrain pass（只绑组0）与 sprite pass（只绑组1）都会判红。
        let terrain_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&material_bgl],
            push_constant_ranges: &[],
        });
        let sprite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&sprite_bgl],
            push_constant_ranges: &[],
        });

        let terrain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
            layout: Some(&terrain_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32],
                }],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let sprite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprite"),
            layout: Some(&sprite_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sprite"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sprite"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: display_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            terrain_pipeline,
            sprite_pipeline,
            material_bgl,
            sprite_bgl,
            sampler,
            ubuf,
            height: None,
            mesh: None,
            target: Mutex::new(None),
        }
    }

    pub fn set_mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        verts: &[[f32; 3]],
        indices: &[u32],
    ) {
        let vb = make_buffer(
            device,
            queue,
            bytemuck::cast_slice(verts),
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        );
        let ib = make_buffer(
            device,
            queue,
            bytemuck::cast_slice(indices),
            wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        );
        self.mesh = Some((vb, ib, indices.len() as u32));
    }

    /// 上传高度场（行主序 f32，dim×dim）并重建材质 bind group。
    pub fn set_height(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        data: &[f32],
        dim: u32,
    ) {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("height"),
            size: wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            tex_info(&tex),
            bytemuck::cast_slice(data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(dim * 4),
                rows_per_image: Some(dim),
            },
            wgpu::Extent3d {
                width: dim,
                height: dim,
                depth_or_array_layers: 1,
            },
        );
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain_material_bg"),
            layout: &self.material_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.ubuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &tex.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
            ],
        });
        self.height = Some((tex, bg, dim));
    }

    /// 调整离屏目标尺寸（与旧尺寸相同则不动）。
    pub fn resize(&self, device: &wgpu::Device, w: u32, h: u32) {
        let w = w.max(1);
        let h = h.max(1);
        {
            let guard = self.target.lock().unwrap();
            if let Some(t) = guard.as_ref() {
                if t.w == w && t.h == h {
                    return;
                }
            }
        }
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen_color"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = color.create_view(&Default::default());
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen_depth"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&Default::default());
        let sprite_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite_bg"),
            layout: &self.sprite_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        *self.target.lock().unwrap() = Some(Target {
            color,
            view,
            depth,
            depth_view,
            sprite_bg,
            w,
            h,
        });
    }

    /// 渲染地形到自持离屏纹理并提交。资源不齐 ⇒ 不动作（返回 false）。
    pub fn draw_terrain(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: [f32; 16],
        height_scale: f32,
    ) -> bool {
        let (dim, material_bg, count) = match (&self.height, &self.mesh) {
            (Some((_, bg, dim)), Some((_, _, count))) => (*dim, bg.clone(), *count),
            _ => return false,
        };
        let uni = Uniforms {
            view_proj,
            height_scale,
            tex_dim: dim as f32,
            _pad: [0.0; 2],
        };
        queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&uni));
        let tgt_guard = self.target.lock().unwrap();
        let Some(t) = tgt_guard.as_ref() else {
            return false;
        };
        let (vb, ib, _) = self.mesh.as_ref().unwrap();

        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.12,
                            b: 0.16,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.terrain_pipeline);
            rp.set_bind_group(0, &material_bg, &[]);
            rp.set_vertex_buffer(0, vb.slice(..));
            rp.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..count, 0, 0..1);
        }
        queue.submit(std::iter::once(enc.finish()));
        true
    }

    /// 在 egui 主 pass 里把离屏纹理贴到像素矩形 (x,y,w,h)。
    pub fn paint(&self, rpass: &mut wgpu::RenderPass<'static>, x: u32, y: u32, w: u32, h: u32) {
        let guard = self.target.lock().unwrap();
        let Some(t) = guard.as_ref() else { return };
        if t.w < 2 || t.h < 2 {
            return;
        }
        rpass.set_pipeline(&self.sprite_pipeline);
        rpass.set_bind_group(0, &t.sprite_bg, &[]);
        rpass.set_viewport(
            x as f32,
            y as f32,
            w.max(1) as f32,
            h.max(1) as f32,
            0.0,
            1.0,
        );
        rpass.draw(0..3, 0..1);
        // 还原整面 viewport，避免影响后续 egui 绘制
        rpass.set_viewport(0.0, 0.0, t.w as f32, t.h as f32, 0.0, 1.0);
    }

    pub fn target_texture(&self) -> Option<wgpu::Texture> {
        self.target
            .lock()
            .unwrap()
            .as_ref()
            .map(|t| t.color.clone())
    }
}

/// 一次渲染的输入。verts 为 [x,z,drop] 三元组；height 为 tex_dim² 的 f32。
pub struct Frame<'a> {
    pub height: &'a [f32],
    pub tex_dim: u32,
    pub verts: &'a [[f32; 3]],
    pub indices: &'a [u32],
    pub view_proj: [f32; 16],
    pub height_scale: f32,
    pub size: u32,
}

/// 自持 device + 单帧接口的离屏渲染器（C1 golden 用）。
pub struct OffscreenTerrain {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    gpu: Arc<Mutex<TerrainGpu>>,
}

fn request_offscreen_device() -> Option<(Arc<wgpu::Device>, Arc<wgpu::Queue>)> {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .or_else(|| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: None,
            force_fallback_adapter: true,
            ..Default::default()
        }))
    })?;
    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("strata-offscreen"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        },
        None,
    ))
    .ok()?;
    Some((Arc::new(device), Arc::new(queue)))
}

impl OffscreenTerrain {
    /// 无 adapter / device 失败 ⇒ None（不 panic）。
    pub fn new() -> Option<Self> {
        let (device, queue) = request_offscreen_device()?;
        let gpu = TerrainGpu::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        Some(Self {
            device,
            queue,
            gpu: Arc::new(Mutex::new(gpu)),
        })
    }

    /// 渲染并回读 RGBA8（size×size）。
    pub fn render(&self, f: &Frame<'_>) -> Option<Vec<u8>> {
        {
            let mut g = self.gpu.lock().unwrap();
            g.set_mesh(&self.device, &self.queue, f.verts, f.indices);
            g.set_height(&self.device, &self.queue, f.height, f.tex_dim);
            g.resize(&self.device, f.size, f.size);
        }
        if !self.gpu.lock().unwrap().draw_terrain(
            &self.device,
            &self.queue,
            f.view_proj,
            f.height_scale,
        ) {
            return None;
        }
        let color = self.gpu.lock().unwrap().target_texture()?;
        readback_rgba8(&self.device, &self.queue, &color, f.size, f.size)
    }
}

/// RGBA8 纹理回读（行按 COPY_BYTES_PER_ROW_ALIGNMENT 对齐）。
pub fn readback_rgba8(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    color: &wgpu::Texture,
    w: u32,
    h: u32,
) -> Option<Vec<u8>> {
    let row_bytes =
        (w * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row_bytes) * u64::from(h),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex_info(color),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit(std::iter::once(enc.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = device.poll(wgpu::Maintain::Wait);
    // 映射完成回调返回 Result；收到 Err（映射失败/设备丢失）⇒ 回读不可用
    match rx.recv() {
        Ok(Ok(())) => {}
        _ => return None,
    }
    let data = slice.get_mapped_range();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for row in data.chunks(row_bytes as usize) {
        out.extend_from_slice(&row[..(w * 4) as usize]);
    }
    Some(out)
}

/// FNV-1a 64 位哈希：C1 golden 对回读像素做非加密指纹（跨平台一致）。
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
