//! 节点实现约定 + 首批 CPU 节点。
//!
//! **Parity 规约**（钉死 01-P8）：每个空间节点成对提供 CPU(Rust) 与 WGSL 实现，
//! 同 seed 输入下量化 i16 后逐位一致。WGSL 侧放 `shaders/nodes/<name>.wgsl`，
//! CI 门 B2 对每个在此注册的节点跑 CPU↔GPU 对比。M0 先立 CPU 侧与注册表骨架，
//! noise/erosion 的成对实现在 M1–M2 填充。
#![allow(clippy::must_use_candidate)] // 与 core 同款豁免高噪声风格项

use strata_field::{Field, HeightTile};
use strata_geo::TilePos;

/// 节点求值的 CPU 侧契约。GPU 侧对应 WGSL entrypoint 同名。
pub trait CpuNode {
    /// 稳定名，与 WGSL 文件名一致（parity 键）。
    fn name(&self) -> &'static str;

    /// 依赖半径等元数据由 strata-exec 的边携带；这里只做逐 tile 求值。
    fn eval_height(&self, out_pos: TilePos, span: u32, halo: u32) -> HeightTile;
}

/// 常数高度节点：把整个 tile 填成标量高度。作为 parity 门的最小真值语料。
pub struct Fill {
    pub value: f32,
}

impl CpuNode for Fill {
    fn name(&self) -> &'static str {
        "fill"
    }

    fn eval_height(&self, out_pos: TilePos, span: u32, halo: u32) -> HeightTile {
        HeightTile::new(out_pos, span, halo, self.value)
    }
}

impl Fill {
    /// 统一入口，供执行器按 Field 分派。
    pub fn eval(&self, out_pos: TilePos, span: u32, halo: u32) -> Field {
        Field::Height(self.eval_height(out_pos, span, halo))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tp(x: i64) -> TilePos {
        TilePos { x, y: 0, level: 0 }
    }

    #[test]
    #[allow(clippy::float_cmp)] // 常量填充应逐位精确等于字面量，正是要断言相等
    fn fill_node_matches_cpu_golden() {
        let node = Fill { value: 0.25 };
        let t = node.eval_height(tp(0), 4, 1);
        // 全网格（含 halo）都是 0.25
        assert!(t.data.iter().all(|&v| v == 0.25));
        // 量化后逐位确定的金标准：0.25 * 32767 = 8191.75 → round 8192
        let side = (4 + 2) as usize; // span 4 + 2*halo(1)
        assert_eq!(t.quantize_i16(), vec![8192i16; side * side]);
    }

    #[test]
    fn fill_quantize_boundary_symmetry() {
        for v in [-0.5, 0.5] {
            let t = Fill { value: v }.eval_height(tp(0), 1, 0);
            let q = t.quantize_i16()[0];
            assert_eq!(q.abs(), 16384);
        }
    }
}
