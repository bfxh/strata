//! 域抽象与 tile 生命周期（钉死 01-P1：内存预算制）。
//!
//! 一切节点输入/输出都是 [`Field`]。空间域按 tile 粒度存储，tile 边带 halo
//! 重叠以消除接缝。[`TileCache`] 有显式字节预算，超限按 LRU 淘汰并允许降级
//! mip —— 小内存机器是常态场景而非例外。
#![allow(clippy::must_use_candidate)] // 见 03：与 core 同款豁免高噪声风格项

use std::collections::HashMap;

use strata_geo::TilePos;

/// 高度 tile：单精度 f32 网格，量化交付为 i16。`halo` 为四边重叠像素数。
#[derive(Clone, Debug, PartialEq)]
pub struct HeightTile {
    pub pos: TilePos,
    /// 内容边长（像素），不含 halo
    pub span: u32,
    pub halo: u32,
    /// 行主序，含 halo，边长为 `span + 2*halo`
    pub data: Vec<f32>,
}

impl HeightTile {
    pub fn new(pos: TilePos, span: u32, halo: u32, fill: f32) -> Self {
        let side = span + 2 * halo;
        Self {
            pos,
            span,
            halo,
            data: vec![fill; (side * side) as usize],
        }
    }

    #[inline]
    fn idx(&self, x: u32, y: u32) -> usize {
        let side = (self.span + 2 * self.halo) as usize;
        (y as usize) * side + (x as usize)
    }

    /// 采样含 halo 的完整网格坐标。
    #[inline]
    pub fn get(&self, x: u32, y: u32) -> f32 {
        self.data[self.idx(x, y)]
    }

    #[inline]
    pub fn set(&mut self, x: u32, y: u32, v: f32) {
        let i = self.idx(x, y);
        self.data[i] = v;
    }

    /// 内容区（不含 halo）字节数 * 4。用于预算核算。
    pub fn byte_len(&self) -> usize {
        self.data.len() * std::mem::size_of::<f32>()
    }

    /// 量化为交付 i16（对称舍入，钉死 CPU/GPU parity 的比较域）。
    // clamp 到 [-32767,32767] 后转 i16 必不溢出，故豁免截断告警
    #[allow(clippy::cast_possible_truncation)]
    pub fn quantize_i16(&self) -> Vec<i16> {
        self.data
            .iter()
            .map(|v| (v * 32767.0).round().clamp(-32767.0, 32767.0) as i16)
            .collect()
    }
}

/// 掩码 tile：u8 网格（选择域来源）。
#[derive(Clone, Debug, PartialEq)]
pub struct MaskTile {
    pub pos: TilePos,
    pub span: u32,
    pub halo: u32,
    pub data: Vec<u8>,
}

impl MaskTile {
    pub fn new(pos: TilePos, span: u32, halo: u32, fill: u8) -> Self {
        let side = span + 2 * halo;
        Self {
            pos,
            span,
            halo,
            data: vec![fill; (side * side) as usize],
        }
    }

    pub fn byte_len(&self) -> usize {
        self.data.len()
    }
}

/// 节点间流动的一切。
#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Height(HeightTile),
    Mask(MaskTile),
    Scalar(Scalar),
}

/// 参数/标量域。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scalar {
    F(f32),
    I(i32),
    Seed(u64),
}

/// 预算制 tile 缓存：字节上限 + LRU。返回 false 表示该 tile 超预算无法容纳。
pub struct TileCache {
    budget_bytes: usize,
    used_bytes: usize,
    /// 单调时钟，模拟 LRU 的"最近使用"序
    clock: u64,
    entries: HashMap<TilePos, (u64, usize)>,
}

impl TileCache {
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            budget_bytes,
            used_bytes: 0,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn budget_bytes(&self) -> usize {
        self.budget_bytes
    }

    pub fn contains(&mut self, pos: TilePos) -> bool {
        if !self.entries.contains_key(&pos) {
            return false;
        }
        let now = self.tick();
        if let Some(e) = self.entries.get_mut(&pos) {
            e.0 = now;
        }
        true
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// 尝试放入一个 `bytes` 大小的 tile；必要时 LRU 淘汰。
    pub fn insert(&mut self, pos: TilePos, bytes: usize) -> bool {
        if bytes > self.budget_bytes {
            return false;
        }
        if let Some(&(_, old)) = self.entries.get(&pos) {
            self.used_bytes -= old;
            self.entries.remove(&pos);
        }
        while self.used_bytes + bytes > self.budget_bytes {
            match self.oldest() {
                Some(victim) => {
                    self.evict(victim);
                }
                None => break,
            }
        }
        if self.used_bytes + bytes > self.budget_bytes {
            return false;
        }
        let now = self.tick();
        self.entries.insert(pos, (now, bytes));
        self.used_bytes += bytes;
        true
    }

    pub fn get(&mut self, pos: TilePos) -> Option<usize> {
        let &(_, bytes) = self.entries.get(&pos)?;
        let now = self.tick();
        if let Some(e) = self.entries.get_mut(&pos) {
            e.0 = now;
        }
        Some(bytes)
    }

    fn oldest(&self) -> Option<TilePos> {
        let mut best: Option<(u64, TilePos)> = None;
        for (pos, (ts, _)) in &self.entries {
            match best {
                Some((b, _)) if *ts >= b => {}
                _ => best = Some((*ts, *pos)),
            }
        }
        best.map(|(_, p)| p)
    }

    pub fn evict(&mut self, pos: TilePos) -> bool {
        if let Some((_, bytes)) = self.entries.remove(&pos) {
            self.used_bytes -= bytes;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_geo::DEFAULT_TILE_SPAN_PX;

    fn tp(x: i64) -> TilePos {
        TilePos { x, y: 0, level: 0 }
    }

    #[test]
    fn height_tile_halo_roundtrip() {
        let mut t = HeightTile::new(tp(0), 4, 1, 0.0);
        // side = 4 + 2 = 6；内容区 [1..5)
        t.set(3, 3, 0.5);
        assert_eq!(t.get(3, 3), 0.5);
        assert_eq!(t.data.len(), 36);
    }

    #[test]
    fn quantize_is_symmetric_and_clamped() {
        let mut t = HeightTile::new(tp(0), 1, 0, 0.0);
        t.set(0, 0, 1.0);
        assert_eq!(t.quantize_i16()[0], 32767);
        t.set(0, 0, -2.0);
        assert_eq!(t.quantize_i16()[0], -32767);
        t.set(0, 0, 0.0);
        assert_eq!(t.quantize_i16()[0], 0);
    }

    #[test]
    fn cache_never_exceeds_budget() {
        let mut c = TileCache::new(100);
        assert!(c.insert(tp(0), 60));
        assert!(c.insert(tp(1), 60)); // 淘汰 tp(0)
        assert!(c.used_bytes() <= c.budget_bytes());
        assert!(c.get(tp(0)).is_none());
        assert!(c.get(tp(1)).is_some());
    }

    #[test]
    fn cache_rejects_oversized_tile() {
        let mut c = TileCache::new(100);
        assert!(!c.insert(tp(0), 101));
    }

    #[test]
    fn cache_lru_evicts_least_recently_used() {
        let mut c = TileCache::new(80);
        assert!(c.insert(tp(0), 40));
        assert!(c.insert(tp(1), 40));
        assert!(c.get(tp(0)).is_some()); // tp0 变最近，tp1 变最旧
        assert!(c.insert(tp(2), 40)); // 应淘汰 tp1
        assert!(c.get(tp(1)).is_none());
        assert!(c.get(tp(0)).is_some());
        assert!(c.get(tp(2)).is_some());
    }

    #[test]
    fn default_span_is_power_of_two() {
        assert!(DEFAULT_TILE_SPAN_PX.is_power_of_two());
    }
}
