//! 应用壳（组装层，不放逻辑，见 03）。M1d：10 层 clipmap 流式地形进中央视口；
//! WASD 以世界 i64 毫米位移漫游（锚点量化保证渲染局部 float 有界），拖拽轨道、滚轮推拉。

use std::sync::{Arc, Mutex};

use eframe::egui;

use strata_app::clipmap_state::{Cam3, ClipmapState, SceneParams};
use strata_exec::{DepRadius, Edge, Graph, NodeId};
use strata_geo::DEFAULT_TILE_SPAN_PX;
use strata_gpu::math::Mat4;
use strata_gpu::renderer::TerrainGpu;
use strata_nodes::region::NoiseNode;
use strata_nodes::Fill;
use strata_render::clipmap::total_texture_bytes;
use strata_render::clipmap_layer_count;
use strata_render::mesh::TerrainMesh;

const MESH_N: u32 = 64; // 与 clipmap res 对齐的正则网格

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
    if let Some(idx) = args.iter().position(|a| a == "--bench-render") {
        let frames: u32 = args
            .get(idx + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(120);
        return match strata_app::bench::run_render_bench(frames) {
            Some(rep) => {
                rep.print();
                Ok(())
            }
            None => {
                println!("BENCH SKIP: 本机无可用 GPU adapter");
                std::process::exit(2);
            }
        };
    }
    if args.iter().any(|a| a == "--bench-erosion") {
        let num = |flag: &str, dflt: u32| -> u32 {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.parse().ok())
                .unwrap_or(dflt)
        };
        let grid_n = num("--grid", 1024);
        let spf = num("--steps-per-frame", 8);
        let frames = num("--frames", 60);
        return match strata_app::bench::run_erosion_bench(grid_n, spf, frames) {
            Some(rep) => {
                rep.print();
                Ok(())
            }
            None => {
                println!("EROSION BENCH SKIP: 本机无可用 GPU adapter 或无 SHADER_INT64");
                std::process::exit(2);
            }
        };
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

/// 演示用的最小图（M3 起由真实 DAG 替换）。
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
        if let Ok(guard) = self.shared.gpu.lock() {
            guard.paint(rpass, x, y, w, h);
        }
    }
}

struct StrataApp {
    graph: Graph,
    fill_value: f32,
    view_range_mm: i64,
    view: Option<Arc<ViewState>>,
    view_error: Option<String>,
    mesh_verts: Vec<[f32; 3]>,
    mesh_idx: Vec<u32>,
    clip: ClipmapState,
    cam: Cam3,
    /// 世界眼位（i64 毫米）——漫游只动它，渲染永远用相对量
    eye_x_mm: i64,
    eye_y_mm: i64,
}

impl StrataApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mesh = TerrainMesh::new(MESH_N, 0.02);
        let params = SceneParams::default();
        let clip = strata_app::clipmap_state::demo_scene();
        let (eye_x, eye_y) = clip.eye_mm();
        let mut app = Self {
            graph: seed_graph(),
            fill_value: 0.25,
            view_range_mm: 100_000,
            view: None,
            view_error: None,
            mesh_verts: mesh.vertices(),
            mesh_idx: mesh.indices(),
            clip,
            cam: Cam3 {
                yaw: 0.6,
                pitch: 0.45,
                dist_m: 900.0,
            },
            eye_x_mm: eye_x,
            eye_y_mm: eye_y,
        };
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            let mut gpu = TerrainGpu::new(&rs.device, rs.target_format);
            gpu.set_mesh(&rs.device, &rs.queue, &app.mesh_verts, &app.mesh_idx);
            gpu.configure_layers(&rs.device, params.layers as usize, params.res);
            gpu.resize(&rs.device, 1024, 768);
            let shared = Arc::new(ViewState {
                device: rs.device.clone(),
                queue: rs.queue.clone(),
                gpu: Mutex::new(gpu),
            });
            app.view = Some(shared);
        } else {
            app.view_error = Some("egui 未启用 wgpu 渲染后端".to_string());
        }
        app
    }

    /// 世界漫游：方向由 yaw 定，步长直接加在世界 i64 毫米上（精确）。
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
        if resp.dragged_by(egui::PointerButton::Primary) {
            let dd = resp.drag_delta();
            self.cam.yaw -= dd.x * 0.005;
            self.cam.pitch = (self.cam.pitch + dd.y * 0.005).clamp(-0.2, 1.45);
            ui.ctx().request_repaint();
        }
        let speed_mm: f32 = 6_000.0; // 6m/帧
        let (sy, cy) = (self.cam.yaw.sin(), self.cam.yaw.cos());
        if up || down || a || d {
            if up {
                self.eye_x_mm += (sy * speed_mm) as i64;
                self.eye_y_mm += (cy * speed_mm) as i64;
            }
            if down {
                self.eye_x_mm -= (sy * speed_mm) as i64;
                self.eye_y_mm -= (cy * speed_mm) as i64;
            }
            if a {
                self.eye_x_mm -= (cy * speed_mm) as i64;
                self.eye_y_mm += (sy * speed_mm) as i64;
            }
            if d {
                self.eye_x_mm += (cy * speed_mm) as i64;
                self.eye_y_mm -= (sy * speed_mm) as i64;
            }
            ui.ctx().request_repaint();
        }
        if scroll.abs() > 0.1 {
            self.cam.dist_m = (self.cam.dist_m * (1.0 - scroll * 0.001)).clamp(20.0, 200_000.0);
            ui.ctx().request_repaint();
        }
    }

    /// 一帧：流式增量 → 层参数 → GPU。
    fn render_frame(&mut self, w_px: u32, h_px: u32, aspect: f32) {
        let Some(shared) = &self.view else { return };
        self.clip.move_eye(self.eye_x_mm, self.eye_y_mm);
        let uploads = self.clip.take_uploads();
        {
            let g = shared.gpu.lock().unwrap();
            g.resize(&shared.device, w_px, h_px);
            for (idx, data) in &uploads {
                g.upload_layer(&shared.queue, *idx, data);
            }
        }
        let eye = self.cam.eye_m();
        let vp = Mat4::perspective_rh(std::f32::consts::FRAC_PI_4, aspect.max(0.01), 0.5, 90_000.0)
            .mul(&Mat4::look_at_rh(eye, [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]));
        let frames = self.clip.layer_frames(vp.0);
        let _ = shared
            .gpu
            .lock()
            .unwrap()
            .draw_layers(&shared.device, &shared.queue, &frames);
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
                    ui.label(format!(
                        "clipmap {} 层 | 显存 {} KB",
                        self.clip.specs().len(),
                        total_texture_bytes(self.clip.specs()) / 1024
                    ));
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
            ui.label(format!(
                "世界眼位 ({}mm, {}mm)",
                self.eye_x_mm, self.eye_y_mm
            ));
            ui.label(format!(
                "增量重生成 {} 次 / 重传 {} 层次",
                self.clip.regen_count, self.clip.upload_count
            ));
            ui.separator();
            ui.label("漫游: WASD | 旋转: 左键拖拽 | 推拉: 滚轮");
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
                ui.centered_and_justified(|ui| ui.heading("视口（无 GPU 或渲染面未就绪）"));
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
