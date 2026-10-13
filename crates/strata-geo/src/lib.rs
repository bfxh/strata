//! 坐标与精度体系（钉死 01-P4）。
//!
//! 世界坐标一律 `i64`（毫米）；渲染与局部计算用 `LocalF32`（相对 tile 原点或
//! 相机原点的毫米偏移）。`WorldMm` 与 `LocalF32` 不允许隐式互通——任何"世界
//! 坐标存 f32"的写法在本类型体系下编译不过。
#![allow(clippy::must_use_candidate)] // core crate 开 pedantic，但豁免此高噪声风格项

/// 默认 tile 边长（像素/顶点）
pub const DEFAULT_TILE_SPAN_PX: u32 = 512;

/// 世界坐标，单位毫米，i64。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WorldMm(pub i64);

/// 局部坐标，单位毫米（相对某个 tile 原点或相机 eye）。
/// 只能由 [`local_from_world`] 这类显式换算产生。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LocalF32(pub f32);

/// tile 定位：整数世界被切成 2 的幂层级网格。`level` 即 mip 层。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TilePos {
    pub x: i64,
    pub y: i64,
    pub level: u8,
}

impl TilePos {
    /// 该层单个 tile 覆盖的世界毫米跨度。`base_px` 为 level 0 的 tile 边长。
    #[inline]
    pub fn span_mm(&self, base_px: u32) -> i64 {
        i64::from(base_px) << self.level
    }

    /// 覆盖给定世界坐标的 tile。负坐标向 −∞ 取整（`floor_div`）。
    pub fn containing(world_x: i64, world_y: i64, level: u8, base_px: u32) -> Self {
        let span = i64::from(base_px) << level;
        Self {
            x: floor_div(world_x, span),
            y: floor_div(world_y, span),
            level,
        }
    }

    /// tile 的世界原点（左下角）。
    pub fn world_origin(&self, base_px: u32) -> (i64, i64) {
        let span = self.span_mm(base_px);
        (self.x * span, self.y * span)
    }

    /// 降到更粗层级：同一坐标所在的大 tile。level 只能变大。
    pub fn to_level(&self, target: u8) -> Option<Self> {
        if target < self.level {
            return None;
        }
        let factor = 1i64 << (target - self.level);
        Some(Self {
            x: floor_div(self.x, factor),
            y: floor_div(self.y, factor),
            level: target,
        })
    }

    /// 升到更细层级（target < level）：该 tile 覆盖的所有子 tile。
    pub fn children_at_level(&self, target: u8) -> Option<Vec<Self>> {
        if target >= self.level {
            return None;
        }
        let factor = 1i64 << (self.level - target);
        let n = usize::try_from(factor * factor).ok()?;
        if n > 4096 {
            return None;
        }
        let base = self.x * factor;
        let by = self.y * factor;
        let mut out = Vec::with_capacity(n);
        for dy in 0..factor {
            for dx in 0..factor {
                out.push(Self {
                    x: base + dx,
                    y: by + dy,
                    level: target,
                });
            }
        }
        Some(out)
    }
}

/// floor 版整数除法（负数向 −∞，保证 tile 覆盖连续无缝）。
#[inline]
pub fn floor_div(a: i64, b: i64) -> i64 {
    debug_assert!(b > 0);
    let q = a / b;
    if a % b != 0 && (a < 0) != (b < 0) {
        q - 1
    } else {
        q
    }
}

/// 世界 → 局部：相对原点 `origin` 的偏移。差值先算（i64 精确），再转 f32。
/// 调用方保证 |偏移| < 2^24 mm（≈16.8 km，RTE 视距内），此时转 f32 无舍入。
#[inline]
#[allow(clippy::cast_precision_loss)] // 有文档值域契约：差值 <2^24 时 f32 精确
pub fn local_from_world(world: WorldMm, origin: WorldMm) -> LocalF32 {
    LocalF32((world.0 - origin.0) as f32)
}

/// relative-to-eye 相机原点：渲染每帧把世界点减去 eye，浮点误差只作用在
/// 小的相对值上（Geometry Clipmaps / Thatcher 2021 的做法）。
#[derive(Clone, Copy, Debug)]
pub struct Eye {
    pub x: WorldMm,
    pub y: WorldMm,
    pub z: WorldMm,
}

impl Eye {
    #[inline]
    pub fn rel(&self, x: WorldMm, y: WorldMm, z: WorldMm) -> (LocalF32, LocalF32, LocalF32) {
        (
            local_from_world(x, self.x),
            local_from_world(y, self.y),
            local_from_world(z, self.z),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_div_negative_goes_to_minus_inf() {
        assert_eq!(floor_div(-1, 512), -1);
        assert_eq!(floor_div(-512, 512), -1);
        assert_eq!(floor_div(-513, 512), -2);
        assert_eq!(floor_div(511, 512), 0);
    }

    #[test]
    fn tile_containing_and_origin_roundtrip() {
        let t = TilePos::containing(-1, 1_024, 0, DEFAULT_TILE_SPAN_PX);
        assert_eq!((t.x, t.y, t.level), (-1, 2, 0));
        let (ox, oy) = t.world_origin(DEFAULT_TILE_SPAN_PX);
        // 原点必须 ≤ 查询点 < 原点+跨度
        assert!(ox <= -1 && -1 < ox + t.span_mm(DEFAULT_TILE_SPAN_PX));
        assert!(oy <= 1_024 && 1_024 < oy + t.span_mm(DEFAULT_TILE_SPAN_PX));
    }

    #[test]
    fn coarse_level_maps_down_consistently() {
        let fine = TilePos::containing(123_456, -700_000, 3, DEFAULT_TILE_SPAN_PX);
        let coarse = fine.to_level(5).unwrap();
        let check = TilePos::containing(123_456, -700_000, 5, DEFAULT_TILE_SPAN_PX);
        assert_eq!(coarse, check);
        assert!(fine.to_level(2).is_none());
    }

    #[test]
    fn children_cover_parent_exactly() {
        // level 越大 = span 越大 = 越粗；children 在更细（更小）level
        let p = TilePos {
            x: -3,
            y: 4,
            level: 2,
        };
        let kids = p.children_at_level(0).unwrap();
        assert_eq!(kids.len(), 16);
        let (po, _) = p.world_origin(DEFAULT_TILE_SPAN_PX);
        let span = p.span_mm(DEFAULT_TILE_SPAN_PX);
        for k in &kids {
            let (ox, _) = k.world_origin(DEFAULT_TILE_SPAN_PX);
            assert!(ox >= po && ox < po + span);
        }
        // 更粗 target 不是 children；同 level 也不是
        assert!(p.children_at_level(3).is_none());
        assert!(p.children_at_level(2).is_none());
    }

    #[test]
    fn rte_difference_is_exact_within_view_range() {
        // i64 差值在 2^24 mm 内转 f32 无舍入：这是 RTE 无抖动的数值前提
        let eye = Eye {
            x: WorldMm(9_000_000_000),
            y: WorldMm(-4_000_000_000),
            z: WorldMm(100),
        };
        let world = WorldMm(eye.x.0 + 8_000_000); // +8 km
        let l = local_from_world(world, eye.x);
        assert_eq!(l.0, 8_000_000.0_f32);
    }
}
