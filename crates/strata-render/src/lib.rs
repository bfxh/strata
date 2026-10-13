//! 渲染层几何算术（P5 的尺子）。M0 先钉死 clipmap 层数计算并可测；
//! wgpu/WGSL 渲染 pass 在 M1 接入本 crate 时以此为准。
#![allow(clippy::must_use_candidate)] // 与 core 同款豁免高噪声风格项

use strata_geo::TilePos;

/// 覆盖 `view_range_mm`（相对 eye 的最大水平距离）所需的 clipmap 层数。
/// 层 `i` 覆盖 eye 周围 `tile_span << i` 毫米；返回最小的 N 使最外层 ≥ 视距，
/// 至少 1 层、至多 `max_layers` 层（防失控显存占用）。
pub fn clipmap_layer_count(view_range_mm: i64, base_px: u32, max_layers: u8) -> u8 {
    assert!(view_range_mm >= 0, "view_range 不能为负");
    let span = i64::from(base_px);
    let mut covered = span;
    let mut n: u8 = 1;
    while covered < view_range_mm && n < max_layers {
        covered = covered.saturating_mul(2);
        n += 1;
    }
    n
}

/// 第 `i` 层对应的 TilePos level（层 0 = 最细 = level 0，逐层变粗）。
pub fn layer_to_level(layer: u8) -> u8 {
    layer
}

/// 相机在层 `i` 上的锚定 tile（floor 对齐，负坐标向 −∞，与 strata-geo 一致）。
pub fn layer_anchor(eye_x_mm: i64, eye_y_mm: i64, layer: u8, base_px: u32) -> TilePos {
    TilePos::containing(eye_x_mm, eye_y_mm, layer_to_level(layer), base_px)
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_geo::DEFAULT_TILE_SPAN_PX;

    #[test]
    fn one_layer_when_span_already_covers_range() {
        assert_eq!(clipmap_layer_count(512, DEFAULT_TILE_SPAN_PX, 12), 1);
        assert_eq!(clipmap_layer_count(0, DEFAULT_TILE_SPAN_PX, 12), 1);
    }

    #[test]
    fn doubling_covers_target_range() {
        // 层跨度 512mm；视距 100_000mm ⇒ 512*2^8=131072≥100000，即 1+8=9 层
        assert_eq!(clipmap_layer_count(100_000, DEFAULT_TILE_SPAN_PX, 16), 9);
        // 边界：恰好等于某层跨度不加层
        assert_eq!(clipmap_layer_count(131_072, DEFAULT_TILE_SPAN_PX, 16), 9);
        assert_eq!(clipmap_layer_count(131_073, DEFAULT_TILE_SPAN_PX, 16), 10);
    }

    #[test]
    fn respects_max_layers_cap() {
        // 巨大视距被上限钉住，防显存失控
        assert_eq!(
            clipmap_layer_count(i64::MAX / 2, DEFAULT_TILE_SPAN_PX, 12),
            12
        );
    }

    #[test]
    fn anchored_at_eye_moves_with_camera() {
        // clipmap 以 eye 为中心：eye 移动 ⇒ anchor 跟随
        let a = layer_anchor(10_000, 10_000, 2, DEFAULT_TILE_SPAN_PX);
        let b = layer_anchor(11_000, 10_000, 2, DEFAULT_TILE_SPAN_PX);
        // 同层，tile 坐标不同
        assert_eq!(a.level, b.level);
        assert_ne!(a.x, b.x);
        // 每层覆盖跨度 = base<<(level)
        assert_eq!(a.span_mm(DEFAULT_TILE_SPAN_PX), 512 << 2);
    }
}
