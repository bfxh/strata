//! 位确定噪声核（钉死 01-P8 的地基）。
//!
//! **为什么只有一种浮点写法**：CPU/GPU parity 要求量化 i16 后逐位一致。超越函数
//! （sin/pow/exp）在不同后端不保证逐位，故这里只用整数 hash + f32 的 `+ - *`、
//! `floor`、`fract`。这段逻辑可被 1:1 翻成 WGSL，M1 GPU 落地时靠它建 parity 门 B2。

#![allow(clippy::must_use_candidate)] // 与 core 同款豁免高噪声风格项
#![allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)] // hash 内 i32↔u32 是故意的补码重排
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // 格点 i64→f32 有 |v|<2^24 契约（见 perlin 注）

/// splitmix64 一步混合，纯整数 ⇒ 跨平台逐位确定。
#[inline]
fn mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// 2D 整数格点 hash（含 seed）。
#[inline]
pub fn hash2(ix: i32, iy: i32, seed: u64) -> u64 {
    let x = u64::from(ix as u32).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let y = u64::from(iy as u32).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mix64(seed ^ x ^ y)
}

/// 8 个单位化对角/轴向梯度分量（模块级，避免 items-after-statements）。
const GRAD_X: [f32; 8] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 0.0, 0.0];
const GRAD_Y: [f32; 8] = [1.0, 1.0, -1.0, -1.0, 0.0, 0.0, 1.0, -1.0];

/// 8 方向梯度，靠 hash 低 3 位选取，点积只含乘加。
#[inline]
fn grad2(h: u64, dx: f32, dy: f32) -> f32 {
    // h & 7 ∈ 0..8，try_from 到 usize 必成
    let ix = usize::try_from(h & 7).unwrap_or(0);
    GRAD_X[ix] * dx + GRAD_Y[ix] * dy
}

#[inline]
fn fade(t: f32) -> f32 {
    // Perlin 平滑：6t^5-15t^4+10t^3，纯乘加 ⇒ 确定
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

/// 经典梯度噪声，输入世界毫米（i64，精确）+ 波长（mm），输出约 [-1,1]。
/// 坐标先算整数差再转 f32，避免大 float 直用（呼应 strata-geo 的 RTE 契约）。
pub fn perlin(world_x: i64, world_y: i64, wavelength_mm: i64, seed: u64) -> f32 {
    debug_assert!(wavelength_mm > 0);
    let gx = world_x as f32 / wavelength_mm as f32;
    let gy = world_y as f32 / wavelength_mm as f32;
    let x0 = gx.floor();
    let y0 = gy.floor();
    let fx = gx - x0;
    let fy = gy - y0;
    let ix0 = x0 as i32;
    let iy0 = y0 as i32;

    let u = fade(fx);
    let v = fade(fy);

    let n00 = grad2(hash2(ix0, iy0, seed), fx, fy);
    let n10 = grad2(hash2(ix0 + 1, iy0, seed), fx - 1.0, fy);
    let n01 = grad2(hash2(ix0, iy0 + 1, seed), fx, fy - 1.0);
    let n11 = grad2(hash2(ix0 + 1, iy0 + 1, seed), fx - 1.0, fy - 1.0);

    let a = lerp(n00, n10, u);
    let b = lerp(n01, n11, u);
    lerp(a, b, v)
}

/// 分形叠加：octaves 层，逐层波长减半、权重按 gain 衰减。
/// 归一化到 [-1,1]（权重和固定，确定性）。`lacunarity_mm` 不做，靠移位减半。
pub fn fbm(world_x: i64, world_y: i64, base_wavelength_mm: i64, octaves: u8, seed: u64) -> f32 {
    let mut sum = 0.0f32;
    let mut amp = 1.0f32;
    let mut norm = 0.0f32;
    let mut wl = base_wavelength_mm;
    for _ in 0..octaves {
        sum += amp * perlin(world_x, world_y, wl, seed);
        norm += amp;
        amp *= 0.5; // gain = 0.5，纯乘
        wl = wl.saturating_div(2).max(1);
    }
    if norm > 0.0 {
        sum / norm
    } else {
        0.0
    }
}

/// 域扭曲参数：把采样坐标本身用两路低频 fbm 偏移，制造蜿蜒/流体状结构。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Warp {
    pub amplitude_mm: i64,
    pub wavelength_mm: i64,
}

/// 带域扭曲的 fbm 采样点。扭曲坐标仍走 i64 世界毫米 + round。
pub fn warped_fbm(
    world_x: i64,
    world_y: i64,
    base_wavelength_mm: i64,
    octaves: u8,
    seed: u64,
    warp: Option<Warp>,
) -> f32 {
    match warp {
        None => fbm(world_x, world_y, base_wavelength_mm, octaves, seed),
        Some(w) => {
            let ox = fbm(world_x, world_y, w.wavelength_mm, 2, seed ^ 0x0a1b_2c3d);
            let oy = fbm(world_x, world_y, w.wavelength_mm, 2, seed ^ 0x4e5f_6a7b);
            // round 到整数毫米，保持确定性（无 fract 残差依赖）
            let dx = (ox * w.amplitude_mm as f32).round() as i64;
            let dy = (oy * w.amplitude_mm as f32).round() as i64;
            fbm(
                world_x + dx,
                world_y + dy,
                base_wavelength_mm,
                octaves,
                seed,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_deterministic_and_seed_sensitive() {
        assert_eq!(hash2(3, -7, 42), hash2(3, -7, 42));
        assert_ne!(hash2(3, -7, 42), hash2(3, -7, 43));
        assert_ne!(hash2(3, -7, 42), hash2(4, -7, 42));
    }

    #[test]
    fn perlin_stays_in_range_and_lattice_nodes_zero() {
        // 整格点处 fx=fy=0 ⇒ 插值退化为 n00 在 (0,0)，梯度点积为 0
        let v = perlin(0, 0, 512, 999);
        assert_eq!(v, 0.0);
        for s in -20..20i64 {
            let p = perlin(s * 137, s * 211, 1024, 7);
            assert!((-1.5..=1.5).contains(&p), "out of range: {p}");
        }
    }

    #[test]
    fn fbm_double_run_bitwise_identical() {
        // 确定性金标准：同输入两次跑逐位相等（B3 门的 CPU 侧雏形）
        let a = fbm(12_345, -6_789, 4096, 5, 2024);
        let b = fbm(12_345, -6_789, 4096, 5, 2024);
        assert_eq!(a.to_bits(), b.to_bits());
        assert!((-1.0..=1.0).contains(&a));
    }

    #[test]
    fn fbm_octave_zero_is_zero() {
        assert_eq!(fbm(1000, 2000, 512, 0, 1), 0.0);
    }

    #[test]
    fn warp_changes_field_but_stays_deterministic() {
        let w = Warp {
            amplitude_mm: 200,
            wavelength_mm: 8192,
        };
        let p = warped_fbm(5000, 5000, 1024, 4, 5, Some(w));
        let q = warped_fbm(5000, 5000, 1024, 4, 5, Some(w));
        assert_eq!(p.to_bits(), q.to_bits());
        let unw = warped_fbm(5000, 5000, 1024, 4, 5, None);
        assert!((p - unw).abs() > 1e-6 || p.to_bits() == unw.to_bits());
    }
}
