//! 应用壳（组装层，不放逻辑，见 03）。M1c：wgpu 地形经 paint callback 进中央视口，
//! 鼠标拖拽轨道旋转、滚轮缩放、WASD 平移。磨砂玻璃与节点图在 M3 落地。

use std::sync::{Arc, Mutex};

use eframe::egui;

use strata_exec::{DepRadius, Edge, Graph, NodeId};
use strata_geo::DEFAULT_TILE_SPAN_PX;
use strata_gpu::math::Mat4;
use strata_gpu::renderer::TerrainGpu;
use strata_nodes::region::NoiseNode;
use strata_nodes::Fill;
use strata_render::clipmap_layer_count;
use strata_render::mesh::TerrainMesh;

const HEIGHT_DIM: u32 = 512; // 单层 512² 高度纹理（M1a 的 CPU 生成核喂入）
const MESH_N: u32 = 256; // 256² 顶点网格

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--probe-gpu") {
        match strata_gpu::probe::probe() {
            Some((backend, name)) => {
                println!("GPU 可用: backend={backend} adapter={name}");
                std::process::exit(0);
            }
            None => {
                println!("GPU 不可用（无 adapter，含 software 回退）");
                std::process::exit(1);
            }
        }
    }
    if let Some(path) = args
        .iter()
        .position(|a| a == "--generate")
        .and_then(|i| args.get(i + 1))
    {
        return headless_generate(path);
    }
    let opts = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Strata — 地形工作台 (M1)"),
        ..Default::default()
    };
    eframe::run_native(
        "strata",
        opts,
        Box::new(|cc| Ok(Box::new(StrataApp::new(cc)))),
    )
}

/// 演示用的最小图：一个 fill 节点。M3 起换成真实 DAG。
fn seed_graph() -> Graph {
    let mut g = Graph::default();
    g.add_node(NodeId(0));
    let _ = DepRadius::Local;
    let _ = Edge {
        from: NodeId(0),
        to: NodeId(0),
        radius: DepRadius::Local,
    };
    g
}

/// GPU 视口共享状态（egui 的 device/queue 上建管线；回调与 App 各持 Arc）。
struct ViewState {
    device: wgpu::Device,
    queue: wgpu::Queue,
    gpu: Mutex<TerrainGpu>,
}

/// 相机：绕 target 的球面轨道（RTE 语义下全部是局部量，无世界大数）。
#[derive(Clone, Copy)]
struct Camera {
    yaw: f32,
    pitch: f32,
    dist: f32,
    target: [f32; 3],
}

impl Camera {
    fn eye(&self) -> [f32; 3] {
        let cp = self.pitch.cos();
        [
            self.target[0] + self.yaw.sin() * cp * self.dist,
            self.target[1] + self.pitch.sin() * self.dist,
            self.target[2] + self.yaw.cos() * cp * self.dist,
        ]
    }
}

/// paint callback：把离屏地形纹理贴到本回调的视口矩形。
struct TerrainCallback {
    shared: Arc<ViewState>,
}

impl egui_wgpu::CallbackTrait for TerrainCallback {
    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        rpass: &mut wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        let vp = info.viewport_in_pixels();
        let x = vp.left_px.max(0) as u32;
        let y = vp.top_px.max(0) as u32;
        let w = vp.width_px.max(0) as u32;
        let h = (vp.from_bottom_px - vp.top_px).max(0) as u32;
        let guard = match self.shared.gpu.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        guard.paint(rpass, x, y, w, h);
    }
}

struct StrataApp {
    graph: Graph,
    fill_value: f32,
    view_range_mm: i64,
    view: Option<Arc<ViewState>>,
    view_error: Option<String>,
    cam: Camera,
    mesh_verts: Vec<[f32; 3]>,
    mesh_idx: Vec<u32>,
    heights: Vec<f32>,
}

impl StrataApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mesh = TerrainMesh::new(MESH_N, 0.02);
        let node = NoiseNode {
            seed: 0x57_0000_0000_0001,
            wavelength_mm: 64,
            octaves: 5,
            warp: Some(strata_nodes::noise::Warp {
                amplitude_mm: 48,
                wavelength_mm: 256,
            }),
        };
        let tile = strata_nodes::CpuNode::eval_height(
            &node,
            strata_geo::TilePos {
                x: 0,
                y: 0,
                level: 0,
            },
            HEIGHT_DIM,
            0,
            DEFAULT_TILE_SPAN_PX,
        );
        let heights = tile.data;

        let mut app = Self {
            graph: seed_graph(),
            fill_value: 0.25,
            view_range_mm: 100_000,
            view: None,
            view_error: None,
            cam: Camera {
                yaw: 0.6,
                pitch: 0.5,
                dist: 2.4,
                target: [0.0, 0.0, 0.0],
            },
            mesh_verts: mesh.vertices(),
            mesh_idx: mesh.indices(),
            heights,
        };
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let gpu = TerrainGpu::new(&rs.device, rs.target_format);
            let shared = Arc::new(ViewState {
                device: rs.device.clone(),
                queue: rs.queue.clone(),
                gpu: Mutex::new(gpu),
            });
            {
                let mut g = shared.gpu.lock().unwrap();
                g.set_mesh(
                    &shared.device,
                    &shared.queue,
                    &app.mesh_verts,
                    &app.mesh_idx,
                );
                g.set_height(&shared.device, &shared.queue, &app.heights, HEIGHT_DIM);
                g.resize(&shared.device, 1024, 768);
            }
            app.view = Some(shared);
        } else {
            app.view_error = Some("egui 未启用 wgpu 渲染后端".to_string());
        }
        app
    }

    fn handle_input(&mut self, ui: &egui::Ui, resp: &egui::Response) {
        let (a, d, up, down, scroll) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::A),
                i.key_pressed(egui::Key::D),
                i.key_pressed(egui::Key::W) || i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::S) || i.key_pressed(egui::Key::ArrowDown),
                i.raw_scroll_delta.y,
            )
        });
        // 拖拽旋转：egui 自己维护 delta（dragged_by），不借 memory 存上一帧
        if resp.dragged_by(egui::PointerButton::Primary) {
            let d = resp.drag_delta();
            self.cam.yaw -= d.x * 0.005;
            self.cam.pitch = (self.cam.pitch + d.y * 0.005).clamp(0.05, 1.45);
            ui.ctx().request_repaint();
        }
        let speed = 0.08;
        if a {
            self.cam.target[0] -= speed;
        }
        if d {
            self.cam.target[0] += speed;
        }
        if up {
            self.cam.target[2] -= speed;
        }
        if down {
            self.cam.target[2] += speed;
        }
        if a || d || up || down {
            ui.ctx().request_repaint();
        }
        if scroll.abs() > 0.1 {
            self.cam.dist = (self.cam.dist * (1.0 - scroll * 0.001)).clamp(0.4, 20.0);
            ui.ctx().request_repaint();
        }
    }

    fn render_frame(&self, w_px: u32, h_px: u32, aspect: f32) {
        let Some(shared) = &self.view else { return };
        {
            let g = shared.gpu.lock().unwrap();
            g.resize(&shared.device, w_px, h_px);
        }
        let eye = self.cam.eye();
        let vp = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect.max(0.01), 0.05, 50.0)
            .mul(&Mat4::look_at_rh(eye, self.cam.target, [0.0, 1.0, 0.0]));
        let ok = shared
            .gpu
            .lock()
            .unwrap()
            .draw_terrain(&shared.device, &shared.queue, vp.0, 0.45);
        if !ok {
            // 资源缺失（如无 GPU）：视口保持上一次内容，M2 起在 UI 上显式提示
        }
    }
}

impl eframe::App for StrataApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Strata");
                ui.separator();
                ui.label(format!("nodes: {}", self.graph.node_count()));
                if self.view.is_some() {
                    ui.label("视口: GPU 地形");
                } else if let Some(err) = &self.view_error {
                    ui.colored_label(egui::Color32::YELLOW, format!("视口降级: {err}"));
                }
            });
        });

        egui::SidePanel::right("inspector").show(ctx, |ui| {
            ui.heading("检视器");
            ui.add(egui::Slider::new(&mut self.fill_value, 0.0..=1.0).text("fill 高度"));
            let _ = Fill {
                value: self.fill_value,
            };
            ui.separator();
            let layers = clipmap_layer_count(self.view_range_mm, DEFAULT_TILE_SPAN_PX, 12);
            ui.label(format!(
                "视距 {}mm → clipmap {} 层",
                self.view_range_mm, layers
            ));
            ui.separator();
            ui.label("相机: 拖拽旋转 / WASD 平移 / 滚轮缩放");
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());
            self.handle_input(ui, &resp);
            let ppp = ctx.pixels_per_point();
            let (w_px, h_px) = ((rect.width() * ppp) as u32, (rect.height() * ppp) as u32);
            if w_px > 1 && h_px > 1 {
                self.render_frame(w_px, h_px, rect.width() / rect.height().max(1e-3));
            }
            if let Some(shared) = &self.view {
                ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    TerrainCallback {
                        shared: shared.clone(),
                    },
                ));
            } else {
                ui.centered_and_justified(|ui| {
                    ui.heading("视口（无 GPU 或渲染面未就绪）");
                });
            }
        });
    }
}

/// 无头生成：4×4 个 512² tile 拼成 2048² 高度场，落 `.sh16`（确定性 i16）。
fn headless_generate(path: &str) -> eframe::Result<()> {
    use strata_field::HeightTile;
    use strata_geo::TilePos;
    use strata_nodes::CpuNode;
    const SPAN: u32 = 512;
    const HALO: u32 = 8;
    const GRID: u32 = 4;
    let node = NoiseNode::new(0x57_0000_0000_0001, 4096, 5);
    let mut tiles: Vec<Vec<HeightTile>> = Vec::new();
    for ty in 0..GRID {
        let mut row = Vec::new();
        for tx in 0..GRID {
            let pos = TilePos {
                x: i64::from(tx),
                y: i64::from(ty),
                level: 0,
            };
            row.push(node.eval_height(pos, SPAN, HALO, DEFAULT_TILE_SPAN_PX));
        }
        tiles.push(row);
    }
    let side = SPAN * GRID;
    let mut big = HeightTile::new(
        TilePos {
            x: 0,
            y: 0,
            level: 0,
        },
        side,
        0,
        0.0,
    );
    for ty in 0..GRID {
        for tx in 0..GRID {
            let t = &tiles[ty as usize][tx as usize];
            for py in 0..SPAN {
                for px in 0..SPAN {
                    let v = t.get(px + HALO, py + HALO);
                    big.set(tx * SPAN + px, ty * SPAN + py, v);
                }
            }
        }
    }
    let bytes = strata_io::write_height_i16_le(&big);
    if let Err(e) = std::fs::write(path, &bytes) {
        eprintln!("写入 {path} 失败: {e}");
        std::process::exit(1);
    }
    eprintln!(
        "已生成 {side}×{side} 高度场（{} 次 tile 求算）→ {path} ({} 字节)",
        GRID * GRID,
        bytes.len()
    );
    Ok(())
}
