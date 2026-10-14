//! WCAG 2.1 相对亮度 / 对比度 + 半透明玻璃合成判据（C2 门的数学内核）。
//!
//! 判据原理（对应 04「玻璃层文字与玻璃底色合成后对比度 ≥ 4.5:1」）：文字画在
//! `玻璃 tint(alpha) 合成到内容底` 之后得到的有效底色上。有效底色的**相对亮度随内容底
//! 亮度单调**，而固定文字对某个底亮度区间的对比度，其最小值必落在区间端点 ⇒ 只需在
//! 纯黑底与纯白底两个极端各合成一次、取更差者，即最坏情况（有依据，非乱采样）。

/// sRGB 三分量，取值 0.0..=1.0（gamma 空间，即最终显示色）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb {
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Rgb {
    #[must_use]
    pub const fn new(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b }
    }
}

/// 单通道 sRGB→线性（WCAG 用 0.04045 拐点；早期文档的 0.03928 差异 <1e-4，忽略）。
fn lin(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG 相对亮度 L ∈ 0..=1。
#[must_use]
pub fn rel_lum(c: Rgb) -> f32 {
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

/// WCAG 对比度 (L_hi+0.05)/(L_lo+0.05)，∈ 1..=21。
#[must_use]
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let la = rel_lum(a);
    let lb = rel_lum(b);
    let hi = la.max(lb);
    let lo = la.min(lb);
    (hi + 0.05) / (lo + 0.05)
}

/// gamma 空间 alpha 合成：`tint`（不透明度 `alpha`）盖在 `backdrop` 上。
/// 简化为逐通道 `tint*a + backdrop*(1-a)`（真实 glass 可在线性域合成，此处口径已在
/// 门文档登记；端点取最坏对合成域不敏感）。
#[must_use]
pub fn over(tint: Rgb, alpha: f32, backdrop: Rgb) -> Rgb {
    let k = 1.0 - alpha;
    Rgb {
        r: tint.r * alpha + backdrop.r * k,
        g: tint.g * alpha + backdrop.g * k,
        b: tint.b * alpha + backdrop.b * k,
    }
}

/// 一条玻璃层规格：tint 色 + 不透明度 alpha。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glass {
    pub tint: Rgb,
    pub alpha: f32,
}

/// 文字画在"该玻璃浮于任意内容底"上时，可能遇到的**最差**对比度（黑底/白底取劣者）。
#[must_use]
pub fn worst_contrast(text: Rgb, glass: Glass) -> f32 {
    let black = Rgb::new(0.0, 0.0, 0.0);
    let white = Rgb::new(1.0, 1.0, 1.0);
    let c_dark = contrast(text, over(glass.tint, glass.alpha, black));
    let c_light = contrast(text, over(glass.tint, glass.alpha, white));
    c_dark.min(c_light)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(h: u32) -> Rgb {
        Rgb::new(
            ((h >> 16) & 0xff) as f32 / 255.0,
            ((h >> 8) & 0xff) as f32 / 255.0,
            (h & 0xff) as f32 / 255.0,
        )
    }

    #[test]
    fn known_contrast_values() {
        // 黑白 21、同色 1，均 WCAG 定义直接可推。
        assert!((contrast(Rgb::new(0.0, 0.0, 0.0), Rgb::new(1.0, 1.0, 1.0)) - 21.0).abs() < 0.01);
        assert!((contrast(Rgb::new(0.5, 0.5, 0.5), Rgb::new(0.5, 0.5, 0.5)) - 1.0).abs() < 1e-6);
        // 权威样例：#767676 on #ffffff ≈ 4.54:1（WCAG 参考实现值）。
        let g = contrast(hex(0x767676), hex(0xffffff));
        assert!((g - 4.54).abs() < 0.02, "#767676/white 实得 {g}");
    }

    #[test]
    fn rel_lum_endpoints() {
        assert!(rel_lum(Rgb::new(0.0, 0.0, 0.0)).abs() < 1e-6);
        assert!((rel_lum(Rgb::new(1.0, 1.0, 1.0)) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn over_is_alpha_blend() {
        let tint = Rgb::new(0.0, 0.0, 0.0);
        let white = Rgb::new(1.0, 1.0, 1.0);
        // 50% 黑 tint 盖白底 ⇒ 0.5 灰。
        let m = over(tint, 0.5, white);
        assert!((m.r - 0.5).abs() < 1e-6 && (m.g - 0.5).abs() < 1e-6);
        // alpha=1 ⇒ 纯 tint；alpha=0 ⇒ 纯底。
        assert_eq!(over(tint, 1.0, white), tint);
        assert_eq!(over(tint, 0.0, white), white);
    }

    #[test]
    fn worst_is_at_extreme_backdrop() {
        // 中间灰玻璃 + 白字：黑底与白底两个端点的对比度都应 ≤ 任何中间底的对比度，
        // 故 worst_contrast 必等于二者之一（单调性验证）。
        let glass = Glass {
            tint: Rgb::new(0.2, 0.2, 0.25),
            alpha: 0.6,
        };
        let text = Rgb::new(0.95, 0.95, 0.95);
        let w = worst_contrast(text, glass);
        let mid = contrast(text, over(glass.tint, glass.alpha, Rgb::new(0.5, 0.5, 0.5)));
        assert!(w <= mid + 1e-6, "最坏值 {w} 不应高于中点 {mid}（单调性）");
    }
}
