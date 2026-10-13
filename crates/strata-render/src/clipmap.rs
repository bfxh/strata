//! Clipmap 层规格纯数学（P4 抖动有界的**可证部分** + P1 显存账 + P3 增量判据）。
//!
//! 关键不变量（全部单测钉死）：
//! 1. 锚点由 floor-div 量化到 `cell` 的整数倍 ⇒ 眼位相对锚点偏移 `local < span`，
//!    世界坐标再大也被数学上界钉死（抖动有界不靠运气）。
//! 2. 层 i 恰好覆盖层 i+1 的中心区域（cell 翻倍、span 翻倍、近似同心）。
//! 3. 锚点不变 ⇒ 高度数据不变 ⇒ 不需要重生成/重传（增量重算的判读位）。

#![allow(clippy::must_use_candidate)]

use strata_geo::floor_div;

/// 一层的网格规格。世界单位 = 毫米（i64，精确）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerSpec {
    /// 网格角点的世界 x（毫米），`cell_mm` 的整数倍量化
    pub anchor_x_mm: i64,
    pub anchor_y_mm: i64,
    /// 网格步长（毫米/顶点）
    pub cell_mm: i64,
    /// 单边顶点数（纹理边长）
    pub res: u32,
    pub level: u8,
}

impl LayerSpec {
    /// 网格覆盖的世界跨度（毫米）：res 个顶点跨 (res-1) 个 cell。
    pub fn span_mm(&self) -> i64 {
        self.cell_mm * (self.res as i64 - 1)
    }

    /// 第 `k` 个顶点对应的世界坐标（从锚点起 k*cell，全程 i64 精确）。
    pub fn world_at(&self, kx: u32, ky: u32) -> (i64, i64) {
        (
            self.anchor_x_mm + i64::from(kx) * self.cell_mm,
            self.anchor_y_mm + i64::from(ky) * self.cell_mm,
        )
    }

    /// 眼位相对锚点的最大可能偏移（= span；渲染用的局部 f32 都被这个界约束）。
    pub fn max_local_mm(&self) -> i64 {
        self.span_mm()
    }
}

/// 以 eye 为中心建 `layers` 层规格：层 0 用 `base_cell_mm`，逐层 cell×2。
///
/// 锚点公式（标准 clipmap）：`ax = floor((eye - span/2) / cell) * cell`。
/// ⇒ 眼位相对锚点偏移 `local = eye - ax ∈ [span/2, span/2 + cell)`，
/// 因 `cell ≤ span/2`（res≥3）故 **local < span**，渲染局部 f32 被 span 一致钉住。
/// 锚点落在 `cell` 网格上 ⇒ 眼每移动一个 cell 锚点才跳一格（P3 条带更新基础）。
pub fn layers_for_eye(
    eye_x_mm: i64,
    eye_y_mm: i64,
    base_cell_mm: i64,
    res: u32,
    layers: u8,
) -> Vec<LayerSpec> {
    assert!(base_cell_mm > 0 && res >= 3 && layers >= 1);
    let mut out = Vec::with_capacity(layers as usize);
    let mut cell = base_cell_mm;
    for level in 0..layers {
        let span = cell * (res as i64 - 1);
        let half = span / 2;
        let ax = floor_div(eye_x_mm - half, cell) * cell;
        let ay = floor_div(eye_y_mm - half, cell) * cell;
        out.push(LayerSpec {
            anchor_x_mm: ax,
            anchor_y_mm: ay,
            cell_mm: cell,
            res,
            level,
        });
        cell *= 2;
    }
    out
}

/// 层高度纹理的显存字节（R32Float，res²×4B）。P1/D2 的账面判据。
pub fn layer_texture_bytes(spec: &LayerSpec) -> usize {
    (spec.res as usize) * (spec.res as usize) * core::mem::size_of::<f32>()
}

/// 全部层的显存合计。
pub fn total_texture_bytes(specs: &[LayerSpec]) -> usize {
    specs.iter().map(layer_texture_bytes).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RES: u32 = 64;
    const CELL0: i64 = 8;

    fn specs(eye: i64) -> Vec<LayerSpec> {
        layers_for_eye(eye, eye, CELL0, RES, 10)
    }

    #[test]
    fn anchors_quantized_so_local_offset_is_bounded() {
        // P4 根证据：世界坐标再大，眼位相对锚点的偏移也被 span 钉死（f32 安全域）
        for eye in [0i64, 999, -12345678901234, i64::MAX / 4] {
            for ls in specs(eye) {
                let local = eye - ls.anchor_x_mm;
                assert!(
                    local >= 0 && local < ls.span_mm(),
                    "level {}: local {local} 越出 [0,{})",
                    ls.level,
                    ls.span_mm()
                );
                // 锚点严格落在 cell 网格上（量化成立，条带更新的前提）
                assert_eq!(ls.anchor_x_mm.rem_euclid(ls.cell_mm), 0);
            }
        }
    }

    #[test]
    fn sub_cell_move_keeps_anchor_full_cell_move_shifts_by_exactly_cell() {
        let start = 1_000_000i64;
        let a = specs(start);
        // P3 增量判据：每层亚-cell 位移(+1mm)若未跨边界则锚点不变；跨边界则恰好移一个 cell。
        // 统一用"位移一个完整 cell ⇒ 该层锚点正好 +cell"这条与起点无关的硬不变量。
        for lvl in 0..a.len() {
            let cell = CELL0 << lvl;
            let moved = specs(start + cell);
            assert_eq!(
                moved[lvl].anchor_x_mm - a[lvl].anchor_x_mm,
                cell,
                "level {lvl}: 整-cell 位移应使锚点正好 +{cell}"
            );
            // 该层锚点仍落在 cell 网格
            assert_eq!(moved[lvl].anchor_x_mm.rem_euclid(cell), 0);
        }
        // 反例护栏：证明上面不是"锚点恒等"的假绿——位移 1mm 时各层锚点要么不变要么 +cell，绝无其它值
        let one = specs(start + 1);
        for lvl in 0..a.len() {
            let d = one[lvl].anchor_x_mm - a[lvl].anchor_x_mm;
            assert!(
                d == 0 || d == CELL0 << lvl,
                "level {lvl}: 1mm 位移锚点跳变异常 d={d}"
            );
        }
    }

    #[test]
    fn nested_layers_are_concentric_and_doubling() {
        let ss = specs(500_000);
        assert_eq!(ss.len(), 10);
        for w in ss.windows(2) {
            assert_eq!(w[1].cell_mm, w[0].cell_mm * 2);
            assert_eq!(w[1].span_mm(), w[0].span_mm() * 2);
            // 近似同心：层中心偏差 ≤ 各自半 cell 之和
            let c0 = w[0].anchor_x_mm + w[0].span_mm() / 2;
            let c1 = w[1].anchor_x_mm + w[1].span_mm() / 2;
            assert!((c0 - c1).abs() <= (w[0].cell_mm + w[1].cell_mm) / 2);
        }
    }

    #[test]
    fn world_at_matches_anchor_math() {
        let ss = specs(12345);
        let l3 = &ss[3];
        let (x, y) = l3.world_at(10, 20);
        assert_eq!(x, l3.anchor_x_mm + 10 * l3.cell_mm);
        assert_eq!(y, l3.anchor_y_mm + 20 * l3.cell_mm);
        // 顶点覆盖恰为 [anchor, anchor+span]（角点对角线）
        let (ex, ey) = l3.world_at(l3.res - 1, l3.res - 1);
        assert_eq!(ex - l3.anchor_x_mm, l3.span_mm());
        assert_eq!(ey - l3.anchor_y_mm, l3.span_mm());
    }

    #[test]
    fn texture_budget_math() {
        let ss = specs(0);
        // 单层 64² f32 = 16KB
        assert_eq!(layer_texture_bytes(&ss[0]), 64 * 64 * 4);
        // 10 层合计 = 160KB << 512MB 预算（账面先钉住）
        assert_eq!(total_texture_bytes(&ss), 10 * 16384);
        assert!(total_texture_bytes(&ss) < 512 * 1024 * 1024);
    }

    #[test]
    fn layers_deterministic_across_calls() {
        assert_eq!(specs(777_777_777), specs(777_777_777));
    }
}
