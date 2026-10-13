//! 应用壳（组装层，不放逻辑，见 03）。M0 目标：开出一个 wgpu+egui 空窗口并立起
//! 三段式骨架（视口 / 右侧检视器 / 底部抽屉），磨砂玻璃与节点图在 M3 落地。

use eframe::egui;

use strata_exec::{DepRadius, Edge, Graph, NodeId};
use strata_field::HeightTile;
use strata_geo::{TilePos, DEFAULT_TILE_SPAN_PX};
use strata_nodes::region::NoiseNode;
use strata_nodes::{CpuNode, Fill};
use strata_render::clipmap_layer_count;

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
        // wgpu 后端（06/03：DX12/Vulkan/Metal/GL 四后端由 wgpu 择一）
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Strata — 地形工作台 (M0)"),
        ..Default::default()
    };
    eframe::run_native(
        "strata",
        opts,
        Box::new(|cc| Ok(Box::new(StrataApp::new(cc)))),
    )
}

/// 无头生成：4×4 个 512² tile 拼成 2048² 高度场，落 `.sh16`（strata-io 确定性 i16）。
/// 不需要 GPU —— 用来坐实"能生成 + 位确定 + 增量生效"三件事（07 M1 验收的 CPU 侧）。
fn headless_generate(path: &str) -> eframe::Result<()> {
    const SPAN: u32 = 512;
    const HALO: u32 = 8;
    const GRID: u32 = 4;
    let node = NoiseNode::new(0x57_0000_0000_0001, 4096, 5);
    let mut tiles: Vec<Vec<HeightTile>> = Vec::new();
    let mut eval_calls = 0u64;
    for ty in 0..GRID {
        let mut row = Vec::new();
        for tx in 0..GRID {
            let pos = TilePos {
                x: i64::from(tx),
                y: i64::from(ty),
                level: 0,
            };
            row.push(node.eval_height(pos, SPAN, HALO, DEFAULT_TILE_SPAN_PX));
            eval_calls += 1;
        }
        tiles.push(row);
    }
    // 拼装去 halo
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
        "已生成 {}×{} 高度场（{} 次 tile 求算）→ {} ({} 字节)",
        side,
        side,
        eval_calls,
        path,
        bytes.len()
    );
    Ok(())
}

/// 演示用的最小图：一个 fill 节点。M1 起换成真实 DAG。
fn seed_graph() -> Graph {
    let mut g = Graph::default();
    g.add_node(NodeId(0));
    // 自环会被拒；这里只放单节点证明装配层能触到内核类型
    let _ = DepRadius::Local;
    let _ = Edge {
        from: NodeId(0),
        to: NodeId(0),
        radius: DepRadius::Local,
    };
    g
}

struct StrataApp {
    graph: Graph,
    fill_value: f32,
    view_range_mm: i64,
}

impl StrataApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            graph: seed_graph(),
            fill_value: 0.25,
            view_range_mm: 100_000,
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
            });
        });

        egui::SidePanel::right("inspector").show(ctx, |ui| {
            ui.heading("检视器");
            ui.add(egui::Slider::new(&mut self.fill_value, 0.0..=1.0).text("fill 高度"));
            // 参数变更即演示 dirty 传播入口（M1 接执行器）
            let _ = Fill {
                value: self.fill_value,
            };
            ui.separator();
            let layers =
                clipmap_layer_count(self.view_range_mm, strata_geo::DEFAULT_TILE_SPAN_PX, 12);
            ui.label(format!(
                "视距 {}mm → clipmap {} 层",
                self.view_range_mm, layers
            ));
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            // M0 空视口占位；M1 此处替换为 wgpu clipmap 渲染画布
            ui.centered_and_justified(|ui| {
                ui.heading("视口（M1 接入地形渲染）");
            });
        });
    }
}
