//! 设计 token（玻璃分层配色 + prominence alpha）与 C2 对比度门的校验。
//!
//! alpha 数字来源：04 文档给的是**实现起点**（regular 0.55 / prominent 0.75），
//! 并明说"最终值由视觉基线锁进 CI"。这里用 [`min_alpha_for`] 实测反推满足
//! "合成后最坏对比度 ≥ 阈值"的最小 alpha，把 token 锁到实测值（带余量），
//! 而不是硬编一个恰好过或假过的数。

use crate::contrast::{worst_contrast, Glass, Rgb};

/// 系统外观（玻璃 tint 随其切换，04「明暗自适应」）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

/// WCAG 判级：body 文本 ≥ 4.5，大字/UI 组件 ≥ 3.0（04 底线 1）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Req(pub f32);

impl Req {
    pub const AA_BODY: Req = Req(4.5);
    pub const AA_LARGE: Req = Req(3.0);
}

/// 一处「文字 × 它所在玻璃层」的合成对，供门逐条判。
#[derive(Clone, Copy, Debug)]
pub struct TextOnGlass {
    pub label: &'static str,
    pub text: Rgb,
    pub glass: Glass,
    pub required: Req,
}

/// 该外观下的玻璃基色（tint，不含 alpha）。深色系统内容多为暗色地形 ⇒ 深灰蓝底玻璃；
/// 浅色反之。数值即 04 的 token，改动须配套重测 alpha。
#[must_use]
pub fn glass_tint(appearance: Appearance) -> Rgb {
    match appearance {
        Appearance::Dark => Rgb::new(0.13, 0.15, 0.19),
        Appearance::Light => Rgb::new(0.93, 0.94, 0.96),
    }
}

/// 该外观下的正文/主文字色。
#[must_use]
pub fn text_color(appearance: Appearance) -> Rgb {
    match appearance {
        // 深色系统上亮字；secondary 稍降饱和但仍走同一条门。
        Appearance::Dark => Rgb::new(0.93, 0.94, 0.96),
        Appearance::Light => Rgb::new(0.10, 0.11, 0.14),
    }
}

/// 用实测 token alpha 构造当前面板（明暗 × regular/prominent 四组正文对）。
#[must_use]
pub fn palette() -> Vec<TextOnGlass> {
    let mut v = Vec::new();
    for a in [Appearance::Dark, Appearance::Light] {
        for (prominent, name) in [(false, "regular"), (true, "prominent")] {
            v.push(TextOnGlass {
                label: match (a, name) {
                    (Appearance::Dark, "regular") => "dark/regular",
                    (Appearance::Dark, _) => "dark/prominent",
                    (Appearance::Light, "regular") => "light/regular",
                    (Appearance::Light, _) => "light/prominent",
                },
                text: text_color(a),
                glass: Glass {
                    tint: glass_tint(a),
                    alpha: alpha_for(a, prominent),
                },
                required: Req::AA_BODY,
            });
        }
    }
    v
}

/// 玻璃 token alpha（最终值，非运行时计算）。
///
/// 数字是 `min_alpha_for(...AA_BODY)` 的**实测最小值 + 余量**锁定的，不是 04 的起点：
/// 实测深色 regular 需 ≥0.671（04 起点 0.55 合成到白底最坏只有 3.11，违反 AA ——
/// 正是 04「对 Apple 被批评点的反向防御」要拦的情况），故抬到下列值。
/// prominence 比 regular 更厚（更高不透明度）。改 tint/文字色后须重跑 `min_alpha_for`。
#[must_use]
pub fn alpha_for(appearance: Appearance, prominent: bool) -> f32 {
    match (appearance, prominent) {
        (Appearance::Dark, false) => 0.72, // 实测 min 0.671 + 余量
        (Appearance::Dark, true) => 0.85,
        (Appearance::Light, false) => 0.62, // 实测 min 0.548 + 余量
        (Appearance::Light, true) => 0.80,
    }
}

/// 实测：给定文字与 tint，满足 `worst_contrast ≥ required` 的最小 alpha（二分，0..=1）。
/// 返回 None ⇒ 该 tint/文字在任何 alpha 下都无法达标（必须换色，不能靠调透明度硬凑）。
#[must_use]
pub fn min_alpha_for(text: Rgb, tint: Rgb, required: Req) -> Option<f32> {
    // alpha 单调：越大越接近纯 tint ⇒ 最坏对比度单调升（tint 本身与文字已够比时）。
    let at = |a: f32| worst_contrast(text, Glass { tint, alpha: a });
    if at(1.0) < required.0 {
        return None; // 纯 tint 都不够，alpha 救不了
    }
    if at(0.0) >= required.0 {
        return Some(0.0);
    }
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..48 {
        let mid = (lo + hi) / 2.0;
        if at(mid) >= required.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(hi)
}

/// C2 门：返回所有 `worst_contrast < required` 的违规项（label, 实得, 要求）。
#[must_use]
pub fn contrast_violations(p: &[TextOnGlass]) -> Vec<(&'static str, f32, f32)> {
    p.iter()
        .filter_map(|t| {
            let got = worst_contrast(t.text, t.glass);
            if got < t.required.0 {
                Some((t.label, got, t.required.0))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_palette_meets_aa_body() {
        // 门判据：四组正文对的最坏对比度都 ≥ 4.5。违规非空即红。
        let bad = contrast_violations(&palette());
        assert!(
            bad.is_empty(),
            "C2 违规（label, 最坏对比度, 要求）: {bad:?} —— 需按 min_alpha_for 抬 alpha 或换色"
        );
    }

    #[test]
    fn negative_control_low_alpha_goes_red() {
        // 门的判红能力：把 regular alpha 压到 0.30（远低于实测所需）⇒ 必被标记。
        let probe = vec![TextOnGlass {
            label: "dark/thin",
            text: text_color(Appearance::Dark),
            glass: Glass {
                tint: glass_tint(Appearance::Dark),
                alpha: 0.30,
            },
            required: Req::AA_BODY,
        }];
        assert!(
            !contrast_violations(&probe).is_empty(),
            "负对照失效：压到 0.30 仍不过门 ⇒ 阈值或合成就错了"
        );
    }

    #[test]
    fn min_alpha_is_just_sufficient_and_monotone() {
        let text = text_color(Appearance::Dark);
        let tint = glass_tint(Appearance::Dark);
        let a = min_alpha_for(text, tint, Req::AA_BODY).expect("该 tint 应可满足 AA");
        // 恰在 min_alpha ⇒ ≥ 阈值（含二分误差）；明显更低 ⇒ 破。
        let at = |al: f32| worst_contrast(text, Glass { tint, alpha: al });
        assert!(at(a) >= 4.5 - 1e-3, "min_alpha={a} 处对比度 {}", at(a));
        assert!(at((a - 0.10).max(0.0)) < 4.5, "min_alpha 之下应破 4.5");
        // token alpha 必须 ≥ 实测所需，否则 token 定低了。
        let tok = alpha_for(Appearance::Dark, false);
        assert!(tok + 1e-6 >= a, "dark/regular token {tok} 低于实测所需 {a}");
    }

    #[test]
    fn report_measured_min_alphas() {
        // 非断言：打印实测最小 alpha 与当前 token，供重锁。
        for a in [Appearance::Dark, Appearance::Light] {
            let m = min_alpha_for(text_color(a), glass_tint(a), Req::AA_BODY);
            let at_reg = worst_contrast(
                text_color(a),
                Glass {
                    tint: glass_tint(a),
                    alpha: alpha_for(a, false),
                },
            );
            println!(
                "C2 {a:?}: min_alpha(4.5)={:?}, 当前 regular token 最坏对比={at_reg:.3}",
                m.map(|x| (x * 1000.0).round() / 1000.0)
            );
        }
    }
}
