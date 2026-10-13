//! 应用壳（组装层，不放逻辑，见 03）。M0 目标：开出一个 wgpu+egui 空窗口并立起
//! 三段式骨架（视口 / 右侧检视器 / 底部抽屉），磨砂玻璃与节点图在 M3 落地。

use eframe::egui;

use strata_exec::{DepRadius, Edge, Graph, NodeId};
use strata_nodes::Fill;
use strata_render::clipmap_layer_count;

fn main() -> eframe::Result<()> {
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
