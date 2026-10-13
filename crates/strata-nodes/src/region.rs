//! 区域级 tile 生成与增量重算（01-P3 的 CPU 侧证据 + P5 数据侧无缝证据）。
//!
//! [`RegionCache`] 只对 dirty tile 求值；跨 tile 的 halo 采样保证相邻 tile 在
//! 重叠列上**逐位一致**（无缝），这是"改一格不炸全图"的最小闭环。

#![allow(clippy::must_use_candidate)] // 与 core 同款豁免高噪声风格项

use std::collections::{HashMap, HashSet};

use strata_field::HeightTile;
use strata_geo::TilePos;

use crate::noise::{self, Warp};
use crate::CpuNode;

/// fBm 高度节点。波长/幅度均为整数毫米 ⇒ 位确定（见 noise 模块纪律）。
#[derive(Clone, Debug, PartialEq)]
pub struct NoiseNode {
    pub seed: u64,
    pub wavelength_mm: i64,
    pub octaves: u8,
    pub warp: Option<Warp>,
}

impl NoiseNode {
    /// 无 warp 的便捷构造（headless 生成、预设用）。
    pub fn new(seed: u64, wavelength_mm: i64, octaves: u8) -> Self {
        Self {
            seed,
            wavelength_mm,
            octaves,
            warp: None,
        }
    }

    pub fn sample(&self, world_x: i64, world_y: i64) -> f32 {
        noise::warped_fbm(
            world_x,
            world_y,
            self.wavelength_mm,
            self.octaves,
            self.seed,
            self.warp,
        )
    }
}

impl CpuNode for NoiseNode {
    fn name(&self) -> &'static str {
        "noise.fbm"
    }

    fn eval_height(&self, out_pos: TilePos, span: u32, halo: u32, base_px: u32) -> HeightTile {
        let (ox, oy) = out_pos.world_origin(base_px);
        let span_i = i64::from(span);
        // 每像素世界步长（毫米）。M1 约束：span 必须整除 tile 跨度。
        let step = out_pos.span_mm(base_px).div_euclid(span_i);
        let side = span + 2 * halo;
        let mut t = HeightTile::new(out_pos, span, halo, 0.0);
        for py in 0..side {
            for px in 0..side {
                let wx = ox + i64::from(px) * step - i64::from(halo) * step;
                let wy = oy + i64::from(py) * step - i64::from(halo) * step;
                t.set(px, py, self.sample(wx, wy));
            }
        }
        t
    }
}

/// 区域 tile 缓存：`ensure` 只补 dirty/缺失 tile，命中零成本。
pub struct RegionCache {
    span: u32,
    halo: u32,
    base_px: u32,
    tiles: HashMap<TilePos, HeightTile>,
    /// 重算计数：D1/E1 门可拿它当"增量真生效"的可观测判据
    pub eval_calls: u64,
}

impl RegionCache {
    pub fn new(span: u32, halo: u32, base_px: u32) -> Self {
        Self {
            span,
            halo,
            base_px,
            tiles: HashMap::new(),
            eval_calls: 0,
        }
    }

    /// 保证集合内 tile 有效；只重算缺失或已失效的。
    pub fn ensure(&mut self, node: &dyn CpuNode, wanted: &HashSet<TilePos>) {
        for pos in wanted {
            if self.tiles.contains_key(pos) {
                continue;
            }
            self.eval_calls += 1;
            let t = node.eval_height(*pos, self.span, self.halo, self.base_px);
            self.tiles.insert(*pos, t);
        }
    }

    /// 参数变更 ⇒ 失效指定 tile。
    pub fn invalidate(&mut self, tiles: &HashSet<TilePos>) {
        for pos in tiles {
            self.tiles.remove(pos);
        }
    }

    pub fn tile(&self, pos: TilePos) -> Option<&HeightTile> {
        self.tiles.get(&pos)
    }

    pub fn assemble(&self, grid: (u32, u32), origin: TilePos) -> Option<HeightTile> {
        let (gx, gy) = grid;
        let side = gx * self.span;
        let mut out = HeightTile::new(
            TilePos {
                x: origin.x,
                y: origin.y,
                level: origin.level,
            },
            side,
            0,
            0.0,
        );
        for ty in 0..gy {
            for tx in 0..gx {
                let pos = TilePos {
                    x: origin.x + i64::from(tx),
                    y: origin.y + i64::from(ty),
                    level: origin.level,
                };
                let t = self.tiles.get(&pos)?;
                for py in 0..self.span {
                    for px in 0..self.span {
                        let v = t.get(px + self.halo, py + self.halo);
                        out.set(tx * self.span + px, ty * self.span + py, v);
                    }
                }
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tp(x: i64, y: i64) -> TilePos {
        TilePos { x, y, level: 0 }
    }

    fn node() -> NoiseNode {
        NoiseNode {
            seed: 42,
            wavelength_mm: 97,
            octaves: 3,
            warp: None,
        }
    }

    const SPAN: u32 = 32;
    const HALO: u32 = 4;
    const BASE: u32 = 32; // 测试用小 tile：base_px == span

    #[test]
    fn halo_columns_match_neighbor_bitwise() {
        // P5 数据侧：A 内容右端 h 列（去掉 A 的 halo 偏移）与 B 左 halo 同世界坐标 ⇒ 逐位一致
        let n = node();
        let a = n.eval_height(tp(0, 0), SPAN, HALO, BASE);
        let b = n.eval_height(tp(1, 0), SPAN, HALO, BASE);
        for h in 0..HALO {
            for py in HALO..HALO + SPAN {
                // A index SPAN+h ⇒ world (SPAN+h)-HALO = 右内容末端起 h；B index h ⇒ world Bx+h-HALO = 同值
                let va = a.get(SPAN + h, py);
                let vb = b.get(h, py);
                assert_eq!(va.to_bits(), vb.to_bits(), "halo col {h} row {py}");
            }
        }
    }

    #[test]
    fn ensure_recomputes_only_dirty_tiles() {
        let n = node();
        let mut cache = RegionCache::new(SPAN, HALO, BASE);
        let all: HashSet<TilePos> = [tp(0, 0), tp(1, 0), tp(0, 1), tp(1, 1)]
            .into_iter()
            .collect();
        cache.ensure(&n, &all);
        assert_eq!(cache.eval_calls, 4);

        // 参数不变再 ensure：零重算
        cache.ensure(&n, &all);
        assert_eq!(cache.eval_calls, 4);

        // 只失效 (1,1)：增量 = 恰好 1 次重算（Gaea/WM 全图重烤的反例）
        cache.invalidate(&[tp(1, 1)].into_iter().collect());
        cache.ensure(&n, &all);
        assert_eq!(cache.eval_calls, 5);

        // 未失效的 tile 逐位不变
        let again = n.eval_height(tp(0, 0), SPAN, HALO, BASE);
        assert_eq!(cache.tile(tp(0, 0)).unwrap().data, again.data);
    }

    #[test]
    fn assemble_strips_halo_and_stays_deterministic() {
        let n = node();
        let mut cache = RegionCache::new(SPAN, HALO, BASE);
        let all: HashSet<TilePos> = [tp(0, 0), tp(1, 0), tp(0, 1), tp(1, 1)]
            .into_iter()
            .collect();
        cache.ensure(&n, &all);
        let big = cache.assemble((2, 2), tp(0, 0)).unwrap();
        assert_eq!(big.data.len(), (64 * 64) as usize);
        // 拼装后的 (0,0) 内容起点 = tile0 去 halo 的第一个采样
        let direct = n.sample(0, 0);
        assert_eq!(big.get(0, 0).to_bits(), direct.to_bits());
    }

    #[test]
    fn quantized_noise_golden_pins_determinism() {
        // 金标准：冻结 seed=42 区域量化首行前 8 值。改算法/改平台不一致 ⇒ 此测试红
        let n = NoiseNode {
            seed: 42,
            wavelength_mm: 64,
            octaves: 4,
            warp: Some(Warp {
                amplitude_mm: 48,
                wavelength_mm: 256,
            }),
        };
        let t = n.eval_height(tp(0, 0), SPAN, 0, BASE);
        let q = t.quantize_i16();
        assert_eq!(&q[..8], &GOLDEN_ROW8);
    }

    const GOLDEN_ROW8: [i16; 8] = [0, 1130, 80, 1339, 2642, 3825, 2969, 4017]; // 实测冻结：改算法/平台不一致即红
}
