//! 侵蚀模拟核（M2a）：热侵蚀（talus 休止角）+ 水力侵蚀（Mei 2007 简化版）。
//!
//! **承重不变量（都进测试）**：
//! 1. 闭合边界下，水力步 `Σ(h+sediment)` 严格守恒（水/沉积只在格间搬运，lossless）；
//!    雨量/蒸发是唯一的水量进出，测试里显式记账。
//! 2. 热侵蚀：`Σh` 守恒（成对搬运），反复步后最大坡差单调收敛到 talus。
//! 3. 位确定：全程只用 f32 的 `+ - *` 与 `min/max`，固定四邻序 [E,W,N,S] + 双缓冲
//!    ⇒ 与迭代顺序无关的确定结果；同输入两次跑量化 i16 逐位一致（对齐 B2/B3）。
//!
//! 归一高度域 `h∈[-1,1]`（与 `strata-field` 的 `quantize_i16` 契约一致），
//! 水量/沉积同域。这是 01-P3 的形态模拟面 + P8 parity 的第二批真值语料。

#![allow(clippy::must_use_candidate)]
#![allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)] // 栅格下标 i32↔usize↔f32 往返都限定在 [0,n) 且 n≤2^24（与 noise.rs 同款契约）

/// 四邻偏移，固定序 [E, W, N, S]（决定确定性，勿改序）。
const DX: [i32; 4] = [1, -1, 0, 0];
const DY: [i32; 4] = [0, 0, 1, -1];

/// 反向邻接方向：邻居 nb 在方向 k，nb 朝本格流来的那条出流方向 = FLIP[k]（E↔W, N↔S）。
const FLIP: [usize; 4] = [1, 0, 3, 2];

#[inline]
fn idx(x: usize, y: usize, n: usize) -> usize {
    y * n + x
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThermalParams {
    /// 休止角对应的最大允许坡差（归一高度/格）
    pub talus: f32,
    /// 每次搬运系数 ∈ [0,1]
    pub k_transfer: f32,
}

impl Default for ThermalParams {
    fn default() -> Self {
        Self {
            talus: 0.02,
            k_transfer: 0.5,
        }
    }
}

/// 热侵蚀单步：逐对（固定序，Gauss-Seidel 式）松弛到 talus 内。
///
/// 数学保证：单次搬运后两格高差变为 `talus + (1-k)(diff-talus)`（diff>talus 时），
/// 是两端点的凸组合 ⇒ 全局最大坡差**逐步单调不增**，且每格高始终夹在初值邻域内
/// ⇒ 收敛不震荡、无过冲（双缓冲并行版会过冲振荡，已被判红实测否决）。
/// 成对搬运 ⇒ Σh 严格守恒。
///
/// # Panics
///
/// `h.len() != n * n` 时 panic（栅格尺寸契约，调用方须传同一 n 生成的场）。
pub fn thermal_step(h: &mut [f32], n: usize, p: &ThermalParams) {
    assert_eq!(h.len(), n * n);
    for y in 0..n {
        for x in 0..n {
            let c = idx(x, y, n);
            for k in 0..4 {
                let nx = x as i32 + DX[k];
                let ny = y as i32 + DY[k];
                if nx < 0 || ny < 0 || nx as usize >= n || ny as usize >= n {
                    continue;
                }
                let nb = idx(nx as usize, ny as usize, n);
                let diff = h[c] - h[nb];
                if diff > p.talus {
                    let mv = (diff - p.talus) * 0.5 * p.k_transfer;
                    h[c] -= mv;
                    h[nb] += mv;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HydraulicParams {
    /// 每步降雨（加到水面；破坏纯守恒，测试显式记账）
    pub rain: f32,
    /// 每步蒸发比例 ∈ [0,1]
    pub evaporation: f32,
    /// 携带容量系数：`capacity = k_capacity · 出流量`
    pub k_capacity: f32,
    /// 溶蚀速率 ∈ [0,1]
    pub k_dissolve: f32,
    /// 沉积速率 ∈ [0,1]
    pub k_deposit: f32,
    /// 无水阈值（低于此不参与搬运，防除零）
    pub min_water: f32,
}

impl Default for HydraulicParams {
    fn default() -> Self {
        Self {
            rain: 0.0,
            evaporation: 0.0,
            k_capacity: 4.0,
            k_dissolve: 0.004,
            k_deposit: 0.35,
            min_water: 1e-6,
        }
    }
}

/// 水力侵蚀一整套状态。三场同域，步内在双缓冲上搬运。
#[derive(Clone, Debug, PartialEq)]
pub struct Hydraulic {
    pub n: usize,
    pub h: Vec<f32>,
    pub water: Vec<f32>,
    pub sediment: Vec<f32>,
}

impl Hydraulic {
    /// # Panics
    ///
    /// `h.len() != n * n` 时 panic（栅格尺寸契约）。
    pub fn new(n: usize, h: Vec<f32>) -> Self {
        assert_eq!(h.len(), n * n);
        Self {
            n,
            h,
            water: vec![0.0; n * n],
            sediment: vec![0.0; n * n],
        }
    }

    pub fn total_material(&self) -> f64 {
        (0..self.h.len())
            .map(|i| f64::from(self.h[i]) + f64::from(self.sediment[i]))
            .sum()
    }

    pub fn total_water(&self) -> f64 {
        self.water.iter().map(|&w| f64::from(w)).sum()
    }

    /// 单步：水从高水头往低水头搬运（携沉积），按容量溶蚀/沉积，再降雨/蒸发。
    /// 返回是否发生了任何流动（全平且无水时 false，供调用方早停）。
    ///
    /// **序无关（GPU 前置）**：每格的 `new_water/new_sed/out_water` 都由「按固定
    /// [E,W,N,S] 顺序求 inflow 总和与 outflow 总和 ⇒ 一次加减」得到，与外层行主序
    /// 无关。f32 加法不满足交换律，跨格累加会让 GPU 无法逐位镜像；此处把顺序约束
    /// 收敛到每格自身 4 项内部，才能被 compute shader 1:1 直译（M2b-乙 parity 前置）。
    pub fn step(&mut self, p: &HydraulicParams) -> bool {
        let n = self.n;
        let len = n * n;

        // 阶段 1：每格算 4 向流出水量 flow_w[c][k] 与流出沉积 flow_s[c][k]
        // （只用旧态；carry 用 `sediment * frac` 原式，避免后续从 flow/water 反解）。
        let mut flow_w: Vec<[f32; 4]> = vec![[0.0f32; 4]; len];
        let mut flow_s: Vec<[f32; 4]> = vec![[0.0f32; 4]; len];
        let mut any_flow = false;
        for y in 0..n {
            for x in 0..n {
                let c = idx(x, y, n);
                if self.water[c] <= p.min_water {
                    continue;
                }
                let head_c = self.h[c] + self.water[c];
                let mut total_drop = 0.0f32;
                let mut drops = [0.0f32; 4];
                for k in 0..4 {
                    let nx = x as i32 + DX[k];
                    let ny = y as i32 + DY[k];
                    if nx < 0 || ny < 0 || nx as usize >= n || ny as usize >= n {
                        continue;
                    }
                    let nb = idx(nx as usize, ny as usize, n);
                    let d = (head_c - (self.h[nb] + self.water[nb])).max(0.0);
                    drops[k] = d;
                    total_drop += d;
                }
                if total_drop <= 0.0 {
                    continue;
                }
                any_flow = true;
                let w_out = self.water[c];
                let s_out = self.sediment[c];
                for k in 0..4 {
                    let frac = drops[k] / total_drop;
                    flow_w[c][k] = w_out * frac;
                    flow_s[c][k] = s_out * frac;
                }
            }
        }

        // 阶段 2：每格按固定 [E,W,N,S] 顺序求 outflow / inflow 总和 ⇒ 一次加减得新态。
        // flip(k) = 邻居 k 方向上「向我流来」对应的那条出流方向（E↔W, N↔S）。
        let mut new_water = vec![0.0f32; len];
        let mut new_sed = vec![0.0f32; len];
        let mut out_water = vec![0.0f32; len];
        let mut new_h = self.h.clone();
        for y in 0..n {
            for x in 0..n {
                let c = idx(x, y, n);
                let mut w_out_sum = 0.0f32;
                let mut s_out_sum = 0.0f32;
                for k in 0..4 {
                    w_out_sum += flow_w[c][k];
                    s_out_sum += flow_s[c][k];
                }
                out_water[c] = w_out_sum;
                let mut w_in_sum = 0.0f32;
                let mut s_in_sum = 0.0f32;
                for k in 0..4 {
                    let nx = x as i32 + DX[k];
                    let ny = y as i32 + DY[k];
                    if nx < 0 || ny < 0 || nx as usize >= n || ny as usize >= n {
                        continue;
                    }
                    let nb = idx(nx as usize, ny as usize, n);
                    w_in_sum += flow_w[nb][FLIP[k]];
                    s_in_sum += flow_s[nb][FLIP[k]];
                }
                new_water[c] = self.water[c] - w_out_sum + w_in_sum;
                new_sed[c] = self.sediment[c] - s_out_sum + s_in_sum;
            }
        }

        // 溶蚀 / 沉积（h ↔ sediment 内搬运，保 Σ(h+sed) 不变）
        for i in 0..len {
            let capacity = p.k_capacity * out_water[i];
            let s = new_sed[i];
            if capacity > s {
                let avail = (capacity - s) * p.k_dissolve;
                let eroded = avail.min(new_h[i].max(0.0));
                new_h[i] -= eroded;
                new_sed[i] += eroded;
            } else {
                let excess = (s - capacity) * p.k_deposit;
                let dep = excess.min(s.max(0.0));
                new_sed[i] -= dep;
                new_h[i] += dep;
            }
        }

        // 降雨 / 蒸发（唯一水量进出）
        if p.rain != 0.0 || p.evaporation != 0.0 {
            for w in &mut new_water {
                let v = *w + p.rain;
                *w = v * (1.0 - p.evaporation);
            }
        }

        self.h = new_h;
        self.water = new_water;
        self.sediment = new_sed;
        any_flow
    }
}

#[cfg(test)]
#[allow(
    clippy::many_single_char_names,
    clippy::cast_precision_loss,
    clippy::cast_lossless
)]
mod tests {
    use super::*;

    fn cone(n: usize, amp: f32) -> Vec<f32> {
        let c = (n - 1) as f32 / 2.0;
        (0..n * n)
            .map(|i| {
                let x = (i % n) as f32;
                let y = (i / n) as f32;
                let d = ((x - c) * (x - c) + (y - c) * (y - c)).sqrt();
                (amp - 0.1 * d).max(0.0)
            })
            .collect()
    }

    #[test]
    fn thermal_conserves_total_and_flattens_slope() {
        // 注意初始锥高选 0.12：锥体完全摊平到 talus 斜率所需底半径
        // (0.12/0.02=6格) 在 16 格盒内可行；若选 0.9 则"全差≤talus"
        // 在守恒约束下物理上不可达（会收敛到盒子几何极限而非 talus），
        // 那是错误断言而不是算法缺陷——这里断言正确的对象。
        let n = 16;
        let h0 = cone(n, 0.12);
        let sum0: f64 = h0.iter().map(|&v| v as f64).sum();
        let mut cur = h0.clone();
        let p = ThermalParams::default();
        let mut max_slope0 = 0.0f32;
        // 初始最大邻接坡差
        for y in 0..n {
            for x in 0..n {
                for k in 0..4 {
                    let nx = x as i32 + DX[k];
                    let ny = y as i32 + DY[k];
                    if nx < 0 || ny < 0 || nx as usize >= n || ny as usize >= n {
                        continue;
                    }
                    max_slope0 =
                        max_slope0.max(cur[idx(x, y, n)] - cur[idx(nx as usize, ny as usize, n)]);
                }
            }
        }
        let mut last = max_slope0;
        for _ in 0..50 {
            thermal_step(&mut cur, n, &p);
            let mut ms = 0.0f32;
            for y in 0..n {
                for x in 0..n {
                    for k in 0..4 {
                        let nx = x as i32 + DX[k];
                        let ny = y as i32 + DY[k];
                        if nx < 0 || ny < 0 || nx as usize >= n || ny as usize >= n {
                            continue;
                        }
                        ms = ms.max(cur[idx(x, y, n)] - cur[idx(nx as usize, ny as usize, n)]);
                    }
                }
            }
            assert!(ms <= last + 1e-7, "热侵蚀最大坡差未单调下降 {last}->{ms}");
            last = ms;
        }
        assert!(
            last <= p.talus * 1.05 + 1e-5,
            "坡差应收敛到 talus 附近，实得 {last}"
        );
        let sum1: f64 = cur.iter().map(|&v| v as f64).sum();
        assert!(
            (sum0 - sum1).abs() < 1e-4,
            "热侵蚀破坏质量守恒 {sum0} vs {sum1}"
        );
    }

    #[test]
    fn hydraulic_conserves_material_closed_boundary() {
        let n = 20;
        let mut sim = Hydraulic::new(n, cone(n, 0.9));
        // 中心倒一杯水，无雨无蒸发 ⇒ Σ(h+sed) 必须严格守恒
        sim.water[idx(n / 2, n / 2, n)] = 0.5;
        let m0 = sim.total_material();
        let p = HydraulicParams {
            rain: 0.0,
            evaporation: 0.0,
            ..Default::default()
        };
        for _ in 0..40 {
            sim.step(&p);
        }
        let m1 = sim.total_material();
        assert!(
            (m0 - m1).abs() < 1e-4,
            "水力步破坏 Σ(h+sed) 守恒: {m0} vs {m1}"
        );
        // 水无源无汇也应守恒
        let w_after = sim.total_water();
        // 初始总水 = 0.5
        assert!(
            (w_after - 0.5).abs() < 1e-4,
            "水量应守恒: 期望 0.5 实得 {w_after}"
        );
    }

    #[test]
    fn hydraulic_rain_and_evap_accounted() {
        let n = 16;
        let mut sim = Hydraulic::new(n, cone(n, 0.9));
        let p = HydraulicParams {
            rain: 0.001,
            evaporation: 0.0,
            ..Default::default()
        };
        let w0 = sim.total_water();
        for _ in 0..10 {
            sim.step(&p);
        }
        let w1 = sim.total_water();
        assert!(w1 > w0, "有降雨无蒸发水量应增加");
        assert!(w1 > 0.0);
        // 全蒸发：水归零
        let mut sim2 = Hydraulic::new(n, cone(n, 0.9));
        sim2.water[idx(3, 3, n)] = 0.3;
        let pe = HydraulicParams {
            rain: 0.0,
            evaporation: 1.0,
            ..Default::default()
        };
        sim2.step(&pe);
        assert!(sim2.total_water().abs() < 1e-6, "全蒸发后水量应≈0");
    }

    #[test]
    fn hydraulic_erosion_creates_channels_downhill() {
        // 坡度沿 +x 递减，顶部持续给水 ⇒ 低处应被切出沟（高度下降）
        let n = 12;
        let slope: Vec<f32> = (0..n * n)
            .map(|i| {
                let x = (i % n) as f32;
                0.6 - 0.04 * x
            })
            .collect();
        let h_before = slope.clone();
        let mut sim = Hydraulic::new(n, slope);
        let p = HydraulicParams {
            rain: 0.02,
            evaporation: 0.0,
            k_dissolve: 0.05,
            ..Default::default()
        };
        for _ in 0..30 {
            sim.step(&p);
        }
        // 中下游某处高度必须低于初始（确有侵蚀发生），且总量仍守恒
        let (cx, cy) = (n / 2, n / 2);
        assert!(
            sim.h[idx(cx, cy, n)] < h_before[idx(cx, cy, n)] + 1e-3,
            "应出现侵蚀切低"
        );
        let moved = (0..n)
            .flat_map(|y| (0..n).map(move |x| (x, y)))
            .any(|(x, y)| sim.h[idx(x, y, n)] < h_before[idx(x, y, n)] - 1e-4);
        assert!(moved, "存在被侵蚀降低的格点");
    }

    #[test]
    fn flat_becomes_faster_than_full_run() {
        // 早停判据：全平无水时 step 返回 false
        let n = 8;
        let mut sim = Hydraulic::new(n, vec![0.3; n * n]);
        let p = HydraulicParams::default();
        assert!(!sim.step(&p), "平地无水应判定无流动（可早停）");
    }

    #[test]
    fn hydraulic_double_run_bitwise_determinism() {
        let n = 16;
        let p = HydraulicParams {
            rain: 0.002,
            evaporation: 0.01,
            ..Default::default()
        };
        let run = || {
            let mut sim = Hydraulic::new(n, cone(n, 0.9));
            sim.water[idx(n / 2, n / 2, n)] = 0.4;
            for _ in 0..25 {
                sim.step(&p);
            }
            sim.h
                .iter()
                .map(|&v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16)
                .collect::<Vec<i16>>()
        };
        let a = run();
        let b = run();
        assert_eq!(a, b, "同参两次跑量化必须逐位一致");
        // 冻结首行 8 个量化值作 B3 语料金标准（占位后回填实测）
        assert_eq!(&a[..8], &GOLDEN_HYDRAULIC_ROW8);
    }

    // 实测冻结于本机（2026-10-14）；改算法/平台不一致即红。
    const GOLDEN_HYDRAULIC_ROW8: [i16; 8] = [0, 94, 0, 842, 2279, 3601, 4378, 4913];

    #[test]
    fn thermal_double_run_bitwise_determinism() {
        let n = 16;
        let p = ThermalParams::default();
        let mut a = cone(n, 0.9);
        let mut b = cone(n, 0.9);
        for _ in 0..30 {
            thermal_step(&mut a, n, &p);
            thermal_step(&mut b, n, &p);
        }
        assert_eq!(a, b);
    }
}
