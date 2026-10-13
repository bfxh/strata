//! GPU 侧编排骨架。M1 在此接入 wgpu：device/queue、compute 调度、texture-array
//! tile cache（受 `gpu_budget_bytes` 预算约束，钉死 01-P1/D2）。
//!
//! 本文件先只钉"预算"这一承重数值与默认值，使其可被测试锁定；wgpu 依赖随
//! 渲染器落地时再加，避免骨架阶段引入重编译面。

/// 默认 GPU tile 缓存预算（字节），对齐 06 文档 budget.toml（默认 512MB）。
/// 小内存机器可下调；上限治理是 D2 门的判据来源。
pub const DEFAULT_GPU_BUDGET_BYTES: usize = 512 * 1024 * 1024;

/// 单个 `HeightTile` 在显存中的字节占用（f32 网格，含 halo 边距）。
/// 与 strata-field 的 `byte_len` 同口径，供预算核算复用。
pub fn height_tile_bytes(side_with_halo: u32) -> usize {
    (side_with_halo as usize)
        .saturating_mul(side_with_halo as usize)
        .saturating_mul(core::mem::size_of::<f32>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_default_is_512mb() {
        assert_eq!(DEFAULT_GPU_BUDGET_BYTES, 536_870_912);
    }

    #[test]
    fn tile_bytes_matches_f32_grid() {
        // 512x512 f32 = 1MB
        assert_eq!(height_tile_bytes(512), 512 * 512 * 4);
    }

    #[test]
    fn budget_fits_integral_tiles() {
        let per = height_tile_bytes(512);
        let fits = DEFAULT_GPU_BUDGET_BYTES / per;
        assert_eq!(fits, 512); // 512MB / 1MB = 512 个满 tile
    }
}

pub mod math;
pub mod probe;
pub mod renderer;
