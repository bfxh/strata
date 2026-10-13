//! Clipmap 流式状态机（可头测：增量判据、同心性、显存账面都在这里钉）。
//!
//! 增量语义（P3 的漫游路径实证）：眼位移动后，**只有锚点变化的层**才重生成高度
//! 并重传纹理；亚-cell 移动 ⇒ 零重生成。计数器 `regen_count/upload_count` 是
//! 可观测判读位，测试直接断言它们。

use strata_nodes::region::NoiseNode;
use strata_render::clipmap::{self, LayerSpec};

/// 渲染米制下的轨道相机（全部 eye-relative 小数，无世界大数进矩阵）。
#[derive(Clone, Copy, Debug)]
pub struct Cam3 {
    pub yaw: f32,
    pub pitch: f32,
    pub dist_m: f32,
}

impl Cam3 {
    pub fn eye_m(&self) -> [f32; 3] {
        let cp = self.pitch.cos();
        [
            self.yaw.sin() * cp * self.dist_m,
            self.pitch.sin() * self.dist_m,
            self.yaw.cos() * cp * self.dist_m,
        ]
    }
}

/// clipmap 场景参数（测试与 UI 共用同一组，避免"演示≠实测"）。
#[derive(Clone, Copy, Debug)]
pub struct SceneParams {
    pub base_cell_mm: i64,
    pub res: u32,
    pub layers: u8,
    pub height_scale_m: f32,
}

impl Default for SceneParams {
    fn default() -> Self {
        Self {
            base_cell_mm: 8_000,
            res: 64,
            layers: 10,
            height_scale_m: 40.0,
        }
    }
}

pub struct ClipmapState {
    params: SceneParams,
    node: NoiseNode,
    eye_x_mm: i64,
    eye_y_mm: i64,
    specs: Vec<LayerSpec>,
    pub heights: Vec<Vec<f32>>,
    dirty: Vec<bool>,
    pending: Vec<usize>,
    pub regen_count: u64,
    pub upload_count: u64,
}

impl ClipmapState {
    pub fn new(params: SceneParams, node: NoiseNode, eye_x_mm: i64, eye_y_mm: i64) -> Self {
        let specs = clipmap::layers_for_eye(
            eye_x_mm,
            eye_y_mm,
            params.base_cell_mm,
            params.res,
            params.layers,
        );
        let mut s = Self {
            params,
            node,
            eye_x_mm,
            eye_y_mm,
            specs,
            heights: Vec::new(),
            dirty: Vec::new(),
            pending: Vec::new(),
            regen_count: 0,
            upload_count: 0,
        };
        s.heights = (0..s.specs.len()).map(|_| Vec::new()).collect();
        s.dirty = vec![true; s.specs.len()];
        s.refresh();
        s
    }

    /// 移动眼位并刷新规格；只有锚点变化的层置 dirty。
    pub fn move_eye(&mut self, x_mm: i64, y_mm: i64) {
        if x_mm == self.eye_x_mm && y_mm == self.eye_y_mm {
            return;
        }
        self.eye_x_mm = x_mm;
        self.eye_y_mm = y_mm;
        let new_specs = clipmap::layers_for_eye(
            x_mm,
            y_mm,
            self.params.base_cell_mm,
            self.params.res,
            self.params.layers,
        );
        for (i, (old, nw)) in self.specs.iter().zip(new_specs.iter()).enumerate() {
            if old != nw {
                self.dirty[i] = true;
            }
        }
        self.specs = new_specs;
        self.refresh();
    }

    /// 生成全部 dirty 层的高度（节点采样是纯函数 ⇒ 确定性）。
    fn refresh(&mut self) {
        for i in 0..self.specs.len() {
            if !self.dirty[i] {
                continue;
            }
            let spec = self.specs[i];
            let res = spec.res;
            let mut data = vec![0.0f32; (res * res) as usize];
            for ky in 0..res {
                for kx in 0..res {
                    let (wx, wy) = spec.world_at(kx, ky);
                    data[(ky * res + kx) as usize] = self.node.sample(wx, wy);
                }
            }
            self.heights[i] = data;
            self.dirty[i] = false;
            self.pending.push(i);
            self.regen_count += 1;
        }
    }

    /// 需要重传到 GPU 的层（取出即清空 pending；每取一层计一次 upload）。
    pub fn take_uploads(&mut self) -> Vec<(usize, Vec<f32>)> {
        let mut out = Vec::new();
        std::mem::swap(&mut out, &mut self.pending);
        self.upload_count += out.len() as u64;
        out.into_iter()
            .map(|i| (i, self.heights[i].clone()))
            .collect()
    }

    pub fn specs(&self) -> &[LayerSpec] {
        &self.specs
    }

    /// 当前世界眼位（毫米）。
    pub fn eye_mm(&self) -> (i64, i64) {
        (self.eye_x_mm, self.eye_y_mm)
    }

    /// 当前显存账面（层高度纹理合计），供 UI/D2 显示与测试断言。
    pub fn texture_bytes(&self) -> usize {
        clipmap::total_texture_bytes(&self.specs)
    }

    /// 由当前状态构造逐层绘制参数。
    pub fn layer_frames(&self, view_proj: [f32; 16]) -> Vec<strata_gpu::renderer::LayerFrame> {
        self.specs
            .iter()
            .map(|s| {
                let span = s.span_mm();
                let ox = s.anchor_x_mm + span / 2 - self.eye_x_mm;
                let oz = s.anchor_y_mm + span / 2 - self.eye_y_mm;
                strata_gpu::renderer::LayerFrame {
                    view_proj,
                    height_scale_m: self.params.height_scale_m,
                    half_span_m: span as f32 / 2_000.0,
                    skirt_scale_m: self.params.height_scale_m,
                    offset_m: [ox as f32 / 1_000.0, oz as f32 / 1_000.0],
                }
            })
            .collect()
    }
}

/// 演示场景的单一真源（app 视口与 C1#2 金标准共用同一定义 ⇒ 测的就是看到的）。
pub fn demo_scene() -> ClipmapState {
    let node = NoiseNode {
        seed: 0x57_0000_0000_0001,
        wavelength_mm: 400_000,
        octaves: 5,
        warp: Some(strata_nodes::noise::Warp {
            amplitude_mm: 250_000,
            wavelength_mm: 1_600_000,
        }),
    };
    ClipmapState::new(SceneParams::default(), node, 25_000_000, -18_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> ClipmapState {
        let node = NoiseNode::new(0xABCD_0000_0000_0042, 400_000, 5);
        ClipmapState::new(SceneParams::default(), node, 1_000_000_000, -700_000_000)
    }

    #[test]
    fn init_uploads_every_layer_once() {
        let s = scene();
        assert_eq!(s.regen_count, SceneParams::default().layers as u64);
        assert_eq!(s.texture_bytes(), 10 * 64 * 64 * 4);
    }

    #[test]
    fn sub_cell_move_regenerates_nothing() {
        // P3 硬判据：1024mm（< base cell 8000mm）的移动不触发任何层重生成
        let mut s = scene();
        let before = s.regen_count;
        let e = (1_000_000_000 + 1_024, -700_000_000);
        s.move_eye(e.0, e.1);
        assert_eq!(s.regen_count, before, "亚-cell 移动不应重生成");
    }

    #[test]
    fn big_move_regenerates_only_dirty_layers() {
        // 12345mm 斜移：外层 cell 大（4096000mm）大概率锚点不动，内层动。
        let mut s = scene();
        let before = s.regen_count;
        s.move_eye(1_000_012_345, -699_987_655);
        let delta = s.regen_count - before;
        assert!(delta >= 1, "该重生成却没动");
        assert!(
            delta < SceneParams::default().layers as u64,
            "全图重烤（delta={delta}）——增量失效"
        );
    }

    #[test]
    fn heights_deterministic_across_states() {
        let a = scene().heights[0].clone();
        let b = scene().heights[0].clone();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.to_bits(), y.to_bits());
        }
    }
}
