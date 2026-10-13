//! 离屏地形渲染器：wgpu 24，把 CPU 高度场 + mesh.rs 的顶点/索引画进纹理并回读。
//! 视口(M1c)与 C1 golden 门共用这条 pass。没有 GPU adapter 时 [`OffscreenTerrain::new`]
//! 返回 None，调用方（含测试）据此跳过而非假绿。

#![allow(clippy::must_use_candidate)]

use std::sync::Arc;

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

pub struct OffscreenTerrain {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
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

fn tex_info(texture: &wgpu::Texture) -> wgpu::TexelCopyTextureInfo<'_> {
    wgpu::TexelCopyTextureInfo {
        texture,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
    }
}

impl OffscreenTerrain {
    /// 无 adapter / device 失败 ⇒ None（不 panic）。
    pub fn new() -> Option<Self> {
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
        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
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

        Some(Self {
            device,
            queue,
            pipeline,
            bgl,
        })
    }

    /// 渲染并回读 RGBA8（size×size）。映射失败 ⇒ None。
    pub fn render(&self, f: &Frame<'_>) -> Option<Vec<u8>> {
        let color = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: f.size,
                height: f.size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: f.size,
                height: f.size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let height_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: f.tex_dim,
                height: f.tex_dim,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            tex_info(&height_tex),
            bytemuck::cast_slice(f.height),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(f.tex_dim * 4),
                rows_per_image: Some(f.tex_dim),
            },
            wgpu::Extent3d {
                width: f.tex_dim,
                height: f.tex_dim,
                depth_or_array_layers: 1,
            },
        );

        // 顶点/索引/统一缓冲：create_buffer + write_buffer（写入需 COPY_DST）
        let vbuf = self.make_buffer(
            bytemuck::cast_slice(f.verts),
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        );
        let ibuf = self.make_buffer(
            bytemuck::cast_slice(f.indices),
            wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        );
        let uni = Uniforms {
            view_proj: f.view_proj,
            height_scale: f.height_scale,
            tex_dim: f.tex_dim as f32,
            _pad: [0.0; 2],
        };
        let ubuf = self.make_buffer(
            bytemuck::bytes_of(&uni),
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        );

        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ubuf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(
                        &height_tex.create_view(&wgpu::TextureViewDescriptor::default()),
                    ),
                },
            ],
        });

        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color.create_view(&Default::default()),
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
                    view: &depth.create_view(&Default::default()),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.pipeline);
            rp.set_bind_group(0, &bind, &[]);
            rp.set_vertex_buffer(0, vbuf.slice(..));
            rp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..f.indices.len() as u32, 0, 0..1);
        }

        let row_pad = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let row_bytes = (f.size * 4).div_ceil(row_pad) * row_pad;
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row_bytes) * u64::from(f.size),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            tex_info(&color),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row_bytes),
                    rows_per_image: Some(f.size),
                },
            },
            wgpu::Extent3d {
                width: f.size,
                height: f.size,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(enc.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::Maintain::Wait);
        // 映射完成回调返回 Result；收到 Err（映射失败/设备丢失）⇒ 回读不可用
        match rx.recv() {
            Ok(Ok(())) => {}
            _ => return None,
        }
        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((f.size * f.size * 4) as usize);
        for row in data.chunks_exact(row_bytes as usize) {
            out.extend_from_slice(&row[..(f.size * 4) as usize]);
        }
        Some(out)
    }

    fn make_buffer(&self, contents: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: contents.len() as u64,
            usage,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&buf, 0, contents);
        buf
    }
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
